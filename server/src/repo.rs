//! What the server needs to remember, and an in-memory implementation.
//!
//! The trait exists so ingest can be tested without a database. Postgres is the
//! real implementation; the in-memory one keeps the test suite fast enough to
//! run on every save, which is what makes anybody actually run it.
//!
//! Every method takes a tenant. There is no way to ask this trait a question
//! that is not scoped to one shop, which is the first line of defence against
//! cross-tenant leakage; row-level security in Postgres is the second.

use std::collections::{HashMap, HashSet};

use openpos_core::protocol::QuarantineReason;

/// A sale as the server keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSale {
    pub tenant: u128,
    pub terminal: u128,
    pub id: u128,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    /// Device clock at the moment of sale. Kept for the receipt and for
    /// ordering within one terminal, never trusted across terminals.
    pub rung_at_ms: u64,
    pub total_minor: i64,
    /// The bytes exactly as the till committed them. Kept verbatim so a dispute
    /// can be settled against what the terminal actually wrote, rather than
    /// against a re-encoding of it.
    pub payload: Vec<u8>,
    /// Set when the sale needs a human. It is still stored either way.
    pub quarantine: Option<QuarantineReason>,
    /// Item id and signed milli-units.
    pub stock: Vec<(u128, i64)>,
}

/// A receipt number block handed to a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseRecord {
    pub tenant: u128,
    pub terminal: u128,
    pub epoch: u64,
    pub first: u64,
    pub last: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoError {
    /// The terminal is not enrolled for this tenant.
    UnknownTerminal,
    /// The backing store refused. Ingest treats this as fatal for the batch: a
    /// till that is told a sale is stored, when it is not, will drop its only
    /// copy.
    Backend,
}

pub type Result<T> = std::result::Result<T, RepoError>;

pub trait Repository {
    /// Whether this sale is already stored. Ingest is idempotent, so a replay
    /// after a dropped connection must not create a second sale.
    fn has_sale(&self, tenant: u128, id: u128) -> Result<bool>;

    /// Whether a receipt number is already used, under a given epoch. Two sales
    /// sharing one number means a terminal was restored or cloned.
    fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool>;

    fn store_sale(&mut self, sale: StoredSale) -> Result<()>;

    /// Whether this terminal belongs to this tenant.
    fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool>;

    /// Allocate the next block of receipt numbers for a terminal.
    fn issue_lease(&mut self, tenant: u128, terminal: u128, count: u32) -> Result<LeaseRecord>;
}

/// In-memory store for tests.
#[derive(Debug, Default)]
pub struct MemoryRepo {
    sales: HashMap<(u128, u128), StoredSale>,
    receipts: HashSet<(u128, String, u64)>,
    terminals: HashSet<(u128, u128)>,
    /// Next unissued number per terminal, and its epoch.
    counters: HashMap<(u128, u128), (u64, u64)>,
}

impl MemoryRepo {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enrol a terminal, as the back office would.
    pub fn enrol(&mut self, tenant: u128, terminal: u128) {
        self.terminals.insert((tenant, terminal));
        self.counters.entry((tenant, terminal)).or_insert((1, 1));
    }

    /// Bump a terminal's epoch, as the back office does when it believes a
    /// device was replaced or restored from a backup.
    pub fn bump_epoch(&mut self, tenant: u128, terminal: u128) {
        if let Some((_, epoch)) = self.counters.get_mut(&(tenant, terminal)) {
            *epoch = epoch.saturating_add(1);
        }
    }

    #[must_use]
    pub fn sale(&self, tenant: u128, id: u128) -> Option<&StoredSale> {
        self.sales.get(&(tenant, id))
    }

    #[must_use]
    pub fn sale_count(&self, tenant: u128) -> usize {
        self.sales.keys().filter(|(owner, _)| *owner == tenant).count()
    }

    /// Every quarantined sale, which is what the repair queue lists.
    #[must_use]
    pub fn quarantined(&self, tenant: u128) -> Vec<&StoredSale> {
        let mut found: Vec<&StoredSale> = self
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant && sale.quarantine.is_some())
            .collect();
        found.sort_by_key(|sale| sale.id);
        found
    }
}

impl Repository for MemoryRepo {
    fn has_sale(&self, tenant: u128, id: u128) -> Result<bool> {
        Ok(self.sales.contains_key(&(tenant, id)))
    }

    fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool> {
        Ok(self
            .receipts
            .contains(&(tenant, receipt_no.to_owned(), epoch)))
    }

    fn store_sale(&mut self, sale: StoredSale) -> Result<()> {
        if let (Some(receipt), Some(epoch)) = (sale.receipt_no.clone(), sale.receipt_epoch) {
            self.receipts.insert((sale.tenant, receipt, epoch));
        }
        self.sales.insert((sale.tenant, sale.id), sale);
        Ok(())
    }

    fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool> {
        Ok(self.terminals.contains(&(tenant, terminal)))
    }

    fn issue_lease(&mut self, tenant: u128, terminal: u128, count: u32) -> Result<LeaseRecord> {
        if !self.terminal_enrolled(tenant, terminal)? {
            return Err(RepoError::UnknownTerminal);
        }
        let entry = self
            .counters
            .get_mut(&(tenant, terminal))
            .ok_or(RepoError::UnknownTerminal)?;
        let (next, epoch) = *entry;
        let span = u64::from(count.max(1));
        let last = next.saturating_add(span).saturating_sub(1);
        entry.0 = last.saturating_add(1);

        Ok(LeaseRecord {
            tenant,
            terminal,
            epoch,
            first: next,
            last,
        })
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

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    #[test]
    fn issues_blocks_that_never_overlap() {
        let mut repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);

        let first = repo.issue_lease(TENANT, TERMINAL, 500).unwrap();
        let second = repo.issue_lease(TENANT, TERMINAL, 500).unwrap();

        assert_eq!((first.first, first.last), (1, 500));
        assert_eq!((second.first, second.last), (501, 1_000));
        assert!(second.first > first.last, "blocks must not overlap");
    }

    #[test]
    fn refuses_to_lease_to_a_terminal_it_does_not_know() {
        let mut repo = MemoryRepo::new();
        assert_eq!(
            repo.issue_lease(TENANT, TERMINAL, 10),
            Err(RepoError::UnknownTerminal)
        );
    }

    #[test]
    fn a_bumped_epoch_marks_later_blocks() {
        let mut repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let before = repo.issue_lease(TENANT, TERMINAL, 10).unwrap();

        // The back office decides this terminal was restored from a backup.
        repo.bump_epoch(TENANT, TERMINAL);
        let after = repo.issue_lease(TENANT, TERMINAL, 10).unwrap();

        assert_eq!(before.epoch, 1);
        assert_eq!(after.epoch, 2, "numbers stay attributable across a restore");
    }

    #[test]
    fn tenants_cannot_see_each_other() {
        let mut repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 900,
            receipt_no: Some("T1-000100".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 0,
            total_minor: 49_450,
            payload: vec![],
            quarantine: None,
            stock: vec![],
        })
        .unwrap();

        assert!(repo.has_sale(TENANT, 900).unwrap());
        assert!(!repo.has_sale(999, 900).unwrap(), "another shop must not see it");
        assert!(repo.receipt_taken(TENANT, "T1-000100", 1).unwrap());
        assert!(!repo.receipt_taken(999, "T1-000100", 1).unwrap());
        // A different epoch is a different number space.
        assert!(!repo.receipt_taken(TENANT, "T1-000100", 2).unwrap());
    }
}
