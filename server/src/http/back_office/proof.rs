//! Sales a test needs to have happened, built the way a till builds them.
//!
//! Shared by the test modules beside each half of the back office, because two
//! copies would drift and the one used least would be the one that is wrong.

#![cfg(test)]
// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

/// A payload no build can decode, which is one of the things a shop holds a
/// sale for.
pub(super) fn alloc_broken_payload() -> Vec<u8> {
    vec![0xff, 0xff, 0xff, 0xff]
}

/// One real sale, encoded as a till writes it.
///
/// Built through the cart rather than hand-assembled, so the totals check on
/// the server sees the arithmetic it would see from a device.
pub(super) fn sale_payload(id: u128, receipt: &str) -> Vec<u8> {
    use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
    use openpos_core::ids::Ulid;
    use openpos_core::money::{Bp, Milli, Minor};

    let mut cart = Cart::new(CartLimits::unrestricted());
    cart.add_item(
        &openpos_core::replica::Item {
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
            supply: openpos_core::domain::Supply::Standard,
            category: "".into(),
        },
        Milli::ONE,
    )
    .unwrap();
    cart.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(49_450),
        reference: None,
    });
    let mut ticket = cart
        .close(
            Ulid::from_u128(id),
            Ulid::from_u128(4_242),
            1_788_600_000_000,
        )
        .unwrap();
    ticket.receipt_no = Some(receipt.into());
    openpos_core::storage::wire::encode_sale(&openpos_core::storage::wire::sale_commit(
        &ticket,
        Some(1),
        None,
    ))
    .unwrap()
}
