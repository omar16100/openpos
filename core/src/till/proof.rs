//! The tills a test needs to have on the counter.
//!
//! Shared by the test modules beside each half of the till, because two copies
//! of a fixture drift and the one used least is the one that is wrong.

#![cfg(test)]
// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

use alloc::vec;

use super::*;
use crate::domain::PriceMode;
use crate::money::Bp;
use crate::storage::backend::MemoryBackend;
use crate::storage::wire::ItemV1;

pub(super) const TENANT: u128 = 42;

pub(super) fn terminal() -> TerminalId {
    Ulid::from_u128(7)
}

pub(super) fn item(seed: u128, price: i64) -> Item {
    Item {
        id: Ulid::from_u128(seed),
        code: alloc::format!("SKU{seed:03}").into_boxed_str(),
        name_en: "Rice Miniket 5kg".into(),
        name_bn: "মিনিকেট চাল ৫ কেজি".into(),
        unit: "Nos".into(),
        price: Minor::new(price),
        cost: Minor::new(price / 2),
        vat_rate: Bp::new(1_500).unwrap(),
        price_mode: PriceMode::Exclusive,
        vat_base: crate::domain::VatBase::Discounted,
        barcodes: vec![alloc::format!("869000000{seed:04}").into_boxed_str()],
        on_hand: Milli::new(40_000),
        active: true,
        supply: crate::domain::Supply::Standard,
        category: "".into(),
    }
}

pub(super) fn stocked_till(backend: MemoryBackend) -> Till<MemoryBackend> {
    let (mut till, _) =
        Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
    till.apply_pull(&ItemDeltasV1 {
        cursor: 1,
        upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
        tombstones: vec![],
    })
    .unwrap();
    // Five hundred numbers, which is the block size the design assumes: big
    // enough to cross a long offline day without renewal.
    till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 599))
        .unwrap();
    // A supervisor at the counter, which is what a one-person shop is. The
    // permission checks are live in every test below because of this line.
    till.put_operator(supervisor_operator()).unwrap();
    till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
    till
}

/// Weak on purpose: the shipped round count is deliberately slow, and this
/// suite signs in on every test.
pub(super) const TEST_ROUNDS: u32 = 16;

pub(super) fn supervisor_operator() -> Operator {
    Operator {
        id: Ulid::from_u128(70),
        name: "Owner".into(),
        pin: crate::auth::PinHash::derive("9999", [3; crate::auth::SALT_LEN], TEST_ROUNDS),
        permissions: crate::auth::Permissions::supervisor(),
        active: true,
    }
}

pub(super) fn pay_cash<B: Backend>(till: &mut Till<B>, amount: i64) {
    till.add_tender(
        Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount),
            reference: None,
        },
        0,
    )
    .unwrap();
}

/// A till in a shop that has said what to do about the shelf.
///
/// Three of one item on the shelf, which is what makes the rule visible.
/// Round the shelf once as well, because a device that has not been round
/// says nothing about the shelf at all, and every rule below is about a
/// till that knows what it is talking about.
pub(super) fn a_till_with_three_on_the_shelf(rule: StockRule) -> Till<MemoryBackend> {
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        terminal(),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();
    let mut stocked = item(1, 43_000);
    stocked.on_hand = Milli::new(3_000);
    till.apply_pull(&ItemDeltasV1 {
        cursor: 1,
        upserts: vec![ItemV1::from_domain(&stocked)],
        tombstones: vec![],
    })
    .unwrap();
    till.put_operator(supervisor_operator()).unwrap();
    till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
    till.set_shop(
        crate::receipt::Shop {
            name: alloc::string::String::from("Karim General Store"),
            bin: None,
            address: None,
            phone: None,
        },
        vec![],
        rule,
    )
    .unwrap();
    till.shelf_swept();
    till
}

/// A till holding one receipt number, that rings two sales: the second
/// closes with nothing to number it, which is the situation the whole
/// unnumbered count exists for.
pub(super) fn a_till_whose_numbers_ran_out() -> (Till<MemoryBackend>, Vec<Ulid>) {
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        terminal(),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();
    till.apply_pull(&ItemDeltasV1 {
        cursor: 1,
        upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
        tombstones: vec![],
    })
    .unwrap();
    till.put_operator(supervisor_operator()).unwrap();
    till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
    till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 100))
        .unwrap();
    till.open_shift(Ulid::from_u128(80), Minor::ZERO, 0)
        .unwrap();

    let mut sold = Vec::new();
    for (index, ring) in [900_u128, 901].into_iter().enumerate() {
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 49_450);
        let sale = till
            .checkout(Ulid::from_u128(ring), 1_000 + index as u64)
            .unwrap();
        sold.push(sale.ticket.id);
    }
    assert_eq!(
        till.status().unwrap().unnumbered_sales,
        1,
        "the block ran out on the second"
    );
    (till, sold)
}
