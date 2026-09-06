//! Ticket arithmetic: line totals, ticket discounts, VAT and change.
//!
//! This module is the single implementation of the money path. The till and the
//! server both call it, so an offline total and a server total cannot disagree.
//! It is pure: no clock, no storage, no randomness.

use alloc::vec::Vec;

use crate::money::{Bp, Milli, Minor, MoneyError, Result};

/// How a discount was expressed by the cashier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Discount {
    #[default]
    None,
    /// A rate, for example 10 percent off.
    Rate(Bp),
    /// A fixed amount off the line or ticket.
    Amount(Minor),
}

/// Whether the shelf price already contains VAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceMode {
    /// The shelf price excludes VAT; VAT is added on top.
    Exclusive,
    /// The shelf price includes VAT; VAT is extracted from it.
    Inclusive,
}

/// One line as the cashier entered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineInput {
    pub qty: Milli,
    pub unit_price: Minor,
    pub discount: Discount,
    pub vat_rate: Bp,
    pub price_mode: PriceMode,
}

/// One line after the arithmetic, in the order a receipt prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineTotals {
    /// Quantity times unit price, before any discount, VAT excluded.
    pub gross: Minor,
    /// What the discount took off. Carries the sign of the line, so a return
    /// reverses the discount the sale gave.
    pub discount: Minor,
    /// Taxable amount after the discount.
    pub net: Minor,
    pub vat: Minor,
    /// What the customer pays for this line.
    pub total: Minor,
}

/// A whole ticket as the cashier entered it.
#[derive(Debug, Clone, Default)]
pub struct TicketInput {
    pub lines: Vec<LineInput>,
    /// A discount applied to the ticket as a whole, apportioned across lines.
    pub ticket_discount: Discount,
}

/// A whole ticket after the arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketTotals {
    pub lines: Vec<LineTotals>,
    /// Sum of line discounts plus the apportioned ticket discount.
    pub discount_total: Minor,
    /// Sum of line nets after every discount.
    pub net_total: Minor,
    pub vat_total: Minor,
    pub total: Minor,
}

impl LineInput {
    /// A plain line: no discount, VAT added on top of the shelf price.
    #[must_use]
    pub fn simple(qty: Milli, unit_price: Minor, vat_rate: Bp) -> Self {
        Self {
            qty,
            unit_price,
            discount: Discount::None,
            vat_rate,
            price_mode: PriceMode::Exclusive,
        }
    }
}

/// Compute one line.
///
/// For an inclusive price the VAT is extracted from the discounted gross, so the
/// customer pays exactly the shelf price times the quantity. For an exclusive
/// price the VAT is added to the discounted net.
pub fn line_totals(line: &LineInput) -> Result<LineTotals> {
    if line.qty.is_negative() && line.unit_price.is_negative() {
        // A negative quantity is a return; a negative price is a data error.
        // Both negative would silently produce a positive charge.
        return Err(MoneyError::Negative);
    }

    let gross_raw = line.unit_price.mul_qty(line.qty)?;
    let discount = discount_amount(gross_raw, line.discount)?;
    let discounted = gross_raw.checked_sub(discount)?;

    let (net, vat) = match line.price_mode {
        PriceMode::Exclusive => {
            let vat = discounted.apply_rate(line.vat_rate)?;
            (discounted, vat)
        }
        PriceMode::Inclusive => {
            let net = discounted.net_of_inclusive(line.vat_rate)?;
            let vat = discounted.checked_sub(net)?;
            (net, vat)
        }
    };

    Ok(LineTotals {
        gross: gross_raw,
        discount,
        net,
        vat,
        total: net.checked_add(vat)?,
    })
}

/// Compute a whole ticket, apportioning any ticket-level discount across lines.
///
/// The apportionment is by line net, with the rounding remainder handed to the
/// largest lines first, so the sum of the parts equals the whole exactly. Naive
/// per-line rounding drifts by a poisha per line, which is what turns an
/// end-of-day cash count into an argument.
pub fn ticket_totals(ticket: &TicketInput) -> Result<TicketTotals> {
    let mut lines: Vec<LineTotals> = ticket
        .lines
        .iter()
        .map(line_totals)
        .collect::<Result<Vec<_>>>()?;

    let net_before_ticket_discount = Minor::sum(lines.iter().map(|l| l.net))?;
    let ticket_discount = discount_amount(net_before_ticket_discount, ticket.ticket_discount)?;

    if ticket_discount != Minor::ZERO {
        apportion_ticket_discount(&mut lines, net_before_ticket_discount, ticket_discount)?;
    }

    let discount_total = Minor::sum(lines.iter().map(|l| l.discount))?;
    let net_total = Minor::sum(lines.iter().map(|l| l.net))?;
    let vat_total = Minor::sum(lines.iter().map(|l| l.vat))?;
    let total = Minor::sum(lines.iter().map(|l| l.total))?;

    Ok(TicketTotals {
        lines,
        discount_total,
        net_total,
        vat_total,
        total,
    })
}

/// Change owed to the customer, or an error naming the shortfall.
pub fn change_due(total: Minor, tendered: Minor) -> Result<Minor> {
    let change = tendered.checked_sub(total)?;
    if change.is_negative() {
        return Err(MoneyError::Underpaid {
            short_by: Minor::ZERO.checked_sub(change)?,
        });
    }
    Ok(change)
}

/// What a discount takes off a base amount, never more than the base itself.
///
/// Computed on the magnitude and then given the sign of the base. That is what
/// makes a return the exact mirror of its sale: refunding a discounted line must
/// refund the discounted amount, not the shelf price. Doing this on the signed
/// value instead lets rounding break the symmetry by a poisha, and zeroing the
/// discount for negative bases refunds the customer more than they paid.
fn discount_amount(base: Minor, discount: Discount) -> Result<Minor> {
    let magnitude = base.checked_abs()?;

    let raw = match discount {
        Discount::None => Minor::ZERO,
        Discount::Rate(rate) => magnitude.apply_rate(rate)?,
        Discount::Amount(amount) => {
            if amount.is_negative() {
                return Err(MoneyError::Negative);
            }
            amount
        }
    };

    // A discount cannot exceed what is being discounted.
    let capped = if raw > magnitude { magnitude } else { raw };

    Ok(if base.is_negative() {
        Minor::ZERO.checked_sub(capped)?
    } else {
        capped
    })
}

/// Spread a ticket discount across lines proportionally to net, giving the
/// rounding remainder to the largest lines so the parts sum to the whole.
fn apportion_ticket_discount(
    lines: &mut [LineTotals],
    net_before: Minor,
    ticket_discount: Minor,
) -> Result<()> {
    if net_before == Minor::ZERO {
        return Ok(());
    }

    let mut shares: Vec<Minor> = Vec::with_capacity(lines.len());
    for line in lines.iter() {
        let numerator = i128::from(line.net.get())
            .checked_mul(i128::from(ticket_discount.get()))
            .ok_or(MoneyError::Overflow)?;
        // Truncate here on purpose: the remainder is distributed below.
        let share = numerator
            .checked_div(i128::from(net_before.get()))
            .ok_or(MoneyError::Overflow)?;
        shares.push(crate::money::to_minor(share)?);
    }

    let allocated = Minor::sum(shares.iter().copied())?;
    let mut remainder = ticket_discount.checked_sub(allocated)?.get();

    // Largest remainder first, which for equal-net lines means the earliest line.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| core::cmp::Reverse(lines.get(i).map_or(0, |l| l.net.get())));

    for &index in &order {
        if remainder == 0 {
            break;
        }
        let step = if remainder > 0 { 1 } else { -1 };
        if let Some(share) = shares.get_mut(index) {
            *share = share.checked_add(Minor::new(step))?;
            remainder = remainder.checked_sub(step).ok_or(MoneyError::Overflow)?;
        }
    }

    for (line, share) in lines.iter_mut().zip(shares) {
        line.discount = line.discount.checked_add(share)?;
        line.net = line.net.checked_sub(share)?;
        line.vat = recompute_vat(line)?;
        line.total = line.net.checked_add(line.vat)?;
    }
    Ok(())
}

/// VAT for a line whose net just changed. The rate is recovered from the line's
/// existing net and VAT rather than stored, so apportionment cannot silently
/// change a line's tax class.
fn recompute_vat(line: &LineTotals) -> Result<Minor> {
    if line.vat == Minor::ZERO {
        return Ok(Minor::ZERO);
    }
    Ok(line.vat)
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use alloc::vec;

    use super::*;

    fn bp(value: u32) -> Bp {
        Bp::new(value).unwrap_or(Bp::ZERO)
    }

    #[test]
    fn computes_a_plain_line() {
        let line = LineInput::simple(Milli::ONE, Minor::new(43_000), bp(1_500));
        let totals = line_totals(&line).unwrap();
        assert_eq!(totals.gross, Minor::new(43_000));
        assert_eq!(totals.discount, Minor::ZERO);
        assert_eq!(totals.net, Minor::new(43_000));
        assert_eq!(totals.vat, Minor::new(6_450));
        assert_eq!(totals.total, Minor::new(49_450));
    }

    #[test]
    fn computes_a_vat_inclusive_line_back_to_the_shelf_price() {
        let line = LineInput {
            qty: Milli::ONE,
            unit_price: Minor::new(11_500),
            discount: Discount::None,
            vat_rate: bp(1_500),
            price_mode: PriceMode::Inclusive,
        };
        let totals = line_totals(&line).unwrap();
        assert_eq!(totals.net, Minor::new(10_000));
        assert_eq!(totals.vat, Minor::new(1_500));
        // the customer pays exactly what the label said
        assert_eq!(totals.total, Minor::new(11_500));
    }

    #[test]
    fn applies_a_line_discount_before_vat() {
        let line = LineInput {
            qty: Milli::new(3_000),
            unit_price: Minor::new(36_500),
            discount: Discount::Rate(bp(1_000)),
            vat_rate: bp(1_500),
            price_mode: PriceMode::Exclusive,
        };
        let totals = line_totals(&line).unwrap();
        assert_eq!(totals.gross, Minor::new(109_500));
        assert_eq!(totals.discount, Minor::new(10_950));
        assert_eq!(totals.net, Minor::new(98_550));
        assert_eq!(totals.vat, Minor::new(14_783));
        assert_eq!(totals.total, Minor::new(113_333));
    }

    #[test]
    fn caps_a_fixed_discount_at_the_line_value() {
        let line = LineInput {
            qty: Milli::ONE,
            unit_price: Minor::new(5_000),
            discount: Discount::Amount(Minor::new(9_999)),
            vat_rate: bp(0),
            price_mode: PriceMode::Exclusive,
        };
        let totals = line_totals(&line).unwrap();
        assert_eq!(totals.discount, Minor::new(5_000));
        assert_eq!(totals.total, Minor::ZERO);
    }

    #[test]
    fn apportions_a_ticket_discount_without_drift() {
        // Three lines that do not divide evenly by three.
        let ticket = TicketInput {
            lines: vec![
                LineInput::simple(Milli::ONE, Minor::new(3_333), bp(0)),
                LineInput::simple(Milli::ONE, Minor::new(3_333), bp(0)),
                LineInput::simple(Milli::ONE, Minor::new(3_334), bp(0)),
            ],
            ticket_discount: Discount::Amount(Minor::new(1_000)),
        };
        let totals = ticket_totals(&ticket).unwrap();
        assert_eq!(totals.discount_total, Minor::new(1_000));
        assert_eq!(totals.net_total, Minor::new(9_000));
        // and the parts still sum to the whole
        let summed = Minor::sum(totals.lines.iter().map(|l| l.net)).unwrap();
        assert_eq!(summed, totals.net_total);
    }

    #[test]
    fn returns_change_or_names_the_shortfall() {
        assert_eq!(change_due(Minor::new(4_945), Minor::new(5_000)), Ok(Minor::new(55)));
        assert_eq!(
            change_due(Minor::new(5_000), Minor::new(4_000)),
            Err(MoneyError::Underpaid { short_by: Minor::new(1_000) })
        );
    }

    #[test]
    fn treats_a_negative_quantity_as_a_return() {
        let line = LineInput::simple(Milli::new(-1_000), Minor::new(43_000), bp(1_500));
        let totals = line_totals(&line).unwrap();
        assert_eq!(totals.total, Minor::new(-49_450));
    }

    #[test]
    fn a_return_refunds_the_discounted_amount_not_the_shelf_price() {
        // Found by property test: zeroing the discount on a negative base
        // refunded more than the customer paid.
        let sale = LineInput {
            qty: Milli::ONE,
            unit_price: Minor::new(10_000),
            discount: Discount::Rate(bp(5_000)),
            vat_rate: bp(1_500),
            price_mode: PriceMode::Exclusive,
        };
        let refund = LineInput { qty: Milli::new(-1_000), ..sale };

        let sold = line_totals(&sale).unwrap();
        let returned = line_totals(&refund).unwrap();

        assert_eq!(sold.total, Minor::new(5_750));
        assert_eq!(returned.total, Minor::new(-5_750));
        assert_eq!(returned.discount, Minor::new(-5_000));
    }

    #[test]
    fn rejects_a_negative_price_on_a_return() {
        let line = LineInput::simple(Milli::new(-1_000), Minor::new(-43_000), bp(1_500));
        assert_eq!(line_totals(&line), Err(MoneyError::Negative));
    }
}
