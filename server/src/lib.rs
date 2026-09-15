//! openpos server: sync hub and back office.
//!
//! Links the same `openpos-core` crate the till runs, so a synced sale is
//! revalidated with the identical arithmetic that produced it. A disagreement
//! therefore cannot be a rounding difference between two implementations,
//! because there is only one.

pub mod auth;
pub mod export;
pub mod http;
pub mod ingest;
pub mod pg;
pub mod ratelimit;
pub mod repo;
/// Read a stored catalogue payload under the schema it was written in.
///
/// Exposed because the rows a shop already holds are the thing worth testing
/// against, and the bytes for that live in a test rather than in here.
#[must_use]
pub fn read_catalogue_payload(
    schema: i16,
    bytes: &[u8],
) -> Option<openpos_core::protocol::ItemWire> {
    pg::decode_catalogue_payload(schema, bytes)
}


pub use ingest::{push, IngestError};
pub use auth::{Caller, Token, TokenHash};
pub use export::{export_tenant, import_tenant, ExportBundle, ExportError, IdentityPolicy};
pub use http::{router, AppState};
pub use pg::PgRepo;
pub use repo::{
    CataloguePage, LeaseRecord, MemoryRepo, RepairItem, Repository, StoredSale, TerminalHealth,
};
