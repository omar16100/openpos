//! Persistence: the frame protocol, and the contract platform backends implement.
//!
//! The protocol lives here rather than in the backends because crash safety is
//! the part that must be property-tested, and the core is the only place a test
//! can reach all three targets at once. Backends stay deliberately thin: open,
//! read, append, flush, truncate.

pub mod frame;

pub use frame::{FrameError, FrameHeader, PayloadKind, Store};
