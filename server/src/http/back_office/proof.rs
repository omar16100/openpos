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
    })
        .expect("money moving the way this ticket runs");
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

/// One sale with two lines on it, for the compatibility branches.
///
/// A field appended to a *line* lands between the lines when there are two of
/// them, which is where a decoder reads it as the start of the next one. With a
/// single line it lands in the middle of the sale instead, which is also wrong
/// and is a different wrong: both are worth having, and this is the one the
/// two-entry rule is about.
pub(super) fn sale_payload_of_two_lines(id: u128, receipt: &str) -> Vec<u8> {
    use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
    use openpos_core::ids::Ulid;
    use openpos_core::money::{Bp, Milli, Minor};

    let item = |number: u128, code: &str, name: &str, price: i64| openpos_core::replica::Item {
        id: Ulid::from_u128(number),
        code: code.into(),
        name_en: name.into(),
        name_bn: name.into(),
        unit: "Nos".into(),
        price: Minor::new(price),
        cost: Minor::new(price / 2),
        vat_rate: Bp::new(1_500).unwrap(),
        price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
        vat_base: openpos_core::domain::pricing::VatBase::Discounted,
        barcodes: vec![],
        on_hand: Milli::new(40_000),
        active: true,
        supply: openpos_core::domain::Supply::Standard,
        category: "".into(),
    };

    let mut cart = Cart::new(CartLimits::unrestricted());
    cart.add_item(&item(1, "RICE5", "Rice Miniket 5kg", 43_000), Milli::ONE)
        .unwrap();
    cart.add_item(&item(2, "OIL1", "Soyabean Oil 1L", 19_000), Milli::new(2_000))
        .unwrap();
    cart.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: cart.totals().unwrap().total,
        reference: None,
    })
    .expect("money moving the way this ticket runs");
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
