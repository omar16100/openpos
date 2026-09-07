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

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::auth::{Action, AuthBook, AuthError, Operator};
use crate::cart::{
    Cart, CartError, CartLimits, CartLine, Tender, TenderKind, TerminalId, Ticket, TicketId,
};
use crate::domain::{Discount, TicketInput, TicketTotals, ticket_totals};
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
    /// A person with the nil id, which every record here uses to mean nobody.
    NamelessOperator,
    /// A basket pointed at somebody this device has never been told about, or
    /// somebody the shop has stopped letting buy on account.
    UnknownCustomer,
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
            Self::NamelessOperator => {
                f.write_str("that person has the id this device uses to mean nobody")
            }
            Self::UnknownCustomer => {
                f.write_str("this till has no such customer, or the shop has stopped their account")
            }
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

impl core::error::Error for TillError {}

pub type Result<T> = core::result::Result<T, TillError>;

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
    /// Drawers counted and closed and not yet sent to the shop. Kept beside the
    /// leases because it survives the critical log being emptied, and a counted
    /// drawer that went with the log is a record nobody can reconstruct.
    unsent_shifts: Vec<wire::ClosedShiftV1>,
    /// What this device allowed and has not sent, and how many it has allowed
    /// ever.
    unsent_allowed: Vec<wire::AllowedV1>,
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
    /// Drawers counted and closed and not yet sent to the shop. Kept beside the
    /// leases because it survives the critical log being emptied, and a counted
    /// drawer that went with the log is a record nobody can reconstruct.
    unsent_shifts: Vec<wire::ClosedShiftV1>,
    /// Privileged actions this device allowed that the shop has not been told
    /// about, and how many it has allowed ever.
    ///
    /// Beside the drawers, for the same reason: the question asked afterwards
    /// is never "was this allowed" but "who allowed it", and until now the only
    /// answer lived in memory and died with the process.
    unsent_allowed: Vec<wire::AllowedV1>,
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
    /// Boot from whatever is on the device.
    ///
    /// Rebuilds the catalogue from its snapshot plus any deltas the log still
    /// holds, restores the receipt-number position from the sales that actually
    /// happened, and reports what it found. No network is involved: this is the
    /// path that must work at eight in the morning with the internet down.
    pub fn open(
        backend: B,
        tenant: u128,
        terminal: TerminalId,
        producer: u16,
        limits: CartLimits,
    ) -> Result<(Self, BootReport)> {
        let (journal, recovery) = Journal::open(backend, tenant, terminal.to_u128(), producer)?;
        let mut replica = Replica::new();
        let (sync, sync_status) = SyncEngine::recover(&journal, &mut replica)?;
        let Standing {
            leases,
            held,
            auth,
            token,
            shop,
            wallets,
            unsent_shifts,
            unsent_allowed,
            allowed_seq,
            customers,
            credential,
        } = Self::recover_terminal_state(&journal)?;
        let shift = Self::recover_shift(&journal, terminal)?;

        let report = BootReport {
            items: replica.len(),
            unsynced_sales: sync_status.unsynced,
            receipt_numbers_left: leases.remaining(),
            cursor: sync_status.cursor,
            repaired: !recovery.is_clean(),
            catalogue_refetched: sync_status.snapshot_unreadable,
            salvaged_bytes: recovery.salvaged_bytes,
        };

        Ok((
            Self {
                journal,
                replica,
                sync,
                leases,
                cart: Cart::new(limits),
                limits,
                terminal,
                held,
                token,
                shop,
                wallets,
                unsent_shifts,
                unsent_allowed,
                allowed_seq,
                taken_audit: 0,
                taken_refusals: 0,
                customers,
                credential,
                balances: Vec::new(),
                balances_at_ms: None,
                auth,
                shift,
            },
            report,
        ))
    }

    /// Rebuild what the terminal owns: its receipt-number blocks and its parked
    /// baskets.
    ///
    /// The blocks and the baskets come from the standing-state blob, which
    /// survives the critical log being emptied on a full acknowledgement. The
    /// position *within* a block then comes from the sales still in the log,
    /// because a sale is the only proof a number was actually handed over, and
    /// a sale committed after the last blob write would otherwise have its
    /// number issued a second time.
    fn recover_terminal_state(journal: &Journal<B>) -> Result<Standing> {
        let mut book = LeaseBook::new();
        let mut held = HeldTicketsV1::default();
        let mut auth = AuthBook::new();
        let mut token = None;
        let mut shop = None;
        let mut wallets: Vec<Box<str>> = Vec::new();
        let mut unsent_shifts: Vec<wire::ClosedShiftV1> = Vec::new();
        let mut unsent_allowed: Vec<wire::AllowedV1> = Vec::new();
        let mut allowed_seq = 0_u64;
        let mut customers: Vec<wire::CustomerV1> = Vec::new();
        let mut credential: Option<wire::CredentialV1> = None;

        if let Some((schema, bytes)) = journal.load_terminal_state()? {
            // The schema the bytes were written under, not this build's. A
            // device upgrading reads what the build before it wrote.
            let state = wire::decode_terminal_state(schema, &bytes)?;
            for grant in state.leases {
                book.grant(Lease::new(
                    Ulid::from_u128(grant.terminal),
                    grant.epoch,
                    &grant.prefix,
                    grant.first,
                    grant.last,
                ));
            }
            held = state.held;
            token = state.token;
            unsent_shifts = state.unsent_shifts;
            unsent_allowed = state.unsent_allowed;
            allowed_seq = state.allowed_seq;
            customers = state.customers;
            credential = state.credential;
            shop = state.shop.map(|stored| {
                wallets = stored.wallets.into_iter().map(Into::into).collect();
                crate::receipt::Shop {
                    name: stored.name,
                    bin: stored.bin,
                    address: stored.address,
                    phone: stored.phone,
                }
            });
            for operator in state.operators {
                auth.put(operator.into_domain()?);
            }
        }

        let mut highest_used: Option<u64> = None;
        let mut unnumbered = 0_u64;
        for record in journal.read(Store::Critical)? {
            if record.header.kind == PayloadKind::SaleCommit {
                let sale: SaleCommitV1 = wire::decode_sale(record.header.schema, &record.payload)?;
                match sale.lease_next {
                    Some(next) => {
                        highest_used = Some(highest_used.map_or(next, |current| current.max(next)));
                    }
                    // Counted from the log rather than read from the blob. A
                    // sale that closes without a number would otherwise need a
                    // blob write, and therefore a second flush, on the one path
                    // that must stay as short as possible. Sales already
                    // acknowledged are gone from the log and are the server's to
                    // number, so an emptied log correctly reports none waiting.
                    None => unnumbered = unnumbered.saturating_add(1),
                }
            }
        }
        if let Some(next) = highest_used {
            book.resume_at(next);
        }
        book.resume_unnumbered(unnumbered);

        Ok(Standing {
            leases: book,
            held,
            auth,
            token,
            shop,
            wallets,
            unsent_shifts,
            unsent_allowed,
            allowed_seq,
            customers,
            credential,
        })
    }

    /// Who the shop lets buy on account, by the name a screen should offer.
    ///
    /// Held on the device, like the people who may sign in, because a sale on
    /// account is written with the internet down and a name typed from memory
    /// is how one Karim ends up paying for another Karim's rice.
    #[must_use]
    pub fn customers(&self) -> &[wire::CustomerV1] {
        &self.customers
    }

    /// Take what the shop says each of them owes.
    pub fn set_balances(&mut self, balances: Vec<(u128, i64)>, at_ms: u64) {
        self.balances = balances;
        self.balances_at_ms = Some(at_ms);
    }

    /// What somebody owed when the shop last said, and when that was.
    ///
    /// Both, always: a number without its age is a number a cashier reads out
    /// as though it were true, and another till may have sold to this person
    /// since.
    #[must_use]
    pub fn owed_by(&self, customer: Ulid) -> Option<(Minor, u64)> {
        let at_ms = self.balances_at_ms?;
        self.balances
            .iter()
            .find(|(id, _)| *id == customer.to_u128())
            .map(|(_, owed)| (Minor::new(*owed), at_ms))
    }

    /// Take the shop's list of who may buy on account.
    pub fn set_customers(&mut self, customers: Vec<wire::CustomerV1>) -> Result<()> {
        self.customers = customers;
        self.persist_terminal_state()
    }

    /// Say which of them this basket is for.
    ///
    /// Named on the ticket rather than only in the tender's reference, so what
    /// somebody owes is added up against a person the shop has a record of
    /// rather than against the spelling a cashier used that day.
    pub fn set_customer(&mut self, customer: Option<Ulid>) -> Result<()> {
        if let Some(id) = customer
            && !self
                .customers
                .iter()
                .any(|known| known.id == id.to_u128() && known.active)
        {
            return Err(TillError::UnknownCustomer);
        }
        self.cart.set_customer(customer);
        Ok(())
    }

    /// Who this basket is for, if anybody.
    #[must_use]
    pub fn customer(&self) -> Option<Ulid> {
        self.cart.customer()
    }

    /// The wallets this shop takes, by the name a report should read.
    #[must_use]
    pub fn wallets(&self) -> &[Box<str>] {
        &self.wallets
    }

    /// The shop, as its receipts describe it.
    #[must_use]
    pub fn shop(&self) -> Option<&crate::receipt::Shop> {
        self.shop.as_ref()
    }

    /// Record the shop's details, durably.
    ///
    /// A shop with no name is refused. It would print a receipt with an empty
    /// line where the shop should be, which looks like a printer fault and is
    /// not something a customer can take back to anybody.
    /// The shop's own details, and what it takes money by.
    ///
    /// The wallets travel with the shop rather than separately because they
    /// arrive together and are wanted together: a till that knows the shop's
    /// name but not that it takes bKash is a till a cashier has to spell it at.
    pub fn set_shop(&mut self, shop: crate::receipt::Shop, wallets: Vec<Box<str>>) -> Result<()> {
        if shop.name.trim().is_empty() {
            return Err(TillError::NamelessShop);
        }
        let held = core::mem::replace(&mut self.wallets, wallets);
        let previous = self.shop.replace(shop);
        if let Err(error) = self.persist_terminal_state() {
            self.shop = previous;
            self.wallets = held;
            return Err(error);
        }
        Ok(())
    }

    /// The credential this terminal syncs with, if it has been enrolled.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Record the credential enrolment returned, durably.
    ///
    /// Written to standing state at once rather than at the next convenient
    /// moment: a device that enrolled, was told it had, and then lost the
    /// credential to a power cut would need the owner to issue another code,
    /// and would give no clue why.
    pub fn set_token(&mut self, token: &str) -> Result<()> {
        let previous = self.token.replace(alloc::string::String::from(token));
        if let Err(error) = self.persist_terminal_state() {
            self.token = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Take a credential and write down when it was taken.
    ///
    /// The time matters as much as the credential: one expires, and a device
    /// that does not know how old its own is cannot renew before it stops
    /// working. `lifetime_ms` is what the shop said one lasts, and is zero on
    /// enrolment because nothing has said yet.
    pub fn take_credential(&mut self, token: &str, at_ms: u64, lifetime_ms: u64) -> Result<()> {
        let previous_token = self.token.replace(alloc::string::String::from(token));
        let previous_note = self.credential.replace(wire::CredentialV1 {
            taken_at_ms: at_ms,
            lifetime_ms,
        });
        if let Err(error) = self.persist_terminal_state() {
            self.token = previous_token;
            self.credential = previous_note;
            return Err(error);
        }
        Ok(())
    }

    /// When this device's credential was taken, and how long the shop says one
    /// lasts. Absent on a device enrolled by a build that did not write it down.
    #[must_use]
    pub fn credential_age(&self) -> Option<(u64, u64)> {
        self.credential
            .map(|note| (note.taken_at_ms, note.lifetime_ms))
    }

    /// Rebuild the open drawer by replaying the log in order.
    ///
    /// Events rather than a stored shift total: the sales are already frames in
    /// this log, so replaying them is what makes the drawer figure and the sales
    /// figure agree by construction. A stored total could only ever disagree
    /// with the sales it claims to summarise, and then there would be no way to
    /// tell which was right.
    fn recover_shift(journal: &Journal<B>, terminal: TerminalId) -> Result<Option<Shift>> {
        let mut shift: Option<Shift> = None;
        for record in journal.read(Store::Critical)? {
            match record.header.kind {
                PayloadKind::ShiftEvent => {
                    let event = wire::decode_shift_event(record.header.schema, &record.payload)?;
                    match event {
                        ShiftEventV1::Opened {
                            id,
                            terminal: on,
                            opening_float_minor,
                            at_ms,
                        } => {
                            shift = Some(Shift::open(
                                Ulid::from_u128(id),
                                Ulid::from_u128(on),
                                Minor::new(opening_float_minor),
                                at_ms,
                            )?);
                        }
                        ShiftEventV1::CashMoved {
                            inward,
                            amount_minor,
                            reason,
                            at_ms,
                        } => {
                            if let Some(open) = shift.as_mut() {
                                let amount = Minor::new(amount_minor);
                                if inward {
                                    open.cash_in(amount, &reason, at_ms)?;
                                } else {
                                    open.cash_out(amount, &reason, at_ms)?;
                                }
                            }
                        }
                        ShiftEventV1::Closed {
                            counted_cash_minor,
                            at_ms,
                        } => {
                            if let Some(open) = shift.as_mut() {
                                open.close(Minor::new(counted_cash_minor), at_ms)?;
                            }
                        }
                    }
                }
                PayloadKind::SaleCommit => {
                    if let Some(open) = shift.as_mut().filter(|open| open.is_open()) {
                        let sale: SaleCommitV1 =
                            wire::decode_sale(record.header.schema, &record.payload)?;
                        // Read before the ticket is consumed. With the change,
                        // which left the drawer as surely as the note came into
                        // it: replaying without it would make a rebuilt drawer
                        // disagree with the one the cashier watched all day.
                        let change = Minor::new(sale.ticket.change_minor);
                        let (_lines, tenders) = sale.ticket.lines_and_tenders()?;
                        open.record_sale(&tenders, change)?;
                    }
                }
                _ => {}
            }
        }
        let _ = terminal;
        Ok(shift)
    }

    /// Write down what this terminal owns.
    ///
    /// Called after anything that changes the blocks in hand or the baskets on
    /// the counter, and always before the critical log is emptied. The blob is
    /// A/B, so a device dying mid-write comes back one step stale rather than
    /// with nothing.
    fn persist_terminal_state(&mut self) -> Result<()> {
        let mut leases = Vec::new();
        for lease in self.leases.blocks() {
            leases.push(LeaseGrantV1 {
                terminal: lease.terminal.to_u128(),
                epoch: lease.epoch,
                prefix: alloc::string::String::from(&*lease.prefix),
                first: lease.next,
                last: lease.last,
            });
        }
        let bytes = wire::encode_terminal_state(&TerminalStateV1 {
            unsent_shifts: self.unsent_shifts.clone(),
            unsent_allowed: self.unsent_allowed.clone(),
            allowed_seq: self.allowed_seq,
            customers: self.customers.clone(),
            credential: self.credential,
            leases,
            held: self.held.clone(),
            unnumbered: self.leases.unnumbered(),
            token: self.token.clone(),
            shop: self.shop.as_ref().map(|shop| wire::ShopV1 {
                name: shop.name.clone(),
                bin: shop.bin.clone(),
                address: shop.address.clone(),
                phone: shop.phone.clone(),
                wallets: self.wallets.iter().map(ToString::to_string).collect(),
            }),
            operators: self
                .auth
                .operators()
                .iter()
                .map(OperatorV1::from_domain)
                .collect(),
        })?;
        self.journal.write_terminal_state(&bytes)?;
        Ok(())
    }

    // -- catalogue -----------------------------------------------------------

    #[must_use]
    pub fn catalogue(&self) -> &Replica {
        &self.replica
    }

    /// Look up a scanned barcode. The hot path: no I/O, no allocation.
    #[must_use]
    pub fn lookup(&self, barcode: &str) -> Option<&Item> {
        self.replica.by_barcode(barcode)
    }

    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Item> {
        self.replica.search(query, limit)
    }

    // -- selling -------------------------------------------------------------

    #[must_use]
    pub fn cart(&self) -> &Cart {
        &self.cart
    }

    /// The catalogue this device holds.
    ///
    /// Read-only on purpose: a caller can look items up and search them, and
    /// changes still arrive the one way they always have, through a pull.
    #[must_use]
    pub fn replica(&self) -> &Replica {
        &self.replica
    }

    /// Scan a barcode straight onto the ticket.
    pub fn scan(&mut self, barcode: &str, qty: Milli) -> Result<usize> {
        let item = self
            .replica
            .by_barcode(barcode)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        self.ring(item, qty)
    }

    /// Put an item on the ticket by its id, for a cashier who looked it up
    /// rather than scanned it.
    ///
    /// A barcode that will not read, loose goods that carry none, a label torn
    /// off: the shop still has to sell the thing. Shares the rules below with
    /// scanning rather than repeating them, because a second way in that forgot
    /// one of them would be a way to sell what the shop has withdrawn.
    pub fn add(&mut self, id: crate::replica::ItemId, qty: Milli) -> Result<usize> {
        let item = self
            .replica
            .by_id(id)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        self.ring(item, qty)
    }

    fn ring(&mut self, item: Item, qty: Milli) -> Result<usize> {
        // A discontinued item cannot be sold and must still be refundable: the
        // shop sold it last week and the customer is standing there with it.
        // Until this existed the flag was honoured by search and ignored by the
        // one lookup that takes money.
        if !item.active && !self.cart.is_refund() {
            return Err(TillError::NoLongerSold);
        }
        Ok(self.cart.add_item(&item, qty)?)
    }

    pub fn set_qty(&mut self, line: usize, qty: Milli) -> Result<()> {
        Ok(self.cart.set_qty(line, qty)?)
    }

    pub fn remove_line(&mut self, line: usize) -> Result<()> {
        self.cart.remove_line(line)?;
        Ok(())
    }

    /// Sell one line at a different price.
    ///
    /// For damaged goods, a short weight, a price a customer was quoted. The
    /// cart has enforced the permission since it was written and the facade did
    /// not forward it, so a supervisor who may override a price had no way to.
    pub fn set_unit_price(&mut self, line: usize, price: Minor) -> Result<()> {
        Ok(self.cart.set_unit_price(line, price)?)
    }

    pub fn set_line_discount(&mut self, line: usize, discount: Discount) -> Result<()> {
        Ok(self.cart.set_line_discount(line, discount)?)
    }

    pub fn set_ticket_discount(&mut self, discount: Discount) -> Result<()> {
        Ok(self.cart.set_ticket_discount(discount)?)
    }

    /// Take back the money entered so far.
    ///
    /// For a mis-keyed amount: five thousand typed instead of five hundred
    /// cannot be unwound by entering more. The cart has been able to do this
    /// since it was written and the facade did not forward it, so nothing on any
    /// screen could reach it.
    pub fn clear_tenders(&mut self) {
        self.cart.clear_tenders();
    }

    /// Take money, or a promise of it.
    ///
    /// A credit tender naming somebody the shop has written down, on a basket
    /// that is not pointed at them, is refused. It reads as harmless and is
    /// not: what somebody owes is added up against their record, and a typed
    /// name is added up against itself, so the two live in different places.
    /// The shop then has a customer who owes for what they took and a phantom
    /// of the same name holding what they brought back, which is exactly what
    /// happened the first time a return was rung on account here.
    pub fn add_tender(&mut self, tender: Tender) -> Result<()> {
        if tender.kind == TenderKind::Credit
            && self.cart.customer().is_none()
            && let Some(named) = tender.reference.as_deref()
            && let Some(known) = self.customer_called(named)
        {
            return Err(TillError::WriteItAgainstThem { name: known });
        }
        // And refused here as well as at the close, because here is where the
        // cashier can still see what they typed. A promise or a card that goes
        // past the basket cannot be given back as change, and finding that out
        // only when the sale is closed means retyping the whole tender with a
        // customer waiting.
        self.cart.would_overpay(&tender)?;
        self.cart.add_tender(tender);
        Ok(())
    }

    /// The name as the shop wrote it, when a typed one means somebody on the
    /// list. Folded the way the account book folds: case and spacing do not
    /// make two people.
    fn customer_called(&self, typed: &str) -> Option<alloc::string::String> {
        let wanted = crate::accounts::account_key(typed);
        self.customers
            .iter()
            .filter(|known| known.active)
            .find(|known| crate::accounts::account_key(&known.name) == wanted)
            .map(|known| known.name.clone())
    }

    pub fn totals(&self) -> Result<TicketTotals> {
        Ok(self.cart.totals()?)
    }

    pub fn balance_due(&self) -> Result<Minor> {
        Ok(self.cart.balance_due()?)
    }

    pub fn change_due(&self) -> Result<Minor> {
        Ok(self.cart.change_due()?)
    }

    /// Abandon the sale in progress.
    pub fn cancel_sale(&mut self) {
        self.lower_limits_to_the_cashier();
        self.cart = Cart::new(self.limits);
    }

    // -- who is at the till -------------------------------------------------

    /// Replace everyone this till knows about, and write them down.
    ///
    /// The whole set rather than one at a time, because that is what the server
    /// sends: somebody removed from the shop has to disappear from the till,
    /// and a list that only ever grows would leave a departed cashier able to
    /// sign in forever.
    pub fn set_operators(&mut self, operators: Vec<Operator>) -> Result<()> {
        let previous = core::mem::replace(&mut self.auth, AuthBook::new());
        for operator in operators {
            self.auth.put(operator);
        }
        if let Err(error) = self.persist_terminal_state() {
            self.auth = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Add or replace an operator, and write them down.
    ///
    /// Persisted immediately rather than at the next sync, because the whole
    /// point of holding credentials on the device is that a cashier can sign in
    /// tomorrow morning with the internet still down.
    pub fn put_operator(&mut self, operator: Operator) -> Result<()> {
        // The nil id is what a record carries when it means nobody: a drawer
        // counted before the till wrote down who counted it says zero. A person
        // holding that id would read back as nobody having counted.
        if operator.id.to_u128() == 0 {
            return Err(TillError::NamelessOperator);
        }
        self.auth.put(operator);
        self.persist_terminal_state()
    }

    /// Sign in with a PIN. The cart's ceilings follow from who signed in, so a
    /// cashier cannot be given permissions by a UI that forgot to ask.
    pub fn sign_in(&mut self, id: crate::auth::OperatorId, pin: &str, now_ms: u64) -> Result<()> {
        let outcome = self.auth.sign_in(id, pin, now_ms);
        // Kept whether the PIN was right or wrong, and before the refusal is
        // handed back: a wrong PIN is the thing worth writing down here, and a
        // caller that returns early would drop it.
        self.keep_what_was_allowed()?;
        outcome?;
        if let Some(operator) = self.auth.signed_in() {
            self.limits = CartLimits {
                max_discount: crate::money::Bp::new(operator.permissions.max_discount_bp)
                    .unwrap_or(crate::money::Bp::ZERO),
                allow_price_override: operator.permissions.may_override_price,
            };
            // An empty cart takes the new limits at once. A cart with something
            // in it keeps the ones it was rung under, because repricing a
            // basket because somebody changed shift is worse than either.
            if self.cart.is_empty() {
                self.cart = Cart::new(self.limits);
            }
        }
        Ok(())
    }

    pub fn sign_out(&mut self) {
        self.auth.sign_out();
        self.limits = CartLimits::default();
    }

    #[must_use]
    pub fn signed_in(&self) -> Option<&Operator> {
        self.auth.signed_in()
    }

    /// A supervisor puts their PIN in to allow the cashier one action.
    /// A supervisor allows the cashier one thing.
    ///
    /// Two of these are not checked where the others are. A refund, a void, the
    /// drawer and the close all go through the auth book, which knows about
    /// authorisations. A discount and a price override are stopped by the
    /// cart's own ceilings, which were set when the cashier signed in and knew
    /// nothing about any of this: the supervisor typed their PIN, the till said
    /// yes, and the discount was refused again. So the ceiling is raised here
    /// too, for the basket on the screen.
    ///
    /// For that basket and no longer. The supervisor is standing at the counter
    /// now and will not be there for the next customer, so finishing or
    /// abandoning the sale puts the ceilings back where the cashier's own
    /// permissions leave them.
    pub fn authorise(
        &mut self,
        supervisor: crate::auth::OperatorId,
        pin: &str,
        action: Action,
        now_ms: u64,
        valid_for_ms: u64,
    ) -> Result<()> {
        let outcome = self
            .auth
            .authorise(supervisor, pin, action, now_ms, valid_for_ms);
        // As with signing in: a supervisor's wrong PIN is written down before
        // the refusal goes back to the caller.
        self.keep_what_was_allowed()?;
        outcome?;
        // The cart has carried this since it was written: it lifts its ceilings
        // for the rest of the ticket and writes the reason onto the ticket, so
        // the waiver is on the customer's paper and in the shop's copy. Nothing
        // called it, which is why a supervisor could type their PIN, be told
        // yes, and watch the discount refused again.
        if matches!(action, Action::Discount { .. } | Action::OverridePrice) {
            let who = self
                .auth
                .operators()
                .iter()
                .find(|operator| operator.id == supervisor)
                .map(|operator| operator.name.to_string())
                .unwrap_or_default();
            let reason = match action {
                Action::Discount { bp } => {
                    alloc::format!("{who} allowed a discount of {bp} basis points")
                }
                _ => alloc::format!("{who} allowed a price to be typed over the catalogue's"),
            };
            self.cart.authorise_override(&reason);
            // Written down here because the auth book never sees these used:
            // the cart's own ceilings stop them, so the moment worth recording
            // is the supervisor allowing it. Without this, the only record of
            // who allowed a discount is prose on the ticket, and a ticket the
            // customer walked out with is not an accountability record.
            let bp = match action {
                Action::Discount { bp } => bp,
                _ => 0,
            };
            let code = if matches!(action, Action::Discount { .. }) {
                1
            } else {
                2
            };
            let cashier = self.auth.signed_in().map_or(supervisor, |who| who.id);
            self.write_down_allowed(now_ms, code, bp, cashier, Some(supervisor));
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// Keep whatever the auth book has just written down, for the shop.
    ///
    /// The book records a privileged action as it allows it, which is the right
    /// place: a caller that forgets is an action with nobody's name on it. What
    /// it could not do is outlive the process, so this copies each entry into
    /// the standing state, where it waits with the counted drawers until the
    /// shop has it.
    fn keep_what_was_allowed(&mut self) -> Result<()> {
        let fresh: Vec<crate::auth::AuditEntry> = self
            .auth
            .audit()
            .iter()
            .skip(self.taken_audit)
            .cloned()
            .collect();
        let refused: Vec<crate::auth::Refusal> = self
            .auth
            .refusals()
            .iter()
            .skip(self.taken_refusals)
            .copied()
            .collect();
        if fresh.is_empty() && refused.is_empty() {
            return Ok(());
        }
        self.taken_refusals = self.auth.refusals().len();
        for one in refused {
            // 7, 8 and 9 rather than variants of the permission enum: typing a
            // PIN, rightly or wrongly, is not an action anybody may be
            // permitted to take, and putting it in that enum would mean
            // writing a permission for it.
            let code = match (one.signed_in, one.locked_out) {
                (true, _) => 9,
                (false, true) => 8,
                (false, false) => 7,
            };
            self.write_down_allowed(one.at_ms, code, 0, one.operator, None);
        }
        self.taken_audit = self.auth.audit().len();
        for entry in fresh {
            let (code, bp) = match entry.action {
                Action::Discount { bp } => (1_u8, bp),
                Action::OverridePrice => (2, 0),
                Action::Refund => (3, 0),
                Action::VoidLine => (4, 0),
                Action::OpenDrawer => (5, 0),
                Action::CloseShift => (6, 0),
            };
            self.write_down_allowed(entry.at_ms, code, bp, entry.operator, entry.authorised_by);
        }
        self.persist_terminal_state()
    }

    /// One line of the trail, with the names as they stand now.
    ///
    /// The names are copied rather than looked up later, for the reason the
    /// counted drawer's is: somebody since renamed, or gone from the shop, is
    /// still the person this belongs to.
    fn write_down_allowed(
        &mut self,
        at_ms: u64,
        action: u8,
        bp: u32,
        operator: crate::auth::OperatorId,
        authorised_by: Option<crate::auth::OperatorId>,
    ) {
        let name_of = |id: crate::auth::OperatorId| {
            self.auth
                .operators()
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.name.to_string())
                .unwrap_or_default()
        };
        let operator_name = name_of(operator);
        let authorised_by_name = authorised_by.map(name_of).unwrap_or_default();
        self.allowed_seq = self.allowed_seq.saturating_add(1);
        self.unsent_allowed.push(wire::AllowedV1 {
            seq: self.allowed_seq,
            at_ms,
            action,
            bp,
            operator: operator.to_u128(),
            operator_name,
            // Zero when nobody had to allow it: the cashier's own ceiling
            // covered it, which is a different fact from a supervisor standing
            // at the counter.
            authorised_by: authorised_by.map_or(0, crate::ids::Ulid::to_u128),
            authorised_by_name,
        });
    }

    /// What this device allowed and the shop has not been told about.
    #[must_use]
    pub fn unsent_allowed(&self) -> &[wire::AllowedV1] {
        &self.unsent_allowed
    }

    /// Forget the entries the shop now holds.
    ///
    /// Called with what the server said it stored, never with what was sent: a
    /// reply that did not arrive must leave the trail here to be sent again.
    pub fn allowed_accepted(&mut self, stored: &[u64]) -> Result<()> {
        let before = self.unsent_allowed.len();
        self.unsent_allowed.retain(|one| !stored.contains(&one.seq));
        if self.unsent_allowed.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// Put the ceilings back to what the cashier's own permissions allow.
    ///
    /// Called wherever a basket ends. An authorisation is for the sale in front
    /// of the supervisor, not for the rest of the shift.
    fn lower_limits_to_the_cashier(&mut self) {
        self.limits = self
            .auth
            .signed_in()
            .map(|who| CartLimits {
                max_discount: crate::money::Bp::new(who.permissions.max_discount_bp)
                    .unwrap_or(crate::money::Bp::ZERO),
                allow_price_override: who.permissions.may_override_price,
            })
            .unwrap_or_default();
    }

    /// Everyone this till knows about.
    ///
    /// An empty list is a different problem from a wrong PIN, and a screen that
    /// cannot tell them apart sends a shopkeeper looking for a forgotten
    /// password when the truth is that nobody has been added yet.
    #[must_use]
    pub fn people(&self) -> &[Operator] {
        self.auth.operators()
    }

    /// Privileged actions taken on this terminal, and on whose authority.
    #[must_use]
    /// What this device has allowed since it started, as the auth book wrote it.
    ///
    /// The transient copy. What a shop reads is the durable one: each of these
    /// is written into the standing state as it happens and sent, so a device
    /// restarted overnight has still told the shop. This is here for a platform
    /// that wants to show the last few actions on the device itself, offline,
    /// without asking anybody.
    pub fn audit(&self) -> &[crate::auth::AuditEntry] {
        self.auth.audit()
    }

    // -- the drawer -----------------------------------------------------------

    /// Open the drawer for the day with a counted float.
    pub fn open_shift(&mut self, id: ShiftId, opening_float: Minor, at_ms: u64) -> Result<()> {
        let shift = Shift::open(id, self.terminal, opening_float, at_ms)?;
        self.commit_shift_event(&ShiftEventV1::Opened {
            id: id.to_u128(),
            terminal: self.terminal.to_u128(),
            opening_float_minor: opening_float.get(),
            at_ms,
        })?;
        self.shift = Some(shift);
        Ok(())
    }

    #[must_use]
    pub fn shift(&self) -> Option<&Shift> {
        self.shift.as_ref()
    }

    /// Put cash in for a stated reason.
    pub fn cash_in(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(true, amount, reason, at_ms)
    }

    /// Take cash out for a stated reason.
    pub fn cash_out(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(false, amount, reason, at_ms)
    }

    fn move_cash(&mut self, inward: bool, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.auth.check(Action::OpenDrawer, at_ms)?;
        self.keep_what_was_allowed()?;
        let shift = self.shift.as_mut().ok_or(TillError::NoOpenShift)?;

        // Applied to a copy first, so a movement the shift refuses is not
        // written to the log where the next boot would replay it.
        let mut next = shift.clone();
        if inward {
            next.cash_in(amount, reason, at_ms)?;
        } else {
            next.cash_out(amount, reason, at_ms)?;
        }

        self.commit_shift_event(&ShiftEventV1::CashMoved {
            inward,
            amount_minor: amount.get(),
            reason: alloc::string::String::from(reason),
            at_ms,
        })?;
        self.shift = Some(next);
        Ok(())
    }

    /// Totals so far, leaving the drawer open.
    pub fn x_report(&self) -> Result<XReport> {
        let shift = self.shift.as_ref().ok_or(TillError::NoOpenShift)?;
        Ok(shift.x_report()?)
    }

    /// Count the drawer and close the shift.
    pub fn close_shift(&mut self, counted_cash: Minor, at_ms: u64) -> Result<ZReport> {
        self.auth.check(Action::CloseShift, at_ms)?;
        self.keep_what_was_allowed()?;
        // Taken before anything else moves, so the record names whoever was
        // standing at the till when it was counted. A variance attached to a
        // terminal and a time is half of what an owner wants to know.
        let (counted_by, counted_by_name) = self
            .auth
            .signed_in()
            .map_or((0, alloc::string::String::new()), |who| {
                (who.id.to_u128(), who.name.to_string())
            });
        let shift = self.shift.as_mut().ok_or(TillError::NoOpenShift)?;

        let mut next = shift.clone();
        let report = next.close(counted_cash, at_ms)?;

        self.commit_shift_event(&ShiftEventV1::Closed {
            counted_cash_minor: counted_cash.get(),
            at_ms,
        })?;

        // Written down for sending before the caller is told it closed. The
        // count is the thing somebody who was not at the till reconciles, and a
        // device that reported "closed" and kept it to itself is the situation
        // this exists to end.
        self.unsent_shifts.push(wire::ClosedShiftV1 {
            id: report.totals.shift.to_u128(),
            closed_by: counted_by,
            closed_by_name: counted_by_name,
            opened_at_ms: report.totals.opened_at_ms,
            closed_at_ms: report.closed_at_ms,
            opening_float_minor: report.totals.opening_float.get(),
            sales: u32::try_from(report.totals.sales).unwrap_or(u32::MAX),
            cash_sales_minor: report.totals.cash_sales.get(),
            non_cash_sales_minor: report.totals.non_cash_sales.get(),
            cash_in_minor: report.totals.cash_in.get(),
            cash_out_minor: report.totals.cash_out.get(),
            expected_cash_minor: report.totals.expected_cash.get(),
            counted_cash_minor: report.counted_cash.get(),
            variance_minor: report.variance.get(),
        });
        self.persist_terminal_state()?;

        self.shift = Some(next);
        Ok(report)
    }

    /// Drawers counted and closed that the shop has not been told about.
    #[must_use]
    pub fn unsent_shifts(&self) -> &[wire::ClosedShiftV1] {
        &self.unsent_shifts
    }

    /// Forget the drawers the shop now holds.
    ///
    /// Called with what the server said it accepted, never with what was sent:
    /// a reply that did not arrive must leave the count here to be sent again.
    pub fn shifts_accepted(&mut self, accepted: &[u128]) -> Result<()> {
        let before = self.unsent_shifts.len();
        self.unsent_shifts
            .retain(|shift| !accepted.contains(&shift.id));
        if self.unsent_shifts.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    fn commit_shift_event(&mut self, event: &ShiftEventV1) -> Result<()> {
        let bytes = wire::encode_shift_event(event)?;
        self.journal.commit(
            Store::Critical,
            PayloadKind::ShiftEvent,
            SHIFT_SCHEMA,
            &bytes,
        )?;
        Ok(())
    }

    /// Baskets currently set aside, newest first.
    pub fn held_tickets(&self) -> Result<Vec<HeldTicket>> {
        let mut listed = Vec::with_capacity(self.held.tickets.len());
        for held in &self.held.tickets {
            let lines = held
                .lines
                .iter()
                .cloned()
                .map(LineV1::into_domain)
                .collect::<core::result::Result<Vec<_>, WireError>>()?;
            let totals = ticket_totals(&TicketInput {
                lines: lines.iter().map(CartLine::as_input).collect(),
                ticket_discount: held.ticket_discount.clone().into_domain()?,
            })
            .map_err(|error| TillError::Cart(CartError::Money(error)))?;
            let total = totals.total;
            listed.push(HeldTicket {
                id: Ulid::from_u128(held.id),
                held_at_ms: held.held_at_ms,
                label: held.label.clone(),
                lines: held.lines.len(),
                total,
            });
        }
        listed.sort_by_key(|held| core::cmp::Reverse(held.held_at_ms));
        Ok(listed)
    }

    /// Set the basket aside so the next customer can be served.
    ///
    /// Persisted before the cart is cleared. A cashier who parks a basket and
    /// then loses power has not lost it, which is the whole reason to write it
    /// down rather than keep it in memory: the customer is still standing there.
    pub fn hold(&mut self, id: TicketId, held_at_ms: u64, label: &str) -> Result<()> {
        if self.cart.is_empty() {
            return Err(TillError::NothingToHold);
        }

        let mut next = self.held.clone();
        next.tickets.push(HeldTicketV1 {
            id: id.to_u128(),
            held_at_ms,
            customer: self.cart.customer().map(Ulid::to_u128),
            label: label.into(),
            lines: self.cart.lines().iter().map(LineV1::from_domain).collect(),
            ticket_discount: DiscountV1::from_domain(self.cart.ticket_discount()),
        });

        self.persist_held(&next)?;
        self.held = next;
        self.cart = Cart::new(self.limits);
        Ok(())
    }

    /// Bring a parked basket back to the screen.
    ///
    /// Refuses while something is already rung, rather than silently merging or
    /// discarding it. Two baskets on one screen is how a customer ends up paying
    /// for somebody else's shopping.
    pub fn resume(&mut self, id: TicketId) -> Result<()> {
        if !self.cart.is_empty() {
            return Err(TillError::TicketInProgress);
        }
        let position = self
            .held
            .tickets
            .iter()
            .position(|held| held.id == id.to_u128())
            .ok_or(TillError::NoSuchHeldTicket)?;

        let held = self
            .held
            .tickets
            .get(position)
            .ok_or(TillError::NoSuchHeldTicket)?
            .clone();

        // Remove it from the parked set first. A basket that is both on screen
        // and in the parked list can be rung twice.
        let mut next = self.held.clone();
        next.tickets.remove(position);
        self.persist_held(&next)?;
        self.held = next;

        let mut cart = Cart::new(self.limits);
        cart.set_customer(held.customer.map(Ulid::from_u128));
        for line in held.lines {
            cart.restore_line(LineV1::into_domain(line)?);
        }
        cart.set_ticket_discount(held.ticket_discount.into_domain()?)?;
        self.cart = cart;
        Ok(())
    }

    /// Throw away a parked basket the customer never came back for.
    pub fn discard_held(&mut self, id: TicketId) -> Result<()> {
        let position = self
            .held
            .tickets
            .iter()
            .position(|held| held.id == id.to_u128())
            .ok_or(TillError::NoSuchHeldTicket)?;

        let mut next = self.held.clone();
        next.tickets.remove(position);
        self.persist_held(&next)?;
        self.held = next;
        Ok(())
    }

    fn persist_held(&mut self, held: &HeldTicketsV1) -> Result<()> {
        let previous = core::mem::replace(&mut self.held, held.clone());
        // Rolled back on failure so the in-memory list can never claim a basket
        // that was not written down.
        if let Err(error) = self.persist_terminal_state() {
            self.held = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Turn the ticket in progress into a refund.
    ///
    /// Scanning then works exactly as it does for a sale: the cashier passes the
    /// goods over the same scanner and the till negates the quantities. Stock
    /// goes back on the shelf when the refund commits, by the same path a sale
    /// takes it off.
    ///
    /// Gated on the refund permission, and refused for a cashier who has none
    /// unless a supervisor has authorised this one. Money leaving the drawer is
    /// the action the permission model exists for.
    pub fn start_refund(&mut self, original_receipt: Option<&str>, now_ms: u64) -> Result<()> {
        self.auth.check(Action::Refund, now_ms)?;
        self.keep_what_was_allowed()?;
        Ok(self.cart.start_refund(original_receipt)?)
    }

    /// Close the sale.
    ///
    /// The order is deliberate and is the reason this method exists:
    ///
    /// 1. close the cart into an immutable ticket
    /// 2. take a receipt number from the leased block
    /// 3. commit the whole thing durably, in one frame
    /// 4. only then move stock and clear the cart
    ///
    /// If step 3 fails, the receipt number is put back and the cart is left
    /// exactly as it was, so the cashier can retry without re-ringing the basket
    /// and without burning a number. Moving stock before the commit would leave
    /// the on-screen quantity wrong after a failed sale.
    pub fn checkout(&mut self, id: TicketId, rung_at_ms: u64) -> Result<CompletedSale> {
        let mut ticket = self.cart.close(id, self.terminal, rung_at_ms)?;

        // Take a number, but keep the book as it was in case the commit fails.
        let book_before = self.leases.clone();
        let number = self.leases.consume();
        let epoch = number.as_ref().map(|issued| issued.epoch);
        let lease_next = self.leases.active().map(|lease| lease.next);
        ticket.receipt_no = number.as_ref().map(|issued| issued.text.as_str().into());

        let payload = wire::sale_commit(&ticket, epoch, lease_next);
        let bytes = wire::encode_sale(&payload)?;

        let sequence = match self.journal.commit(
            Store::Critical,
            PayloadKind::SaleCommit,
            SALE_SCHEMA,
            &bytes,
        ) {
            Ok(sequence) => sequence,
            Err(error) => {
                // Nothing happened. Give the number back and leave the basket be.
                self.leases = book_before;
                return Err(error.into());
            }
        };

        // Durable. Only now may in-memory state move, and a receipt be printed.
        for line in &ticket.lines {
            let sold = line.qty.get().saturating_neg();
            self.replica.adjust_on_hand(line.item_id, Milli::new(sold));
        }

        // Into the drawer, if one is open. A sale is never refused for want of
        // an open shift: a till that will not sell because nobody pressed the
        // right button in the morning is a till the shop works around.
        // Recovery replays the same sale frames in the same order, so the
        // in-memory figure and the one rebuilt after a reboot agree.
        if let Some(shift) = self.shift.as_mut().filter(|shift| shift.is_open()) {
            shift.record_sale(&ticket.tenders, ticket.change)?;
        }

        self.cart = Cart::new(self.limits);

        Ok(CompletedSale {
            receipt_no: number.map(|issued| issued.text),
            ticket,
            journal_sequence: sequence,
        })
    }

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
            self.persist_terminal_state()?;
            self.journal.truncate_critical(0)?;
        }
        Ok(outcome.confirmed)
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
    use crate::domain::PriceMode;
    use crate::money::Bp;
    use crate::storage::backend::{Fault, FaultyBackend, MemoryBackend};
    use crate::storage::wire::ItemV1;

    const TENANT: u128 = 42;

    fn terminal() -> TerminalId {
        Ulid::from_u128(7)
    }

    fn item(seed: u128, price: i64) -> Item {
        Item {
            id: Ulid::from_u128(seed),
            code: alloc::format!("SKU{seed:03}").into_boxed_str(),
            name_en: "Rice Miniket 5kg".into(),
            name_bn: "মিনিকেট চাল ৫ কেজি".into(),
            unit: "Nos".into(),
            price: Minor::new(price),
            cost: Minor::new(price / 2),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            vat_base: crate::domain::VatBase::Discounted,
            barcodes: vec![alloc::format!("869000000{seed:04}").into_boxed_str()],
            on_hand: Milli::new(40_000),
            active: true,
        }
    }

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
            held: crate::storage::wire::HeldTicketsV1::default(),
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

    #[test]
    fn an_item_the_shop_stopped_selling_cannot_be_rung_and_can_still_be_refunded() {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        // A supervisor, because starting a refund needs the permission and this
        // test is about the item, not about who may refund.
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        let mut retired = item(1, 43_000);
        retired.active = false;
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&retired)],
            tombstones: vec![],
        })
        .unwrap();
        let barcode = alloc::string::String::from(&*retired.barcodes[0]);

        // The flag was honoured by search and ignored by the lookup that takes
        // money, so a discontinued item went on selling to anyone holding a box
        // of it.
        assert_eq!(
            till.scan(&barcode, Milli::ONE),
            Err(TillError::NoLongerSold)
        );

        // And refusing it outright would be worse: the shop sold this last week
        // and the customer is standing there with it.
        till.start_refund(Some("T1-000100"), 0).unwrap();
        assert!(till.scan(&barcode, Milli::ONE).is_ok());
    }

    fn stocked_till(backend: MemoryBackend) -> Till<MemoryBackend> {
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        // Five hundred numbers, which is the block size the design assumes: big
        // enough to cross a long offline day without renewal.
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 599))
            .unwrap();
        // A supervisor at the counter, which is what a one-person shop is. The
        // permission checks are live in every test below because of this line.
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        till
    }

    /// Weak on purpose: the shipped round count is deliberately slow, and this
    /// suite signs in on every test.
    const TEST_ROUNDS: u32 = 16;

    fn supervisor_operator() -> Operator {
        Operator {
            id: Ulid::from_u128(70),
            name: "Owner".into(),
            pin: crate::auth::PinHash::derive("9999", [3; crate::auth::SALT_LEN], TEST_ROUNDS),
            permissions: crate::auth::Permissions::supervisor(),
            active: true,
        }
    }

    fn pay_cash<B: Backend>(till: &mut Till<B>, amount: i64) {
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount),
            reference: None,
        })
        .unwrap();
    }

    #[test]
    fn boots_empty_and_reports_it() {
        let (_till, report) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        assert_eq!(report.items, 0);
        assert_eq!(report.unsynced_sales, 0);
        assert_eq!(report.receipt_numbers_left, 0);
        assert!(!report.repaired);
    }

    #[test]
    fn rings_a_sale_from_scan_to_receipt() {
        let mut till = stocked_till(MemoryBackend::new());

        till.scan("8690000000001", Milli::ONE).unwrap();
        assert_eq!(till.totals().unwrap().total, Minor::new(49_450));
        pay_cash(&mut till, 50_000);

        let sale = till
            .checkout(Ulid::from_u128(900), 1_788_600_000_000)
            .unwrap();
        assert_eq!(sale.receipt_no.as_deref(), Some("T1-000100"));
        assert_eq!(sale.ticket.change, Minor::new(550));

        // Stock moved, cart cleared, sale queued for the server.
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );
        assert!(till.cart().is_empty());
        assert_eq!(till.status().unwrap().unsynced_sales, 1);
    }

    #[test]
    fn an_unknown_barcode_is_reported_not_guessed() {
        let mut till = stocked_till(MemoryBackend::new());
        assert_eq!(
            till.scan("0000000000000", Milli::ONE),
            Err(TillError::UnknownBarcode)
        );
    }

    #[test]
    fn a_failed_commit_leaves_the_basket_and_the_numbers_untouched() {
        // Write operations, in order: the pull appends and flushes, the lease
        // grant appends and flushes, then the sale appends and flushes. Break
        // that last flush, which is the worst case: the bytes are written but
        // never made durable, so a naive implementation would leave a phantom
        // sale behind and burn a receipt number for it.
        let backend = FaultyBackend::new().with_fault(5, Fault::Fail);
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 599))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let numbers_before = till.leases().remaining();

        let result = till.checkout(Ulid::from_u128(900), 0);
        assert!(
            result.is_err(),
            "a sale that cannot be made durable must not succeed"
        );

        // The cashier can retry: nothing was consumed and nothing was lost.
        assert_eq!(
            till.cart().lines().len(),
            1,
            "the basket survives a failed commit"
        );
        assert_eq!(
            till.leases().remaining(),
            numbers_before,
            "no number was burned"
        );
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(40_000)),
            "stock does not move for a sale that did not happen"
        );
        assert_eq!(
            till.pending_sales(10).unwrap().len(),
            0,
            "a sale that failed to commit must never reach the server"
        );
    }

    #[test]
    fn keeps_selling_when_the_numbers_run_out() {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        // A block with exactly one number in it.
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 100))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let first = till.checkout(Ulid::from_u128(901), 0).unwrap();
        assert_eq!(first.receipt_no.as_deref(), Some("T1-000100"));

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let second = till.checkout(Ulid::from_u128(902), 0).unwrap();

        assert!(second.receipt_no.is_none(), "the sale still completes");
        assert_eq!(till.status().unwrap().unnumbered_sales, 1);
        assert_eq!(till.status().unwrap().unsynced_sales, 2, "both sales sync");
    }

    #[test]
    fn a_cold_start_restores_the_catalogue_stock_and_number_position() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            till.checkout(Ulid::from_u128(900), 0).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, report) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();

        assert_eq!(report.items, 1, "the catalogue comes back from the log");
        assert_eq!(
            report.unsynced_sales, 1,
            "the sale is still owed to the server"
        );
        assert_eq!(
            report.receipt_numbers_left, 499,
            "the used number is not reissued"
        );

        // The next number continues rather than restarting the block.
        let mut till = till;
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let next = till.checkout(Ulid::from_u128(901), 0).unwrap();
        assert_eq!(next.receipt_no.as_deref(), Some("T1-000101"));
    }

    #[test]
    fn parks_a_basket_and_brings_it_back_unchanged() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        let parked_total = till.totals().unwrap().total;

        till.hold(
            Ulid::from_u128(500),
            1_788_600_000_000,
            "Rahim, gone for cash",
        )
        .unwrap();
        assert!(
            till.cart().is_empty(),
            "the counter is free for the next customer"
        );

        let waiting = till.held_tickets().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].lines, 1);
        assert_eq!(waiting[0].total, parked_total);
        assert_eq!(&waiting[0].label, "Rahim, gone for cash");

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(till.totals().unwrap().total, parked_total);
        assert!(
            till.held_tickets().unwrap().is_empty(),
            "and it is no longer parked"
        );
    }

    #[test]
    fn a_parked_basket_survives_the_outbox_draining() {
        // Same hazard as the receipt numbers: a full acknowledgement empties the
        // critical log, and a customer who stepped out for cash has not stopped
        // existing because the shop got its internet back.
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();

            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 0, "gone for cash").unwrap();
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(till.held_tickets().unwrap().len(), 1);
    }

    #[test]
    fn a_parked_basket_survives_the_tablet_dying() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 1_788_600_000_000, "Karim")
                .unwrap();
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let waiting = till.held_tickets().unwrap();
        assert_eq!(waiting.len(), 1, "the customer is still standing there");

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(till.cart().lines().len(), 1);
    }

    #[test]
    fn a_resumed_basket_keeps_the_price_it_was_parked_at() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        let quoted = till.totals().unwrap().total;
        till.hold(Ulid::from_u128(500), 0, "waiting").unwrap();

        // A price rise arrives from the server while the basket is parked.
        let mut repriced = item(1, 99_000);
        repriced.barcodes = vec!["8690000000001".into()];
        till.apply_pull(&ItemDeltasV1 {
            cursor: 2,
            upserts: vec![ItemV1::from_domain(&repriced)],
            tombstones: vec![],
        })
        .unwrap();

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(
            till.totals().unwrap().total,
            quoted,
            "the customer pays what they were quoted before they walked off"
        );
    }

    #[test]
    fn refuses_to_resume_over_a_basket_in_progress() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.hold(Ulid::from_u128(500), 0, "first").unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Two baskets on one screen is how a customer pays for somebody else's
        // shopping.
        assert_eq!(
            till.resume(Ulid::from_u128(500)),
            Err(TillError::TicketInProgress)
        );
    }

    #[test]
    fn refuses_to_park_nothing_and_to_resume_what_is_not_there() {
        let mut till = stocked_till(MemoryBackend::new());
        assert_eq!(
            till.hold(Ulid::from_u128(500), 0, ""),
            Err(TillError::NothingToHold)
        );
        assert_eq!(
            till.resume(Ulid::from_u128(999)),
            Err(TillError::NoSuchHeldTicket)
        );
    }

    #[test]
    fn a_discarded_basket_does_not_come_back_after_a_restart() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 0, "abandoned").unwrap();
            till.discard_held(Ulid::from_u128(500)).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            till.held_tickets().unwrap().is_empty(),
            "the newest written set is the answer, so a cancellation sticks"
        );
    }

    #[test]
    fn a_refund_puts_stock_back_and_pays_the_customer() {
        let mut till = stocked_till(MemoryBackend::new());

        // Sell one first, so the shelf and the ledger have somewhere to return to.
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );

        // The customer brings it back with the receipt.
        till.start_refund(sale.receipt_no.as_deref(), 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        assert_eq!(till.totals().unwrap().total, Minor::new(-49_450));

        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        })
        .unwrap();
        let refund = till.checkout(Ulid::from_u128(901), 0).unwrap();

        assert_eq!(refund.ticket.totals.total, Minor::new(-49_450));
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(40_000)),
            "the goods are back on the shelf"
        );
        assert_eq!(till.status().unwrap().unsynced_sales, 2);
    }

    #[test]
    fn a_refund_carries_the_receipt_it_reverses_to_the_server() {
        let mut till = stocked_till(MemoryBackend::new());
        till.start_refund(Some("T1-000100"), 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        })
        .unwrap();
        till.checkout(Ulid::from_u128(902), 0).unwrap();

        let pending = till.pending_sales(10).unwrap();
        let stored = crate::storage::wire::decode_sale(
            crate::storage::wire::SALE_SCHEMA,
            &pending[0].payload,
        )
        .unwrap();

        assert_eq!(stored.refund_of.as_deref(), Some("T1-000100"));
        // Stock moves the other way, which is what the server will post.
        assert_eq!(stored.stock, alloc::vec![(1_u128, 1_000_i64)]);
    }

    #[test]
    fn two_lines_of_one_item_move_the_stock_once_for_the_full_amount() {
        let mut till = stocked_till(MemoryBackend::new());
        // The cart deliberately opens a second line when the first is
        // discounted, so one item on two lines is the normal case, not an edge.
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.set_line_discount(0, Discount::Rate(crate::money::Bp::new(1_000).unwrap()))
            .unwrap();
        till.scan("8690000000001", Milli::new(2_000)).unwrap();
        assert_eq!(till.cart().lines().len(), 2, "two lines, one item");

        pay_cash(&mut till, 200_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
        let stored = wire::sale_commit(&sale.ticket, None, None);

        // The server keys a movement on the sale and the item, so a second entry
        // for the same pair is discarded and the ledger undercounts for good.
        assert_eq!(
            stored.stock,
            alloc::vec![(1_u128, -3_000_i64)],
            "one entry per item, carrying every line's quantity"
        );
    }

    #[test]
    fn a_cashier_signs_in_with_the_internet_down_the_next_morning() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            let mut cashier = supervisor_operator();
            cashier.id = Ulid::from_u128(71);
            cashier.name = "Karim".into();
            cashier.pin =
                crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
            cashier.permissions = crate::auth::Permissions::cashier();
            till.put_operator(cashier).unwrap();
            backend = till.journal().backend().clone();
        }

        // Cold start, no network. The whole reason credentials sit on the
        // device.
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(till.sign_in(Ulid::from_u128(71), "4321", 0).is_err());
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();
        assert_eq!(till.signed_in().map(|who| &*who.name), Some("Karim"));
    }

    #[test]
    fn a_cashier_cannot_refund_without_a_supervisor_standing_there() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        assert!(
            till.start_refund(None, 0).is_err(),
            "money leaving the drawer is what the permission model is for"
        );

        till.authorise(Ulid::from_u128(70), "9999", Action::Refund, 0, 90_000)
            .unwrap();
        till.start_refund(None, 1_000).unwrap();

        // And the supervisor's name is on it afterwards.
        assert_eq!(
            till.audit().last().map(|entry| entry.authorised_by),
            Some(Some(Ulid::from_u128(70)))
        );
    }

    #[test]
    fn a_cashiers_ceiling_follows_the_person_not_the_screen() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        // Before this, the limits were whatever the caller passed to open().
        assert!(
            till.set_line_discount(0, Discount::Rate(crate::money::Bp::new(500).unwrap()))
                .is_err()
        );
        assert!(
            !till.signed_in().unwrap().permissions.may_override_price,
            "and the price override went with the ceiling"
        );
    }

    #[test]
    fn the_drawer_adds_up_and_survives_a_reboot() {
        let mut backend = MemoryBackend::new();
        let expected;
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(200_000), 0)
                .unwrap();

            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 49_450);
            till.checkout(Ulid::from_u128(900), 0).unwrap();

            till.cash_out(Minor::new(50_000), "drop to the safe", 1_000)
                .unwrap();

            expected = till.shift().unwrap().expected_cash().unwrap();
            // 2,000 float plus 494.50 taken less 500 dropped.
            assert_eq!(expected, Minor::new(199_450));
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.shift().and_then(|shift| shift.expected_cash().ok()),
            Some(expected),
            "the drawer figure is replayed from the same frames as the sales"
        );

        till.sign_in(Ulid::from_u128(70), "9999", 2_000).unwrap();
        let report = till.close_shift(Minor::new(199_000), 3_000).unwrap();
        assert_eq!(
            report.variance,
            Minor::new(-450),
            "a short drawer is a fact to report, not an error to refuse"
        );
    }

    #[test]
    fn what_a_supervisor_allows_is_on_the_paper_and_ends_with_the_sale() {
        let mut till = stocked_till(MemoryBackend::new());
        let cashier = Ulid::from_u128(11);
        till.set_operators(alloc::vec![
            Operator {
                id: cashier,
                name: "Rahima".into(),
                pin: crate::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: crate::auth::Permissions {
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
            supervisor_operator(),
        ])
        .unwrap();
        till.sign_in(cashier, "4321", 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Nothing, on this cashier's word.
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_err()
        );

        // The supervisor allows it, and it goes through.
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            crate::auth::Action::Discount { bp: 1_000 },
            1_000,
            60_000,
        )
        .unwrap();
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_ok()
        );

        // And it is on the paper. A waiver nobody can see afterwards is a
        // waiver nobody can ask about.
        pay_cash(&mut till, 100_000);
        let sold = till.checkout(Ulid::from_u128(900), 2_000).unwrap();
        assert_eq!(sold.ticket.overrides.len(), 1);
        assert!(sold.ticket.overrides[0].contains("Owner"));
        assert!(sold.ticket.overrides[0].contains("1000"));

        // The supervisor has walked away. The next customer gets the cashier's
        // own ceiling back, which is nothing.
        till.scan("8690000000001", Milli::ONE).unwrap();
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_err(),
            "an authorisation is for the sale in front of them, not the shift"
        );
    }

    #[test]
    fn a_basket_can_only_be_for_somebody_the_shop_wrote_down() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![
            wire::CustomerV1 {
                id: 21,
                name: "Karim, flat 3".into(),
                phone: Some("01711000000".into()),
                active: true,
            },
            wire::CustomerV1 {
                id: 22,
                name: "Rina".into(),
                phone: None,
                // Stopped: what she already owes is still owed, and nothing new
                // goes on the account.
                active: false,
            },
        ])
        .unwrap();

        assert!(till.set_customer(Some(Ulid::from_u128(21))).is_ok());
        assert_eq!(till.customer(), Some(Ulid::from_u128(21)));

        // A stopped account, and somebody this till has never heard of. Both
        // are a cashier about to write a debt nobody can chase.
        assert!(matches!(
            till.set_customer(Some(Ulid::from_u128(22))),
            Err(TillError::UnknownCustomer)
        ));
        assert!(matches!(
            till.set_customer(Some(Ulid::from_u128(99))),
            Err(TillError::UnknownCustomer)
        ));
        assert_eq!(till.customer(), Some(Ulid::from_u128(21)), "and unchanged");

        // Nobody is always allowed: a shop that has written nobody down still
        // sells on account against a name typed at the till.
        assert!(till.set_customer(None).is_ok());
        assert_eq!(till.customer(), None);
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
    fn a_credit_tender_naming_a_written_down_customer_is_refused() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: 21,
            name: "Karim, flat 3".into(),
            phone: None,
            active: true,
        }])
        .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // The cashier types the name instead of choosing the person. It reads
        // as harmless: it is how one person ends up with two accounts, one
        // holding what they took and one holding what they brought back.
        let refused = till.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(49_450),
            reference: Some("karim, FLAT 3".into()),
        });
        assert!(matches!(
            refused,
            Err(TillError::WriteItAgainstThem { ref name }) if name == "Karim, flat 3"
        ));

        // Choosing them is the answer, and then the same tender is fine: what
        // is owed goes against the record rather than against a spelling.
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();
        assert!(
            till.add_tender(Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(49_450),
                reference: Some("karim, FLAT 3".into()),
            })
            .is_ok()
        );
    }

    #[test]
    fn more_on_the_account_than_the_basket_is_refused_as_it_is_typed() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Six hundred on an account for a basket of 494.50. Refused here, with
        // the cashier still looking at what they typed, rather than at the
        // close with a customer waiting and the whole tender to enter again.
        let refused = till.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(60_000),
            reference: Some("the man from the tailor's".into()),
        });
        assert!(matches!(
            refused,
            Err(TillError::Cart(CartError::ChangeFromAPromise { .. }))
        ));

        // A hundred taka note and the rest on the account is the ordinary case
        // and is untouched: the change comes out of the note.
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(10_000),
            reference: None,
        })
        .unwrap();
        till.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(40_000),
            reference: Some("the man from the tailor's".into()),
        })
        .unwrap();
        assert_eq!(till.change_due().unwrap(), Minor::new(550));
    }

    #[test]
    fn a_name_nobody_wrote_down_is_still_taken() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: 21,
            name: "Karim, flat 3".into(),
            phone: None,
            active: true,
        }])
        .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // A shop takes a promise from somebody it has not written down, all
        // day. Refusing that would be refusing the ordinary case to prevent
        // the confusing one.
        assert!(
            till.add_tender(Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(49_450),
                reference: Some("the man from the tailor's".into()),
            })
            .is_ok()
        );
    }

    #[test]
    fn who_buys_on_account_survives_a_reboot() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.set_customers(alloc::vec![wire::CustomerV1 {
                id: 21,
                name: "Karim, flat 3".into(),
                phone: None,
                active: true,
            }])
            .unwrap();
            backend = till.journal().backend().clone();
        }

        // The point of holding them at all: a sale on account is written with
        // the internet down, and a name typed from memory is how one Karim ends
        // up paying for another Karim's rice.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(till.customers().len(), 1);
        assert_eq!(till.customers()[0].name, "Karim, flat 3");
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
    fn a_counted_drawer_names_whoever_counted_it() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(50_000), 0)
                .unwrap();
            till.sign_in(Ulid::from_u128(70), "9999", 1_000).unwrap();
            till.close_shift(Minor::new(45_000), 2_000).unwrap();
            backend = till.journal().backend().clone();
        }

        // A variance attached to a till and a time is half of what an owner
        // wants to know. The other half is standing at the counter.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let waiting = till.unsent_shifts();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].variance_minor, -5_000);
        assert_eq!(waiting[0].closed_by, Ulid::from_u128(70).to_u128());
        assert_eq!(
            waiting[0].closed_by_name, "Owner",
            "the name is written down at the time, not looked up later"
        );
    }

    #[test]
    fn what_a_device_allowed_survives_the_process_that_allowed_it() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(50_000), 1_000)
                .unwrap();
            // Cash out of the drawer outside a sale, which is the movement an
            // owner asks about when the count comes up short.
            till.cash_out(Minor::new(20_000), "paid the milk man", 2_000)
                .unwrap();
            backend = till.journal().backend().clone();
        }

        // Until now this lived in memory and died with the tab. The question
        // asked a week later is "who opened it", and the answer was gone.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let kept = till.unsent_allowed();
        assert!(!kept.is_empty(), "the trail outlives the process");
        let opened = kept
            .iter()
            .find(|one| one.action == 5)
            .expect("the drawer opening");
        assert_eq!(opened.operator, Ulid::from_u128(70).to_u128());
        assert_eq!(opened.operator_name, "Owner");
        assert_eq!(opened.at_ms, 2_000);
        assert_eq!(
            opened.authorised_by, 0,
            "nobody had to allow it: their own permission covered it"
        );
    }

    #[test]
    fn a_discount_a_supervisor_allowed_is_written_down_with_both_names() {
        let mut till = stocked_till(MemoryBackend::new());
        // A cashier who may not discount at all, which is the ordinary case.
        let cashier = Operator {
            id: Ulid::from_u128(71),
            name: "Rahima".into(),
            pin: crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS),
            permissions: crate::auth::Permissions::default(),
            active: true,
        };
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 1_000).unwrap();

        till.authorise(
            Ulid::from_u128(70),
            "9999",
            crate::auth::Action::Discount { bp: 1_000 },
            2_000,
            90_000,
        )
        .unwrap();

        // The auth book never sees a discount used: the cart's own ceiling
        // stops it, so the moment worth recording is the supervisor allowing
        // it. Without this the only record is prose on a ticket the customer
        // walked out with.
        let waived: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| one.action == 1)
            .collect();
        assert_eq!(waived.len(), 1);
        assert_eq!(waived[0].bp, 1_000);
        assert_eq!(waived[0].operator_name, "Rahima", "who did it");
        assert_eq!(waived[0].authorised_by_name, "Owner", "and who allowed it");
        // And both sign-ins are there, which is who was standing at the till.
        assert_eq!(
            till.unsent_allowed()
                .iter()
                .filter(|one| one.action == 9)
                .count(),
            2
        );
    }

    #[test]
    fn a_pin_typed_wrongly_is_written_down_and_so_is_the_lockout() {
        let mut till = stocked_till(MemoryBackend::new());

        // Somebody at the till after closing, trying the owner's PIN until it
        // locks. Five attempts is the shipped policy.
        for at_ms in [1_000_u64, 2_000, 3_000, 4_000, 5_000] {
            assert!(till.sign_in(Ulid::from_u128(70), "0000", at_ms).is_err());
        }

        // The sign-in that opened the till is in the list too, so the wrong
        // ones are counted apart from it.
        let wrong: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| matches!(one.action, 7 | 8))
            .collect();
        assert_eq!(wrong.len(), 5, "every attempt, not only the last");
        assert!(
            wrong.iter().take(4).all(|one| one.action == 7),
            "a wrong PIN"
        );
        assert_eq!(
            wrong[4].action, 8,
            "and the one that used the last attempt says so"
        );
        assert_eq!(wrong[0].operator_name, "Owner", "whose button was pressed");
        assert_eq!(wrong[0].authorised_by, 0);

        // And the sixth is refused for being locked out rather than for the
        // PIN, which is the state the count is there to reach.
        assert!(matches!(
            till.sign_in(Ulid::from_u128(70), "9999", 6_000),
            Err(TillError::Auth(crate::auth::AuthError::LockedOut { .. }))
        ));
    }

    #[test]
    fn who_took_the_till_is_written_down_with_the_rest() {
        let mut till = stocked_till(MemoryBackend::new());
        let cashier = Operator {
            id: Ulid::from_u128(71),
            name: "Rahima".into(),
            pin: crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS),
            permissions: crate::auth::Permissions::cashier(),
            active: true,
        };
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 2_000).unwrap();

        // Who was standing at a till when something happened at it is half of
        // every question an owner asks about that evening, and it used to be
        // inferable only from what they sold.
        let took: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| one.action == 9)
            .collect();
        assert_eq!(took.len(), 2, "the owner at open, then the cashier");
        assert_eq!(took[1].operator_name, "Rahima");
        assert_eq!(took[1].at_ms, 2_000);
        assert_eq!(
            took[1].authorised_by, 0,
            "nobody allows somebody to sign in: they type their own PIN"
        );
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

    #[test]
    fn a_cash_movement_the_shift_refuses_is_not_written_down() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::ZERO, 0)
            .unwrap();

        // Negative amounts are a caller mistake: direction is the operation.
        assert!(till.cash_in(Minor::new(-100), "typo", 0).is_err());

        let backend = till.journal().backend().clone();
        let (recovered, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            recovered.shift().map(|shift| shift.movements().len()),
            Some(0),
            "a refused movement in the log would be replayed as a real one"
        );
    }

    #[test]
    fn a_note_and_its_change_leave_the_drawer_holding_the_basket() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
            .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        // A five hundred note for a basket of 494.50.
        pay_cash(&mut till, 50_000);
        till.checkout(Ulid::from_u128(900), 1_000).unwrap();

        let standing = till.x_report().unwrap();
        assert_eq!(standing.cash_sales, Minor::new(49_450));
        assert_eq!(
            standing.expected_cash,
            Minor::new(30_000 + 49_450),
            "the float and the basket, not the float and the note"
        );

        // And the same after a reboot, because recovery replays the frames and
        // has to reach the figure the cashier watched all day.
        let backend = till.journal().backend().clone();
        let (recovered, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            recovered.x_report().unwrap().expected_cash,
            Minor::new(30_000 + 49_450)
        );
    }

    #[test]
    fn selling_is_never_refused_for_want_of_an_open_drawer() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);

        // A till that will not sell because nobody pressed the right button in
        // the morning is a till the shop works around.
        assert!(till.checkout(Ulid::from_u128(900), 0).is_ok());
        assert!(till.shift().is_none());
    }

    #[test]
    fn refuses_to_boot_on_another_terminals_log() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            till.checkout(Ulid::from_u128(900), 0).unwrap();
            backend = till.journal().backend().clone();
        }

        // The same image on a second tablet. Booting it would issue the first
        // terminal's numbers a second time, under the same epoch, on paper.
        let cloned = Till::open(
            backend,
            TENANT,
            Ulid::from_u128(999),
            1,
            CartLimits::unrestricted(),
        );
        assert!(matches!(
            cloned.err(),
            Some(TillError::Journal(JournalError::ForeignLog { .. }))
        ));
    }

    #[test]
    fn sales_still_waiting_for_a_number_are_still_counted_after_a_reboot() {
        let mut backend = MemoryBackend::new();
        {
            // No lease block, so the sale closes without a receipt number. It is
            // still a valid sale and the back office has to number it.
            let (mut till, _) = Till::open(
                backend.clone(),
                TENANT,
                terminal(),
                1,
                CartLimits::unrestricted(),
            )
            .unwrap();
            till.apply_pull(&ItemDeltasV1 {
                cursor: 1,
                upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
                tombstones: vec![],
            })
            .unwrap();
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            assert_eq!(sale.receipt_no, None);
            assert_eq!(till.status().unwrap().unnumbered_sales, 1);
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.status().unwrap().unnumbered_sales,
            1,
            "the count reset on every reboot, and nothing on the till said so"
        );
    }

    #[test]
    fn journal_sequences_do_not_restart_after_the_log_is_emptied() {
        let mut backend = MemoryBackend::new();
        let before;
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            before = sale.journal_sequence;
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let after = till.checkout(Ulid::from_u128(901), 0).unwrap();

        assert!(
            after.journal_sequence > before,
            "sequences must not rewind when the log is emptied: {} then {}",
            before,
            after.journal_sequence
        );
    }

    #[test]
    fn a_drained_till_still_holds_its_numbers_after_a_reboot() {
        // The commonest path in the product: the shop drains its outbox at close
        // of business, then opens next morning with the internet down.
        let mut backend = MemoryBackend::new();
        {
            // The helper already holds a block of five hundred numbers.
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.leases().remaining(),
            499,
            "draining the outbox must not spend the numbers the server already issued"
        );
    }

    #[test]
    fn a_reboot_after_crossing_a_block_boundary_does_not_reissue_numbers() {
        let mut backend = MemoryBackend::new();
        let issued;
        {
            // A bare till, so the only blocks in hand are the two granted here:
            // a short active block and the reserve that lets the till keep
            // selling across the boundary while offline.
            let (mut till, _) = Till::open(
                backend.clone(),
                TENANT,
                terminal(),
                1,
                CartLimits::unrestricted(),
            )
            .unwrap();
            till.apply_pull(&ItemDeltasV1 {
                cursor: 1,
                upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
                tombstones: vec![],
            })
            .unwrap();
            till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 101))
                .unwrap();
            till.grant_lease(&Lease::new(terminal(), 1, "T1", 600, 699))
                .unwrap();

            let mut numbers = alloc::vec::Vec::new();
            for index in 0..4_u128 {
                till.scan("8690000000001", Milli::ONE).unwrap();
                pay_cash(&mut till, 50_000);
                let sale = till.checkout(Ulid::from_u128(900 + index), 0).unwrap();
                numbers.push(sale.ticket.receipt_no.clone());
            }
            issued = numbers;
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let after = till.checkout(Ulid::from_u128(999), 0).unwrap();

        assert!(
            !issued.contains(&after.ticket.receipt_no),
            "a number already on a customer\'s receipt was printed again: {:?} after {:?}",
            after.ticket.receipt_no,
            issued
        );
    }

    #[test]
    fn acknowledged_sales_leave_the_outbox() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();

        assert_eq!(till.acknowledge(&[sale.ticket.id]).unwrap(), 1);
        assert_eq!(till.status().unwrap().unsynced_sales, 0);
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
