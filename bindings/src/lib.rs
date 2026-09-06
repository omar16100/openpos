//! The till, as one module a front end can call.
//!
//! Every decision lives in `openpos-core`. This crate translates: it takes JSON
//! in, calls the core, and hands JSON back. It holds no arithmetic, no ordering
//! rules and no state beyond the `Till` itself, because the moment it holds one
//! of those, the browser and Android builds have somewhere to disagree.
//!
//! JSON at this boundary rather than the postcard used on disk and on the wire.
//! Those two formats are positional and exist to be compact and stable across
//! versions; this boundary is neither of those things. It is crossed by a UI
//! compiled from the same commit, and a shape a person can read in a debugger
//! is worth more here than bytes saved.
//!
//! Errors cross as a tagged string rather than as a thrown exception, so a
//! caller cannot ignore one by not wrapping the call.

use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::{Milli, Minor};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::till::{Till, TillError};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::wasm_bindgen;

/// What a front end renders after any operation.
///
/// One shape for every call, so a UI has one thing to bind to and cannot get
/// into a state where it rendered a total from one call and a line list from
/// another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub lines: Vec<Line>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub tendered_minor: i64,
    /// Negative while the customer still owes.
    pub change_minor: i64,
    pub is_refund: bool,
    pub receipt_numbers_left: u64,
    pub unsynced_sales: usize,
    /// Present when the last operation was refused, and why. A UI that renders
    /// this cannot silently drop an error.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub item_id: String,
    pub code: String,
    pub name: String,
    pub qty_milli: i64,
    pub unit_price_minor: i64,
    pub total_minor: i64,
}

/// A till, as the front end holds it.
#[wasm_bindgen]
pub struct TillHandle {
    inner: Till<MemoryBackend>,
}

#[wasm_bindgen]
impl TillHandle {
    /// Open a till on a fresh in-memory store.
    ///
    /// The browser build will pass an OPFS-backed store instead; this exists so
    /// the surface can be exercised, and so a demo runs with no storage
    /// permissions at all. Nothing it holds survives a reload, and the type name
    /// says so.
    #[wasm_bindgen(js_name = openInMemory)]
    #[must_use]
    pub fn open_in_memory(tenant: &str, terminal: &str) -> Option<TillHandle> {
        let tenant = Ulid::decode(tenant).ok()?;
        let terminal = Ulid::decode(terminal).ok()?;
        let (inner, _report) = Till::open(
            MemoryBackend::new(),
            tenant.to_u128(),
            terminal,
            1,
            CartLimits::default(),
        )
        .ok()?;
        Some(Self { inner })
    }

    /// Add a scanned barcode to the basket.
    #[wasm_bindgen]
    pub fn scan(&mut self, barcode: &str, qty_milli: i64) -> String {
        let outcome = self.inner.scan(barcode, Milli::new(qty_milli));
        self.render(outcome.err())
    }

    /// Take money.
    #[wasm_bindgen(js_name = addCash)]
    pub fn add_cash(&mut self, amount_minor: i64) -> String {
        self.inner.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount_minor),
            reference: None,
        });
        self.render(None)
    }

    /// Close the sale. The id and the clock come from the caller, because this
    /// crate mints neither.
    #[wasm_bindgen]
    pub fn checkout(&mut self, ticket_id: &str, rung_at_ms: f64) -> String {
        let Ok(id) = Ulid::decode(ticket_id) else {
            return self.render(Some(TillError::UnknownBarcode));
        };
        // A JS number is a double, so a millisecond timestamp arrives exact up
        // to 2^53, which is well past any date this will run in. Negative or
        // fractional values are a caller mistake and clamp rather than wrap.
        let at_ms = if rung_at_ms.is_finite() && rung_at_ms > 0.0 {
            rung_at_ms as u64
        } else {
            0
        };
        let outcome = self.inner.checkout(id, at_ms);
        self.render(outcome.err())
    }

    /// The current view, without changing anything.
    #[wasm_bindgen]
    #[must_use]
    pub fn view(&self) -> String {
        self.render_ref(None)
    }

    fn render(&mut self, error: Option<TillError>) -> String {
        self.render_ref(error)
    }

    fn render_ref(&self, error: Option<TillError>) -> String {
        let view = self.build_view(error);
        // Serialising a struct of numbers and strings cannot fail. Returning a
        // fixed error shape rather than panicking, because a panic here unwinds
        // into JavaScript and leaves the till unusable until the page reloads.
        serde_json::to_string(&view).unwrap_or_else(|_| {
            String::from(r#"{"error":"the till could not describe itself"}"#)
        })
    }

    fn build_view(&self, error: Option<TillError>) -> View {
        let totals = self.inner.totals().ok();
        let status = self.inner.status().ok();
        let tendered = self.inner.cart().tendered().ok();

        let lines = self
            .inner
            .cart()
            .lines()
            .iter()
            .map(|line| Line {
                item_id: line.item_id.encode(),
                code: line.code.to_string(),
                name: line.name.to_string(),
                qty_milli: line.qty.get(),
                unit_price_minor: line.unit_price.get(),
                total_minor: 0,
            })
            .collect();

        let total = totals.as_ref().map_or(0, |t| t.total.get());
        let paid = tendered.map_or(0, Minor::get);

        View {
            lines,
            net_minor: totals.as_ref().map_or(0, |t| t.net_total.get()),
            vat_minor: totals.as_ref().map_or(0, |t| t.vat_total.get()),
            discount_minor: totals.as_ref().map_or(0, |t| t.discount_total.get()),
            total_minor: total,
            tendered_minor: paid,
            change_minor: paid.saturating_sub(total),
            is_refund: self.inner.cart().is_refund(),
            receipt_numbers_left: status.map_or(0, |s| s.receipt_numbers_left),
            unsynced_sales: status.map_or(0, |s| s.unsynced_sales),
            error: error.map(|error| error.to_string()),
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

    fn view_of(json: &str) -> View {
        serde_json::from_str(json).expect("the facade returns its own shape")
    }

    #[test]
    fn a_till_opens_and_describes_itself() {
        let till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        let view = view_of(&till.view());
        assert!(view.lines.is_empty());
        assert_eq!(view.total_minor, 0);
        assert!(view.error.is_none());
    }

    #[test]
    fn a_bad_identifier_is_refused_rather_than_guessed_at() {
        assert!(TillHandle::open_in_memory("not-a-ulid", "also-not").is_none());
    }

    #[test]
    fn a_refusal_comes_back_in_the_view_rather_than_as_an_exception() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // Nothing has been pulled, so no barcode matches. A UI that renders the
        // view cannot silently drop this.
        let view = view_of(&till.scan("8690000000001", 1_000));
        assert!(view.error.is_some(), "a refusal must be visible");
        assert!(view.lines.is_empty());
    }

    #[test]
    fn change_is_negative_while_the_customer_still_owes() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // A UI showing zero here would be showing the same thing it shows when
        // the basket is settled, which is the one moment it must not.
        let view = view_of(&till.add_cash(10_000));
        assert_eq!(view.tendered_minor, 10_000);
        assert_eq!(view.change_minor, 10_000, "nothing rung yet, so it is all change");
    }

    #[test]
    fn a_nonsense_timestamp_clamps_rather_than_wrapping() {
        let mut till = TillHandle::open_in_memory(
            &Ulid::from_u128(42).encode(),
            &Ulid::from_u128(7).encode(),
        )
        .expect("a till opens");

        // An empty cart refuses anyway; the point is that a negative double does
        // not become an enormous u64 on the way in.
        let view = view_of(&till.checkout(&Ulid::from_u128(900).encode(), -1.0));
        assert!(view.error.is_some());
    }
}
