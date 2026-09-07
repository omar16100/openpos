//! What a sale costs to write down before the cashier is told it is done.
//!
//! The one number that decides whether a queue moves. A till commits the whole
//! sale in one frame and flushes it before printing anything, because a receipt
//! is a promise that the sale survives the power going out, and until there was
//! a file-backed store there was nothing native to measure: the browser's OPFS
//! is measured in a browser, and memory measures the arithmetic and nothing
//! else.
//!
//! ```text
//! cargo run --release -p openpos-bindings --example flush_cost -- 200
//! ```
//!
//! What it does not measure: a cheap Android tablet's flash, which is where
//! this figure actually matters and is slower than any desk. The shape of the
//! answer is what transfers, not the milliseconds.

// A tool run by hand. It panics on anything unexpected on purpose: there is
// nobody to hand an error to, and a stack trace is more use here than a message.
#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss
)]

use std::time::Instant;

use openpos_bindings::files::FileBackend;
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::domain::{PriceMode, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::Item;
use openpos_core::storage::backend::{Backend, MemoryBackend};
use openpos_core::till::Till;

fn main() {
    let wanted: usize = std::env::args()
        .nth(1)
        .and_then(|given| given.parse().ok())
        .unwrap_or(200);

    let home = std::env::temp_dir().join(format!(
        "openpos-flush-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));

    let on_files = ring(
        FileBackend::open(&home).expect("a store opens"),
        wanted,
        "on files, flushed every sale",
    );
    let in_memory = ring(MemoryBackend::new(), wanted, "in memory, nothing flushed");

    println!();
    println!(
        "the storage costs {:.2} ms a sale on this machine: {:.3} less {:.3}",
        on_files - in_memory,
        on_files,
        in_memory
    );
    println!(
        "a queue moves at one customer every few seconds, so what matters is that this is \
         milliseconds and not tenths of a second"
    );
    println!("measured on a desk, not on the cheap tablet where it decides anything");

    std::fs::remove_dir_all(&home).ok();
}

/// Ring sales one after another, as a counter does, and say what each cost.
fn ring<B: Backend>(backend: B, wanted: usize, what: &str) -> f64 {
    let (mut till, _) = Till::open(
        backend,
        42,
        Ulid::from_u128(7),
        1,
        CartLimits::unrestricted(),
    )
    .expect("a till opens");
    till.apply_pull(&openpos_core::storage::wire::ItemDeltasV1 {
        cursor: 1,
        upserts: alloc_items(),
        tombstones: alloc::vec::Vec::new(),
    })
    .expect("a catalogue");

    let mut slowest = 0.0_f64;
    let began = Instant::now();
    for index in 0..wanted {
        till.scan("8690000000001", Milli::ONE).expect("a scan");
        let total = till.totals().expect("totals").total;
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: total,
            reference: None,
        })
        .expect("cash");
        let one = Instant::now();
        till.checkout(
            Ulid::from_u128(900_000 + index as u128),
            1_788_600_000_000 + index as u64,
        )
        .expect("a sale");
        slowest = slowest.max(one.elapsed().as_secs_f64() * 1_000.0);
    }
    let each = began.elapsed().as_secs_f64() * 1_000.0 / wanted as f64;
    // Three decimals, because the arithmetic on its own is microseconds and a
    // bare 0.00 reads as a measurement that did not run.
    println!("{what}: {each:.3} ms a sale, slowest {slowest:.1} ms");
    each
}

extern crate alloc;

fn alloc_items() -> alloc::vec::Vec<openpos_core::storage::wire::ItemV1> {
    alloc::vec![openpos_core::storage::wire::ItemV1::from_domain(&Item {
        id: Ulid::from_u128(1),
        code: "RICE5".into(),
        name_en: "Rice Miniket 5kg".into(),
        name_bn: "মিনিকেট চাল ৫ কেজি".into(),
        unit: "Nos".into(),
        price: Minor::new(43_000),
        cost: Minor::new(38_000),
        vat_rate: Bp::new(1_500).expect("a rate"),
        price_mode: PriceMode::Exclusive,
        vat_base: VatBase::Discounted,
        barcodes: alloc::vec!["8690000000001".into()],
        on_hand: Milli::new(1_000_000),
        active: true,
    })]
}
