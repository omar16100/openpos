//! The crash-safety protocol: commit, recover, checkpoint.
//!
//! Two rules drive everything here.
//!
//! **A sale is durable before its receipt prints.** Paper leaving the printer is
//! the customer's evidence that they paid. If the tablet dies a moment later and
//! the sale was never on disk, the shop has given away goods, consumed a receipt
//! number invisibly, and has nothing to reconcile against. So a commit is one
//! frame containing the whole sale, followed by a flush, and only then may the
//! caller print.
//!
//! **A checkpoint never overwrites the only good copy.** Snapshots alternate
//! between two slots, each self-describing and carrying its own generation, so a
//! device that dies mid-checkpoint still boots from the other slot. Cold start is
//! the product's single promise; a corrupt snapshot breaks it permanently, where
//! a lost delta merely re-syncs.

use alloc::vec::Vec;

use super::backend::{Backend, BackendError, Blob};
use super::frame::{self, FrameHeader, PayloadKind, Store};

/// Why a journal operation failed.
///
/// Every variant means the same thing to a caller mid-sale: nothing was made
/// durable, so do not print a receipt. The cause is carried through for
/// diagnostics rather than for control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    Backend(BackendError),
}

impl From<BackendError> for JournalError {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
}

pub type Result<T> = core::result::Result<T, JournalError>;

/// What opening the journal found and repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recovery {
    /// Frames that verified in the critical log.
    pub critical_frames: usize,
    /// Frames that verified in the replica log.
    pub replica_frames: usize,
    /// Bytes discarded as a torn tail, per store. Non-zero means a device died
    /// mid-append, which is worth surfacing in the till's diagnostics.
    pub critical_discarded: usize,
    pub replica_discarded: usize,
    /// Generation of the snapshot that was loadable, if any.
    pub snapshot_generation: Option<u64>,
    /// True when one snapshot slot failed to verify. Not fatal, since the other
    /// slot is what booted, but it means a checkpoint was interrupted.
    pub snapshot_slot_damaged: bool,
}

impl Recovery {
    /// Whether anything was lost or repaired.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.critical_discarded == 0 && self.replica_discarded == 0 && !self.snapshot_slot_damaged
    }
}

/// A committed record read back from a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

/// The persistence protocol over a platform backend.
pub struct Journal<B: Backend> {
    backend: B,
    tenant: u128,
    terminal: u128,
    producer: u16,
    /// Core-owned monotonic counter, never a wall clock.
    sequence: u64,
    /// Slot holding the newest good snapshot, and its generation.
    active_slot: Blob,
    /// Slot holding the newest standing terminal state, and its generation.
    terminal_slot: Blob,
    terminal_generation: u64,
    generation: u64,
    /// Bytes currently in each log, tracked so a failed commit can be rolled
    /// back without reading the whole log to find out where it ended.
    critical_len: usize,
    replica_len: usize,
}

impl<B: Backend> Journal<B> {
    /// Open, repair and report.
    ///
    /// Repair happens here, once, rather than being left for the first write to
    /// trip over: a torn tail is truncated so the next append cannot be stranded
    /// behind unreadable bytes.
    pub fn open(backend: B, tenant: u128, terminal: u128, producer: u16) -> Result<(Self, Recovery)> {
        let mut journal = Self {
            backend,
            tenant,
            terminal,
            producer,
            sequence: 0,
            active_slot: Blob::SnapshotA,
            terminal_slot: Blob::TerminalB,
            terminal_generation: 0,
            generation: 0,
            critical_len: 0,
            replica_len: 0,
        };
        let recovery = journal.recover()?;
        Ok((journal, recovery))
    }

    fn recover(&mut self) -> Result<Recovery> {
        let mut recovery = Recovery::default();
        let mut highest_sequence = 0_u64;

        for store in [Store::Critical, Store::ReplicaCache] {
            let bytes = self.backend.read_log(store)?;
            let scan = frame::scan(&bytes);
            let discarded = bytes.len().saturating_sub(scan.valid_len);

            for found in &scan.frames {
                highest_sequence = highest_sequence.max(found.header.sequence);
            }
            let count = scan.frames.len();

            if discarded > 0 {
                // Cut the torn tail away now. Leaving it means every future
                // append lands after bytes that will never verify, so the log is
                // unreadable from the tear onward.
                self.backend.truncate_log(store, scan.valid_len)?;
                self.backend.flush()?;
            }

            match store {
                Store::Critical => {
                    recovery.critical_frames = count;
                    recovery.critical_discarded = discarded;
                    self.critical_len = scan.valid_len;
                }
                Store::ReplicaCache => {
                    recovery.replica_frames = count;
                    recovery.replica_discarded = discarded;
                    self.replica_len = scan.valid_len;
                }
            }
        }

        // Pick the newest snapshot slot that verifies. Both slots are
        // self-describing, so no manifest is needed and there is no third file
        // to tear.
        let mut best: Option<(Blob, u64)> = None;
        let mut damaged = false;
        for slot in [Blob::SnapshotA, Blob::SnapshotB] {
            let bytes = self.backend.read_blob(slot)?;
            if bytes.is_empty() {
                continue;
            }
            match frame::decode(&bytes) {
                Ok(found) if found.header.kind == PayloadKind::Snapshot => {
                    let generation = found.header.sequence;
                    if best.is_none_or(|(_, current)| generation > current) {
                        best = Some((slot, generation));
                    }
                }
                _ => damaged = true,
            }
        }

        if let Some((slot, generation)) = best {
            self.active_slot = slot;
            self.generation = generation;
            recovery.snapshot_generation = Some(generation);
            highest_sequence = highest_sequence.max(generation);
        }
        recovery.snapshot_slot_damaged = damaged;

        // Same again for the standing terminal state. Recovering which slot is
        // newest matters more here than it looks: writing the next state into
        // the wrong slot with a generation lower than the one already there
        // would make the stale copy win every subsequent boot, and the stale
        // copy is a set of receipt numbers this terminal has already spent.
        for slot in [Blob::TerminalA, Blob::TerminalB] {
            let bytes = self.backend.read_blob(slot)?;
            if bytes.is_empty() {
                continue;
            }
            if let Ok(found) = frame::decode(&bytes)
                && found.header.kind == PayloadKind::TerminalState
                && found.header.sequence >= self.terminal_generation
            {
                self.terminal_slot = slot;
                self.terminal_generation = found.header.sequence;
            }
        }

        self.sequence = highest_sequence;
        Ok(recovery)
    }

    /// Sequence number the next commit will carry.
    #[must_use]
    pub fn next_sequence(&self) -> u64 {
        self.sequence.saturating_add(1)
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Commit one record durably.
    ///
    /// Returns only after the flush succeeds, so a caller holding an `Ok` may
    /// print. Everything belonging to one business event must arrive in a single
    /// payload: a sale carries its ticket, the lease state after consuming a
    /// number, the shift and cash movement, and the outbox entry together. Split
    /// across two commits, a crash between them yields either a receipt number
    /// consumed with no sale behind it, or a sale that re-uses a number after
    /// reboot.
    pub fn commit(
        &mut self,
        store: Store,
        kind: PayloadKind,
        schema: u16,
        payload: &[u8],
    ) -> Result<u64> {
        let sequence = self.next_sequence();
        let header = FrameHeader {
            store,
            kind,
            schema,
            producer: self.producer,
            tenant: self.tenant,
            terminal: self.terminal,
            sequence,
        };

        let mut bytes = Vec::with_capacity(frame::HEADER_LEN.saturating_add(payload.len()));
        frame::encode(&header, payload, &mut bytes);

        // A commit that cannot be completed must leave no trace. Without this,
        // an append that succeeds followed by a flush that fails leaves the
        // frame in the log, where the next successful commit flushes it: the
        // cashier saw an error, re-rang the basket under a new id, and the shop
        // syncs two sales for one basket. Found by the recovery property tests.
        let mark = self.log_len(store);
        if let Err(error) = self.backend.append_log(store, &bytes) {
            self.rollback(store, mark);
            return Err(error.into());
        }
        if let Err(error) = self.backend.flush() {
            self.rollback(store, mark);
            return Err(error.into());
        }

        self.set_log_len(store, mark.saturating_add(bytes.len()));
        self.sequence = sequence;
        Ok(sequence)
    }

    fn log_len(&self, store: Store) -> usize {
        match store {
            Store::Critical => self.critical_len,
            Store::ReplicaCache => self.replica_len,
        }
    }

    fn set_log_len(&mut self, store: Store, len: usize) {
        match store {
            Store::Critical => self.critical_len = len,
            Store::ReplicaCache => self.replica_len = len,
        }
    }

    /// Undo a partial append. Best effort on purpose: if the device has died the
    /// truncation fails too, but nothing was flushed either, so the bytes vanish
    /// with the power.
    fn rollback(&mut self, store: Store, mark: usize) {
        let _ = self.backend.truncate_log(store, mark);
        self.set_log_len(store, mark);
    }

    /// Append without waiting for durability.
    ///
    /// Only for records whose loss is acceptable: a print attempt, a diagnostic.
    /// Never for anything that moves money or stock.
    pub fn append_unflushed(
        &mut self,
        store: Store,
        kind: PayloadKind,
        schema: u16,
        payload: &[u8],
    ) -> Result<u64> {
        let sequence = self.next_sequence();
        let header = FrameHeader {
            store,
            kind,
            schema,
            producer: self.producer,
            tenant: self.tenant,
            terminal: self.terminal,
            sequence,
        };
        let mut bytes = Vec::new();
        frame::encode(&header, payload, &mut bytes);
        let mark = self.log_len(store);
        if let Err(error) = self.backend.append_log(store, &bytes) {
            self.rollback(store, mark);
            return Err(error.into());
        }
        self.set_log_len(store, mark.saturating_add(bytes.len()));
        self.sequence = sequence;
        Ok(sequence)
    }

    /// Every verified record in a log, in order.
    pub fn read(&self, store: Store) -> Result<Vec<Record>> {
        let bytes = self.backend.read_log(store)?;
        let scan = frame::scan(&bytes);
        Ok(scan
            .frames
            .iter()
            .map(|found| Record {
                header: found.header,
                payload: found.payload.to_vec(),
            })
            .collect())
    }

    /// The newest snapshot payload, if one is loadable.
    pub fn load_snapshot(&self) -> Result<Option<Vec<u8>>> {
        let bytes = self.backend.read_blob(self.active_slot)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        match frame::decode(&bytes) {
            Ok(found) if found.header.kind == PayloadKind::Snapshot => {
                Ok(Some(found.payload.to_vec()))
            }
            _ => Ok(None),
        }
    }

    /// The terminal's standing state, from whichever slot is newer and verifies.
    ///
    /// Both slots are read rather than a pointer file being consulted, for the
    /// same reason the snapshot does it: a pointer is a third thing that can
    /// disagree with the two it describes.
    pub fn load_terminal_state(&self) -> Result<Option<Vec<u8>>> {
        let mut best: Option<(u64, Vec<u8>)> = None;
        for slot in [Blob::TerminalA, Blob::TerminalB] {
            let bytes = self.backend.read_blob(slot)?;
            if bytes.is_empty() {
                continue;
            }
            if let Ok(found) = frame::decode(&bytes) {
                if found.header.kind != PayloadKind::TerminalState {
                    continue;
                }
                let newer = best
                    .as_ref()
                    .is_none_or(|(generation, _)| found.header.sequence > *generation);
                if newer {
                    best = Some((found.header.sequence, found.payload.to_vec()));
                }
            }
        }
        Ok(best.map(|(_, payload)| payload))
    }

    /// Write the terminal's standing state to the slot not currently in use.
    ///
    /// Alternating slots means a device dying mid-write still boots from the
    /// previous state, which is one trading period stale at worst. Overwriting
    /// in place would risk a torn write leaving a terminal with no record of the
    /// receipt numbers it owns, and no way to tell that it had any.
    pub fn write_terminal_state(&mut self, payload: &[u8]) -> Result<u64> {
        let generation = self.terminal_generation.saturating_add(1);
        let target = self.terminal_slot.other();

        let header = FrameHeader {
            store: Store::Critical,
            kind: PayloadKind::TerminalState,
            schema: super::wire::TERMINAL_SCHEMA,
            producer: self.producer,
            tenant: self.tenant,
            terminal: self.terminal,
            sequence: generation,
        };
        let mut bytes = Vec::with_capacity(frame::HEADER_LEN.saturating_add(payload.len()));
        frame::encode(&header, payload, &mut bytes);

        self.backend.write_blob(target, &bytes)?;
        self.backend.flush()?;

        self.terminal_slot = target;
        self.terminal_generation = generation;
        Ok(generation)
    }

    /// Write a new snapshot and drop the deltas it now covers.
    ///
    /// Ordering is the whole protocol:
    ///
    /// 1. write the snapshot into the slot **not** currently in use
    /// 2. flush, so it is durable before anything depends on it
    /// 3. only then truncate the replica log
    ///
    /// Dying between 1 and 2 leaves the old slot intact and the log untouched, so
    /// the next boot is exactly the previous state. Dying between 2 and 3 leaves
    /// both slots valid and replays deltas already folded into the new snapshot,
    /// which is harmless because item deltas are upserts and tombstones and
    /// therefore idempotent. Truncating first would lose those deltas outright if
    /// the snapshot write then failed.
    pub fn checkpoint(&mut self, payload: &[u8]) -> Result<u64> {
        let generation = self.generation.saturating_add(1).max(self.next_sequence());
        let target = self.active_slot.other();

        let header = FrameHeader {
            store: Store::ReplicaCache,
            kind: PayloadKind::Snapshot,
            schema: 1,
            producer: self.producer,
            tenant: self.tenant,
            terminal: self.terminal,
            sequence: generation,
        };
        let mut bytes = Vec::with_capacity(frame::HEADER_LEN.saturating_add(payload.len()));
        frame::encode(&header, payload, &mut bytes);

        self.backend.write_blob(target, &bytes)?;
        self.backend.flush()?;

        // The new snapshot is durable; the deltas it covers are now redundant.
        self.backend.truncate_log(Store::ReplicaCache, 0)?;
        self.backend.flush()?;
        self.replica_len = 0;

        self.active_slot = target;
        self.generation = generation;
        self.sequence = self.sequence.max(generation);
        Ok(generation)
    }

    /// Discard acknowledged records from the critical log.
    ///
    /// Called only with a length computed from records the server has confirmed.
    /// The critical log is truncated on sync acknowledgement, never on
    /// checkpoint, which is why the two stores cannot share one log.
    pub fn truncate_critical(&mut self, valid_len: usize) -> Result<()> {
        self.backend.truncate_log(Store::Critical, valid_len)?;
        self.backend.flush()?;
        self.critical_len = valid_len;
        Ok(())
    }

    /// Borrow the backend, for tests and for platform-specific maintenance.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
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

    use super::super::backend::{Fault, FaultyBackend, MemoryBackend};
    use super::*;

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    fn open(backend: MemoryBackend) -> (Journal<MemoryBackend>, Recovery) {
        Journal::open(backend, TENANT, TERMINAL, 1).unwrap()
    }

    #[test]
    fn commits_and_reads_back_in_order() {
        let (mut journal, recovery) = open(MemoryBackend::new());
        assert!(recovery.is_clean());
        assert_eq!(recovery.critical_frames, 0);

        journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"sale one").unwrap();
        journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"sale two").unwrap();

        let records = journal.read(Store::Critical).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].payload, b"sale one");
        assert_eq!(records[1].header.sequence, 2);
    }

    #[test]
    fn sequences_continue_across_a_reopen() {
        let mut backend = MemoryBackend::new();
        {
            let (mut journal, _) = open(backend.clone());
            journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"a").unwrap();
            journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"b").unwrap();
            backend = journal.backend().clone();
        }
        let (journal, recovery) = open(backend);
        assert_eq!(recovery.critical_frames, 2);
        assert_eq!(journal.next_sequence(), 3, "a reopen must not reissue sequences");
    }

    #[test]
    fn a_torn_tail_is_truncated_on_open_so_the_next_append_is_readable() {
        let mut backend = MemoryBackend::new();
        {
            let (mut journal, _) = open(backend.clone());
            journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"kept").unwrap();
            journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"torn away").unwrap();
            backend = journal.backend().clone();
        }
        // Simulate a device that died partway through the second append.
        let whole = backend.read_log(Store::Critical).unwrap();
        backend.truncate_log(Store::Critical, whole.len() - 5).unwrap();

        let (mut journal, recovery) = open(backend);
        assert_eq!(recovery.critical_frames, 1);
        assert!(recovery.critical_discarded > 0);
        assert!(!recovery.is_clean());

        // The repaired log accepts new work and reads back cleanly.
        journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"after repair").unwrap();
        let records = journal.read(Store::Critical).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].payload, b"after repair");
    }

    #[test]
    fn a_checkpoint_alternates_slots_and_clears_the_delta_log() {
        let (mut journal, _) = open(MemoryBackend::new());
        journal.commit(Store::ReplicaCache, PayloadKind::ItemDeltas, 1, b"delta").unwrap();

        journal.checkpoint(b"snapshot one").unwrap();
        assert_eq!(journal.load_snapshot().unwrap().as_deref(), Some(&b"snapshot one"[..]));
        assert!(journal.read(Store::ReplicaCache).unwrap().is_empty());
        let first_slot = journal.active_slot;

        journal.checkpoint(b"snapshot two").unwrap();
        assert_ne!(journal.active_slot, first_slot, "a checkpoint must not overwrite the live slot");
        assert_eq!(journal.load_snapshot().unwrap().as_deref(), Some(&b"snapshot two"[..]));
    }

    #[test]
    fn a_device_dying_mid_checkpoint_still_boots_from_the_other_slot() {
        let mut durable = MemoryBackend::new();
        {
            let (mut journal, _) = open(durable.clone());
            journal.commit(Store::ReplicaCache, PayloadKind::ItemDeltas, 1, b"delta").unwrap();
            journal.checkpoint(b"good snapshot").unwrap();
            durable = journal.backend().clone();
        }

        // Second checkpoint tears halfway through writing the other slot.
        let faulty = FaultyBackend::from_durable(durable).with_fault(0, Fault::Tear { bytes: 20 });
        let (mut journal, _) = Journal::open(faulty, TENANT, TERMINAL, 1).unwrap();
        assert!(journal.checkpoint(b"doomed snapshot").is_err());

        // Reboot on whatever survived.
        let after_reboot = journal.backend().durable();
        let (journal, recovery) = open(after_reboot);
        assert_eq!(
            journal.load_snapshot().unwrap().as_deref(),
            Some(&b"good snapshot"[..]),
            "cold start must survive a checkpoint that died halfway"
        );
        assert!(recovery.snapshot_generation.is_some());
    }

    #[test]
    fn an_unflushed_commit_does_not_survive_a_power_cut() {
        let faulty = FaultyBackend::new();
        let (mut journal, _) = Journal::open(faulty, TENANT, TERMINAL, 1).unwrap();

        journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"durable sale").unwrap();
        journal
            .append_unflushed(Store::Critical, PayloadKind::PrintAttempt, 1, b"print log")
            .unwrap();

        let after_reboot = journal.backend().durable();
        let (journal, _) = open(after_reboot);
        let records = journal.read(Store::Critical).unwrap();
        assert_eq!(records.len(), 1, "only the flushed sale survives");
        assert_eq!(records[0].payload, b"durable sale");
    }

    #[test]
    fn a_failed_commit_is_reported_so_the_caller_does_not_print() {
        let faulty = FaultyBackend::new().with_fault(0, Fault::Fail);
        let (mut journal, _) = Journal::open(faulty, TENANT, TERMINAL, 1).unwrap();

        let result = journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"sale");
        assert_eq!(result, Err(JournalError::Backend(BackendError::Io)));
        assert_eq!(journal.next_sequence(), 1, "a failed commit consumes no sequence");
    }

    #[test]
    fn the_two_stores_have_independent_lifecycles() {
        let (mut journal, _) = open(MemoryBackend::new());
        journal.commit(Store::Critical, PayloadKind::SaleCommit, 1, b"unsynced sale").unwrap();
        journal.commit(Store::ReplicaCache, PayloadKind::ItemDeltas, 1, b"delta").unwrap();

        // A checkpoint clears catalogue deltas and must not touch the sale that
        // has not reached the server yet.
        journal.checkpoint(b"snapshot").unwrap();
        assert!(journal.read(Store::ReplicaCache).unwrap().is_empty());
        assert_eq!(journal.read(Store::Critical).unwrap().len(), 1);
    }
}
