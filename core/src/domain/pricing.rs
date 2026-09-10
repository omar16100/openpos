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

/// Which amount tax is charged on.
///
/// Two shops can both be right about this, which is why it is a setting on the
/// item rather than a rule in the arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VatBase {
    /// Tax follows the money. A discount reduces the consideration, so it
    /// reduces the taxable amount and the tax with it. The ordinary treatment,
    /// and what most goods want.
    #[default]
    Discounted,
    /// Tax is charged on the undiscounted price and does not move when a
    /// discount is given, so the shop funds the whole discount from its own
    /// margin. This is what a listed-price regime asks for, where the tax is
    /// fixed to the price printed on the packet whatever the shop charges.
    Undiscounted,
}

/// What kind of supply a line is, for the return the shop files.
///
/// A rate of zero is not one thing. A zero-rated supply is taxable at nothing,
/// and an exempt supply is outside the tax altogether; they are added up in
/// different places on a return, and one carries a credit for the tax the shop
/// paid on its own inputs where the other does not. A shop that has to tell
/// them apart cannot do it from a rate of zero.
///
/// Which goods fall in which is the revenue's word and the shop's to set. This
/// only keeps the two apart once somebody has said which is which, and says
/// nothing about any particular item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Supply {
    /// Taxable at the rate on the item. The ordinary case.
    #[default]
    Standard,
    /// Taxable, at nothing.
    ZeroRated,
    /// Outside the tax.
    Exempt,
}

impl Supply {
    /// The number this is stored and sent as.
    ///
    /// Appended, never renumbered: these go into sales that outlive the build
    /// that wrote them, and a number that changes meaning rewrites history.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::ZeroRated => 1,
            Self::Exempt => 2,
        }
    }

    /// Anything this build does not know is read as the ordinary case, which is
    /// the one that charges tax. A newer kind read as standard overcharges the
    /// shop's own return rather than understating it, and an understatement is
    /// the failure that costs a shop money it did not know it owed.
    #[must_use]
    pub const fn from_u8(stored: u8) -> Self {
        match stored {
            1 => Self::ZeroRated,
            2 => Self::Exempt,
            _ => Self::Standard,
        }
    }

    /// Whether tax is charged at all.
    #[must_use]
    pub const fn is_taxed(self) -> bool {
        matches!(self, Self::Standard)
    }

    /// What a receipt and a report call this.
    #[must_use]
    pub const fn in_words(self) -> &'static str {
        match self {
            Self::Standard => "VAT",
            Self::ZeroRated => "Zero rated",
            Self::Exempt => "Exempt",
        }
    }
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
    pub vat_base: VatBase,
    /// Standard, zero rated or exempt. A line that is not standard is taxed at
    /// nothing whatever rate the item carries, so the two cannot disagree.
    pub supply: Supply,
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
    /// The rate this line was taxed at, carried through so a ticket discount
    /// can retax the reduced net. Recovering the rate from net and VAT after
    /// the fact does not work: the division is lossy at small amounts, and a
    /// zero-VAT line is indistinguishable from an exempt one.
    pub vat_rate: Bp,
    /// Which amount that rate was charged on, carried for the same reason: a
    /// ticket discount has to know whether this line's tax moves with it.
    pub vat_base: VatBase,
    /// What kind of supply this was. A rate of zero cannot say whether the line
    /// was zero rated or exempt, and a return needs the two apart.
    pub supply: Supply,
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
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
        }
    }
}

/// Compute one line.
///
/// For an inclusive price the VAT is extracted from the discounted gross, so the
/// customer pays exactly the shelf price times the quantity. For an exclusive
/// price the VAT is added to the discounted net.
pub fn line_totals(line: &LineInput) -> Result<LineTotals> {
    if line.unit_price.is_negative() {
        // A negative quantity is a return and is expected. A negative price
        // never is: paired with a positive quantity it is a covert refund that
        // no permission gates and no report calls a refund, and paired with a
        // negative quantity it silently becomes a charge.
        return Err(MoneyError::Negative);
    }

    // A supply that is not standard is taxed at nothing, whatever rate the item
    // carries. Kept here rather than asked of every caller: an exempt line that
    // charged tax because somebody forgot to zero the rate is money taken from
    // a customer and declared to nobody.
    let rate = if line.supply.is_taxed() {
        line.vat_rate
    } else {
        Bp::ZERO
    };

    let gross_raw = line.unit_price.mul_qty(line.qty)?;
    let discount = discount_amount(gross_raw, line.discount)?;
    let discounted = gross_raw.checked_sub(discount)?;

    // What the rate is charged on. For the ordinary treatment this is the
    // discounted amount; for a listed-price regime it is the price on the
    // packet, and the discount comes out of the shop's margin instead.
    let taxable = match line.vat_base {
        VatBase::Discounted => discounted,
        VatBase::Undiscounted => gross_raw,
    };

    let (net, vat) = match line.price_mode {
        PriceMode::Exclusive => {
            let vat = taxable.apply_rate(rate)?;
            (discounted, vat)
        }
        PriceMode::Inclusive => {
            // The customer pays the discounted shelf price whatever the base,
            // because an inclusive price is what the customer pays. The base
            // decides only how much of it is tax.
            let taxable_net = taxable.net_of_inclusive(rate)?;
            let vat = taxable.checked_sub(taxable_net)?;
            (discounted.checked_sub(vat)?, vat)
        }
    };

    Ok(LineTotals {
        gross: gross_raw,
        discount,
        net,
        vat,
        total: net.checked_add(vat)?,
        vat_rate: rate,
        vat_base: line.vat_base,
        supply: line.supply,
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

    match ticket.ticket_discount {
        // A rate off the whole ticket is that rate off each line, taken on the
        // same amount a line discount is taken on: the line is worked out again
        // with the two discounts added together.
        //
        // Sharing the money out by line net instead was right for a shop that
        // prices before tax and wrong for one that prices on the packet, which
        // is most of them here. An inclusive line's net is worked out by taking
        // the tax back out of the shelf price, so a rate against it is a rate
        // against a rounded figure, and the tax then goes back on the rounded
        // remainder: 1.04 at fifteen percent with ten percent off the ticket
        // came to 0.93, where the same ten percent on the line came to 0.94.
        // The customer was short-changed a poisha and the receipt said 0.09 off
        // a discount the shopkeeper had called ten percent. This file's own
        // note says the two must agree, and until now they agreed in one of the
        // two pricing modes.
        Discount::Rate(rate) if rate != crate::money::Bp::ZERO => {
            lines = ticket
                .lines
                .iter()
                .zip(lines.iter())
                .map(|(input, worked)| {
                    let left = worked.gross.checked_abs()?.checked_sub(
                        worked.discount.checked_abs()?,
                    )?;
                    let extra = left.apply_rate(rate)?;
                    line_totals(&LineInput {
                        discount: Discount::Amount(
                            worked.discount.checked_abs()?.checked_add(extra)?,
                        ),
                        ..*input
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        }
        _ => {
            let net_before_ticket_discount = Minor::sum(lines.iter().map(|l| l.net))?;
            let ticket_discount =
                discount_amount(net_before_ticket_discount, ticket.ticket_discount)?;

            if ticket_discount != Minor::ZERO {
                apportion_ticket_discount(&mut lines, net_before_ticket_discount, ticket_discount)?;
            }
        }
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

/// One kind of supply on a ticket: what was sold that way, and its tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VatRow {
    pub rate_bp: u32,
    /// Standard, zero rated or exempt. Two rows can both be at nothing and mean
    /// different things on a return, which is the whole reason this is here.
    pub supply: Supply,
    pub net: Minor,
    pub vat: Minor,
}

/// What a ticket owes the revenue, by the rate it was charged at.
///
/// One row per distinct rate and kind of supply, smallest first, because that
/// is how a return is filed: a shop declares what it sold at each rate, and
/// declares what it sold outside the tax somewhere else again. Refund lines
/// carry their own sign and subtract, which is also what a return wants.
///
/// Computed from the recomputed line totals rather than from anything a device
/// stored, for the same reason the stock movements are: what a shop declares to
/// the revenue must not be something a payload could assert.
#[must_use]
pub fn vat_by_rate(totals: &TicketTotals) -> Vec<VatRow> {
    let mut rows: Vec<VatRow> = Vec::new();
    for line in &totals.lines {
        let rate = line.vat_rate.get();
        match rows
            .iter_mut()
            .find(|row| row.rate_bp == rate && row.supply == line.supply)
        {
            Some(row) => {
                // Saturating rather than checked: this is a report, and a shop
                // whose day overflows an i64 of poisha has a different problem.
                row.net = Minor::new(row.net.get().saturating_add(line.net.get()));
                row.vat = Minor::new(row.vat.get().saturating_add(line.vat.get()));
            }
            None => rows.push(VatRow {
                rate_bp: rate,
                supply: line.supply,
                net: line.net,
                vat: line.vat,
            }),
        }
    }
    // By rate, then by kind, so a return reads the same way twice running.
    rows.sort_by_key(|row| (row.rate_bp, row.supply.as_u8()));
    rows
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

/// VAT for a line whose net just changed.
///
/// A discount reduces the consideration, so it reduces the taxable amount and
/// the tax with it. Leaving the original VAT in place would charge the customer
/// tax on money they did not pay and would over-declare it to the revenue: a
/// line of 10,000 at 15 percent given 1,000 off owes 1,350, not 1,500. It would
/// also make a 10 percent line discount and a 10 percent ticket discount on the
/// same single-line basket produce different totals, which a shopkeeper
/// checking the arithmetic with a pen finds immediately.
fn recompute_vat(line: &LineTotals) -> Result<Minor> {
    match line.vat_base {
        VatBase::Discounted => line.net.apply_rate(line.vat_rate),
        // Fixed to the listed price, so a ticket discount moves the net and
        // leaves the tax where it was. Recomputing here would quietly turn a
        // listed-price line back into an ordinary one the moment somebody gave
        // a discount on the whole basket.
        VatBase::Undiscounted => Ok(line.vat),
    }
}

#[cfg(test)]
mod tests {

    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use alloc::vec;

    use super::*;

    #[test]
    fn what_a_ticket_owes_the_revenue_is_grouped_by_rate() {
        // A basket at two rates, which is an ordinary Bangladeshi basket: rice
        // at fifteen percent and something exempt beside it. The exempt line
        // says so rather than carrying a rate of zero, which is the distinction
        // a return is filed on.
        let totals = ticket_totals(&TicketInput {
            lines: vec![
                LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(43_000),
                    discount: Discount::None,
                    vat_rate: Bp::new(1_500).unwrap(),
                    price_mode: PriceMode::Exclusive,
                    vat_base: VatBase::Discounted,
                supply: Supply::Standard,
                },
                LineInput {
                    qty: Milli::new(2_000),
                    unit_price: Minor::new(10_000),
                    discount: Discount::None,
                    vat_rate: Bp::ZERO,
                    price_mode: PriceMode::Exclusive,
                    vat_base: VatBase::Discounted,
                    supply: Supply::Exempt,
                },
                LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(7_000),
                    discount: Discount::None,
                    vat_rate: Bp::new(1_500).unwrap(),
                    price_mode: PriceMode::Exclusive,
                    vat_base: VatBase::Discounted,
                supply: Supply::Standard,
                },
            ],
            ticket_discount: Discount::None,
        })
        .expect("realistic input");

        let rows = vat_by_rate(&totals);
        assert_eq!(rows.len(), 2, "one row per rate, not per line");
        // Smallest first, which is the order a return is read in.
        assert_eq!(rows[0].rate_bp, 0);
        assert_eq!(rows[0].supply, Supply::Exempt, "and it says which nothing");
        assert_eq!(rows[0].net, Minor::new(20_000), "exempt is still declared");
        assert_eq!(rows[0].vat, Minor::ZERO);
        assert_eq!(rows[1].rate_bp, 1_500);
        assert_eq!(rows[1].net, Minor::new(50_000), "the two lines together");
        assert_eq!(rows[1].vat, Minor::new(7_500));
    }

    /// Zero rated and exempt are both nothing, and are not the same nothing.
    #[test]
    fn what_is_taxed_at_nothing_still_says_which_nothing_it_is() {
        let totals = ticket_totals(&TicketInput {
            lines: vec![
                LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(20_000),
                    discount: Discount::None,
                    vat_rate: Bp::ZERO,
                    price_mode: PriceMode::Exclusive,
                    vat_base: VatBase::Discounted,
                    supply: Supply::ZeroRated,
                },
                LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(30_000),
                    discount: Discount::None,
                    vat_rate: Bp::ZERO,
                    price_mode: PriceMode::Exclusive,
                    vat_base: VatBase::Discounted,
                    supply: Supply::Exempt,
                },
            ],
            ticket_discount: Discount::None,
        })
        .expect("a basket of both is ordinary");

        let rows = vat_by_rate(&totals);
        assert_eq!(rows.len(), 2, "two rows at one rate, because they differ");
        assert_eq!(rows[0].supply, Supply::ZeroRated);
        assert_eq!(rows[0].net, Minor::new(20_000));
        assert_eq!(rows[1].supply, Supply::Exempt);
        assert_eq!(rows[1].net, Minor::new(30_000));
        assert_eq!(totals.vat_total, Minor::ZERO, "and neither charges tax");
    }

    /// A rate left on an item the shop has since called exempt charges nothing.
    ///
    /// The two cannot be allowed to disagree: an exempt line that charged tax
    /// because somebody forgot to zero the rate is money taken from a customer
    /// and declared to nobody.
    #[test]
    fn an_exempt_line_charges_nothing_whatever_rate_it_carries() {
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(43_000),
                discount: Discount::None,
                vat_rate: Bp::new(1_500).unwrap(),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Exempt,
            }],
            ticket_discount: Discount::None,
        })
        .expect("realistic input");

        assert_eq!(totals.vat_total, Minor::ZERO);
        assert_eq!(totals.total, Minor::new(43_000), "the customer pays the price");
        let rows = vat_by_rate(&totals);
        assert_eq!(rows[0].rate_bp, 0, "and the return says nothing was charged");
        assert_eq!(rows[0].supply, Supply::Exempt);
    }

    /// The same, for a price with the tax already in it.
    ///
    /// An inclusive price is what the customer pays, so an exempt line at an
    /// inclusive price is all net: extracting tax that is not charged would
    /// understate the turnover on the return by the tax fraction.
    #[test]
    fn an_exempt_inclusive_price_is_all_of_it_net() {
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(11_500),
                discount: Discount::None,
                vat_rate: Bp::new(1_500).unwrap(),
                price_mode: PriceMode::Inclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Exempt,
            }],
            ticket_discount: Discount::None,
        })
        .expect("realistic input");

        assert_eq!(totals.vat_total, Minor::ZERO);
        assert_eq!(totals.net_total, Minor::new(11_500));
        assert_eq!(totals.total, Minor::new(11_500));
    }

    #[test]
    fn a_refund_subtracts_from_what_is_declared() {
        // A return of one bag of rice. What a shop owes the revenue this month
        // goes down by what it went up by when the bag was sold, which is what
        // makes a refund a refund rather than a second sale.
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::new(-1_000),
                unit_price: Minor::new(43_000),
                discount: Discount::None,
                vat_rate: Bp::new(1_500).unwrap(),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Standard,
            }],
            ticket_discount: Discount::None,
        })
        .expect("a refund is realistic input");

        let rows = vat_by_rate(&totals);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].net, Minor::new(-43_000));
        assert_eq!(rows[0].vat, Minor::new(-6_450));
    }

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
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
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
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
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
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
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
        assert_eq!(
            change_due(Minor::new(4_945), Minor::new(5_000)),
            Ok(Minor::new(55))
        );
        assert_eq!(
            change_due(Minor::new(5_000), Minor::new(4_000)),
            Err(MoneyError::Underpaid {
                short_by: Minor::new(1_000)
            })
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
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
        };
        let refund = LineInput {
            qty: Milli::new(-1_000),
            ..sale
        };

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

    #[test]
    fn a_ticket_discount_reduces_the_tax_with_the_taxable_amount() {
        // 10,000 net at 15 percent, then 1,000 off the ticket. The customer
        // pays tax on 9,000, not on the 10,000 they were never charged.
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput::simple(Milli::ONE, Minor::new(10_000), bp(1_500))],
            ticket_discount: Discount::Amount(Minor::new(1_000)),
        })
        .unwrap();

        assert_eq!(totals.net_total, Minor::new(9_000));
        assert_eq!(totals.vat_total, Minor::new(1_350));
        assert_eq!(totals.total, Minor::new(10_350));
    }

    #[test]
    fn a_line_discount_and_a_ticket_discount_of_the_same_size_agree() {
        // The arithmetic a shopkeeper checks with a pen: ten percent off is ten
        // percent off, whichever button the cashier pressed.
        let line = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(10_000),
                discount: Discount::Rate(bp(1_000)),
                vat_rate: bp(1_500),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Standard,
            }],
            ticket_discount: Discount::None,
        })
        .unwrap();

        let ticket = ticket_totals(&TicketInput {
            lines: vec![LineInput::simple(Milli::ONE, Minor::new(10_000), bp(1_500))],
            ticket_discount: Discount::Rate(bp(1_000)),
        })
        .unwrap();

        assert_eq!(line.total, ticket.total);
        assert_eq!(line.vat_total, ticket.vat_total);

        // And in a shop that prices on the packet, which is most of them here.
        // This is where the two stopped agreeing: an inclusive line's net is
        // the shelf price with the tax taken back out, and a rate against that
        // rounded figure is not the rate the shopkeeper said. 1.04 with ten
        // percent off the ticket came to 0.93 where the line came to 0.94, and
        // the receipt said 0.09 off.
        let on_the_packet = |where_it_goes: (Discount, Discount)| {
            ticket_totals(&TicketInput {
                lines: vec![LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(104),
                    discount: where_it_goes.0,
                    vat_rate: bp(1_500),
                    price_mode: PriceMode::Inclusive,
                    vat_base: VatBase::Discounted,
                    supply: Supply::Standard,
                }],
                ticket_discount: where_it_goes.1,
            })
            .unwrap()
        };
        let on_the_line = on_the_packet((Discount::Rate(bp(1_000)), Discount::None));
        let on_the_ticket = on_the_packet((Discount::None, Discount::Rate(bp(1_000))));
        assert_eq!(on_the_line.total, Minor::new(94), "ten percent off 1.04 is 0.94");
        assert_eq!(on_the_ticket.total, on_the_line.total);
        assert_eq!(on_the_ticket.vat_total, on_the_line.vat_total);
        assert_eq!(
            on_the_ticket.discount_total,
            Minor::new(10),
            "and the receipt says the ten the shopkeeper said"
        );
    }

    #[test]
    fn a_line_discount_a_ticket_discount_and_vat_all_apply_to_one_line() {
        // 100.00 shelf, 10 percent off the line, then 5 percent off the whole
        // ticket, then 15 percent VAT. All three at once on the same line is the
        // ordinary case in a shop running a promotion, not an edge.
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(10_000),
                discount: Discount::Rate(bp(1_000)),
                vat_rate: bp(1_500),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Standard,
            }],
            ticket_discount: Discount::Rate(bp(500)),
        })
        .unwrap();

        // 100.00 less 10.00 is 90.00; less 5 percent of that is 85.50.
        assert_eq!(totals.net_total, Minor::new(8_550));
        // VAT follows the amount actually charged, not the shelf price.
        assert_eq!(totals.vat_total, Minor::new(1_283));
        assert_eq!(totals.total, Minor::new(9_833));
        // Both discounts are reported together: 10.00 off the line and 4.50 off
        // the ticket is 14.50 the customer did not pay.
        assert_eq!(totals.discount_total, Minor::new(1_450));
    }

    #[test]
    fn tax_fixed_to_the_listed_price_does_not_move_when_a_discount_is_given() {
        // 100.00 listed, 15 percent tax fixed to that, then 10 percent off the
        // line and 5 percent off the ticket. The customer pays 85.50 for the
        // goods and 15.00 of tax either way: the shop funds the whole discount.
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(10_000),
                discount: Discount::Rate(bp(1_000)),
                vat_rate: bp(1_500),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Undiscounted,
                supply: Supply::Standard,
            }],
            ticket_discount: Discount::Rate(bp(500)),
        })
        .unwrap();

        assert_eq!(totals.net_total, Minor::new(8_550), "the goods, discounted");
        assert_eq!(
            totals.vat_total,
            Minor::new(1_500),
            "the tax is fixed to the listed price and neither discount moves it"
        );
        assert_eq!(totals.total, Minor::new(10_050));
    }

    #[test]
    fn the_two_tax_bases_agree_when_nothing_is_discounted() {
        // Whatever the base, an undiscounted line is the same line. A difference
        // here would mean the setting changes prices for goods never on offer.
        for base in [VatBase::Discounted, VatBase::Undiscounted] {
            let totals = ticket_totals(&TicketInput {
                lines: vec![LineInput {
                    qty: Milli::ONE,
                    unit_price: Minor::new(10_000),
                    discount: Discount::None,
                    vat_rate: bp(1_500),
                    price_mode: PriceMode::Exclusive,
                    vat_base: base,
                    supply: Supply::Standard,
                }],
                ticket_discount: Discount::None,
            })
            .unwrap();
            assert_eq!(totals.total, Minor::new(11_500), "{base:?}");
            assert_eq!(totals.vat_total, Minor::new(1_500), "{base:?}");
        }
    }

    #[test]
    fn a_listed_price_refund_gives_back_exactly_what_the_sale_took() {
        // The rounding rule is symmetric about zero, and a fixed tax must not
        // break that: a customer returning goods gets the tax back too.
        let line = |qty: Milli| LineInput {
            qty,
            unit_price: Minor::new(10_000),
            discount: Discount::Rate(bp(1_000)),
            vat_rate: bp(1_500),
            price_mode: PriceMode::Exclusive,
            vat_base: VatBase::Undiscounted,
            supply: Supply::Standard,
        };

        let sale = ticket_totals(&TicketInput {
            lines: vec![line(Milli::ONE)],
            ticket_discount: Discount::None,
        })
        .unwrap();
        let refund = ticket_totals(&TicketInput {
            lines: vec![line(Milli::new(-1_000))],
            ticket_discount: Discount::None,
        })
        .unwrap();

        assert_eq!(refund.total.get(), -sale.total.get());
        assert_eq!(refund.vat_total.get(), -sale.vat_total.get());
    }

    #[test]
    fn an_inclusive_listed_price_still_charges_what_the_shelf_says() {
        // With a tax-inclusive price the customer pays the discounted shelf
        // price; the base decides only how much of it was tax.
        let totals = ticket_totals(&TicketInput {
            lines: vec![LineInput {
                qty: Milli::ONE,
                unit_price: Minor::new(11_500),
                discount: Discount::Rate(bp(1_000)),
                vat_rate: bp(1_500),
                price_mode: PriceMode::Inclusive,
                vat_base: VatBase::Undiscounted,
                supply: Supply::Standard,
            }],
            ticket_discount: Discount::None,
        })
        .unwrap();

        assert_eq!(totals.total, Minor::new(10_350), "ten percent off 115.00");
        assert_eq!(
            totals.vat_total,
            Minor::new(1_500),
            "tax fixed to the listed price"
        );
        assert_eq!(totals.net_total, Minor::new(8_850));
    }
}
