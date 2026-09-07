//! Property tests for the money path.
//!
//! These protect the invariants a shopkeeper checks with a pen: the parts sum to
//! the whole, a VAT-inclusive line charges exactly the shelf price, and a return
//! is the exact negative of the sale it reverses.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects
)]

use openpos_core::domain::{Discount, LineInput, PriceMode, Supply, TicketInput, VatBase, change_due, line_totals, ticket_totals};
use openpos_core::money::{Bp, Milli, Minor};
use proptest::prelude::*;

/// Realistic retail ranges. Prices up to 100,000 minor units (1,000 currency
/// units), quantities up to 100 units, so the tests exercise the arithmetic
/// rather than the overflow guards, which have their own unit tests.
fn line_strategy() -> impl Strategy<Value = LineInput> {
    (
        1i64..=100_000i64,                                 // qty in milli-units
        0i64..=10_000_000i64,                              // unit price in minor units
        0u32..=10_000u32,                                  // discount rate in basis points
        prop::sample::select(vec![0u32, 500, 750, 1_500]), // plausible VAT rates
        any::<bool>(),
    )
        .prop_map(|(qty, price, discount_bp, vat_bp, inclusive)| LineInput {
            // Generated lines use the ordinary treatment, which is what the
            // properties below are about. The other base has its own tests.
            vat_base: VatBase::Discounted,
            qty: Milli::new(qty),
            unit_price: Minor::new(price),
            discount: Discount::Rate(Bp::new(discount_bp).unwrap_or(Bp::ZERO)),
            vat_rate: Bp::vat(vat_bp).unwrap_or(Bp::ZERO),
            price_mode: if inclusive {
                PriceMode::Inclusive
            } else {
                PriceMode::Exclusive
            },
            supply: Supply::Standard,
        })
}

proptest! {
    /// Every line balances: what the customer pays is the taxable amount plus tax.
    #[test]
    fn line_total_is_net_plus_vat(line in line_strategy()) {
        let totals = line_totals(&line).expect("realistic input must not overflow");
        prop_assert_eq!(totals.total, totals.net.checked_add(totals.vat).expect("no overflow"));
    }

    /// A discount never exceeds the line it discounts, so a line can reach zero
    /// but never turns into a payout.
    #[test]
    fn line_discount_never_exceeds_gross(line in line_strategy()) {
        let totals = line_totals(&line).expect("realistic input must not overflow");
        // The discount carries the line's sign, so compare magnitudes.
        prop_assert!(totals.discount.get().abs() <= totals.gross.get().abs());
        prop_assert_eq!(totals.discount.is_negative(), totals.gross.is_negative() && totals.discount.get() != 0);
        prop_assert!(!totals.total.is_negative());
    }

    /// A VAT-inclusive line charges exactly the shelf price after discount. If
    /// this drifts, the price on the label stops matching the price at the till.
    #[test]
    fn inclusive_pricing_charges_the_shelf_price(
        qty in 1i64..=50_000i64,
        price in 1i64..=1_000_000i64,
        vat_bp in prop::sample::select(vec![0u32, 500, 750, 1_500]),
    ) {
        let line = LineInput {
            vat_base: VatBase::Discounted,
            qty: Milli::new(qty),
            unit_price: Minor::new(price),
            discount: Discount::None,
            vat_rate: Bp::vat(vat_bp).unwrap_or(Bp::ZERO),
            price_mode: PriceMode::Inclusive,
            supply: Supply::Standard,
        };
        let totals = line_totals(&line).expect("realistic input must not overflow");
        prop_assert_eq!(totals.total, totals.gross);
    }

    /// The parts sum to the whole across a whole ticket, with or without a
    /// ticket-level discount. Naive apportionment drifts a poisha per line, and
    /// that drift is what makes an end-of-day cash count irreconcilable.
    #[test]
    fn ticket_totals_are_the_sum_of_their_lines(
        lines in prop::collection::vec(line_strategy(), 1..12),
        discount_bp in 0u32..=10_000u32,
    ) {
        let ticket = TicketInput {
            lines,
            ticket_discount: Discount::Rate(Bp::new(discount_bp).unwrap_or(Bp::ZERO)),
        };
        let totals = ticket_totals(&ticket).expect("realistic input must not overflow");

        let net: i64 = totals.lines.iter().map(|l| l.net.get()).sum();
        let vat: i64 = totals.lines.iter().map(|l| l.vat.get()).sum();
        let total: i64 = totals.lines.iter().map(|l| l.total.get()).sum();
        let discount: i64 = totals.lines.iter().map(|l| l.discount.get()).sum();

        prop_assert_eq!(net, totals.net_total.get());
        prop_assert_eq!(vat, totals.vat_total.get());
        prop_assert_eq!(total, totals.total.get());
        prop_assert_eq!(discount, totals.discount_total.get());
    }

    /// A fixed ticket discount is applied in full, to the poisha, however many
    /// lines it is spread across.
    #[test]
    fn a_fixed_ticket_discount_is_applied_exactly(
        lines in prop::collection::vec(line_strategy(), 1..12),
        requested in 1i64..=50_000i64,
    ) {
        let plain: Vec<LineInput> = lines
            .into_iter()
            .map(|l| LineInput { discount: Discount::None, ..l })
            .collect();
        let undiscounted = ticket_totals(&TicketInput {
            lines: plain.clone(),
            ticket_discount: Discount::None,
        })
        .expect("realistic input must not overflow");

        // Only meaningful when the ticket is worth at least the discount.
        prop_assume!(undiscounted.net_total.get() >= requested);

        let discounted = ticket_totals(&TicketInput {
            lines: plain,
            ticket_discount: Discount::Amount(Minor::new(requested)),
        })
        .expect("realistic input must not overflow");

        prop_assert_eq!(discounted.discount_total.get(), requested);
        prop_assert_eq!(
            discounted.net_total.get(),
            undiscounted.net_total.get() - requested
        );
    }

    /// A return is the exact negative of the sale it reverses, which is why the
    /// rounding rule is symmetric about zero.
    #[test]
    fn a_return_mirrors_its_sale(line in line_strategy()) {
        let sale = line_totals(&line).expect("realistic input must not overflow");
        let refund_line = LineInput { qty: Milli::new(-line.qty.get()), ..line };
        let refund = line_totals(&refund_line).expect("realistic input must not overflow");

        prop_assert_eq!(refund.net.get(), -sale.net.get());
        prop_assert_eq!(refund.vat.get(), -sale.vat.get());
        prop_assert_eq!(refund.total.get(), -sale.total.get());
    }

    /// Change is exact, and paying too little is an error rather than negative
    /// change handed to the customer.
    #[test]
    fn change_is_exact_or_refused(total in 0i64..=1_000_000i64, tendered in 0i64..=1_000_000i64) {
        let result = change_due(Minor::new(total), Minor::new(tendered));
        if tendered >= total {
            prop_assert_eq!(result.map(Minor::get), Ok(tendered - total));
        } else {
            prop_assert!(result.is_err());
        }
    }

    /// Every line's VAT is the rate applied to the net it is charged on, to the
    /// poisha where the arithmetic allows it and to one poisha where it cannot.
    ///
    /// The property the old apportionment quietly broke: it reduced a line's net
    /// and left the VAT computed on the amount before the discount, so a
    /// ticket-discounted sale overcharged the customer and over-declared the tax.
    ///
    /// An inclusive line is the exception, and not because of sloppiness. There
    /// the shelf price is the promise: the customer pays what the label says, and
    /// the tax is a share of it. For some prices and rates no whole number of
    /// poisha satisfies both that promise and the rate exactly, because adding a
    /// poisha to the net can add two to the total. Proptest found one: 4,000,567.99
    /// at 7.5 percent has no split at all, and the nearest are a poisha either
    /// side. When those two cannot both hold, the price on the label wins, which
    /// is the one of them a customer can see.
    #[test]
    fn vat_always_matches_the_net_it_is_charged_on(
        lines in prop::collection::vec(line_strategy(), 1..6),
        discount_bp in 0u32..=5_000u32,
    ) {
        let sent = lines.clone();
        let totals = ticket_totals(&TicketInput {
            lines,
            ticket_discount: Discount::Rate(Bp::new(discount_bp).unwrap_or(Bp::ZERO)),
        })
        .expect("realistic input must not overflow");

        for (line, input) in totals.lines.iter().zip(&sent) {
            let by_rate = line
                .net
                .apply_rate(line.vat_rate)
                .expect("rate application must not overflow");
            prop_assert_eq!(line.total, line.net.checked_add(line.vat).unwrap());

            // A ticket discount moves the net, and the VAT is recomputed from
            // the rate when it does, whichever way the price was quoted.
            let quoted_inclusive =
                input.price_mode == PriceMode::Inclusive && discount_bp == 0;
            if quoted_inclusive {
                prop_assert!(
                    (line.vat.get() - by_rate.get()).abs() <= 1,
                    "a poisha at most, and only where no split can do both"
                );
                // And the promise that costs it: the customer pays the price on
                // the label, less whatever was taken off this line.
                prop_assert_eq!(line.total, line.gross.checked_sub(line.discount).unwrap());
            } else {
                prop_assert_eq!(line.vat, by_rate);
            }
        }
    }
}
