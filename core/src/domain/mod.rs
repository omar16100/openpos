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
/// Refunds are left alone. A refund's total is below nothing so the comparison
/// is false anyway, and goods coming back are not a supply.
#[must_use]
pub fn buyer_wanted_on_the_invoice(total: crate::money::Minor) -> bool {
    total.get() > NAME_THE_BUYER_ABOVE.get()
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

    #[test]
    fn goods_coming_back_are_not_a_supply() {
        assert!(!buyer_wanted_on_the_invoice(Minor::new(-3_000_000)));
    }
}