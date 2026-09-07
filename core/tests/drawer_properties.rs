//! Property tests for the money that crosses a counter.
//!
//! The pricing properties protect the arithmetic on the paper. These protect
//! the arithmetic in the drawer, which is the other half and the one a shop
//! counts at the end of the evening.
//!
//! Both of the things checked here were wrong in this codebase until they were
//! written down as properties: a till handed banknotes back for an overpayment
//! on an account, and a drawer expected the note a customer handed over rather
//! than the basket it paid for. Both passed every example test there was,
//! because every example paid the exact amount.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::cart::{Cart, CartError, CartLimits, Tender, TenderKind};
use openpos_core::domain::{PriceMode, Supply, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::Item;
use openpos_core::shift::Shift;
use proptest::prelude::*;

/// A shop's goods: a few taka to a few hundred, taxed or not.
fn item(price_minor: i64, vat_bp: u32) -> Item {
    Item {
        id: Ulid::from_u128(1),
        code: "SKU".into(),
        name_en: "Something".into(),
        name_bn: "কিছু".into(),
        unit: "Nos".into(),
        price: Minor::new(price_minor),
        cost: Minor::new(price_minor / 2),
        vat_rate: Bp::new(vat_bp).unwrap_or(Bp::ZERO),
        price_mode: PriceMode::Exclusive,
        vat_base: VatBase::Discounted,
        barcodes: vec!["8690000000001".into()],
        on_hand: Milli::new(100_000),
        active: true,
        supply: Supply::Standard,
    }
}

/// What a customer hands over: how much cash, and how much on something that
/// cannot hand change back.
fn payment() -> impl Strategy<Value = (i64, i64, u32, i64)> {
    (
        1i64..=200_000i64, // unit price
        1i64..=5_000i64,   // quantity in milli
        prop::sample::select(vec![0u32, 500, 750, 1_500]),
        0i64..=300_000i64, // what goes on the account, before the cash
    )
}

proptest! {
    /// Change never exceeds the cash that was handed over.
    ///
    /// Banknotes come out of the drawer. A promise and a card cannot make them,
    /// and a till that answered otherwise had a cashier hand real money out
    /// against a debt the customer was still carrying.
    #[test]
    fn change_never_exceeds_the_cash_handed_over(
        (price, qty, vat_bp, on_account) in payment(),
        cash in 0i64..=500_000i64,
    ) {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(price, vat_bp), Milli::new(qty)).unwrap();

        // Taken without asking whether they should be, on purpose. Gating the
        // tenders on the rule under test would only prove the rule agrees with
        // itself: the arithmetic has to hold for whatever a screen, a script or
        // a platform that ignored the refusal puts in.
        cart.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(on_account),
            reference: Some("somebody".into()),
        });
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(cash),
            reference: None,
        });

        let change = cart.change_due().unwrap();
        prop_assert!(
            change.get() <= cash,
            "change {} came out of {} in cash",
            change.get(),
            cash
        );
        prop_assert!(change.get() >= 0);
    }

    /// A ticket that closes balances: what was handed over, less the change
    /// handed back, is what the basket came to.
    #[test]
    fn a_closed_ticket_balances(
        (price, qty, vat_bp, on_account) in payment(),
        cash in 0i64..=500_000i64,
    ) {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(price, vat_bp), Milli::new(qty)).unwrap();
        // Unconditionally, for the reason above: what closes and what is
        // refused is the property, not the setup.
        cart.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(on_account),
            reference: Some("somebody".into()),
        });
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(cash),
            reference: None,
        });

        match cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 1_788_600_000_000) {
            Ok(ticket) => {
                let tendered = Minor::sum(ticket.tenders.iter().map(|one| one.amount)).unwrap();
                prop_assert_eq!(
                    tendered.checked_sub(ticket.change).unwrap(),
                    ticket.totals.total,
                    "a closed ticket that does not balance is money nobody can account for"
                );
                prop_assert!(
                    ticket.change.get() <= cash,
                    "a ticket closed handing back {} out of {} in cash",
                    ticket.change.get(),
                    cash
                );
            }
            // The two refusals a sale has: not enough handed over, or too much
            // on something that cannot give change back.
            Err(CartError::Underpaid { .. }) | Err(CartError::ChangeFromAPromise { .. }) => {}
            Err(other) => prop_assert!(false, "unexpected refusal: {:?}", other),
        }
    }

    /// What the drawer should hold is the float plus the cash that stayed.
    ///
    /// Every sale, whatever it was paid with and however much change came back.
    /// This is the figure a person counts against at the end of an evening, and
    /// it was too high by the day's change until a shift was told about it.
    #[test]
    fn the_drawer_holds_the_float_and_what_stayed(
        payments in prop::collection::vec((payment(), 0i64..=500_000i64), 1..6),
        float in 0i64..=200_000i64,
    ) {
        let mut shift = Shift::open(
            Ulid::from_u128(80),
            Ulid::from_u128(7),
            Minor::new(float),
            1_000,
        )
        .unwrap();

        let mut kept = 0_i64;
        for ((price, qty, vat_bp, on_account), cash) in payments {
            let mut cart = Cart::new(CartLimits::unrestricted());
            cart.add_item(&item(price, vat_bp), Milli::new(qty)).unwrap();
            cart.add_tender(Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(on_account),
                reference: Some("somebody".into()),
            });
            cart.add_tender(Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(cash),
                reference: None,
            });
            let Ok(ticket) = cart.close(Ulid::from_u128(9), Ulid::from_u128(7), 1_788_600_000_000)
            else {
                // Underpaid or over on a promise: no sale, so nothing crossed
                // the drawer.
                continue;
            };
            let in_cash: i64 = ticket
                .tenders
                .iter()
                .filter(|one| one.kind == TenderKind::Cash)
                .map(|one| one.amount.get())
                .sum();
            kept += in_cash - ticket.change.get();
            shift.record_sale(&ticket.tenders, ticket.change).unwrap();
        }

        prop_assert_eq!(
            shift.expected_cash().unwrap().get(),
            float + kept,
            "the drawer expects the float and the cash that stayed in it"
        );
    }
}

/// A check on the checks: the properties above are worth nothing if every
/// generated basket is refused before it reaches the arithmetic.
#[test]
fn the_properties_reach_the_cases_they_are_about() {
    let mut closed = 0_u32;
    let mut with_change = 0_u32;
    let mut refused_on_a_promise = 0_u32;

    for step in 0..400_u32 {
        let price = 1_000 + i64::from(step) * 37;
        let qty = 1_000 + i64::from(step % 5) * 1_000;
        let on_account = i64::from(step % 7) * 20_000;
        let cash = i64::from(step % 11) * 30_000;

        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(price, 1_500), Milli::new(qty)).unwrap();
        for tender in [
            Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(on_account),
                reference: Some("somebody".into()),
            },
            Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(cash),
                reference: None,
            },
        ] {
            if cart.would_overpay(&tender).is_err() {
                refused_on_a_promise += 1;
            }
            cart.add_tender(tender);
        }
        if let Ok(ticket) = cart.close(Ulid::from_u128(9), Ulid::from_u128(7), 1_788_600_000_000) {
            closed += 1;
            if ticket.change.get() > 0 {
                with_change += 1;
            }
        }
    }

    assert!(closed > 50, "only {closed} baskets closed at all");
    assert!(
        with_change > 20,
        "only {with_change} gave change, so the change properties prove nothing"
    );
    assert!(
        refused_on_a_promise > 10,
        "only {refused_on_a_promise} overpayments were refused, so that path is untested"
    );
}
