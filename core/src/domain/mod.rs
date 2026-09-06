//! Business rules. Pure functions over integer money, no I/O and no clock.

pub mod pricing;

pub use pricing::{
    change_due, line_totals, ticket_totals, Discount, LineInput, LineTotals, PriceMode, TicketInput,
    TicketTotals,
};
