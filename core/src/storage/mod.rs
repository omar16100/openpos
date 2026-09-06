//! Persistence: the frame protocol, and the contract platform backends implement.
//!
//! The protocol lives here rather than in the backends because crash safety is
//! the part that must be property-tested, and the core is the only place a test
//! can reach all three targets at once. Backends stay deliberately thin: open,
//! read, append, flush, truncate.

pub mod backend;
pub mod frame;
pub mod journal;
pub mod wire;

pub use backend::{Backend, BackendError, Blob, Fault, FaultyBackend, MemoryBackend};
pub use frame::{FrameError, FrameHeader, PayloadKind, Store};
pub use journal::{Journal, JournalError, Recovery, Record};
pub use wire::{SaleCommitV1, WireError, SALE_SCHEMA, SNAPSHOT_SCHEMA};
