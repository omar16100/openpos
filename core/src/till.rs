//! The whole till, as one object.
//!
//! This is what the FFI exposes and what both front ends drive. A UI holds no
//! business rules: it sends intent here and renders what comes back. That is the
//! arrangement that makes two front ends affordable instead of two places for
//! the arithmetic to diverge.
//!
//! The checkout path is the reason this type exists rather than leaving callers
//! to wire the pieces together. Closing a sale has to consume a receipt number,
//! commit the sale durably, and only then move stock and clear the cart, in that
//! order, with a rollback if the commit fails. Left to a UI, that ordering would
//! be a convention, and it would be got wrong on one of the two platforms.

mod drawer;
mod opening;
#[cfg(test)]
mod proof;
mod people;
mod selling;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::auth::{Action, AuthBook, AuthError, Operator};
use crate::cart::{
    Cart, CartError, CartLimits, CartLine, Tender, TenderKind, TerminalId, Ticket, TicketId,
};
use crate::domain::{Discount, StockRule, TicketInput, TicketTotals, ticket_totals};
use crate::ids::Ulid;
use crate::lease::{DEFAULT_RENEWAL_THRESHOLD, Lease, LeaseBook};
use crate::money::{Milli, Minor};
use crate::replica::{Item, Replica};
use crate::shift::{Shift, ShiftError, ShiftId, XReport, ZReport};
use crate::storage::backend::Backend;
use crate::storage::frame::{PayloadKind, Store};
use crate::storage::journal::{Journal, JournalError};
use crate::storage::wire::{
    self, DiscountV1, HeldTicketV1, HeldTicketsV1, ItemDeltasV1, LeaseGrantV1, LineV1, OperatorV1,
    SALE_SCHEMA, SHIFT_SCHEMA, SaleCommitV1, ShiftEventV1, TerminalStateV1, WireError,
};
use crate::sync::driver::Situation;
use crate::sync::{Outbox, PendingSale, SyncEngine, SyncError, SyncStatus};

/// A basket set aside, as the cashier sees it in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTicket {
    pub id: TicketId,
    pub held_at_ms: u64,
    pub label: alloc::string::String,
    pub lines: usize,
    pub total: Minor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TillError {
    /// The scanned code matches nothing in the catalogue.
    UnknownBarcode,
    /// The item is in the catalogue and the shop has stopped selling it.
    ///
    /// Separate from an unknown barcode, because the two need different things
    /// doing: one is a code nobody recognises, the other is a decision somebody
    /// made, and a cashier told "no such item" about a box they are holding
    /// will scan it again and then ring it manually.
    NoLongerSold,
    /// Nothing to park.
    NothingToHold,
    /// No parked ticket with that id.
    NoSuchHeldTicket,
    /// Resuming would discard the basket already on screen.
    TicketInProgress,
    Cart(CartError),
    Auth(AuthError),
    Shift(ShiftError),
    /// An action that needs an open drawer arrived with none open.
    NoOpenShift,
    /// The server described a shop with no name, which cannot head a receipt.
    NamelessShop,
    /// An item written down at the till with nothing to call it. A receipt line
    /// with no name on it is a line nobody can query afterwards.
    NamelessItem,
    /// Somebody written down at the till with nothing to call them. A debt
    /// against a blank name is a debt nobody can collect.
    NamelessCustomer,
    /// An item written down at the till with no barcode. The whole reason it is
    /// being written down is that something was scanned, and without the code
    /// the next person to scan it is where this started.
    NoBarcodeToFindItBy,
    /// A person with the nil id, which every record here uses to mean nobody.
    NamelessOperator,
    /// A basket pointed at somebody this device has never been told about, or
    /// somebody the shop has stopped letting buy on account.
    UnknownCustomer,
    /// The shop asked to be told, and this is more of that item than it
    /// believes it has.
    ///
    /// Only ever raised when a shop has set its rule to refuse. Carries the
    /// figures rather than prose, so a screen can put them in front of a cashier
    /// without parsing a sentence, and a supervisor can allow it.
    MoreThanTheShelfHolds {
        name: alloc::string::String,
        /// What the shop believes is there, in thousandths.
        on_hand_milli: i64,
        /// What the basket would take it to.
        wanted_milli: i64,
    },
    /// More on somebody's account than the shop said they may owe.
    ///
    /// Carries what they owed when the shop last said so and when that was, so
    /// the screen can tell a cashier how old the figure is: a till that has not
    /// synced since morning is refusing on the morning's number, and saying so
    /// is the difference between a rule and a machine being difficult.
    BeyondTheirLimit {
        name: alloc::string::String,
        owed_minor: i64,
        owed_as_of_ms: u64,
        limit_minor: i64,
        /// What this basket would take it to.
        wanted_minor: i64,
    },
    /// A credit tender named somebody the shop has written down, without
    /// pointing the basket at them.
    ///
    /// The two are added up separately: one against the person's record, the
    /// other against the spelling that was typed. A shop that let both happen
    /// would have a customer who owes for what they took and a phantom of the
    /// same name holding what they brought back.
    WriteItAgainstThem {
        /// The name as the shop wrote it down, so a screen can offer them.
        name: alloc::string::String,
    },
    Journal(JournalError),
    Sync(SyncError),
    Wire(WireError),
}

impl From<CartError> for TillError {
    fn from(error: CartError) -> Self {
        Self::Cart(error)
    }
}

impl From<AuthError> for TillError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<ShiftError> for TillError {
    fn from(error: ShiftError) -> Self {
        Self::Shift(error)
    }
}

impl From<JournalError> for TillError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<SyncError> for TillError {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}

impl From<WireError> for TillError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

impl core::fmt::Display for TillError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownBarcode => f.write_str("no item in the catalogue has that barcode"),
            Self::NoLongerSold => f.write_str("the shop has stopped selling that item"),
            Self::NothingToHold => f.write_str("there is nothing on the screen to set aside"),
            Self::NoSuchHeldTicket => f.write_str("no basket is parked under that ticket"),
            Self::TicketInProgress => {
                f.write_str("a basket is already on the screen; close or park it first")
            }
            Self::NoOpenShift => f.write_str("no drawer is open on this terminal"),
            Self::NamelessShop => {
                f.write_str("the shop has no name set, so a receipt would have nothing at the top")
            }
            Self::NamelessItem => {
                f.write_str("an item needs a name, or its line on the receipt says nothing")
            }
            Self::NamelessCustomer => {
                f.write_str("somebody buying on account needs a name to write the debt against")
            }
            Self::NoBarcodeToFindItBy => {
                f.write_str("an item written down here needs the barcode that was scanned")
            }
            Self::NamelessOperator => {
                f.write_str("that person has the id this device uses to mean nobody")
            }
            Self::UnknownCustomer => {
                f.write_str("this till has no such customer, or the shop has stopped their account")
            }
            Self::MoreThanTheShelfHolds {
                name,
                on_hand_milli,
                wanted_milli,
            } => write!(
                f,
                "the shop has {} {name} and this basket wants {}",
                crate::receipt::quantity_of(*on_hand_milli),
                crate::receipt::quantity_of(*wanted_milli)
            ),
            Self::BeyondTheirLimit {
                name,
                owed_minor,
                limit_minor,
                wanted_minor,
                ..
            } => write!(
                f,
                "{name} owes {} and you allow {}: this would take them to {}",
                crate::receipt::money_of(*owed_minor),
                crate::receipt::money_of(*limit_minor),
                crate::receipt::money_of(*wanted_minor)
            ),
            Self::WriteItAgainstThem { name } => write!(
                f,
                "{name} is written down here: choose them, or this goes on a second account under \
                 the same name"
            ),
            Self::Cart(error) => write!(f, "{error}"),
            Self::Auth(error) => write!(f, "{error}"),
            Self::Shift(error) => write!(f, "{error}"),
            Self::Journal(error) => write!(f, "{error}"),
            Self::Sync(error) => write!(f, "{error}"),
            Self::Wire(error) => write!(f, "{error}"),
        }
    }
}

impl TillError {
    /// A stable name for this refusal, for a screen that has to say it in a
    /// language this crate does not hold.
    ///
    /// The words above are English and are what a log and a developer read. A
    /// shop in Bangladesh has a cashier reading the screen, and the moment
    /// something is refused is exactly the moment they need it in their own
    /// language: matching on the English sentence to translate it would break
    /// the day somebody improved the wording.
    ///
    /// Stable, and frozen by a test. Renaming one is renaming a key every
    /// screen and every dictionary holds, which is a deliberate act rather than
    /// a tidy-up.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownBarcode => "unknown-barcode",
            Self::NoLongerSold => "no-longer-sold",
            Self::NothingToHold => "nothing-to-hold",
            Self::NoSuchHeldTicket => "no-such-held-ticket",
            Self::TicketInProgress => "ticket-in-progress",
            Self::NoOpenShift => "no-open-shift",
            Self::NamelessShop => "nameless-shop",
            Self::NamelessItem => "nameless-item",
            Self::NamelessCustomer => "nameless-customer",
            Self::NoBarcodeToFindItBy => "no-barcode-to-find-it-by",
            Self::NamelessOperator => "nameless-operator",
            Self::UnknownCustomer => "unknown-customer",
            Self::MoreThanTheShelfHolds { .. } => "more-than-the-shelf-holds",
            Self::BeyondTheirLimit { .. } => "beyond-their-limit",
            Self::WriteItAgainstThem { .. } => "write-it-against-them",
            Self::Cart(error) => error.code(),
            Self::Auth(error) => error.code(),
            Self::Shift(error) => error.code(),
            Self::Journal(_) => "journal",
            Self::Sync(_) => "sync",
            Self::Wire(_) => "wire",
        }
    }
}

impl core::error::Error for TillError {}

pub type Result<T> = core::result::Result<T, TillError>;

/// A line the shop believes it does not have enough of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShortOfStock {
    /// Which line on the screen, so a cashier is shown the one in question
    /// rather than a sentence about the basket.
    pub line: usize,
    pub name: alloc::string::String,
    pub on_hand_milli: i64,
    pub wanted_milli: i64,
}

/// One sale a device is holding and the shop has not got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedSale {
    pub id: Ulid,
    /// The schema its bytes were written under, which travels with them.
    pub schema: u16,
    pub total_minor: i64,
    pub payload: alloc::vec::Vec<u8>,
    /// True when it came out of the salvage blob rather than the outbox: read
    /// back from a torn log, and worth a person's eyes before it is believed.
    pub salvaged: bool,
}

/// What a cold start found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootReport {
    pub items: usize,
    pub unsynced_sales: usize,
    pub receipt_numbers_left: u64,
    pub cursor: u64,
    /// True when recovery had to discard a torn tail or found a damaged snapshot
    /// slot. Worth surfacing: it means a device died mid-write at some point.
    pub repaired: bool,
    /// True when a snapshot was there and this build could not read it, so the
    /// catalogue is being fetched again from the start.
    ///
    /// Not an error: a snapshot is a cache and the shop's prices come back from
    /// the server. Surfaced because a device that quietly re-downloads its whole
    /// catalogue every morning, on a shop's mobile data, is a bill nobody can
    /// explain.
    pub catalogue_refetched: bool,
    /// Bytes recovery could not read and copied aside instead of destroying.
    ///
    /// Distinct from `repaired`, and worse. A torn tail is one interrupted sale
    /// and is expected on cheap hardware. Whole frames in here are sales that
    /// were committed, printed, and are now gone from the log: the shop needs a
    /// person, not a retry.
    pub salvaged_bytes: usize,
}

/// What the cashier and the owner need to see at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TillStatus {
    pub cart_lines: usize,
    /// Whether the drawer's running figure on this device is behind the sales
    /// it has taken, which is a thing to say before somebody counts against it.
    pub drawer_is_behind: bool,
    pub unsynced_sales: usize,
    pub receipt_numbers_left: u64,
    /// Sales closed with no number available, awaiting a block.
    pub unnumbered_sales: u64,
    pub cursor: u64,
    pub wants_checkpoint: bool,
    pub wants_lease_renewal: bool,
}

/// A sale that has been committed and may now be printed.
///
/// Handed back only after the commit is durable, so holding one of these is
/// permission to put paper in a customer's hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedSale {
    pub ticket: Ticket,
    /// Absent when the terminal has run out of leased numbers. The sale is still
    /// valid and still syncs; the back office numbers it when a block arrives.
    pub receipt_no: Option<alloc::string::String>,
    pub journal_sequence: u64,
}

/// Put a written-down drawer back as the shift it was.
///
/// Every figure comes back as it was folded, including the movements, because a
/// drawer short by five hundred with a drop of five hundred in it is a different
/// evening from one with no movements at all.
fn as_it_was(drawer: &wire::OpenDrawerV1) -> Result<Shift> {
    let mut totals = Vec::with_capacity(drawer.tenders.len());
    for total in &drawer.tenders {
        let kind = total.kind.clone().into_domain();
        totals.push(crate::shift::TenderTotal {
            in_drawer: crate::shift::lands_in_drawer(&kind),
            kind,
            amount: Minor::new(total.amount_minor),
        });
    }
    Ok(Shift::as_it_was(crate::shift::OpenDrawer {
        id: Ulid::from_u128(drawer.id),
        terminal: Ulid::from_u128(drawer.terminal),
        opened_at_ms: drawer.opened_at_ms,
        opening_float: Minor::new(drawer.opening_float_minor),
        sales: usize::try_from(drawer.sales).unwrap_or(usize::MAX),
        tender_totals: totals,
        cash_sales: Minor::new(drawer.cash_sales_minor),
        cash_in_total: Minor::new(drawer.cash_in_minor),
        cash_out_total: Minor::new(drawer.cash_out_minor),
        movements: drawer
            .movements
            .iter()
            .map(|moved| crate::shift::CashMovement {
                direction: if moved.inward {
                    crate::shift::CashDirection::In
                } else {
                    crate::shift::CashDirection::Out
                },
                amount: Minor::new(moved.amount_minor),
                reason: moved.reason.as_str().into(),
                at_ms: moved.at_ms,
            })
            .collect(),
    }))
}

/// The drawer the log describes, and who counted it if it was counted.
struct RecoveredShift {
    shift: Option<Shift>,
    counted_by: Option<(u128, alloc::string::String)>,
}

/// What a terminal owns, read back off its own store.
///
/// A struct rather than a tuple because it grew to five things, and a caller
/// unpacking five positional values gets two of them the wrong way round
/// eventually.
struct Standing {
    leases: LeaseBook,
    held: HeldTicketsV1,
    auth: AuthBook,
    token: Option<alloc::string::String>,
    shop: Option<crate::receipt::Shop>,
    /// What this shop takes money by, beside the shop's own details: they
    /// arrive together and are wanted together.
    wallets: Vec<Box<str>>,
    /// What this shop wants done when a basket asks for more than the shelf
    /// holds. Arrives with the shop's details and is kept with them, because a
    /// till decides this with the internet down like everything else.
    stock_rule: StockRule,
    /// Drawers counted and closed and not yet sent to the shop. Kept beside the
    /// leases because it survives the critical log being emptied, and a counted
    /// drawer that went with the log is a record nobody can reconstruct.
    unsent_shifts: Vec<wire::ClosedShiftV1>,
    /// The drawer that was open when the log under it was dropped, and the
    /// sequence it was folded through. Absent while the log still holds it.
    folded_drawer: Option<wire::OpenDrawerV1>,
    /// What this device allowed and has not sent, and how many it has allowed
    /// ever.
    unsent_allowed: Vec<wire::AllowedV1>,
    /// Items this till wrote down itself and the shop has not got.
    unsent_items: Vec<wire::ItemV1>,
    /// People this till wrote down itself and the shop has not got.
    unsent_customers: Vec<wire::CustomerV1>,
    allowed_seq: u64,
    /// Who the shop lets buy on account. Here rather than in the catalogue
    /// because it is not a catalogue: a cashier needs the name with the line
    /// down, and a name typed from memory is how one Karim pays for another
    /// Karim's rice.
    customers: Vec<wire::CustomerV1>,
    /// When the credential was taken, and how long one lasts.
    credential: Option<wire::CredentialV1>,
}

/// Everything a terminal is and knows.
pub struct Till<B: Backend> {
    journal: Journal<B>,
    replica: Replica,
    sync: SyncEngine,
    leases: LeaseBook,
    cart: Cart,
    /// A supervisor allowed this basket past the shelf. Held here rather than on
    /// the cart because the shelf is the till's question: the cart knows what is
    /// on it and nothing about what the shop has. Cleared with the basket, like
    /// the raised ceilings it sits beside: allowing one sale past the shelf is
    /// not allowing the rest of the day.
    beyond_stock_allowed: bool,
    /// Whether the drawer's running figure on this device is behind the sales.
    ///
    /// Set when a sale is durable and the drawer could not take it, which is
    /// arithmetic at figures no shop reaches. Kept rather than swallowed
    /// because it is the one thing that would make an evening's count argue
    /// with the till for a reason nobody could see; the next boot rebuilds the
    /// drawer from the sale frames and it goes.
    drawer_is_behind: bool,
    /// Whether this device has ever been told what the shelves hold.
    ///
    /// A till learns the shelf two hundred items at a time, five minutes
    /// apart, so a shop of two thousand lines takes fifty minutes to go round
    /// once. Until an item's turn comes the till holds whatever the catalogue
    /// row carried, which is usually nothing, and nothing reads as none.
    ///
    /// That was a till enrolled this morning warning "the shop has 0" about an
    /// item the shop had sixty-one of, and under the rule that stops a sale it
    /// is worse than a wrong warning: a new till put on the counter refuses
    /// everything scanned at it until its own figures catch up, which is its
    /// first hour, with a queue in front of it. Measured, on a till enrolled
    /// two minutes earlier.
    ///
    /// So the shelf rules say nothing at all until this device has been round
    /// the shelf once. A rule that stops a sale has to rest on a figure
    /// somebody stands behind, and until the lap is done there is no figure
    /// here, only an absence that looks like one.
    ///
    /// A fact about this run and not written down, which was learned the hard
    /// way: it was written down for a day. The figures it is about are not.
    /// They live in the replica, and the replica reaches the disk as a snapshot
    /// that is rewritten when the delta log has grown, so a shelf sweep is
    /// saved by luck or not at all. A till that came back from a reload saying
    /// it had been round the shelf was holding the catalogue's own figures,
    /// which are zero, and in a shop whose rule says refuse it turned away
    /// everything scanned at it. A claim must not outlive the thing it is a
    /// claim about, so this one lasts as long as the figures do.
    shelf_swept: bool,
    limits: CartLimits,
    terminal: TerminalId,
    held: HeldTicketsV1,
    /// The credential this terminal syncs with, recovered from standing state.
    token: Option<alloc::string::String>,
    /// The shop, as its receipts describe it.
    shop: Option<crate::receipt::Shop>,
    /// What this shop takes money by, beside the shop's own details: they
    /// arrive together and are wanted together.
    wallets: Vec<Box<str>>,
    /// What this shop wants done when a basket asks for more than the shelf
    /// holds. Arrives with the shop's details and is kept with them, because a
    /// till decides this with the internet down like everything else.
    stock_rule: StockRule,
    /// Drawers counted and closed and not yet sent to the shop. Kept beside the
    /// leases because it survives the critical log being emptied, and a counted
    /// drawer that went with the log is a record nobody can reconstruct.
    unsent_shifts: Vec<wire::ClosedShiftV1>,
    /// The open drawer as it stood when the log under it was dropped, and the
    /// sequence that fold covers.
    ///
    /// Absent while the log still holds the drawer, which is the ordinary case:
    /// an open drawer is the frames that opened it, the cash that moved and the
    /// sales rung under it, and replaying them is what makes the drawer figure
    /// and the sales figure agree. This exists because the log cannot be dropped
    /// under a drawer that lives only inside it, and a shop that never counts
    /// its drawer therefore never lets a byte go. See fold_the_open_drawer.
    folded_drawer: Option<wire::OpenDrawerV1>,
    /// Privileged actions this device allowed that the shop has not been told
    /// about, and how many it has allowed ever.
    ///
    /// Beside the drawers, for the same reason: the question asked afterwards
    /// is never "was this allowed" but "who allowed it", and until now the only
    /// answer lived in memory and died with the process.
    unsent_allowed: Vec<wire::AllowedV1>,
    /// Items this till wrote down itself because a delivery arrived with a
    /// barcode nobody's catalogue had, kept until the shop says it has them.
    unsent_items: Vec<wire::ItemV1>,
    /// People this till wrote down itself because somebody bought on account
    /// who was in nobody's list, kept until the shop says it has them.
    unsent_customers: Vec<wire::CustomerV1>,
    allowed_seq: u64,
    /// How many wrong PINs have already been kept for the shop. Beside the
    /// audit cursor and for the same reason.
    taken_refusals: usize,
    /// How many of the auth book's entries have already been kept for the shop.
    ///
    /// Not persisted, and it does not need to be: the auth book's list is built
    /// in memory during one run, so a device that restarts starts both at zero
    /// together. What was already kept is in the standing state.
    taken_audit: usize,
    /// Who the shop lets buy on account, as the shop last said.
    customers: Vec<wire::CustomerV1>,
    /// When this device's credential was taken and how long one lasts. Written
    /// down so a till renews before it expires rather than stopping dead a year
    /// after enrolment, and so a tablet switched off nightly does not renew
    /// every morning.
    credential: Option<wire::CredentialV1>,
    /// What each of them owed when the shop last said so, and when that was.
    ///
    /// Not written to the standing state on purpose. A balance goes stale the
    /// moment another till sells to the same person, and a figure a device
    /// carried through a night is worse than no figure: a cashier reads it out
    /// across the counter as though it were true. A till that has just started
    /// says nothing until it has asked.
    balances: Vec<(u128, i64)>,
    balances_at_ms: Option<u64>,
    auth: AuthBook,
    shift: Option<Shift>,
}

impl<B: Backend> Till<B> {
    // -- receipt numbers -----------------------------------------------------

    /// Record a block granted by the server.
    pub fn grant_lease(&mut self, lease: &Lease) -> Result<()> {
        self.leases.grant(lease.clone());
        // Into the standing-state blob, not the critical log. A block written to
        // the log would be thrown away with it the next time the server confirms
        // every sale, leaving a terminal that believes it has no numbers while
        // the server believes it holds hundreds.
        self.persist_terminal_state()
    }

    #[must_use]
    pub fn leases(&self) -> &LeaseBook {
        &self.leases
    }

    // -- sync ----------------------------------------------------------------

    /// Sales waiting to reach the server, oldest first.
    pub fn pending_sales(&self, limit: usize) -> Result<Vec<PendingSale>> {
        Ok(Outbox::batch(&self.journal, limit)?)
    }

    /// Everything this device is still holding that the shop has not got.
    ///
    /// The outbox, plus whatever could be read back out of the salvage blob: a
    /// torn log copies its unreadable tail aside on recovery, and those bytes
    /// can hold sales that were rung and printed. This is what somebody carries
    /// to the back office when a till cannot send: its own till was deleted, or
    /// it holds sales and has to be re-enrolled as another terminal.
    ///
    /// Salvaged sales are last and are marked, because they are the ones a
    /// person has to look at rather than trust: they came from bytes that were
    /// being written when the power went.
    pub fn carried_out(&self, limit: usize) -> Result<Vec<CarriedSale>> {
        let mut found: Vec<CarriedSale> = self
            .pending_sales(limit)?
            .into_iter()
            .map(|sale| CarriedSale {
                id: sale.id,
                schema: wire::SALE_SCHEMA,
                total_minor: sale.total_minor,
                payload: sale.payload,
                salvaged: false,
            })
            .collect();

        for (schema, payload) in self.journal.salvaged()? {
            if found.len() >= limit {
                break;
            }
            let Ok(sale) = wire::decode_sale(schema, &payload) else {
                continue;
            };
            let id = Ulid::from_u128(sale.ticket.id);
            // A sale that is also in the outbox is the same sale, and sending
            // it twice under one id costs nothing; showing it twice to a person
            // counting what they are carrying does.
            if found.iter().any(|held| held.id == id) {
                continue;
            }
            found.push(CarriedSale {
                id,
                schema,
                total_minor: sale.ticket.total_minor,
                payload,
                salvaged: true,
            });
        }
        Ok(found)
    }

    /// Record what the server confirmed.
    ///
    /// When that leaves nothing outstanding the log is emptied, but only after
    /// the terminal's blocks and parked baskets are durable in their own slot.
    /// In that order: a crash in between replays the log and reaches the same
    /// state, whereas emptying first would take the numbers with it.
    pub fn acknowledge(&mut self, acknowledged: &[Ulid]) -> Result<usize> {
        let outcome = Outbox::acknowledge(&mut self.journal, acknowledged)?;
        if outcome.drained {
            // The server has now seen every sale this terminal made, so its
            // stock figures no longer need re-adjusting on top.
            self.replica.settle_local_stock();
            // And the sales that closed with no number are the server's to
            // number, which is what a cold start works out from the log. Said
            // here as well, or the figure on the screen keeps asking for numbers
            // the shop has already taken and only a restart settles it.
            self.leases.clear_unnumbered();
            self.persist_terminal_state()?;
            self.empty_the_log_if_nothing_needs_it()?;
        }
        Ok(outcome.confirmed)
    }

    /// Drop the critical log, but only when nothing is still reading it.
    ///
    /// Two things need it. The sales the server has not taken, which is what the
    /// caller has just established there are none of. And the open drawer, which
    /// is rebuilt by replaying this log and lives nowhere else: emptying it under
    /// a drawer that is still open loses the float the owner put in, every
    /// movement since, and the day's takings. The cashier would be asked to open
    /// a shift that is already open, and the evening count would be against a
    /// drawer that began at nothing. Closed drawers were given a home in the
    /// standing state for exactly this reason; the open one was not.
    ///
    /// Deliberately all or nothing rather than a cut back to the opening frame.
    /// Dropping the front of a log means rewriting it, and a crash inside that
    /// rewrite takes the unsent tail with it. So the log holds the day, which is
    /// what it holds anyway on a day with no internet, and goes once the drawer
    /// is counted.
    fn empty_the_log_if_nothing_needs_it(&mut self) -> Result<()> {
        if !Outbox::pending(&self.journal)?.is_empty() {
            return Ok(());
        }
        // What the outbox can see is what reads through. A frame that has gone
        // bad in the middle of this log hides every sale behind it, and an
        // outbox that cannot see them says there is nothing left to send: the
        // log would be emptied and the sales were on this device and nowhere
        // else. A log that does not read through keeps every byte, and the next
        // open puts what nobody can read into the salvage file.
        if !self.journal.reads_through(Store::Critical)? {
            return Ok(());
        }
        // The drawer, if one is open, into the standing state before the frames
        // that describe it go. Written first on purpose: a crash between the two
        // leaves a fold and a log that still holds the same events, and the fold
        // carries the sequence it covers so the next boot skips them. The other
        // order loses the day's drawer.
        self.fold_the_open_drawer()?;
        self.journal.truncate_critical(0)?;
        Ok(())
    }

    /// Write the open drawer down, so the log under it can go.
    ///
    /// A drawer that is open is the frames in the critical log: the opening, the
    /// cash that moved, and every sale rung under it. That is the right way
    /// round while the log is there, because replaying them is what makes the
    /// drawer figure and the sales figure agree by construction.
    ///
    /// It also meant the log could never be dropped under an open drawer, and a
    /// shop that never counts its drawer never let a byte go: about 145 KB of
    /// every thousand sales, kept for ever, on a tablet. So the drawer is
    /// written down at the one moment the log is about to be dropped, with the
    /// sequence it was folded through, and the next boot starts from it and
    /// replays only what came after. That is the catalogue's own shape, a
    /// snapshot and the deltas after it: what is written down is a checkpoint of
    /// the replay rather than a second opinion about it.
    ///
    /// A closed drawer is not folded here. It has a home of its own in the
    /// standing state already, with the count and the name on it.
    fn fold_the_open_drawer(&mut self) -> Result<()> {
        let Some(open) = self.shift.as_ref().filter(|shift| shift.is_open()) else {
            // Nothing open. Any fold left from a drawer that has since been
            // counted is stale, and a stale fold would be put back on the next
            // boot as a drawer nobody opened.
            if self.folded_drawer.is_some() {
                self.folded_drawer = None;
                self.persist_terminal_state()?;
            }
            return Ok(());
        };
        let held = open.what_it_holds();
        self.folded_drawer = Some(wire::OpenDrawerV1 {
            id: held.id.to_u128(),
            terminal: held.terminal.to_u128(),
            opened_at_ms: held.opened_at_ms,
            opening_float_minor: held.opening_float.get(),
            sales: u32::try_from(held.sales).unwrap_or(u32::MAX),
            cash_sales_minor: held.cash_sales.get(),
            cash_in_minor: held.cash_in_total.get(),
            cash_out_minor: held.cash_out_total.get(),
            tenders: held
                .tender_totals
                .iter()
                .map(|total| wire::DrawerTenderV1 {
                    kind: wire::TenderKindV1::from_domain(&total.kind),
                    amount_minor: total.amount.get(),
                })
                .collect(),
            movements: held
                .movements
                .iter()
                .map(|moved| wire::DrawerMovementV1 {
                    inward: matches!(moved.direction, crate::shift::CashDirection::In),
                    amount_minor: moved.amount.get(),
                    reason: moved.reason.to_string(),
                    at_ms: moved.at_ms,
                })
                .collect(),
            // Everything committed so far. The next frame this device writes
            // takes the number after it, so the boundary is exact.
            folded_through: self.journal.next_sequence().saturating_sub(1),
        });
        self.persist_terminal_state()
    }

    /// Take what the shop says is on the shelves.
    ///
    /// Returns how many of them this till holds. A figure for an item this
    /// device has never heard of is dropped rather than invented into the
    /// catalogue: the catalogue is what a pull says it is.
    pub fn apply_on_hand(&mut self, figures: &[(crate::replica::ItemId, Milli)]) -> usize {
        self.replica.apply_on_hand(figures)
    }

    /// The items this till holds, in the order the catalogue keeps them, so a
    /// platform can ask the shop about a window of them.
    #[must_use]
    pub fn item_window(&self, from: usize, limit: usize) -> Vec<crate::replica::ItemId> {
        self.replica
            .items()
            .iter()
            .skip(from)
            .take(limit)
            .map(|item| item.id)
            .collect()
    }

    /// Apply catalogue changes pulled from the server.
    pub fn apply_pull(&mut self, deltas: &ItemDeltasV1) -> Result<u64> {
        Ok(self
            .sync
            .apply_pull(&mut self.journal, &mut self.replica, deltas)?)
    }

    /// Fold the delta log into a fresh snapshot, if it has grown enough.
    ///
    /// Returns the new generation when one was written. Never call this with a
    /// ticket open: it rewrites a couple of megabytes and belongs between
    /// customers.
    pub fn checkpoint_if_needed(&mut self) -> Result<Option<u64>> {
        let status = self.sync.status(&self.journal)?;
        if !status.wants_checkpoint() {
            return Ok(None);
        }
        Ok(Some(
            self.sync.checkpoint(&mut self.journal, &self.replica)?,
        ))
    }

    /// Force a checkpoint regardless of log length, used after an upgrade.
    pub fn checkpoint_now(&mut self) -> Result<u64> {
        Ok(self.sync.checkpoint(&mut self.journal, &self.replica)?)
    }

    pub fn sync_status(&self) -> Result<SyncStatus> {
        Ok(self.sync.status(&self.journal)?)
    }

    /// Everything the UI puts on screen about the terminal itself.
    pub fn status(&self) -> Result<TillStatus> {
        let sync = self.sync.status(&self.journal)?;
        Ok(TillStatus {
            cart_lines: self.cart.lines().len(),
            drawer_is_behind: self.drawer_is_behind,
            unsynced_sales: sync.unsynced,
            receipt_numbers_left: self.leases.remaining(),
            unnumbered_sales: self.leases.unnumbered(),
            cursor: sync.cursor,
            wants_checkpoint: sync.wants_checkpoint(),
            wants_lease_renewal: self.leases.needs_renewal(DEFAULT_RENEWAL_THRESHOLD),
        })
    }

    /// What the sync driver needs to know, taken from the till rather than
    /// assembled by a platform.
    ///
    /// A platform that had to build this itself would be a platform that can get
    /// it wrong, and the wrong answer is a till that stops pushing sales while
    /// believing it has none.
    pub fn situation(&self, online: bool, more_to_pull: bool) -> Result<Situation> {
        let status = self.status()?;
        Ok(Situation {
            unsynced_sales: status.unsynced_sales,
            unsent_shifts: self.unsent_shifts.len(),
            unsent_allowed: self.unsent_allowed.len(),
            unsent_items: self.unsent_items.len(),
            unsent_customers: self.unsent_customers.len(),
            drawer_open: self.shift().is_some(),
            credential_taken_at_ms: self
                .credential
                .map(|note| note.taken_at_ms)
                .unwrap_or_default(),
            credential_lifetime_ms: self
                .credential
                .map(|note| note.lifetime_ms)
                .unwrap_or_default(),
            enrolled: self.token.is_some(),
            has_customers: self.customers.iter().any(|known| known.active),
            watches_stock: self.stock_rule != StockRule::Off,
            items: self.replica.len(),
            cursor: status.cursor,
            receipt_numbers_left: status.receipt_numbers_left,
            more_to_pull,
            online,
        })
    }

    /// Which terminal this is. Needed by anything building a request, and taken
    /// from the till rather than carried alongside it, because two copies of an
    /// identifier are two chances to send somebody else's.
    #[must_use]
    pub fn terminal(&self) -> TerminalId {
        self.terminal
    }

    /// Borrow the journal, for tests and platform maintenance.
    /// The journal, for a test that has to make a log go bad under a running
    /// till. Nothing in the product writes through this: a till that reached
    /// past its own journal would be a second writer to the thing that holds
    /// the shop's sales.
    #[cfg(test)]
    pub(crate) fn journal_mut(&mut self) -> &mut Journal<B> {
        &mut self.journal
    }

    pub fn journal(&self) -> &Journal<B> {
        &self.journal
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

    use alloc::vec;

    use super::*;
    use crate::cart::TenderKind;
    
    
    use crate::storage::backend::MemoryBackend;
    

    use super::proof::*;

    #[test]
    fn a_till_whose_state_was_written_by_the_build_before_still_opens() {
        use crate::storage::backend::{Backend, Blob};
        use crate::storage::frame::{self, FrameHeader, PayloadKind, Store};

        // What the previous build wrote: a shop with no wallets, stamped with
        // the schema number it used. Decoding this as the current version loses
        // the leases, the parked sales and the credential, which is every till
        // in every shop on the morning after an upgrade.
        let old = crate::storage::wire::TerminalStateV1Legacy {
            leases: vec![],
            held: crate::storage::wire::HeldTicketsV2Legacy::default(),
            unnumbered: 3,
            operators: vec![],
            token: Some(alloc::string::String::from("a-credential")),
            shop: Some(crate::storage::wire::ShopV1Legacy {
                name: alloc::string::String::from("Karim General Store"),
                bin: None,
                address: None,
                phone: None,
            }),
        };
        let payload = postcard::to_allocvec(&old).unwrap();
        let header = FrameHeader {
            store: Store::Critical,
            kind: PayloadKind::TerminalState,
            schema: crate::storage::wire::TERMINAL_SCHEMA_V1,
            producer: 1,
            tenant: TENANT,
            terminal: terminal().to_u128(),
            sequence: 1,
        };
        let mut bytes = Vec::new();
        frame::encode(&header, &payload, &mut bytes).unwrap();

        let mut backend = MemoryBackend::new();
        backend.write_blob(Blob::TerminalA, &bytes).unwrap();
        backend.flush().unwrap();

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();

        assert_eq!(till.token(), Some("a-credential"));
        assert_eq!(
            till.shop().map(|shop| shop.name.as_str()),
            Some("Karim General Store")
        );
        // A shop never told which wallets it takes takes none, and the till lets
        // a cashier name one instead.
        assert!(till.wallets().is_empty());
    }

    /// A mis-scan is not a void, and a line taken off money is.
    ///
    /// The permission existed on every operator record and nothing anywhere
    /// enforced it, so a cashier could ring goods, take the cash for them, take
    /// the line off, and leave a smaller sale and no trace. Enforcing it on
    /// every removal instead would mean a supervisor for every double scan,
    /// which is a till a shop turns off.
    #[test]
    fn a_line_comes_off_freely_until_somebody_has_paid_towards_it() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        // Scanned in error, before anybody has handed anything over.
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.remove_line(0, 1_000)
            .expect("a mis-scan is not a theft");
        assert!(till.cart().lines().is_empty());

        // Now a basket the customer has paid for.
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 49_450);
        let refused = till.remove_line(0, 2_000).unwrap_err();
        assert!(
            matches!(
                refused,
                TillError::Auth(crate::auth::AuthError::NotPermitted {
                    action: Action::VoidLine
                })
            ),
            "refused with {refused:?}"
        );
        assert_eq!(till.cart().lines().len(), 1, "and the line is still on it");

        // Which is the moment a shop wants written down, whoever it was.
        assert!(
            till.unsent_allowed()
                .iter()
                .any(|one| one.action == 11 && one.operator_name == "Karim"),
            "a cashier who tried is the record a shop wants most"
        );

        // A supervisor standing there allows it, once.
        till.authorise(Ulid::from_u128(70), "9999", Action::VoidLine, 2_000, 90_000)
            .unwrap();
        till.remove_line(0, 3_000).expect("the supervisor said so");
        assert!(till.cart().lines().is_empty());
        assert_eq!(
            till.audit().last().map(|entry| entry.authorised_by),
            Some(Some(Ulid::from_u128(70))),
            "and the supervisor's name is on it"
        );
    }

    #[test]
    fn a_balance_is_never_shown_without_saying_how_old_it_is() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.set_customers(alloc::vec![wire::CustomerV1 {
                id: 21,
                name: "Karim, flat 3".into(),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 0,
            }])
            .unwrap();

            // Nothing until the shop has been asked. A till that has just
            // started must not answer "how much do I owe" with a guess.
            assert_eq!(till.owed_by(Ulid::from_u128(21)), None);

            till.set_balances(alloc::vec![(21, 39_450)], 1_788_600_000_000);
            assert_eq!(
                till.owed_by(Ulid::from_u128(21)),
                Some((Minor::new(39_450), 1_788_600_000_000))
            );
            // Somebody the shop wrote down who owes nothing is not in the
            // answer at all, which is different from not having asked.
            assert_eq!(till.owed_by(Ulid::from_u128(22)), None);
            backend = till.journal().backend().clone();
        }

        // And it is gone after a reboot, on purpose. A figure carried through a
        // night is worse than none: another till may have sold to this person,
        // and a cashier reads a stale number out as though it were true.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(till.customers().len(), 1, "the name is kept");
        assert_eq!(till.owed_by(Ulid::from_u128(21)), None, "the number is not");
    }

    #[test]
    fn a_name_nobody_wrote_down_is_still_taken() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: 21,
            name: "Karim, flat 3".into(),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        }])
        .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // A shop takes a promise from somebody it has not written down, all
        // day. Refusing that would be refusing the ordinary case to prevent
        // the confusing one.
        assert!(
            till.add_tender(
                Tender {
                    kind: TenderKind::Credit,
                    amount: Minor::new(49_450),
                    reference: Some("the man from the tailor's".into()),
                },
                0
            )
            .is_ok()
        );
    }

    #[test]
    fn nobody_may_hold_the_id_that_means_nobody() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut nameless = supervisor_operator();
        nameless.id = Ulid::from_u128(0);

        // Zero is what a drawer counted by an older build carries. A person
        // holding it would read back as nobody having counted.
        assert!(matches!(
            till.put_operator(nameless),
            Err(TillError::NamelessOperator)
        ));
    }

    #[test]
    fn the_shop_taking_the_trail_is_what_clears_it() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::ZERO, 0)
            .unwrap();
        till.cash_out(Minor::new(1_000), "change for the float", 1_000)
            .unwrap();
        till.cash_out(Minor::new(2_000), "paid the milk man", 2_000)
            .unwrap();
        // One sign-in from opening the till, then the two drawer openings.
        let seqs: Vec<u64> = till.unsent_allowed().iter().map(|one| one.seq).collect();
        assert_eq!(seqs, alloc::vec![1, 2, 3], "its own count, not a clock");
        assert_eq!(till.unsent_allowed()[0].action, 9, "somebody took the till");

        // What the server said it stored, never what was sent: a reply that did
        // not arrive must leave the trail here to go again.
        till.allowed_accepted(&[1, 2]).unwrap();
        assert_eq!(till.unsent_allowed().len(), 1);
        assert_eq!(till.unsent_allowed()[0].seq, 3);
    }

    /// A shop that wants to be told sells it and says so.
    #[test]
    fn a_shop_that_wants_telling_is_told_which_line() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Warn);
        till.scan("8690000000001", Milli::new(2_000)).unwrap();
        assert!(till.beyond_the_shelf().is_empty(), "two of three is fine");

        till.scan("8690000000001", Milli::new(2_000)).unwrap();
        let short = till.beyond_the_shelf();
        assert_eq!(short.len(), 1, "the same item, counted across the basket");
        assert_eq!(short[0].line, 0);
        assert_eq!(short[0].on_hand_milli, 3_000);
        assert_eq!(short[0].wanted_milli, 4_000);
    }

    /// Turning the rule off and on again starts the lap again.
    ///
    /// A till stops being sent figures the moment its shop stops watching the
    /// shelf, so what it holds stops being maintained. A shop that turned the
    /// rule off in Ramadan and on again in July would otherwise have a till
    /// refusing sales on figures from before the change.
    #[test]
    fn a_rule_turned_off_and_on_again_waits_for_a_fresh_lap() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        assert!(till.shelf_known(), "it went round while the rule was on");

        let shop = till.shop().cloned().expect("the shop it already has");
        till.set_shop(shop.clone(), vec![], StockRule::Off).unwrap();
        assert!(!till.shelf_known(), "nobody is sending it figures now");

        till.set_shop(shop, vec![], StockRule::Block).unwrap();
        assert!(
            !till.shelf_known(),
            "and turning it back on does not restore what it stopped being told"
        );
        till.scan("8690000000001", Milli::new(9_000))
            .expect("so it sells, and waits for the lap");
    }

    /// A delivery arrives during an outage with a barcode nobody has.
    ///
    /// The whole cold-start promise turns on this: a till that can only say "no
    /// such item" loses the sale, and the shop sells it off the paper and
    /// reconciles nothing.
    #[test]
    fn something_the_shop_never_heard_of_can_be_written_down_and_sold() {
        let mut till = stocked_till(MemoryBackend::new());
        assert!(
            matches!(
                till.scan("8690000000099", Milli::ONE),
                Err(TillError::UnknownBarcode)
            ),
            "nobody has heard of it yet"
        );

        let mut arrived = item(9, 12_000);
        arrived.name_en = "Biscuits, the new ones".into();
        arrived.barcodes = vec!["8690000000099".into()];
        arrived.on_hand = Milli::ZERO;
        till.quick_add(arrived).unwrap();

        till.scan("8690000000099", Milli::new(2_000))
            .expect("and now it sells");
        let totals = till.totals().unwrap();
        assert_eq!(
            totals.net_total,
            Minor::new(24_000),
            "two at a hundred and twenty"
        );
        assert_eq!(
            totals.vat_total,
            Minor::new(3_600),
            "and the tax the cashier said"
        );

        // Held for the shop, and still held after the tablet restarts: an item
        // that went with the process is a sale naming something nobody can look
        // up.
        assert_eq!(till.unsent_items().len(), 1);
        let backend = till.journal().backend().clone();
        let (again, boot) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(again.unsent_items().len(), 1, "still owed to the shop");
        assert_eq!(
            again.unsent_items()[0].name_en,
            "Biscuits, the new ones",
            "as it was written down"
        );
        assert_eq!(boot.items, 2, "and it is in the catalogue like any other");
    }

    /// The shop's list arriving must not take away somebody it has not heard of.
    #[test]
    fn the_shops_list_does_not_drop_whoever_this_till_just_wrote_down() {
        let mut till = stocked_till(MemoryBackend::new());
        till.write_customer(wire::CustomerV1 {
            id: Ulid::from_u128(21).to_u128(),
            name: alloc::string::String::from("Karim, flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        })
        .unwrap();

        // The list the shop knows about, which cannot name them yet.
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: Ulid::from_u128(22).to_u128(),
            name: alloc::string::String::from("Rahima"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        }])
        .unwrap();

        assert_eq!(till.customers().len(), 2, "both, until the shop has both");
        assert!(
            till.customers()
                .iter()
                .any(|one| one.name == "Karim, flat 3"),
            "the person the debt was just rung against is still on the screen"
        );

        // Once the shop has them, its list is the whole list.
        till.customers_accepted(&[Ulid::from_u128(21).to_u128()])
            .unwrap();
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: Ulid::from_u128(22).to_u128(),
            name: alloc::string::String::from("Rahima"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        }])
        .unwrap();
        assert_eq!(till.customers().len(), 1);
    }

    /// The driver has to be told there is somebody to send.
    #[test]
    fn a_till_that_wrote_somebody_down_says_it_has_them_to_send() {
        let mut till = stocked_till(MemoryBackend::new());
        till.write_customer(wire::CustomerV1 {
            id: Ulid::from_u128(21).to_u128(),
            name: alloc::string::String::from("Karim, flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        })
        .unwrap();

        let situation = till.situation(true, false).unwrap();
        assert_eq!(
            situation.unsent_customers, 1,
            "or the sync loop never asks to send them"
        );
    }

    #[test]
    fn somebody_with_no_name_is_refused() {
        let mut till = stocked_till(MemoryBackend::new());
        assert!(matches!(
            till.write_customer(wire::CustomerV1 {
                id: Ulid::from_u128(21).to_u128(),
                name: alloc::string::String::from("   "),
                phone: Some(alloc::string::String::from("01711000000")),
                active: true,
                bin: None,
                limit_minor: 0,
            }),
            Err(TillError::NamelessCustomer)
        ));
        assert!(till.customers().is_empty());
    }

    #[test]
    fn reports_when_a_checkpoint_or_a_renewal_is_wanted() {
        let mut till = stocked_till(MemoryBackend::new());
        let status = till.status().unwrap();
        assert!(!status.wants_checkpoint);
        assert!(
            !status.wants_lease_renewal,
            "five hundred numbers is plenty"
        );

        // The threshold is a trigger, not an emergency: renewal is wanted while
        // there is still a comfortable block left to sell against.
        let mut nearly_out = stocked_till(MemoryBackend::new());
        nearly_out
            .grant_lease(&Lease::new(terminal(), 1, "T1", 700, 700))
            .unwrap();
        while nearly_out.leases().remaining() > DEFAULT_RENEWAL_THRESHOLD {
            nearly_out.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut nearly_out, 50_000);
            nearly_out.checkout(Ulid::from_u128(1_000), 0).unwrap();
        }
        assert!(nearly_out.status().unwrap().wants_lease_renewal);

        assert!(till.checkpoint_if_needed().unwrap().is_none());
        let generation = till.checkpoint_now().unwrap();
        assert!(generation > 0);
        assert_eq!(till.sync_status().unwrap().pending_deltas, 0);
    }
}
