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
use std::time::{Duration, SystemTime};

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

    /// Withdraw one credential. Returns whether anything was withdrawn.
    fn revoke_token(&self, token: &TokenHash) -> impl Future<Output = Result<bool>> + Send;

    /// Withdraw every credential a terminal holds, which is what a shop needs
    /// the moment a tablet is lost or stolen. Returns how many were withdrawn.
    fn revoke_all_tokens(&self, caller: Caller) -> impl Future<Output = Result<usize>> + Send;

    /// Offer a short code that can be exchanged for a credential.
    fn issue_enrolment_code(
        &self,
        caller: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Exchange a code for the terminal it names, consuming it.
    ///
    /// Returns `None` for a code that is unknown, expired or already used. The
    /// three are indistinguishable to the caller on purpose: an attacker
    /// guessing codes learns nothing from being told which of those it hit.
    fn redeem_enrolment_code(
        &self,
        code: &TokenHash,
    ) -> impl Future<Output = Result<Option<Caller>>> + Send;

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

    /// Sales still waiting on a human, oldest first.
    ///
    /// Oldest first because the queue is worked from the top and the oldest
    /// entry is the one whose evidence is decaying: the customer who disputes a
    /// receipt is remembered for a week, not a quarter.
    fn repair_queue(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RepairItem>>> + Send;

    /// Take one sale out of the queue, recording what was decided.
    ///
    /// Returns whether anything moved. Resolving twice is not an error, because
    /// two people working the same queue is the normal case and the second one
    /// should be told "already done" rather than shown a failure.
    ///
    /// The sale itself is never altered or removed. It happened, and the stored
    /// bytes are what a dispute is settled against.
    fn resolve_quarantine(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Every terminal in the shop, with what support needs to triage it.
    fn terminal_health(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Vec<TerminalHealth>>> + Send;

    /// Record that this terminal was heard from just now.
    ///
    /// Separate from the work of a sync rather than folded into it, so a till
    /// that syncs an empty batch still counts as alive. A device that stopped
    /// selling and a device that stopped talking need different visits, and one
    /// timestamp per successful sync is what tells them apart.
    fn mark_terminal_seen(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Record a catalogue upsert, returning the sequence it landed at.
    fn upsert_item(&self, tenant: u128, item: &ItemWire)
        -> impl Future<Output = Result<u64>> + Send;

    /// Record a catalogue deletion, returning the sequence it landed at.
    fn delete_item(&self, tenant: u128, item_id: u128) -> impl Future<Output = Result<u64>> + Send;
}

/// One sale in the repair queue.
///
/// Carries the payload's summary rather than the payload. The queue is a list a
/// person scans; whoever needs the bytes fetches the sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairItem {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    pub received_at_ms: u64,
    pub reason: String,
}

/// One terminal, as support sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalHealth {
    pub terminal: u128,
    pub label: String,
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    /// `None` for a terminal not heard from since the column existed. Absent
    /// rather than zero, because zero would render as 1970 and read as a fault.
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    pub open_repairs: u64,
}

/// Turn a quarantine reason into the sentence a shopkeeper reads.
///
/// Stored and returned as text rather than as a structured code. It is read by a
/// human deciding what to do about a sale, never queried on, and text cannot
/// drift out of step with the enum the way a numeric code would after a release
/// that adds a variant. Both repositories call this, so the queue reads the same
/// whether it is served from Postgres or from memory.
#[must_use]
pub fn describe_quarantine(reason: &QuarantineReason) -> String {
    match reason {
        QuarantineReason::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => format!(
            "totals mismatch: the till stored {stored_minor} and the server recomputed {recomputed_minor}"
        ),
        QuarantineReason::DuplicateReceiptNumber { receipt_no } => {
            format!("receipt number {receipt_no} was already used by another sale")
        }
        QuarantineReason::Undecodable => {
            "the payload could not be decoded under the schema it claimed".to_owned()
        }
    }
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
    /// When each sale arrived, keyed as the sales are. Kept beside them rather
    /// than inside `StoredSale`, because that struct is what ingest builds from
    /// a till's own bytes and arrival is the server's fact, not the till's.
    received: HashMap<(u128, u128), u64>,
    /// Notes left on resolved quarantines, keyed by tenant and sale. Presence is
    /// what takes an entry out of the queue; the sale itself is never touched.
    resolutions: HashMap<(u128, u128), String>,
    receipts: HashSet<(u128, String, u64)>,
    terminals: HashMap<(u128, u128), TerminalRecord>,
    /// Next unissued number per terminal, and its epoch.
    counters: HashMap<(u128, u128), (u64, u64)>,
    /// Catalogue changes in the order they happened, which is what a till
    /// replays. A real store keeps this as a sequence column rather than a
    /// vector, but the shape of the answer is the same.
    changes: HashMap<u128, Vec<CatalogueChange>>,
    tokens: HashMap<TokenHash, Caller>,
    codes: HashMap<TokenHash, (Caller, SystemTime)>,
}

/// One catalogue change, as the server records it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogueChange {
    Upsert(Box<ItemWire>),
    Delete(u128),
}

/// What the in-memory store keeps about an enrolled terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalRecord {
    label: String,
    enrolled_at_ms: u64,
    last_seen_ms: Option<u64>,
}

/// Wall clock in milliseconds.
///
/// Saturates instead of failing. A clock set before 1970 is a misconfigured
/// machine, and refusing to answer a health question over it would hide the very
/// state an operator is trying to see.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
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
        self.enrol_labelled(tenant, terminal, "");
    }

    /// Enrol a terminal under a name a person would recognise.
    ///
    /// The label is what the health list is read by. A support call starts with
    /// "the one by the door", not with a 128-bit identifier.
    pub fn enrol_labelled(&self, tenant: u128, terminal: u128, label: &str) {
        let mut inner = self.lock();
        // Enrolling again keeps the original date, matching the `on conflict do
        // nothing` the Postgres store uses. A terminal that re-enrols has not
        // become a new device, and rewriting the date would erase how long it
        // has been in the shop.
        let record = inner
            .terminals
            .entry((tenant, terminal))
            .or_insert_with(|| TerminalRecord {
                label: String::new(),
                enrolled_at_ms: now_ms(),
                last_seen_ms: None,
            });
        if !label.is_empty() {
            record.label = label.to_owned();
        }
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
    ///
    /// Shadows the trait method of the same name, on purpose. Tests build a
    /// shop's catalogue synchronously before a runtime exists, and an inherent
    /// method wins method resolution, so those call sites keep working while the
    /// asynchronous trait method serves the HTTP route.
    pub fn upsert_item(&self, tenant: u128, item: ItemWire) -> u64 {
        let mut inner = self.lock();
        let log = inner.changes.entry(tenant).or_default();
        log.push(CatalogueChange::Upsert(Box::new(item)));
        log.len() as u64
    }

    /// Record a deletion. Shadows the trait method, for the reason above.
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
        // Arrival is recorded once. A replay stores the same sale again, and the
        // queue should keep showing when it first landed rather than moving to
        // the bottom every time a till retries.
        inner.received.entry((sale.tenant, sale.id)).or_insert_with(now_ms);
        inner.sales.insert((sale.tenant, sale.id), sale);
        Ok(())
    }

    async fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool> {
        Ok(self.lock().terminals.contains_key(&(tenant, terminal)))
    }

    async fn authenticate(&self, token: &TokenHash) -> Result<Option<Caller>> {
        Ok(self.lock().tokens.get(token).copied())
    }

    async fn store_token(&self, caller: Caller, token: &TokenHash) -> Result<()> {
        self.lock().tokens.insert(token.clone(), caller);
        Ok(())
    }

    async fn revoke_token(&self, token: &TokenHash) -> Result<bool> {
        Ok(self.lock().tokens.remove(token).is_some())
    }

    async fn revoke_all_tokens(&self, caller: Caller) -> Result<usize> {
        let mut inner = self.lock();
        let before = inner.tokens.len();
        inner.tokens.retain(|_, owner| *owner != caller);
        Ok(before.saturating_sub(inner.tokens.len()))
    }

    async fn issue_enrolment_code(
        &self,
        caller: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> Result<()> {
        let expires = SystemTime::now().checked_add(valid_for).ok_or(RepoError::Backend)?;
        self.lock().codes.insert(code.clone(), (caller, expires));
        Ok(())
    }

    async fn redeem_enrolment_code(&self, code: &TokenHash) -> Result<Option<Caller>> {
        let mut inner = self.lock();
        // Removed rather than marked, so a code cannot be used twice even if two
        // devices race to redeem it.
        let Some((caller, expires)) = inner.codes.remove(code) else {
            return Ok(None);
        };
        if SystemTime::now() > expires {
            return Ok(None);
        }
        Ok(Some(caller))
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
        if !inner.terminals.contains_key(&(tenant, terminal)) {
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

    async fn repair_queue(&self, tenant: u128, limit: u32) -> Result<Vec<RepairItem>> {
        let inner = self.lock();
        let mut found: Vec<RepairItem> = inner
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant)
            .filter(|sale| !inner.resolutions.contains_key(&(tenant, sale.id)))
            .filter_map(|sale| {
                let reason = sale.quarantine.as_ref()?;
                Some(RepairItem {
                    id: sale.id,
                    receipt_no: sale.receipt_no.clone(),
                    total_minor: sale.total_minor,
                    received_at_ms: inner
                        .received
                        .get(&(tenant, sale.id))
                        .copied()
                        .unwrap_or_default(),
                    reason: describe_quarantine(reason),
                })
            })
            .collect();

        // Sorted by id rather than by arrival. A sale id is a ULID, whose
        // leading bits are its mint time, so this is the order the shop rang
        // them up in even when a batch of a day's offline sales all arrived in
        // the same millisecond.
        found.sort_by_key(|item| item.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn resolve_quarantine(&self, tenant: u128, sale: u128, note: &str) -> Result<bool> {
        let mut inner = self.lock();
        let quarantined = inner
            .sales
            .get(&(tenant, sale))
            .is_some_and(|found| found.quarantine.is_some());
        if !quarantined || inner.resolutions.contains_key(&(tenant, sale)) {
            return Ok(false);
        }
        inner.resolutions.insert((tenant, sale), note.to_owned());
        Ok(true)
    }

    async fn terminal_health(&self, tenant: u128) -> Result<Vec<TerminalHealth>> {
        let inner = self.lock();
        let mut found: Vec<TerminalHealth> = inner
            .terminals
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|((_, terminal), record)| {
                let sales = inner
                    .sales
                    .values()
                    .filter(|sale| sale.tenant == tenant && sale.terminal == *terminal);
                let open_repairs = sales
                    .clone()
                    .filter(|sale| sale.quarantine.is_some())
                    .filter(|sale| !inner.resolutions.contains_key(&(tenant, sale.id)))
                    .count();

                TerminalHealth {
                    terminal: *terminal,
                    label: record.label.clone(),
                    epoch: inner
                        .counters
                        .get(&(tenant, *terminal))
                        .map_or(1, |(_, epoch)| *epoch),
                    enrolled_at_ms: record.enrolled_at_ms,
                    last_seen_ms: record.last_seen_ms,
                    sales: u64::try_from(sales.count()).unwrap_or(u64::MAX),
                    open_repairs: u64::try_from(open_repairs).unwrap_or(u64::MAX),
                }
            })
            .collect();

        // A stable order, so the list does not shuffle between two loads of the
        // same page and make an operator doubt what they read.
        found.sort_by_key(|health| health.terminal);
        Ok(found)
    }

    async fn mark_terminal_seen(&self, tenant: u128, terminal: u128) -> Result<()> {
        let mut inner = self.lock();
        let now = now_ms();
        if let Some(record) = inner.terminals.get_mut(&(tenant, terminal)) {
            record.last_seen_ms = Some(now);
        }
        Ok(())
    }

    async fn upsert_item(&self, tenant: u128, item: &ItemWire) -> Result<u64> {
        Ok(MemoryRepo::upsert_item(self, tenant, item.clone()))
    }

    async fn delete_item(&self, tenant: u128, item_id: u128) -> Result<u64> {
        Ok(MemoryRepo::delete_item(self, tenant, item_id))
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
