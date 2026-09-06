//! What a platform must provide, and two implementations for testing.
//!
//! Backends are deliberately stupid. They open, read, append, overwrite,
//! truncate and flush. Every decision about framing, ordering, recovery and
//! checkpointing lives in [`super::journal`], in the core, where one property
//! test covers all three targets at once.
//!
//! The alternative was a backend rich enough to be interesting, which would mean
//! writing the crash-safety protocol once in Rust for the server, once in Dart
//! for Android and once in JavaScript for the browser, and testing it properly in
//! none of them.

use alloc::vec::Vec;

use super::frame::Store;

/// A fixed, named blob. Snapshots use two slots so a checkpoint never overwrites
/// the only copy that works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blob {
    SnapshotA,
    SnapshotB,
    /// Standing terminal state: the receipt-number blocks in hand and the
    /// baskets parked at the counter.
    ///
    /// Deliberately not in the critical log. That log is emptied when the server
    /// confirms every sale in it, which is the normal end of a trading day, and
    /// emptying it would take the terminal's unspent receipt numbers and its
    /// parked baskets with it. A shop that drained last night would open next
    /// morning, offline, with no numbers to print.
    TerminalA,
    TerminalB,
}

impl Blob {
    /// The slot a checkpoint should write, given the one currently in use.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::SnapshotA => Self::SnapshotB,
            Self::SnapshotB => Self::SnapshotA,
            Self::TerminalA => Self::TerminalB,
            Self::TerminalB => Self::TerminalA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendError {
    /// The underlying store refused the operation. The journal treats this as
    /// fatal for the current commit and reports it upward; a till that cannot
    /// persist must not print a receipt.
    Io,
    /// A power cut or a crash was simulated in a test.
    PowerLoss,
}

pub type Result<T> = core::result::Result<T, BackendError>;

/// The whole contract a platform implements.
///
/// Synchronous on purpose. It never runs on a UI thread: in the browser the core
/// lives in a dedicated Web Worker where OPFS access handles are synchronous, on
/// Android it runs on a background thread. Making this async would put suspension
/// points inside a sale commit, which is where a second scan arriving mid-await
/// turns into a reentrancy bug.
pub trait Backend {
    /// Read a whole blob. Missing blobs read as empty rather than erroring,
    /// because a fresh install legitimately has none.
    fn read_blob(&self, blob: Blob) -> Result<Vec<u8>>;

    /// Replace a blob wholesale.
    fn write_blob(&mut self, blob: Blob, bytes: &[u8]) -> Result<()>;

    /// Read a whole log.
    fn read_log(&self, store: Store) -> Result<Vec<u8>>;

    /// Append to a log. Must not reorder against earlier appends.
    fn append_log(&mut self, store: Store, bytes: &[u8]) -> Result<()>;

    /// Cut a log back, used to discard a torn tail and to empty the replica log
    /// after a checkpoint.
    fn truncate_log(&mut self, store: Store, len: usize) -> Result<()>;

    /// The durability barrier. Returning `Ok` asserts that everything written so
    /// far survives a power cut. This is the promise a receipt is printed on, so
    /// a backend that cannot keep it must not pretend to: SQLite needs
    /// `synchronous=FULL`, OPFS needs a real `flush()`, and IndexedDB, which
    /// treats durability as a hint, is why it is not used here.
    fn flush(&mut self) -> Result<()>;
}

/// An in-memory backend for tests and for the server, which persists through
/// Postgres and has no use for any of this.
#[derive(Debug, Default, Clone)]
pub struct MemoryBackend {
    snapshot_a: Vec<u8>,
    snapshot_b: Vec<u8>,
    terminal_a: Vec<u8>,
    terminal_b: Vec<u8>,
    critical: Vec<u8>,
    replica: Vec<u8>,
}

impl MemoryBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn blob_mut(&mut self, blob: Blob) -> &mut Vec<u8> {
        match blob {
            Blob::SnapshotA => &mut self.snapshot_a,
            Blob::SnapshotB => &mut self.snapshot_b,
            Blob::TerminalA => &mut self.terminal_a,
            Blob::TerminalB => &mut self.terminal_b,
        }
    }

    fn blob(&self, blob: Blob) -> &Vec<u8> {
        match blob {
            Blob::SnapshotA => &self.snapshot_a,
            Blob::SnapshotB => &self.snapshot_b,
            Blob::TerminalA => &self.terminal_a,
            Blob::TerminalB => &self.terminal_b,
        }
    }

    fn log_mut(&mut self, store: Store) -> &mut Vec<u8> {
        match store {
            Store::Critical => &mut self.critical,
            Store::ReplicaCache => &mut self.replica,
        }
    }

    fn log(&self, store: Store) -> &Vec<u8> {
        match store {
            Store::Critical => &self.critical,
            Store::ReplicaCache => &self.replica,
        }
    }
}

impl Backend for MemoryBackend {
    fn read_blob(&self, blob: Blob) -> Result<Vec<u8>> {
        Ok(self.blob(blob).clone())
    }

    fn write_blob(&mut self, blob: Blob, bytes: &[u8]) -> Result<()> {
        let slot = self.blob_mut(blob);
        slot.clear();
        slot.extend_from_slice(bytes);
        Ok(())
    }

    fn read_log(&self, store: Store) -> Result<Vec<u8>> {
        Ok(self.log(store).clone())
    }

    fn append_log(&mut self, store: Store, bytes: &[u8]) -> Result<()> {
        self.log_mut(store).extend_from_slice(bytes);
        Ok(())
    }

    fn truncate_log(&mut self, store: Store, len: usize) -> Result<()> {
        self.log_mut(store).truncate(len);
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// How a simulated failure behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The operation fails and changes nothing.
    Fail,
    /// The operation writes only the first `bytes` of its input, then the device
    /// dies. This is the torn write that a checksum has to catch.
    Tear { bytes: usize },
    /// The device loses power immediately after this operation. Everything not
    /// yet flushed is lost, which is what an OS page cache actually does.
    PowerCut,
}

/// A backend that breaks on purpose.
///
/// The review that shaped this module asked for fault injection from day one,
/// on the grounds that a test culture for crash recovery is never retrofitted.
/// Every write path can be told to fail, tear, or cut power at a chosen
/// operation, and `durable` exposes what would have survived.
#[derive(Debug, Clone)]
pub struct FaultyBackend {
    live: MemoryBackend,
    /// State as of the last successful flush: what a power cut would leave.
    durable: MemoryBackend,
    operations: usize,
    fault_at: Option<(usize, Fault)>,
    dead: bool,
}

impl FaultyBackend {
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: MemoryBackend::new(),
            durable: MemoryBackend::new(),
            operations: 0,
            fault_at: None,
            dead: false,
        }
    }

    /// Start from an existing durable image, as a reboot would.
    #[must_use]
    pub fn from_durable(durable: MemoryBackend) -> Self {
        Self {
            live: durable.clone(),
            durable,
            operations: 0,
            fault_at: None,
            dead: false,
        }
    }

    /// Break at the nth write operation, counting from zero.
    #[must_use]
    pub fn with_fault(mut self, at: usize, fault: Fault) -> Self {
        self.fault_at = Some((at, fault));
        self
    }

    /// What survives a power cut: everything up to the last successful flush.
    #[must_use]
    pub fn durable(&self) -> MemoryBackend {
        self.durable.clone()
    }

    /// Whether the simulated device has died.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    #[must_use]
    pub fn operations(&self) -> usize {
        self.operations
    }

    /// Decide what this operation should do, and advance the counter.
    fn next_fault(&mut self) -> Option<Fault> {
        if self.dead {
            return Some(Fault::Fail);
        }
        let current = self.operations;
        self.operations = self.operations.saturating_add(1);
        match self.fault_at {
            Some((at, fault)) if at == current => Some(fault),
            _ => None,
        }
    }

    fn die(&mut self) {
        self.dead = true;
    }
}

impl Default for FaultyBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for FaultyBackend {
    fn read_blob(&self, blob: Blob) -> Result<Vec<u8>> {
        if self.dead {
            return Err(BackendError::PowerLoss);
        }
        self.live.read_blob(blob)
    }

    fn write_blob(&mut self, blob: Blob, bytes: &[u8]) -> Result<()> {
        match self.next_fault() {
            None => self.live.write_blob(blob, bytes),
            Some(Fault::Fail) => Err(BackendError::Io),
            Some(Fault::Tear { bytes: kept }) => {
                let partial = bytes.get(..kept.min(bytes.len())).unwrap_or_default();
                self.live.write_blob(blob, partial)?;
                self.die();
                Err(BackendError::PowerLoss)
            }
            Some(Fault::PowerCut) => {
                self.live.write_blob(blob, bytes)?;
                self.die();
                Err(BackendError::PowerLoss)
            }
        }
    }

    fn read_log(&self, store: Store) -> Result<Vec<u8>> {
        if self.dead {
            return Err(BackendError::PowerLoss);
        }
        self.live.read_log(store)
    }

    fn append_log(&mut self, store: Store, bytes: &[u8]) -> Result<()> {
        match self.next_fault() {
            None => self.live.append_log(store, bytes),
            Some(Fault::Fail) => Err(BackendError::Io),
            Some(Fault::Tear { bytes: kept }) => {
                let partial = bytes.get(..kept.min(bytes.len())).unwrap_or_default();
                self.live.append_log(store, partial)?;
                self.die();
                Err(BackendError::PowerLoss)
            }
            Some(Fault::PowerCut) => {
                self.live.append_log(store, bytes)?;
                self.die();
                Err(BackendError::PowerLoss)
            }
        }
    }

    fn truncate_log(&mut self, store: Store, len: usize) -> Result<()> {
        match self.next_fault() {
            None | Some(Fault::Tear { .. }) => self.live.truncate_log(store, len),
            Some(Fault::Fail) => Err(BackendError::Io),
            Some(Fault::PowerCut) => {
                self.live.truncate_log(store, len)?;
                self.die();
                Err(BackendError::PowerLoss)
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        match self.next_fault() {
            None => {
                // Everything written so far is now safe.
                self.durable = self.live.clone();
                Ok(())
            }
            Some(Fault::Fail) => Err(BackendError::Io),
            Some(Fault::Tear { .. } | Fault::PowerCut) => {
                // Power died during the flush: nothing new became durable.
                self.die();
                Err(BackendError::PowerLoss)
            }
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

    #[test]
    fn memory_backend_round_trips() {
        let mut backend = MemoryBackend::new();
        backend.append_log(Store::Critical, b"one").unwrap();
        backend.append_log(Store::Critical, b"two").unwrap();
        assert_eq!(backend.read_log(Store::Critical).unwrap(), b"onetwo");

        backend.truncate_log(Store::Critical, 3).unwrap();
        assert_eq!(backend.read_log(Store::Critical).unwrap(), b"one");

        backend.write_blob(Blob::SnapshotA, b"snap").unwrap();
        assert_eq!(backend.read_blob(Blob::SnapshotA).unwrap(), b"snap");
        assert!(backend.read_blob(Blob::SnapshotB).unwrap().is_empty());
    }

    #[test]
    fn logs_and_slots_do_not_bleed_into_each_other() {
        let mut backend = MemoryBackend::new();
        backend.append_log(Store::Critical, b"sale").unwrap();
        backend.append_log(Store::ReplicaCache, b"item").unwrap();
        assert_eq!(backend.read_log(Store::Critical).unwrap(), b"sale");
        assert_eq!(backend.read_log(Store::ReplicaCache).unwrap(), b"item");
    }

    #[test]
    fn a_power_cut_loses_everything_since_the_last_flush() {
        let mut backend = FaultyBackend::new();
        backend.append_log(Store::Critical, b"committed").unwrap();
        backend.flush().unwrap();
        backend.append_log(Store::Critical, b"in flight").unwrap();

        // The device dies before the second append is flushed.
        let survived = backend.durable();
        assert_eq!(survived.read_log(Store::Critical).unwrap(), b"committed");
    }

    #[test]
    fn a_torn_write_leaves_a_prefix_and_kills_the_device() {
        let mut backend = FaultyBackend::new().with_fault(0, Fault::Tear { bytes: 4 });
        let result = backend.append_log(Store::Critical, b"a whole sale record");
        assert_eq!(result, Err(BackendError::PowerLoss));
        assert!(backend.is_dead());
        assert_eq!(backend.live.read_log(Store::Critical).unwrap(), b"a wh");
    }

    #[test]
    fn a_dead_device_refuses_everything_afterwards() {
        let mut backend = FaultyBackend::new().with_fault(0, Fault::PowerCut);
        assert!(backend.append_log(Store::Critical, b"x").is_err());
        assert_eq!(backend.append_log(Store::Critical, b"y"), Err(BackendError::Io));
        assert_eq!(backend.read_log(Store::Critical), Err(BackendError::PowerLoss));
    }

    #[test]
    fn a_failed_flush_makes_nothing_durable() {
        let mut backend = FaultyBackend::new().with_fault(1, Fault::PowerCut);
        backend.append_log(Store::Critical, b"sale").unwrap();
        assert!(backend.flush().is_err());
        assert!(backend.durable().read_log(Store::Critical).unwrap().is_empty());
    }
}
