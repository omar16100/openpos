//! openpos server: sync hub and back office.
//!
//! Links the same `openpos-core` crate the till runs, so a synced sale is
//! revalidated with the identical arithmetic that produced it. A disagreement
//! therefore cannot be a rounding difference between two implementations,
//! because there is only one.

pub mod auth;
pub mod http;
pub mod ingest;
pub mod pg;
pub mod ratelimit;
pub mod repo;

pub use ingest::{push, IngestError};
pub use auth::{Caller, Token, TokenHash};
pub use http::{router, AppState};
pub use pg::PgRepo;
pub use repo::{CataloguePage, LeaseRecord, MemoryRepo, Repository, StoredSale};
