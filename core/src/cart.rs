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

use crate::domain::{
    Discount, LineInput, PriceMode, Supply, TicketInput, TicketTotals, VatBase, ticket_totals,
};
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
    /// Which amount the rate is charged on, frozen with the price. A line rung
    /// under one rule must not be repriced under another because the back
    /// office changed the item mid-basket.
    pub vat_base: VatBase,
    /// What it was sold by, frozen with the price for the same reason the price
    /// is: an item re-measured from kilos to litres next month must not change
    /// what last week's receipt says was handed over.
    pub unit: Box<str>,
    /// Standard, zero rated or exempt, frozen with the price for the same
    /// reason: what a line was on the day is what the return for that day
    /// declares, whatever the shop reclassifies the item as afterwards.
    pub supply: Supply,
    /// What the shop paid for one of these, frozen with the price.
    ///
    /// Frozen because a margin is a fact about the day the goods were sold: a
    /// sack bought at 380 and sold at 430 made fifty taka, and repricing that
    /// sale next month when the supplier puts the sack up would rewrite a
    /// figure the owner already acted on. Zero where the shop has never said
    /// what it paid, which is most shops on their first week and is a thing to
    /// report rather than to guess at.
    pub cost: Minor,
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
            vat_base: self.vat_base,
            supply: self.supply,
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
    /// No ceiling at all, and prices may be typed over.
    ///
    /// Not what a supervisor gets: a supervisor is capped at what their own
    /// permissions say, which is a fifth off in the preset every shop uses.
    /// This is for a basket nobody is limiting, which is what a test or a demo
    /// wants and what the shop's own rules never ask for.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            max_discount: Bp::new(crate::money::BP_ONE).unwrap_or(Bp::ZERO),
            allow_price_override: true,
        }
    }

    /// What one authorised action lets this basket do.
    ///
    /// A discount raises the ceiling to the rate that was allowed and no
    /// further. A price typed over the catalogue's is not a rate at all, so it
    /// opens that door and leaves the discount ceiling alone.
    #[must_use]
    pub fn allowing(action: crate::auth::Action) -> Self {
        match action {
            crate::auth::Action::Discount { bp } => Self {
                max_discount: Bp::new(bp).unwrap_or(Bp::ZERO),
                allow_price_override: false,
            },
            crate::auth::Action::OverridePrice => Self {
                max_discount: Bp::ZERO,
                allow_price_override: true,
            },
            // Neither is a thing the cart's ceilings know about: selling past
            // the shelf is the till's own rule and the rest go through the auth
            // book. Nothing here is raised for them.
            _ => Self {
                max_discount: Bp::ZERO,
                allow_price_override: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CartError {
    /// No line at that position.
    NoSuchLine { index: usize },
    /// The cart has nothing in it.
    Empty,
    /// A sale line was added to a refund, or a refund line to a sale.
    MixedSaleAndReturn,
    /// A refund was closed without handing over the full amount. Distinct from
    /// `Underpaid`, which is the customer owing the shop.
    RefundNotSettled { outstanding: Minor },
    /// Discount above what this cashier may give.
    DiscountAboveCeiling { requested: u32, ceiling: u32 },
    /// Price override attempted without the permission for it.
    PriceOverrideNotAllowed,
    /// A price below zero, which would make the line pay the customer.
    NegativePrice { price: Minor },
    /// The tendered amounts do not cover the total.
    Underpaid { short_by: Minor },
    /// More was put on an account, a card or a wallet than the basket came to,
    /// and there is not enough cash in the tender to give the difference back.
    ///
    /// Change is banknotes. A promise cannot make them, and neither can a card:
    /// a till that answered "change 100" here would have a cashier hand real
    /// money out of the drawer against a debt the customer now also owes. The
    /// tender is taken back and entered again, which is two presses and the
    /// only honest answer.
    ChangeFromAPromise { over_by: Minor, cash: Minor },
    /// Arithmetic went wrong, which for realistic baskets means bad data.
    Money(MoneyError),
}

impl CartError {
    /// A stable name for this refusal. See `TillError::code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSuchLine { .. } => "no-such-line",
            Self::Empty => "empty-basket",
            Self::MixedSaleAndReturn => "mixed-sale-and-return",
            Self::RefundNotSettled { .. } => "refund-not-settled",
            Self::DiscountAboveCeiling { .. } => "discount-above-ceiling",
            Self::PriceOverrideNotAllowed => "price-override-not-allowed",
            Self::NegativePrice { .. } => "negative-price",
            Self::Underpaid { .. } => "underpaid",
            Self::ChangeFromAPromise { .. } => "change-from-a-promise",
            Self::Money(_) => "money",
        }
    }
}

impl From<MoneyError> for CartError {
    fn from(error: MoneyError) -> Self {
        Self::Money(error)
    }
}

impl core::fmt::Display for CartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoSuchLine { index } => write!(f, "there is no line {index} on this ticket"),
            Self::Empty => f.write_str("the basket is empty"),
            Self::MixedSaleAndReturn => {
                f.write_str("a sale line and a return line cannot share one ticket")
            }
            Self::RefundNotSettled { outstanding } => write!(
                f,
                "the refund is out by {} minor units and must balance exactly",
                outstanding.get()
            ),
            // As rates. This sentence is the fallback a screen shows when it
            // has no words of its own for the refusal, so it is read by
            // somebody standing at a counter, and nobody there reads basis
            // points.
            Self::DiscountAboveCeiling { requested, ceiling } => write!(
                f,
                "a discount of {} is above this cashier's ceiling of {}",
                crate::receipt::rate_of(*requested),
                crate::receipt::rate_of(*ceiling)
            ),
            Self::PriceOverrideNotAllowed => {
                f.write_str("this cashier may not type a price over the catalogue's")
            }
            Self::NegativePrice { price } => {
                write!(f, "a price of {} minor units is below zero", price.get())
            }
            Self::ChangeFromAPromise { over_by, cash } => write!(
                f,
                "{} more than the basket was put on an account or a card, and only {} was in cash: \
                 change cannot come out of a promise",
                over_by.get(),
                cash.get()
            ),
            Self::Underpaid { short_by } => {
                write!(f, "short by {} minor units", short_by.get())
            }
            Self::Money(error) => write!(f, "{error}"),
        }
    }
}

impl core::error::Error for CartError {}

pub type Result<T> = core::result::Result<T, CartError>;

/// Whether this ticket takes money or gives it back.
///
/// Kept as explicit state rather than inferred from the sign of the lines,
/// because an empty refund and an empty sale look identical and the cashier has
/// already told the till which one they are doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Sale,
    /// Goods coming back. The original receipt number is recorded when the
    /// customer has it, and is deliberately optional: a shop that refuses a
    /// refund because the paper was lost will simply lose the customer, and the
    /// permission to do this at all is governed by the cashier's limits.
    Refund { original_receipt: Option<Box<str>> },
}

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
    direction: Direction,
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

    #[must_use]
    pub fn direction(&self) -> &Direction {
        &self.direction
    }

    #[must_use]
    pub fn ticket_discount(&self) -> Discount {
        self.ticket_discount
    }

    /// Put a line back exactly as it was, when a parked basket is resumed.
    ///
    /// Deliberately not `add_item`: that reprices from the catalogue and merges
    /// with matching lines, both of which would change a basket the cashier has
    /// already quoted. A resumed ticket must be the ticket that was parked.
    pub fn restore_line(&mut self, line: CartLine) {
        self.lines.push(line);
    }

    /// Put back a discount the whole basket already carried.
    ///
    /// Not `set_ticket_discount`, for the reason `restore_line` is not
    /// `add_item`: this basket was already priced and somebody already allowed
    /// what is on it. Checking it against the ceiling of whoever happens to be
    /// at the till now refuses a basket a supervisor approved an hour ago, and
    /// the cashier who resumed it is not the person who can approve it again.
    pub fn restore_ticket_discount(&mut self, discount: Discount) {
        self.ticket_discount = discount;
    }

    /// Put back the notes a resumed basket already carried.
    ///
    /// What was waived belongs on the customer's paper and in the shop's copy,
    /// and a basket that went through a supervisor before it was parked has to
    /// come back saying so. It did not: the notes were left behind, so the one
    /// line explaining why the price differs from the shelf was missing from
    /// exactly the sales that had a reason for it.
    pub fn restore_overrides(&mut self, notes: Vec<Box<str>>) {
        self.overrides = notes;
    }

    /// Bring back a refund that was parked as a refund.
    ///
    /// `start_refund` refuses once anything is rung, which is right at a
    /// counter and wrong here: the lines being restored are the parked refund's
    /// own. Without this a parked refund came back as a sale with negative
    /// lines on it, which is money going the wrong way with nothing on the
    /// screen to say so.
    pub fn restore_refund(&mut self, original_receipt: Option<&str>) {
        self.direction = Direction::Refund {
            original_receipt: original_receipt.map(Into::into),
        };
    }

    /// The receipt a refund is against, when it named one.
    #[must_use]
    pub fn refund_of(&self) -> Option<&str> {
        match &self.direction {
            Direction::Refund { original_receipt } => original_receipt.as_deref(),
            Direction::Sale => None,
        }
    }

    /// The notes this basket carries, for parking it.
    #[must_use]
    pub fn overrides(&self) -> &[Box<str>] {
        &self.overrides
    }

    #[must_use]
    pub fn is_refund(&self) -> bool {
        matches!(self.direction, Direction::Refund { .. })
    }

    /// Turn this ticket into a refund.
    ///
    /// Only possible while the cart is empty. A basket half rung as a sale
    /// cannot be reinterpreted as a refund without silently changing what every
    /// line already on it means.
    pub fn start_refund(&mut self, original_receipt: Option<&str>) -> Result<()> {
        if !self.lines.is_empty() {
            return Err(CartError::MixedSaleAndReturn);
        }
        self.direction = Direction::Refund {
            original_receipt: original_receipt.map(Into::into),
        };
        Ok(())
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
        // A refund's lines are the negative of the same goods sold. Doing this
        // here rather than asking callers to pass a negative quantity means a
        // scanner, which only ever reports one of something, works unchanged in
        // both directions.
        let qty = if self.is_refund() {
            Milli::new(qty.get().checked_neg().ok_or(MoneyError::Overflow)?)
        } else {
            qty
        };

        // A ticket that mixes directions has an ambiguous total and an
        // unreportable tax position: the shop cannot say whether it took money
        // or gave it back. Exchanges are two tickets, which is also what the
        // paper trail should show.
        let mixes = self
            .lines
            .iter()
            .any(|line| line.qty.is_negative() != qty.is_negative());
        if mixes {
            return Err(CartError::MixedSaleAndReturn);
        }

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
            unit: item.unit.clone(),
            item_id: item.id,
            code: item.code.clone(),
            name: item.name_en.clone(),
            unit_price: item.price,
            qty,
            discount: Discount::None,
            vat_rate: item.vat_rate,
            price_mode: item.price_mode,
            vat_base: item.vat_base,
            supply: item.supply,
            cost: item.cost,
        });
        Ok(self.lines.len().saturating_sub(1))
    }

    /// Change a line's quantity, in the direction the ticket is already going.
    ///
    /// The sign is checked for the same reason `add_item` checks it. Setting a
    /// line on a sale to a negative quantity made the ticket total negative,
    /// which passed the underpaid check trivially with no tender at all, and
    /// then handed the customer the whole amount as "change": a refund with no
    /// refund permission, no original receipt, and cash out of the drawer
    /// recorded as change given.
    pub fn set_qty(&mut self, index: usize, qty: Milli) -> Result<()> {
        if qty.is_negative() != self.is_refund() && qty != Milli::ZERO {
            return Err(CartError::MixedSaleAndReturn);
        }
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
        if price.is_negative() {
            // A negative price is not a discount, it is the till paying the
            // customer to take the goods. Supervisors mistype, and the ticket
            // arithmetic would carry it through without complaint.
            return Err(CartError::NegativePrice { price });
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
    /// Raises the limits to what was allowed, for the rest of this ticket, and
    /// no further. It used to raise them to everything, which meant a fifteen
    /// percent discount approved by a supervisor left the basket able to take
    /// ninety: the trail said "Karim allowed a discount of 1500 basis points"
    /// and the customer walked out with the rest, so the one record a shop has
    /// of what was waived described something that did not happen. Found by
    /// review, not by a test, because every test asked for one discount.
    ///
    /// A ceiling already higher than what was allowed is left where it is. The
    /// supervisor is adding permission, not taking any away, and a cashier who
    /// may give ten percent unaided does not lose that because somebody
    /// approved five.
    ///
    /// The reason is carried onto the ticket so the owner can see, later, what
    /// was waived and why.
    pub fn authorise_override(&mut self, reason: &str, allowed: CartLimits) {
        self.limits = CartLimits {
            max_discount: if allowed.max_discount.get() > self.limits.max_discount.get() {
                allowed.max_discount
            } else {
                self.limits.max_discount
            },
            allow_price_override: self.limits.allow_price_override || allowed.allow_price_override,
        };
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

    /// Whether taking this tender would put more than the basket on something
    /// that cannot hand change back.
    ///
    /// Checked when the tender is offered rather than only when the sale
    /// closes, because this is the moment the cashier can still see what they
    /// typed. A discount given after the tender can still make an overpayment
    /// out of one that was fine, which is why `close` checks it too.
    pub fn would_overpay(&self, tender: &Tender) -> Result<()> {
        if self.is_refund() {
            // A refund balances exactly and is checked as a whole at the close.
            // Its tenders are negative, and "more than the basket" is a
            // different question there.
            return Ok(());
        }
        let total = self.totals()?.total;
        let paid = self.tendered()?.checked_add(tender.amount)?;
        let over = paid.checked_sub(total)?;
        if over.get() <= 0 {
            return Ok(());
        }
        let mut cash = self.cash_tendered()?;
        if tender.kind == TenderKind::Cash {
            cash = cash.checked_add(tender.amount)?;
        }
        if over.get() > cash.get() {
            return Err(CartError::ChangeFromAPromise {
                over_by: over,
                cash,
            });
        }
        Ok(())
    }

    /// Change owed back, which is only ever what an overpayment in cash leaves.
    ///
    /// Capped at the cash tendered on purpose. An over-tender on an account, a
    /// card or a wallet is not change: handing banknotes back for it takes real
    /// money out of the drawer against a promise, and leaves the customer owing
    /// for it as well. What is over that cap stops the sale at `close` rather
    /// than being quietly dropped here.
    pub fn change_due(&self) -> Result<Minor> {
        let total = self.totals()?.total;
        let paid = self.tendered()?;
        let over = paid.checked_sub(total)?;
        if over.is_negative() {
            return Ok(Minor::ZERO);
        }
        let cash = self.cash_tendered()?;
        Ok(if over.get() > cash.get() { cash } else { over })
    }

    /// What of the tender was actual money.
    fn cash_tendered(&self) -> Result<Minor> {
        Ok(Minor::sum(
            self.tenders
                .iter()
                .filter(|tender| tender.kind == TenderKind::Cash)
                .map(|tender| tender.amount),
        )?)
    }

    /// Close the sale into an immutable ticket.
    ///
    /// The id, the terminal and the clock are supplied by the caller: this crate
    /// has no clock and mints no identity of its own, so the same code is
    /// deterministic in a test, in a browser and on a phone.
    pub fn close(&self, id: TicketId, terminal: TerminalId, rung_at_ms: u64) -> Result<Ticket> {
        if self.lines.is_empty() {
            return Err(CartError::Empty);
        }
        let totals = self.totals()?;
        let paid = self.tendered()?;
        let shortfall = totals.total.checked_sub(paid)?;

        if self.is_refund() {
            // A refund must balance exactly. The sale path tolerates
            // overpayment because the difference is change handed back, but
            // there is no such thing as giving a customer too much of their own
            // money by accident: any difference here is money leaving the shop
            // unaccounted for. Note that the sale check alone would pass a
            // refund with no tender at all, because a negative shortfall is not
            // greater than zero.
            if shortfall != Minor::ZERO {
                return Err(CartError::RefundNotSettled {
                    outstanding: shortfall,
                });
            }
        } else if shortfall.get() > 0 {
            return Err(CartError::Underpaid {
                short_by: shortfall,
            });
        } else {
            // Over by more than there is cash to give back. Something other
            // than money was over-tendered, and the difference cannot leave the
            // drawer: it would be the shop handing out banknotes against a debt
            // the customer still owes.
            let over = paid.checked_sub(totals.total)?;
            let cash = self.cash_tendered()?;
            if over.get() > cash.get() {
                return Err(CartError::ChangeFromAPromise {
                    over_by: over,
                    cash,
                });
            }
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
            change: if self.is_refund() {
                Minor::ZERO
            } else {
                self.change_due()?
            },
            overrides: self.overrides.clone(),
            direction: self.direction.clone(),
        })
    }

    fn check_ceiling(&self, discount: Discount, index: usize) -> Result<()> {
        let rate = match discount {
            Discount::None => return Ok(()),
            Discount::Rate(rate) => rate.get(),
            // What that amount is, as a share of what it comes off. A ceiling
            // that only looked at rates was a ceiling a cashier walked around by
            // naming an amount: "bounded by the line itself" is a bound of a
            // hundred percent, which is not what the shop set.
            Discount::Amount(off) => self.as_rate_of(off, index)?,
        };
        if rate > self.limits.max_discount.get() {
            return Err(CartError::DiscountAboveCeiling {
                requested: rate,
                ceiling: self.limits.max_discount.get(),
            });
        }
        Ok(())
    }

    /// An amount off, in basis points of what it is off.
    ///
    /// Rounded up, because this decides whether somebody is allowed to give
    /// money away: a hair over the ceiling is over it. Against the line's own
    /// gross, or the whole ticket's when the discount is the ticket's, which is
    /// the same basis the price on the screen is quoted in.
    fn as_rate_of(&self, off: Minor, index: usize) -> Result<u32> {
        let totals = self.totals()?;
        let base = if index == usize::MAX {
            Minor::sum(totals.lines.iter().map(|line| line.gross))?
        } else {
            totals
                .lines
                .get(index)
                .ok_or(CartError::NoSuchLine { index })?
                .gross
        };
        // Nothing to come off is everything off, which no ceiling below a
        // hundred percent allows. A refund's negative gross reads the same way:
        // its own amount is the thing being compared, and the sign belongs to
        // the goods rather than to the permission.
        let (off, base) = (i128::from(off.get()).abs(), i128::from(base.get()).abs());
        if base == 0 {
            return Ok(crate::money::BP_ONE);
        }
        let scaled = off
            .checked_mul(i128::from(crate::money::BP_ONE))
            .ok_or(MoneyError::Overflow)?;
        let whole = scaled.div_euclid(base);
        let bp = if scaled.rem_euclid(base) == 0 {
            whole
        } else {
            whole.saturating_add(1)
        };
        Ok(u32::try_from(bp).unwrap_or(u32::MAX))
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
    /// Whether this ticket took money or gave it back, and against which
    /// receipt if the customer had one.
    pub direction: Direction,
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
            vat_base: VatBase::Discounted,
            barcodes: vec!["8690000000012".into()],
            on_hand: Milli::new(40_000),
            active: true,
            supply: crate::domain::Supply::Standard,
            category: "".into(),
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

        assert_eq!(
            cart.lines().len(),
            1,
            "a cashier expects one line, quantity two"
        );
        assert_eq!(cart.lines()[0].qty, Milli::new(2_000));
    }

    #[test]
    fn a_discounted_line_does_not_absorb_later_scans() {
        let mut cart = cashier();
        let rice = item(1, 43_000);
        cart.add_item(&rice, Milli::ONE).unwrap();
        cart.set_line_discount(0, Discount::Rate(Bp::new(1_000).unwrap()))
            .unwrap();
        cart.add_item(&rice, Milli::ONE).unwrap();

        assert_eq!(
            cart.lines().len(),
            2,
            "merging would extend the discount silently"
        );
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
            Err(CartError::DiscountAboveCeiling {
                requested: 2_500,
                ceiling: 1_000
            })
        );

        cart.authorise_override(
            "manager approved clearance",
            CartLimits::allowing(crate::auth::Action::Discount { bp: 2_500 }),
        );
        cart.set_line_discount(0, too_much).unwrap();
        assert_eq!(cart.lines()[0].discount, too_much);

        // And no further. What was allowed was a quarter off; the basket does
        // not become one anybody may empty. The trail records the figure the
        // supervisor approved, and a basket that could then take ninety percent
        // would leave that record describing something that did not happen.
        assert_eq!(
            cart.set_line_discount(0, Discount::Rate(Bp::new(2_501).unwrap())),
            Err(CartError::DiscountAboveCeiling {
                requested: 2_501,
                ceiling: 2_500
            })
        );
    }

    /// The same ceiling, against an amount rather than a rate.
    ///
    /// A ceiling that only looked at rates was a ceiling a cashier walked around
    /// by naming an amount: the old comment said a fixed amount is "bounded by
    /// the line itself", which is a bound of a hundred percent and not what the
    /// shop set. Nothing reached it, because no screen offered an amount and no
    /// command carried one, which is the only reason this was not money going
    /// out of a shop.
    #[test]
    fn an_amount_off_is_measured_against_the_same_ceiling() {
        let mut cart = cashier();
        // Four hundred and thirty taka, and a cashier who may give ten percent.
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();

        cart.set_line_discount(0, Discount::Amount(Minor::new(4_300)))
            .expect("ten percent of it, to the poisha");
        assert_eq!(
            cart.set_line_discount(0, Discount::Amount(Minor::new(4_301))),
            Err(CartError::DiscountAboveCeiling {
                requested: 1_001,
                ceiling: 1_000
            }),
            "a hair over the ceiling is over it, because this is money going out"
        );

        // And a supervisor lifts it, the same way they lift a rate. An amount
        // is measured as a share of what it comes off, so what has to be
        // allowed is that share: ten thousand off a line of forty-three
        // thousand is a shade over a quarter.
        cart.authorise_override(
            "manager approved a hundred taka off",
            CartLimits::allowing(crate::auth::Action::Discount { bp: 2_500 }),
        );
        cart.set_line_discount(0, Discount::Amount(Minor::new(10_000)))
            .unwrap();
        assert_eq!(
            cart.lines()[0].discount,
            Discount::Amount(Minor::new(10_000))
        );
    }

    /// Two lines, and an amount off the whole basket.
    #[test]
    fn an_amount_off_the_ticket_is_measured_against_the_whole_basket() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.add_item(&item(2, 37_000), Milli::ONE).unwrap();

        // Eight hundred taka in the basket, so eighty is the ten percent this
        // cashier may give.
        cart.set_ticket_discount(Discount::Amount(Minor::new(8_000)))
            .expect("ten percent of the basket");
        assert_eq!(
            cart.set_ticket_discount(Discount::Amount(Minor::new(8_001))),
            Err(CartError::DiscountAboveCeiling {
                requested: 1_001,
                ceiling: 1_000
            })
        );
    }

    /// An empty basket, where an amount off is everything off.
    #[test]
    fn an_amount_off_nothing_is_everything_off() {
        let mut cart = cashier();
        assert_eq!(
            cart.set_ticket_discount(Discount::Amount(Minor::new(100))),
            Err(CartError::DiscountAboveCeiling {
                requested: crate::money::BP_ONE,
                ceiling: 1_000
            }),
            "nothing to come off is not a licence to give a hundred taka away"
        );
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
            Err(CartError::Underpaid {
                short_by: Minor::new(48_450)
            })
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

        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(1), 1_788_600_000_000)
            .unwrap();
        assert_eq!(ticket.totals.total, Minor::new(49_450));
        assert_eq!(ticket.change, Minor::new(550));
        assert_eq!(ticket.rung_at_ms, 1_788_600_000_000);
        assert!(
            ticket.receipt_no.is_none(),
            "the number comes from a lease, later"
        );
    }

    #[test]
    fn change_never_comes_out_of_a_promise() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        // "Put six hundred on my account" for a basket of 494.50, or a cashier
        // typing one digit too many. A till that answered "change 105.50" would
        // have somebody hand real money out of the drawer against a debt the
        // customer is now also carrying.
        cart.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(60_000),
            reference: Some("Karim".into()),
        });
        assert_eq!(cart.change_due().unwrap(), Minor::ZERO);
        assert_eq!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::ChangeFromAPromise {
                over_by: Minor::new(10_550),
                cash: Minor::ZERO,
            })
        );
    }

    #[test]
    fn change_comes_out_of_the_note_that_was_handed_over() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        // Four hundred on the account and a hundred taka note: the basket is
        // 494.50, so 5.50 goes back, and it comes from the note.
        cart.add_tender(Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(40_000),
            reference: Some("Karim".into()),
        });
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(10_000),
            reference: None,
        });
        assert_eq!(cart.change_due().unwrap(), Minor::new(550));
        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(1), 0)
            .expect("a hundred taka covers the change");
        assert_eq!(ticket.change, Minor::new(550));
    }

    #[test]
    fn a_card_cannot_make_change_either() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        // Charging a card for more than the basket and handing the difference
        // over in notes is somebody's cash advance, not a sale.
        cart.add_tender(Tender {
            kind: TenderKind::Card,
            amount: Minor::new(50_000),
            reference: None,
        });
        assert!(matches!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::ChangeFromAPromise { .. })
        ));
    }

    #[test]
    fn a_refund_negates_what_is_scanned() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some("T1-000100")).unwrap();
        // The scanner reports one of something, exactly as it does for a sale.
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();

        assert!(cart.is_refund());
        assert_eq!(cart.lines()[0].qty, Milli::new(-1_000));
        assert_eq!(cart.totals().unwrap().total, Minor::new(-49_450));
    }

    #[test]
    fn a_refund_is_the_exact_mirror_of_the_sale() {
        let mut sale = Cart::new(CartLimits::unrestricted());
        sale.add_item(&item(1, 43_000), Milli::new(3_000)).unwrap();
        sale.set_line_discount(0, Discount::Rate(Bp::new(1_000).unwrap()))
            .unwrap();

        let mut refund = Cart::new(CartLimits::unrestricted());
        refund.start_refund(Some("T1-000100")).unwrap();
        refund
            .add_item(&item(1, 43_000), Milli::new(3_000))
            .unwrap();
        refund
            .set_line_discount(0, Discount::Rate(Bp::new(1_000).unwrap()))
            .unwrap();

        assert_eq!(
            refund.totals().unwrap().total.get(),
            -sale.totals().unwrap().total.get(),
            "a discounted item returned must refund the discounted price"
        );
    }

    #[test]
    fn a_ticket_cannot_mix_a_sale_and_a_refund() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();

        // Switching direction mid-basket would reinterpret what is already rung.
        assert_eq!(cart.start_refund(None), Err(CartError::MixedSaleAndReturn));

        // And an exchange is two tickets, which is what the paper trail should
        // show anyway.
        let mut refund = Cart::new(CartLimits::unrestricted());
        refund.start_refund(None).unwrap();
        refund.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        refund.direction = Direction::Sale;
        assert_eq!(
            refund.add_item(&item(2, 10_000), Milli::ONE),
            Err(CartError::MixedSaleAndReturn)
        );
    }

    #[test]
    fn a_refund_must_be_handed_over_in_full() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some("T1-000100")).unwrap();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();

        // The sale path's check alone would let this through, because a
        // negative shortfall is not greater than zero. That would close a refund
        // having given the customer nothing.
        assert_eq!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::RefundNotSettled {
                outstanding: Minor::new(-49_450)
            })
        );

        // Paying out too little is refused as well.
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-40_000),
            reference: None,
        });
        assert!(matches!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::RefundNotSettled { .. })
        ));

        cart.clear_tenders();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-49_450),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(1), 0)
            .unwrap();
        assert_eq!(ticket.totals.total, Minor::new(-49_450));
        assert_eq!(ticket.change, Minor::ZERO, "a refund gives no change");
        assert_eq!(
            ticket.direction,
            Direction::Refund {
                original_receipt: Some("T1-000100".into())
            }
        );
    }

    #[test]
    fn paying_out_too_much_on_a_refund_is_refused() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(None).unwrap();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(-60_000),
            reference: None,
        });

        // There is no such thing as accidentally giving a customer too much of
        // their own money: the difference is simply money leaving the shop.
        assert_eq!(
            cart.close(Ulid::from_u128(9), Ulid::from_u128(1), 0),
            Err(CartError::RefundNotSettled {
                outstanding: Minor::new(10_550)
            })
        );
    }

    #[test]
    fn carries_overrides_onto_the_ticket_for_review() {
        let mut cart = cashier();
        cart.add_item(&item(1, 43_000), Milli::ONE).unwrap();
        cart.authorise_override(
            "manager approved clearance",
            CartLimits::allowing(crate::auth::Action::Discount { bp: 5_000 }),
        );
        cart.set_line_discount(0, Discount::Rate(Bp::new(5_000).unwrap()))
            .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(30_000),
            reference: None,
        });

        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(1), 0)
            .unwrap();
        assert_eq!(&*ticket.overrides[0], "manager approved clearance");
    }

    #[test]
    fn a_negative_quantity_cannot_be_smuggled_onto_a_sale() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(1, 49_450), Milli::ONE).unwrap();

        // Without the sign check this made the ticket total negative, the
        // underpaid check passed with no tender at all, and close() handed the
        // whole amount over as "change": a refund with no permission, no
        // original receipt and no record that money left the drawer.
        assert_eq!(
            cart.set_qty(0, Milli::new(-1_000)),
            Err(CartError::MixedSaleAndReturn)
        );
    }

    #[test]
    fn a_price_override_cannot_go_below_zero() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(1, 49_450), Milli::ONE).unwrap();

        assert_eq!(
            cart.set_unit_price(0, Minor::new(-100)),
            Err(CartError::NegativePrice {
                price: Minor::new(-100)
            })
        );
    }

    #[test]
    fn a_negative_price_on_a_positive_quantity_is_refused() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        let mut underwater = item(1, 49_450);
        underwater.price = Minor::new(-1_000);
        cart.add_item(&underwater, Milli::ONE).unwrap();

        // The arithmetic used to reject this only when the quantity was also
        // negative, so a positive quantity carried it through as a covert refund
        // that no report would ever call one.
        assert!(cart.totals().is_err());
    }
}
