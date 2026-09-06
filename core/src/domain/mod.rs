//! Business rules. Pure functions over integer money, no I/O and no clock.

pub mod pricing;

pub use pricing::{
    Discount, LineInput, LineTotals, PriceMode, TicketInput, TicketTotals, VatBase, change_due,
    line_totals, ticket_totals, vat_by_rate,
};
