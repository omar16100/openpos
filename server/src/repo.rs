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
use std::future::Future;
use std::sync::Mutex;

use openpos_core::protocol::{ItemWire, QuarantineReason};

use crate::auth::{Caller, Token, TokenHash};

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

/// What the server needs to remember.
///
/// Asynchronous, because the real implementation talks to Postgres, and taking
/// `&self` rather than `&mut self`, because a connection pool manages its own
/// concurrency. Requiring `&mut self` would force a lock around the whole
/// server and serialise every shop behind every other one.
///
/// Futures are explicitly `Send` so the handlers can be spawned on a
/// multi-threaded runtime.
pub trait Repository: Send + Sync {
    /// Whether this sale is already stored. Ingest is idempotent, so a replay
    /// after a dropped connection must not create a second sale.
    fn has_sale(&self, tenant: u128, id: u128) -> impl Future<Output = Result<bool>> + Send;

    /// Whether a receipt number is already used, under a given epoch. Two sales
    /// sharing one number means a terminal was restored or cloned.
    fn receipt_taken(
        &self,
        tenant: u128,
        receipt_no: &str,
        epoch: u64,
    ) -> impl Future<Output = Result<bool>> + Send;

    fn store_sale(&self, sale: StoredSale) -> impl Future<Output = Result<()>> + Send;

    /// Whether this terminal belongs to this tenant.
    fn terminal_enrolled(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Allocate the next block of receipt numbers for a terminal.
    fn issue_lease(
        &self,
        tenant: u128,
        terminal: u128,
        count: u32,
    ) -> impl Future<Output = Result<LeaseRecord>> + Send;

    /// Resolve a presented token to the terminal that owns it.
    ///
    /// Returns `None` for an unknown or revoked token. Deliberately not an
    /// error: an attacker probing tokens learns nothing from the difference
    /// between "no such token" and "that one was revoked".
    fn authenticate(&self, token: &TokenHash) -> impl Future<Output = Result<Option<Caller>>> + Send;

    /// Attach a freshly issued token to a terminal.
    fn store_token(
        &self,
        caller: Caller,
        token: &TokenHash,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Catalogue changes after `cursor`, oldest first.
    ///
    /// Returns the upserts, the ids of deleted items, the cursor after this
    /// batch, and whether more is waiting. Tombstones travel explicitly: without
    /// them a deleted item lingers on every till that already has it.
    fn items_since(
        &self,
        tenant: u128,
        cursor: u64,
        limit: u32,
    ) -> impl Future<Output = Result<CataloguePage>> + Send;
}

/// One page of catalogue changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CataloguePage {
    pub upserts: Vec<ItemWire>,
    pub tombstones: Vec<u128>,
    pub cursor: u64,
    pub more: bool,
}

/// In-memory store for tests.
///
/// Interior mutability, so it satisfies the same `&self` interface Postgres
/// does. The lock lives inside one shop's store rather than around the whole
/// server.
#[derive(Debug, Default)]
pub struct MemoryRepo {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    sales: HashMap<(u128, u128), StoredSale>,
    receipts: HashSet<(u128, String, u64)>,
    terminals: HashSet<(u128, u128)>,
    /// Next unissued number per terminal, and its epoch.
    counters: HashMap<(u128, u128), (u64, u64)>,
    /// Catalogue changes in the order they happened, which is what a till
    /// replays. A real store keeps this as a sequence column rather than a
    /// vector, but the shape of the answer is the same.
    changes: HashMap<u128, Vec<CatalogueChange>>,
    tokens: HashMap<TokenHash, Caller>,
}

/// One catalogue change, as the server records it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogueChange {
    Upsert(Box<ItemWire>),
    Delete(u128),
}

impl MemoryRepo {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A poisoned lock means a test panicked while holding it. Recover the data
    /// rather than cascading the panic: the store itself is still coherent.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Enrol a terminal, as the back office would.
    pub fn enrol(&self, tenant: u128, terminal: u128) {
        let mut inner = self.lock();
        inner.terminals.insert((tenant, terminal));
        inner.counters.entry((tenant, terminal)).or_insert((1, 1));
    }

    /// Enrol a terminal and hand back its credential, as the back office does.
    pub fn enrol_with_token(&self, tenant: u128, terminal: u128) -> Token {
        self.enrol(tenant, terminal);
        let token = Token::generate();
        self.lock()
            .tokens
            .insert(token.hash(), Caller { tenant, terminal });
        token
    }

    /// Bump a terminal's epoch, as the back office does when it believes a
    /// device was replaced or restored from a backup.
    pub fn bump_epoch(&self, tenant: u128, terminal: u128) {
        if let Some((_, epoch)) = self.lock().counters.get_mut(&(tenant, terminal)) {
            *epoch = epoch.saturating_add(1);
        }
    }

    /// Record a catalogue change, as the back office would.
    pub fn upsert_item(&self, tenant: u128, item: ItemWire) -> u64 {
        let mut inner = self.lock();
        let log = inner.changes.entry(tenant).or_default();
        log.push(CatalogueChange::Upsert(Box::new(item)));
        log.len() as u64
    }

    /// Record a deletion.
    pub fn delete_item(&self, tenant: u128, id: u128) -> u64 {
        let mut inner = self.lock();
        let log = inner.changes.entry(tenant).or_default();
        log.push(CatalogueChange::Delete(id));
        log.len() as u64
    }

    #[must_use]
    pub fn sale(&self, tenant: u128, id: u128) -> Option<StoredSale> {
        self.lock().sales.get(&(tenant, id)).cloned()
    }

    #[must_use]
    pub fn sale_count(&self, tenant: u128) -> usize {
        self.lock()
            .sales
            .keys()
            .filter(|(owner, _)| *owner == tenant)
            .count()
    }

    /// Every quarantined sale, which is what the repair queue lists.
    #[must_use]
    pub fn quarantined(&self, tenant: u128) -> Vec<StoredSale> {
        let mut found: Vec<StoredSale> = self
            .lock()
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant && sale.quarantine.is_some())
            .cloned()
            .collect();
        found.sort_by_key(|sale| sale.id);
        found
    }
}

impl Repository for MemoryRepo {
    async fn has_sale(&self, tenant: u128, id: u128) -> Result<bool> {
        Ok(self.lock().sales.contains_key(&(tenant, id)))
    }

    async fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool> {
        Ok(self
            .lock()
            .receipts
            .contains(&(tenant, receipt_no.to_owned(), epoch)))
    }

    async fn store_sale(&self, sale: StoredSale) -> Result<()> {
        let mut inner = self.lock();
        if let (Some(receipt), Some(epoch)) = (sale.receipt_no.clone(), sale.receipt_epoch) {
            inner.receipts.insert((sale.tenant, receipt, epoch));
        }
        inner.sales.insert((sale.tenant, sale.id), sale);
        Ok(())
    }

    async fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool> {
        Ok(self.lock().terminals.contains(&(tenant, terminal)))
    }

    async fn authenticate(&self, token: &TokenHash) -> Result<Option<Caller>> {
        Ok(self.lock().tokens.get(token).copied())
    }

    async fn store_token(&self, caller: Caller, token: &TokenHash) -> Result<()> {
        self.lock().tokens.insert(token.clone(), caller);
        Ok(())
    }

    async fn items_since(&self, tenant: u128, cursor: u64, limit: u32) -> Result<CataloguePage> {
        let inner = self.lock();
        let empty = Vec::new();
        let log = inner.changes.get(&tenant).unwrap_or(&empty);
        let start = usize::try_from(cursor).unwrap_or(usize::MAX).min(log.len());
        let take = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);

        let mut page = CataloguePage {
            cursor,
            ..CataloguePage::default()
        };
        for (offset, change) in log.iter().skip(start).take(take).enumerate() {
            match change {
                CatalogueChange::Upsert(item) => page.upserts.push((**item).clone()),
                CatalogueChange::Delete(id) => page.tombstones.push(*id),
            }
            page.cursor = cursor.saturating_add(offset as u64).saturating_add(1);
        }
        page.more = usize::try_from(page.cursor).unwrap_or(usize::MAX) < log.len();
        Ok(page)
    }

    async fn issue_lease(&self, tenant: u128, terminal: u128, count: u32) -> Result<LeaseRecord> {
        let mut inner = self.lock();
        if !inner.terminals.contains(&(tenant, terminal)) {
            return Err(RepoError::UnknownTerminal);
        }
        let entry = inner
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

    #[tokio::test]
    async fn issues_blocks_that_never_overlap() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);

        let first = repo.issue_lease(TENANT, TERMINAL, 500).await.unwrap();
        let second = repo.issue_lease(TENANT, TERMINAL, 500).await.unwrap();

        assert_eq!((first.first, first.last), (1, 500));
        assert_eq!((second.first, second.last), (501, 1_000));
        assert!(second.first > first.last, "blocks must not overlap");
    }

    #[tokio::test]
    async fn refuses_to_lease_to_a_terminal_it_does_not_know() {
        let repo = MemoryRepo::new();
        assert_eq!(
            repo.issue_lease(TENANT, TERMINAL, 10).await,
            Err(RepoError::UnknownTerminal)
        );
    }

    #[tokio::test]
    async fn a_bumped_epoch_marks_later_blocks() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let before = repo.issue_lease(TENANT, TERMINAL, 10).await.unwrap();

        // The back office decides this terminal was restored from a backup.
        repo.bump_epoch(TENANT, TERMINAL);
        let after = repo.issue_lease(TENANT, TERMINAL, 10).await.unwrap();

        assert_eq!(before.epoch, 1);
        assert_eq!(after.epoch, 2, "numbers stay attributable across a restore");
    }

    #[tokio::test]
    async fn tenants_cannot_see_each_other() {
        let repo = MemoryRepo::new();
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
        .await
        .unwrap();

        assert!(repo.has_sale(TENANT, 900).await.unwrap());
        assert!(!repo.has_sale(999, 900).await.unwrap(), "another shop must not see it");
        assert!(repo.receipt_taken(TENANT, "T1-000100", 1).await.unwrap());
        assert!(!repo.receipt_taken(999, "T1-000100", 1).await.unwrap());
        // A different epoch is a different number space.
        assert!(!repo.receipt_taken(TENANT, "T1-000100", 2).await.unwrap());
    }
}
