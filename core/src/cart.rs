//! The sale in progress, and the immutable ticket it becomes.
//!
//! This is the state machine every UI drives. The Flutter till and the Svelte
//! till both send the same commands here and render what comes back; neither
//! holds business rules of its own. That is what makes two front ends affordable
//! rather than two places for the arithmetic to drift apart.
//!
//! Everything is synchronous and in memory. Closing a ticket produces a value;
//! persisting it is somebody else's job.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::domain::{ticket_totals, Discount, LineInput, PriceMode, TicketInput, TicketTotals};
use crate::ids::Ulid;
use crate::money::{Bp, Milli, Minor, MoneyError};
use crate::replica::{Item, ItemId};

pub type TicketId = Ulid;
pub type CustomerId = Ulid;
pub type TerminalId = Ulid;

/// How a customer paid. Cash covers most Bangladeshi retail today; the wallet
/// variant exists so bKash and Nagad can be recorded from day one, by reference,
/// long before there is an API integration to talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenderKind {
    Cash,
    /// A mobile wallet, named because a shop may accept several.
    Wallet(Box<str>),
    Card,
    /// Sold on account, settled later.
    Credit,
    Other(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tender {
    pub kind: TenderKind,
    pub amount: Minor,
    /// A wallet transaction id, or a card approval code. Recorded, never trusted.
    pub reference: Option<Box<str>>,
}

/// A line as it sits in the cart.
///
/// The price, name and tax rate are copied from the catalogue when the line is
/// added, not looked up later. A price change syncing in mid-sale must not
/// silently reprice a basket the cashier already quoted to the customer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartLine {
    pub item_id: ItemId,
    pub code: Box<str>,
    pub name: Box<str>,
    pub unit_price: Minor,
    pub qty: Milli,
    pub discount: Discount,
    pub vat_rate: Bp,
    pub price_mode: PriceMode,
}

impl CartLine {
    /// The pricing input this line represents.
    ///
    /// Public because the server rebuilds it to revalidate a synced sale with
    /// the same arithmetic the till used. That check is only meaningful if both
    /// sides start from the same input.
    #[must_use]
    pub fn as_input(&self) -> LineInput {
        LineInput {
            qty: self.qty,
            unit_price: self.unit_price,
            discount: self.discount,
            vat_rate: self.vat_rate,
            price_mode: self.price_mode,
        }
    }
}

/// What this cashier is allowed to do, snapshotted from their role.
///
/// Enforced here rather than in the UI, because there are two UIs and because
/// the till runs offline where no server can be asked. Offline enforcement is
/// advisory by nature, so every override is recorded on the ticket for review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CartLimits {
    /// Largest discount this cashier may apply without a supervisor.
    pub max_discount: Bp,
    /// Whether they may type a price over the catalogue's.
    pub allow_price_override: bool,
}

impl Default for CartLimits {
    fn default() -> Self {
        Self {
            max_discount: Bp::ZERO,
            allow_price_override: false,
        }
    }
}

impl CartLimits {
    /// A supervisor: no ceiling, may override prices.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            max_discount: Bp::new(crate::money::BP_ONE).unwrap_or(Bp::ZERO),
            allow_price_override: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CartError {
    /// No line at that position.
    NoSuchLine { index: usize },
    /// The cart has nothing in it.
    Empty,
    /// Discount above what this cashier may give.
    DiscountAboveCeiling { requested: u32, ceiling: u32 },
    /// Price override attempted without the permission for it.
    PriceOverrideNotAllowed,
    /// The tendered amounts do not cover the total.
    Underpaid { short_by: Minor },
    /// Arithmetic went wrong, which for realistic baskets means bad data.
    Money(MoneyError),
}

impl From<MoneyError> for CartError {
    fn from(error: MoneyError) -> Self {
        Self::Money(error)
    }
}

pub type Result<T> = core::result::Result<T, CartError>;

/// A sale being built.
#[derive(Debug, Clone, Default)]
pub struct Cart {
    lines: Vec<CartLine>,
    ticket_discount: Discount,
    customer: Option<CustomerId>,
    tenders: Vec<Tender>,
    limits: CartLimits,
    /// Set whenever a limit was exceeded under supervisor authority, so the
    /// ticket carries evidence of who allowed what.
    overrides: Vec<Box<str>>,
}

impl Cart {
    #[must_use]
    pub fn new(limits: CartLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn lines(&self) -> &[CartLine] {
        &self.lines
    }

    #[must_use]
    pub fn tenders(&self) -> &[Tender] {
        &self.tenders
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[must_use]
    pub fn customer(&self) -> Option<CustomerId> {
        self.customer
    }

    pub fn set_customer(&mut self, customer: Option<CustomerId>) {
        self.customer = customer;
    }

    /// Add a scanned or selected item.
    ///
    /// Scanning the same barcode twice increases the quantity of the existing
    /// line rather than stacking duplicates, which is what a cashier expects and
    /// what keeps a fifty item basket readable. A line carrying its own discount
    /// is left alone and a new line is started, because merging would silently
    /// extend that discount to the new units.
    pub fn add_item(&mut self, item: &Item, qty: Milli) -> Result<usize> {
        let mergeable = self.lines.iter().position(|line| {
            line.item_id == item.id
                && line.discount == Discount::None
                && line.unit_price == item.price
        });

        if let Some(index) = mergeable {
            if let Some(line) = self.lines.get_mut(index) {
                line.qty = line.qty.checked_add(qty)?;
            }
            return Ok(index);
        }

        self.lines.push(CartLine {
            item_id: item.id,
            code: item.code.clone(),
            name: item.name_en.clone(),
            unit_price: item.price,
            qty,
            discount: Discount::None,
            vat_rate: item.vat_rate,
            price_mode: item.price_mode,
        });
        Ok(self.lines.len().saturating_sub(1))
    }

    pub fn set_qty(&mut self, index: usize, qty: Milli) -> Result<()> {
        let line = self
            .lines
            .get_mut(index)
            .ok_or(CartError::NoSuchLine { index })?;
        line.qty = qty;
        Ok(())
    }

    pub fn remove_line(&mut self, index: usize) -> Result<CartLine> {
        if index >= self.lines.len() {
            return Err(CartError::NoSuchLine { index });
        }
        Ok(self.lines.remove(index))
    }

    /// Override a line's unit price, if this cashier may.
    pub fn set_unit_price(&mut self, index: usize, price: Minor) -> Result<()> {
        if !self.limits.allow_price_override {
            return Err(CartError::PriceOverrideNotAllowed);
        }
        let line = self
            .lines
            .get_mut(index)
            .ok_or(CartError::NoSuchLine { index })?;
        line.unit_price = price;
        Ok(())
    }

    /// Discount one line, subject to the cashier's ceiling.
    pub fn set_line_discount(&mut self, index: usize, discount: Discount) -> Result<()> {
        self.check_ceiling(discount, index)?;
        let line = self
            .lines
            .get_mut(index)
            .ok_or(CartError::NoSuchLine { index })?;
        line.discount = discount;
        Ok(())
    }

    /// Discount the whole ticket, subject to the same ceiling.
    pub fn set_ticket_discount(&mut self, discount: Discount) -> Result<()> {
        self.check_ceiling(discount, usize::MAX)?;
        self.ticket_discount = discount;
        Ok(())
    }

    /// Record that a supervisor authorised something over the ceiling.
    ///
    /// Raises the limits for the rest of this ticket only. The reason is carried
    /// onto the ticket so the owner can see, later, what was waived and why.
    pub fn authorise_override(&mut self, reason: &str) {
        self.limits = CartLimits::unrestricted();
        self.overrides.push(reason.into());
    }

    pub fn add_tender(&mut self, tender: Tender) {
        self.tenders.push(tender);
    }

    pub fn clear_tenders(&mut self) {
        self.tenders.clear();
    }

    /// What the customer owes.
    pub fn totals(&self) -> Result<TicketTotals> {
        let input = TicketInput {
            lines: self.lines.iter().map(CartLine::as_input).collect(),
            ticket_discount: self.ticket_discount,
        };
        Ok(ticket_totals(&input)?)
    }

    /// Sum of what has been tendered so far.
    pub fn tendered(&self) -> Result<Minor> {
        Ok(Minor::sum(self.tenders.iter().map(|t| t.amount))?)
    }

    /// Still owing, or zero once covered.
    pub fn balance_due(&self) -> Result<Minor> {
        let total = self.totals()?.total;
        let paid = self.tendered()?;
        let due = total.checked_sub(paid)?;
        Ok(if due.is_negative() { Minor::ZERO } else { due })
    }

    /// Change owed back, which is only ever positive on an overpayment in cash.
    pub fn change_due(&self) -> Result<Minor> {
        let total = self.totals()?.total;
        let paid = self.tendered()?;
        let change = paid.checked_sub(total)?;
        Ok(if change.is_negative() { Minor::ZERO } else { change })
    }

    /// Close the sale into an immutable ticket.
    ///
    /// The id, the terminal and the clock are supplied by the caller: this crate
    /// has no clock and mints no identity of its own, so the same code is
    /// deterministic in a test, in a browser and on a phone.
    pub fn close(
        &self,
        id: TicketId,
        terminal: TerminalId,
        rung_at_ms: u64,
    ) -> Result<Ticket> {
        if self.lines.is_empty() {
            return Err(CartError::Empty);
        }
        let totals = self.totals()?;
        let paid = self.tendered()?;
        let shortfall = totals.total.checked_sub(paid)?;
        if shortfall.get() > 0 {
            return Err(CartError::Underpaid { short_by: shortfall });
        }

        Ok(Ticket {
            id,
            terminal,
            rung_at_ms,
            receipt_no: None,
            customer: self.customer,
            lines: self.lines.clone(),
            ticket_discount: self.ticket_discount,
            tenders: self.tenders.clone(),
            totals,
            change: self.change_due()?,
            overrides: self.overrides.clone(),
        })
    }

    fn check_ceiling(&self, discount: Discount, _index: usize) -> Result<()> {
        let Discount::Rate(rate) = discount else {
            // A fixed amount is bounded by the line itself, which pricing already
            // enforces. Only rates are compared against the ceiling.
            return Ok(());
        };
        if rate.get() > self.limits.max_discount.get() {
            return Err(CartError::DiscountAboveCeiling {
                requested: rate.get(),
                ceiling: self.limits.max_discount.get(),
            });
        }
        Ok(())
    }
}

/// A closed sale. Immutable: corrections are new documents, never edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub id: TicketId,
    pub terminal: TerminalId,
    /// Device clock at the moment of sale. Recorded for the receipt and for
    /// ordering within one terminal, never trusted for business ordering.
    pub rung_at_ms: u64,
    /// Assigned from a server-leased block. `None` until one is consumed, which
    /// is why identity and presentation are separate fields.
    pub receipt_no: Option<Box<str>>,
    pub customer: Option<CustomerId>,
    pub lines: Vec<CartLine>,
    pub ticket_discount: Discount,
    pub tenders: Vec<Tender>,
    pub totals: TicketTotals,
    pub change: Minor,
    pub overrides: Vec<Box<str>>,
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
    use crate::money::Bp;
    use crate::replica::Item;

    fn item(seed: u128, price: i64) -> Item {
        Item {
            id: Ulid::from_u128(seed),
            code: "SKU001".into(),
            name_en: "Rice Miniket 5kg".into(),
            name_bn: "মিনিকেট চাল ৫ কেজি".into(),
            unit: "Nos".into(),
            price: Minor::new(price),
            cost: Minor::new(price / 2),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            barcodes: vec!["8690000000012".into()],
            on_hand: Milli::new(40_000),
            active: true,
        }
    }

    fn cashier() -> Cart {
        Cart::new(CartLimits {
            max_discount: Bp::new(1_000).unwrap(),
            allow_price_override: false,
        })
    }

    #[test]
    fn scanning_the_same_item_twice_merges_the_line() {
        let mut cart = cashier();
        let rice = item(1, 43_000);
        cart.add_item(&rice, Milli::ONE).unwrap();
        cart.add_item(&rice, Milli::ONE).unwrap();

        assert_eq!(cart.lines().len(), 1, "a cashier expects one line, quantity two");
        assert_eq!(cart.lines()[0].qty, Milli::new(2_000));
    }

    #[test]
    fn a_discounted_line_does_not_absorb_later_scans() {
        let mut cart = cashier();
        let rice = item(1, 43_000);
        cart.add_item(&rice, Milli::ONE).unwrap();
        cart.set_line_discount(0, Discount::Rate(Bp::new(1_000).unwrap())).unwrap();
        cart.add_item(&rice, Milli::ONE).unwrap();

        assert_eq!(cart.lines().len(), 2, "merging would extend the discount silently");
        assert_eq!(cart.lines()[1].discount, Discount::None);
    }

    #[test]
    fn a_line_keeps_the_price_it_was_added_at() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        // catalogue changes mid-sale, as a sync batch might do
        let repriced = item(1, 99_000);
        assert_eq!(cart.lines()[0].unit_price, Minor::new(43_000));
        assert_ne!(cart.lines()[0].unit_price, repriced.price);
    }

    #[test]
    fn enforces_the_discount_ceiling() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        let too_much = Discount::Rate(Bp::new(2_500).unwrap());
        assert_eq!(
            cart.set_line_discount(0, too_much),
            Err(CartError::DiscountAboveCeiling { requested: 2_500, ceiling: 1_000 })
        );

        cart.authorise_override("manager approved clearance");
        cart.set_line_discount(0, too_much).unwrap();
        assert_eq!(cart.lines()[0].discount, too_much);
    }

    #[test]
    fn refuses_a_price_override_without_permission() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        assert_eq!(
            cart.set_unit_price(0, Minor::new(1)),
            Err(CartError::PriceOverrideNotAllowed)
        );
    }

    #[test]
    fn tracks_the_balance_across_split_tenders() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        let total = cart.totals().unwrap().total;
        assert_eq!(total, Minor::new(49_450));

        cart.add_tender(Tender {
            kind: TenderKind::Wallet("bKash".into()),
            amount: Minor::new(20_000),
            reference: Some("TRX123".into()),
        });
        assert_eq!(cart.balance_due().unwrap(), Minor::new(29_450));
        assert_eq!(cart.change_due().unwrap(), Minor::ZERO);

        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(30_000),
            reference: None,
        });
        assert_eq!(cart.balance_due().unwrap(), Minor::ZERO);
        assert_eq!(cart.change_due().unwrap(), Minor::new(550));
    }

    #[test]
    fn refuses_to_close_underpaid_or_empty() {
        let empty = cashier();
        assert_eq!(
            empty.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::Empty)
        );

        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(1_000),
            reference: None,
        });
        assert_eq!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::Underpaid { short_by: Minor::new(48_450) })
        );
    }

    #[test]
    fn closes_into_an_immutable_ticket() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(50_000),
            reference: None,
        });

        let ticket = cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 1_788_600_000_000).unwrap();
        assert_eq!(ticket.totals.total, Minor::new(49_450));
        assert_eq!(ticket.change, Minor::new(550));
        assert_eq!(ticket.rung_at_ms, 1_788_600_000_000);
        assert!(ticket.receipt_no.is_none(), "the number comes from a lease, later");
    }

    #[test]
    fn carries_overrides_onto_the_ticket_for_review() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.authorise_override("manager approved clearance");
        cart.set_line_discount(0, Discount::Rate(Bp::new(5_000).unwrap())).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(30_000),
            reference: None,
        });

        let ticket = cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0).unwrap();
        assert_eq!(&*ticket.overrides[0], "manager approved clearance");
    }
}
