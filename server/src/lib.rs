//! openpos server: sync hub and back office.
//!
//! Links the same `openpos-core` crate the till runs, so a synced sale is
//! revalidated with the identical arithmetic that produced it. A disagreement
//! therefore cannot be a rounding difference between two implementations,
//! because there is only one.

pub mod ingest;
pub mod repo;

pub use ingest::{push, IngestError};
pub use repo::{LeaseRecord, MemoryRepo, Repository, StoredSale};
