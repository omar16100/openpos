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
/// The shapes that cross this boundary: what a screen renders, and what it can
/// ask for. Re-exported below, because they are this crate's surface and a
/// caller should not have to know which file they are written in.
pub mod shapes;
pub mod sync;

pub use shapes::*;

extern crate alloc;

use alloc::collections::BTreeMap;

use openpos_core::cart::{CartLimits, Tender, TenderKind, Ticket};
use openpos_core::domain::pricing::Discount;
use openpos_core::ids::Ulid;
use openpos_core::money::{Milli, Minor};
use openpos_core::receipt;
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::storage::wire::ItemDeltasV1;
use openpos_core::sync::driver::Driver;
use openpos_core::till::{Till, TillError};
use openpos_core::voice;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::{JsError, wasm_bindgen};

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
            till.add_tender(
                Tender {
                    kind,
                    amount: Minor::new(amount_minor),
                    reference: Some(reference.trim())
                        .filter(|value| !value.is_empty())
                        .map(Into::into),
                },
                at_ms,
            )
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
        // Answered where the till is held, because it needs the till.
        Command::OpenDrawer { .. } => None,
        // Handled by the caller, which holds the driver, the tenant and the
        // last sale. Listed rather than caught by a wildcard, so adding a
        // command forces a decision here instead of silently doing nothing.
        Command::Reprinted { .. }
        | Command::TryNow
        | Command::SyncStep { .. }
        | Command::SyncApply { .. }
        | Command::SyncFailed { .. }
        | Command::Enrol { .. }
        | Command::SignIn { .. }
        | Command::SignOut
        | Command::Catalogue { .. }
        | Command::Check { .. }
        | Command::Heard { .. }
        | Command::Carrying
        | Command::SetCustomer { .. }
        | Command::Everyone
        | Command::SetQty { .. }
        | Command::RemoveLine { .. }
        | Command::SetUnitPrice { .. }
        | Command::ReturnLine { .. }
        | Command::QuickAdd { .. }
        | Command::WriteCustomer { .. }
        | Command::SetLineDiscount { .. }
        | Command::TakeOffLine { .. }
        | Command::SetTicketDiscount { .. }
        | Command::TakeOffTicket { .. }
        | Command::Authorise { .. } => None,
    }
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
    /// Which receipt the page on the paper is of, when it is a sale at all.
    ///
    /// Kept here rather than read off the screen because the screen cannot be
    /// trusted with it and does not need to be: the same button lays out a
    /// drawer slip, a customer's account and a sale, and a reprint of the
    /// first two is a reprint of no receipt. A screen that answered this would
    /// answer it out of whatever it happened to be holding, and the answer
    /// lands in the trail a shop reads to decide whether somebody took money.
    last_receipt_no: Option<String>,
    /// How wide the paper this device last laid out was, and the words it was
    /// laid out with. Kept so a reprint can mark the same paper as a copy
    /// without being handed the whole context again: a reprint is a button, not
    /// a request carrying a layout.
    last_paper: Option<(usize, receipt::Words)>,
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
    last_heard: Option<Heard>,
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

    /// The same refusal, named and with its figures beside it.
    ///
    /// `refusalInWords` gives an English sentence and nothing else, which left
    /// the server the last place in this system that could only speak English:
    /// a save built on a stale copy, a barcode another item already holds, an
    /// item the shop has traded, a rate no till could price. Those are exactly
    /// the moments an owner needs their own language.
    ///
    /// The shape is the one every other refusal here uses: a frozen code, the
    /// figures named and already formatted, and the sentence beside them as the
    /// fallback. A screen older than the server says the sentence; one that
    /// knows the code says it in the shop's language.
    ///
    /// Empty when the body is not a refusal this build knows.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = refusalNamed))]
    #[must_use]
    pub fn refusal_named(body: &str) -> String {
        use openpos_core::protocol::ProtocolError;

        let Some(refusal) = sync::from_hex_public(body)
            .and_then(|bytes| postcard::from_bytes::<ProtocolError>(&bytes).ok())
        else {
            return String::new();
        };
        let mut parts: BTreeMap<String, String> = BTreeMap::new();
        match &refusal {
            // The same three figures whichever of the two is behind: what
            // differs is the sentence the screen builds around them, and which
            // room somebody is sent to.
            ProtocolError::UnsupportedVersion {
                requested,
                minimum,
                current,
            }
            | ProtocolError::ShopNeedsUpdating {
                requested,
                minimum,
                current,
            } => {
                parts.insert(String::from("requested"), alloc::format!("{requested}"));
                parts.insert(String::from("minimum"), alloc::format!("{minimum}"));
                parts.insert(String::from("current"), alloc::format!("{current}"));
            }
            ProtocolError::TooManyAttempts {
                retry_after_seconds,
            } => {
                parts.insert(
                    String::from("seconds"),
                    alloc::format!("{retry_after_seconds}"),
                );
            }
            ProtocolError::BarcodeInUse { barcode } => {
                parts.insert(String::from("barcode"), barcode.clone());
            }
            // Already a sentence when it was built, because what is wrong with
            // a rate is decided where the rate is read. Named all the same, so
            // a screen puts the shop's own words around it.
            ProtocolError::NotAPrice { said } => {
                parts.insert(String::from("said"), said.clone());
            }
            // The three that replaced it, each carrying its figure rather than
            // a clause about it, so a screen can put the shop's own words
            // around it instead of inside it.
            ProtocolError::RateIsNotARate { bp } => {
                parts.insert(
                    String::from("rate"),
                    alloc::format!("{}", f64::from(*bp) / 100.0),
                );
            }
            ProtocolError::PriceBelowNothing { minor } => {
                parts.insert(
                    String::from("price"),
                    openpos_core::receipt::money_of(*minor).to_string(),
                );
            }
            ProtocolError::CostBelowNothing { minor } => {
                parts.insert(
                    String::from("cost"),
                    openpos_core::receipt::money_of(*minor).to_string(),
                );
            }
            ProtocolError::UnknownTerminal
            | ProtocolError::Malformed
            | ProtocolError::Unauthenticated
            | ProtocolError::NotPermitted
            | ProtocolError::Stale
            | ProtocolError::ItemHasHistory => {}
        }
        let named = Named {
            code: String::from(refusal.code()),
            parts,
            said: alloc::format!("{refusal}"),
        };
        serde_json::to_string(&named).unwrap_or_default()
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

    /// What each role a shop can pick means, by name.
    ///
    /// Exposed so the back office asks rather than holds a copy. It held one,
    /// and the two disagreed: the screen's cashier could open the drawer and
    /// the core's could not, the screen's supervisor was capped at a fifth off
    /// and the core's at everything. Every caller of the core's pair was a
    /// test, so nothing a shop ran was inconsistent and nothing would have
    /// complained until the first one that was not.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = roles)]
    #[must_use]
    pub fn roles() -> String {
        use openpos_core::auth::{EVERY_ROLE, Permissions};

        let named: BTreeMap<String, Permissions> = EVERY_ROLE
            .iter()
            .filter_map(|role| {
                Permissions::named(role).map(|allowed| (String::from(*role), allowed))
            })
            .collect();
        // Infallible in practice: a plain map of plain fields. An empty object
        // rather than a panic if it ever is not, because a screen with no
        // presets shows a shopkeeper an empty dropdown, and a worker that
        // panicked would take the whole till with it.
        serde_json::to_string(&named).unwrap_or_else(|_| String::from("{}"))
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
        let at_ms = exact(at_ms)
            .filter(|ms| *ms >= 0)
            .unwrap_or(0)
            .unsigned_abs();
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
        let outstanding = with_till!(ref self, |till| till.outstanding().ok());
        let settled = with_till!(ref self, |till| till.settled().unwrap_or(false));

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
                unit: line.unit.to_string(),
                net_minor: line_totals.get(at).map_or(0, |computed| computed.net.get()),
                vat_minor: line_totals.get(at).map_or(0, |computed| computed.vat.get()),
                vat_bp: line.vat_rate.get(),
                supply: line.supply.as_u8(),
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
            outstanding_minor: outstanding.map_or(0, Minor::get),
            settled,
            is_refund,
            receipt_numbers_left: status.map_or(0, |s| s.receipt_numbers_left),
            unsynced_sales: status.map_or(0, |s| s.unsynced_sales),
            shop: with_till!(ref self, |till| till.shop().map(|shop| Shop {
                name: shop.name.clone(),
                bin: shop.bin.clone(),
                address: shop.address.clone(),
                phone: shop.phone.clone(),
            })),
            receipt_no: self
                .last_sale
                .as_ref()
                .and_then(|sale| sale.receipt_no.as_deref().map(String::from)),
            unnumbered_sales: status.map_or(0, |s| s.unnumbered_sales),
            drawer_is_behind: status.is_some_and(|s| s.drawer_is_behind),
            enrolled: with_till!(ref self, |till| till.token().is_some()),
            credential_refused: self.refused,
            wallets: with_till!(ref self, |till| till
                .wallets()
                .iter()
                .map(ToString::to_string)
                .collect()),
            languages: with_till!(ref self, |till| till
                .languages()
                .iter()
                .map(ToString::to_string)
                .collect()),
            // What a supervisor would have to allow, when the last thing tried
            // was refused for want of permission. The screen shows a PIN box
            // and sends this back as it stands.
            needs_supervisor: Self::blocked_by(error.as_ref()),
            needs_customer: Self::wants_customer(error.as_ref()),
            beyond_the_shelf: with_till!(ref self, |till| till.beyond_the_shelf()),
            shelf_known: with_till!(ref self, |till| till.shelf_known()),
            stock_rule: with_till!(ref self, |till| till.stock_rule().as_u8()),
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
                        address: known.address.clone(),
                    }
                })
                .collect()),
            customer: with_till!(ref self, |till| till.customer().map(|id| id.encode())),
            // Said while the customer is still standing there, which is the
            // only moment their details can be asked for. Not a refusal: the
            // goods leave the counter either way, and a till that will not
            // sell is a till a shop works around.
            buyer_wanted: openpos_core::domain::buyer_wanted_on_the_invoice(
                openpos_core::money::Minor::new(total),
            ) && with_till!(ref self, |till| till.customer().is_none()),
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
            heard: self.last_heard.clone(),
            everyone: self.last_everyone.clone(),
            carrying: self.last_carrying.clone(),
            drawer: with_till!(ref self, |till| till.shift().map(|shift| Drawer {
                open: shift.is_open(),
                opening_float_minor: shift.opening_float().get(),
                sales: shift.sales(),
                expected_cash_minor: shift.expected_cash().map_or(0, Minor::get),
                movements: shift.movements().len(),
                cash_in_minor: shift.cash_in_total().get(),
                cash_out_minor: shift.cash_out_total().get(),
                opened_at_ms: shift.opened_at_ms(),
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
            last_receipt_no: None,
            last_paper: None,
            last_job: None,
            last_report: None,
            last_account: Vec::new(),
            last_checked: None,
            last_drawer: None,
            last_sale: None,
            last_catalogue: None,
            last_heard: None,
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
            Command::OpenDrawer { now_ms } => {
                let outcome = with_till!(self, |till| till.open_the_drawer(now_ms));
                match outcome {
                    Ok(job) => {
                        // Where a receipt's bytes go, because a platform picks
                        // them up the same way: this is a job for the printer,
                        // and it happens to print nothing.
                        self.last_job = Some(PrintJob {
                            bytes: sync::to_hex_public(&job.bytes),
                            unprintable: job.unprintable,
                        });
                        return self.render_ref(None);
                    }
                    Err(refusal) => return self.render_ref(Some(refusal)),
                }
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
                ref words,
            } => {
                let at = at.clone();
                let till_named = till.clone();
                // Who counted it, from the till rather than from the caller.
                // The slip is printed at the moment of counting, so the person
                // signed in now is the person counting now, and a platform that
                // can pass a name is a platform that can pass the wrong one.
                let who = with_till!(ref self, |till| till
                    .signed_in()
                    .map(|who| who.name.to_string()));
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
            Command::ReturnLine {
                item_id,
                qty_milli,
                charged_each_minor,
                came_off_minor,
                was_on_milli,
            } => {
                let (Some(qty), Some(each), Some(off), Some(was_on)) = (
                    exact(qty_milli),
                    exact(charged_each_minor),
                    exact(came_off_minor),
                    exact(was_on_milli),
                ) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let Ok(id) = openpos_core::ids::Ulid::decode(item_id.trim()) else {
                    return self.refuse(NOT_A_WHOLE_NUMBER);
                };
                let outcome = with_till!(self, |till| till.return_line(
                    id,
                    Milli::new(qty),
                    Minor::new(each),
                    Minor::new(off),
                    Milli::new(was_on)
                ));
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
            address: None,
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
            Command::Heard {
                ref transcript,
                limit,
            } => {
                let (transcript, limit) = (transcript.clone(), limit.min(50));
                let made_of_it = with_till!(ref self, |till| {
                    let understood = voice::understand(&transcript);
                    let replica = till.replica();
                    let found = voice::resolve(replica, &understood, limit);
                    Heard {
                        used: understood.terms.iter().map(|t| t.to_string()).collect(),
                        ignored: understood.ignored.iter().map(|t| t.to_string()).collect(),
                        // Carried only when the till would stand behind it. A
                        // screen offering "1" it invented and a screen offering
                        // "3" the cashier said look the same, and only one of
                        // them is worth a press.
                        qty_milli: understood.count.map(|_| understood.quantity().get()),
                        qty_note: understood.refused.as_ref().map(alloc::string::ToString::to_string),
                        candidates: found
                            .candidates
                            .iter()
                            .filter_map(|id| replica.by_id(*id))
                            .map(WireItem::of)
                            .collect(),
                        sure: found.sure,
                    }
                });
                self.last_heard = Some(made_of_it);
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
            Command::Reprinted { now_ms } => {
                // Which receipt comes from the page that was laid out, not from
                // the caller: a platform asked for it would answer out of
                // whatever its screen was holding, and this lands in the trail.
                let of = self.last_receipt_no.clone();
                let outcome = with_till!(self, |till| till.reprinted(now_ms, of));
                // And the paper says so. The trail has always recorded a
                // reprint, where the customer holding the paper cannot see it
                // and the person handed it cannot either: two identical
                // receipts for one sale is how a refund gets claimed twice.
                // Marked once, not once per press, so a third print is still
                // one copy rather than a stack of headings.
                if outcome.is_ok()
                    && let Some(paper) = self.last_receipt.as_ref()
                    && let Some((width, words)) = self.last_paper.as_ref()
                    && !receipt::already_a_copy(paper, *width, words)
                {
                    self.last_receipt = Some(receipt::as_a_copy(paper, *width, words));
                }
                return self.render_ref(outcome.err());
            }
            Command::TryNow => {
                self.driver.try_now();
                return self.render_ref(None);
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
                self.last_receipt_no = None;
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
                // And which receipt it was rung on, already here because the
                // shop sends it with the line. Nothing extra is asked of the
                // screen for it.
                receipt_no: one.receipt_no.clone(),
            })
            .collect();
        // The shop sends an account newest first, because that is what a screen
        // shows. A person reading their own account reads down the page in the
        // order the days happened, so the paper turns it over, dates and money
        // together.
        lines.reverse();

        let shop = with_till!(ref self, |till| till.shop().cloned()).unwrap_or_default();
        self.last_receipt_no = None;
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
        // A drawer slip is of no receipt, so a reprint of it names none.
        self.last_receipt_no = None;
        self.last_receipt = Some(lines);
        self.last_job = None;
        self.render_ref(None)
    }

    /// Lay the last sale out for paper, as lines or as printer bytes.
    fn print(&mut self, command: Command) -> String {
        let (width, rung_at, words, printer) = match command {
            Command::Receipt {
                width,
                rung_at,
                words,
            } => (width, rung_at, receipt::Words::of(words), None),
            // Nothing for the thermal path: no ESC/POS code page carries
            // Bangla, so a printer is handed the English this crate defaults
            // to and `escpos::encode` says which lines it could not print.
            Command::Escpos {
                width,
                rung_at,
                feed_lines,
                cut,
            } => (
                width,
                rung_at,
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
        // A shop with no name is the same refusal as no shop at all, and it is
        // the state a brand new shop is in until somebody fills in the first
        // screen of the back office. Printing it would put a blank line where a
        // tax invoice names the supplier, which is the one line on the paper
        // nobody proof-reads, because it is their own.
        let shop = match with_till!(ref self, |till| till.shop().cloned()) {
            Some(shop) if !shop.name.trim().is_empty() => shop,
            _ => {
                return self.refuse(
                    "this terminal does not know its shop yet, so a receipt would have no name on                      it",
                );
            }
        };

        // Who served, from the sale rather than from the caller and rather than
        // from whoever is signed in now. The ticket carries the id of the person
        // who was at the till when it was rung, and the name is looked up here
        // for the same reason the customer's is: an operator is kept rather than
        // deleted so that their name still resolves on yesterday's tickets.
        //
        // A reprint hours later is the case this exists for. Taking the name
        // from the current sign-in would put the evening cashier on the morning
        // cashier's paper, and a wrong name on a receipt is worse than no name:
        // the whole use of the line is to settle who was at the counter.
        let cashier = sale.operator.and_then(|id| {
            with_till!(ref self, |till| till
                .people()
                .iter()
                .find(|who| who.id == id)
                .map(|who| who.name.to_string()))
        });

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
        let customer_bin = known.as_ref().and_then(|known| known.bin.clone());
        // And where they are, which is the third of the three a tax invoice
        // names once a supply is worth more than 25,000 taka.
        let customer_address = known.and_then(|known| known.address.clone());

        // Remembered so a reprint can mark the same paper as a copy. A reprint
        // is a button rather than a request carrying a layout, so the width and
        // the words have to come from the last time paper was laid out here.
        self.last_paper = Some((width, words.clone()));
        let lines = receipt::render(
            &sale,
            &receipt::Context {
                shop,
                rung_at,
                cashier,
                customer,
                customer_bin,
                customer_address,
                width,
                words,
            },
        );

        // The number the paper itself carries, taken from the sale it was laid
        // out from rather than from the lines, which are text by now.
        self.last_receipt_no = self
            .last_sale
            .as_ref()
            .and_then(|sale| sale.receipt_no.as_deref().map(String::from));

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
                    // Only where there were any. A screen showing "0 refunds"
                    // on every drawer teaches a supervisor to stop reading the
                    // line, and the one evening it matters it is the line they
                    // have stopped reading.
                    refunds: totals
                        .refunds
                        .filter(|refunds| refunds.count > 0)
                        .map(|refunds| usize::try_from(refunds.count).unwrap_or(usize::MAX)),
                    refunded_cash_minor: totals
                        .refunds
                        .filter(|refunds| refunds.count > 0)
                        .map(|refunds| refunds.cash.get()),
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

    pub(super) fn view_of(json: &str) -> View {
        serde_json::from_str(json).expect("the facade returns its own shape")
    }

    /// A refusal the shop's server gave reaches a screen named, with its
    /// figures, and with the English beside it.
    ///
    /// This was the last place in the system that could only speak English.
    /// The screen matched on nothing and showed the sentence, so a shop that
    /// reads Bangla read "another item you sell already has the barcode
    /// 8901234567890" at the moment it was deciding what to do about it.
    #[test]
    fn a_refusal_from_the_shop_reaches_a_screen_named() {
        use openpos_core::protocol::ProtocolError;

        let refusal = ProtocolError::BarcodeInUse {
            barcode: "8901234567890".to_owned(),
        };
        let hex = sync::to_hex_public(&postcard::to_allocvec(&refusal).expect("a refusal encodes"));
        let named: Named =
            serde_json::from_str(&TillHandle::refusal_named(&hex)).expect("it is named");

        assert_eq!(named.code, "barcode-in-use");
        assert_eq!(
            named.parts.get("barcode").map(String::as_str),
            Some("8901234567890"),
            "the barcode is a figure, not a word baked into a sentence: a screen wording this in \
             Bangla has to put it somewhere else in the sentence"
        );
        assert_eq!(
            named.said,
            alloc::format!("{refusal}"),
            "the English travels beside the code, because a screen older than the server it talks \
             to says something imperfect rather than nothing"
        );

        // A refusal with nothing to say about itself still carries a name.
        let named: Named = serde_json::from_str(&TillHandle::refusal_named(&sync::to_hex_public(
            &postcard::to_allocvec(&ProtocolError::Stale).expect("a refusal encodes"),
        )))
        .expect("it is named");
        assert_eq!(named.code, "stale");
        assert!(named.parts.is_empty());

        // A version refusal carries all three numbers, because "it needs
        // updating" without them is a shopkeeper ringing somebody to ask which
        // version.
        let named: Named = serde_json::from_str(&TillHandle::refusal_named(&sync::to_hex_public(
            &postcard::to_allocvec(&ProtocolError::UnsupportedVersion {
                requested: 1,
                minimum: 2,
                current: 3,
            })
            .expect("a refusal encodes"),
        )))
        .expect("it is named");
        assert_eq!(named.code, "device-needs-updating");
        assert_eq!(named.parts.get("requested").map(String::as_str), Some("1"));
        assert_eq!(named.parts.get("minimum").map(String::as_str), Some("2"));
        assert_eq!(named.parts.get("current").map(String::as_str), Some("3"));

        // And a body that is not a refusal this build knows says nothing, which
        // is a server one release ahead. The screen falls back to the status.
        assert!(
            TillHandle::refusal_named("ff").is_empty() || {
                let named: Named = serde_json::from_str(&TillHandle::refusal_named("ff"))
                    .expect("either nothing or a shape");
                !named.code.is_empty()
            }
        );
        assert!(TillHandle::refusal_named("not hex").is_empty());
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
        let paid =
            view_of(&till.run_json(r#"{"op":"add_tender","kind":"cash","amount_minor":10000}"#));
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
            view_of(
                &till.run_json(r#"{"op":"close_shift","counted_cash_minor":29550,"at_ms":3000}"#)
            )
            .error
            .is_none()
        );
        assert!(
            view_of(&till.run_json(
                r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40"}"#
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
                    note: String::new(),
                    receipt_no: String::from("T1-000140"),
                },
                AccountEntryWire {
                    source_id: 2,
                    is_sale: false,
                    written_off: false,
                    amount_minor: -20_000,
                    at_ms: 1_788_800_000_000,
                    note: String::from("cash"),
                    // A payment has no receipt, and the page shows the day
                    // alone against it.
                    receipt_no: String::new(),
                },
                AccountEntryWire {
                    source_id: 1,
                    is_sale: true,
                    written_off: false,
                    amount_minor: 49_450,
                    at_ms: 1_788_700_000_000,
                    note: String::new(),
                    receipt_no: String::from("T1-000101"),
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
        assert!(paper.contains("Sale"), "{paper}");
        assert!(paper.contains("Paid, cash"), "{paper}");
        // The receipt each debt was rung on, across from the day it happened.
        // Two sales of the same size on one day are otherwise two identical
        // entries on a page the customer is holding to argue from, and this is
        // the one thing on the line they may have in their own pocket.
        assert!(paper.contains("T1-000101"), "{paper}");
        assert!(paper.contains("T1-000140"), "{paper}");
        for line in paper.lines() {
            if line.contains("Paid, cash") {
                assert!(
                    !line.contains('T'),
                    "a payment has no receipt to name: {line}"
                );
            }
        }
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
        let refused =
            view_of(&till.run_json(r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40"}"#));
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
        let closed = view_of(
            &till.run_json(r#"{"op":"close_shift","counted_cash_minor":25500,"at_ms":3000}"#),
        );
        assert!(closed.error.is_none(), "{:?}", closed.error);

        let printed = view_of(&till.run_json(
            r#"{"op":"drawer_paper","width":32,"at":"08/09/2026, 21:40","till":"Front counter"}"#,
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

    /// A till stocked the way the demo shop is, names in both scripts.
    fn a_shop_that_speaks_bangla() -> TillHandle {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let goods = [
            (1_u128, "RICE5", "Rice Miniket 5kg", "মিনিকেট চাল ৫ কেজি", 43_000),
            (2, "OIL1", "Soybean Oil 1L", "সয়াবিন তেল ১ লিটার", 18_500),
            (3, "DAL1", "Masoor Dal 1kg", "মসুর ডাল ১ কেজি", 14_000),
        ];
        let items: Vec<String> = goods
            .iter()
            .map(|(seed, code, name, name_bn, price)| {
                format!(
                    r#"{{"id":"{}","code":"{code}","name":"{name}","name_bn":"{name_bn}",
                        "price_minor":{price},"vat_bp":0,"price_inclusive":false,
                        "barcodes":["869000000000{seed}"],"on_hand_milli":40000}}"#,
                    Ulid::from_u128(*seed).encode()
                )
            })
            .collect();
        let json = format!("[{}]", items.join(","));
        assert!(view_of(&till.apply_items(&json)).error.is_none());
        till
    }

    fn heard(till: &mut TillHandle, said: &str) -> Heard {
        let request = serde_json::to_string(&serde_json::json!({
            "op": "heard",
            "transcript": said,
        }))
        .expect("a request");
        view_of(&till.run_json(&request))
            .heard
            .expect("the till says what it made of it")
    }

    /// The operation both platforms get, driven the way both platforms drive it.
    #[test]
    fn something_said_at_the_counter_comes_back_as_something_to_press() {
        let mut till = a_shop_that_speaks_bangla();
        let made_of_it = heard(&mut till, "ভাই একটু চাল দাও");

        assert_eq!(made_of_it.used, ["চাল"]);
        assert_eq!(made_of_it.ignored, ["ভাই", "একটু", "দাও"]);
        assert_eq!(
            made_of_it.candidates.first().map(|item| item.code.as_str()),
            Some("RICE5")
        );
        assert!(made_of_it.sure);
        assert_eq!(made_of_it.qty_milli, None, "nothing was said about how many");
    }

    /// The promise the whole design rests on, held at the boundary rather than
    /// only inside the core: nothing said to the till reaches the ticket.
    #[test]
    fn nothing_said_to_the_till_ever_reaches_the_ticket() {
        let mut till = a_shop_that_speaks_bangla();
        for said in [
            "ভাই একটু চাল দাও",
            "তিন প্যাকেট চাল",
            "মিনিকেট চাল ৫ কেজি",
            "একশ টাকার চাল",
            "সয়াবিন তেল",
            "",
        ] {
            let request = serde_json::to_string(&serde_json::json!({
                "op": "heard", "transcript": said,
            }))
            .expect("a request");
            let view = view_of(&till.run_json(&request));
            assert!(
                view.lines.is_empty(),
                "{said:?} put something on the ticket"
            );
            assert_eq!(view.total_minor, 0, "{said:?} moved the total");
        }
    }

    /// A quantity is carried only when the till would stand behind it, and the
    /// reason travels with the refusal. A screen offering a "1" it invented and
    /// a screen offering a "3" the cashier said look identical otherwise.
    #[test]
    fn a_quantity_is_offered_only_when_the_till_would_stand_behind_it() {
        let mut till = a_shop_that_speaks_bangla();

        let counted = heard(&mut till, "তিন প্যাকেট চাল");
        assert_eq!(counted.qty_milli, Some(3_000));
        assert!(counted.qty_note.is_none());

        // The demo's own first item, read off the packet. Five bags at 430 is
        // 2,150 for a customer buying one.
        let packet = heard(&mut till, "মিনিকেট চাল ৫ কেজি");
        assert_eq!(packet.qty_milli, None, "five bags for a customer buying one");
        assert!(
            packet.qty_note.is_some_and(|note| !note.is_empty()),
            "and the screen must be able to say why"
        );
    }

    /// The press that follows is the one the lookup list has always taken, so
    /// a spoken phrase reaches a ticket by exactly the route a typed one does.
    #[test]
    fn what_was_heard_is_rung_by_the_same_press_a_looked_up_item_is() {
        let mut till = a_shop_that_speaks_bangla();
        let made_of_it = heard(&mut till, "তিন প্যাকেট চাল");
        let first = made_of_it.candidates.first().expect("something to press");

        let press = serde_json::to_string(&serde_json::json!({
            "op": "add",
            "item_id": first.id,
            "qty_milli": made_of_it.qty_milli.expect("a count was offered"),
        }))
        .expect("a request");
        let view = view_of(&till.run_json(&press));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].qty_milli, 3_000);
        assert_eq!(view.total_minor, 129_000, "three bags at 430");
    }

    /// An item the shop has stopped selling is refused in the same words a scan
    /// gets, because it goes through the same door.
    #[test]
    fn a_withdrawn_item_is_not_offered_to_something_said() {
        let mut till = a_shop_that_speaks_bangla();
        let gone = format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","name_bn":"মিনিকেট চাল ৫ কেজি",
                 "price_minor":43000,"vat_bp":0,"price_inclusive":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000,"active":false}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&gone)).error.is_none());
        assert!(heard(&mut till, "মিনিকেট চাল").candidates.is_empty());
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
            alloc::vec![],
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
            address: None,
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
        assert!(
            !paper.contains("COPY OF A PRINTED"),
            "the first one off the printer is not a copy: {paper}"
        );

        // Somebody has to be at the till to print again: a reprint goes into
        // the trail under a name, and a till with nobody signed in has no
        // receipt on its screen either.
        let who = openpos_core::auth::OperatorId::from_u128(11);
        let put = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Rahima".into(),
                pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: openpos_core::auth::Permissions::cashier(),
                active: true,
            },
        ]));
        assert!(put.is_ok());
        assert!(
            view_of(&till.sign_in(&who.encode(), "4321", 0))
                .error
                .is_none()
        );

        // Printed again, and the paper says so. The shop's trail has always
        // recorded a reprint, where the customer holding the paper cannot see
        // it and the person handed it cannot either: two identical receipts for
        // one sale is how a refund gets claimed twice, and for a tax invoice it
        // is two originals for one transaction.
        let again = view_of(&till.run_json(r#"{"op":"reprinted","now_ms":1788600001000}"#));
        assert!(again.error.is_none(), "{:?}", again.error);
        let copy = again
            .receipt
            .expect("the same paper, marked")
            .into_iter()
            .map(|line| line.text)
            .collect::<alloc::vec::Vec<_>>()
            .join("\n");
        assert!(copy.contains("COPY OF A PRINTED RECEIPT"), "{copy}");
        assert!(copy.contains("Karim, flat 3"), "and the rest of it: {copy}");
        assert!(
            copy.find("COPY OF A PRINTED") < copy.find("Karim, flat 3"),
            "at the top, where somebody looks: {copy}"
        );

        // A third press is still one copy rather than a stack of headings.
        let third = view_of(&till.run_json(r#"{"op":"reprinted","now_ms":1788600002000}"#));
        let stack = third
            .receipt
            .expect("the same paper again")
            .into_iter()
            .filter(|line| line.text.contains("COPY OF A PRINTED"))
            .count();
        assert_eq!(stack, 1, "marked once, however many times it is printed");
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
    pub(super) fn till_with_a_listed_price_item() -> TillHandle {
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
        let view =
            view_of(&till.run_json(
                r#"{"op":"catalogue","query":"8690000000002","limit":10,"retired":false}"#,
            ));
        let found = view.catalogue.expect("a list");
        assert_eq!(found.len(), 1, "the one with that barcode");
        assert_eq!(found[0].code, "CIG20");

        // And a name still finds it, which is the path this must not break.
        let view = view_of(
            &till.run_json(r#"{"op":"catalogue","query":"cig","limit":10,"retired":false}"#),
        );
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
        assert!(
            view.checked.is_none(),
            "and no stale answer left on the screen"
        );
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

    /// A refund built from the paper gives back what was paid.
    ///
    /// Rung by scanning the goods again, a line sold for 3.87 after a discount
    /// comes back at today's catalogue price of 4.30: the shop gives the
    /// discount away a second time, and if the price has moved since it gives
    /// away the difference as well. This is the same line brought back off the
    /// receipt, with the money from the paper and the tax from the item.
    #[test]
    fn goods_brought_back_off_a_receipt_come_back_at_what_was_paid() {
        let mut till =
            TillHandle::open_in_memory(&Ulid::from_u128(42).encode(), &Ulid::from_u128(7).encode())
                .expect("a till opens");
        let items = alloc::format!(
            r#"[{{"id":"{}","code":"RICE5","name":"Rice Miniket 5kg","price_minor":45000,
                 "vat_bp":1500,"price_inclusive":false,"vat_on_undiscounted":false,
                 "barcodes":["8690000000001"],"on_hand_milli":40000}}]"#,
            Ulid::from_u128(1).encode()
        );
        assert!(view_of(&till.apply_items(&items)).error.is_none());

        // Somebody who may take goods back, which is the permission a refund
        // asks for and the only one this needs.
        let who = openpos_core::auth::OperatorId::from_u128(9);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: who,
                name: "Supervisor".into(),
                pin: openpos_core::auth::PinHash::derive("1234", [7_u8; 16], 1_000),
                permissions: openpos_core::auth::Permissions {
                    may_refund: true,
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

        let view = view_of(&till.run_json(
            r#"{"op":"start_refund","original_receipt":"T1-000100","now_ms":1788600000000}"#,
        ));
        assert!(view.error.is_none(), "{:?}", view.error);

        // The shelf price has moved to 4.50 since, and the customer paid 4.30
        // less a discount of 0.43.
        let id = Ulid::from_u128(1).encode();
        let view = view_of(&till.run_json(&alloc::format!(
            r#"{{"op":"return_line","item_id":"{id}","qty_milli":1000,"charged_each_minor":43000,"came_off_minor":4300,"was_on_milli":1000}}"#
        )));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].unit_price_minor, 43_000, "what was charged");
        assert_eq!(view.lines[0].qty_milli, -1_000, "and it is coming back");
        assert_eq!(
            view.total_minor, -44_505,
            "38.70 back, and the tax that was charged on it"
        );
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
        assert_eq!(
            view_of(&till.add_cash(500_000.0, 0.0)).tendered_minor,
            500_000
        );

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

#[cfg(test)]
mod sales_waiting_for_a_number {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::tests::{till_with_a_listed_price_item, view_of};

    #[test]
    fn a_till_with_no_block_says_how_many_sales_are_waiting() {
        // A till out of receipt numbers sells anyway: the goods are going over
        // the counter either way, and the shop numbers the sale when the next
        // block arrives. What it could not say was how many were waiting, so a
        // shop reading "0 numbers" could not tell one such sale from a
        // morning's trading, which is the difference between a shrug and an
        // inspector's question.
        let mut till = till_with_a_listed_price_item();

        let before = view_of(&till.run_json(r#"{"op":"view"}"#));
        assert_eq!(before.unnumbered_sales, 0, "nothing is waiting yet");

        for at in 0..2_u128 {
            assert!(
                view_of(&till.run_json(
                    r#"{"op":"scan","barcode":"8690000000002","qty_milli":1000}"#
                ))
                .error
                .is_none()
            );
            assert!(
                view_of(&till.run_json(r#"{"op":"add_cash","amount_minor":11500,"at_ms":1}"#))
                    .error
                    .is_none()
            );
            let done = view_of(&till.run_json(&alloc::format!(
                r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1}}"#,
                openpos_core::ids::Ulid::from_u128(900 + at).encode()
            )));
            assert!(done.error.is_none(), "{:?}", done.error);
        }

        let after = view_of(&till.run_json(r#"{"op":"view"}"#));
        assert_eq!(
            after.unnumbered_sales, 2,
            "two sales are rung, sent and counted, and neither has a number on its paper"
        );
        assert_eq!(after.receipt_numbers_left, 0, "which is why");
    }
}

#[cfg(test)]
mod a_shop_with_no_name_yet {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::tests::{till_with_a_listed_price_item, view_of};
    use super::Store;

    /// A brand new shop has no name until somebody fills in the first screen
    /// of the back office, and a receipt cannot be printed before then.
    ///
    /// It used to have a name to print. A shop created by enrolling its first
    /// device was named after that device, so a brand new shop was called "a
    /// back office enrolled from the command line" and put that at the head of
    /// every receipt, which is where a tax invoice names the supplier and is
    /// the one line nobody proof-reads because it is their own.
    ///
    /// A shop starts with no name now, and two things refuse it: the core will
    /// not hold a nameless shop, so a till never has one, and the print path
    /// refuses a blank name as well as a missing shop. The back office's first
    /// screen asks for it and says a till cannot print without it, which is
    /// true and is what gets it filled in.
    #[test]
    fn a_till_will_not_print_until_the_shop_has_a_name() {
        let mut till = till_with_a_listed_price_item();
        // The core will not hold one. A shop arrives at a till from the shop's
        // own record, and a record with no name is refused here rather than
        // carried and printed: that is the first of the two locks, and the one
        // that does the work.
        let refused = with_till!(till, |inner| inner.set_shop(
            openpos_core::receipt::Shop {
                name: alloc::string::String::new(),
                bin: None,
                address: None,
                phone: None,
            },
            alloc::vec![],
            openpos_core::domain::StockRule::Off,
            alloc::vec![],
        ));
        assert!(refused.is_err(), "a shop with no name is not a shop");

        // So the till holds no shop at all, which is the state a brand new one
        // is in until somebody fills in the first screen of the back office.

        assert!(
            view_of(&till.run_json(
                r#"{"op":"scan","barcode":"8690000000002","qty_milli":1000}"#
            ))
            .error
            .is_none()
        );
        assert!(
            view_of(&till.run_json(r#"{"op":"add_cash","amount_minor":11500,"at_ms":1}"#))
                .error
                .is_none()
        );
        let done = view_of(&till.run_json(&alloc::format!(
            r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1}}"#,
            openpos_core::ids::Ulid::from_u128(950).encode()
        )));
        assert!(done.error.is_none(), "the sale is rung either way: {:?}", done.error);

        let printed = view_of(&till.run_json(
            r#"{"op":"receipt","width":32,"rung_at":"13/09/2026, 19:04"}"#,
        ));
        assert!(
            printed.error.is_some(),
            "a receipt headed with nothing is worse than no receipt"
        );
        assert!(printed.receipt.is_none());
    }
}

#[cfg(test)]
mod naming_the_buyer_on_a_big_invoice {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::tests::{till_with_a_listed_price_item, view_of};
    use super::Store;

    /// A cashier is told while the customer is still there, and not before.
    ///
    /// Section 51(1)(c): above 25,000 taka the invoice carries the buyer's
    /// name, address and BIN, and 51(2) says the buyer has no input tax credit
    /// without them. The item here is 100.00 with tax on the listed price, so
    /// the basket crosses the line partway through a quantity a wholesaler
    /// would buy.
    #[test]
    fn a_supply_over_twenty_five_thousand_asks_for_the_buyer() {
        let mut till = till_with_a_listed_price_item();

        let small = view_of(&till.run_json(
            r#"{"op":"scan","barcode":"8690000000002","qty_milli":100000}"#,
        ));
        assert!(small.error.is_none(), "{:?}", small.error);
        assert_eq!(
            small.total_minor, 11_500_00,
            "a hundred at a hundred, and the tax on the listed price"
        );
        assert!(
            !small.buyer_wanted,
            "ten thousand is nobody's business but the shop's"
        );

        let big = view_of(&till.run_json(
            r#"{"op":"scan","barcode":"8690000000002","qty_milli":200000}"#,
        ));
        assert!(big.error.is_none(), "{:?}", big.error);
        assert_eq!(big.total_minor, 34_500_00, "three hundred of them");
        assert!(
            big.buyer_wanted,
            "and thirty thousand is an invoice that has to say who bought it"
        );
    }

    /// And stops asking once the sale names somebody the shop wrote down.
    #[test]
    fn attaching_a_customer_answers_it() {
        let mut till = till_with_a_listed_price_item();
        with_till!(till, |inner| inner.set_customers(alloc::vec![
            openpos_core::storage::wire::CustomerV1 {
                id: 21,
                name: alloc::string::String::from("Rahman Wholesale"),
                phone: None,
                active: true,
                bin: Some(alloc::string::String::from("123456789-0202")),
                limit_minor: 0,
            address: None,
            }
        ]))
        .expect("the shop's people");

        let rung = view_of(&till.run_json(
            r#"{"op":"scan","barcode":"8690000000002","qty_milli":300000}"#,
        ));
        assert!(rung.error.is_none(), "{:?}", rung.error);
        assert!(rung.buyer_wanted, "thirty-four and a half thousand");

        let named = view_of(&till.run_json(&alloc::format!(
            r#"{{"op":"set_customer","customer":"{}"}}"#,
            openpos_core::ids::Ulid::from_u128(21).encode()
        )));
        assert!(named.error.is_none(), "{:?}", named.error);
        assert!(
            !named.buyer_wanted,
            "the shop has done what it can with what it holds"
        );
    }
}

#[cfg(test)]
mod who_was_at_the_counter {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::tests::{till_with_a_listed_price_item, view_of};
    use super::Store;
    use openpos_core::ids::Ulid;

    /// The shop's own name, which a receipt refuses to be laid out without.
    fn and_a_shop(till: &mut super::TillHandle) {
        with_till!(till, |inner| inner.set_shop(
            openpos_core::receipt::Shop {
                name: alloc::string::String::from("Karim General Store"),
                bin: None,
                address: None,
                phone: None,
            },
            alloc::vec![],
            openpos_core::domain::StockRule::Off,
            alloc::vec![],
        ))
        .expect("a shop");
    }

    fn rung_and_paid(till: &mut super::TillHandle, at: u128) {
        assert!(
            view_of(&till.run_json(
                r#"{"op":"scan","barcode":"8690000000002","qty_milli":1000}"#
            ))
            .error
            .is_none()
        );
        assert!(
            view_of(&till.run_json(r#"{"op":"add_cash","amount_minor":11500,"at_ms":1}"#))
                .error
                .is_none()
        );
        let done = view_of(&till.run_json(&alloc::format!(
            r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1}}"#,
            Ulid::from_u128(at).encode()
        )));
        assert!(done.error.is_none(), "{:?}", done.error);
    }

    fn paper(till: &mut super::TillHandle) -> alloc::string::String {
        let printed = view_of(&till.run_json(
            r#"{"op":"receipt","width":32,"rung_at":"13/09/2026, 19:04"}"#,
        ));
        assert!(printed.error.is_none(), "{:?}", printed.error);
        printed
            .receipt
            .expect("the paper")
            .into_iter()
            .map(|line| line.text)
            .collect::<alloc::vec::Vec<_>>()
            .join("\n")
    }

    /// The line the core has laid out since receipts existed, filled at last.
    #[test]
    fn the_paper_says_who_rang_it() {
        let mut till = till_with_a_listed_price_item();
        and_a_shop(&mut till);
        rung_and_paid(&mut till, 901);

        let printed = paper(&mut till);
        assert!(
            printed.contains("Served by"),
            "the core lays this line out and nothing filled it: {printed}"
        );
        assert!(printed.contains("Supervisor"), "{printed}");
    }

    /// The reason it is taken from the sale rather than from the sign-in.
    ///
    /// A reprint is a button. Somebody asks for a copy an hour later, and by
    /// then the evening cashier is at the till. Naming them would put the wrong
    /// person on the paper at the one moment the line exists to settle who was
    /// at the counter, and a wrong name is worse than no name.
    #[test]
    fn a_reprint_after_the_shift_changed_still_names_the_one_who_rang_it() {
        let mut till = till_with_a_listed_price_item();
        and_a_shop(&mut till);
        rung_and_paid(&mut till, 902);

        // The evening takes over. Same till, different person.
        let evening = openpos_core::auth::OperatorId::from_u128(11);
        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: openpos_core::auth::OperatorId::from_u128(9),
                name: "Supervisor".into(),
                pin: openpos_core::auth::PinHash::derive("1234", [7_u8; 16], 1_000),
                permissions: openpos_core::auth::Permissions {
                    max_discount_bp: 2_000,
                    may_override_price: true,
                    ..Default::default()
                },
                active: true,
            },
            openpos_core::auth::Operator {
                id: evening,
                name: "Shefali".into(),
                pin: openpos_core::auth::PinHash::derive("5678", [8_u8; 16], 1_000),
                permissions: openpos_core::auth::Permissions::cashier(),
                active: true,
            }
        ]));
        assert!(outcome.is_ok());
        assert!(
            view_of(&till.sign_in(&evening.encode(), "5678", 9_000))
                .error
                .is_none()
        );

        let printed = paper(&mut till);
        assert!(
            printed.contains("Supervisor"),
            "the morning rang it: {printed}"
        );
        assert!(
            !printed.contains("Shefali"),
            "whoever is at the till now did not: {printed}"
        );
    }

    /// A person renamed is a person renamed on their old paper too.
    ///
    /// The id travels with the sale and the name is looked up when the paper is
    /// laid out, which is the same rule the customer's name follows. An operator
    /// record is kept rather than deleted for exactly this.
    #[test]
    fn a_reprint_uses_the_name_the_shop_calls_them_now() {
        let mut till = till_with_a_listed_price_item();
        and_a_shop(&mut till);
        rung_and_paid(&mut till, 903);

        let outcome = with_till!(till, |inner| inner.set_operators(alloc::vec![
            openpos_core::auth::Operator {
                id: openpos_core::auth::OperatorId::from_u128(9),
                name: "Rahima Khatun".into(),
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

        let printed = paper(&mut till);
        assert!(printed.contains("Rahima Khatun"), "{printed}");
    }
}

#[cfg(test)]
mod everything_the_till_knows_about_itself {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use openpos_core::till::TillStatus;

    /// Every field of `TillStatus`, and where each one goes.
    ///
    /// This is a compile-time question rather than a runtime one: the
    /// destructure below has no `..`, so adding a field to `TillStatus` stops
    /// this crate building until somebody says here what a screen does with it.
    ///
    /// It exists because three things had been computed and never shown by the
    /// time anybody counted: the shop's telephone number, which the receipt
    /// prints and no box set; the "Served by" line, which the core lays out and
    /// nothing fills; and `drawer_is_behind`, which the core has set since the
    /// drawer was written, whose own doc says it is "a thing to say before
    /// somebody counts against it", and which no screen could read. A value
    /// that crosses no boundary is a value nobody will miss until the evening
    /// it matters.
    #[test]
    fn every_field_either_reaches_a_screen_or_says_why_not() {
        let status = TillStatus {
            cart_lines: 0,
            drawer_is_behind: false,
            unsynced_sales: 0,
            receipt_numbers_left: 0,
            unnumbered_sales: 0,
            cursor: 0,
            wants_checkpoint: false,
            wants_lease_renewal: false,
        };
        let TillStatus {
            // The basket, which the view carries as its lines rather than as a
            // count.
            cart_lines,
            // On the till, above the drawer, before anybody counts against it.
            drawer_is_behind,
            // "0 to send" on both screens.
            unsynced_sales,
            // "490 numbers" on both screens.
            receipt_numbers_left,
            // Beside the numbers left, and only when there are any: a sale
            // closed with no number is paper in a customer's hand with no
            // number on it, and "0 numbers" says the shape of that and not its
            // size.
            unnumbered_sales,
            // The sync loop's own bookkeeping, in the worker. A screen showing
            // a cursor is a screen showing a number nobody can act on; where a
            // device has got to in the catalogue is a different field and is
            // shown.
            cursor,
            wants_checkpoint,
            wants_lease_renewal,
        } = status;

        assert_eq!(cart_lines, 0);
        assert!(!drawer_is_behind);
        assert_eq!(unsynced_sales, 0);
        assert_eq!(receipt_numbers_left, 0);
        assert_eq!(unnumbered_sales, 0);
        assert_eq!(cursor, 0);
        assert!(!wants_checkpoint);
        assert!(!wants_lease_renewal);
    }
}
