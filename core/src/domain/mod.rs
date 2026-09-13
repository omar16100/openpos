//! Business rules. Pure functions over integer money, no I/O and no clock.

pub mod pricing;

pub use pricing::{
    Discount, LineInput, LineTotals, PriceMode, Supply, TicketInput, TicketTotals, VatBase,
    VatRow, change_due, line_totals, ticket_totals, vat_by_rate,
};

/// The supply above which the buyer has to be named on the invoice.
///
/// Twenty-five thousand taka, in poisha. From section 51(1)(c) of the Value
/// Added Tax and Supplementary Duty Act, 2012, which lists what a serially
/// numbered tax invoice must carry and makes the buyer's name, address and
/// business identification number conditional on the value of the supply being
/// more than that. Section 51(2) is the consequence the shop's customer feels:
/// without that clause on the invoice, no input tax credit is admissible
/// against it.
///
/// Read from the National Board of Revenue's own published English translation
/// of the Act, whose title page marks it unofficial; the Bengali text is the
/// one that governs. That is as close to the source as this project has got,
/// and closer than the vendor blogs these notes used to rest on.
///
/// The figure is here rather than on a screen because it is the rule, and
/// because the day it moves it moves in one place. Nothing in this crate
/// refuses a sale over it: a till that will not sell is a till a shop works
/// around, and the goods have left the counter either way. What the till does
/// is say so while the customer is still standing there, which is the only
/// moment the buyer's details can be asked for at all.
pub const NAME_THE_BUYER_ABOVE: crate::money::Minor = crate::money::Minor::new(2_500_000);

/// Whether this supply is one the invoice has to name the buyer on.
///
/// The figure is **the value of the supply, exclusive of VAT**, and that is the
/// Act's own reading rather than an interpretation: clause (c) says "the value
/// of the supply" and clause (e) of the same sub-section, listing what the
/// invoice carries, says "the value of the supply (exclusive of VAT)". Section
/// 32(1) says the same thing from the other end, making the value of a taxable
/// supply the consideration less the tax fraction of it.
///
/// It was the total the customer pays, which is that figure with the tax added
/// back on. At fifteen percent that asks for a BIN from a basket of 21,740
/// upwards, three thousand taka before the Act does, and a till that asks for
/// something the law does not is a till whose cashiers learn to wave the
/// message away, including on the sale where it was right.
///
/// Refunds are left alone. A refund's figures are below nothing so the
/// comparison is false anyway, and goods coming back are governed by section 52
/// and `buyer_wanted_on_the_credit_note` below.
#[must_use]
pub fn buyer_wanted_on_the_invoice(value_of_supply: crate::money::Minor) -> bool {
    value_of_supply.get() > NAME_THE_BUYER_ABOVE.get()
}

/// The VAT above which the paper for goods coming back has to name the buyer.
///
/// Five thousand taka, in poisha, and it is a figure about tax where the one
/// above is a figure about the value of a supply. From section 52(1)(f) of the
/// same Act, which lists what a credit note must carry and makes the buyer's
/// name, address and business identification number conditional on the VAT
/// payable on the supply being more than five thousand taka. Section 52(2) is
/// the consequence, and it is sharper than the one for an invoice: without that
/// clause the note "shall not be used in support of a claim for any decreasing
/// adjustment". The buyer has already claimed the credit on the original
/// invoice; this is the paper that gives it back, and a business that cannot
/// give it back has a return that does not match its shelves.
///
/// Two figures rather than one, because the Act uses two. A shop reading only
/// the twenty-five thousand rule asks for a BIN on the way out and not on the
/// way back, and at fifteen percent the tax on a supply of thirty-four thousand
/// is over five thousand while the supply itself is nowhere near twenty-five
/// thousand on any single line of it: they are different questions about
/// different numbers, and collapsing them into one would be this code deciding
/// something the Act did not.
pub const NAME_THE_BUYER_ON_A_CREDIT_ABOVE: crate::money::Minor =
    crate::money::Minor::new(500_000);

/// Whether the paper for these goods coming back has to name the buyer.
///
/// The figure tested is the VAT being adjusted rather than the value of the
/// goods, which is what section 52 counts. Taken whichever way the sign runs: a
/// refund's figures are below nothing here, and the question the Act asks is
/// how much tax is being given back, not which direction the arithmetic went.
#[must_use]
pub fn buyer_wanted_on_the_credit_note(vat: crate::money::Minor) -> bool {
    vat.get().saturating_abs() > NAME_THE_BUYER_ON_A_CREDIT_ABOVE.get()
}

/// What a shop wants done when a till is asked to sell more than it believes is
/// on the shelf.
///
/// A shop's own decision, because the figure is only as good as the shop's
/// stock keeping. A shop that has never counted holds zero of everything, and a
/// till that refused to sell on that basis would be a till that cannot sell.
///
/// Off by default for exactly that reason. Turning it on is a statement that
/// the figures mean something, and that is not a statement this code can make
/// on a shop's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StockRule {
    /// Sell whatever is asked for and say nothing. What a shop gets until it
    /// says otherwise.
    #[default]
    Off,
    /// Sell it, and say on the screen that the shelf disagrees. For a shop
    /// whose figures are worth reading and not worth stopping a queue over.
    Warn,
    /// Refuse it, and let a supervisor allow it. For a shop that would rather
    /// find out at the counter than at the count.
    Block,
}

impl StockRule {
    /// As it travels: a number, because it is stored and sent in places that
    /// carry no names.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Warn => 1,
            Self::Block => 2,
        }
    }

    /// Read back. Anything this build does not know is Off, which is the answer
    /// that keeps a till selling: a newer back office setting a rule an older
    /// device cannot enforce must not stop that device trading.
    #[must_use]
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Warn,
            2 => Self::Block,
            _ => Self::Off,
        }
    }
}

#[cfg(test)]
mod naming_the_buyer {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::money::Minor;

    /// The figure and the comparison, both from section 51(1)(c): more than
    /// 25,000 taka, not 25,000 and upward.
    #[test]
    fn the_line_is_above_twenty_five_thousand_rather_than_at_it() {
        assert_eq!(NAME_THE_BUYER_ABOVE.get(), 2_500_000, "in poisha");
        assert!(!buyer_wanted_on_the_invoice(Minor::new(2_499_999)));
        assert!(
            !buyer_wanted_on_the_invoice(Minor::new(2_500_000)),
            "exactly 25,000 is not more than 25,000, and a rule read one poisha \
             out is a rule a shop is told about by an auditor"
        );
        assert!(buyer_wanted_on_the_invoice(Minor::new(2_500_001)));
    }

    /// The figure is the supply, not the supply with the tax added back on.
    ///
    /// Twenty-four thousand of goods at fifteen percent is 27,600 across the
    /// counter. The Act asks about the first number and the till used to read
    /// the second, so this basket was asked to name its buyer three thousand
    /// taka early.
    #[test]
    fn it_is_the_supply_that_is_measured_and_not_what_the_customer_hands_over() {
        let supply = Minor::new(2_400_000);
        let with_tax = Minor::new(2_760_000);
        assert!(!buyer_wanted_on_the_invoice(supply));
        assert!(
            buyer_wanted_on_the_invoice(with_tax),
            "which is what made the old reading ask: the figure itself is over the line, \
             and it is not the figure section 51(1)(c) names"
        );
    }

    #[test]
    fn goods_coming_back_are_not_a_supply() {
        assert!(!buyer_wanted_on_the_invoice(Minor::new(-3_000_000)));
    }

    /// The other figure, and the other question. Section 52(1)(f): more than
    /// five thousand taka of VAT, not five thousand and upward.
    #[test]
    fn the_credit_note_line_is_above_five_thousand_of_tax() {
        assert_eq!(NAME_THE_BUYER_ON_A_CREDIT_ABOVE.get(), 500_000, "in poisha");
        assert!(!buyer_wanted_on_the_credit_note(Minor::new(499_999)));
        assert!(
            !buyer_wanted_on_the_credit_note(Minor::new(500_000)),
            "exactly 5,000 is not more than 5,000"
        );
        assert!(buyer_wanted_on_the_credit_note(Minor::new(500_001)));
    }

    /// A refund's figures run below nothing, which is where this rule is read.
    #[test]
    fn tax_given_back_is_counted_whichever_way_the_sign_runs() {
        assert!(buyer_wanted_on_the_credit_note(Minor::new(-500_001)));
        assert!(!buyer_wanted_on_the_credit_note(Minor::new(-500_000)));
        assert!(!buyer_wanted_on_the_credit_note(Minor::new(-499_999)));
    }

    /// The two rules are about different numbers, and a sale can be under one
    /// and over the other.
    ///
    /// Thirty-four thousand of goods at fifteen percent carries 5,100 of tax.
    /// Read only by the invoice rule the buyer is asked for, because the supply
    /// is over 25,000; read only by the credit rule they are asked for too.
    /// What the sale below shows is the case that made this worth writing down:
    /// a basket of 24,000 carrying 3,600 of tax names nobody on either paper,
    /// and one of 40,000 names them on both.
    #[test]
    fn the_two_figures_answer_different_questions() {
        assert!(!buyer_wanted_on_the_invoice(Minor::new(2_400_000)));
        assert!(!buyer_wanted_on_the_credit_note(Minor::new(360_000)));

        assert!(buyer_wanted_on_the_invoice(Minor::new(4_000_000)));
        assert!(buyer_wanted_on_the_credit_note(Minor::new(600_000)));
    }
}