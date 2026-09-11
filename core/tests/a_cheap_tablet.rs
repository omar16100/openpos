//! What a shop of ten thousand items costs the till that has to hold it.
//!
//! The device this is for is a cheap Android tablet, and every figure below is
//! taken on a developer's machine, which is not one. What the numbers are for
//! is the shape of the cost and the direction it moves in: something that takes
//! a millisecond here is fine there, something that takes a second here is a
//! till that stutters in front of a customer, and something quadratic here is
//! quadratic there.
//!
//! Five things a till does that grow with the catalogue:
//!
//! 1. Reading its own catalogue back at boot, which is the cold start the whole
//!    design exists for.
//! 2. Building the search index, which happens at boot and again after every
//!    page of catalogue changes.
//! 3. A scan, which is the thing a cashier does all day.
//! 4. A keystroke in the search box, which is what a cashier does when the
//!    barcode will not read.
//! 5. Writing the catalogue back down as a snapshot.
//!
//! The bounds are deliberately loose: this is a guard against something going
//! quadratic, not a benchmark to tune against. It only asserts in release,
//! because a debug build is a different machine.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::time::{Duration, Instant};

use openpos_core::domain::Supply;
use openpos_core::domain::pricing::{PriceMode, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::{Item, ItemDelta, Replica};
use openpos_core::storage::wire::{ItemV1, ItemDeltasV1, decode_snapshot, encode_snapshot};

/// A shop with ten thousand lines. Bigger than the shops this is for, which is
/// the point: the shop it stops working for should be one nobody has.
const ITEMS: usize = 10_000;

/// What a page of catalogue changes holds, matching the server's own page.
const PAGE: usize = 200;

fn item(index: usize) -> Item {
    // Names that share prefixes, because a catalogue where every name is
    // unique in its first two letters is a search index with no work to do.
    let kind = ["Rice", "Oil", "Soap", "Atta", "Dal", "Sugar", "Salt", "Tea"][index % 8];
    Item {
        id: Ulid::from_u128(index as u128 + 1),
        code: format!("SKU{index:05}").into(),
        name_en: format!("{kind} {index} pack").into(),
        name_bn: format!("চাল {index} প্যাক").into(),
        unit: "Nos".into(),
        price: Minor::new(4_300 + (index as i64 % 500)),
        cost: Minor::new(3_800),
        vat_rate: Bp::new(1_500).unwrap(),
        price_mode: PriceMode::Exclusive,
        vat_base: VatBase::Discounted,
        barcodes: vec![format!("869{index:010}").into()],
        on_hand: Milli::new(40_000),
        active: true,
        supply: Supply::Standard,
        category: kind.to_lowercase().into(),
    }
}

fn took(what: &str, run: impl FnOnce()) -> Duration {
    let started = Instant::now();
    run();
    let spent = started.elapsed();
    println!("  {what}: {spent:?}");
    spent
}

/// Assert only where the measurement means something.
///
/// A debug build is ten to fifty times slower than what ships, so a bound that
/// held there would either be useless in release or fail every time somebody
/// runs the suite the ordinary way. The numbers still print in both.
fn no_slower_than(what: &str, spent: Duration, limit: Duration) {
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        spent < limit,
        "{what} took {spent:?} for a shop of {ITEMS} items, and the bound is {limit:?}. That bound \
         is loose on purpose: this is a guard against something turning quadratic, not a target to \
         tune against. The device this is for is a cheap tablet, so whatever this machine takes, \
         assume ten times worse in a shop."
    );
}

#[test]
fn a_shop_of_ten_thousand_items_is_a_till_that_still_works() {
    println!("a catalogue of {ITEMS} items, on this machine:");

    let items: Vec<Item> = (0..ITEMS).map(item).collect();

    // 1. The index, which is built at boot and again after every page of
    //    changes. This is the one with the most room to go wrong: it is the
    //    whole catalogue's worth of work, and a till catching up from nothing
    //    pays it once per page.
    let mut replica = Replica::new();
    let building = took("building the search index", || {
        replica.apply(items.iter().cloned().map(ItemDelta::Upsert));
    });
    assert_eq!(replica.len(), ITEMS);
    no_slower_than("building the index", building, Duration::from_millis(500));

    // 2. A scan. A cashier does this all day and it must not depend on the size
    //    of the catalogue at all.
    let scanning = took("ten thousand scans", || {
        for index in 0..ITEMS {
            let found = replica.by_barcode(&format!("869{index:010}"));
            assert!(found.is_some());
        }
    });
    no_slower_than("ten thousand scans", scanning, Duration::from_millis(200));

    // 3. A keystroke in the search box, on a prefix that matches a great many
    //    items, which is the expensive case: "ri" for rice.
    let searching = took("a thousand searches", || {
        for _ in 0..1_000 {
            let found = replica.search("ri", 50);
            assert!(!found.is_empty());
        }
    });
    no_slower_than("a thousand searches", searching, Duration::from_millis(300));

    // 4. A page of catalogue changes arriving, which rebuilds the index.
    let page = ItemDeltasV1 {
        cursor: 1,
        upserts: items[..PAGE].iter().map(ItemV1::from_domain).collect(),
        tombstones: vec![],
    };
    let applying = took("one page of catalogue changes", || {
        let upserts: Vec<ItemDelta> = page
            .upserts
            .iter()
            .cloned()
            .map(|held| ItemDelta::Upsert(held.into_domain().unwrap()))
            .collect();
        replica.apply(upserts);
    });
    no_slower_than("a page of changes", applying, Duration::from_millis(500));

    // 5. Writing the catalogue down, which is what a checkpoint does, and
    //    reading it back, which is what a cold start does.
    let mut bytes = Vec::new();
    let writing = took("writing the snapshot", || {
        bytes = encode_snapshot(replica.items(), 1).unwrap();
    });
    println!("  the snapshot is {} bytes", bytes.len());
    no_slower_than("writing the snapshot", writing, Duration::from_millis(500));

    let reading = took("reading the snapshot back", || {
        let (read, cursor) =
            decode_snapshot(openpos_core::storage::wire::SNAPSHOT_SCHEMA, &bytes).unwrap();
        assert_eq!(read.len(), ITEMS);
        assert_eq!(cursor, 1);
    });
    no_slower_than("reading the snapshot", reading, Duration::from_millis(500));
}
