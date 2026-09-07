//! Copyright (C) 2026 the openpos authors.
//!
//! This program is free software: you can redistribute it and modify it under
//! the terms of the GNU Affero General Public License as published by the Free
//! Software Foundation, version 3. It is distributed in the hope that it will
//! be useful, and with no warranty: see the LICENSE file at the root of this
//! repository, or <https://www.gnu.org/licenses/>.
//!
//! Section 13 is the one that matters here and is why this licence was chosen:
//! anybody who runs a modified copy of this as a service for other people has
//! to offer those people its source. A shop's own copy, modified for its own
//! counter, is its own business.

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

pub mod accounts;
pub mod auth;
pub mod cart;
pub mod domain;
pub mod ids;
pub mod lease;
pub mod money;
pub mod protocol;
pub mod receipt;
pub mod replica;
pub mod shift;
pub mod storage;
pub mod sync;
pub mod till;

pub use cart::{Cart, CartError, CartLimits, Tender, TenderKind, Ticket};
pub use ids::Ulid;
pub use lease::{Lease, LeaseBook, ReceiptNumber};
pub use money::{Bp, Milli, Minor, MoneyError};
pub use replica::{Item, ItemDelta, ItemId, Replica};
pub use shift::{CashDirection, CashMovement, Shift, ShiftError, ShiftId, XReport, ZReport};
pub use sync::{Outbox, PendingSale, SyncEngine, SyncStatus};
pub use till::{BootReport, CompletedSale, Till, TillError, TillStatus};
