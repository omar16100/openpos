//! Business rules. Pure functions over integer money, no I/O and no clock.

pub mod pricing;

pub use pricing::{
    Discount, LineInput, LineTotals, PriceMode, Supply, TicketInput, TicketTotals, VatBase,
    VatRow, change_due, line_totals, ticket_totals, vat_by_rate,
};

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
