//! What a front end renders, and what it can ask for.
//!
//! The shapes that cross the boundary in both directions: the one view every
//! call hands back, the lists inside it, and the commands a screen sends. They
//! are here rather than beside the dispatch because they are the contract, and
//! a contract is easier to read when it is not interleaved with the code that
//! honours it.
//!
//! JSON at this boundary rather than the postcard used on disk and on the wire.
//! Those two are positional and exist to be compact and stable across versions;
//! this boundary is neither. It is crossed by a UI compiled from the same
//! commit, and a shape a person can read in a debugger is worth more here than
//! bytes saved.

extern crate alloc;

use alloc::collections::BTreeMap;

use openpos_core::cart::TenderKind;
use openpos_core::receipt;
use openpos_core::domain::pricing::Discount;
use openpos_core::ids::Ulid;
use openpos_core::money::Bp;
use openpos_core::storage::wire::ItemV1;
use openpos_core::till::TillError;
use serde::{Deserialize, Serialize};

use crate::sync;

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
    /// What still has to change hands, with its sign: positive while the
    /// customer owes the shop, negative while the shop owes the customer.
    /// From the core, because the screen used to do this subtraction itself.
    pub outstanding_minor: i64,
    /// Whether the money on the basket covers it, answered by the same rule
    /// that closes the sale. A screen deciding this for itself is a second
    /// answer, and the customer sees the one that is not the drawer's.
    pub settled: bool,
    pub is_refund: bool,
    pub receipt_numbers_left: u64,
    pub unsynced_sales: usize,
    /// Sales closed with no receipt number left to give them.
    ///
    /// The till says how many numbers it has left, which says the shape of this
    /// problem and not its size: a shop cannot tell one sale waiting for a
    /// number from forty, and forty is a morning's trading with an inspector's
    /// question attached to it. The shop numbers them when a block arrives.
    pub unnumbered_sales: u64,
    /// Whether this device's running drawer figure has fallen behind what it
    /// has sold.
    ///
    /// The core has set this since the drawer was written and nothing could
    /// read it, so no screen could say it. It happens when the arithmetic that
    /// adds a sale into the open drawer fails, which is at figures no shop
    /// reaches: the sale is durable and the receipt is already printing by
    /// then, so nothing after the commit is allowed to turn it into a failure,
    /// and what it costs instead is this device's expected-cash figure until
    /// the app is opened again, which rebuilds it by replaying the same sales.
    ///
    /// Unreachable is not the same as unimportant. On the one evening it
    /// happens, somebody counts a drawer against a figure that is quietly
    /// wrong, and a variance nobody can explain is how a shop stops believing
    /// its till.
    pub drawer_is_behind: bool,
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
    /// The languages this shop offers its own staff, by the codes the screens
    /// use. Empty means the shop has never said, which is all of them.
    ///
    /// On the view rather than fetched, because a screen decides what language
    /// it is drawn in on every render and with the internet down. A screen that
    /// only consulted this when drawing the button that switches would strand a
    /// device somebody had left in Bangla, in a shop that has since turned
    /// Bangla off, in a language with the way out removed.
    #[serde(default)]
    pub languages: Vec<String>,
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
    /// What the till made of something said to it, when it was asked.
    ///
    /// Its own field rather than sharing the catalogue's, because "the cashier
    /// searched" and "the till heard" are different facts, and a screen that
    /// cannot tell them apart cannot show what it thought it heard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heard: Option<Heard>,
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
    /// Whether this device has been round the shelf once, so its shop's rule
    /// about the shelf means anything yet.
    ///
    /// A till learns what the shelves hold two hundred items at a time, five
    /// minutes apart, and until it has been round it holds a figure for some
    /// items and nothing for the rest. It says nothing about the shelf in that
    /// window, which is right, and a shopkeeper who has just turned the rule on
    /// and is watching the counter should be told that is what they are seeing
    /// rather than left thinking the rule does not work.
    #[serde(default)]
    pub shelf_known: bool,
    /// What this shop asked its tills to do about the shelf: nothing, say so,
    /// or refuse it. Carried so a screen can explain its own silence while the
    /// figures are still arriving.
    #[serde(default)]
    pub stock_rule: u8,
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
pub(crate) fn parts_of(error: &TillError) -> BTreeMap<String, String> {
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
            say(
                "outstanding",
                receipt::money_of(outstanding.get()).to_string(),
            );
        }
        TillError::Cart(CartError::DiscountAboveCeiling { requested, ceiling }) => {
            say("requested", alloc::format!("{}", *requested as f64 / 100.0));
            say("ceiling", alloc::format!("{}", *ceiling as f64 / 100.0));
        }
        TillError::Cart(CartError::NegativePrice { price }) => {
            say("price", receipt::money_of(price.get()).to_string());
        }
        TillError::Cart(CartError::NegativeQuantity { qty }) => {
            say("qty", receipt::quantity_of(qty.get()));
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
        | TillError::Auth(AuthError::UnknownOperator | AuthError::AuthorisationExpired)
        | TillError::Shift(ShiftError::StillOpen | ShiftError::NoReason | ShiftError::Money(_))
        | TillError::Journal(_)
        | TillError::Sync(_)
        | TillError::Wire(_) => {}
    }
    parts
}

/// A refusal the shop's server gave, in the shape every refusal here takes.
///
/// The sentence travels beside the code rather than instead of it. A screen
/// that has never heard of the code says the sentence, which is what a back
/// office one release behind its server is: imperfect rather than silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub code: String,
    pub parts: BTreeMap<String, String>,
    pub said: String,
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
    pub(crate) fn of(item: &openpos_core::Item) -> Self {
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

    pub(crate) fn into_wire(self) -> ItemV1 {
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

pub(crate) fn alloc_empty() -> Vec<u128> {
    Vec::new()
}

/// A line number, or nothing when it is not one. JavaScript has one number
/// type, so a caller can pass 1.5 or -1 and mean nothing by it.
pub(crate) fn index(line: f64) -> Option<usize> {
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
pub(crate) fn rate_of(percent: f64) -> Option<Discount> {
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
pub(crate) const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A discount larger than the price is not a discount, and the arithmetic caps
/// it silently, which would leave a cashier believing they gave one thing and
/// the customer another.
pub(crate) const NOT_A_PERCENTAGE: &str = "a discount must be between nothing and a hundred percent";
pub(crate) const NOT_AN_AMOUNT: &str = "an amount off is a whole number of poisha, and not a negative one";

pub(crate) const NOT_A_WHOLE_NUMBER: &str =
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
pub(crate) fn exact(value: f64) -> Option<i64> {
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

/// How many things a spoken phrase may come back with.
///
/// Smaller than a typed search's, because this is a list a cashier reads at a
/// counter with somebody waiting, not a screen an owner browses. Past about
/// five, a list stops being read and starts being guessed at, which is the
/// failure the whole of `core::voice` is arranged to avoid.
const fn default_heard_limit() -> usize {
    5
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
    /// What the cashier said, as the platform heard it.
    ///
    /// Read-only, on purpose and permanently. This is the first input the till
    /// takes where the till, rather than a printed barcode or a person's finger,
    /// decides which item was meant. It answers with what it thinks and never
    /// puts anything on a ticket: the screen commits with `Add`, which is the
    /// same press the lookup list has always taken.
    ///
    /// The transcript is text and nothing else. Whether it came from a
    /// microphone, a model, a keyboard or a test is the platform's business, and
    /// keeping it that way is what lets the whole of this be exercised without a
    /// microphone in the room.
    Heard {
        transcript: String,
        #[serde(default = "default_heard_limit")]
        limit: usize,
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
    /// Bring one line of a receipt back, at what it was charged.
    ///
    /// For a refund built from the paper the customer is holding rather than by
    /// scanning the goods again: scanning prices them out of today's catalogue,
    /// so a basket sold with something off comes back at full price and the
    /// shop gives the discount away twice.
    ///
    /// `came_off_minor` and `was_on_minor` are the whole line as the paper has
    /// it. The core takes the share of the discount that belongs to what is
    /// coming back, so that the division is done in the same arithmetic as the
    /// rest of the money rather than in the screen's.
    ReturnLine {
        item_id: String,
        qty_milli: f64,
        charged_each_minor: f64,
        came_off_minor: f64,
        was_on_milli: f64,
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
    /// A receipt printed a second time.
    ///
    /// Not gated on anything: a customer who lost their copy is the ordinary
    /// reason. It is written into the trail, because a second copy of a receipt
    /// is a second piece of paper somebody can hand over.
    Reprinted {
        now_ms: u64,
    },
    /// Try the shop now, rather than waiting out the backoff.
    ///
    /// A fallback and not the path: the loop syncs on its own and a shop should
    /// never have to press anything. It exists for the one moment the loop is
    /// wrong, which is a shopkeeper who has just restarted the router looking
    /// at a till that says it will try again in four minutes.
    TryNow,
    /// Open the cash drawer without selling anything.
    ///
    /// A cashier gives change for something bought next door, or puts the float
    /// in at the start of a shift. Permission-gated on the same action a cash
    /// movement is, because it is the same act: the drawer coming open with
    /// nothing on the paper to say why.
    OpenDrawer {
        now_ms: u64,
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

/// What the till made of something said to it.
///
/// Everything here is for showing, and none of it has happened. A cashier who
/// cannot see what the till thought they said has no way to learn what it
/// listens to, and no way to tell a wrong item from a misheard word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    /// The words it looked the item up by.
    pub used: Vec<String>,
    /// The words it set aside as politeness or grammar. Shown, not hidden: a
    /// till that quietly throws words away teaches nobody anything.
    pub ignored: Vec<String>,
    /// How many, when the till was willing to say. A proposal for the screen to
    /// offer, never a quantity that has been applied to anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty_milli: Option<i64>,
    /// Why there is no quantity, when a number was said and refused. In the
    /// core's words, because a screen inventing its own would be a second place
    /// the rule lives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty_note: Option<String>,
    /// What it could be, best first.
    pub candidates: Vec<WireItem>,
    /// Whether the first is worth showing on its own rather than in a list.
    /// Never a licence to ring it.
    pub sure: bool,
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
pub(crate) fn marked(bytes: &[u8]) -> alloc::string::String {
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

pub(crate) fn tender_kind_name(kind: &TenderKind) -> String {
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
    /// Money put in and taken out for a stated reason, while this drawer has
    /// been open.
    ///
    /// The screen has had the count of movements and neither total, so a
    /// cashier watching "should hold" could not see that it had gone down
    /// because somebody paid the delivery boy out of the till. The back office
    /// says it about a drawer already counted; this is the same thing said to
    /// the person standing at it.
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    /// When it was opened, by the clock of the device that opened it.
    ///
    /// The screen showed what the drawer holds and never when it started
    /// holding it. A drawer nobody closes stays open, so a cashier arriving in
    /// the morning reads "7 sales, should hold 1,500.40" of yesterday's trading
    /// and has no way to tell: the figure is right and belongs to another day,
    /// and closing it counts two days as one with a variance that means
    /// nothing. The shop's own screen has said "open since" all along, which is
    /// the wrong end of the shop to find it out from.
    pub opened_at_ms: u64,
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
pub(crate) fn person_seen(who: &openpos_core::auth::Operator) -> Person {
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
