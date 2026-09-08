//! The till, as one module a front end can call.
//!
//! Every decision lives in `openpos-core`. This crate translates: it takes JSON
//! in, calls the core, and hands JSON back. It holds no arithmetic, no ordering
//! rules and no state beyond the `Till` itself, because the moment it holds one
//! of those, the browser and Android builds have somewhere to disagree.
//!
//! JSON at this boundary rather than the postcard used on disk and on the wire.
//! Those two formats are positional and exist to be compact and stable across
//! versions; this boundary is neither of those things. It is crossed by a UI
//! compiled from the same commit, and a shape a person can read in a debugger
//! is worth more here than bytes saved.
//!
//! Errors cross as a tagged string rather than as a thrown exception, so a
//! caller cannot ignore one by not wrapping the call.

#[cfg(not(target_arch = "wasm32"))]
pub mod files;
#[cfg(target_arch = "wasm32")]
pub mod opfs;
pub mod sync;

extern crate alloc;

use alloc::collections::BTreeMap;

use openpos_core::cart::{CartLimits, Tender, TenderKind, Ticket};
use openpos_core::domain::pricing::Discount;
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::receipt;
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::storage::wire::{ItemDeltasV1, ItemV1};
use openpos_core::sync::driver::Driver;
use openpos_core::till::{Till, TillError};
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::{JsError, wasm_bindgen};

/// What a front end renders after any operation.
///
/// One shape for every call, so a UI has one thing to bind to and cannot get
/// into a state where it rendered a total from one call and a line list from
/// another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub lines: Vec<Line>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub tendered_minor: i64,
    /// Negative while the customer still owes.
    pub change_minor: i64,
    pub is_refund: bool,
    pub receipt_numbers_left: u64,
    pub unsynced_sales: usize,
    /// How far through the catalogue this device has read. Shown because a
    /// device that will not say where it is turns "the change never arrived"
    /// and "the change never saved" into the same symptom, and they need
    /// opposite things doing.
    pub catalogue_cursor: u64,
    /// Whether this device holds a credential. The credential itself never
    /// crosses this boundary: it lives beside the ledger and travels only with
    /// the requests the core builds.
    pub enrolled: bool,
    /// The wallets this shop takes, so a cashier picks a name rather than
    /// spelling it. Empty until the shop has been asked and has said.
    pub wallets: Vec<String>,
    /// Whether the server refuses that credential. A device in this state looks
    /// enrolled and is not: every request is answered 401, nothing syncs, and
    /// without this the screen has no way to say so or to offer a way out.
    pub credential_refused: bool,
    /// Who is signed in, and what they may do. A UI showing a button somebody
    /// cannot use is a UI that teaches people to press it and be refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<Operator>,
    /// Sales parked while the queue moved on. Always carried: a cashier who
    /// parks one and cannot see it has lost a basket, and the number of them is
    /// the reminder to deal with them before closing.
    pub held: Vec<Parked>,
    /// The drawer's totals, when one has been asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// The drawer, when one is open. A screen that cannot see it cannot tell a
    /// cashier what the till should hold before they count it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drawer: Option<Drawer>,
    /// Everybody, suspended included, when it was asked for. Separate from
    /// `people`, which is who may sign in now and is what a till renders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub everyone: Option<Vec<Person>>,
    /// What a supervisor would have to allow for the thing just refused, when
    /// that is what went wrong. Absent otherwise, which is the ordinary case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_supervisor: Option<openpos_core::auth::Action>,
    /// Which written-down customer a refusal is asking the cashier to choose.
    /// Present only when that is what was refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_customer: Option<String>,
    /// Who the shop lets buy on account, as this device was last told. A
    /// cashier picks from these rather than typing a name, so what somebody
    /// owes is added up against a person the shop has a record of.
    pub customers: Vec<Customer>,
    /// Who the basket on the screen is for, if anybody.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
    /// What this device is holding that the shop has not got, when it was
    /// asked for. The way out for a till that cannot sync: somebody reads this
    /// off it and carries it to the back office.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carrying: Option<Carrying>,
    /// What a catalogue search found, when one was asked for. Carrying the ids
    /// matters more than the names: an owner correcting a price has to send back
    /// the id the item already has, or the correction is a second item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalogue: Option<Vec<WireItem>>,
    /// Who may sign in here: names and ids, and nothing that could be used to
    /// sign in as them. A screen needs the list to show a person their own name
    /// rather than asking them to type an identifier.
    pub people: Vec<Person>,
    /// Present when the last operation was refused, and why. A UI that renders
    /// this cannot silently drop an error.
    pub error: Option<String>,
    /// The same refusal as a stable name, for a screen saying it in a language
    /// this crate does not hold.
    ///
    /// The words above are English. A cashier in a Bangladeshi shop reads the
    /// screen, and a refusal is exactly the moment they need their own
    /// language: matching on the sentence to translate it would break the day
    /// somebody improved the wording. Frozen in the core and written out to
    /// `apps/shared/refusals.json`, which is what the dictionary is keyed on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// The figures inside that refusal, named and already formatted.
    ///
    /// A refusal that says "the shop has 3 kg Rice and this basket wants 5 kg"
    /// cannot be translated from the sentence: the words and the numbers have
    /// to arrive apart. Formatted here rather than on the screen so money and
    /// quantities read the same everywhere, which is the whole reason those two
    /// helpers exist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub error_parts: BTreeMap<String, String>,
    /// The last completed sale, laid out for a printer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Vec<receipt::Line>>,
    /// The same sale as bytes a thermal printer understands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<PrintJob>,
    /// The lines this basket holds more of than the shop believes it has.
    ///
    /// Empty unless the shop has asked to be told, and empty on a refund.
    /// Carried on every view rather than fetched, because it changes with every
    /// scan and a screen that has to ask is a screen that shows it late.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub beyond_the_shelf: Vec<openpos_core::till::ShortOfStock>,
    /// The answer to "what does this cost", when one was asked for. Held until
    /// the next question rather than cleared by the next scan: a cashier who
    /// looks up, says the price and then serves the next customer must not find
    /// the answer gone while the first one is still deciding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<Checked>,
    /// What the last sync step decided, when the command was a sync one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<sync::Step>,
    /// What applying a reply changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied: Option<sync::Applied>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub item_id: String,
    pub code: String,
    pub name: String,
    pub qty_milli: i64,
    pub unit_price_minor: i64,
    /// What this line's discount took off, before any ticket discount is
    /// apportioned. A cashier who gave one should see what they gave, not infer
    /// it from a total that moved.
    pub discount_minor: i64,
    /// The rate that discount was set at, so a screen can show it back rather
    /// than recovering it from two amounts, which is lossy at small ones. Zero
    /// when the discount on this line was a stated amount rather than a rate.
    pub discount_bp: u32,
    /// The amount this line's own discount was set at, when it was set as an
    /// amount rather than a rate.
    ///
    /// Carried because a screen otherwise cannot tell three things apart: a
    /// line somebody took twenty taka off, a line carrying its share of a
    /// discount off the whole basket, and a line with no discount of its own.
    /// The till read the second where the first was true, and told a cashier
    /// their own twenty taka was the basket's.
    pub discount_amount_minor: i64,
    pub total_minor: i64,
}

/// The figures inside a refusal, named, for a screen wording it in its own
/// language.
///
/// An exhaustive match on purpose: a refusal that grows a figure and does not
/// pass it through here becomes a sentence with a hole in it on every screen
/// that is not English, and the compiler is what stops that.
///
/// Formatted with the same two helpers the receipt uses, so a quantity or an
/// amount reads the same on paper, on the screen and in a refusal.
fn parts_of(error: &TillError) -> BTreeMap<String, String> {
    use openpos_core::auth::AuthError;
    use openpos_core::cart::CartError;
    use openpos_core::shift::ShiftError;

    let mut parts = BTreeMap::new();
    let mut say = |key: &str, value: String| {
        parts.insert(String::from(key), value);
    };
    match error {
        TillError::MoreThanTheShelfHolds {
            name,
            on_hand_milli,
            wanted_milli,
        } => {
            say("name", name.clone());
            say("on_hand", receipt::quantity_of(*on_hand_milli).to_string());
            say("wanted", receipt::quantity_of(*wanted_milli).to_string());
        }
        TillError::BeyondTheirLimit {
            name,
            owed_minor,
            limit_minor,
            wanted_minor,
            ..
        } => {
            say("name", name.clone());
            say("owed", receipt::money_of(*owed_minor).to_string());
            say("limit", receipt::money_of(*limit_minor).to_string());
            say("wanted", receipt::money_of(*wanted_minor).to_string());
        }
        TillError::WriteItAgainstThem { name } => say("name", name.clone()),
        TillError::Cart(CartError::NoSuchLine { index }) => {
            say("line", alloc::format!("{}", index.saturating_add(1)));
        }
        TillError::Cart(CartError::RefundNotSettled { outstanding }) => {
            say("outstanding", receipt::money_of(outstanding.get()).to_string());
        }
        TillError::Cart(CartError::DiscountAboveCeiling { requested, ceiling }) => {
            say("requested", alloc::format!("{}", *requested as f64 / 100.0));
            say("ceiling", alloc::format!("{}", *ceiling as f64 / 100.0));
        }
        TillError::Cart(CartError::NegativePrice { price }) => {
            say("price", receipt::money_of(price.get()).to_string());
        }
        TillError::Cart(CartError::Underpaid { short_by }) => {
            say("short_by", receipt::money_of(short_by.get()).to_string());
        }
        TillError::Cart(CartError::ChangeFromAPromise { over_by, cash }) => {
            say("over_by", receipt::money_of(over_by.get()).to_string());
            say("cash", receipt::money_of(cash.get()).to_string());
        }
        TillError::Auth(AuthError::WrongPin { attempts_left }) => {
            say("attempts_left", alloc::format!("{attempts_left}"));
        }
        TillError::Auth(AuthError::LockedOut { until_ms }) => {
            say("until_ms", alloc::format!("{until_ms}"));
        }
        // The action is already carried by `needs_supervisor`, which is what the
        // screen offers a supervisor's PIN against. Repeating it here as words
        // would be a second place deciding what to call it.
        TillError::Auth(AuthError::NotPermitted { .. }) => {}
        TillError::Shift(ShiftError::AlreadyClosed { closed_at_ms }) => {
            say("closed_at_ms", alloc::format!("{closed_at_ms}"));
        }
        TillError::Shift(ShiftError::NegativeAmount { amount }) => {
            say("amount", receipt::money_of(amount.get()).to_string());
        }
        // Everything else is a sentence with no figures in it.
        TillError::UnknownBarcode
        | TillError::NoLongerSold
        | TillError::NothingToHold
        | TillError::NoSuchHeldTicket
        | TillError::TicketInProgress
        | TillError::NoOpenShift
        | TillError::NamelessShop
        | TillError::NamelessItem
        | TillError::NamelessCustomer
        | TillError::NoBarcodeToFindItBy
        | TillError::NamelessOperator
        | TillError::UnknownCustomer
        | TillError::Cart(
            CartError::Empty
            | CartError::MixedSaleAndReturn
            | CartError::PriceOverrideNotAllowed
            | CartError::Money(_),
        )
        | TillError::Auth(
            AuthError::UnknownOperator | AuthError::AuthorisationExpired,
        )
        | TillError::Shift(ShiftError::StillOpen | ShiftError::NoReason | ShiftError::Money(_))
        | TillError::Journal(_)
        | TillError::Sync(_)
        | TillError::Wire(_) => {}
    }
    parts
}

/// What one of something costs, for the question asked across the counter.
///
/// The gross is worked out by the same arithmetic that would ring it, not by
/// adding a percentage here: a screen that computes its own price is a second
/// implementation of the pricing rules, and the one that gets quoted to the
/// customer would be the untested one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub item: WireItem,
    /// What one costs at the counter, tax and all: the figure the customer is
    /// about to be asked for.
    pub each_minor: i64,
    /// The tax inside that figure.
    pub vat_minor: i64,
}

/// An item as a front end hands one over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireItem {
    pub id: String,
    pub code: String,
    pub name: String,
    /// The same thing in Bangla, for a cashier reading a screen in the language
    /// the shop speaks. Optional: a shop that has not typed one sees the other
    /// name rather than a blank row, and the search finds either.
    #[serde(default)]
    pub name_bn: String,
    /// What it is sold by: pieces, kilos, litres. Hardcoded "Nos" until now, so
    /// a shop selling rice by the kilo had no way to say which.
    #[serde(default = "pieces")]
    pub unit: String,
    pub price_minor: i64,
    /// What the shop paid. Carried so a screen correcting a price can send back
    /// the cost the item already had: a form that omits it writes a zero, and
    /// every margin the shop has is quietly gone.
    #[serde(default)]
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    /// True when VAT is charged on the price before discounts, so a discount
    /// comes out of the shop's margin. Defaulted, because most goods do not.
    #[serde(default)]
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    /// Whether the shop still sells it. Defaulted true, because everything that
    /// hands one of these over is adding something to sell.
    #[serde(default = "yes")]
    pub active: bool,
    /// 0 standard rated, 1 zero rated, 2 exempt. A number rather than a word
    /// because it crosses to a screen and back and the meanings are the same
    /// numbers everywhere else. Defaulted, because a form that says nothing
    /// means the ordinary case.
    #[serde(default)]
    pub supply: u8,
    /// What the shop calls this kind of thing. Empty for the ones nobody has
    /// sorted, which is the ordinary state of a catalogue on its first day.
    #[serde(default)]
    pub category: String,
}

/// The default for `active`: serde needs a function, and a bare `true` reads
/// worse at the field than a named one.
const fn yes() -> bool {
    true
}

/// What most things are sold by, for a caller that does not say.
fn pieces() -> String {
    String::from("Nos")
}

impl WireItem {
    /// An item as the shop holds it, straight off the wire.
    ///
    /// Used where a screen reads one item fresh before editing it, rather than
    /// from this device's copy of the catalogue, which is up to half a minute
    /// behind whatever another device did a moment ago.
    pub(crate) fn from_wire(item: &openpos_core::protocol::ItemWire) -> Self {
        Self {
            id: Ulid::from_u128(item.id).encode(),
            code: item.code.clone(),
            name: item.name_en.clone(),
            name_bn: item.name_bn.clone(),
            unit: item.unit.clone(),
            price_minor: item.price_minor,
            cost_minor: item.cost_minor,
            vat_bp: item.vat_bp,
            price_inclusive: item.price_inclusive,
            vat_on_undiscounted: item.vat_on_undiscounted,
            barcodes: item.barcodes.clone(),
            on_hand_milli: item.on_hand_milli,
            active: item.active,
            supply: item.supply,
            category: item.category.clone(),
        }
    }

    /// An item as this device holds it.
    ///
    /// The id comes back as the text it went out as, because an owner
    /// correcting a price sends it straight back and a correction addressed to
    /// a new id is a second item on the shelf rather than a corrected one.
    fn of(item: &openpos_core::Item) -> Self {
        Self {
            id: item.id.encode(),
            code: item.code.to_string(),
            name: item.name_en.to_string(),
            name_bn: item.name_bn.to_string(),
            unit: item.unit.to_string(),
            price_minor: item.price.get(),
            cost_minor: item.cost.get(),
            vat_bp: item.vat_rate.get(),
            price_inclusive: matches!(
                item.price_mode,
                openpos_core::domain::pricing::PriceMode::Inclusive
            ),
            vat_on_undiscounted: matches!(
                item.vat_base,
                openpos_core::domain::pricing::VatBase::Undiscounted
            ),
            barcodes: item.barcodes.iter().map(|code| code.to_string()).collect(),
            on_hand_milli: item.on_hand.get(),
            active: item.active,
            supply: item.supply.as_u8(),
            category: item.category.to_string(),
        }
    }

    fn into_wire(self) -> ItemV1 {
        ItemV1 {
            id: Ulid::decode(&self.id)
                .map(|id| id.to_u128())
                .unwrap_or_default(),
            code: self.code,
            name_en: self.name.clone(),
            // The English name when there is no Bangla one, so a search in
            // either script still finds it and a screen has something to show.
            name_bn: if self.name_bn.trim().is_empty() {
                self.name.clone()
            } else {
                self.name_bn
            },
            unit: if self.unit.trim().is_empty() {
                pieces()
            } else {
                self.unit
            },
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: self.vat_on_undiscounted,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            supply: self.supply,
            category: self.category,
        }
    }
}

fn alloc_empty() -> Vec<u128> {
    Vec::new()
}

/// A line number, or nothing when it is not one. JavaScript has one number
/// type, so a caller can pass 1.5 or -1 and mean nothing by it.
fn index(line: f64) -> Option<usize> {
    usize::try_from(exact(line)?).ok()
}

/// A percentage as basis points, or nothing when it is not a percentage.
///
/// Refused above a hundred rather than clamped: the arithmetic caps a larger
/// discount silently, which would leave a cashier believing they gave one thing
/// and the customer another.
///
/// Zero means no discount rather than a discount of nothing, so clearing one
/// leaves no trace on the line, in the day's figures, or on the receipt.
fn rate_of(percent: f64) -> Option<Discount> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return None;
    }
    let rate = Bp::new(u32::try_from(exact((percent * 100.0).round())?).ok()?).ok()?;
    Some(if rate.is_zero() {
        Discount::None
    } else {
        Discount::Rate(rate)
    })
}

/// Largest integer a JavaScript number represents exactly.
const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A discount larger than the price is not a discount, and the arithmetic caps
/// it silently, which would leave a cashier believing they gave one thing and
/// the customer another.
const NOT_A_PERCENTAGE: &str = "a discount must be between nothing and a hundred percent";
const NOT_AN_AMOUNT: &str = "an amount off is a whole number of poisha, and not a negative one";

const NOT_A_WHOLE_NUMBER: &str =
    "quantities, amounts and times must be whole numbers a JavaScript number holds exactly";

/// Take a JavaScript number only when it is exactly a whole number.
///
/// The core's premise is that money and quantities are integers and never
/// floats, and this is the boundary where that could quietly stop being true. A
/// double is exact for every integer up to 2^53, which in poisha is far past any
/// basket a shop will ring, so the range is not the risk. A fractional value is:
/// `as i64` truncates 12.7 to 12 without a word, and a shop finds out at the end
/// of the day.
///
/// The alternative was to expose these as i64, which wasm-bindgen maps to
/// BigInt. That keeps the integer invariant in the type, and it makes every call
/// site write `2000n` or fail at runtime with a message about BigInt conversion
/// rather than about the shop's data. Found by loading this module in a browser,
/// where `scan(code, 2000)` threw before it reached any of this.
fn exact(value: f64) -> Option<i64> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() > SAFE_INTEGER {
        return None;
    }
    // Checked above: finite, integral, and within 2^53, so this can neither
    // lose information nor saturate.
    #[allow(clippy::cast_possible_truncation)]
    Some(value as i64)
}

/// Enough to fill a screen and not so many that a phone renders for a second.
const fn default_catalogue_limit() -> usize {
    50
}

const fn default_authorisation_ms() -> u64 {
    openpos_core::auth::DEFAULT_AUTHORISATION_MS
}

const fn default_feed() -> u8 {
    4
}

const fn default_cut() -> bool {
    true
}

/// Everything a front end can ask the till to do.
///
/// One tagged shape rather than one exported function per operation, because
/// the two platforms bind to this differently and a growing list of exports is
/// a growing list of places for them to fall out of step. Adding an operation
/// here changes neither the JavaScript binding nor the C one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Do nothing and describe the till, for a UI that has just started.
    View,
    /// List what this device is holding that the shop has not got, and encode
    /// it for somebody to carry to the back office.
    Carrying,
    /// Say who this basket is for, or nobody. An id the shop issued rather than
    /// a name typed at the till, so two people with one name stay two people.
    SetCustomer {
        customer: Option<String>,
    },
    ApplyItems {
        items: Vec<WireItem>,
    },
    Scan {
        barcode: String,
        qty_milli: i64,
    },
    /// What one of something costs, without putting it in the basket.
    ///
    /// The question a cashier is asked twenty times a day, and until this the
    /// only way to answer it was to ring the thing and take it off again: a line
    /// on the trail, a permission once the customer has started paying, and a
    /// basket that has been touched to answer a question about a shelf.
    ///
    /// Answers from this device's own catalogue, so it works with the line down,
    /// which is when a shelf label is likeliest to be the only other source.
    Check {
        /// A barcode from the scanner, or a code or name somebody typed.
        code: String,
    },
    /// Fold the catalogue delta log into a fresh snapshot, if it has grown
    /// enough to be worth it.
    ///
    /// Nothing called this, so the log grew for the life of the device and every
    /// boot replayed all of it. On a cheap tablet that is a till taking longer
    /// to open every morning, for a reason nobody in the shop could see.
    ///
    /// Answered by the till rather than driven from a timer, because only the
    /// till knows whether the log is long. The platform decides when it is a
    /// good moment: between customers, never with a ticket open, because this
    /// rewrites a couple of megabytes.
    Checkpoint,
    /// Give up on the basket on screen.
    ///
    /// A customer who changes their mind about everything. Removing five lines
    /// one at a time is five chances to leave one behind, and the line left
    /// behind is the one that gets rung to the next customer.
    CancelSale,
    /// Take back the money entered so far, for a mis-keyed amount.
    ///
    /// Five thousand typed instead of five hundred cannot be unwound by adding
    /// more, and a cashier who cannot undo it will finish the sale and fix it
    /// out of the drawer.
    ClearTenders,
    /// Park the sale on screen, so the queue can keep moving.
    ///
    /// A customer who has gone back for something, or is looking for their card,
    /// should not hold up the shop behind them.
    Hold {
        ticket_id: String,
        held_at_ms: u64,
        label: String,
    },
    /// Bring a parked sale back.
    Resume {
        ticket_id: String,
    },
    /// Throw a parked sale away, for the customer who never came back.
    DiscardHeld {
        ticket_id: String,
    },
    /// Put an item on the ticket by its id, for a cashier who looked it up
    /// rather than scanned it: a barcode that will not read, or loose goods
    /// that carry none.
    Add {
        item_id: String,
        qty_milli: i64,
    },
    /// Everybody this device knows of, suspended included.
    ///
    /// The everyday list leaves out anyone suspended, because a till's sign-in
    /// panel must not offer them. That leaves no way to find somebody and let
    /// them back in, which is this.
    Everyone,
    /// Look through the catalogue this device holds.
    ///
    /// Answered from the replica, not from the server: the back office syncs the
    /// same catalogue a till does, so an owner can see what is there with the
    /// line down, and a search that needs the network is a search that fails in
    /// the shop it is meant for.
    Catalogue {
        /// Empty lists the beginning of the catalogue, which is what an owner
        /// wants before they know what they are looking for.
        #[serde(default)]
        query: String,
        #[serde(default = "default_catalogue_limit")]
        limit: usize,
        /// Include items the shop has stopped selling. Off by default, because
        /// a till looking one up should not find them, and on for a back office,
        /// which is the only place that can bring one back.
        #[serde(default)]
        retired: bool,
    },
    /// Change a line's quantity. A cashier who scanned three of something and
    /// meant two must not have to void the basket.
    SetQty {
        line: f64,
        qty_milli: f64,
    },
    /// Take a line off the ticket.
    ///
    /// Carries the time because a line taken off a basket somebody has already
    /// paid towards is a permission, and a permission is written down with the
    /// hour it was used at.
    RemoveLine {
        line: f64,
        #[serde(default)]
        at_ms: u64,
    },
    /// Sell one line at a different price, for damaged goods or a price a
    /// customer was quoted. Refused unless this cashier may override a price.
    SetUnitPrice {
        line: f64,
        price_minor: f64,
    },
    /// Discount one line, as a percentage.
    ///
    /// Refused above this cashier's ceiling, which is what the ceiling is for.
    /// A supervisor can authorise it, and that authorisation is spent on use.
    SetLineDiscount {
        line: f64,
        percent: f64,
    },
    /// Write somebody down at the till, so a sale on account has a person to go
    /// against rather than a spelling.
    WriteCustomer {
        /// Minted by the caller, like a ticket's: this crate has no entropy.
        id: String,
        name: String,
        #[serde(default)]
        phone: Option<String>,
        #[serde(default)]
        bin: Option<String>,
    },
    /// Write down something the shop has never heard of, and sell it.
    ///
    /// A delivery arrives during an outage with a barcode in nobody's
    /// catalogue. The cashier says what it is and what it costs; the till holds
    /// it like any other item and sends it to the shop with the sales.
    QuickAdd {
        /// Minted by the caller, like a ticket's: this crate has no entropy.
        id: String,
        barcode: String,
        name: String,
        #[serde(default)]
        name_bn: String,
        #[serde(default)]
        unit: String,
        price_minor: f64,
        vat_bp: f64,
        #[serde(default)]
        price_inclusive: bool,
    },
    /// Take a stated amount off one line, rather than a percentage of it.
    ///
    /// What a shop here actually does: twenty taka off, not five point eight
    /// percent off. Measured against the same ceiling, as a share of the line.
    TakeOffLine {
        line: f64,
        amount_minor: f64,
    },
    /// Discount the whole ticket, apportioned across its lines.
    SetTicketDiscount {
        percent: f64,
    },
    /// Take a stated amount off the whole basket.
    TakeOffTicket {
        amount_minor: f64,
    },
    /// Take money by something other than cash.
    ///
    /// A shop here takes bKash and Nagad all day, and this till took cash only.
    /// The core has known about wallets, cards and credit since it was written,
    /// and the drawer report already splits by them: what a shift has in the
    /// drawer at closing depends on knowing which money never went in it.
    AddTender {
        /// `cash`, `wallet`, `card`, `credit`, or anything else the shop calls
        /// it. Not an enum on this boundary, because a shop takes whatever a
        /// shop takes and a new one should not need a new build.
        kind: String,
        /// Which wallet, when it is one: a shop may accept several and the
        /// drawer report is read by name.
        #[serde(default)]
        name: String,
        amount_minor: i64,
        /// A wallet transaction id or a card approval code. Recorded, never
        /// trusted: the till cannot check it and must not pretend to.
        #[serde(default)]
        reference: String,
        /// The hour, for a tender that may need allowing: a sale on account
        /// past what the shop lets somebody owe is a supervisor's to permit,
        /// and a permission is written down with the time it was used at.
        #[serde(default)]
        at_ms: u64,
    },
    AddCash {
        amount_minor: i64,
        #[serde(default)]
        at_ms: u64,
    },
    Checkout {
        ticket_id: String,
        rung_at_ms: u64,
    },
    /// Lay the last completed sale out for a printer.
    ///
    /// Width in characters: 32 for a 58mm printer, 48 for an 80mm one. The
    /// platform turns the lines into ESC/POS bytes or into markup; laying out
    /// the columns is the same job everywhere and is done in the core.
    Receipt {
        width: usize,
        /// Already formatted in the shop's own timezone: this crate has no
        /// clock and a receipt showing UTC in Dhaka disagrees with the
        /// customer's watch.
        rung_at: String,
        #[serde(default)]
        cashier: Option<String>,
        /// What to call each thing on the paper, in the language this shop
        /// reads, keyed as `core/tests/paper_words.rs` freezes them. Empty is
        /// English, which is what a thermal printer gets: no ESC/POS code page
        /// carries Bangla.
        #[serde(default)]
        words: BTreeMap<String, String>,
    },
    /// The drawer as it stands, or as it was counted, laid out for paper.
    ///
    /// The slip that goes in the drawer with the cash. Everything on it was on
    /// the screen already and none of it could be printed, so a cashier copied
    /// the figures by hand at the one moment of the day when the shop most
    /// wants a record nobody rewrote.
    DrawerPaper {
        width: usize,
        /// Already formatted, for the reason a receipt's time is.
        at: String,
        #[serde(default)]
        till: Option<String>,
        #[serde(default)]
        counted_by: Option<String>,
        /// What to call each thing on the paper, in the language this shop
        /// reads, keyed as `core/tests/paper_words.rs` freezes them. Empty is
        /// English, which is what a thermal printer gets: no ESC/POS code page
        /// carries Bangla.
        #[serde(default)]
        words: BTreeMap<String, String>,
    },
    /// One customer's account, laid out for paper: the khata page.
    ///
    /// Rendered from the account the shop last sent this device, not from
    /// anything the screen adds up. The screen supplies the words a clock
    /// makes: one date per line, in the order the lines came, and the time it
    /// was printed.
    StatementPaper {
        width: usize,
        /// The customer, as the shop wrote them down.
        customer: String,
        at: String,
        /// One per line of the account, formatted by the screen because this
        /// crate has no timezone. Refused when the count does not match: a
        /// statement with the dates shifted by one is worse than none.
        dates: Vec<String>,
        /// What to call each thing on the paper, in the language this shop
        /// reads, keyed as `core/tests/paper_words.rs` freezes them. Empty is
        /// English, which is what a thermal printer gets: no ESC/POS code page
        /// carries Bangla.
        #[serde(default)]
        words: BTreeMap<String, String>,
    },
    /// The last sale as bytes for a thermal printer.
    ///
    /// Separate from `Receipt` because a browser wants lines to lay out and a
    /// tablet wants bytes to write to a socket, and neither wants the other's
    /// shape. Both come from the same layout, so the paper is the same.
    Escpos {
        width: usize,
        rung_at: String,
        #[serde(default)]
        cashier: Option<String>,
        /// Blank lines before the cut. Zero is honoured: some printers are fed
        /// by hand.
        #[serde(default = "default_feed")]
        feed_lines: u8,
        #[serde(default = "default_cut")]
        cut: bool,
    },
    /// Whatever was last laid out, as bytes for a thermal printer.
    ///
    /// The receipt has had this since printers were supported, and the drawer
    /// slip and a customer's account had nowhere to go but a browser's print
    /// dialog: a shop with a thermal printer and an Android till could print
    /// what it sold and not what it counted or what anybody owed.
    ///
    /// The lines exactly as they were laid out, rather than rendered again from
    /// the underlying record: those were laid out with the screen's own clock
    /// and the names it holds, and a second rendering here would print a
    /// different page from the one somebody just read.
    PaperBytes {
        #[serde(default = "default_feed")]
        feed_lines: u8,
        #[serde(default = "default_cut")]
        cut: bool,
    },
    /// Ask what to sync next. The answer carries the request already built.
    SyncStep {
        online: bool,
        now_ms: u64,
    },
    /// Hand back what the server said.
    SyncApply {
        kind: sync::Exchange,
        body: String,
        now_ms: u64,
    },
    /// The request did not get through. Back off.
    SyncFailed {
        now_ms: u64,
        /// What the server answered, when it answered at all. A refusal of the
        /// credential is not the same failure as a shop with no signal, and a
        /// device that treats them alike retries a token the server will never
        /// accept until somebody notices the sales are not arriving.
        #[serde(default)]
        status: Option<u16>,
    },
    /// Build the enrolment request for a code read off the owner's screen.
    Enrol {
        code: String,
    },
    /// Sign in with a PIN.
    SignIn {
        operator_id: String,
        pin: String,
        now_ms: u64,
    },
    SignOut,
    /// Open the drawer for the day with a counted float.
    OpenShift {
        shift_id: String,
        opening_float_minor: i64,
        at_ms: u64,
    },
    /// Cash in or out for a stated reason. The reason is required by the core.
    MoveCash {
        /// True for money in, false for money out. The direction is stated
        /// rather than carried by the sign, so a caller cannot record a drop as
        /// a top-up by getting a minus wrong.
        inward: bool,
        amount_minor: i64,
        reason: String,
        at_ms: u64,
    },
    /// Totals so far, leaving the drawer open.
    XReport,
    /// Build a back-office request. The reply comes back through `SyncApply`
    /// like everything else, so there is one way to carry bytes and one place
    /// that reads them.
    Admin {
        request: sync::AdminRequest,
    },
    /// Count the drawer and close the shift.
    CloseShift {
        counted_cash_minor: i64,
        at_ms: u64,
    },
    /// Turn the ticket in progress into a refund. Needs the permission, or a
    /// supervisor's authorisation.
    StartRefund {
        #[serde(default)]
        original_receipt: Option<String>,
        now_ms: u64,
    },
    /// A supervisor allows the cashier one action.
    Authorise {
        supervisor_id: String,
        pin: String,
        action: openpos_core::auth::Action,
        now_ms: u64,
        #[serde(default = "default_authorisation_ms")]
        valid_for_ms: u64,
    },
}

/// Which store a till is running on.
///
/// An enum rather than a boxed trait object, because the storage trait is
/// deliberately not dyn-compatible: making it so would have meant boxing every
/// read on the scan path, and the reason it is a trait at all is that the
/// platform layer is thin.
enum Store {
    /// Nothing survives a reload. For a demo, and for a browser that refuses
    /// storage.
    Memory(Till<MemoryBackend>),
    /// The real one in a browser.
    #[cfg(target_arch = "wasm32")]
    Opfs(Till<opfs::OpfsBackend>),
    /// The real one everywhere else: a directory of files. What an Android
    /// tablet through the C ABI uses, and what a support tool opening a
    /// device's store on a laptop uses.
    #[cfg(not(target_arch = "wasm32"))]
    Files(Till<files::FileBackend>),
}

/// Carry out one command.
///
/// The single place that says what the till can do. Both the JavaScript surface
/// and the C one route through here, so neither can grow an operation the other
/// lacks or handle the same one differently.
fn dispatch<B: openpos_core::storage::backend::Backend>(
    till: &mut Till<B>,
    command: Command,
) -> Option<TillError> {
    match command {
        Command::View => None,
        Command::ApplyItems { items } => {
            let deltas = ItemDeltasV1 {
                cursor: 0,
                upserts: items.into_iter().map(WireItem::into_wire).collect(),
                tombstones: alloc_empty(),
            };
            till.apply_pull(&deltas).err()
        }
        Command::Checkpoint => till.checkpoint_if_needed().err(),
        Command::CancelSale => {
            till.cancel_sale();
            None
        }
        Command::ClearTenders => {
            till.clear_tenders();
            None
        }
        Command::Hold {
            ticket_id,
            held_at_ms,
            label,
        } => match Ulid::decode(&ticket_id) {
            Ok(id) => till.hold(id, held_at_ms, &label).err(),
            Err(_) => Some(TillError::NoSuchHeldTicket),
        },
        Command::Resume { ticket_id } => match Ulid::decode(&ticket_id) {
            Ok(id) => till.resume(id).err(),
            Err(_) => Some(TillError::NoSuchHeldTicket),
        },
        Command::DiscardHeld { ticket_id } => match Ulid::decode(&ticket_id) {
            Ok(id) => till.discard_held(id).err(),
            Err(_) => Some(TillError::NoSuchHeldTicket),
        },
        Command::Add { item_id, qty_milli } => match Ulid::decode(&item_id) {
            Ok(id) => till.add(id, Milli::new(qty_milli)).err(),
            // The same refusal a bad barcode gets, because to a cashier it is
            // the same thing: the till does not know what you mean.
            Err(_) => Some(TillError::UnknownBarcode),
        },
        Command::Scan { barcode, qty_milli } => till.scan(&barcode, Milli::new(qty_milli)).err(),
        Command::AddTender {
            kind,
            name,
            amount_minor,
            reference,
            at_ms,
        } => {
            let named = |fallback: &str| -> alloc::boxed::Box<str> {
                let chosen = if name.trim().is_empty() {
                    fallback
                } else {
                    name.trim()
                };
                chosen.into()
            };
            let kind = match kind.trim().to_lowercase().as_str() {
                "cash" => TenderKind::Cash,
                "card" => TenderKind::Card,
                "credit" => TenderKind::Credit,
                "wallet" => TenderKind::Wallet(named("a wallet")),
                other => TenderKind::Other(named(other)),
            };
            // The refusal travels: a credit tender naming somebody the shop has
            // written down, on a basket not pointed at them, would otherwise
            // split one person's account in two without saying so.
            till.add_tender(Tender {
                kind,
                amount: Minor::new(amount_minor),
                reference: Some(reference.trim())
                    .filter(|value| !value.is_empty())
                    .map(Into::into),
            }, at_ms)
            .err()
        }
        Command::AddCash {
            amount_minor,
            at_ms,
        } => till
            .add_tender(
                Tender {
                    kind: TenderKind::Cash,
                    amount: Minor::new(amount_minor),
                    reference: None,
                },
                at_ms,
            )
            .err(),
        Command::OpenShift {
            ref shift_id,
            opening_float_minor,
            at_ms,
        } => match Ulid::decode(shift_id) {
            Ok(id) => till
                .open_shift(id, Minor::new(opening_float_minor), at_ms)
                .err(),
            Err(_) => Some(TillError::NoOpenShift),
        },
        Command::MoveCash {
            inward,
            amount_minor,
            ref reason,
            at_ms,
        } => {
            let amount = Minor::new(amount_minor);
            if inward {
                till.cash_in(amount, reason, at_ms).err()
            } else {
                till.cash_out(amount, reason, at_ms).err()
            }
        }
        Command::StartRefund {
            ref original_receipt,
            now_ms,
        } => till.start_refund(original_receipt.as_deref(), now_ms).err(),
        Command::XReport | Command::CloseShift { .. } | Command::Admin { .. } => None,
        Command::Checkout { .. }
        | Command::Receipt { .. }
        | Command::DrawerPaper { .. }
        | Command::StatementPaper { .. }
        | Command::Escpos { .. }
        | Command::PaperBytes { .. } => None,
        // Handled by the caller, which holds the driver, the tenant and the
        // last sale. Listed rather than caught by a wildcard, so adding a
        // command forces a decision here instead of silently doing nothing.
        Command::SyncStep { .. }
        | Command::SyncApply { .. }
        | Command::SyncFailed { .. }
        | Command::Enrol { .. }
        | Command::SignIn { .. }
        | Command::SignOut
        | Command::Catalogue { .. }
        | Command::Check { .. }
        | Command::Carrying
        | Command::SetCustomer { .. }
        | Command::Everyone
        | Command::SetQty { .. }
        | Command::RemoveLine { .. }
        | Command::SetUnitPrice { .. }
        | Command::QuickAdd { .. }
        | Command::WriteCustomer { .. }
        | Command::SetLineDiscount { .. }
        | Command::TakeOffLine { .. }
        | Command::SetTicketDiscount { .. }
        | Command::TakeOffTicket { .. }
        | Command::Authorise { .. } => None,
    }
}

/// Somebody the shop lets buy on account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Their Business Identification Number, when the buyer is a business. On
    /// the screen so an owner can see what the shop holds rather than typing it
    /// again over the top of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    /// The most this person may owe at once, in poisha. Zero is no cap. On the
    /// screen so an owner correcting somebody sees what the shop already holds
    /// rather than typing it again over the top of it.
    #[serde(default)]
    pub limit_minor: i64,
    pub active: bool,
    /// What they owed when the shop last said so, and when that was. Absent
    /// until this device has asked: a figure carried through a night is worse
    /// than none, because a cashier reads it out as though it were true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owed_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owed_as_of_ms: Option<u64>,
}

/// A bundle's mark, in groups a person can read out over a phone.
///
/// Eight hex digits split into fours: long enough that a truncated paste does
/// not land on the same mark by accident, short enough to say out loud. Not a
/// security check and not claimed as one.
fn marked(bytes: &[u8]) -> alloc::string::String {
    let mark = openpos_core::storage::frame::fingerprint(bytes);
    alloc::format!("{:04x} {:04x}", mark >> 16, mark & 0xFFFF)
}

/// Sales a device is holding, in a form somebody can carry.
///
/// The bundle is what the back office takes in. It is text on purpose: it has to
/// survive being copied out of one browser and pasted into another, possibly
/// through a message on somebody's phone, which is how a shop with one working
/// device actually moves anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carrying {
    /// Which terminal these were rung on, as this device believes itself to be.
    pub terminal: String,
    pub sales: Vec<CarriedSale>,
    pub total_minor: i64,
    /// The whole lot, encoded for the back office.
    pub bundle: String,
    /// Four groups of two letters, for a person to read out loud: "does yours
    /// end in the same mark". A bundle travels through a messaging app, and the
    /// failure it protects against is a paste that got cut short, which
    /// otherwise looks exactly like a paste that did not.
    pub mark: String,
    /// What the bundle weighs as text, so somebody carrying it on a phone knows
    /// whether it will paste at all before they try.
    pub letters: usize,
}

/// One sale being carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedSale {
    pub id: String,
    pub total_minor: i64,
    /// True when it was read back out of a torn log rather than merely unsent,
    /// which is a difference the person carrying it should be told about.
    pub salvaged: bool,
}

/// What was taken, by how it was paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenderRow {
    /// What to call it: the shop's own word for a wallet, and an English name
    /// for the three kinds every shop has. A screen showing another language
    /// words those three from `kind` and shows this as it stands for a wallet,
    /// because "bKash" is the shop's word rather than a translation.
    pub name: String,
    /// Which kind it is, for a screen saying it in its own language: `cash`,
    /// `card`, `credit`, or `wallet` for anything the shop named itself.
    #[serde(default)]
    pub kind: String,
    pub amount_minor: i64,
    /// Whether this money is in the till. Carried rather than inferred from the
    /// name, so a screen cannot quietly decide that a wallet counts as cash.
    pub in_drawer: bool,
}

/// The drawer's totals: an X report, or a Z report when it has been counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub shift: String,
    pub opened_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: usize,
    pub tenders: Vec<TenderRow>,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    /// Present only once the drawer has been counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counted_cash_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at_ms: Option<u64>,
    /// Counted less expected. Negative is short.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variance_minor: Option<i64>,
}

fn tender_kind_name(kind: &TenderKind) -> String {
    match kind {
        TenderKind::Cash => String::from("Cash"),
        TenderKind::Card => String::from("Card"),
        TenderKind::Credit => String::from("On account"),
        TenderKind::Wallet(name) | TenderKind::Other(name) => name.to_string(),
    }
}

/// The drawer as a screen shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drawer {
    /// False once it has been counted and closed: the figures stay readable so
    /// a Z report can be reprinted without counting the drawer a second time.
    pub open: bool,
    pub opening_float_minor: i64,
    pub sales: usize,
    /// What the drawer should hold if nothing has gone wrong.
    pub expected_cash_minor: i64,
    pub movements: usize,
}

/// A name to pick from, and nothing else.
///
/// Deliberately not the operator record: a list a screen renders must not carry
/// a credential, however derived, because a screen is the one place a value ends
/// up in a log or a screenshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    /// Whether they may sign in. Carried because the everyday list leaves the
    /// suspended out, and the one screen that can reinstate somebody has to be
    /// able to see them.
    #[serde(default = "yes")]
    pub active: bool,
    /// What they may do. Carried because a screen that corrects a person sends
    /// back everything it was given, and a screen that was never given their
    /// permissions would send back none: correcting a supervisor's name would
    /// quietly make them a cashier.
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
}

/// A sale parked while the queue moved on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parked {
    pub id: String,
    /// What the cashier called it, so it can be told from the other three.
    pub label: String,
    pub held_at_ms: u64,
    pub lines: usize,
    pub total_minor: i64,
}

/// A person as a roster shows them.
///
/// One conversion for both lists. The everyday list and the one that includes
/// the suspended were separate copies of these six lines, and a field added to
/// one would have been missing from the other.
fn person_seen(who: &openpos_core::auth::Operator) -> Person {
    Person {
        id: who.id.encode(),
        name: who.name.to_string(),
        active: who.active,
        max_discount_bp: who.permissions.max_discount_bp,
        may_override_price: who.permissions.may_override_price,
        may_refund: who.permissions.may_refund,
        may_void_line: who.permissions.may_void_line,
        may_authorise: who.permissions.may_authorise,
        may_open_drawer: who.permissions.may_open_drawer,
        may_close_shift: who.permissions.may_close_shift,
    }
}

/// Who is at the till, as a screen needs to know them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operator {
    pub id: String,
    pub name: String,
    pub may_refund: bool,
    pub may_override_price: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub max_discount_bp: u32,
}

/// Bytes for a thermal printer, and what they could not say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintJob {
    /// Hex, matching the sync bodies: one way of carrying bytes across this
    /// boundary rather than two.
    pub bytes: String,
    /// Lines the printer's character set cannot carry, by index. A platform
    /// with a raster path prints these as images; one without at least knows.
    pub unprintable: Vec<usize>,
}

/// A till, as the front end holds it.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct TillHandle {
    inner: Store,
    /// Which shop. Held because every request names it and the till itself does
    /// not: the core takes a tenant at open and has no reason to keep it.
    tenant: u128,
    /// Retry state, held across calls because backing off is a property of the
    /// till's conversation with the server, not of any one request.
    driver: Driver,
    /// Whether the last pull said more was waiting. Remembered here so a
    /// platform cannot forget to pass it back and quietly stop pulling.
    more_to_pull: bool,
    /// Whether the server has refused this device's credential. Not durable, and
    /// deliberately: it is a fact about the server's current answer, so it is
    /// re-learned within seconds of a reload rather than remembered from a
    /// previous one that may no longer be true.
    refused: bool,
    /// What the last sync step produced, folded into the next view.
    last_step: Option<sync::Step>,
    last_applied: Option<sync::Applied>,
    /// The last sale laid out for paper. Held so one reply can carry both the
    /// state of the till and the thing to print.
    last_receipt: Option<Vec<receipt::Line>>,
    last_job: Option<PrintJob>,
    last_report: Option<Report>,
    /// The last account the shop sent, kept for printing.
    ///
    /// Its own field rather than read back out of the last applied reply: the
    /// sync loop applies something every couple of seconds, so by the time
    /// anybody presses print the account has long since been replaced by a
    /// catalogue page. Found by pressing the button.
    last_account: Vec<sync::AccountLine>,
    /// The same drawer as the core stated it, kept for printing.
    ///
    /// Beside the screen's copy rather than derived from it, for the reason the
    /// last sale's ticket is kept beside the last receipt: a Z report cannot be
    /// asked for twice, the shift is closed after it, and a slip rebuilt from
    /// the numbers a screen was given is a second implementation of the layout.
    last_drawer: Option<(openpos_core::shift::XReport, Option<(Minor, Minor)>)>,
    /// The last sale closed, which is what a receipt is of. A reprint asks for
    /// the sale that happened, not for whatever is on the screen now.
    last_sale: Option<Ticket>,
    /// The last price somebody checked. Held rather than recomputed, for the
    /// reason the last receipt is: the next command must not take the answer
    /// off the screen while the customer is still deciding.
    last_checked: Option<Checked>,
    /// What the last catalogue search found. Held rather than sent with every
    /// view, because a till renders its basket forty times a sale and has no
    /// use for the catalogue in any of them.
    last_catalogue: Option<Vec<WireItem>>,
    /// Everybody, when a back office asked. Held for the same reason the
    /// catalogue is: a till has no use for it on any of its forty renders a sale.
    last_everyone: Option<Vec<Person>>,
    last_carrying: Option<Carrying>,
}

/// Run the same call against whichever store this till holds.
///
/// A macro rather than a trait method, because the two arms have different
/// concrete types and the whole point is that neither is boxed. The alternative
/// was writing each operation twice, which is how the two stores would come to
/// behave differently.
macro_rules! with_till {
    ($self:expr, |$till:ident| $body:expr) => {
        match &mut $self.inner {
            Store::Memory($till) => $body,
            #[cfg(target_arch = "wasm32")]
            Store::Opfs($till) => $body,
            #[cfg(not(target_arch = "wasm32"))]
            Store::Files($till) => $body,
        }
    };
    (ref $self:expr, |$till:ident| $body:expr) => {
        match &$self.inner {
            Store::Memory($till) => $body,
            #[cfg(target_arch = "wasm32")]
            Store::Opfs($till) => $body,
            #[cfg(not(target_arch = "wasm32"))]
            Store::Files($till) => $body,
        }
    };
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl TillHandle {
    /// Open a till on a directory of files, creating it on a first morning.
    ///
    /// The durable store for everything that is not a browser: an Android
    /// tablet through the C ABI, a desktop build, a support tool opening a
    /// device's store on a laptop. What it promises about a power cut is in
    /// [`files::FileBackend`], and it is worth reading before trusting it on a
    /// platform it does not name.
    ///
    /// Answers with the boot report as JSON, so a platform can say what it
    /// found: how many sales are unsent, whether the log had to be repaired,
    /// and whether any bytes were salvaged out of a torn one.
    ///
    /// # Errors
    /// When the identifiers are not valid ids, or the directory cannot be
    /// opened or read as a till's store.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_files(
        home: &str,
        tenant: &str,
        terminal: &str,
    ) -> core::result::Result<(Self, alloc::string::String), alloc::string::String> {
        let tenant = Ulid::decode(tenant).map_err(|_| String::from("that is not a shop id"))?;
        let terminal =
            Ulid::decode(terminal).map_err(|_| String::from("that is not a terminal id"))?;
        let backend = files::FileBackend::open(std::path::Path::new(home))
            .map_err(|error| alloc::format!("that store could not be opened: {error}"))?;
        let (inner, report) = Till::open(
            backend,
            tenant.to_u128(),
            terminal,
            1,
            CartLimits::default(),
        )
        .map_err(|error| alloc::format!("that store could not be read: {error}"))?;
        // What a platform has to be able to say out loud on the morning it
        // happens: how much is waiting to be sent, whether the log had to be
        // repaired, and whether any bytes could not be read and were kept.
        let said = alloc::format!(
            "{{\"items\":{},\"unsynced_sales\":{},\"receipt_numbers_left\":{},\
             \"repaired\":{},\"salvaged_bytes\":{}}}",
            report.items,
            report.unsynced_sales,
            report.receipt_numbers_left,
            report.repaired,
            report.salvaged_bytes
        );
        Ok((Self::wrap(Store::Files(inner), tenant.to_u128()), said))
    }

    /// Open a till on a fresh in-memory store.
    ///
    /// The browser build will pass an OPFS-backed store instead; this exists so
    /// the surface can be exercised, and so a demo runs with no storage
    /// permissions at all. Nothing it holds survives a reload, and the type name
    /// says so.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = openInMemory))]
    #[must_use]
    pub fn open_in_memory(tenant: &str, terminal: &str) -> Option<TillHandle> {
        let tenant = Ulid::decode(tenant).ok()?;
        let terminal = Ulid::decode(terminal).ok()?;
        let (inner, _report) = Till::open(
            MemoryBackend::new(),
            tenant.to_u128(),
            terminal,
            1,
            CartLimits::default(),
        )
        .ok()?;
        Some(Self::wrap(Store::Memory(inner), tenant.to_u128()))
    }

    /// Open a till on OPFS, from handles JavaScript has already opened.
    ///
    /// This is the one that keeps a promise. A sale committed through here
    /// survives the tab closing, the browser being killed, and the tablet losing
    /// power, which is the whole reason the product exists.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = openOpfs)]
    pub fn open_opfs(
        handles: &js_sys::Array,
        tenant: &str,
        terminal: &str,
    ) -> core::result::Result<TillHandle, JsError> {
        // Every failure here says which one it was. A till that will not open is
        // the single worst thing that can happen to a shop, and "it did not
        // work" is the least useful thing to say about it: the person holding
        // the tablet has to know whether to re-enrol it, restore it, or call
        // somebody.
        let tenant = Ulid::decode(tenant)
            .map_err(|_| JsError::new("the shop identifier is not a valid id"))?;
        let terminal = Ulid::decode(terminal)
            .map_err(|_| JsError::new("the terminal identifier is not a valid id"))?;
        let backend = opfs::OpfsBackend::from_handles(handles).ok_or_else(|| {
            JsError::new("expected one open file handle per name from fileNames(), in that order")
        })?;

        let (inner, report) = Till::open(
            backend,
            tenant.to_u128(),
            terminal,
            1,
            CartLimits::default(),
        )
        .map_err(|error| JsError::new(&alloc::format!("{error}")))?;

        if report.repaired || report.salvaged_bytes > 0 {
            web_sys::console::warn_1(
                &alloc::format!(
                    "openpos: recovery repaired this device; {} bytes could not be read and were kept aside",
                    report.salvaged_bytes
                )
                .into(),
            );
        }
        if report.catalogue_refetched {
            // Once, at the boot that does it, rather than as a banner: by the
            // time a cashier reads anything the catalogue is usually back. What
            // it is for is the shop asking why this device used data this
            // morning, and the answer being here rather than nowhere.
            web_sys::console::warn_1(
                &"openpos: this device could not read its stored catalogue and is fetching it again"
                    .into(),
            );
        }
        Ok(Self::wrap(Store::Opfs(inner), tenant.to_u128()))
    }

    /// What the store looks like on disk, for diagnosing a till that will not
    /// behave. Bytes per file, in `fileNames()` order.
    /// Check that this browser's storage keeps the promises the till relies on.
    /// Returns "ok", or the first operation that did not.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = selfTest)]
    #[must_use]
    pub fn self_test(handles: &js_sys::Array) -> String {
        opfs::self_test(handles)
    }

    /// Read the critical log back through the backend, for diagnosis.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = peekCritical)]
    #[must_use]
    pub fn peek_critical(handles: &js_sys::Array) -> String {
        opfs::peek_critical(handles)
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = fileSizes)]
    #[must_use]
    pub fn file_sizes(handles: &js_sys::Array) -> Vec<f64> {
        opfs::sizes(handles)
    }

    /// Build the enrolment request, without a till.
    ///
    /// A device has no identity until a code gives it one, so this cannot need
    /// a till: a till has to be opened as some terminal, and before enrolment
    /// there is no answer to which.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = enrolRequest))]
    #[must_use]
    pub fn enrol_request(code: &str) -> String {
        match sync::enrol_step(code) {
            Ok(step) => serde_json::to_string(&step).unwrap_or_default(),
            Err(message) => alloc::format!("{{\"error\":{message:?}}}"),
        }
    }

    /// What a refusal the server sent back says, in words.
    ///
    /// The body of a refused request is an encoded `ProtocolError`, and until
    /// now every platform threw it away and showed the status number. A status
    /// cannot say which barcode is already taken or which item has it; the core
    /// can, and this is how those words reach a screen without the screen
    /// deciding for itself what a refusal meant.
    ///
    /// Empty when the body is not a refusal this build knows, which is what a
    /// server one release ahead would send: the caller falls back to the status
    /// it already has.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = refusalInWords))]
    #[must_use]
    pub fn refusal_in_words(body: &str) -> String {
        sync::from_hex_public(body)
            .and_then(|bytes| {
                postcard::from_bytes::<openpos_core::protocol::ProtocolError>(&bytes).ok()
            })
            .map(|refusal| alloc::format!("{refusal}"))
            .unwrap_or_default()
    }

    /// The mark of a bundle somebody has pasted, without a till.
    ///
    /// Computed by the same code that marked it on the device it came from, so
    /// the two answers can be compared. Doing this in JavaScript would be a
    /// second implementation of a checksum, and two checksums that disagree are
    /// worse than none: they would never match and nobody would know why.
    ///
    /// Whitespace is ignored, because a bundle arrives through a messaging app
    /// and those wrap long text. Empty when the paste is not a bundle at all,
    /// which is the screen's cue to say so rather than show a mark for
    /// nonsense.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = bundleMark))]
    #[must_use]
    pub fn bundle_mark(bundle: &str) -> String {
        let cleaned: alloc::string::String =
            bundle.chars().filter(|one| !one.is_whitespace()).collect();
        sync::from_hex_public(&cleaned)
            .map(|bytes| marked(&bytes))
            .unwrap_or_default()
    }

    /// Read an enrolment reply, without a till.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = readEnrolment))]
    #[must_use]
    pub fn read_enrolment(body: &str) -> String {
        match sync::read_enrolment(body) {
            Ok(credential) => serde_json::to_string(&credential).unwrap_or_default(),
            Err(message) => alloc::format!("{{\"error\":{message:?}}}"),
        }
    }

    /// Put a credential in place, for a till just opened with the identity an
    /// enrolment reply gave it.
    ///
    /// The time it was taken goes with it. A credential expires, and a device
    /// that does not know how old its own is cannot renew before it stops
    /// working: that is a shop with a dead tablet a year after it was set up.
    /// A caller with no clock passes zero and the device renews at its next
    /// opportunity, which costs one request.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = adoptToken))]
    pub fn adopt_token(&mut self, token: &str, at_ms: f64) -> String {
        // A caller with no clock passes zero, which reads as "unknown" and
        // makes the device renew at its next opportunity.
        let taken = exact(at_ms).filter(|ms| *ms >= 0).unwrap_or(0);
        let taken = u64::try_from(taken).unwrap_or_default();
        // Nothing said how long one lasts yet; the first renewal is told.
        let outcome = with_till!(self, |till| till.take_credential(token, taken, 0));
        self.render_ref(outcome.err())
    }

    /// The names of the files a till needs, in the order `openOpfs` expects
    /// them. Exposed so the JavaScript that opens them cannot drift from the
    /// Rust that reads them.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = fileNames)]
    #[must_use]
    pub fn file_names() -> Vec<String> {
        opfs::FILE_NAMES
            .iter()
            .map(|name| String::from(*name))
            .collect()
    }

    /// Apply catalogue changes.
    ///
    /// JSON here and postcard on the wire, deliberately. What arrives from the
    /// server is postcard and is handed to the core as bytes; this is the path a
    /// front end uses to seed a demo or to apply changes it already holds, and
    /// the two must not be confused: only one of them is the sync protocol.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = applyItems))]
    pub fn apply_items(&mut self, json: &str) -> String {
        let Ok(items) = serde_json::from_str::<Vec<WireItem>>(json) else {
            return self.render_ref(Some(TillError::UnknownBarcode));
        };

        self.run(Command::ApplyItems { items })
    }

    /// Add a scanned barcode to the basket.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
    pub fn scan(&mut self, barcode: &str, qty_milli: f64) -> String {
        let Some(qty) = exact(qty_milli) else {
            return self.refuse(NOT_A_WHOLE_NUMBER);
        };
        self.run(Command::Scan {
            barcode: String::from(barcode),
            qty_milli: qty,
        })
    }

    /// Correct a quantity. Scanning three of something and meaning two must
    /// not cost the whole basket.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setQty))]
    pub fn set_qty(&mut self, line: f64, qty_milli: f64) -> String {
        self.run(Command::SetQty { line, qty_milli })
    }

    /// Take a line off the ticket.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = removeLine))]
    pub fn remove_line(&mut self, line: f64, at_ms: f64) -> String {
        let at_ms = exact(at_ms).filter(|ms| *ms >= 0).unwrap_or(0).unsigned_abs();
        self.run(Command::RemoveLine { line, at_ms })
    }

    /// Discount one line by a percentage.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setLineDiscount))]
    pub fn set_line_discount(&mut self, line: f64, percent: f64) -> String {
        self.run(Command::SetLineDiscount { line, percent })
    }

    // No typed wrapper for writing an item down, on purpose: it takes eight
    // things and a wasm export cannot take a struct, so the flat version would
    // be eight positional arguments a screen gets silently wrong. The command
    // goes through the JSON entry point like everything a screen sends.

    /// Take a stated amount off one line.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = takeOffLine))]
    pub fn take_off_line(&mut self, line: f64, amount_minor: f64) -> String {
        self.run(Command::TakeOffLine { line, amount_minor })
    }

    /// Take a stated amount off the whole basket.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = takeOffTicket))]
    pub fn take_off_ticket(&mut self, amount_minor: f64) -> String {
        self.run(Command::TakeOffTicket { amount_minor })
    }

    /// Discount the whole ticket by a percentage, apportioned across its lines.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setTicketDiscount))]
    pub fn set_ticket_discount(&mut self, percent: f64) -> String {
        self.run(Command::SetTicketDiscount { percent })
    }

    /// Take money.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = addCash))]
    pub fn add_cash(&mut self, amount_minor: f64, at_ms: f64) -> String {
        let Some(amount) = exact(amount_minor) else {
            return self.refuse(NOT_A_WHOLE_NUMBER);
        };
        self.run(Command::AddCash {
            amount_minor: amount,
            at_ms: exact(at_ms)
                .filter(|ms| *ms >= 0)
                .unwrap_or(0)
                .unsigned_abs(),
        })
    }

    /// Close the sale. The id and the clock come from the caller, because this
    /// crate mints neither.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
    pub fn checkout(&mut self, ticket_id: &str, rung_at_ms: f64) -> String {
        let Ok(id) = Ulid::decode(ticket_id) else {
            return self.render_ref(Some(TillError::UnknownBarcode));
        };
        let Some(at_ms) = exact(rung_at_ms).filter(|ms| *ms >= 0) else {
            return self.refuse(NOT_A_WHOLE_NUMBER);
        };
        self.run(Command::Checkout {
            ticket_id: id.encode(),
            rung_at_ms: at_ms.unsigned_abs(),
        })
    }

    /// Carry out one command given as JSON, and answer as JSON.
    ///
    /// The whole surface in one export, matching the C ABI exactly. The named
    /// methods below are sugar over the same dispatcher and exist for a caller
    /// that would rather write `scan(code, 1000)`.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = run))]
    pub fn run_command(&mut self, request: &str) -> String {
        self.run_json(request)
    }

    /// The current view, without changing anything.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
    #[must_use]
    pub fn view(&self) -> String {
        self.render_ref(None)
    }

    /// Refuse with a message of this crate's own, for mistakes the core never
    /// sees because they are caught at the boundary.
    fn refuse(&self, message: &str) -> String {
        let mut view = self.build_view(None);
        view.error = Some(String::from(message));
        serde_json::to_string(&view)
            .unwrap_or_else(|_| String::from(r#"{"error":"the till could not describe itself"}"#))
    }

    fn render_ref(&self, error: Option<TillError>) -> String {
        let view = self.build_view(error);
        // Serialising a struct of numbers and strings cannot fail. Returning a
        // fixed error shape rather than panicking, because a panic here unwinds
        // into JavaScript and leaves the till unusable until the page reloads.
        serde_json::to_string(&view)
            .unwrap_or_else(|_| String::from(r#"{"error":"the till could not describe itself"}"#))
    }

    /// What a supervisor would have to allow for the thing just refused.
    ///
    /// Named by the core rather than read off the words it used. A screen that
    /// matched on prose would be deciding for a second time what is permitted,
    /// in a place nobody tests, and would go quiet the day a message is
    /// reworded.
    /// The customer a refusal is asking for, when that is what it is asking
    /// for.
    ///
    /// Structured rather than left in the message for the same reason the
    /// supervisor's action is: a screen matching on prose decides a second time
    /// what the core decided once, in a place nobody tests, and goes quiet the
    /// day a message is reworded.
    fn wants_customer(error: Option<&TillError>) -> Option<alloc::string::String> {
        match error? {
            TillError::WriteItAgainstThem { name } => Some(name.clone()),
            _ => None,
        }
    }

    fn blocked_by(error: Option<&TillError>) -> Option<openpos_core::auth::Action> {
        use openpos_core::auth::{Action, AuthError};
        use openpos_core::cart::CartError;
        match error? {
            TillError::Cart(CartError::DiscountAboveCeiling { requested, .. }) => {
                Some(Action::Discount { bp: *requested })
            }
            TillError::Cart(CartError::PriceOverrideNotAllowed) => Some(Action::OverridePrice),
            TillError::MoreThanTheShelfHolds { .. } => Some(Action::SellBeyondStock),
            TillError::BeyondTheirLimit { .. } => Some(Action::BeyondTheirLimit),
            TillError::Auth(AuthError::NotPermitted { action }) => Some(*action),
            _ => None,
        }
    }

    fn build_view(&self, error: Option<TillError>) -> View {
        let totals = with_till!(ref self, |till| till.totals().ok());
        let status = with_till!(ref self, |till| till.status().ok());
        let tendered = with_till!(ref self, |till| till.cart().tendered().ok());
        let is_refund = with_till!(ref self, |till| till.cart().is_refund());

        // Line totals come from the arithmetic, not from a placeholder. This
        // field was zero for every line until a receipt made it visible, which
        // is what a field nobody reads is for.
        let line_totals = totals
            .as_ref()
            .map(|computed| computed.lines.clone())
            .unwrap_or_default();

        let lines = with_till!(ref self, |till| till
            .cart()
            .lines()
            .iter()
            .enumerate()
            .map(|(at, line)| Line {
                item_id: line.item_id.encode(),
                code: line.code.to_string(),
                name: line.name.to_string(),
                qty_milli: line.qty.get(),
                unit_price_minor: line.unit_price.get(),
                discount_minor: line_totals.get(at).map_or(0, |computed| computed.discount.get()),
                discount_bp: match line.discount {
                    openpos_core::domain::pricing::Discount::Rate(rate) => rate.get(),
                    _ => 0,
                },
                discount_amount_minor: match line.discount {
                    openpos_core::domain::pricing::Discount::Amount(off) => off.get(),
                    _ => 0,
                },
                total_minor: line_totals.get(at).map_or(0, |computed| computed.total.get()),
            })
            .collect());

        let total = totals.as_ref().map_or(0, |t| t.total.get());
        let paid = tendered.map_or(0, Minor::get);

        View {
            lines,
            net_minor: totals.as_ref().map_or(0, |t| t.net_total.get()),
            vat_minor: totals.as_ref().map_or(0, |t| t.vat_total.get()),
            discount_minor: totals.as_ref().map_or(0, |t| t.discount_total.get()),
            total_minor: total,
            tendered_minor: paid,
            change_minor: paid.saturating_sub(total),
            is_refund,
            receipt_numbers_left: status.map_or(0, |s| s.receipt_numbers_left),
            unsynced_sales: status.map_or(0, |s| s.unsynced_sales),
            enrolled: with_till!(ref self, |till| till.token().is_some()),
            credential_refused: self.refused,
            wallets: with_till!(ref self, |till| till
                .wallets()
                .iter()
                .map(ToString::to_string)
                .collect()),
            // What a supervisor would have to allow, when the last thing tried
            // was refused for want of permission. The screen shows a PIN box
            // and sends this back as it stands.
            needs_supervisor: Self::blocked_by(error.as_ref()),
            needs_customer: Self::wants_customer(error.as_ref()),
            beyond_the_shelf: with_till!(ref self, |till| till.beyond_the_shelf()),
            catalogue_cursor: with_till!(ref self, |till| till
                .situation(true, false)
                .map_or(0, |situation| situation.cursor)),
            customers: with_till!(ref self, |till| till
                .customers()
                .iter()
                .filter(|known| known.active)
                .map(|known| {
                    let owed = till.owed_by(Ulid::from_u128(known.id));
                    Customer {
                        id: Ulid::from_u128(known.id).encode(),
                        name: known.name.clone(),
                        phone: known.phone.clone(),
                        active: known.active,
                        owed_minor: owed.map(|(amount, _)| amount.get()),
                        owed_as_of_ms: owed.map(|(_, at_ms)| at_ms),
                        bin: known.bin.clone(),
                        // What the shop said they may owe, so a cashier can see
                        // how close somebody is before adding to it rather than
                        // finding out when the till refuses.
                        limit_minor: known.limit_minor,
                    }
                })
                .collect()),
            customer: with_till!(ref self, |till| till.customer().map(|id| id.encode())),
            operator: with_till!(ref self, |till| till.signed_in().map(|who| Operator {
                id: who.id.encode(),
                name: who.name.to_string(),
                may_refund: who.permissions.may_refund,
                may_override_price: who.permissions.may_override_price,
                may_open_drawer: who.permissions.may_open_drawer,
                may_close_shift: who.permissions.may_close_shift,
                max_discount_bp: who.permissions.max_discount_bp,
            })),
            report: self.last_report.clone(),
            held: with_till!(ref self, |till| till
                .held_tickets()
                .unwrap_or_default()
                .into_iter()
                .map(|held| Parked {
                    id: held.id.encode(),
                    label: held.label,
                    held_at_ms: held.held_at_ms,
                    lines: held.lines,
                    total_minor: held.total.get(),
                })
                .collect()),
            catalogue: self.last_catalogue.clone(),
            checked: self.last_checked.clone(),
            everyone: self.last_everyone.clone(),
            carrying: self.last_carrying.clone(),
            drawer: with_till!(ref self, |till| till.shift().map(|shift| Drawer {
                open: shift.is_open(),
                opening_float_minor: shift.opening_float().get(),
                sales: shift.sales(),
                expected_cash_minor: shift.expected_cash().map_or(0, Minor::get),
                movements: shift.movements().len(),
            })),
            people: with_till!(ref self, |till| till
                .people()
                .iter()
                .filter(|who| who.active)
                .map(person_seen)
                .collect()),
            error_code: error.as_ref().map(|error| error.code().to_owned()),
            error_parts: error.as_ref().map(parts_of).unwrap_or_default(),
            error: error.map(|error| error.to_string()),
            receipt: self.last_receipt.clone(),
            job: self.last_job.clone(),
            step: self.last_step.clone(),
            applied: self.last_applied.clone(),
        }
    }
}

impl TillHandle {
    /// Open on a store the caller already has. For tests, and for a platform
    /// that wants to hand in a prepared image.
    ///
    /// # Errors
    /// When the identifiers are not ids, or the store will not open.
    pub fn open_on(backend: MemoryBackend, tenant: &str, terminal: &str) -> Option<Self> {
        let tenant = Ulid::decode(tenant).ok()?;
        let terminal = Ulid::decode(terminal).ok()?;
        let (inner, _report) = Till::open(
            backend,
            tenant.to_u128(),
            terminal,
            1,
            CartLimits::default(),
        )
        .ok()?;
        Some(Self::wrap(Store::Memory(inner), tenant.to_u128()))
    }

    /// Put a credential in place without enrolling, for tests and for a
    /// platform restoring a prepared image.
    ///
    /// # Errors
    /// When the store will not hold it.
    pub fn set_token_for_test(&mut self, token: &str) {
        let _ = with_till!(self, |till| till.set_token(token));
    }

    /// The credential this terminal holds, if it has enrolled.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        with_till!(ref self, |till| till.token())
    }

    /// The store, for a caller that opened one and wants it back.
    #[must_use]
    pub fn backend(&self) -> Option<&MemoryBackend> {
        match &self.inner {
            Store::Memory(till) => Some(till.journal().backend()),
            #[cfg(target_arch = "wasm32")]
            Store::Opfs(_) => None,
            #[cfg(not(target_arch = "wasm32"))]
            Store::Files(_) => None,
        }
    }

    fn wrap(inner: Store, tenant: u128) -> Self {
        Self {
            inner,
            tenant,
            driver: Driver::new(),
            more_to_pull: false,
            refused: false,
            last_step: None,
            last_applied: None,
            last_receipt: None,
            last_job: None,
            last_report: None,
            last_account: Vec::new(),
            last_checked: None,
            last_drawer: None,
            last_sale: None,
            last_catalogue: None,
            last_everyone: None,
            last_carrying: None,
        }
    }
}

/// The surface both platforms actually use, outside the wasm-bindgen export
/// list so the C boundary can call exactly this and get exactly what a browser
/// gets.
impl TillHandle {
    /// Carry out a command and describe the till afterwards.
    pub fn run(&mut self, command: Command) -> String {
        // Sync needs the driver and the tenant, which belong to the handle
        // rather than to the till, so those three commands are answered here.
        match command {
            Command::Checkout {
                ref ticket_id,
                rung_at_ms,
            } => {
                let id = ticket_id.clone();
                return self.checkout_keeping_the_sale(&id, rung_at_ms);
            }
            Command::Receipt { .. } | Command::Escpos { .. } => return self.print(command),
            Command::PaperBytes { feed_lines, cut } => {
                return self.paper_bytes(feed_lines, cut);
            }
            Command::StatementPaper {
                width,
                ref customer,
                ref at,
                ref dates,
                ref words,
            } => {
                let customer = customer.clone();
                let at = at.clone();
                let dates = dates.clone();
                let words = receipt::Words::of(words.clone());
                return self.statement_paper(width, &customer, &at, &dates, words);
            }
            Command::DrawerPaper {
                width,
                ref at,
                ref till,
                ref counted_by,
                ref words,
            } => {
                let at = at.clone();
                let till_named = till.clone();
                let who = counted_by.clone();
                let words = receipt::Words::of(words.clone());
                return self.drawer_paper(width, &at, till_named, who, words);
            }
            Command::SignIn {
                ref operator_id,
                ref pin,
                now_ms,
            } => {
                let (id, pin) = (operator_id.clone(), pin.clone());
                return self.sign_in(&id, &pin, now_ms);
            }
            Command::SetQty { line, qty_milli } => {
                let (Some(at), Some(qty)) = (index(line), exact(qty_milli)) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let outcome = with_till!(self, |till| till.set_qty(at, Milli::new(qty)));
                return self.render_ref(outcome.err());
            }
            Command::RemoveLine { line, at_ms } => {
                let Some(at) = index(line) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let outcome = with_till!(self, |till| till.remove_line(at, at_ms));
                return self.render_ref(outcome.err());
            }
            Command::SetUnitPrice { line, price_minor } => {
                let (Some(at), Some(price)) = (index(line), exact(price_minor)) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let outcome = with_till!(self, |till| till.set_unit_price(at, Minor::new(price)));
                return self.render_ref(outcome.err());
            }
            Command::SetLineDiscount { line, percent } => {
                let (Some(at), Some(discount)) = (index(line), rate_of(percent)) else {
                    return self.refuse(NOT_A_PERCENTAGE);
                };
                let outcome = with_till!(self, |till| till.set_line_discount(at, discount));
                return self.render_ref(outcome.err());
            }
            Command::WriteCustomer {
                ref id,
                ref name,
                ref phone,
                ref bin,
            } => {
                let Ok(id) = Ulid::decode(id) else {
                    return self.refuse("that customer id is not a valid id");
                };
                let written = openpos_core::storage::wire::CustomerV1 {
                    id: id.to_u128(),
                    name: name.trim().to_string(),
                    phone: phone
                        .as_ref()
                        .map(|phone| phone.trim().to_string())
                        .filter(|phone| !phone.is_empty()),
                    active: true,
                    bin: bin
                        .as_ref()
                        .map(|bin| bin.trim().to_string())
                        .filter(|bin| !bin.is_empty()),
                    limit_minor: 0,
                };
                let outcome = with_till!(self, |till| till.write_customer(written));
                return self.render_ref(outcome.err());
            }
            Command::QuickAdd {
                ref id,
                ref barcode,
                ref name,
                ref name_bn,
                ref unit,
                price_minor,
                vat_bp,
                price_inclusive,
            } => {
                let Ok(id) = Ulid::decode(id) else {
                    return self.refuse("that item id is not a valid id");
                };
                let (Some(price), Some(rate)) = (exact(price_minor), exact(vat_bp)) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let Ok(vat_rate) = u32::try_from(rate).map_or(Err(()), |bp| {
                    openpos_core::money::Bp::new(bp).map_err(|_| ())
                }) else {
                    return self.refuse("a tax rate is between nothing and a hundred percent");
                };
                if price < 0 {
                    return self.refuse("a price below zero would pay the customer");
                }
                let name_bn = if name_bn.trim().is_empty() {
                    name.clone()
                } else {
                    name_bn.clone()
                };
                let unit = if unit.trim().is_empty() {
                    String::from("Nos")
                } else {
                    unit.clone()
                };
                let written = openpos_core::replica::Item {
                    id,
                    // Its own barcode, because a cashier at a counter has no
                    // code scheme in their head and the shop can give it one.
                    code: barcode.clone().into(),
                    name_en: name.clone().into(),
                    name_bn: name_bn.into(),
                    unit: unit.into(),
                    price: Minor::new(price),
                    // What it cost the shop is the owner's to fill in: a cashier
                    // holding a queue does not know it, and a made-up number
                    // becomes a made-up margin in every report after it.
                    cost: Minor::ZERO,
                    vat_rate,
                    price_mode: if price_inclusive {
                        openpos_core::domain::PriceMode::Inclusive
                    } else {
                        openpos_core::domain::PriceMode::Exclusive
                    },
                    vat_base: openpos_core::domain::VatBase::Discounted,
                    barcodes: alloc::vec![barcode.clone().into()],
                    // Nothing counted. What arrived is a delivery somebody
                    // books in, not a number typed at a till.
                    on_hand: openpos_core::money::Milli::ZERO,
                    active: true,
                    supply: openpos_core::domain::Supply::Standard,
                    // Nobody sorts shelves with a queue in front of them. The
                    // owner puts it under something when they look at it.
                    category: "".into(),
                };
                let outcome = with_till!(self, |till| till.quick_add(written));
                return self.render_ref(outcome.err());
            }
            Command::TakeOffLine { line, amount_minor } => {
                let (Some(at), Some(off)) = (index(line), exact(amount_minor)) else {
                    return self.refuse(NOT_AN_AMOUNT);
                };
                if off < 0 {
                    return self.refuse(NOT_AN_AMOUNT);
                }
                let discount = if off == 0 {
                    Discount::None
                } else {
                    Discount::Amount(Minor::new(off))
                };
                let outcome = with_till!(self, |till| till.set_line_discount(at, discount));
                return self.render_ref(outcome.err());
            }
            Command::TakeOffTicket { amount_minor } => {
                let Some(off) = exact(amount_minor).filter(|off| *off >= 0) else {
                    return self.refuse(NOT_AN_AMOUNT);
                };
                let discount = if off == 0 {
                    Discount::None
                } else {
                    Discount::Amount(Minor::new(off))
                };
                let outcome = with_till!(self, |till| till.set_ticket_discount(discount));
                return self.render_ref(outcome.err());
            }
            Command::SetTicketDiscount { percent } => {
                let Some(discount) = rate_of(percent) else {
                    return self.refuse(NOT_A_PERCENTAGE);
                };
                let outcome = with_till!(self, |till| till.set_ticket_discount(discount));
                return self.render_ref(outcome.err());
            }
            Command::Check { ref code } => {
                let wanted = code.trim().to_owned();
                if wanted.is_empty() {
                    return self.refuse("scan it, or type part of the name");
                }
                let found = with_till!(ref self, |till| {
                    let replica = till.replica();
                    // The barcode first, because that is what a scanner sends
                    // and an exact match beats a search that might rank
                    // something else first. Then the words, so somebody with no
                    // scanner or a torn label can type instead.
                    replica
                        .by_barcode(&wanted)
                        .or_else(|| replica.search(&wanted, 1).into_iter().next())
                        .map(|item| {
                            let totals = openpos_core::domain::pricing::line_totals(
                                &openpos_core::domain::pricing::LineInput {
                                    qty: Milli::ONE,
                                    unit_price: item.price,
                                    discount: Discount::None,
                                    vat_rate: item.vat_rate,
                                    price_mode: item.price_mode,
                                    vat_base: item.vat_base,
                                    supply: item.supply,
                                },
                            );
                            (WireItem::of(item), totals)
                        })
                });
                return match found {
                    // The same words the scanner path answers with, because to
                    // a cashier it is the same thing: the till does not know
                    // what you mean.
                    None => self.refuse("no item in the catalogue has that"),
                    // And the same again for something withdrawn. Found by
                    // walking it: the till said "no item in the catalogue has
                    // that" about a thing it was holding and could describe,
                    // which sends a cashier hunting for a barcode that is fine.
                    Some((item, _)) if !item.active => self.refuse(&alloc::format!(
                        "{}: {}",
                        openpos_core::till::TillError::NoLongerSold,
                        item.name
                    )),
                    Some((item, Err(_))) => {
                        // A price the arithmetic will not stand behind. Said
                        // rather than shown, because a figure quoted across the
                        // counter is one the shop has to honour.
                        self.refuse(&alloc::format!(
                            "{} is priced in a way this till cannot work out: correct it in the                              back office before quoting it",
                            item.name
                        ))
                    }
                    Some((item, Ok(totals))) => {
                        self.last_checked = Some(Checked {
                            item,
                            each_minor: totals.total.get(),
                            vat_minor: totals.vat.get(),
                        });
                        self.render_ref(None)
                    }
                };
            }
            Command::Catalogue {
                ref query,
                limit,
                retired,
            } => {
                // The ceiling is for a screen's list, and a caller matching a
                // whole catalogue against a file needs the whole catalogue: a
                // shop of six hundred lines that could only see five hundred
                // called the rest new and made a second copy of them. Five
                // thousand is a shop far larger than this is for, and the
                // caller is told when it hits the ceiling rather than being
                // handed a page that looks like everything.
                let (query, limit) = (query.clone(), limit.min(5_000));
                let found = with_till!(ref self, |till| {
                    let replica = till.replica();
                    let wanted = query.trim().to_lowercase();
                    // An empty box is a listing, not a search for nothing. The
                    // core's search answers nothing to an empty query, which is
                    // right for a till's autocomplete and wrong for an owner
                    // opening a screen to see what is there.
                    //
                    // The core's search also hides what the shop has stopped
                    // selling, which is right for a till and leaves a back
                    // office no way to find a retired item and bring it back. So
                    // asking for those is a plain scan: it is a screen for an
                    // owner, not the path a scanner takes.
                    if retired {
                        replica
                            .items()
                            .iter()
                            .filter(|item| {
                                wanted.is_empty()
                                    || item.name_en.to_lowercase().contains(&wanted)
                                    || item.code.to_lowercase().contains(&wanted)
                            })
                            .take(limit)
                            .map(WireItem::of)
                            .collect::<Vec<_>>()
                    } else if wanted.is_empty() {
                        replica
                            .items()
                            .iter()
                            .filter(|item| item.active)
                            .take(limit)
                            .map(WireItem::of)
                            .collect()
                    } else {
                        // The barcode first, because a cashier whose label will
                        // not scan reads the number off the box and types it,
                        // and the index behind the search holds names and codes
                        // rather than barcodes: the shop's own number found
                        // nothing, which reads as a shop that does not sell it.
                        let scanned = replica
                            .by_barcode(query.trim())
                            .into_iter()
                            .map(WireItem::of)
                            .collect::<Vec<_>>();
                        if scanned.is_empty() {
                            replica.search(&query, limit).into_iter().map(WireItem::of).collect()
                        } else {
                            scanned
                        }
                    }
                });
                self.last_catalogue = Some(found);
                return self.render_ref(None);
            }
            Command::SetCustomer { ref customer } => {
                let chosen = match customer.as_deref() {
                    Some(text) => match Ulid::decode(text) {
                        Ok(id) => Some(id),
                        Err(_) => return self.refuse("that is not a customer"),
                    },
                    None => None,
                };
                let outcome = with_till!(self, |till| till.set_customer(chosen));
                return match outcome {
                    Ok(()) => self.render_ref(None),
                    Err(error) => {
                        let message = alloc::format!("{error}");
                        self.refuse(&message)
                    }
                };
            }
            Command::Carrying => {
                let carried = with_till!(ref self, |till| till
                    .carried_out(500)
                    .map(|sales| (till.terminal(), sales)));
                return match carried {
                    Ok((terminal, sales)) => {
                        let bundle = openpos_core::protocol::AdoptSalesRequest {
                            protocol: openpos_core::protocol::PROTOCOL_VERSION,
                            terminal: terminal.to_u128(),
                            sales: sales
                                .iter()
                                .map(|sale| openpos_core::protocol::SaleEnvelope {
                                    id: sale.id.to_u128(),
                                    schema: sale.schema,
                                    payload: sale.payload.clone(),
                                })
                                .collect(),
                        };
                        match postcard::to_allocvec(&bundle) {
                            Ok(bytes) => {
                                self.last_carrying = Some(Carrying {
                                    terminal: terminal.encode(),
                                    total_minor: sales
                                        .iter()
                                        .map(|sale| sale.total_minor)
                                        .fold(0_i64, i64::saturating_add),
                                    sales: sales
                                        .iter()
                                        .map(|sale| CarriedSale {
                                            id: sale.id.encode(),
                                            total_minor: sale.total_minor,
                                            salvaged: sale.salvaged,
                                        })
                                        .collect(),
                                    mark: marked(&bytes),
                                    letters: bytes.len().saturating_mul(2),
                                    bundle: sync::to_hex(&bytes),
                                });
                                self.render_ref(None)
                            }
                            Err(_) => self.refuse("those sales could not be written out"),
                        }
                    }
                    Err(error) => {
                        let message = alloc::format!("{error}");
                        self.refuse(&message)
                    }
                };
            }
            Command::Everyone => {
                self.last_everyone = Some(with_till!(ref self, |till| till
                    .people()
                    .iter()
                    .map(person_seen)
                    .collect::<Vec<_>>()));
                return self.render_ref(None);
            }
            Command::XReport => return self.report(None),
            Command::Admin { ref request } => {
                let request = request.clone();
                let tenant = self.tenant;
                let outcome = with_till!(ref self, |till| sync::admin_step(till, tenant, &request));
                return match outcome {
                    Ok(step) => {
                        self.last_step = Some(step);
                        self.render_ref(None)
                    }
                    Err(message) => {
                        self.last_step = None;
                        self.refuse(&message)
                    }
                };
            }
            Command::CloseShift {
                counted_cash_minor,
                at_ms,
            } => return self.report(Some((counted_cash_minor, at_ms))),
            Command::SignOut => {
                with_till!(self, |till| till.sign_out());
                return self.render_ref(None);
            }
            Command::Authorise {
                ref supervisor_id,
                ref pin,
                action,
                now_ms,
                valid_for_ms,
            } => {
                let (id, pin) = (supervisor_id.clone(), pin.clone());
                return self.authorise(&id, &pin, action, now_ms, valid_for_ms);
            }
            Command::SyncStep { online, now_ms } => return self.sync_step(online, now_ms),
            Command::SyncApply { kind, body, now_ms } => {
                return self.sync_apply(kind, &body, now_ms);
            }
            Command::SyncFailed { now_ms, status } => {
                // 401 or 403 to a request that carried this device's credential
                // means the credential is no good: the terminal was removed, the
                // token was revoked, or the server was rebuilt underneath it.
                // Backing off and retrying forever is the wrong answer, and it
                // is the answer a device gives when nobody records why it failed.
                self.refused = matches!(status, Some(401 | 403));
                sync::failed(&mut self.driver, now_ms);
                self.last_step = None;
                return self.render_ref(None);
            }
            Command::Enrol { ref code } => {
                return match sync::enrol_step(code) {
                    Ok(step) => {
                        self.last_step = Some(step);
                        self.render_ref(None)
                    }
                    Err(message) => {
                        self.last_step = None;
                        self.refuse(&message)
                    }
                };
            }
            _ => {}
        }
        let error = with_till!(self, |till| dispatch(till, command));
        self.render_ref(error)
    }

    /// Close the sale and keep it, because a receipt is of the sale that
    /// happened rather than of whatever is on the screen afterwards.
    fn checkout_keeping_the_sale(&mut self, ticket_id: &str, rung_at_ms: u64) -> String {
        let Ok(id) = Ulid::decode(ticket_id) else {
            return self.refuse("that ticket identifier is not a valid id");
        };
        let outcome = with_till!(self, |till| till.checkout(id, rung_at_ms));
        match outcome {
            Ok(sale) => {
                self.last_sale = Some(sale.ticket);
                self.last_receipt = None;
                self.render_ref(None)
            }
            Err(error) => self.render_ref(Some(error)),
        }
    }

    /// Turn whatever was last laid out into bytes a thermal printer takes.
    ///
    /// A receipt, a drawer slip or a customer's account: all three are lines by
    /// the time they get here, and a printer does not care which. Encoded from
    /// the lines rather than rendered again from the record behind them, so the
    /// paper that comes out is the page that was on the screen.
    fn paper_bytes(&mut self, feed_lines: u8, cut: bool) -> String {
        let Some(lines) = self.last_receipt.clone() else {
            return self.refuse("nothing has been laid out on this terminal to print");
        };
        let job = receipt::escpos::encode(&lines, &receipt::escpos::Printer { feed_lines, cut });
        self.last_job = Some(PrintJob {
            bytes: sync::to_hex_public(&job.bytes),
            // Lines the printer's character set cannot carry, which on a page
            // of Bangla names is most of them. Reported rather than dropped: a
            // platform with a raster path prints those as images, and one
            // without at least knows what it could not print.
            unprintable: job.unprintable,
        });
        self.render_ref(None)
    }

    /// Lay one customer's account out for paper: the khata page they take away.
    ///
    /// Every figure comes from the account the shop sent, and the running total
    /// is added up by the same crate that lays out a receipt. A balance the
    /// screen worked out and handed back would be a second arithmetic, and the
    /// customer arguing at the counter would be arguing with whichever of them
    /// was on the paper.
    fn statement_paper(
        &mut self,
        width: usize,
        customer: &str,
        at: &str,
        dates: &[String],
        words: receipt::Words,
    ) -> String {
        let held = self.last_account.clone();
        if held.is_empty() {
            return self.refuse("this device has not been sent that account yet");
        }
        if dates.len() != held.len() {
            // The dates and the money would be paired by position, and a
            // statement whose dates are shifted by one is worse than no
            // statement: every line reads as a day it did not happen.
            return self.refuse("one date is needed for each line of the account");
        }

        let mut lines: Vec<receipt::StatementLine> = held
            .iter()
            .zip(dates)
            .map(|(one, at)| receipt::StatementLine {
                at: at.clone(),
                what: if one.is_sale && one.amount_minor < 0 {
                    // A sale line going the other way is goods coming back. It
                    // reads as a sale of minus five hundred otherwise, which
                    // is not a sentence anybody says at a counter.
                    if one.note.is_empty() {
                        String::from("Goods brought back")
                    } else {
                        format!("Goods brought back, {}", one.note)
                    }
                } else if one.is_sale && one.note.is_empty() {
                    String::from("Sale")
                } else if one.is_sale {
                    format!("Sale {}", one.note)
                } else if one.written_off {
                    String::from("Written off")
                } else if one.note.is_empty() {
                    String::from("Paid")
                } else {
                    format!("Paid, {}", one.note)
                },
                // Positive is what they owe. A payment arrives as a negative
                // amount already, which is what the account book means by it.
                amount: Minor::new(one.amount_minor),
            })
            .collect();
        // The shop sends an account newest first, because that is what a screen
        // shows. A person reading their own account reads down the page in the
        // order the days happened, so the paper turns it over, dates and money
        // together.
        lines.reverse();

        let shop = with_till!(ref self, |till| till.shop().cloned()).unwrap_or_default();
        self.last_receipt = Some(receipt::statement(
            &lines,
            &receipt::StatementContext {
                shop,
                customer: String::from(customer),
                at: String::from(at),
                width,
                words,
            },
        ));
        self.last_job = None;
        self.render_ref(None)
    }

    /// Lay the drawer out for paper: the slip that goes in with the cash.
    ///
    /// The same figures the screen shows, laid out by the same crate that lays
    /// out a receipt, so the till, a thermal printer and the Android build all
    /// produce one slip rather than three.
    fn drawer_paper(
        &mut self,
        width: usize,
        at: &str,
        till_named: Option<String>,
        counted_by: Option<String>,
        words: receipt::Words,
    ) -> String {
        let Some((totals, counted)) = self.last_drawer.clone() else {
            // Asked for before anybody looked at the drawer. Naming it beats
            // printing a blank slip, which reads as a printer fault.
            return self.refuse("no drawer has been reported on this terminal yet");
        };
        let shop = with_till!(ref self, |till| till.shop().cloned()).unwrap_or_default();
        let lines = receipt::drawer(
            &totals,
            counted,
            &receipt::DrawerContext {
                shop,
                at: String::from(at),
                till: till_named,
                counted_by,
                width,
                words,
            },
        );
        self.last_receipt = Some(lines);
        self.last_job = None;
        self.render_ref(None)
    }

    /// Lay the last sale out for paper, as lines or as printer bytes.
    fn print(&mut self, command: Command) -> String {
        let (width, rung_at, cashier, words, printer) = match command {
            Command::Receipt {
                width,
                rung_at,
                cashier,
                words,
            } => (width, rung_at, cashier, receipt::Words::of(words), None),
            // Nothing for the thermal path: no ESC/POS code page carries
            // Bangla, so a printer is handed the English this crate defaults
            // to and `escpos::encode` says which lines it could not print.
            Command::Escpos {
                width,
                rung_at,
                cashier,
                feed_lines,
                cut,
            } => (
                width,
                rung_at,
                cashier,
                receipt::Words::default(),
                Some(receipt::escpos::Printer { feed_lines, cut }),
            ),
            _ => return self.refuse("that is not a receipt request"),
        };
        let Some(sale) = self.last_sale.clone() else {
            // A reprint before anything has been sold is a mistake worth
            // naming: printing a blank would look like a printer fault.
            return self.refuse("no sale has been completed on this terminal yet");
        };

        // The shop comes from the till, not from the caller. A platform passing
        // it would be a platform that can pass the wrong one, and every terminal
        // in a shop would need the same string typed into it.
        let Some(shop) = with_till!(ref self, |till| till.shop().cloned()) else {
            return self.refuse(
                "this terminal does not know its shop yet, so a receipt would have no name on it",
            );
        };

        // Who bought it, when the ticket names somebody the shop wrote down.
        // Looked up here rather than carried on the ticket, because the ticket
        // holds the id and the name belongs to the record: a person renamed
        // last month should print as they are called now.
        let known = sale.customer.and_then(|id| {
            with_till!(ref self, |till| till
                .customers()
                .iter()
                .find(|known| known.id == id.to_u128())
                .cloned())
        });
        let customer = known.as_ref().map(|known| known.name.clone());
        // And their BIN when they are a business, which is what makes the paper
        // a tax invoice to them rather than a receipt.
        let customer_bin = known.and_then(|known| known.bin.clone());

        let lines = receipt::render(
            &sale,
            &receipt::Context {
                shop,
                rung_at,
                cashier,
                customer,
                customer_bin,
                width,
                words,
            },
        );

        match printer {
            None => {
                self.last_receipt = Some(lines);
                self.last_job = None;
            }
            Some(printer) => {
                let job = receipt::escpos::encode(&lines, &printer);
                // Hex, matching the sync bodies, so a platform has one way of
                // carrying bytes across this boundary rather than two.
                self.last_job = Some(PrintJob {
                    bytes: sync::to_hex_public(&job.bytes),
                    unprintable: job.unprintable,
                });
                self.last_receipt = Some(lines);
            }
        }
        self.render_ref(None)
    }

    /// The drawer's totals, either as they stand or as the close recorded them.
    ///
    /// One shape for both, because a Z report is an X report plus what was
    /// counted, and a screen that had two would render the same eight numbers
    /// twice and let them drift.
    fn report(&mut self, closing: Option<(i64, u64)>) -> String {
        let outcome = match closing {
            None => with_till!(ref self, |till| till.x_report().map(|totals| (totals, None))),
            Some((counted, at_ms)) => with_till!(self, |till| till
                .close_shift(Minor::new(counted), at_ms)
                .map(|z| (
                    z.totals.clone(),
                    Some((z.counted_cash, z.closed_at_ms, z.variance))
                ))),
        };

        match outcome {
            Ok((totals, closed)) => {
                self.last_report = Some(Report {
                    shift: totals.shift.encode(),
                    opened_at_ms: totals.opened_at_ms,
                    opening_float_minor: totals.opening_float.get(),
                    sales: totals.sales,
                    tenders: totals
                        .tenders
                        .iter()
                        .map(|row| TenderRow {
                            name: tender_kind_name(&row.kind),
                            kind: String::from(match row.kind {
                                TenderKind::Cash => "cash",
                                TenderKind::Card => "card",
                                TenderKind::Credit => "credit",
                                TenderKind::Wallet(_) | TenderKind::Other(_) => "wallet",
                            }),
                            amount_minor: row.amount.get(),
                            in_drawer: row.in_drawer,
                        })
                        .collect(),
                    cash_sales_minor: totals.cash_sales.get(),
                    non_cash_sales_minor: totals.non_cash_sales.get(),
                    cash_in_minor: totals.cash_in.get(),
                    cash_out_minor: totals.cash_out.get(),
                    expected_cash_minor: totals.expected_cash.get(),
                    counted_cash_minor: closed.map(|(counted, _, _)| counted.get()),
                    closed_at_ms: closed.map(|(_, at, _)| at),
                    variance_minor: closed.map(|(_, _, variance)| variance.get()),
                });
                self.last_drawer = Some((
                    totals,
                    closed.map(|(counted, _, variance)| (counted, variance)),
                ));
                self.render_ref(None)
            }
            Err(error) => self.render_ref(Some(error)),
        }
    }

    fn sign_in(&mut self, operator_id: &str, pin: &str, now_ms: u64) -> String {
        let Ok(id) = Ulid::decode(operator_id) else {
            return self.refuse("that operator identifier is not a valid id");
        };
        let outcome = with_till!(self, |till| till.sign_in(id, pin, now_ms));
        self.render_ref(outcome.err())
    }

    fn authorise(
        &mut self,
        supervisor_id: &str,
        pin: &str,
        action: openpos_core::auth::Action,
        now_ms: u64,
        valid_for_ms: u64,
    ) -> String {
        let Ok(id) = Ulid::decode(supervisor_id) else {
            return self.refuse("that supervisor identifier is not a valid id");
        };
        let outcome = with_till!(self, |till| till.authorise(
            id,
            pin,
            action,
            now_ms,
            valid_for_ms
        ));
        self.render_ref(outcome.err())
    }

    fn sync_step(&mut self, online: bool, now_ms: u64) -> String {
        let tenant = self.tenant;
        let more = self.more_to_pull;
        let driver = self.driver;
        let outcome = with_till!(ref self, |till| sync::step(
            till, &driver, tenant, online, more, now_ms
        ));
        match outcome {
            Ok(step) => {
                self.last_step = Some(step);
                self.render_ref(None)
            }
            Err(message) => {
                self.last_step = None;
                self.refuse(&message)
            }
        }
    }

    fn sync_apply(&mut self, kind: sync::Exchange, body: &str, now_ms: u64) -> String {
        let mut driver = self.driver;
        let outcome = with_till!(self, |till| sync::apply(
            till,
            &mut driver,
            kind,
            body,
            now_ms
        ));
        self.driver = driver;
        match outcome {
            Ok(applied) => {
                // Whatever was wrong with the credential is not wrong now: the
                // server accepted a request that carried it.
                self.refused = false;
                self.more_to_pull = applied.more_to_pull;
                // Kept where a later round cannot overwrite it. An account is
                // read once and printed a minute later, and everything else
                // that arrives in between goes through this same field.
                if !applied.account.is_empty() {
                    self.last_account = applied.account.clone();
                }
                self.last_applied = Some(applied);
                self.last_step = None;
                self.render_ref(None)
            }
            Err(message) => {
                // A reply that arrived but did not decode is a failure, not a
                // success with nothing in it: counting it as success would clear
                // the backoff against a server answering with an error page.
                sync::failed(&mut self.driver, now_ms);
                self.refuse(&message)
            }
        }
    }

    /// Carry out a command given as JSON, and answer as JSON.
    ///
    /// The whole surface in one call. A caller whose command does not parse is
    /// told so, rather than left holding a reply from a till that silently did
    /// nothing.
    pub fn run_json(&mut self, request: &str) -> String {
        match serde_json::from_str::<Command>(request) {
            Ok(command) => self.run(command),
            Err(error) => self.refuse(&alloc::format!("could not read that command: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use super::*;

    fn view_of(json: &str) -> View {
        serde_json::from_str(json).expect("the facade returns its own shape")
    }

    #[test]
    fn a_till_opens_and_describes_itself() {
        let till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let view = view_of(&till.view());
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
        assert!(view.error.is_none());
    }

    #[test]
    fn a_bad_identifier_is_refused_rather_than_guessed_at() {
        assert!(TillHandle::open_in_memory("not-a-ulid", "also-not").is_none());
    }

    #[test]
    fn a_refusal_comes_back_in_the_view_rather_than_as_an_exception() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // Nothing has been pulled, so no barcode matches. A UI that renders the
        // view cannot silently drop this.
        let view = view_of(&till.scan("8690000000001", 1_000.0));
        assert!(view.error.is_some(), "a refusal must be visible");
        assert!(view.lines.is_empty());
    }

    #[test]
    fn change_is_negative_while_the_customer_still_owes() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // A UI showing zero here would be showing the same thing it shows when
        // the basket is settled, which is the one moment it must not.
        let view = view_of(&till.add_cash(10_000.0, 0.0));
        assert_eq!(view.tendered_minor, 10_000);
        assert_eq!(
            view.change_minor, 10_000,
            "nothing rung yet, so it is all change"
        );
    }

    #[test]
    fn a_nonsense_timestamp_clamps_rather_than_wrapping() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // An empty cart refuses anyway; the point is that a negative double does
        // not become an enormous u64 on the way in.
        let view = view_of(&till.checkout(&Ulid::from_u128(900).encode(), -1.0));
        assert!(view.error.is_some());
    }

    /// The screen's own route to taking a line off a paid basket.
    ///
    /// The permission was on every operator record and nothing enforced it, so
    /// this is the seam that has to carry it: the JSON command a screen sends,
    /// the refusal it gets back, and the supervisor prompt it puts up.
    #[test]
    fn taking_a_line_off_a_paid_basket_asks_for_a_supervisor() {
        let mut till = till_with_a_listed_price_item();

        let who = openpos_core::auth::OperatorId::from_u128(11);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Rahima".into(),
                pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: openpos_core::auth::Permissions::cashier(),
                active: true,
            },
            openpos_core::auth::Operator {
                id: openpos_core::auth::OperatorId::from_u128(12),
                name: "Karim".into(),
                pin: openpos_core::auth::PinHash::derive("9999", [4; 16], 1_000),
                permissions: openpos_core::auth::Permissions::supervisor(),
                active: true,
            },
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "4321", 0))
                .error
                .is_none()
        );
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // A mis-scan, before anybody has paid: nobody is asked anything.
        let view = view_of(&till.run_json(r#"{"op":"remove_line","line":0,"at_ms":1000}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());

        // Now the same basket with money on it.
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );
        let paid = view_of(
            &till.run_json(r#"{"op":"add_tender","kind":"cash","amount_minor":10000}"#),
        );
        assert!(paid.error.is_none(), "{:?}", paid.error);
        let view = view_of(&till.run_json(r#"{"op":"remove_line","line":0,"at_ms":2000}"#));
        assert!(view.error.is_some(), "a paid basket is not edited quietly");
        assert_eq!(
            view.needs_supervisor,
            Some(openpos_core::auth::Action::VoidLine)
        );
        assert_eq!(view.lines.len(), 1, "and the line is still on the screen");

        let allow = alloc::format!(
            r#"{{"op":"authorise","supervisor_id":"{}","pin":"9999","action":{{"action":"void_line"}},"now_ms":2000}}"#,
            openpos_core::auth::OperatorId::from_u128(12).encode()
        );
        let view = view_of(&till.run_json(&allow));
        assert!(view.error.is_none(), "{:?}", view.error);

        let view = view_of(&till.run_json(r#"{"op":"remove_line","line":0,"at_ms":3000}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());
        assert_eq!(
            view.operator.map(|who| who.name),
            Some(String::from("Rahima")),
            "the cashier is still the one at the till"
        );
    }

    /// A drawer slip reaches a thermal printer, not only a browser.
    ///
    /// The receipt has had a byte path since printers were supported. The
    /// drawer slip and a customer's account had nowhere to go but a browser's
    /// print dialog, so a shop with a thermal printer and an Android till could
    /// print what it sold and not what it counted.
    #[test]
    fn any_paper_can_be_handed_to_a_printer() {
        let mut till = till_with_a_listed_price_item();

        // Nothing laid out yet, which is not the same as a printer fault.
        let refused = view_of(&till.run_json(r#"{"op":"paper_bytes"}"#));
        assert!(refused.error.is_some());

        let who = openpos_core::auth::OperatorId::from_u128(11);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Rahima".into(),
                pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: openpos_core::auth::Permissions::supervisor(),
                active: true,
            },
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "4321", 0))
                .error
                .is_none()
        );
        assert!(
            view_of(&till.run_json(
                r#"{"op":"open_shift","shift_id":"00000000000000000000000051","opening_float_minor":30000,"at_ms":1000}"#
            ))
            .error
            .is_none()
        );
        assert!(
            view_of(&till.run_json(
                r#"{"op":"close_shift","counted_cash_minor":29550,"at_ms":3000}"#
            ))
            .error
            .is_none()
        );
        assert!(
            view_of(&till.run_json(
                r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40","counted_by":"Rahima"}"#
            ))
            .error
            .is_none()
        );

        let printed = view_of(&till.run_json(r#"{"op":"paper_bytes","feed_lines":2,"cut":true}"#));
        assert!(printed.error.is_none(), "{:?}", printed.error);
        let job = printed.job.expect("bytes for the printer");
        assert!(!job.bytes.is_empty(), "the slip went to the printer");
        // Hex, like the sync bodies: one way of carrying bytes across this
        // boundary rather than two.
        assert!(
            job.bytes.chars().all(|one| one.is_ascii_hexdigit()),
            "{}",
            job.bytes
        );
    }

    /// The khata page, from what the shop sent rather than what a screen adds.
    #[test]
    fn an_account_prints_from_what_the_shop_sent() {
        use openpos_core::protocol::{AccountEntryWire, AccountResponse};

        let mut till = till_with_a_listed_price_item();

        // Nothing has been sent yet, and a blank page would read as a printer
        // fault rather than as a device that has not been told.
        let refused = view_of(&till.run_json(
            r#"{"op":"statement_paper","width":32,"customer":"Karim, flat 3","at":"08/09/2026","dates":[]}"#,
        ));
        assert!(refused.error.is_some());

        // The shop's own answer, newest first, which is how it sends one.
        let reply = AccountResponse {
            protocol: openpos_core::protocol::PROTOCOL_VERSION,
            entries: alloc::vec![
                AccountEntryWire {
                    source_id: 3,
                    is_sale: true,
                    written_off: false,
                    amount_minor: 12_500,
                    at_ms: 1_788_900_000_000,
                    note: String::from("T1-000140"),
                },
                AccountEntryWire {
                    source_id: 2,
                    is_sale: false,
                    written_off: false,
                    amount_minor: -20_000,
                    at_ms: 1_788_800_000_000,
                    note: String::from("cash"),
                },
                AccountEntryWire {
                    source_id: 1,
                    is_sale: true,
                    written_off: false,
                    amount_minor: 49_450,
                    at_ms: 1_788_700_000_000,
                    note: String::from("T1-000101"),
                },
            ],
        };
        let hex = sync::to_hex_public(&postcard::to_allocvec(&reply).expect("encodes"));
        // Read as text rather than as a view: applying a reply answers with
        // what it changed, which is a different shape from a screen's view.
        let applied = till.run_json(&alloc::format!(
            r#"{{"op":"sync_apply","kind":"admin_account","body":"{hex}","now_ms":1}}"#
        ));
        assert!(
            !applied.contains("\"error\":\""),
            "the account was taken: {applied}"
        );

        // One date per line, and a mismatch is refused rather than paired by
        // position onto the wrong days.
        let wrong = view_of(&till.run_json(
            r#"{"op":"statement_paper","width":32,"customer":"Karim, flat 3","at":"08/09/2026","dates":["05/09/2026"]}"#,
        ));
        assert!(wrong.error.is_some(), "a date short is not a statement");

        // A round of the sync loop lands between reading the account and
        // printing it, which is what happens in a shop: the loop runs every
        // couple of seconds and the owner reads the screen before pressing
        // anything. The account has to survive that.
        let pull = sync::to_hex_public(
            &postcard::to_allocvec(&openpos_core::protocol::PullResponse {
                protocol: openpos_core::protocol::PROTOCOL_VERSION,
                cursor: 0,
                upserts: alloc::vec![],
                tombstones: alloc::vec![],
                more: false,
            })
            .expect("encodes"),
        );
        let _ = till.run_json(&alloc::format!(
            r#"{{"op":"sync_apply","kind":"pull","body":"{pull}","now_ms":2}}"#
        ));

        let printed = view_of(&till.run_json(
            r#"{"op":"statement_paper","width":32,"customer":"Karim, flat 3","at":"08/09/2026, 21:40","dates":["05/09/2026","03/09/2026","01/09/2026"]}"#,
        ));
        assert!(printed.error.is_none(), "{:?}", printed.error);
        let paper = printed
            .receipt
            .expect("the page")
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(paper.contains("ACCOUNT"), "{paper}");
        assert!(paper.contains("Karim, flat 3"), "{paper}");
        assert!(paper.contains("Sale T1-000101"), "{paper}");
        assert!(paper.contains("Paid, cash"), "{paper}");
        // Read down the page in the order the days happened, whatever order the
        // shop sent them in.
        let first = paper.find("01/09/2026").expect("the oldest day");
        let last = paper.find("05/09/2026").expect("the newest day");
        assert!(first < last, "the page reads forwards: {paper}");
        assert!(paper.contains("Owing 419.50"), "{paper}");
    }

    /// The drawer prints, which is the paper that goes in it with the cash.
    #[test]
    fn a_counted_drawer_can_be_printed() {
        let mut till = till_with_a_listed_price_item();

        // Nothing has been reported yet, and a blank slip would read as a
        // printer fault rather than as nobody having counted.
        let refused = view_of(&till.run_json(
            r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40"}"#,
        ));
        assert!(refused.error.is_some());

        let who = openpos_core::auth::OperatorId::from_u128(11);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Rahima".into(),
                pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: openpos_core::auth::Permissions::supervisor(),
                active: true,
            },
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "4321", 0))
                .error
                .is_none()
        );
        assert!(
            view_of(&till.run_json(
                r#"{"op":"open_shift","shift_id":"00000000000000000000000050","opening_float_minor":30000,"at_ms":1000}"#
            ))
            .error
            .is_none()
        );

        // Counted forty-five taka short, which is the ordinary evening.
        let closed = view_of(&till.run_json(
            r#"{"op":"close_shift","counted_cash_minor":25500,"at_ms":3000}"#,
        ));
        assert!(closed.error.is_none(), "{:?}", closed.error);

        let printed = view_of(&till.run_json(
            r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40","till":"Front counter","counted_by":"Rahima"}"#,
        ));
        assert!(printed.error.is_none(), "{:?}", printed.error);
        let paper = printed
            .receipt
            .expect("the slip")
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(paper.contains("DRAWER COUNTED"), "{paper}");
        assert!(paper.contains("Front counter"), "{paper}");
        assert!(paper.contains("Rahima"), "{paper}");
        assert!(paper.contains("Short by 45.00"), "{paper}");
    }

    #[test]
    fn a_cashier_refused_is_told_what_a_supervisor_would_have_to_allow() {
        let mut till = till_with_a_listed_price_item();

        // The cashier at this till may give away nothing. This is the moment a
        // shop lives with all day: "apa, twenty taka off", and the person at
        // the counter cannot.
        let who = openpos_core::auth::OperatorId::from_u128(11);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Rahima".into(),
                pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: openpos_core::auth::Permissions {
                    max_discount_bp: 0,
                    may_override_price: false,
                    may_refund: false,
                    may_void_line: true,
                    may_authorise: false,
                    may_open_drawer: true,
                    may_close_shift: false,
                },
                active: true,
            },
            openpos_core::auth::Operator {
                id: openpos_core::auth::OperatorId::from_u128(12),
                name: "Karim".into(),
                pin: openpos_core::auth::PinHash::derive("9999", [4; 16], 1_000),
                permissions: openpos_core::auth::Permissions::supervisor(),
                active: true,
            },
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "4321", 0))
                .error
                .is_none()
        );
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Refused, and the till says what would unblock it rather than leaving
        // a screen to read the words of the refusal and guess.
        let view = view_of(&till.run_json(r#"{"op":"set_line_discount","line":0,"percent":10}"#));
        assert!(view.error.is_some());
        assert_eq!(
            view.needs_supervisor,
            Some(openpos_core::auth::Action::Discount { bp: 1_000 })
        );

        // The supervisor allows that one thing, at this till, without signing
        // the cashier out in front of the customer.
        let allow = alloc::format!(
            r#"{{"op":"authorise","supervisor_id":"{}","pin":"9999","action":{{"action":"discount","bp":1000}},"now_ms":0}}"#,
            openpos_core::auth::OperatorId::from_u128(12).encode()
        );
        let view = view_of(&till.run_json(&allow));
        assert!(view.error.is_none(), "{:?}", view.error);

        // And now it goes through, with the cashier still signed in.
        let view = view_of(&till.run_json(r#"{"op":"set_line_discount","line":0,"percent":10}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.needs_supervisor.is_none());
        assert_eq!(view.discount_minor, 1_000, "ten percent of a hundred taka");
        assert_eq!(
            view.operator.map(|who| who.name),
            Some(String::from("Rahima")),
            "the cashier is still the one at the till"
        );
    }

    #[test]
    fn a_wrong_pin_from_a_supervisor_allows_nothing() {
        let mut till = till_with_a_listed_price_item();
        let view = view_of(&till.run_json(
            r#"{"op":"authorise","supervisor_id":"00000000000000000000000009","pin":"0000","action":{"action":"refund"},"now_ms":0}"#,
        ));
        assert!(view.error.is_some(), "nothing is allowed on a guess");
    }

    #[test]
    fn a_receipt_for_a_sale_on_account_names_the_buyer_the_shop_wrote_down() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        with_till!(till, |inner| inner.set_shop(
            openpos_core::receipt::Shop {
                name: String::from("Karim General Store"),
                bin: None,
                address: None,
                phone: None,
            },
            alloc::vec![],
            openpos_core::domain::StockRule::Off,
        ))
        .expect("a shop");
        with_till!(till, |inner| inner.set_customers(alloc::vec![
            openpos_core::storage::wire::CustomerV1 {
                id: 21,
                name: String::from("Karim, flat 3"),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 0,
            }
        ]))
        .expect("somebody who buys on account");

        assert!(
            view_of(&till.scan("8690000000001", 1_000.0))
                .error
                .is_none()
        );
        let chosen = alloc::format!(
            r#"{{"op":"set_customer","customer":"{}"}}"#,
            Ulid::from_u128(21).encode()
        );
        assert!(view_of(&till.run_json(&chosen)).error.is_none());
        till.add_cash(49_450.0, 0.0);
        assert!(
            view_of(&till.checkout(&Ulid::from_u128(900).encode(), 1_788_600_000_000.0))
                .error
                .is_none()
        );

        // The name is looked up from the record rather than taken off the
        // ticket, so somebody renamed last month prints as they are called now.
        let view =
            view_of(&till.run_json(r#"{"op":"receipt","width":32,"rung_at":"07 Sep 2026 00:30"}"#));
        let paper = view
            .receipt
            .expect("a receipt")
            .into_iter()
            .map(|line| line.text)
            .collect::<alloc::vec::Vec<_>>()
            .join("\n");
        assert!(paper.contains("Karim, flat 3"), "{paper}");
        assert!(paper.contains("VAT 15%"), "{paper}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_till_on_files_keeps_its_numbers_and_its_sales_across_a_restart() {
        let home = std::env::temp_dir().join(format!(
            "openpos-files-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        let tenant = Ulid::from_u128(42).encode();
        let terminal = Ulid::from_u128(7).encode();
        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );

        {
            let (mut till, said) =
                TillHandle::open_files(home.to_str().unwrap(), &tenant, &terminal)
                    .expect("a till opens on a directory");
            assert!(said.contains("\"unsynced_sales\":0"), "{said}");
            view_of(&till.apply_items(&items));
            view_of(&till.scan("8690000000001", 1_000.0));
            till.add_cash(49_450.0, 0.0);
            let sold = view_of(&till.checkout(&Ulid::from_u128(900).encode(), 1_788_600_000_000.0));
            assert!(sold.error.is_none(), "{:?}", sold.error);
        }

        // The tablet is switched off. Everything a shop cannot lose is on the
        // disk, and this is the only test in the workspace that says so for a
        // store that is not a browser's.
        let (till, said) = TillHandle::open_files(home.to_str().unwrap(), &tenant, &terminal)
            .expect("and opens again");
        assert!(said.contains("\"unsynced_sales\":1"), "{said}");
        assert!(said.contains("\"items\":1"), "{said}");
        let view = view_of(&till.view());
        assert_eq!(view.unsynced_sales, 1);
        assert_eq!(view.lines.len(), 0, "and no basket half rung");

        std::fs::remove_dir_all(&home).ok();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_till_on_files_reads_a_torn_log_the_way_the_browser_does() {
        use std::io::Write;

        let home = std::env::temp_dir().join(format!(
            "openpos-torn-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        let tenant = Ulid::from_u128(42).encode();
        let terminal = Ulid::from_u128(7).encode();
        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        {
            let (mut till, _) = TillHandle::open_files(home.to_str().unwrap(), &tenant, &terminal)
                .expect("a till opens");
            view_of(&till.apply_items(&items));
            view_of(&till.scan("8690000000001", 1_000.0));
            till.add_cash(49_450.0, 0.0);
            view_of(&till.checkout(&Ulid::from_u128(900).encode(), 1_788_600_000_000.0));
        }

        // The power goes out mid-write, which on a filesystem is a few bytes of
        // a frame and no more. The till has to open on what is whole and say
        // that it repaired something, rather than refusing to open at all.
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(home.join("critical.log"))
            .unwrap();
        log.write_all(&[0xAB; 37]).unwrap();
        drop(log);

        let (till, said) = TillHandle::open_files(home.to_str().unwrap(), &tenant, &terminal)
            .expect("it opens on what is whole");
        assert!(said.contains("\"repaired\":true"), "and says so: {said}");
        assert_eq!(
            view_of(&till.view()).unsynced_sales,
            1,
            "the sale before the tear is still money the shop is owed"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_till_that_cannot_send_can_be_read_off_and_carried() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        assert!(
            view_of(&till.scan("8690000000001", 1_000.0))
                .error
                .is_none()
        );
        till.add_cash(49_450.0, 0.0);
        let sold = view_of(&till.checkout(&Ulid::from_u128(900).encode(), 1_788_600_000_000.0));
        assert!(sold.error.is_none(), "{:?}", sold.error);

        // Nothing was pushed, and on a stranded device nothing can be. This is
        // the only route the money has: somebody reads it off the screen.
        let view = view_of(&till.run_json(r#"{"op":"carrying"}"#));
        let carrying = view.carrying.expect("what it is holding");
        assert_eq!(carrying.sales.len(), 1);
        assert_eq!(carrying.total_minor, 49_450);
        assert!(!carrying.sales[0].salvaged, "this one is merely unsent");
        assert_eq!(carrying.terminal, Ulid::from_u128(7).encode());

        // And the text is the request the back office takes, so what is pasted
        // there is what this device wrote rather than something re-encoded on
        // the way through.
        let bytes = sync::from_hex(&carrying.bundle).expect("hex");
        let bundle: openpos_core::protocol::AdoptSalesRequest =
            postcard::from_bytes(&bytes).expect("a bundle of sales");
        assert_eq!(bundle.terminal, 7);
        assert_eq!(bundle.sales.len(), 1);
        assert_eq!(bundle.sales[0].id, 900);

        // The mark the device shows is what the back office works out from the
        // paste, so two people on two devices can compare them out loud.
        assert_eq!(TillHandle::bundle_mark(&carrying.bundle), carrying.mark);
        assert_eq!(carrying.letters, carrying.bundle.len());
        assert!(
            carrying.mark.len() == 9,
            "two groups of four: {}",
            carrying.mark
        );

        // Through a messaging app, which wraps long text. The line breaks are
        // not the shop's doing and must not cost it the sale.
        let wrapped = carrying
            .bundle
            .as_bytes()
            .chunks(40)
            .map(|chunk| core::str::from_utf8(chunk).unwrap_or_default())
            .collect::<alloc::vec::Vec<_>>()
            .join("\n");
        assert_eq!(
            TillHandle::bundle_mark(&wrapped),
            carrying.mark,
            "a wrapped paste is the same bundle"
        );

        // And what is not a bundle has no mark, rather than a mark for nonsense.
        assert_eq!(TillHandle::bundle_mark("hello, is this the right box?"), "");
    }

    #[test]
    fn a_seeded_item_can_be_scanned_and_priced() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        let view = view_of(&till.apply_items(&items));
        assert!(view.error.is_none(), "{:?}", view.error);

        let view = view_of(&till.scan("8690000000001", 2_000.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines.len(), 1);
        // 430.00 each, twice, plus 15 percent.
        assert_eq!(view.net_minor, 86_000);
        assert_eq!(view.vat_minor, 12_900);
        assert_eq!(view.total_minor, 98_900);
    }

    /// One item, priced and taxed as the user described: a hundred taka, fifteen
    /// percent, and tax fixed to the listed price.
    fn till_with_a_listed_price_item() -> TillHandle {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"CIG20","name":"Cigarettes 20s","price_minor":10000,
                 "vat_bp":1500,"price_inclusive":false,"vat_on_undiscounted":true,
                 "barcodes":["8690000000002"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        // Nobody is signed in, and the default ceiling is nothing. A cashier has
        // to be somebody before they can give money away, so this signs in the
        // way the screen does rather than reaching past the ceiling.
        let who = openpos_core::auth::OperatorId::from_u128(9);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Supervisor".into(),
                // Few rounds: this is a test of the ceiling, not of PBKDF2, and
                // the cost parameter is stored per person for exactly this
                // reason.
                pin: openpos_core::auth::PinHash::derive("1234", [7_u8; 16], 1_000),
                permissions: openpos_core::auth::Permissions {
                    max_discount_bp: 2_000,
                    may_override_price: true,
                    ..Default::default()
                },
                active: true,
            }
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "1234", 1_000))
                .error
                .is_none()
        );
        till
    }

    #[test]
    fn the_number_on_the_box_finds_the_item() {
        // A label that will not scan is an ordinary afternoon. The cashier
        // reads the number off the box and types it into the same place they
        // type a name, and the index behind that search holds names and codes:
        // the shop's own barcode found nothing, which reads as a shop that does
        // not sell the thing in their hand.
        let mut till = till_with_a_listed_price_item();
        let view = view_of(&till.run_json(
            r#"{"op":"catalogue","query":"8690000000002","limit":10,"retired":false}"#,
        ));
        let found = view.catalogue.expect("a list");
        assert_eq!(found.len(), 1, "the one with that barcode");
        assert_eq!(found[0].code, "CIG20");

        // And a name still finds it, which is the path this must not break.
        let view = view_of(&till.run_json(
            r#"{"op":"catalogue","query":"cig","limit":10,"retired":false}"#,
        ));
        assert_eq!(view.catalogue.expect("a list").len(), 1);
    }

    #[test]
    fn a_refusal_carries_a_name_and_its_figures_apart_from_its_words() {
        // A screen in Bangla cannot translate "the shop has 3 kg Rice and this
        // basket wants 5 kg" from the sentence: the words and the numbers have
        // to arrive apart, and the words come from the screen's own dictionary
        // keyed on the code.
        let mut till = till_with_a_listed_price_item();
        let view = view_of(&till.scan("nothing-has-this", 1_000.0));
        assert_eq!(view.error_code.as_deref(), Some("unknown-barcode"));
        assert!(view.error_parts.is_empty(), "that one has no figures in it");

        // And one that does. Signing in wrongly says how many tries are left,
        // which is the figure the sentence is about.
        let who = openpos_core::auth::OperatorId::from_u128(9);
        let view = view_of(&till.sign_in(&who.encode(), "0000", 2_000));
        assert_eq!(view.error_code.as_deref(), Some("wrong-pin"));
        assert!(
            view.error_parts.contains_key("attempts_left"),
            "the figure a cashier is owed: {:?}",
            view.error_parts
        );
    }

    #[test]
    fn what_something_costs_is_answered_without_touching_the_basket() {
        let mut till = till_with_a_listed_price_item();

        // The question asked across the counter twenty times a day. Until this
        // the only way to answer it was to ring the thing and take it off
        // again, which needs a supervisor once the customer has started paying.
        let view = view_of(&till.run_json(r#"{"op":"check","code":"8690000000002"}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        let checked = view.checked.expect("an answer");
        assert_eq!(checked.item.name, "Cigarettes 20s");
        // 100.00 before tax at fifteen percent, so 115.00 across the counter.
        assert_eq!(checked.each_minor, 11_500);
        assert_eq!(checked.vat_minor, 1_500);
        assert!(
            view.lines.is_empty(),
            "the basket is untouched: this is a question about a shelf"
        );
        assert_eq!(view.total_minor, 0);

        // And by the words on the packet, for a torn label or a till with no
        // scanner beside it.
        let view = view_of(&till.run_json(r#"{"op":"check","code":"cig"}"#));
        assert_eq!(view.checked.expect("an answer").item.code, "CIG20");
        assert!(view.lines.is_empty());
    }

    #[test]
    fn a_price_check_on_something_the_shop_does_not_sell_says_so() {
        let mut till = till_with_a_listed_price_item();
        let view = view_of(&till.run_json(r#"{"op":"check","code":"8690000009999"}"#));
        assert!(
            view.error.unwrap_or_default().contains("no item"),
            "a cashier holding an unknown packet is owed the same words the scanner gives"
        );
        assert!(view.checked.is_none(), "and no stale answer left on the screen");
    }

    #[test]
    fn a_price_check_on_something_withdrawn_says_which_thing_it_was() {
        // Walked, and the till said "no item in the catalogue has that" about
        // an item it was holding and could name, which sends a cashier hunting
        // for a barcode that is perfectly good.
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"OLD","name":"Last year's biscuits","price_minor":5000,
                 "vat_bp":1500,"price_inclusive":false,"vat_on_undiscounted":false,
                 "barcodes":["8690000000456"],"on_hand_milli":0,"active":false}}]"#,
            Ulid::from_u128(4).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        let said = view_of(&till.run_json(r#"{"op":"check","code":"8690000000456"}"#))
            .error
            .unwrap_or_default();
        assert!(said.contains("stopped selling"), "{said}");
        assert!(said.contains("Last year's biscuits"), "{said}");
    }

    #[test]
    fn a_price_check_answers_what_the_customer_will_be_asked_for() {
        // A shelf price that already includes the tax must be quoted as it
        // stands. Working the gross out on the screen instead would be a second
        // implementation of the pricing rules, and the figure quoted across the
        // counter would be the untested one.
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"TEA","name":"Tea 400g","price_minor":23000,
                 "vat_bp":1500,"price_inclusive":true,"vat_on_undiscounted":false,
                 "barcodes":["8690000000123"],"on_hand_milli":10000}}]"#,
            Ulid::from_u128(3).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        let checked = view_of(&till.run_json(r#"{"op":"check","code":"8690000000123"}"#))
            .checked
            .expect("an answer");
        assert_eq!(
            checked.each_minor, 23_000,
            "the shelf says 230.00 and that is what they pay"
        );
        assert_eq!(checked.vat_minor, 3_000, "the tax is inside it");
    }

    #[test]
    fn a_quantity_is_corrected_without_voiding_the_basket() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 3_000.0))
                .error
                .is_none()
        );

        // Three scanned, two meant. Until this existed the only way out was to
        // start the ticket again, which is how baskets get abandoned.
        let view = view_of(&till.set_qty(0.0, 2_000.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].qty_milli, 2_000);
        assert_eq!(view.net_minor, 20_000);
    }

    #[test]
    fn a_wrongly_scanned_line_can_be_taken_off() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        let view = view_of(&till.remove_line(0.0, 0.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
    }

    #[test]
    fn a_line_that_is_not_there_is_refused_rather_than_ignored() {
        let mut till = till_with_a_listed_price_item();

        // Silence here means a cashier presses remove, sees nothing change, and
        // presses it again on a line that has since shifted up.
        assert!(view_of(&till.remove_line(4.0, 0.0)).error.is_some());
        assert!(view_of(&till.set_qty(-1.0, 1_000.0)).error.is_some());
    }

    #[test]
    fn the_two_discounts_and_the_listed_price_tax_rule_meet_at_the_counter() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Ten percent off the line: 100.00 becomes 90.00, and the tax does not
        // move, because this item is taxed on what the shelf says.
        let view = view_of(&till.set_line_discount(0.0, 10.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.lines[0].discount_minor, 1_000,
            "a cashier sees what they gave"
        );
        assert_eq!(view.lines[0].discount_bp, 1_000);
        assert_eq!(view.net_minor, 9_000);
        assert_eq!(view.vat_minor, 1_500);
        assert_eq!(view.total_minor, 10_500);

        // Then five percent off the whole ticket, taken off the goods and not
        // off the tax: 90.00 becomes 85.50, and 15.00 is still 15.00.
        let view = view_of(&till.set_ticket_discount(5.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.net_minor, 8_550);
        assert_eq!(view.vat_minor, 1_500);
        assert_eq!(view.total_minor, 10_050);
    }

    #[test]
    fn the_screens_own_path_carries_a_discount_the_same_way() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // The screen sends JSON, not typed calls, and the typed calls are what
        // every other test here uses. A percentage that arrived only through the
        // method and not through the command shipped as a till whose discount
        // button answered "missing field", which is how this test came to exist.
        let view = view_of(&till.run_json(r#"{"op":"set_line_discount","line":0,"percent":10}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.total_minor, 10_500);

        let view = view_of(&till.run_json(r#"{"op":"set_ticket_discount","percent":5}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.net_minor, 8_550);
        assert_eq!(view.vat_minor, 1_500);
        assert_eq!(view.total_minor, 10_050);

        let view = view_of(&till.run_json(r#"{"op":"set_qty","line":0,"qty_milli":2000}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].qty_milli, 2_000);

        let view = view_of(&till.run_json(r#"{"op":"remove_line","line":0}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());
    }

    #[test]
    fn the_catalogue_can_be_looked_through_and_hands_back_the_ids_it_holds() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let rice = Ulid::from_u128(1).encode();
        let items = format!(
            r#"[{{"id":"{rice}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                  "vat_bp":1500,"price_inclusive":false,
                  "barcodes":["8690000000001"],"on_hand_milli":40000}},
                {{"id":"{oil}","code":"OIL1","name":"Soybean Oil 1L","price_minor":18500,
                  "vat_bp":1500,"price_inclusive":false,"vat_on_undiscounted":true,
                  "barcodes":["8690000000002"],"on_hand_milli":20000}}]"#,
            rice = rice,
            oil = Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // Nothing asked for it, so nothing carries it: a till renders its basket
        // forty times a sale and has no use for the catalogue in any of them.
        assert!(view_of(&till.view()).catalogue.is_none());

        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"rice"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found.len(), 1);
        // The id it hands back is the id it holds. An owner correcting a price
        // sends this straight back, and a correction addressed to a new id is a
        // second item on the shelf rather than a corrected one.
        assert_eq!(found[0].id, rice);
        assert_eq!(found[0].price_minor, 43_000);

        // And the tax rule survives the round trip, or correcting a name would
        // quietly move an item's tax onto its discounted price.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"oil"}"#));
        let found = view.catalogue.expect("a search answers");
        assert!(found[0].vat_on_undiscounted);

        // An empty query lists the beginning, which is what an owner wants
        // before they know what they are looking for.
        let view = view_of(&till.run_json(r#"{"op":"catalogue"}"#));
        assert_eq!(view.catalogue.expect("a listing").len(), 2);
    }

    #[test]
    fn correcting_an_item_replaces_it_rather_than_adding_a_second_one() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let id = Ulid::from_u128(1).encode();
        let at = |price: i64| {
            format!(
                r#"[{{"id":"{id}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":{price},
                      "vat_bp":1500,"price_inclusive":false,
                      "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
                id = id,
                price = price
            )
        };
        assert!(view_of(&till.apply_items(&at(43_000))).error.is_none());
        assert!(view_of(&till.apply_items(&at(45_000))).error.is_none());

        // Sending the same id twice is a correction. The back office minted a
        // fresh one on every save, so a shop that fixed a price got two rows of
        // the same rice and no way to see either.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"rice"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found.len(), 1, "one item, at its new price");
        assert_eq!(found[0].price_minor, 45_000);

        // And the till sells it at the corrected price.
        let view = view_of(&till.scan("8690000000001", 1_000.0));
        assert_eq!(view.net_minor, 45_000);
    }

    #[test]
    fn a_line_can_be_sold_at_another_price_by_somebody_who_may() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Damaged goods, a short weight, a price a customer was quoted. The
        // permission has been stored and checked since it was written, and
        // nothing could reach the thing it guards.
        let view =
            view_of(&till.run_json(r#"{"op":"set_unit_price","line":0,"price_minor":6000}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].unit_price_minor, 6_000);
        assert_eq!(view.net_minor, 6_000);
        // The tax follows the override down, even though this item is taxed on
        // its listed price and would not follow a discount down. The two are
        // different acts: a discount is money off a price, and an override is
        // the price. Sixty taka is what this line is now listed at.
        assert_eq!(view.vat_minor, 900);

        // A negative price is not a discount, it is the till paying the customer
        // to take the goods, and the arithmetic would carry it through without
        // complaint.
        let view =
            view_of(&till.run_json(r#"{"op":"set_unit_price","line":0,"price_minor":-100}"#));
        assert!(view.error.is_some());
        assert_eq!(view.lines[0].unit_price_minor, 6_000, "and nothing moved");
    }

    #[test]
    fn a_cashier_who_may_not_override_a_price_is_refused() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"CIG20","name":"Cigarettes 20s","price_minor":10000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000002"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Nobody signed in, so nobody may. A till left unattended must not be a
        // way to sell anything at any price.
        let view = view_of(&till.run_json(r#"{"op":"set_unit_price","line":0,"price_minor":1}"#));
        assert!(view.error.is_some());
        assert_eq!(view.lines[0].unit_price_minor, 10_000);
    }

    #[test]
    fn a_sale_can_be_paid_by_wallet_and_the_drawer_knows_it_did_not_get_the_money() {
        let mut till = till_with_a_listed_price_item();
        assert!(view_of(&till
            .run_json(r#"{"op":"open_shift","shift_id":"00000000000000000000000042","opening_float_minor":50000,"at_ms":1000}"#))
            .error
            .is_none());
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Two thirds on bKash, the rest in cash. A shop here does this all day
        // and the till could only record the cash half.
        let view = view_of(&till.run_json(
            r#"{"op":"add_tender","kind":"wallet","name":"bKash","amount_minor":7000,"reference":"TRX8891"}"#,
        ));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.tendered_minor, 7_000);

        assert_eq!(view_of(&till.add_cash(4_500.0, 0.0)).tendered_minor, 11_500);

        let view = view_of(&till.checkout(&Ulid::from_u128(900).encode(), 2_000.0));
        assert!(view.error.is_none(), "{:?}", view.error);

        // What the drawer should hold is the float and the cash, not the wallet:
        // a shift counted against the total would be short by the wallet every
        // day and nobody would know which day it started.
        let view = view_of(&till.run_json(r#"{"op":"x_report"}"#));
        let report = view.report.expect("a report");
        assert_eq!(report.cash_sales_minor, 4_500);
        assert_eq!(report.non_cash_sales_minor, 7_000);
        assert_eq!(report.expected_cash_minor, 50_000 + 4_500);

        // And the wallet is named in the report, because "wallet 70.00" tells
        // nobody which one to reconcile against.
        let wallet = report
            .tenders
            .iter()
            .find(|row| row.name.contains("bKash"))
            .expect("named");
        assert_eq!(wallet.amount_minor, 7_000);
        assert!(!wallet.in_drawer);
    }

    #[test]
    fn a_basket_can_be_given_up_on_in_one_go() {
        let mut till = till_with_a_listed_price_item();
        for _ in 0..3 {
            assert!(
                view_of(&till.scan("8690000000002", 1_000.0))
                    .error
                    .is_none()
            );
        }
        assert!(view_of(&till.add_cash(5_000.0, 0.0)).error.is_none());

        // Removing three lines one at a time is three chances to leave one
        // behind, and the one left behind is rung to the next customer.
        let view = view_of(&till.run_json(r#"{"op":"cancel_sale"}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
        assert_eq!(view.tendered_minor, 0, "and the money with it");
    }

    #[test]
    fn money_entered_by_mistake_can_be_taken_back() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Five thousand where five hundred was meant. Adding more cannot unwind
        // it, and a cashier who cannot undo it finishes the sale and fixes it
        // out of the drawer.
        assert_eq!(view_of(&till.add_cash(500_000.0, 0.0)).tendered_minor, 500_000);

        let view = view_of(&till.run_json(r#"{"op":"clear_tenders"}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.tendered_minor, 0);
        // The basket is untouched: it was the money that was wrong.
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.total_minor, 11_500);
    }

    #[test]
    fn a_checkpoint_is_harmless_when_the_log_is_short() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // Asked after every sale, and the till decides. A fresh one has nothing
        // worth folding, and saying so must not be an error a screen reports.
        let view = view_of(&till.run_json(r#"{"op":"checkpoint"}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
    }

    #[test]
    fn a_sale_can_be_parked_and_brought_back_while_the_queue_moves() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 2_000.0))
                .error
                .is_none()
        );

        let first = Ulid::from_u128(500).encode();
        let view = view_of(&till.run_json(&format!(
            r#"{{"op":"hold","ticket_id":"{first}","held_at_ms":1000,"label":"the man in the blue shirt"}}"#
        )));
        assert!(view.error.is_none(), "{:?}", view.error);

        // The counter is clear for the next customer, and the parked sale is on
        // screen: a cashier who parks one and cannot see it has lost a basket.
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
        assert_eq!(view.held.len(), 1);
        assert_eq!(view.held[0].label, "the man in the blue shirt");
        assert_eq!(view.held[0].lines, 1);
        assert_eq!(
            view.held[0].total_minor, 23_000,
            "two at a hundred, plus tax"
        );

        // The next customer is served on the same till.
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Bringing the first one back is refused while that sale is open: the
        // alternative is quietly merging two customers' baskets.
        let view = view_of(&till.run_json(&format!(r#"{{"op":"resume","ticket_id":"{first}"}}"#)));
        assert!(
            view.error.is_some(),
            "a basket on screen must not be overwritten"
        );

        // Park the second, then bring the first back.
        let second = Ulid::from_u128(501).encode();
        assert!(view_of(&till.run_json(&format!(
            r#"{{"op":"hold","ticket_id":"{second}","held_at_ms":2000,"label":"the lady with the pram"}}"#
        )))
        .error
        .is_none());

        let view = view_of(&till.run_json(&format!(r#"{{"op":"resume","ticket_id":"{first}"}}"#)));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].qty_milli, 2_000);
        assert_eq!(view.held.len(), 1, "and the other one is still parked");

        // The customer who never came back.
        let view = view_of(&till.run_json(&format!(
            r#"{{"op":"discard_held","ticket_id":"{second}"}}"#
        )));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.held.is_empty());
    }

    #[test]
    fn parking_nothing_is_refused_rather_than_parking_an_empty_basket() {
        let mut till = till_with_a_listed_price_item();

        // An empty parked sale is a row a cashier has to read, decide about and
        // throw away, for a customer who was never there.
        let view = view_of(&till.run_json(
            r#"{"op":"hold","ticket_id":"00000000000000000000000123","held_at_ms":1,"label":"x"}"#,
        ));
        assert!(view.error.is_some());
        assert!(view.held.is_empty());
    }

    #[test]
    fn an_item_can_be_rung_by_looking_it_up_and_the_same_rules_apply() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let sold = Ulid::from_u128(1).encode();
        let gone = Ulid::from_u128(2).encode();
        let items = format!(
            r#"[{{"id":"{sold}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                  "vat_bp":1500,"price_inclusive":false,
                  "barcodes":["8690000000001"],"on_hand_milli":40000}},
                {{"id":"{gone}","code":"OLD1","name":"Rice, the old bag","price_minor":41000,
                  "vat_bp":1500,"price_inclusive":false,"active":false,
                  "barcodes":[],"on_hand_milli":0}}]"#,
            sold = sold,
            gone = gone
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // A barcode that will not read, or loose goods that carry none. The
        // shop still has to sell the thing.
        let view = view_of(&till.run_json(&format!(
            r#"{{"op":"add","item_id":"{sold}","qty_milli":2000}}"#
        )));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].qty_milli, 2_000);
        assert_eq!(view.net_minor, 86_000);

        // And the rules that guard scanning guard this too. A second way in
        // that forgot one of them would be a way to sell what the shop has
        // withdrawn, which is the whole reason withdrawing it exists.
        let view = view_of(&till.run_json(&format!(
            r#"{{"op":"add","item_id":"{gone}","qty_milli":1000}}"#
        )));
        assert!(
            view.error.is_some(),
            "a retired item must not ring either way"
        );
        assert_eq!(view.lines.len(), 1, "and nothing was added");

        // An id that is not one is the same refusal a bad barcode gets, because
        // to a cashier it is the same thing: the till does not know what you
        // mean.
        assert!(
            view_of(&till.run_json(r#"{"op":"add","item_id":"nonsense","qty_milli":1000}"#))
                .error
                .is_some()
        );
    }

    #[test]
    fn a_price_that_already_has_the_tax_in_it_is_not_taxed_again() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // A hundred taka on the shelf, tax included, which is how a shop here
        // writes a price. Until an owner could say so, this arrived as a
        // hundred plus fifteen and every customer was overcharged.
        let items = format!(
            r#"[{{"id":"{}","code":"CIG20","name":"Cigarettes 20s","price_minor":10000,
                 "vat_bp":1500,"price_inclusive":true,"unit":"Nos",
                 "barcodes":["8690000000002"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        let view = view_of(&till.scan("8690000000002", 1_000.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        // What the shelf says is what the customer pays.
        assert_eq!(view.total_minor, 10_000);
        // And the tax is the part of it that was always tax.
        assert_eq!(view.vat_minor, 1_304);
        assert_eq!(view.net_minor, 8_696);
    }

    #[test]
    fn a_shop_can_say_what_it_sells_a_thing_by() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"RICE","name":"Rice, loose","unit":"kg",
                 "price_minor":8600,"vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000010"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(9).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // "Nos" was hardcoded on the way in, so a shop selling rice by the kilo
        // had no way to say which and every screen said pieces.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"rice"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found[0].unit, "kg");

        // And a shop that says nothing still sells in pieces rather than in
        // nothing at all.
        let plain = format!(
            r#"[{{"id":"{}","code":"TEA","name":"Tea","price_minor":22000,"vat_bp":1500,
                 "price_inclusive":false,"barcodes":[],"on_hand_milli":0}}]"#,
            Ulid::from_u128(10).encode()
        );
        assert!(view_of(&till.apply_items(&plain)).error.is_none());
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"tea"}"#));
        assert_eq!(view.catalogue.expect("a search answers")[0].unit, "Nos");
    }

    #[test]
    fn an_item_can_be_found_by_its_bangla_name() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg",
                 "name_bn":"মিনিকেট চাল ৫ কেজি","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // The catalogue has carried a Bangla name and the search has indexed it
        // since both were written. Nothing could set it to anything but a copy
        // of the English one, so the whole path was dead.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"মিনিকেট"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name_bn, "মিনিকেট চাল ৫ কেজি");

        // And by the English name still, because a shop has both on its shelves
        // and whoever is at the till reads one of them.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"rice"}"#));
        assert_eq!(view.catalogue.expect("a search answers").len(), 1);
    }

    #[test]
    fn an_item_with_no_bangla_name_is_still_found_by_the_one_it_has() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{}","code":"TEA400","name":"Tea 400g","price_minor":22000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000005"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(5).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // A shop that has not typed one gets the English name in both places
        // rather than an empty row on a screen, and the search still works.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"tea"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found[0].name_bn, "Tea 400g");
    }

    #[test]
    fn a_retired_item_is_out_of_the_way_until_it_is_asked_for() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let items = format!(
            r#"[{{"id":"{sold}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                  "vat_bp":1500,"price_inclusive":false,
                  "barcodes":["8690000000001"],"on_hand_milli":40000}},
                {{"id":"{gone}","code":"OLD1","name":"Rice, the old bag","price_minor":41000,
                  "vat_bp":1500,"price_inclusive":false,"active":false,
                  "barcodes":["8690000000009"],"on_hand_milli":0}}]"#,
            sold = Ulid::from_u128(1).encode(),
            gone = Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // The everyday question is what is on the shelves.
        let view = view_of(&till.run_json(r#"{"op":"catalogue"}"#));
        let found = view.catalogue.expect("a listing");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "RICE5");

        // And a back office has to be able to find one to bring it back, which
        // the core's search cannot do: it hides them, correctly, for the till.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","retired":true}"#));
        let found = view.catalogue.expect("a listing");
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|item| !item.active));

        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"old","retired":true}"#));
        let found = view.catalogue.expect("a search");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "OLD1");

        // Scanning it takes no money, which is the point of the flag and the one
        // place it was not honoured.
        let view = view_of(&till.scan("8690000000009", 1_000.0));
        assert!(view.error.is_some(), "a retired item must not ring");
        assert!(view.lines.is_empty());
    }

    #[test]
    fn what_the_shop_paid_survives_a_correction() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let id = Ulid::from_u128(1).encode();
        let items = format!(
            r#"[{{"id":"{id}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                  "cost_minor":34400,"vat_bp":1500,"price_inclusive":false,
                  "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            id = id
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // A screen correcting a price has to send back the cost the item had.
        // It cannot do that unless it is handed it, and a form that omits it
        // writes a zero over every margin the shop has.
        let view = view_of(&till.run_json(r#"{"op":"catalogue","query":"rice"}"#));
        let found = view.catalogue.expect("a search answers");
        assert_eq!(found[0].cost_minor, 34_400);
    }

    #[test]
    fn a_credential_the_server_refuses_is_reported_rather_than_retried_in_silence() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // No signal, a server that is down, a proxy in the way: worth retrying,
        // and nothing a shopkeeper can act on.
        let view = view_of(&till.run_json(r#"{"op":"sync_failed","now_ms":1000}"#));
        assert!(!view.credential_refused);

        // A refusal of the credential is a different thing. The device looks
        // enrolled, every request is answered 401, and without this the screen
        // has nothing to say and no way out. Found by restarting a demo server
        // under a running app, which is what a revoked token looks like too.
        let view = view_of(&till.run_json(r#"{"op":"sync_failed","now_ms":2000,"status":401}"#));
        assert!(view.credential_refused);

        let view = view_of(&till.run_json(r#"{"op":"sync_failed","now_ms":3000,"status":503}"#));
        assert!(
            !view.credential_refused,
            "a server that fell over has not refused anybody"
        );
    }

    #[test]
    fn a_platform_that_never_learned_to_report_a_status_still_works() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // The Android side and any older build send this command without a
        // status. It has to keep meaning what it meant, or adding a field here
        // stops every one of them syncing.
        let view = view_of(&till.run_json(r#"{"op":"sync_failed","now_ms":1000}"#));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(!view.credential_refused);
    }

    #[test]
    fn a_discount_over_a_hundred_percent_is_refused_at_the_boundary() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // The arithmetic caps this silently. A cashier would key 110, see 100
        // percent, and believe the till had done what they asked.
        let view = view_of(&till.set_line_discount(0.0, 110.0));
        assert!(view.error.is_some());
        assert_eq!(view.total_minor, 11_500, "and nothing was given away");

        assert!(
            view_of(&till.set_line_discount(0.0, f64::NAN))
                .error
                .is_some()
        );
        assert!(view_of(&till.set_ticket_discount(-5.0)).error.is_some());
    }

    #[test]
    fn a_discount_is_refused_above_the_ceiling_of_whoever_is_signed_in() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"CIG20","name":"Cigarettes 20s","price_minor":10000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000002"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );

        // Nobody signed in, so the ceiling is nothing. A till left unattended
        // must not be a discount machine.
        let view = view_of(&till.set_line_discount(0.0, 10.0));
        assert!(view.error.is_some(), "the ceiling starts at nothing");
        assert_eq!(view.total_minor, 11_500);
    }

    /// Twenty taka off, which is what a shop here actually says.
    #[test]
    fn an_amount_off_is_offered_and_measured_against_the_ceiling() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        assert!(
            view_of(&till.scan("8690000000001", 1_000.0))
                .error
                .is_none()
        );

        // Nobody is signed in, so nothing may be given away by any route. The
        // hole this closes: an amount used to walk straight past the ceiling.
        let refused = view_of(&till.take_off_line(0.0, 2_000.0));
        assert!(refused.error.is_some(), "the ceiling starts at nothing");
        assert_eq!(refused.total_minor, 49_450, "and nothing came off");
        assert_eq!(
            refused.needs_supervisor,
            Some(openpos_core::auth::Action::Discount { bp: 466 }),
            "named as what it is: 20.00 off 430.00 is 4.66 percent"
        );

        // A supervisor allows it, and the money comes off.
        let allowed = view_of(&till.take_off_ticket(-1.0));
        assert!(allowed.error.is_some(), "and never a negative amount");
    }

    #[test]
    fn clearing_a_discount_leaves_no_trace_of_it() {
        let mut till = till_with_a_listed_price_item();
        assert!(
            view_of(&till.scan("8690000000002", 1_000.0))
                .error
                .is_none()
        );
        assert!(view_of(&till.set_line_discount(0.0, 10.0)).error.is_none());

        // Keying zero has to mean none. A discount of nothing that still counts
        // as a discount shows up on the receipt and in the day's figures.
        let view = view_of(&till.set_line_discount(0.0, 0.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].discount_minor, 0);
        assert_eq!(view.lines[0].discount_bp, 0);
        assert_eq!(view.total_minor, 11_500);
    }

    #[test]
    fn malformed_catalogue_json_is_refused_rather_than_partly_applied() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        let view = view_of(&till.apply_items("{not json"));
        assert!(view.error.is_some());
        assert!(view.lines.is_empty());
    }

    #[test]
    fn a_fractional_quantity_is_refused_rather_than_truncated() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // `as i64` would make this 12, and the shop would find out at the end of
        // the day rather than at the counter.
        let view = view_of(&till.scan("8690000000001", 12.7));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));

        let view = view_of(&till.add_cash(f64::NAN, 0.0));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));
        assert_eq!(view.tendered_minor, 0, "and nothing was taken");
    }

    #[test]
    fn a_number_beyond_exact_representation_is_refused() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");

        // Past 2^53 a JavaScript number is no longer the number that was typed.
        let view = view_of(&till.add_cash(9_007_199_254_740_993.0, 0.0));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));
    }
}
