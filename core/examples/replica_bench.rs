//! Hot-path timings for the in-memory catalogue.
//!
//! Native numbers on a development machine. A cheap Android tablet is roughly 5
//! to 15 times slower, so read these as a lower bound and apply the multiplier,
//! the same convention used by the browser harness in `bench/`.
//!
//! Run with: `cargo run --release --example replica_bench`

// A benchmark builds synthetic data and divides elapsed time by an iteration
// count. Plain arithmetic is the right tool here; the workspace bans it in the
// code that handles real money.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::expect_used
)]

use std::hint::black_box;
use std::time::Instant;

use openpos_core::domain::{line_totals, ticket_totals, Discount, LineInput, PriceMode, TicketInput};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::{Item, ItemDelta, Replica, DEFAULT_SEARCH_LIMIT};
use openpos_core::storage::wire::{decode_snapshot, encode_snapshot, SNAPSHOT_SCHEMA};

const CATALOGUE: usize = 20_000;
const LOOKUPS: usize = 200_000;

fn build_catalogue(count: usize) -> Vec<Item> {
    (0..count)
        .map(|i| Item {
            id: Ulid::from_u128(i as u128 + 1),
            code: format!("SKU{i:05}").into(),
            name_en: format!("Item number {i}").into(),
            name_bn: format!("পণ্য নম্বর {i}").into(),
            unit: "Nos".into(),
            price: Minor::new(1_000 + i as i64),
            cost: Minor::new(800 + i as i64),
            vat_rate: Bp::new(1_500).unwrap_or(Bp::ZERO),
            price_mode: PriceMode::Exclusive,
            barcodes: vec![format!("{}", 8_690_000_000_000_u64 + i as u64).into()],
            on_hand: Milli::new(1_000 * (i as i64 % 90)),
            active: true,
        })
        .collect()
}

fn main() {
    println!("catalogue: {CATALOGUE} items\n");

    let items = build_catalogue(CATALOGUE);
    let started = Instant::now();
    let mut replica = Replica::from_items(items);
    let build = started.elapsed();
    println!(
        "  build replica and all indices   {:>9.1} ms   ({} tokens)",
        build.as_secs_f64() * 1_000.0,
        replica.token_count()
    );

    // The scan path: a barcode to an item.
    let barcodes: Vec<String> = (0..LOOKUPS)
        .map(|i| format!("{}", 8_690_000_000_000_u64 + (i % CATALOGUE) as u64))
        .collect();
    let started = Instant::now();
    let mut found = 0_usize;
    for barcode in &barcodes {
        if black_box(replica.by_barcode(barcode)).is_some() {
            found += 1;
        }
    }
    let elapsed = started.elapsed();
    assert_eq!(found, LOOKUPS, "every generated barcode must resolve");
    println!(
        "  barcode lookup                  {:>9.3} us   (per lookup, {LOOKUPS} iterations)",
        elapsed.as_secs_f64() * 1_000_000.0 / LOOKUPS as f64
    );

    // The whole scan-to-line path: lookup plus the money math for one line.
    let started = Instant::now();
    for barcode in barcodes.iter().take(LOOKUPS) {
        if let Some(item) = replica.by_barcode(barcode) {
            let line = LineInput {
                qty: Milli::ONE,
                unit_price: item.price,
                discount: Discount::None,
                vat_rate: item.vat_rate,
                price_mode: item.price_mode,
            };
            black_box(line_totals(&line).ok());
        }
    }
    let elapsed = started.elapsed();
    println!(
        "  scan to priced line             {:>9.3} us   (lookup plus VAT math)",
        elapsed.as_secs_f64() * 1_000_000.0 / LOOKUPS as f64
    );

    // Totalling a full basket, which happens on every line change.
    let ticket = TicketInput {
        lines: (0..30)
            .map(|i| LineInput {
                qty: Milli::new(1_000 + i * 250),
                unit_price: Minor::new(4_300 + i * 17),
                discount: Discount::Rate(Bp::new(500).unwrap_or(Bp::ZERO)),
                vat_rate: Bp::new(1_500).unwrap_or(Bp::ZERO),
                price_mode: PriceMode::Exclusive,
            })
            .collect(),
        ticket_discount: Discount::Rate(Bp::new(250).unwrap_or(Bp::ZERO)),
    };
    let rounds = 20_000;
    let started = Instant::now();
    for _ in 0..rounds {
        black_box(ticket_totals(&ticket).ok());
    }
    let elapsed = started.elapsed();
    println!(
        "  total a 30 line ticket          {:>9.3} us   (with apportioned ticket discount)",
        elapsed.as_secs_f64() * 1_000_000.0 / f64::from(rounds)
    );

    // Search, which runs on every keystroke.
    for query in ["ite", "item num", "সংখ্যা", "sku00001"] {
        let rounds = 2_000;
        let started = Instant::now();
        let mut hits = 0;
        for _ in 0..rounds {
            hits = black_box(replica.search(query, DEFAULT_SEARCH_LIMIT)).len();
        }
        let elapsed = started.elapsed();
        println!(
            "  search {query:<12}             {:>9.3} us   ({hits} hits, capped at {DEFAULT_SEARCH_LIMIT})",
            elapsed.as_secs_f64() * 1_000_000.0 / f64::from(rounds)
        );
    }

    // A sync batch landing while the shop is open.
    let deltas: Vec<ItemDelta> = build_catalogue(1_000)
        .into_iter()
        .map(|mut item| {
            item.price = Minor::new(9_999);
            ItemDelta::Upsert(item)
        })
        .collect();
    let started = Instant::now();
    replica.apply(deltas);
    let elapsed = started.elapsed();
    println!(
        "\n  apply 1,000 deltas and reindex  {:>9.1} ms",
        elapsed.as_secs_f64() * 1_000.0
    );

    // The actual cold start: bytes on disk to an indexed catalogue.
    let fresh = build_catalogue(CATALOGUE);
    let started = Instant::now();
    let encoded = encode_snapshot(&fresh, 0).expect("snapshot encodes");
    let encode = started.elapsed();
    println!(
        "\n  encode snapshot                 {:>9.1} ms   ({:.1} MB, {} bytes an item)",
        encode.as_secs_f64() * 1_000.0,
        encoded.len() as f64 / 1_000_000.0,
        encoded.len() / CATALOGUE
    );

    let started = Instant::now();
    let (decoded, _cursor) = decode_snapshot(SNAPSHOT_SCHEMA, &encoded).expect("snapshot decodes");
    let decode = started.elapsed();
    let started_index = Instant::now();
    let booted = Replica::from_items(decoded);
    let index = started_index.elapsed();
    println!(
        "  decode snapshot                 {:>9.1} ms",
        decode.as_secs_f64() * 1_000.0
    );
    println!(
        "  cold start, bytes to sellable   {:>9.1} ms   ({} items indexed)",
        (decode + index).as_secs_f64() * 1_000.0,
        booted.len()
    );

    // Rough resident cost of the catalogue itself.
    let bytes: usize = replica
        .items()
        .iter()
        .map(|item| {
            std::mem::size_of::<Item>()
                + item.code.len()
                + item.name_en.len()
                + item.name_bn.len()
                + item.unit.len()
                + item.barcodes.iter().map(|b| b.len() + 16).sum::<usize>()
        })
        .sum();
    println!(
        "  catalogue heap, items only      {:>9.1} MB   (indices and tokens on top)",
        bytes as f64 / 1_000_000.0
    );
}
