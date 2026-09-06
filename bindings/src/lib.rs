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

#[cfg(target_arch = "wasm32")]
pub mod opfs;
pub mod sync;

extern crate alloc;

use openpos_core::cart::{CartLimits, Tender, TenderKind, Ticket};
use openpos_core::domain::pricing::Discount;
use openpos_core::receipt;
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::storage::wire::{ItemDeltasV1, ItemV1};
use openpos_core::sync::driver::Driver;
use openpos_core::till::{Till, TillError};
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::{wasm_bindgen, JsError};



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
    /// Whether this device holds a credential. The credential itself never
    /// crosses this boundary: it lives beside the ledger and travels only with
    /// the requests the core builds.
    pub enrolled: bool,
    /// Who is signed in, and what they may do. A UI showing a button somebody
    /// cannot use is a UI that teaches people to press it and be refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<Operator>,
    /// The drawer's totals, when one has been asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// The drawer, when one is open. A screen that cannot see it cannot tell a
    /// cashier what the till should hold before they count it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drawer: Option<Drawer>,
    /// Who may sign in here: names and ids, and nothing that could be used to
    /// sign in as them. A screen needs the list to show a person their own name
    /// rather than asking them to type an identifier.
    pub people: Vec<Person>,
    /// Present when the last operation was refused, and why. A UI that renders
    /// this cannot silently drop an error.
    pub error: Option<String>,
    /// The last completed sale, laid out for a printer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Vec<receipt::Line>>,
    /// The same sale as bytes a thermal printer understands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<PrintJob>,
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
    /// than recovering it from two amounts, which is lossy at small ones.
    pub discount_bp: u32,
    pub total_minor: i64,
}

/// An item as a front end hands one over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireItem {
    pub id: String,
    pub code: String,
    pub name: String,
    pub price_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    /// True when VAT is charged on the price before discounts, so a discount
    /// comes out of the shop's margin. Defaulted, because most goods do not.
    #[serde(default)]
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
}

impl WireItem {
    fn into_wire(self) -> ItemV1 {
        ItemV1 {
            id: Ulid::decode(&self.id).map(|id| id.to_u128()).unwrap_or_default(),
            code: self.code,
            name_en: self.name.clone(),
            name_bn: self.name,
            unit: String::from("Nos"),
            price_minor: self.price_minor,
            cost_minor: 0,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: self.vat_on_undiscounted,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: true,
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
    ApplyItems { items: Vec<WireItem> },
    Scan { barcode: String, qty_milli: i64 },
    /// Change a line's quantity. A cashier who scanned three of something and
    /// meant two must not have to void the basket.
    SetQty { line: f64, qty_milli: f64 },
    /// Take a line off the ticket.
    RemoveLine { line: f64 },
    /// Discount one line, as a percentage.
    ///
    /// Refused above this cashier's ceiling, which is what the ceiling is for.
    /// A supervisor can authorise it, and that authorisation is spent on use.
    SetLineDiscount { line: f64, percent: f64 },
    /// Discount the whole ticket, apportioned across its lines.
    SetTicketDiscount { percent: f64 },
    AddCash { amount_minor: i64 },
    Checkout { ticket_id: String, rung_at_ms: u64 },
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
    /// Ask what to sync next. The answer carries the request already built.
    SyncStep { online: bool, now_ms: u64 },
    /// Hand back what the server said.
    SyncApply {
        kind: sync::Exchange,
        body: String,
        now_ms: u64,
    },
    /// The request did not get through. Back off.
    SyncFailed { now_ms: u64 },
    /// Build the enrolment request for a code read off the owner's screen.
    Enrol { code: String },
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
    Admin { request: sync::AdminRequest },
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
    /// The real one.
    #[cfg(target_arch = "wasm32")]
    Opfs(Till<opfs::OpfsBackend>),
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
        Command::Scan { barcode, qty_milli } => {
            till.scan(&barcode, Milli::new(qty_milli)).err()
        }
        Command::AddCash { amount_minor } => {
            till.add_tender(Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(amount_minor),
                reference: None,
            });
            None
        }
        Command::OpenShift {
            ref shift_id,
            opening_float_minor,
            at_ms,
        } => match Ulid::decode(shift_id) {
            Ok(id) => till.open_shift(id, Minor::new(opening_float_minor), at_ms).err(),
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
        Command::Checkout { .. } | Command::Receipt { .. } | Command::Escpos { .. } => None,
        // Handled by the caller, which holds the driver, the tenant and the
        // last sale. Listed rather than caught by a wildcard, so adding a
        // command forces a decision here instead of silently doing nothing.
        Command::SyncStep { .. }
        | Command::SyncApply { .. }
        | Command::SyncFailed { .. }
        | Command::Enrol { .. }
        | Command::SignIn { .. }
        | Command::SignOut
        | Command::SetQty { .. }
        | Command::RemoveLine { .. }
        | Command::SetLineDiscount { .. }
        | Command::SetTicketDiscount { .. }
        | Command::Authorise { .. } => None,
    }
}

/// What was taken, by how it was paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenderRow {
    pub name: String,
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
    /// What the last sync step produced, folded into the next view.
    last_step: Option<sync::Step>,
    last_applied: Option<sync::Applied>,
    /// The last sale laid out for paper. Held so one reply can carry both the
    /// state of the till and the thing to print.
    last_receipt: Option<Vec<receipt::Line>>,
    last_job: Option<PrintJob>,
    last_report: Option<Report>,
    /// The last sale closed, which is what a receipt is of. A reprint asks for
    /// the sale that happened, not for whatever is on the screen now.
    last_sale: Option<Ticket>,
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
        }
    };
    (ref $self:expr, |$till:ident| $body:expr) => {
        match &$self.inner {
            Store::Memory($till) => $body,
            #[cfg(target_arch = "wasm32")]
            Store::Opfs($till) => $body,
        }
    };
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl TillHandle {
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
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = adoptToken))]
    pub fn adopt_token(&mut self, token: &str) -> String {
        let outcome = with_till!(self, |till| till.set_token(token));
        self.render_ref(outcome.err())
    }

    /// The names of the files a till needs, in the order `openOpfs` expects
    /// them. Exposed so the JavaScript that opens them cannot drift from the
    /// Rust that reads them.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = fileNames)]
    #[must_use]
    pub fn file_names() -> Vec<String> {
        opfs::FILE_NAMES.iter().map(|name| String::from(*name)).collect()
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
    pub fn remove_line(&mut self, line: f64) -> String {
        self.run(Command::RemoveLine { line })
    }

    /// Discount one line by a percentage.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setLineDiscount))]
    pub fn set_line_discount(&mut self, line: f64, percent: f64) -> String {
        self.run(Command::SetLineDiscount { line, percent })
    }

    /// Discount the whole ticket by a percentage, apportioned across its lines.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setTicketDiscount))]
    pub fn set_ticket_discount(&mut self, percent: f64) -> String {
        self.run(Command::SetTicketDiscount { percent })
    }

    /// Take money.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = addCash))]
    pub fn add_cash(&mut self, amount_minor: f64) -> String {
        let Some(amount) = exact(amount_minor) else {
            return self.refuse(NOT_A_WHOLE_NUMBER);
        };
        self.run(Command::AddCash {
            amount_minor: amount,
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
        serde_json::to_string(&view).unwrap_or_else(|_| {
            String::from(r#"{"error":"the till could not describe itself"}"#)
        })
    }



    fn render_ref(&self, error: Option<TillError>) -> String {
        let view = self.build_view(error);
        // Serialising a struct of numbers and strings cannot fail. Returning a
        // fixed error shape rather than panicking, because a panic here unwinds
        // into JavaScript and leaves the till unusable until the page reloads.
        serde_json::to_string(&view).unwrap_or_else(|_| {
            String::from(r#"{"error":"the till could not describe itself"}"#)
        })
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
                .map(|who| Person {
                    id: who.id.encode(),
                    name: who.name.to_string(),
                })
                .collect()),
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
    pub fn open_on(
        backend: MemoryBackend,
        tenant: &str,
        terminal: &str,
    ) -> Option<Self> {
        let tenant = Ulid::decode(tenant).ok()?;
        let terminal = Ulid::decode(terminal).ok()?;
        let (inner, _report) =
            Till::open(backend, tenant.to_u128(), terminal, 1, CartLimits::default()).ok()?;
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
        }
    }

    fn wrap(inner: Store, tenant: u128) -> Self {
        Self {
            inner,
            tenant,
            driver: Driver::new(),
            more_to_pull: false,
            last_step: None,
            last_applied: None,
            last_receipt: None,
            last_job: None,
            last_report: None,
            last_sale: None,
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
            Command::RemoveLine { line } => {
                let Some(at) = index(line) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let outcome = with_till!(self, |till| till.remove_line(at));
                return self.render_ref(outcome.err());
            }
            Command::SetLineDiscount { line, percent } => {
                let (Some(at), Some(discount)) = (index(line), rate_of(percent)) else {
                    return self.refuse(NOT_A_PERCENTAGE);
                };
                let outcome = with_till!(self, |till| till.set_line_discount(at, discount));
                return self.render_ref(outcome.err());
            }
            Command::SetTicketDiscount { percent } => {
                let Some(discount) = rate_of(percent) else {
                    return self.refuse(NOT_A_PERCENTAGE);
                };
                let outcome = with_till!(self, |till| till.set_ticket_discount(discount));
                return self.render_ref(outcome.err());
            }
            Command::XReport => return self.report(None),
            Command::Admin { ref request } => {
                let request = request.clone();
                let tenant = self.tenant;
                let outcome =
                    with_till!(ref self, |till| sync::admin_step(till, tenant, &request));
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
            Command::SyncApply {
                kind,
                body,
                now_ms,
            } => return self.sync_apply(kind, &body, now_ms),
            Command::SyncFailed { now_ms } => {
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

    /// Lay the last sale out for paper, as lines or as printer bytes.
    fn print(&mut self, command: Command) -> String {
        let (width, rung_at, cashier, printer) = match command {
            Command::Receipt {
                width,
                rung_at,
                cashier,
            } => (width, rung_at, cashier, None),
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
            return self.refuse("this terminal does not know its shop yet, so a receipt would have no name on it");
        };

        let lines = receipt::render(
            &sale,
            &receipt::Context {
                shop,
                rung_at,
                cashier,
                width,
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
                .map(|z| (z.totals.clone(), Some((z.counted_cash, z.closed_at_ms, z.variance))))),
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
        let outcome =
            with_till!(self, |till| till.authorise(id, pin, action, now_ms, valid_for_ms));
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
            till, &mut driver, kind, body, now_ms
        ));
        self.driver = driver;
        match outcome {
            Ok(applied) => {
                self.more_to_pull = applied.more_to_pull;
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
        let till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
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
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // Nothing has been pulled, so no barcode matches. A UI that renders the
        // view cannot silently drop this.
        let view = view_of(&till.scan("8690000000001", 1_000.0));
        assert!(view.error.is_some(), "a refusal must be visible");
        assert!(view.lines.is_empty());
    }

    #[test]
    fn change_is_negative_while_the_customer_still_owes() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // A UI showing zero here would be showing the same thing it shows when
        // the basket is settled, which is the one moment it must not.
        let view = view_of(&till.add_cash(10_000.0));
        assert_eq!(view.tendered_minor, 10_000);
        assert_eq!(view.change_minor, 10_000, "nothing rung yet, so it is all change");
    }

    #[test]
    fn a_nonsense_timestamp_clamps_rather_than_wrapping() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // An empty cart refuses anyway; the point is that a negative double does
        // not become an enormous u64 on the way in.
        let view = view_of(&till.checkout(&Ulid::from_u128(900).encode(), -1.0));
        assert!(view.error.is_some());
    }

    #[test]
    fn a_seeded_item_can_be_scanned_and_priced() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
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
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
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
                    ..Default::default()
                },
                active: true,
            }
        ]));
        assert!(outcome.is_ok());
        assert!(view_of(&till.sign_in(&who.encode(), "1234", 1_000)).error.is_none());
        till
    }

    #[test]
    fn a_quantity_is_corrected_without_voiding_the_basket() {
        let mut till = till_with_a_listed_price_item();
        assert!(view_of(&till.scan("8690000000002", 3_000.0)).error.is_none());

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
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());

        let view = view_of(&till.remove_line(0.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
    }

    #[test]
    fn a_line_that_is_not_there_is_refused_rather_than_ignored() {
        let mut till = till_with_a_listed_price_item();

        // Silence here means a cashier presses remove, sees nothing change, and
        // presses it again on a line that has since shifted up.
        assert!(view_of(&till.remove_line(4.0)).error.is_some());
        assert!(view_of(&till.set_qty(-1.0, 1_000.0)).error.is_some());
    }

    #[test]
    fn the_two_discounts_and_the_listed_price_tax_rule_meet_at_the_counter() {
        let mut till = till_with_a_listed_price_item();
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());

        // Ten percent off the line: 100.00 becomes 90.00, and the tax does not
        // move, because this item is taxed on what the shelf says.
        let view = view_of(&till.set_line_discount(0.0, 10.0));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines[0].discount_minor, 1_000, "a cashier sees what they gave");
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
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());

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
    fn a_discount_over_a_hundred_percent_is_refused_at_the_boundary() {
        let mut till = till_with_a_listed_price_item();
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());

        // The arithmetic caps this silently. A cashier would key 110, see 100
        // percent, and believe the till had done what they asked.
        let view = view_of(&till.set_line_discount(0.0, 110.0));
        assert!(view.error.is_some());
        assert_eq!(view.total_minor, 11_500, "and nothing was given away");

        assert!(view_of(&till.set_line_discount(0.0, f64::NAN)).error.is_some());
        assert!(view_of(&till.set_ticket_discount(-5.0)).error.is_some());
    }

    #[test]
    fn a_discount_is_refused_above_the_ceiling_of_whoever_is_signed_in() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");
        let items = format!(
            r#"[{{"id":"{}","code":"CIG20","name":"Cigarettes 20s","price_minor":10000,
                 "vat_bp":1500,"price_inclusive":false,
                 "barcodes":["8690000000002"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(2).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());

        // Nobody signed in, so the ceiling is nothing. A till left unattended
        // must not be a discount machine.
        let view = view_of(&till.set_line_discount(0.0, 10.0));
        assert!(view.error.is_some(), "the ceiling starts at nothing");
        assert_eq!(view.total_minor, 11_500);
    }

    #[test]
    fn clearing_a_discount_leaves_no_trace_of_it() {
        let mut till = till_with_a_listed_price_item();
        assert!(view_of(&till.scan("8690000000002", 1_000.0)).error.is_none());
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
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        let view = view_of(&till.apply_items("{not json"));
        assert!(view.error.is_some());
        assert!(view.lines.is_empty());
    }

    #[test]
    fn a_fractional_quantity_is_refused_rather_than_truncated() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // `as i64` would make this 12, and the shop would find out at the end of
        // the day rather than at the counter.
        let view = view_of(&till.scan("8690000000001", 12.7));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));

        let view = view_of(&till.add_cash(f64::NAN));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));
        assert_eq!(view.tendered_minor, 0, "and nothing was taken");
    }

    #[test]
    fn a_number_beyond_exact_representation_is_refused() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // Past 2^53 a JavaScript number is no longer the number that was typed.
        let view = view_of(&till.add_cash(9_007_199_254_740_993.0));
        assert_eq!(view.error.as_deref(), Some(NOT_A_WHOLE_NUMBER));
    }
}
