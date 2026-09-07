//! The receipt, laid out once for every printer.
//!
//! Rendered here rather than on each platform for the same reason the money is
//! computed here: a browser receipt and a tablet receipt that differ are two
//! documents claiming to describe one sale, and the one the customer holds is
//! whichever they happened to be given. A dispute is settled against paper.
//!
//! The output is lines of text and nothing else. Turning them into ESC/POS
//! bytes, into HTML for a browser's print dialog, or into a PDF is the
//! platform's job, and each of those is a different job; laying out columns is
//! the same job everywhere and is done once.
//!
//! # What this is not
//!
//! Not a fiscal document. Bangladesh requires a Mushak 6.3 tax invoice with the
//! supplier's and buyer's BIN, and for covered categories the number comes from
//! an EFD rather than from this software. What is printed here carries the
//! fields this system holds and is honest about the rest: a receipt that looked
//! like a tax invoice without being one would be worse than a plain one, because
//! a shopkeeper would believe it.
//!
//! # Width
//!
//! Columns are counted in characters, which is right for the Latin and digit
//! text a printer renders in a fixed-width font and approximate for Bengali,
//! where a cluster may occupy a different number of cells than it has `char`s.
//! The alternative is a font-metrics table this crate has no business holding.
//! Where it matters the layout leaves slack rather than truncating a price.

pub mod escpos;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::cart::{Direction, Ticket};
use crate::money::Minor;

/// A 58mm printer, the common cheap one.
pub const NARROW: usize = 32;

/// An 80mm printer.
pub const WIDE: usize = 48;

/// How a line should look. Deliberately small: a platform that has to interpret
/// a rich style language will interpret it differently on each platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emphasis {
    Normal,
    /// The shop's name, and the amount the customer pays. The two things a
    /// person looks for without reading.
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    pub text: String,
    pub emphasis: Emphasis,
}

impl Line {
    fn plain(text: String) -> Self {
        Self {
            text,
            emphasis: Emphasis::Normal,
        }
    }

    fn strong(text: String) -> Self {
        Self {
            text,
            emphasis: Emphasis::Strong,
        }
    }
}

/// Who the shop is, as it should appear on paper.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shop {
    pub name: String,
    /// Business Identification Number. Printed when there is one and omitted
    /// when there is not, rather than printed as a blank label: a line reading
    /// "BIN:" with nothing after it looks like a fault in the printer.
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
}

/// Everything the paper needs that the ticket does not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub shop: Shop,
    /// Local date and time, already formatted. This crate has no clock and no
    /// timezone database, and a receipt showing UTC in Dhaka is a receipt that
    /// disagrees with the customer's watch.
    pub rung_at: String,
    /// Who served, when the shop shows it.
    pub cashier: Option<String>,
    /// Who bought it, when the shop knows: the name it has written down for
    /// somebody buying on account. A sale on account is a document the shop and
    /// the customer will both refer to weeks later, and a piece of paper naming
    /// neither of them is no use to either.
    pub customer: Option<String>,
    /// The buyer's own Business Identification Number, when they are a business
    /// and the shop has written it down.
    ///
    /// A tax invoice in this country names both BINs, the supplier's and the
    /// buyer's. The shop's is at the top of every receipt already; this is the
    /// other one, printed only when there is one, because a label with nothing
    /// after it looks like a fault.
    pub customer_bin: Option<String>,
    pub width: usize,
}

/// Lay a ticket out for printing.
#[must_use]
pub fn render(ticket: &Ticket, context: &Context) -> Vec<Line> {
    // A width below this cannot hold a name and a price on one row, and
    // silently producing gibberish is worse than producing something narrow.
    let width = context.width.max(24);
    let mut out = Vec::new();

    out.push(Line::strong(centre(&context.shop.name, width)));
    for detail in [
        context.shop.address.as_deref(),
        context.shop.phone.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        out.push(Line::plain(centre(detail, width)));
    }
    if let Some(bin) = context.shop.bin.as_deref() {
        out.push(Line::plain(centre(&format!("BIN {bin}"), width)));
    }
    out.push(Line::plain(rule(width)));

    // A refund says so at the top, in the place a person looks first. Buried
    // among the totals it is a line nobody reads until the money is gone.
    if let Direction::Refund { original_receipt } = &ticket.direction {
        out.push(Line::strong(centre("REFUND", width)));
        if let Some(against) = original_receipt.as_deref() {
            out.push(Line::plain(centre(&format!("against {against}"), width)));
        }
        out.push(Line::plain(rule(width)));
    }

    match ticket.receipt_no.as_deref() {
        Some(number) => out.push(Line::plain(columns("Receipt", number, width))),
        // Said plainly rather than left blank. A sale rung while the terminal
        // had no numbers left is valid and will be numbered by the back office,
        // and the customer is entitled to know that is what happened.
        None => out.push(Line::plain(columns("Receipt", "to be assigned", width))),
    }
    out.push(Line::plain(columns("Date", &context.rung_at, width)));
    if let Some(cashier) = context.cashier.as_deref() {
        out.push(Line::plain(columns("Served by", cashier, width)));
    }
    if let Some(customer) = context.customer.as_deref() {
        out.push(Line::plain(columns("Customer", customer, width)));
    }
    if let Some(bin) = context.customer_bin.as_deref() {
        out.push(Line::plain(columns("Buyer BIN", bin, width)));
    }
    out.push(Line::plain(rule(width)));

    for (line, totals) in ticket.lines.iter().zip(ticket.totals.lines.iter()) {
        // The name gets its own row, so a long one is never truncated and never
        // pushes a price off the edge.
        out.push(Line::plain(clip(&line.name, width)));
        // "2 kg x 100.00" where a shop sells by weight, and "2 x 100.00" where
        // it sells things. Printing "2 Nos x" would be noise on every line of
        // every receipt for the sake of the few that are weighed.
        let quantity = if line.unit.eq_ignore_ascii_case("Nos") || line.unit.trim().is_empty() {
            format!(
                "  {} x {}",
                quantity_of(line.qty.get()),
                money(line.unit_price)
            )
        } else {
            format!(
                "  {} {} x {}",
                quantity_of(line.qty.get()),
                line.unit,
                money(line.unit_price)
            )
        };
        // What that many at that price comes to, in the same basis as the price
        // just printed. Not the line's own total: that is what the customer
        // pays after the discount, and printing it above a discount row makes a
        // receipt nobody can follow. A person reads "one at 430.00, less 43.00"
        // and expects the arithmetic to work downwards.
        //
        // For a shelf price that excludes tax, that is the line before tax. For
        // one that includes it, it is the line with the tax still in, which is
        // the total with the discount added back: either way, the quantity
        // times the price on the shelf.
        let at_that_price = match line.price_mode {
            crate::domain::PriceMode::Exclusive => totals.gross,
            crate::domain::PriceMode::Inclusive => totals
                .total
                .checked_add(totals.discount)
                .unwrap_or(totals.total),
        };
        out.push(Line::plain(columns(
            &quantity,
            &money(at_that_price),
            width,
        )));
        if totals.discount != Minor::ZERO {
            out.push(Line::plain(columns(
                "  discount",
                &money(
                    Minor::ZERO
                        .checked_sub(totals.discount)
                        .unwrap_or(totals.discount),
                ),
                width,
            )));
        }
    }

    out.push(Line::plain(rule(width)));
    out.push(Line::plain(columns(
        "Net",
        &money(ticket.totals.net_total),
        width,
    )));
    if ticket.totals.discount_total != Minor::ZERO {
        out.push(Line::plain(columns(
            "Discount",
            &money(
                Minor::ZERO
                    .checked_sub(ticket.totals.discount_total)
                    .unwrap_or(ticket.totals.discount_total),
            ),
            width,
        )));
    }
    // Tax by the rate it was charged at, which is how it is declared. One rate
    // is the ordinary basket and prints one line; a basket holding rice at
    // fifteen percent and something exempt beside it prints both, because a
    // single "VAT 64.50" on that receipt says nothing about which goods were
    // taxed and the customer is entitled to see it.
    let by_rate = crate::domain::vat_by_rate(&ticket.totals);
    if by_rate.len() > 1 {
        for row in &by_rate {
            // Zero rated and exempt are named rather than printed as "VAT 0%",
            // because that line is the only thing on the paper that tells a
            // customer, and an auditor, which of the two this shop said it was.
            let said = if row.supply.is_taxed() {
                format!("VAT {} on {}", percent(row.rate_bp), money(row.net))
            } else {
                format!("{} on {}", row.supply.in_words(), money(row.net))
            };
            out.push(Line::plain(columns(&said, &money(row.vat), width)));
        }
        out.push(Line::plain(columns(
            "VAT in all",
            &money(ticket.totals.vat_total),
            width,
        )));
    } else {
        let named = by_rate.first().map_or_else(
            || String::from("VAT"),
            |row| {
                if row.supply.is_taxed() {
                    format!("VAT {}", percent(row.rate_bp))
                } else {
                    String::from(row.supply.in_words())
                }
            },
        );
        out.push(Line::plain(columns(
            &named,
            &money(ticket.totals.vat_total),
            width,
        )));
    }
    out.push(Line::strong(columns(
        "TOTAL",
        &money(ticket.totals.total),
        width,
    )));

    for tender in &ticket.tenders {
        out.push(Line::plain(columns(
            &tender_line(tender),
            &money(tender.amount),
            width,
        )));
    }
    if ticket.change != Minor::ZERO {
        out.push(Line::plain(columns("Change", &money(ticket.change), width)));
    }

    // Overrides are printed because they are the reason a price on this paper
    // differs from the price on the shelf, and that is the first question asked
    // about a receipt somebody disputes.
    if !ticket.overrides.is_empty() {
        out.push(Line::plain(rule(width)));
        for note in &ticket.overrides {
            out.push(Line::plain(clip(note, width)));
        }
    }

    out.push(Line::plain(String::new()));
    out.push(Line::plain(centre("Thank you", width)));
    out
}

/// Minor units as a person reads them. Always two decimals: a price shown as 43
/// when it means 43.00 reads as a different price at a glance.
fn money(amount: Minor) -> String {
    let minor = amount.get();
    let sign = if minor < 0 { "-" } else { "" };
    let whole = minor.checked_abs().unwrap_or(i64::MAX).saturating_div(100);
    let part = minor
        .checked_abs()
        .unwrap_or(i64::MAX)
        .saturating_sub(whole.saturating_mul(100));
    format!("{sign}{whole}.{part:02}")
}

/// Thousandths as a quantity, with the trailing zeros most lines do not need.
/// A quantity as a person reads it. Shared with the till's refusals, so a
/// cashier is told about a shelf in the same words the paper uses.
pub(crate) fn quantity_of(milli: i64) -> String {
    if milli.checked_rem(1_000) == Some(0) {
        return milli.saturating_div(1_000).to_string();
    }
    let whole = milli.saturating_div(1_000);
    let part = milli
        .checked_abs()
        .unwrap_or(i64::MAX)
        .saturating_sub(whole.checked_abs().unwrap_or(0).saturating_mul(1_000));
    format!("{whole}.{part:03}")
}

/// What to call a tender on paper.
///
/// A tax rate as a person reads it: 1500 basis points is 15%, and 750 is 7.5%.
fn percent(rate: u32) -> String {
    let whole = rate / 100;
    let part = rate % 100;
    if part == 0 {
        format!("{whole}%")
    } else if part.is_multiple_of(10) {
        format!("{whole}.{}%", part / 10)
    } else {
        format!("{whole}.{part:02}%")
    }
}

/// A wallet prints its own name, because "bKash" and "Nagad" are what a customer
/// asks about and "Wallet" is what nobody does. The reference is printed with
/// it: a mobile payment queried a week later is looked up by that number.
fn tender_line(tender: &crate::cart::Tender) -> String {
    let named = |label: &str| match tender.reference.as_deref() {
        Some(reference) => format!("{label} {reference}"),
        None => String::from(label),
    };
    match &tender.kind {
        crate::cart::TenderKind::Cash => String::from("Cash"),
        crate::cart::TenderKind::Card => named("Card"),
        // Who owes it. A sale on account with nobody's name against it is money
        // the shop has given away and cannot chase, and this line is the only
        // record of it the customer ever sees.
        crate::cart::TenderKind::Credit => named("On account"),
        crate::cart::TenderKind::Wallet(name) | crate::cart::TenderKind::Other(name) => {
            match tender.reference.as_deref() {
                Some(reference) => format!("{name} {reference}"),
                None => name.to_string(),
            }
        }
    }
}

/// A label on the left and an amount on the right, filling the width.
///
/// When the two cannot fit, the amount wins and the label gives way. A price is
/// the one thing on a receipt that must not be cut.
fn columns(left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    if right_len >= width {
        return right.to_string();
    }
    let room = width.saturating_sub(right_len).saturating_sub(1);
    let left = clip(left, room);
    let gap = width
        .saturating_sub(left.chars().count())
        .saturating_sub(right_len);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn centre(text: &str, width: usize) -> String {
    let text = clip(text, width);
    let pad = width.saturating_sub(text.chars().count()).saturating_div(2);
    format!("{}{text}", " ".repeat(pad))
}

fn rule(width: usize) -> String {
    "-".repeat(width)
}

/// Cut to a column count without splitting a character.
fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width).collect()
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
    use crate::cart::{Cart, CartLimits, Tender, TenderKind};

    #[test]
    fn a_weighed_line_says_what_was_weighed() {
        let mut sold = item(10_000, "Rice, loose");
        sold.unit = "kg".into();
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&sold, Milli::new(1_500)).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(17_250),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(1), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();

        let printed = text(&render(&ticket, &context()));

        // A kilo and a half of loose rice, to the gram, which is what a scale
        // reads. Without the unit this said "1.500 x 100.00", a number of
        // nothing.
        assert!(printed.contains("1.500 kg x 100.00"), "{printed}");
    }

    #[test]
    fn a_thing_sold_in_pieces_does_not_say_so_on_every_line() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::new(2_000))
            .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(98_900),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(1), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();

        let printed = text(&render(&ticket, &context()));

        // "2 Nos x 430.00" would be noise on every line of every receipt, for
        // the sake of the few that are weighed.
        assert!(printed.contains("2 x 430.00"), "{printed}");
        assert!(!printed.contains("Nos"), "{printed}");
    }

    #[test]
    fn a_sale_on_account_prints_who_owes_it() {
        // The only record of a debt the customer ever gets, and the shop's copy
        // of the same line is what it chases against. Without the name it says
        // "On account" and nothing else, which is money given away.
        let owed = Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(49_450),
            reference: Some("Karim, flat 3".into()),
        };
        assert_eq!(tender_line(&owed), "On account Karim, flat 3");

        // A card approval code prints for the same reason a wallet's reference
        // does: a payment queried a week later is looked up by that number.
        let card = Tender {
            kind: TenderKind::Card,
            amount: Minor::new(49_450),
            reference: Some("A0417".into()),
        };
        assert_eq!(tender_line(&card), "Card A0417");

        // And cash is cash.
        let cash = Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(49_450),
            reference: None,
        };
        assert_eq!(tender_line(&cash), "Cash");
    }

    use crate::domain::{PriceMode, VatBase};
    use crate::ids::Ulid;
    use crate::money::{Bp, Milli};
    use crate::replica::Item;

    fn item(price_minor: i64, name: &str) -> Item {
        Item {
            id: Ulid::from_u128(1),
            code: "RICE5".into(),
            name_en: name.into(),
            name_bn: name.into(),
            unit: "Nos".into(),
            price: Minor::new(price_minor),
            cost: Minor::new(3_800),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            vat_base: VatBase::Discounted,
            barcodes: vec!["8690000000001".into()],
            on_hand: Milli::new(40_000),
            active: true,
            supply: crate::domain::Supply::Standard,
        }
    }

    fn context() -> Context {
        Context {
            customer: None,
            shop: Shop {
                name: "Karim General Store".into(),
                bin: Some("001234567-0101".into()),
                address: Some("12 Mirpur Road, Dhaka".into()),
                phone: None,
            },
            rung_at: "06 Sep 2026 15:42".into(),
            cashier: Some("Rahim".into()),
            width: NARROW,
            customer_bin: None,
        }
    }

    fn sale() -> Ticket {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::new(2_000))
            .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(100_000),
            reference: None,
        });
        let mut ticket = cart
            .close(Ulid::from_u128(900), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();
        ticket.receipt_no = Some("T1-000100".into());
        ticket
    }

    fn text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn tax_is_printed_by_the_rate_it_was_charged_at() {
        // The ordinary basket: one rate, and the rate is named. "VAT 64.50" on
        // its own leaves a customer to work out what it was charged on.
        let paper = text(&render(&sale(), &context()));
        assert!(paper.contains("VAT 15%"), "{paper}");
        assert!(!paper.contains("VAT in all"), "one rate needs no total row");

        // Rice at fifteen percent and something exempt beside it, which is an
        // ordinary Bangladeshi basket. A single VAT number on that receipt says
        // nothing about which goods were taxed.
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::ONE)
            .unwrap();
        let mut exempt = item(10_000, "Lentils, loose");
        exempt.id = Ulid::from_u128(2);
        exempt.vat_rate = Bp::ZERO;
        cart.add_item(&exempt, Milli::new(2_000)).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(100_000),
            reference: None,
        });
        let mut ticket = cart
            .close(Ulid::from_u128(901), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();
        ticket.receipt_no = Some("T1-000101".into());

        let paper = text(&render(&ticket, &context()));
        assert!(paper.contains("VAT 0% on 200.00"), "{paper}");
        assert!(paper.contains("VAT 15% on 430.00"), "{paper}");
        assert!(paper.contains("VAT in all"), "{paper}");
        // And what it adds up to is the same number the ticket carries, which
        // is the same one the shop declares.
        assert!(paper.contains("64.50"), "{paper}");
    }

    /// The paper says which nothing, because the paper is the only place an
    /// auditor or a customer can read it.
    #[test]
    fn what_was_taxed_at_nothing_is_named_on_the_paper() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::ONE)
            .unwrap();
        let mut exempt = item(10_000, "A school exercise book");
        exempt.id = Ulid::from_u128(2);
        exempt.supply = crate::domain::Supply::Exempt;
        cart.add_item(&exempt, Milli::new(2_000)).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(100_000),
            reference: None,
        });
        let mut ticket = cart
            .close(Ulid::from_u128(902), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();
        ticket.receipt_no = Some("T1-000102".into());

        let paper = text(&render(&ticket, &context()));
        assert!(paper.contains("Exempt on 200.00"), "{paper}");
        assert!(
            !paper.contains("VAT 0%"),
            "a rate of zero says nothing about which nothing this was: {paper}"
        );
        assert!(paper.contains("VAT 15% on 430.00"), "{paper}");
    }

    #[test]
    fn a_rate_with_a_half_in_it_reads_as_a_person_writes_it() {
        assert_eq!(percent(1_500), "15%");
        assert_eq!(percent(750), "7.5%");
        assert_eq!(percent(0), "0%");
        assert_eq!(percent(1_025), "10.25%");
    }

    #[test]
    fn a_sale_on_account_names_the_buyer_when_the_shop_knows_them() {
        // A sale on account is a document both sides refer to weeks later, and
        // a piece of paper naming neither of them is no use to either.
        let mut named = context();
        named.customer = Some("Karim, flat 3".into());
        let paper = text(&render(&sale(), &named));
        assert!(paper.contains("Customer"), "{paper}");
        assert!(paper.contains("Karim, flat 3"), "{paper}");

        // A shop that has written nobody down prints no line at all rather than
        // an empty one.
        assert!(!text(&render(&sale(), &context())).contains("Customer"));
    }

    #[test]
    fn a_discounted_line_reads_downwards() {
        // What the row above a discount says used to be the line's own total,
        // which already had the discount in it. A customer read "430.00 each,
        // 445.05, less 43.00" and could make no sense of any of it.
        let mut cart = crate::cart::Cart::new(crate::cart::CartLimits::unrestricted());
        let mut on_the_shelf_with_tax_in_it = item(43_000, "Tea 400g");
        on_the_shelf_with_tax_in_it.id = Ulid::from_u128(2);
        on_the_shelf_with_tax_in_it.price_mode = PriceMode::Inclusive;
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::ONE)
            .unwrap();
        cart.add_item(&on_the_shelf_with_tax_in_it, Milli::new(2_000))
            .unwrap();
        cart.set_line_discount(0, crate::domain::Discount::Amount(Minor::new(4_300)))
            .unwrap();
        cart.set_line_discount(1, crate::domain::Discount::Amount(Minor::new(4_300)))
            .unwrap();
        cart.add_tender(crate::cart::Tender {
            kind: crate::cart::TenderKind::Cash,
            amount: Minor::new(200_000),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(1), 1_788_600_000_000)
            .unwrap();
        let paper = text(&render(&ticket, &context()));

        // One at 430.00 is 430.00, and the discount takes 43.00 off it. The
        // price printed and the amount beside it are in the same basis, which
        // is the whole of the fix.
        let rows: alloc::vec::Vec<&str> = paper.lines().collect();
        let at = rows
            .iter()
            .position(|row| row.contains("Rice Miniket 5kg"))
            .expect("the rice line");
        assert!(rows[at + 1].starts_with("  1 x 430.00"), "{paper}");
        assert!(rows[at + 1].ends_with("430.00"), "{paper}");
        assert!(
            rows[at + 2].contains("discount") && rows[at + 2].ends_with("-43.00"),
            "{paper}"
        );

        // And where the shelf price has the tax in it, so does the amount: two
        // at 430.00 is 860.00, not the 747.83 the tax-exclusive figure would be.
        let tea = rows
            .iter()
            .position(|row| row.contains("Tea 400g"))
            .expect("the tea line");
        assert!(rows[tea + 1].starts_with("  2 x 430.00"), "{paper}");
        assert!(rows[tea + 1].ends_with("860.00"), "{paper}");

        // The summary still adds up to what is paid.
        assert!(
            paper.contains("TOTAL") && paper.contains("1262.05"),
            "{paper}"
        );
    }

    #[test]
    fn a_receipt_carries_the_shop_the_number_and_the_money() {
        let printed = text(&render(&sale(), &context()));

        assert!(printed.contains("Karim General Store"));
        assert!(printed.contains("BIN 001234567-0101"));
        assert!(printed.contains("T1-000100"));
        assert!(printed.contains("Rice Miniket 5kg"));
        assert!(printed.contains("2 x 430.00"));
        assert!(printed.contains("989.00"), "{printed}");
        assert!(printed.contains("Change"));
    }

    #[test]
    fn nothing_is_wider_than_the_paper() {
        for width in [24_usize, NARROW, WIDE] {
            let mut context = context();
            context.width = width;
            for line in render(&sale(), &context) {
                assert!(
                    line.text.chars().count() <= width,
                    "{width}: {:?} is {} wide",
                    line.text,
                    line.text.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_long_name_never_pushes_a_price_off_the_edge() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(
            &item(
                43_000,
                "Premium Aromatic Chinigura Rice, Extra Long Grain, 5 kg Sack",
            ),
            Milli::ONE,
        )
        .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(100_000),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(901), Ulid::from_u128(7), 0)
            .unwrap();

        let printed = render(&ticket, &context());
        // The price is the one thing that must survive: the name gets its own
        // row and is cut if it has to be.
        assert!(text(&printed).contains("494.50"), "{}", text(&printed));
        for line in printed {
            assert!(line.text.chars().count() <= NARROW);
        }
    }

    #[test]
    fn a_refund_says_so_where_a_person_looks_first() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some("T1-000100")).unwrap();
        // A positive quantity: the cart negates it because the ticket is a
        // refund, which is the whole point of the direction being state.
        cart.add_item(&item(43_000, "Rice Miniket 5kg"), Milli::ONE)
            .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(902), Ulid::from_u128(7), 0)
            .unwrap();

        let printed = render(&ticket, &context());
        let head = text(&printed[..6.min(printed.len())]);
        assert!(head.contains("REFUND"), "buried among the totals: {head}");
        assert!(head.contains("against T1-000100"));
        assert!(text(&printed).contains("-494.50"));
    }

    #[test]
    fn a_sale_with_no_number_yet_says_so_rather_than_leaving_a_gap() {
        let mut ticket = sale();
        ticket.receipt_no = None;

        // The sale is valid and the back office will number it. A blank line
        // would read as a fault in the printer.
        assert!(text(&render(&ticket, &context())).contains("to be assigned"));
    }

    #[test]
    fn a_shop_with_no_bin_prints_no_bin_line() {
        let mut context = context();
        context.shop.bin = None;
        // A label with nothing after it looks broken, and a receipt that looks
        // broken is one a customer will not accept as proof of anything.
        assert!(!text(&render(&sale(), &context)).contains("BIN"));
    }

    #[test]
    fn an_override_is_printed_because_it_explains_the_price() {
        let mut ticket = sale();
        ticket.overrides = vec!["price override by Rahim".into()];

        // The first question about a disputed receipt is why this price differs
        // from the shelf.
        assert!(text(&render(&ticket, &context())).contains("price override by Rahim"));
    }

    #[test]
    fn money_reads_the_way_a_person_writes_it() {
        assert_eq!(money(Minor::new(0)), "0.00");
        assert_eq!(money(Minor::new(5)), "0.05");
        assert_eq!(money(Minor::new(100)), "1.00");
        assert_eq!(money(Minor::new(98_900)), "989.00");
        assert_eq!(money(Minor::new(-49_450)), "-494.50");
    }

    #[test]
    fn quantities_drop_the_zeros_most_lines_do_not_need() {
        assert_eq!(quantity_of(1_000), "1");
        assert_eq!(quantity_of(2_000), "2");
        assert_eq!(quantity_of(1_500), "1.500");
        assert_eq!(quantity_of(-1_000), "-1");
    }
}
