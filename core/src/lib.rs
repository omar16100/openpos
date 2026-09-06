//! openpos core.
//!
//! Everything that decides anything lives here: money math, the in-memory
//! catalogue replica, snapshot storage, the outbox and sync engine, receipt
//! number leases, and offline authentication. It compiles three ways: to WASM
//! for the browser till, to a native library for the Android till, and as an
//! ordinary crate linked into the server.
//!
//! The point of that arrangement is that the money path has exactly one
//! implementation. An offline total and a server total cannot disagree, because
//! they are the same code.

#![no_std]
extern crate alloc;

pub mod cart;
pub mod domain;
pub mod ids;
pub mod lease;
pub mod money;
pub mod replica;

pub use cart::{Cart, CartError, CartLimits, Tender, TenderKind, Ticket};
pub use ids::Ulid;
pub use lease::{Lease, LeaseBook, ReceiptNumber};
pub use money::{Bp, Milli, Minor, MoneyError};
pub use replica::{Item, ItemDelta, ItemId, Replica};
