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

use alloc::vec::Vec;

use crate::cart::{
    Cart, CartError, CartLimits, CartLine, TerminalId, Tender, Ticket, TicketId,
};
use crate::domain::{ticket_totals, Discount, TicketInput, TicketTotals};
use crate::ids::Ulid;
use crate::lease::{Lease, LeaseBook, DEFAULT_RENEWAL_THRESHOLD};
use crate::money::{Milli, Minor};
use crate::replica::{Item, Replica};
use crate::storage::backend::Backend;
use crate::storage::frame::{PayloadKind, Store};
use crate::storage::journal::{Journal, JournalError};
use crate::storage::wire::{
    self, DiscountV1, HeldTicketV1, HeldTicketsV1, ItemDeltasV1, LeaseGrantV1, LineV1,
    SaleCommitV1, TerminalStateV1, WireError, SALE_SCHEMA, TERMINAL_SCHEMA,
};
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
    /// Nothing to park.
    NothingToHold,
    /// No parked ticket with that id.
    NoSuchHeldTicket,
    /// Resuming would discard the basket already on screen.
    TicketInProgress,
    Cart(CartError),
    Journal(JournalError),
    Sync(SyncError),
    Wire(WireError),
}

impl From<CartError> for TillError {
    fn from(error: CartError) -> Self {
        Self::Cart(error)
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

pub type Result<T> = core::result::Result<T, TillError>;

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
        let (leases, held) = Self::recover_terminal_state(&journal)?;

        let report = BootReport {
            items: replica.len(),
            unsynced_sales: sync_status.unsynced,
            receipt_numbers_left: leases.remaining(),
            cursor: sync_status.cursor,
            repaired: !recovery.is_clean(),
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
    fn recover_terminal_state(journal: &Journal<B>) -> Result<(LeaseBook, HeldTicketsV1)> {
        let mut book = LeaseBook::new();
        let mut held = HeldTicketsV1::default();

        if let Some(bytes) = journal.load_terminal_state()? {
            let state = wire::decode_terminal_state(TERMINAL_SCHEMA, &bytes)?;
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
        }

        let mut highest_used: Option<u64> = None;
        for record in journal.read(Store::Critical)? {
            if record.header.kind == PayloadKind::SaleCommit {
                let sale: SaleCommitV1 = wire::decode_sale(record.header.schema, &record.payload)?;
                if let Some(next) = sale.lease_next {
                    highest_used = Some(highest_used.map_or(next, |current| current.max(next)));
                }
            }
        }
        if let Some(next) = highest_used {
            book.resume_at(next);
        }

        Ok((book, held))
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
            leases,
            held: self.held.clone(),
            unnumbered: self.leases.unnumbered(),
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

    /// Scan a barcode straight onto the ticket.
    pub fn scan(&mut self, barcode: &str, qty: Milli) -> Result<usize> {
        let item = self
            .replica
            .by_barcode(barcode)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        Ok(self.cart.add_item(&item, qty)?)
    }

    pub fn set_qty(&mut self, line: usize, qty: Milli) -> Result<()> {
        Ok(self.cart.set_qty(line, qty)?)
    }

    pub fn remove_line(&mut self, line: usize) -> Result<()> {
        self.cart.remove_line(line)?;
        Ok(())
    }

    pub fn set_line_discount(&mut self, line: usize, discount: Discount) -> Result<()> {
        Ok(self.cart.set_line_discount(line, discount)?)
    }

    pub fn set_ticket_discount(&mut self, discount: Discount) -> Result<()> {
        Ok(self.cart.set_ticket_discount(discount)?)
    }

    pub fn authorise_override(&mut self, reason: &str) {
        self.cart.authorise_override(reason);
    }

    pub fn add_tender(&mut self, tender: Tender) {
        self.cart.add_tender(tender);
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
        self.cart = Cart::new(self.limits);
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
    pub fn start_refund(&mut self, original_receipt: Option<&str>) -> Result<()> {
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

    /// Record what the server confirmed.
    ///
    /// When that leaves nothing outstanding the log is emptied, but only after
    /// the terminal's blocks and parked baskets are durable in their own slot.
    /// In that order: a crash in between replays the log and reaches the same
    /// state, whereas emptying first would take the numbers with it.
    pub fn acknowledge(&mut self, acknowledged: &[Ulid]) -> Result<usize> {
        let outcome = Outbox::acknowledge(&mut self.journal, acknowledged)?;
        if outcome.drained {
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
        Ok(Some(self.sync.checkpoint(&mut self.journal, &self.replica)?))
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
            barcodes: vec![alloc::format!("869000000{seed:04}").into_boxed_str()],
            on_hand: Milli::new(40_000),
            active: true,
        }
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
        till
    }

    fn pay_cash<B: Backend>(till: &mut Till<B>, amount: i64) {
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount),
            reference: None,
        });
    }

    #[test]
    fn boots_empty_and_reports_it() {
        let (_till, report) =
            Till::open(MemoryBackend::new(), TENANT, terminal(), 1, CartLimits::unrestricted())
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

        let sale = till.checkout(Ulid::from_u128(900), 1_788_600_000_000).unwrap();
        assert_eq!(sale.receipt_no.as_deref(), Some("T1-000100"));
        assert_eq!(sale.ticket.change, Minor::new(550));

        // Stock moved, cart cleared, sale queued for the server.
        assert_eq!(
            till.catalogue().by_id(Ulid::from_u128(1)).map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );
        assert!(till.cart().is_empty());
        assert_eq!(till.status().unwrap().unsynced_sales, 1);
    }

    #[test]
    fn an_unknown_barcode_is_reported_not_guessed() {
        let mut till = stocked_till(MemoryBackend::new());
        assert_eq!(till.scan("0000000000000", Milli::ONE), Err(TillError::UnknownBarcode));
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
        assert!(result.is_err(), "a sale that cannot be made durable must not succeed");

        // The cashier can retry: nothing was consumed and nothing was lost.
        assert_eq!(till.cart().lines().len(), 1, "the basket survives a failed commit");
        assert_eq!(till.leases().remaining(), numbers_before, "no number was burned");
        assert_eq!(
            till.catalogue().by_id(Ulid::from_u128(1)).map(|i| i.on_hand),
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
        assert_eq!(report.unsynced_sales, 1, "the sale is still owed to the server");
        assert_eq!(report.receipt_numbers_left, 499, "the used number is not reissued");

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

        till.hold(Ulid::from_u128(500), 1_788_600_000_000, "Rahim, gone for cash")
            .unwrap();
        assert!(till.cart().is_empty(), "the counter is free for the next customer");

        let waiting = till.held_tickets().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].lines, 1);
        assert_eq!(waiting[0].total, parked_total);
        assert_eq!(&waiting[0].label, "Rahim, gone for cash");

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(till.totals().unwrap().total, parked_total);
        assert!(till.held_tickets().unwrap().is_empty(), "and it is no longer parked");
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
        assert_eq!(till.hold(Ulid::from_u128(500), 0, ""), Err(TillError::NothingToHold));
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
            till.catalogue().by_id(Ulid::from_u128(1)).map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );

        // The customer brings it back with the receipt.
        till.start_refund(sale.receipt_no.as_deref()).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        assert_eq!(till.totals().unwrap().total, Minor::new(-49_450));

        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        });
        let refund = till.checkout(Ulid::from_u128(901), 0).unwrap();

        assert_eq!(refund.ticket.totals.total, Minor::new(-49_450));
        assert_eq!(
            till.catalogue().by_id(Ulid::from_u128(1)).map(|i| i.on_hand),
            Some(Milli::new(40_000)),
            "the goods are back on the shelf"
        );
        assert_eq!(till.status().unwrap().unsynced_sales, 2);
    }

    #[test]
    fn a_refund_carries_the_receipt_it_reverses_to_the_server() {
        let mut till = stocked_till(MemoryBackend::new());
        till.start_refund(Some("T1-000100")).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        });
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
            let (mut till, _) =
                Till::open(backend.clone(), TENANT, terminal(), 1, CartLimits::unrestricted())
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
        assert!(!status.wants_lease_renewal, "five hundred numbers is plenty");

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
