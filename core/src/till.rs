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

use crate::cart::{Cart, CartError, CartLimits, TerminalId, Tender, Ticket, TicketId};
use crate::domain::{Discount, TicketTotals};
use crate::ids::Ulid;
use crate::lease::{Lease, LeaseBook, DEFAULT_RENEWAL_THRESHOLD};
use crate::money::{Milli, Minor};
use crate::replica::{Item, Replica};
use crate::storage::backend::Backend;
use crate::storage::frame::{PayloadKind, Store};
use crate::storage::journal::{Journal, JournalError};
use crate::storage::wire::{
    self, ItemDeltasV1, LeaseGrantV1, SaleCommitV1, WireError, LEASE_SCHEMA, SALE_SCHEMA,
};
use crate::sync::{Outbox, PendingSale, SyncEngine, SyncError, SyncStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TillError {
    /// The scanned code matches nothing in the catalogue.
    UnknownBarcode,
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
        let leases = Self::recover_leases(&journal)?;

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
            },
            report,
        ))
    }

    /// Rebuild the receipt-number book from the ledger.
    ///
    /// Blocks come from grant frames; the position within the active block comes
    /// from the sales themselves, because a sale is the only proof a number was
    /// actually used. Trusting the grant alone would reissue every number the
    /// terminal handed out since the block arrived.
    fn recover_leases(journal: &Journal<B>) -> Result<LeaseBook> {
        let mut book = LeaseBook::new();
        let mut highest_used: Option<u64> = None;

        for record in journal.read(Store::Critical)? {
            match record.header.kind {
                PayloadKind::LeaseGrant => {
                    let grant = wire::decode_lease(record.header.schema, &record.payload)?;
                    book.grant(Lease::new(
                        Ulid::from_u128(grant.terminal),
                        grant.epoch,
                        &grant.prefix,
                        grant.first,
                        grant.last,
                    ));
                }
                PayloadKind::SaleCommit => {
                    let sale: SaleCommitV1 =
                        wire::decode_sale(record.header.schema, &record.payload)?;
                    if let Some(next) = sale.lease_next {
                        highest_used = Some(highest_used.map_or(next, |current| current.max(next)));
                    }
                }
                _ => {}
            }
        }

        if let Some(next) = highest_used {
            book.resume_at(next);
        }
        Ok(book)
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
        let payload = wire::encode_lease(&LeaseGrantV1 {
            terminal: lease.terminal.to_u128(),
            epoch: lease.epoch,
            prefix: alloc::string::String::from(&*lease.prefix),
            first: lease.next,
            last: lease.last,
        })?;
        self.journal.commit(
            Store::Critical,
            PayloadKind::LeaseGrant,
            LEASE_SCHEMA,
            &payload,
        )?;
        self.leases.grant(lease.clone());
        Ok(())
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
    pub fn acknowledge(&mut self, acknowledged: &[Ulid]) -> Result<usize> {
        Ok(Outbox::acknowledge(&mut self.journal, acknowledged)?)
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
