//! A till holding money the shop has not got.
//!
//! Two ways a device ends up in this state, and until now neither had a way out.
//!
//! One is corruption: a byte goes bad in the middle of the critical log, and
//! recovery truncates from there, which throws away every frame after it as
//! well. Those may be sales that were rung, paid for and printed. The bytes are
//! copied into the salvage blob at the last moment anybody could have recovered
//! them, and then nothing read that blob, which made the copy a gesture.
//!
//! The other is a till whose terminal the shop deleted, or one that has to be
//! re-enrolled as a different terminal. It cannot push and cannot be adopted,
//! and its outbox is the only record of goods that left the shop.
//!
//! What this insists on: what the device is still holding can be listed, whole,
//! by a person standing at it, and what came out of the torn part is marked as
//! such rather than mixed in with what is merely unsent.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::domain::Supply;
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::Item;
use openpos_core::storage::backend::{Backend, Blob, MemoryBackend};
use openpos_core::storage::frame::{self, Store};
use openpos_core::till::Till;

const TENANT: u128 = 42;
const TERMINAL: u128 = 7;

fn item() -> Item {
    Item {
        id: Ulid::from_u128(1),
        code: "RICE5".into(),
        name_en: "Rice Miniket 5kg".into(),
        name_bn: "মিনিকেট চাল ৫ কেজি".into(),
        unit: "Nos".into(),
        price: Minor::new(43_000),
        cost: Minor::new(38_000),
        vat_rate: Bp::new(1_500).unwrap(),
        price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
        vat_base: openpos_core::domain::pricing::VatBase::Discounted,
        barcodes: vec!["8690000000001".into()],
        on_hand: Milli::new(40_000),
        active: true,
        supply: Supply::Standard,
        category: "".into(),
    }
}

fn till(backend: MemoryBackend) -> Till<MemoryBackend> {
    let (mut till, _) = Till::open(
        backend,
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .expect("a till opens");
    till.apply_pull(&openpos_core::storage::wire::ItemDeltasV1 {
        cursor: 1,
        upserts: vec![openpos_core::storage::wire::ItemV1::from_domain(&item())],
        tombstones: vec![],
    })
    .expect("a catalogue of one");
    till
}

fn sell(till: &mut Till<MemoryBackend>, id: u128) {
    till.scan("8690000000001", Milli::ONE).unwrap();
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(49_450),
        reference: None,
    }, 0)
    .unwrap();
    till.checkout(Ulid::from_u128(id), 1_788_600_000_000)
        .unwrap();
}

#[test]
fn a_torn_log_gives_back_the_sales_it_cut_off() {
    let mut backend = MemoryBackend::new();
    {
        let mut selling = till(backend.clone());
        sell(&mut selling, 900);
        sell(&mut selling, 901);
        sell(&mut selling, 902);
        backend = selling.journal().backend().clone();
    }

    // A byte goes bad inside the second sale. Recovery truncates from there,
    // which takes the third sale with it: it is whole, and it is after the tear.
    let mut bytes = backend.read_log(Store::Critical).unwrap();
    let frames = frame::scan(&bytes);
    assert_eq!(frames.frames.len(), 3);
    let second = frames.frames[0].len;
    bytes[second + 40] ^= 0xFF;
    backend.truncate_log(Store::Critical, 0).unwrap();
    backend.append_log(Store::Critical, &bytes).unwrap();
    backend.flush().unwrap();

    let recovered = till(backend);

    // The first sale is still in the outbox and will sync by itself.
    assert_eq!(recovered.pending_sales(10).unwrap().len(), 1);

    // And the third is readable in the salvage blob, which is the whole reason
    // those bytes were copied aside. The second is not: its own bytes are the
    // damaged ones, and no amount of scanning invents them back.
    let carried = recovered.carried_out(50).unwrap();
    assert_eq!(carried.len(), 2);
    assert_eq!(carried[0].id, Ulid::from_u128(900));
    assert!(!carried[0].salvaged, "this one is merely unsent");
    assert_eq!(carried[1].id, Ulid::from_u128(902));
    assert!(
        carried[1].salvaged,
        "read back out of a torn log, and a person should look at it"
    );
    assert_eq!(carried[1].total_minor, 49_450);
}

#[test]
fn a_device_holding_nothing_carries_nothing() {
    let empty = till(MemoryBackend::new());
    assert!(empty.carried_out(50).unwrap().is_empty());
}

#[test]
fn salvaged_bytes_from_another_terminal_are_not_this_ones_sales() {
    let mut backend = MemoryBackend::new();
    {
        let mut selling = till(backend.clone());
        sell(&mut selling, 900);
        backend = selling.journal().backend().clone();
    }

    // A blob from a cloned tablet image, or a file copied by somebody trying to
    // help. Its frames verify: they were written properly, by another device.
    // Adopting them here would sell another till's sales under this one's name.
    let mut foreign = MemoryBackend::new();
    {
        let (mut other, _) = Till::open(
            foreign.clone(),
            TENANT,
            Ulid::from_u128(999),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        other
            .apply_pull(&openpos_core::storage::wire::ItemDeltasV1 {
                cursor: 1,
                upserts: vec![openpos_core::storage::wire::ItemV1::from_domain(&item())],
                tombstones: vec![],
            })
            .unwrap();
        other.scan("8690000000001", Milli::ONE).unwrap();
        other
            .add_tender(Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(49_450),
                reference: None,
            }, 0)
            .unwrap();
        other
            .checkout(Ulid::from_u128(901), 1_788_600_000_000)
            .unwrap();
        foreign = other.journal().backend().clone();
    }
    let theirs = foreign.read_log(Store::Critical).unwrap();
    backend.write_blob(Blob::Salvage, &theirs).unwrap();
    backend.flush().unwrap();

    let recovered = till(backend);
    let carried = recovered.carried_out(50).unwrap();
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].id, Ulid::from_u128(900), "only its own");
}
