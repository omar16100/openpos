//! Talking to the server, without ever standing between a cashier and a sale.
//!
//! Two directions, with different rules.
//!
//! **Push.** The outbox is not a separate queue. It is the critical log itself,
//! read back and filtered by what the server has not yet acknowledged. A queue
//! held alongside the ledger is a queue that can disagree with it: a crash
//! between writing the sale and enqueueing it loses the sale from sync, or
//! duplicates it. Deriving the queue from the ledger makes that class of bug
//! unrepresentable.
//!
//! **Pull.** Deltas are persisted before they are applied, and the cursor
//! advances only once that write is durable. The alternative loses a price the
//! cashier has already quoted: pulled, shown, then gone after a power cut, with
//! the cursor claiming it was seen.
//!
//! Nothing here performs I/O. The core builds requests and consumes responses;
//! the platform carries the bytes.

pub mod driver;
pub mod outbox;

use alloc::vec::Vec;

use crate::ids::Ulid;
use crate::replica::{ItemDelta, Replica};
use crate::storage::backend::Backend;
use crate::storage::frame::{PayloadKind, Store};
use crate::storage::journal::{Journal, JournalError};
use crate::storage::wire::{self, ItemDeltasV1, ItemV1, WireError, DELTAS_SCHEMA};

pub use outbox::{Outbox, PendingSale};

/// Translate a server's reply into the batch the replica log stores.
///
/// The explicit conversion is the point. Network and disk types are separate so
/// they can evolve apart, and a converter is where that separation is paid for:
/// one function to update when either side changes, instead of a silent
/// mismatch the day a field is added to only one of them.
pub fn deltas_from_pull(response: &crate::protocol::PullResponse) -> ItemDeltasV1 {
    ItemDeltasV1 {
        cursor: response.cursor,
        upserts: response.upserts.iter().map(item_from_wire).collect(),
        tombstones: response.tombstones.clone(),
    }
}

/// One item, network shape to disk shape.
#[must_use]
fn item_from_wire(item: &crate::protocol::ItemWire) -> ItemV1 {
    ItemV1 {
        id: item.id,
        code: item.code.clone(),
        name_en: item.name_en.clone(),
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

/// Wrap a pending sale for the wire, forwarding the committed bytes untouched.
///
/// Under the number they were written with, which is not always the number this
/// build writes. A till that was offline when it was upgraded holds sales in
/// the shape the older build wrote, and stamping today's number on them tells
/// the shop to read them as something they are not: postcard is positional, so
/// the shop cannot decode them, keeps the bytes as a repair nobody can read,
/// and the till drops them as sent. The goods, the tax and anybody's account go
/// with them. The shop knows every schema this build's ancestors wrote and
/// decodes by the one it is told.
#[must_use]
pub fn envelope_for(sale: &PendingSale) -> crate::protocol::SaleEnvelope {
    crate::protocol::SaleEnvelope {
        id: sale.id.to_u128(),
        schema: sale.schema,
        payload: sale.payload.clone(),
    }
}

/// Deltas beyond which the replica log is folded into a fresh snapshot.
///
/// Chosen from measurement rather than taste: applying 1,000 deltas and
/// reindexing takes 33 ms on a development machine, so roughly 500 ms on a cheap
/// tablet. Two thousand entries is about the point where replaying the log on a
/// cold start starts eating the boot budget, which is the one number that must
/// not drift.
pub const CHECKPOINT_AFTER_DELTAS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Journal(JournalError),
    Wire(WireError),
}

impl core::fmt::Display for SyncError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Journal(error) => write!(f, "{error}"),
            Self::Wire(error) => write!(f, "{error}"),
        }
    }
}

impl core::error::Error for SyncError {}

impl From<JournalError> for SyncError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<WireError> for SyncError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

pub type Result<T> = core::result::Result<T, SyncError>;

/// Where this terminal has reached, and what it still owes the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncStatus {
    /// Server sequence the replica is current as of.
    pub cursor: u64,
    /// Sales not yet acknowledged. This is what the till shows the cashier.
    pub unsynced: usize,
    /// Delta frames sitting in the replica log, awaiting a checkpoint.
    pub pending_deltas: usize,
    /// True when a snapshot was there and could not be read.
    ///
    /// Not fatal, because a snapshot is a cache: the catalogue is re-pulled from
    /// the server and the till opens. Reported because the alternative is a
    /// device that quietly re-downloads its whole catalogue every morning and
    /// nobody knowing why.
    pub snapshot_unreadable: bool,
}

impl SyncStatus {
    /// Whether the log has grown enough to be worth folding into a snapshot.
    #[must_use]
    pub fn wants_checkpoint(&self) -> bool {
        self.pending_deltas >= CHECKPOINT_AFTER_DELTAS
    }
}

/// Drives sync against a journal and a replica.
///
/// Holds no connection and performs no I/O: the caller fetches bytes and hands
/// them in. That keeps the whole protocol testable without a network, and keeps
/// the core free of an HTTP client it would otherwise need three versions of.
#[derive(Debug, Default)]
pub struct SyncEngine {
    cursor: u64,
}

impl SyncEngine {
    #[must_use]
    pub fn new(cursor: u64) -> Self {
        Self { cursor }
    }

    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Rebuild state after a cold start.
    ///
    /// The cursor is recovered from the snapshot and then advanced by any delta
    /// frames the log still holds, which is why both carry it. A delta frame that
    /// was written but not yet applied is replayed here, so a reboot between
    /// persisting and applying loses nothing.
    pub fn recover<B: Backend>(
        journal: &Journal<B>,
        replica: &mut Replica,
    ) -> Result<(Self, SyncStatus)> {
        let mut cursor = 0_u64;

        let mut snapshot_unreadable = false;
        if let Some((schema, bytes)) = journal.load_snapshot()? {
            // The schema the bytes carry, not this build's constant.
            match wire::decode_snapshot(schema, &bytes) {
                Ok((items, snapshot_cursor)) => {
                    *replica = Replica::from_items(items);
                    cursor = snapshot_cursor;
                }
                // A cache this build cannot read is a cache, not a catastrophe.
                // Refusing to open would be a till that will not sell because
                // its copy of the catalogue is stale, when the answer is to
                // fetch the catalogue again from cursor zero.
                Err(_) => snapshot_unreadable = true,
            }
        }

        let records = journal.read(Store::ReplicaCache)?;
        let mut pending_deltas = 0_usize;
        for record in &records {
            if record.header.kind != PayloadKind::ItemDeltas {
                continue;
            }
            let deltas = wire::decode_deltas(record.header.schema, &record.payload)?;
            apply_deltas_to_replica(replica, &deltas)?;
            cursor = cursor.max(deltas.cursor);
            pending_deltas = pending_deltas.saturating_add(1);
        }

        let unsynced = Outbox::pending(journal)?.len();
        Ok((
            Self { cursor },
            SyncStatus {
                cursor,
                unsynced,
                pending_deltas,
                snapshot_unreadable,
            },
        ))
    }

    /// Persist a pulled batch, then apply it, then advance the cursor.
    ///
    /// The ordering is the whole point. Applying first and persisting second
    /// means a power cut leaves the replica holding prices that no longer exist
    /// anywhere, with a cursor that will never fetch them again.
    pub fn apply_pull<B: Backend>(
        &mut self,
        journal: &mut Journal<B>,
        replica: &mut Replica,
        deltas: &ItemDeltasV1,
    ) -> Result<u64> {
        let bytes = wire::encode_deltas(deltas)?;
        journal.commit(
            Store::ReplicaCache,
            PayloadKind::ItemDeltas,
            DELTAS_SCHEMA,
            &bytes,
        )?;

        apply_deltas_to_replica(replica, deltas)?;
        self.cursor = self.cursor.max(deltas.cursor);
        Ok(self.cursor)
    }

    /// Fold the replica into a fresh snapshot and drop the deltas it covers.
    ///
    /// Never called on the sale path. A checkpoint rewrites a couple of megabytes
    /// and belongs at idle, between customers, with no ticket open.
    pub fn checkpoint<B: Backend>(
        &self,
        journal: &mut Journal<B>,
        replica: &Replica,
    ) -> Result<u64> {
        let bytes = wire::encode_snapshot(replica.items(), self.cursor)?;
        Ok(journal.checkpoint(&bytes)?)
    }

    /// What the till should show, and whether it is time to checkpoint.
    pub fn status<B: Backend>(&self, journal: &Journal<B>) -> Result<SyncStatus> {
        let pending_deltas = journal
            .read(Store::ReplicaCache)?
            .iter()
            .filter(|record| record.header.kind == PayloadKind::ItemDeltas)
            .count();
        Ok(SyncStatus {
            cursor: self.cursor,
            unsynced: Outbox::pending(journal)?.len(),
            pending_deltas,
            // Asked at boot, not here: this is the running state of the log, and
            // whether a snapshot could be read was settled when it was opened.
            snapshot_unreadable: false,
        })
    }
}

/// Apply a decoded batch to the in-memory catalogue.
fn apply_deltas_to_replica(replica: &mut Replica, deltas: &ItemDeltasV1) -> Result<()> {
    let mut changes: Vec<ItemDelta> = Vec::with_capacity(
        deltas.upserts.len().saturating_add(deltas.tombstones.len()),
    );
    for wire_item in &deltas.upserts {
        let item = ItemV1::into_domain(wire_item.clone())?;
        changes.push(ItemDelta::Upsert(item));
    }
    for id in &deltas.tombstones {
        changes.push(ItemDelta::Tombstone(Ulid::from_u128(*id)));
    }
    replica.apply(changes);
    Ok(())
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

    #[test]
    fn a_snapshot_is_read_under_the_schema_it_was_written_with() {
        use crate::storage::backend::{Backend, Blob};
        use crate::storage::frame::{self, FrameHeader};
        use crate::storage::backend::MemoryBackend;

        // A snapshot written by a build whose schema this one does not know.
        // Read under this build's constant it would decode as whatever the
        // current shape happens to be, and a till would boot with a catalogue
        // made of misread bytes rather than an error anybody can act on.
        let payload = crate::storage::wire::encode_snapshot(&[], 7).unwrap();
        let header = FrameHeader {
            store: Store::ReplicaCache,
            kind: PayloadKind::Snapshot,
            schema: 99,
            producer: 1,
            tenant: TENANT,
            terminal: TERMINAL,
            sequence: 1,
        };
        let mut bytes = Vec::new();
        frame::encode(&header, &payload, &mut bytes).unwrap();

        let mut backend = MemoryBackend::new();
        backend.write_blob(Blob::SnapshotA, &bytes).unwrap();
        backend.flush().unwrap();

        let (journal, _) = Journal::open(backend, TENANT, TERMINAL, 1).unwrap();
        let mut replica = Replica::new();

        let (engine, status) =
            SyncEngine::recover(&journal, &mut replica).expect("a till still opens");

        // Read under this build's constant it would decode as whatever the
        // current shape happens to be, and the till would boot with a catalogue
        // made of misread bytes. Read under its own, this build knows it cannot
        // read it.
        assert!(status.snapshot_unreadable);
        // And that is not fatal, because a snapshot is a cache. The till opens,
        // the cursor is zero, and the catalogue comes back from the server.
        // Refusing would be a till that will not sell because its copy of the
        // prices is stale.
        assert_eq!(engine.cursor(), 0);
        assert!(replica.is_empty());
    }

    #[test]
    fn a_retirement_pulled_after_the_item_takes_effect() {
        use crate::storage::backend::MemoryBackend;

        let mut journal = Journal::open(MemoryBackend::new(), TENANT, TERMINAL, 1)
            .expect("a journal opens")
            .0;
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        let mut item = ItemV1::from_domain(&item(1, 43_000));
        item.active = true;
        engine
            .apply_pull(
                &mut journal,
                &mut replica,
                &ItemDeltasV1 {
                    cursor: 6,
                    upserts: vec![item.clone()],
                    tombstones: vec![],
                },
            )
            .expect("the first page applies");
        assert!(replica.items()[0].active);

        // The shop stops selling it, which arrives as a later page holding the
        // same item with the flag turned off. A device that has already read the
        // item has to take the second one, or a discontinued line stays
        // sellable on every till that was running when it was retired.
        item.active = false;
        let cursor = engine
            .apply_pull(
                &mut journal,
                &mut replica,
                &ItemDeltasV1 {
                    cursor: 7,
                    upserts: vec![item],
                    tombstones: vec![],
                },
            )
            .expect("the second page applies");

        assert_eq!(cursor, 7);
        assert_eq!(replica.items().len(), 1, "a correction, not a second copy");
        assert!(!replica.items()[0].active);
    }

    use crate::domain::PriceMode;
    use crate::money::{Bp, Milli, Minor};
    use crate::replica::Item;
    use crate::storage::backend::MemoryBackend;
    use crate::storage::wire::ItemV1;

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

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
            supply: crate::domain::Supply::Standard,
            category: "".into(),
        }
    }

    fn deltas(cursor: u64, upserts: &[Item], tombstones: &[u128]) -> ItemDeltasV1 {
        ItemDeltasV1 {
            cursor,
            upserts: upserts.iter().map(ItemV1::from_domain).collect(),
            tombstones: tombstones.to_vec(),
        }
    }

    fn open(backend: MemoryBackend) -> Journal<MemoryBackend> {
        Journal::open(backend, TENANT, TERMINAL, 1).unwrap().0
    }

    #[test]
    fn a_pull_is_persisted_before_it_is_applied() {
        let mut journal = open(MemoryBackend::new());
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        engine
            .apply_pull(&mut journal, &mut replica, &deltas(5, &[item(1, 43_000)], &[]))
            .unwrap();

        assert_eq!(engine.cursor(), 5);
        assert_eq!(replica.len(), 1);
        // The frame is on disk, so a reboot right now replays it.
        assert_eq!(journal.read(Store::ReplicaCache).unwrap().len(), 1);
    }

    #[test]
    fn a_reboot_between_persisting_and_applying_loses_nothing() {
        let mut backend = MemoryBackend::new();
        {
            let mut journal = open(backend.clone());
            let mut replica = Replica::new();
            let mut engine = SyncEngine::new(0);
            engine
                .apply_pull(&mut journal, &mut replica, &deltas(9, &[item(1, 43_000)], &[]))
                .unwrap();
            backend = journal.backend().clone();
        }

        // Cold start with an empty replica: the log rebuilds it.
        let journal = open(backend);
        let mut replica = Replica::new();
        let (engine, status) = SyncEngine::recover(&journal, &mut replica).unwrap();

        assert_eq!(engine.cursor(), 9, "the cursor is recovered from the log");
        assert_eq!(replica.len(), 1);
        assert_eq!(status.pending_deltas, 1);
    }

    #[test]
    fn a_checkpoint_folds_the_log_and_keeps_the_cursor() {
        let mut journal = open(MemoryBackend::new());
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        engine
            .apply_pull(&mut journal, &mut replica, &deltas(3, &[item(1, 43_000)], &[]))
            .unwrap();
        engine
            .apply_pull(&mut journal, &mut replica, &deltas(4, &[item(2, 47_500)], &[]))
            .unwrap();
        engine.checkpoint(&mut journal, &replica).unwrap();

        assert!(journal.read(Store::ReplicaCache).unwrap().is_empty());

        // Boot from the snapshot alone.
        let mut restored = Replica::new();
        let (recovered, status) = SyncEngine::recover(&journal, &mut restored).unwrap();
        assert_eq!(recovered.cursor(), 4);
        assert_eq!(restored.len(), 2);
        assert_eq!(status.pending_deltas, 0);
    }

    #[test]
    fn tombstones_travel_through_the_log() {
        let mut journal = open(MemoryBackend::new());
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        engine
            .apply_pull(
                &mut journal,
                &mut replica,
                &deltas(1, &[item(1, 43_000), item(2, 47_500)], &[]),
            )
            .unwrap();
        engine
            .apply_pull(&mut journal, &mut replica, &deltas(2, &[], &[1]))
            .unwrap();
        assert_eq!(replica.len(), 1);

        let mut restored = Replica::new();
        let journal = open(journal.backend().clone());
        SyncEngine::recover(&journal, &mut restored).unwrap();
        assert_eq!(restored.len(), 1, "a delete must not come back on reboot");
        assert!(restored.by_id(Ulid::from_u128(1)).is_none());
    }

    #[test]
    fn status_reports_what_the_cashier_needs_to_see() {
        let mut journal = open(MemoryBackend::new());
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        engine
            .apply_pull(&mut journal, &mut replica, &deltas(1, &[item(1, 43_000)], &[]))
            .unwrap();
        let status = engine.status(&journal).unwrap();
        assert_eq!(status.cursor, 1);
        assert_eq!(status.unsynced, 0);
        assert_eq!(status.pending_deltas, 1);
        assert!(!status.wants_checkpoint());
    }

    #[test]
    fn a_snapshot_with_a_bad_rate_is_refused_rather_than_sold_at() {
        let mut journal = open(MemoryBackend::new());
        let mut replica = Replica::new();
        let mut engine = SyncEngine::new(0);

        let mut poisoned = deltas(1, &[item(1, 43_000)], &[]);
        poisoned.upserts[0].vat_bp = 50_000;
        let result = engine.apply_pull(&mut journal, &mut replica, &poisoned);

        assert_eq!(result, Err(SyncError::Wire(WireError::OutOfRange)));
        assert!(replica.is_empty(), "nothing invalid reaches the catalogue");
    }
}
