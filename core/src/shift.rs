//! One drawer's trading period, and the two reports it produces.
//!
//! A shift belongs to a terminal, never to a shop. A shop-wide open shift is a
//! single mutable row, and two offline terminals closing it is the one write
//! conflict an append-only ledger cannot absorb: both closes are honest, both
//! carry a different counted total, and no rule picks a winner after the fact.
//! Each drawer owns its own shift instead, and the shop-level Z report is an
//! aggregation over terminals computed in the back office.
//!
//! Nothing here reads a clock or invents an identifier. The id and every
//! timestamp arrive from the caller, which is what keeps this crate identical
//! in a test, in a browser and on a phone.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::cart::{Tender, TenderKind, TerminalId};
use crate::ids::Ulid;
use crate::money::{Minor, MoneyError};

pub type ShiftId = Ulid;

/// Whether a tender of this kind is money the cashier can physically be short
/// of at the end of the day.
///
/// This is the distinction that makes a drawer count meaningful at all. A bKash
/// payment is revenue and belongs in the report, but it never entered the till,
/// so counting it into the expected cash would manufacture a variance at every
/// close and train the shop to ignore the number.
#[must_use]
pub fn lands_in_drawer(kind: &TenderKind) -> bool {
    matches!(kind, TenderKind::Cash)
}

/// Which way cash crossed the drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CashDirection {
    /// A float top-up, a change fund, money put back in.
    In,
    /// A drop to the safe, a supplier paid from the till.
    Out,
}

/// Cash that moved for a reason other than a sale.
///
/// The reason is mandatory and free text. An unexplained movement is
/// indistinguishable from theft when the variance is read a week later, so the
/// type refuses to let one be recorded without an explanation attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CashMovement {
    pub direction: CashDirection,
    /// Always non-negative. The direction carries the sign, so a movement can
    /// never be read backwards by a report that forgot to check it.
    pub amount: Minor,
    pub reason: Box<str>,
    /// Device clock when the cash moved, supplied by the caller. Recorded so a
    /// variance can be placed against the hour it happened, never trusted for
    /// ordering across terminals.
    pub at_ms: u64,
}

/// Gross taken under one tender kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenderTotal {
    pub kind: TenderKind,
    pub amount: Minor,
    /// Carried on the row rather than recomputed by each consumer, so a report
    /// renderer cannot quietly decide that a wallet counts as cash.
    pub in_drawer: bool,
}

/// An open drawer as something that can be written down and put back.
///
/// Every field the shift itself holds while it is open, and nothing about
/// closing: a closed drawer is a different record with a count and a name on
/// it. See `Shift::what_it_holds`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDrawer {
    pub id: ShiftId,
    pub terminal: TerminalId,
    pub opened_at_ms: u64,
    pub opening_float: Minor,
    pub sales: usize,
    pub tender_totals: Vec<TenderTotal>,
    pub cash_sales: Minor,
    pub cash_in_total: Minor,
    pub cash_out_total: Minor,
    pub movements: Vec<CashMovement>,
}

/// Everything the shift knows so far, without ending it.
///
/// A cashier checks this mid-shift to see whether the drawer already disagrees
/// with the till, while there is still a day left to find out why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XReport {
    pub shift: ShiftId,
    pub terminal: TerminalId,
    pub opened_at_ms: u64,
    pub opening_float: Minor,
    /// Sales rung into this shift, not lines and not tenders.
    pub sales: usize,
    /// One row per distinct tender kind, in the order each was first taken.
    pub tenders: Vec<TenderTotal>,
    /// Taken in cash, so it is in the till.
    pub cash_sales: Minor,
    /// Taken by wallet, card or on account, so it is revenue that never reached
    /// the drawer.
    pub non_cash_sales: Minor,
    pub cash_in: Minor,
    pub cash_out: Minor,
    /// Float plus cash sales plus cash in, less cash out. What the drawer should
    /// hold if nothing has gone wrong.
    pub expected_cash: Minor,
}

/// The close: the X totals as they stood, plus what was actually counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZReport {
    pub totals: XReport,
    pub closed_at_ms: u64,
    pub counted_cash: Minor,
    /// Counted less expected. Negative means the drawer is short, which is a
    /// fact to report and act on rather than an error to refuse: a shift that
    /// could not be closed short would simply be closed dishonestly.
    pub variance: Minor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftError {
    /// A sale or a movement arrived after the drawer was counted. Accepting it
    /// would change a total that has already been reported and signed off.
    AlreadyClosed {
        closed_at_ms: u64,
    },
    /// A Z report was asked for while the shift is still trading.
    StillOpen,
    /// A negative float, movement or drawer count. Direction is expressed by
    /// the operation, so a negative amount is always a caller mistake.
    NegativeAmount {
        amount: Minor,
    },
    /// Cash crossed the drawer with nothing said about why.
    ///
    /// The one entry on a drawer that money leaves by without a sale behind it.
    /// A hundred taka out with no reason beside it is indistinguishable from
    /// theft when the count comes up short, and the person who has to answer
    /// for the drawer is not the person who took it.
    NoReason,
    Money(MoneyError),
}

impl ShiftError {
    /// A stable name for this refusal. See `TillError::code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::AlreadyClosed { .. } => "drawer-already-closed",
            Self::StillOpen => "drawer-still-open",
            Self::NegativeAmount { .. } => "negative-amount",
            Self::NoReason => "no-reason",
            Self::Money(_) => "money",
        }
    }
}

impl fmt::Display for ShiftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReason => f.write_str(
                "say what the money was for: a movement with no reason \
                 beside it is one nobody can answer for later",
            ),
            Self::AlreadyClosed { closed_at_ms } => {
                write!(
                    f,
                    "the shift was closed at {closed_at_ms} and takes no more entries"
                )
            }
            Self::StillOpen => write!(f, "the shift is still open, so there is no Z report yet"),
            Self::NegativeAmount { amount } => {
                write!(f, "amount {} minor units is negative", amount.get())
            }
            Self::Money(error) => write!(f, "{error}"),
        }
    }
}

impl core::error::Error for ShiftError {}

impl From<MoneyError> for ShiftError {
    fn from(error: MoneyError) -> Self {
        Self::Money(error)
    }
}

pub type Result<T> = core::result::Result<T, ShiftError>;

/// What the close recorded, kept so the Z report can be reprinted without
/// re-counting the drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Closing {
    counted_cash: Minor,
    closed_at_ms: u64,
    variance: Minor,
}

/// A terminal's trading period: what it opened with, what it took, and what
/// crossed the drawer in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shift {
    id: ShiftId,
    terminal: TerminalId,
    opened_at_ms: u64,
    opening_float: Minor,
    sales: usize,
    tender_totals: Vec<TenderTotal>,
    cash_sales: Minor,
    cash_in_total: Minor,
    cash_out_total: Minor,
    movements: Vec<CashMovement>,
    closing: Option<Closing>,
}

impl Shift {
    /// Open a drawer with a counted float.
    ///
    /// The float is counted by a person, so it can be zero: a terminal that
    /// takes wallet payments only starts its day with an empty till and must
    /// still be able to open. It cannot be negative, because that is not a
    /// quantity of notes anybody counted.
    pub fn open(
        id: ShiftId,
        terminal: TerminalId,
        opening_float: Minor,
        opened_at_ms: u64,
    ) -> Result<Self> {
        if opening_float.is_negative() {
            return Err(ShiftError::NegativeAmount {
                amount: opening_float,
            });
        }
        Ok(Self {
            id,
            terminal,
            opened_at_ms,
            opening_float,
            sales: 0,
            tender_totals: Vec::new(),
            cash_sales: Minor::ZERO,
            cash_in_total: Minor::ZERO,
            cash_out_total: Minor::ZERO,
            movements: Vec::new(),
            closing: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> ShiftId {
        self.id
    }

    #[must_use]
    pub fn terminal(&self) -> TerminalId {
        self.terminal
    }

    #[must_use]
    pub fn opened_at_ms(&self) -> u64 {
        self.opened_at_ms
    }

    #[must_use]
    pub fn opening_float(&self) -> Minor {
        self.opening_float
    }

    #[must_use]
    pub fn sales(&self) -> usize {
        self.sales
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.closing.is_none()
    }

    #[must_use]
    pub fn closed_at_ms(&self) -> Option<u64> {
        self.closing.map(|closing| closing.closed_at_ms)
    }

    /// Every non-sale movement, in the order it happened. The audit trail a
    /// variance is read against.
    #[must_use]
    pub fn movements(&self) -> &[CashMovement] {
        &self.movements
    }

    /// What this drawer holds, for a caller that has to write it down.
    ///
    /// A drawer that is open lives in the critical log: the frames that opened
    /// it, the cash that moved, and every sale rung under it. Replaying them is
    /// what makes the drawer figure and the sales figure agree by construction,
    /// and that is the right way round while the log is there.
    ///
    /// The log is emptied once the shop has taken every sale in it, and it
    /// cannot be emptied under a drawer that only exists inside it: a shop that
    /// never counts its drawer never lets a byte go. So the drawer is written
    /// down at the moment the log is dropped, with the sequence it was folded
    /// through, and the next boot starts from it and replays what came after.
    /// That is the same shape as the catalogue's snapshot and its delta log,
    /// and it keeps the agreement the replay gave: what is written down is a
    /// checkpoint of the replay, not a second opinion about it.
    #[must_use]
    pub fn what_it_holds(&self) -> OpenDrawer {
        OpenDrawer {
            id: self.id,
            terminal: self.terminal,
            opened_at_ms: self.opened_at_ms,
            opening_float: self.opening_float,
            sales: self.sales,
            tender_totals: self.tender_totals.clone(),
            cash_sales: self.cash_sales,
            cash_in_total: self.cash_in_total,
            cash_out_total: self.cash_out_total,
            movements: self.movements.clone(),
        }
    }

    /// Put one back, as it was written down. Always open: a closed drawer is
    /// written down as a `ClosedShift` and is a different record.
    #[must_use]
    pub fn as_it_was(held: OpenDrawer) -> Self {
        Self {
            id: held.id,
            terminal: held.terminal,
            opened_at_ms: held.opened_at_ms,
            opening_float: held.opening_float,
            sales: held.sales,
            tender_totals: held.tender_totals,
            cash_sales: held.cash_sales,
            cash_in_total: held.cash_in_total,
            cash_out_total: held.cash_out_total,
            movements: held.movements,
            closing: None,
        }
    }

    /// Record a closed sale by the tenders that paid for it and the change
    /// given back.
    ///
    /// The shift is handed tenders rather than a ticket because that is the
    /// only part of a sale a drawer has an opinion about, and because a return
    /// settled in cash is the same event with a negative cash tender: it leaves
    /// the drawer, and the expected total must fall with it.
    ///
    /// The change comes with them because it leaves the drawer too. A customer
    /// handing over a five hundred note for a basket of 494.50 puts a note in
    /// and takes 5.50 out, and a drawer that counted the note and forgot the
    /// change would expect five and a half taka more than it holds. Once a day
    /// that is a curiosity; every cash sale where somebody has no change is
    /// every evening of the year ending short, and a shop that sees that either
    /// stops trusting the till or goes looking for a thief who is not there.
    /// Put the drawer's running cash figure where a test needs it.
    ///
    /// For one test: that a sale already durable is never reported as failed.
    /// The only way the drawer can refuse a sale is arithmetic at figures no
    /// shop reaches, and reaching them through the front door means a basket
    /// whose own totals overflow first.
    #[cfg(test)]
    pub(crate) fn set_cash_for_test(&mut self, cash: Minor) {
        self.cash_sales = cash;
    }

    pub fn record_sale(&mut self, tenders: &[Tender], change: Minor) -> Result<()> {
        self.ensure_open()?;

        // Accumulate into copies first. A sale that overflows halfway would
        // otherwise leave the shift holding part of a sale, which is a worse
        // number to reconcile than a rejected one.
        let mut totals = self.tender_totals.clone();
        let mut cash = self.cash_sales;
        for tender in tenders {
            credit(&mut totals, &tender.kind, tender.amount)?;
            if lands_in_drawer(&tender.kind) {
                cash = cash.checked_add(tender.amount)?;
            }
        }
        // Change is always money, and by the cart's own rule it never exceeds
        // the cash that was handed over, so this cannot make a sale take money
        // out of a drawer it never put in.
        cash = cash.checked_sub(change)?;

        self.tender_totals = totals;
        self.cash_sales = cash;
        self.sales = self.sales.saturating_add(1);
        Ok(())
    }

    /// Put cash in for a stated reason: a float top-up, change brought from the
    /// safe, money returned to the till.
    pub fn cash_in(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(CashDirection::In, amount, reason, at_ms)
    }

    /// Take cash out for a stated reason: a drop to the safe, a supplier paid
    /// from the drawer.
    pub fn cash_out(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(CashDirection::Out, amount, reason, at_ms)
    }

    /// What the drawer should hold right now.
    pub fn expected_cash(&self) -> Result<Minor> {
        let taken = Minor::sum([self.opening_float, self.cash_sales, self.cash_in_total])?;
        Ok(taken.checked_sub(self.cash_out_total)?)
    }

    /// Totals so far, leaving the shift open.
    pub fn x_report(&self) -> Result<XReport> {
        let non_cash_sales = Minor::sum(
            self.tender_totals
                .iter()
                .filter(|total| !total.in_drawer)
                .map(|total| total.amount),
        )?;

        // The cash row says what stayed, not what was handed over. A person
        // reads these rows and then reads "should hold" under them, and a row
        // saying 994.50 above a figure that counts 989.00 is a report they have
        // to be told to distrust. The change went back across the counter with
        // the goods; the drawer kept the basket.
        let mut tenders = self.tender_totals.clone();
        for row in tenders.iter_mut().filter(|row| row.in_drawer) {
            row.amount = self.cash_sales;
        }

        Ok(XReport {
            shift: self.id,
            terminal: self.terminal,
            opened_at_ms: self.opened_at_ms,
            opening_float: self.opening_float,
            sales: self.sales,
            tenders,
            cash_sales: self.cash_sales,
            non_cash_sales,
            cash_in: self.cash_in_total,
            cash_out: self.cash_out_total,
            expected_cash: self.expected_cash()?,
        })
    }

    /// Close the drawer against a counted total.
    ///
    /// The variance is counted less expected and is allowed to be negative.
    /// Refusing a short drawer would only push the shortfall into a fabricated
    /// count, which is the number the shop most needs to be true.
    pub fn close(&mut self, counted_cash: Minor, closed_at_ms: u64) -> Result<ZReport> {
        self.ensure_open()?;
        if counted_cash.is_negative() {
            return Err(ShiftError::NegativeAmount {
                amount: counted_cash,
            });
        }

        let variance = counted_cash.checked_sub(self.expected_cash()?)?;
        self.closing = Some(Closing {
            counted_cash,
            closed_at_ms,
            variance,
        });
        self.z_report()
    }

    /// The Z report of an already closed shift.
    ///
    /// Separate from `close` so a reprint costs nothing and, more importantly,
    /// cannot be obtained by counting the drawer a second time.
    pub fn z_report(&self) -> Result<ZReport> {
        let closing = self.closing.ok_or(ShiftError::StillOpen)?;
        Ok(ZReport {
            totals: self.x_report()?,
            closed_at_ms: closing.closed_at_ms,
            counted_cash: closing.counted_cash,
            variance: closing.variance,
        })
    }

    fn move_cash(
        &mut self,
        direction: CashDirection,
        amount: Minor,
        reason: &str,
        at_ms: u64,
    ) -> Result<()> {
        self.ensure_open()?;
        if amount.is_negative() {
            return Err(ShiftError::NegativeAmount { amount });
        }
        // The type says it refuses one without an explanation attached, and
        // until now it did not. Money out of a drawer with nothing beside it is
        // indistinguishable from theft when the variance is read a week later,
        // which is the whole reason a movement carries a reason at all.
        if reason.trim().is_empty() {
            return Err(ShiftError::NoReason);
        }

        match direction {
            CashDirection::In => self.cash_in_total = self.cash_in_total.checked_add(amount)?,
            CashDirection::Out => self.cash_out_total = self.cash_out_total.checked_add(amount)?,
        }
        self.movements.push(CashMovement {
            direction,
            amount,
            reason: reason.into(),
            at_ms,
        });
        Ok(())
    }

    fn ensure_open(&self) -> Result<()> {
        match self.closing {
            Some(closing) => Err(ShiftError::AlreadyClosed {
                closed_at_ms: closing.closed_at_ms,
            }),
            None => Ok(()),
        }
    }
}

/// Add to the row for this kind, or start one.
///
/// Wallets are keyed by name, so bKash and Nagad are separate lines: a shop
/// reconciles each wallet against its own statement, and one merged "wallet"
/// figure would have to be taken apart by hand.
fn credit(totals: &mut Vec<TenderTotal>, kind: &TenderKind, amount: Minor) -> Result<()> {
    if let Some(existing) = totals.iter_mut().find(|total| total.kind == *kind) {
        existing.amount = existing.amount.checked_add(amount)?;
        return Ok(());
    }
    totals.push(TenderTotal {
        kind: kind.clone(),
        amount,
        in_drawer: lands_in_drawer(kind),
    });
    Ok(())
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

    use super::*;

    const OPENED_AT: u64 = 1_788_600_000_000;

    fn shift(float: i64) -> Shift {
        Shift::open(
            Ulid::from_u128(11),
            Ulid::from_u128(7),
            Minor::new(float),
            OPENED_AT,
        )
        .unwrap()
    }

    fn cash(amount: i64) -> Tender {
        Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount),
            reference: None,
        }
    }

    fn wallet(name: &str, amount: i64) -> Tender {
        Tender {
            kind: TenderKind::Wallet(name.into()),
            amount: Minor::new(amount),
            reference: Some("TRX123".into()),
        }
    }

    fn total_for<'a>(report: &'a XReport, kind: &TenderKind) -> Option<&'a TenderTotal> {
        report.tenders.iter().find(|total| total.kind == *kind)
    }

    #[test]
    fn a_shift_opened_with_no_float_expects_an_empty_drawer() {
        let shift = shift(0);
        let report = shift.x_report().unwrap();
        assert_eq!(report.opening_float, Minor::ZERO);
        assert_eq!(report.expected_cash, Minor::ZERO);
        assert_eq!(report.sales, 0);
        assert!(report.tenders.is_empty());
        assert!(shift.is_open());
    }

    #[test]
    fn refuses_to_open_on_a_negative_float() {
        assert_eq!(
            Shift::open(
                Ulid::from_u128(11),
                Ulid::from_u128(7),
                Minor::new(-1),
                OPENED_AT
            ),
            Err(ShiftError::NegativeAmount {
                amount: Minor::new(-1)
            })
        );
    }

    #[test]
    fn only_cash_tenders_reach_the_drawer() {
        let mut shift = shift(200_000);
        shift
            .record_sale(&[wallet("bKash", 30_000), cash(20_000)], Minor::ZERO)
            .unwrap();

        let report = shift.x_report().unwrap();
        assert_eq!(report.sales, 1, "one split-tender sale is one sale");
        assert_eq!(report.cash_sales, Minor::new(20_000));
        assert_eq!(
            report.non_cash_sales,
            Minor::new(30_000),
            "revenue, but not in the till"
        );
        assert_eq!(
            report.expected_cash,
            Minor::new(220_000),
            "the bKash payment must not inflate what the cashier is asked to count"
        );
    }

    #[test]
    fn every_tender_kind_keeps_its_own_line() {
        let mut shift = shift(0);
        shift
            .record_sale(&[wallet("bKash", 10_000)], Minor::ZERO)
            .unwrap();
        shift
            .record_sale(&[wallet("Nagad", 5_000)], Minor::ZERO)
            .unwrap();
        shift
            .record_sale(&[wallet("bKash", 1_000), cash(500)], Minor::ZERO)
            .unwrap();
        shift
            .record_sale(
                &[Tender {
                    kind: TenderKind::Credit,
                    amount: Minor::new(7_000),
                    reference: None,
                }],
                Minor::ZERO,
            )
            .unwrap();

        let report = shift.x_report().unwrap();
        assert_eq!(report.sales, 4);
        assert_eq!(
            report.tenders.len(),
            4,
            "two wallets are two lines to reconcile"
        );

        let bkash = total_for(&report, &TenderKind::Wallet("bKash".into())).unwrap();
        assert_eq!(bkash.amount, Minor::new(11_000));
        assert!(!bkash.in_drawer);

        let nagad = total_for(&report, &TenderKind::Wallet("Nagad".into())).unwrap();
        assert_eq!(nagad.amount, Minor::new(5_000));

        let till_cash = total_for(&report, &TenderKind::Cash).unwrap();
        assert_eq!(till_cash.amount, Minor::new(500));
        assert!(till_cash.in_drawer);

        let credit = total_for(&report, &TenderKind::Credit).unwrap();
        assert!(
            !credit.in_drawer,
            "sold on account is not money in the drawer"
        );
        assert_eq!(report.non_cash_sales, Minor::new(23_000));
    }

    #[test]
    fn cash_movements_shift_what_the_drawer_should_hold() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();
        shift
            .cash_in(Minor::new(20_000), "change from the safe", OPENED_AT + 1)
            .unwrap();
        shift
            .cash_out(Minor::new(30_000), "paid the milk supplier", OPENED_AT + 2)
            .unwrap();

        let report = shift.x_report().unwrap();
        assert_eq!(report.cash_in, Minor::new(20_000));
        assert_eq!(report.cash_out, Minor::new(30_000));
        assert_eq!(report.expected_cash, Minor::new(140_000));

        assert_eq!(shift.movements().len(), 2);
        assert_eq!(shift.movements()[0].direction, CashDirection::In);
        assert_eq!(&*shift.movements()[1].reason, "paid the milk supplier");
        assert_eq!(shift.movements()[1].at_ms, OPENED_AT + 2);
        assert!(
            !shift.movements()[1].amount.is_negative(),
            "the direction carries the sign, never the amount"
        );
    }

    #[test]
    fn refuses_a_cash_movement_with_nothing_said_about_why() {
        let mut shift = shift(100_000);
        // The type has said it refuses one without an explanation since it was
        // written, and until now it took whatever it was handed. Money out of a
        // drawer with nothing beside it is indistinguishable from theft when
        // the count comes up short.
        assert_eq!(
            shift.cash_out(Minor::new(10_000), "", OPENED_AT + 1),
            Err(ShiftError::NoReason)
        );
        assert_eq!(
            shift.cash_in(Minor::new(10_000), "   ", OPENED_AT + 1),
            Err(ShiftError::NoReason)
        );
        assert!(
            shift.movements().is_empty(),
            "and nothing was recorded either way"
        );
        assert_eq!(shift.expected_cash().unwrap(), Minor::new(100_000));
    }

    #[test]
    fn refuses_a_negative_cash_movement() {
        let mut shift = shift(100_000);
        assert_eq!(
            shift.cash_in(Minor::new(-1), "typo", OPENED_AT),
            Err(ShiftError::NegativeAmount {
                amount: Minor::new(-1)
            })
        );
        assert_eq!(
            shift.cash_out(Minor::new(-500), "typo", OPENED_AT),
            Err(ShiftError::NegativeAmount {
                amount: Minor::new(-500)
            })
        );
        assert!(
            shift.movements().is_empty(),
            "a rejected movement leaves no trace"
        );
        assert_eq!(shift.expected_cash().unwrap(), Minor::new(100_000));
    }

    #[test]
    fn the_change_handed_back_leaves_the_drawer_with_it() {
        let mut shift = shift(100_000);
        // A five hundred note for a basket of 494.50. The note goes in and
        // 5.50 comes back out, so the drawer holds the basket.
        shift.record_sale(&[cash(50_000)], Minor::new(550)).unwrap();

        let report = shift.x_report().unwrap();
        assert_eq!(
            report.cash_sales,
            Minor::new(49_450),
            "the basket, not the note"
        );
        assert_eq!(
            shift.expected_cash().unwrap(),
            Minor::new(100_000 + 49_450),
            "the float and the basket"
        );
        // Counting the note and forgetting the change is a drawer that ends
        // short by the day's change, every day, and a shop that goes looking
        // for a thief who is not there.
        assert_ne!(report.cash_sales, Minor::new(50_000));
        // And the row a person reads says the same thing as the figure under
        // it, rather than the note that was handed over.
        let cash_row = report
            .tenders
            .iter()
            .find(|row| row.in_drawer)
            .expect("the cash row");
        assert_eq!(cash_row.amount, Minor::new(49_450));
    }

    #[test]
    fn a_refund_paid_in_cash_lowers_the_expected_drawer() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();
        // A return settled from the till: the same event with the sign reversed.
        shift.record_sale(&[cash(-20_000)], Minor::ZERO).unwrap();

        assert_eq!(shift.expected_cash().unwrap(), Minor::new(130_000));
        assert_eq!(shift.x_report().unwrap().cash_sales, Minor::new(30_000));
    }

    #[test]
    fn a_short_drawer_closes_with_a_negative_variance() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();

        let z = shift
            .close(Minor::new(148_500), OPENED_AT + 60_000)
            .unwrap();
        assert_eq!(z.totals.expected_cash, Minor::new(150_000));
        assert_eq!(z.counted_cash, Minor::new(148_500));
        assert_eq!(
            z.variance,
            Minor::new(-1_500),
            "short by fifteen taka, and that is the point"
        );
        assert_eq!(z.closed_at_ms, OPENED_AT + 60_000);
        assert!(!shift.is_open());
    }

    #[test]
    fn an_over_drawer_closes_with_a_positive_variance() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();

        let z = shift
            .close(Minor::new(150_700), OPENED_AT + 60_000)
            .unwrap();
        assert_eq!(z.variance, Minor::new(700));
    }

    #[test]
    fn a_shift_that_sold_nothing_closes_at_its_float() {
        let mut shift = shift(100_000);
        let z = shift.close(Minor::new(100_000), OPENED_AT + 1).unwrap();
        assert_eq!(z.totals.sales, 0);
        assert_eq!(z.variance, Minor::ZERO);
        assert!(z.totals.tenders.is_empty());
    }

    #[test]
    fn refuses_a_negative_drawer_count() {
        let mut shift = shift(100_000);
        assert_eq!(
            shift.close(Minor::new(-1), OPENED_AT + 1),
            Err(ShiftError::NegativeAmount {
                amount: Minor::new(-1)
            })
        );
        assert!(
            shift.is_open(),
            "a rejected count does not close the drawer"
        );
    }

    #[test]
    fn a_closed_shift_refuses_further_sales() {
        let mut shift = shift(100_000);
        shift
            .close(Minor::new(100_000), OPENED_AT + 60_000)
            .unwrap();

        assert_eq!(
            shift.record_sale(&[cash(10_000)], Minor::ZERO),
            Err(ShiftError::AlreadyClosed {
                closed_at_ms: OPENED_AT + 60_000
            })
        );
        assert_eq!(
            shift.sales(),
            0,
            "the reported total cannot move after it is reported"
        );
    }

    #[test]
    fn a_closed_shift_refuses_further_movements() {
        let mut shift = shift(100_000);
        shift
            .close(Minor::new(100_000), OPENED_AT + 60_000)
            .unwrap();

        let closed = Err(ShiftError::AlreadyClosed {
            closed_at_ms: OPENED_AT + 60_000,
        });
        assert_eq!(
            shift.cash_in(Minor::new(5_000), "late float", OPENED_AT + 70_000),
            closed
        );
        assert_eq!(
            shift.cash_out(Minor::new(5_000), "late drop", OPENED_AT + 70_000),
            closed
        );
        assert!(shift.movements().is_empty());
        assert_eq!(shift.expected_cash().unwrap(), Minor::new(100_000));
    }

    #[test]
    fn a_shift_cannot_be_closed_twice() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();
        let first = shift
            .close(Minor::new(148_500), OPENED_AT + 60_000)
            .unwrap();

        // A second count would silently replace a variance somebody has already
        // been asked to explain.
        assert_eq!(
            shift.close(Minor::new(150_000), OPENED_AT + 70_000),
            Err(ShiftError::AlreadyClosed {
                closed_at_ms: OPENED_AT + 60_000
            })
        );
        assert_eq!(
            shift.z_report().unwrap(),
            first,
            "the first close is the one that stands"
        );
    }

    #[test]
    fn the_z_report_can_be_reprinted_without_recounting() {
        let mut shift = shift(100_000);
        shift
            .record_sale(&[cash(50_000), wallet("bKash", 25_000)], Minor::ZERO)
            .unwrap();
        let z = shift
            .close(Minor::new(150_000), OPENED_AT + 60_000)
            .unwrap();

        assert_eq!(shift.z_report().unwrap(), z);
        assert_eq!(
            shift.z_report().unwrap().totals.non_cash_sales,
            Minor::new(25_000)
        );
    }

    #[test]
    fn an_open_shift_has_no_z_report_yet() {
        let shift = shift(100_000);
        assert_eq!(shift.z_report(), Err(ShiftError::StillOpen));
        assert_eq!(shift.closed_at_ms(), None);
    }

    #[test]
    fn an_x_report_leaves_the_shift_trading() {
        let mut shift = shift(100_000);
        shift.record_sale(&[cash(50_000)], Minor::ZERO).unwrap();
        let midday = shift.x_report().unwrap();
        assert_eq!(midday.expected_cash, Minor::new(150_000));

        shift.record_sale(&[cash(10_000)], Minor::ZERO).unwrap();
        assert!(shift.is_open());
        assert_eq!(shift.x_report().unwrap().sales, 2);
        assert_eq!(
            midday.sales, 1,
            "an X report is a snapshot, not a live view"
        );
    }

    #[test]
    fn the_report_names_the_terminal_it_belongs_to() {
        // The shop's day report is an aggregation over these, so every report
        // has to say which drawer produced it.
        let shift = shift(0);
        let report = shift.x_report().unwrap();
        assert_eq!(report.terminal, Ulid::from_u128(7));
        assert_eq!(report.shift, Ulid::from_u128(11));
        assert_eq!(report.opened_at_ms, OPENED_AT);
    }

    #[test]
    fn reports_overflow_rather_than_wrapping() {
        let mut shift = shift(i64::MAX);
        shift.record_sale(&[cash(1)], Minor::ZERO).unwrap();
        assert_eq!(
            shift.expected_cash(),
            Err(ShiftError::Money(MoneyError::Overflow))
        );
        assert_eq!(
            shift.close(Minor::new(1), OPENED_AT + 1),
            Err(ShiftError::Money(MoneyError::Overflow))
        );
        assert!(
            shift.is_open(),
            "a close that could not be computed did not happen"
        );
    }

    #[test]
    fn a_sale_that_overflows_leaves_the_shift_untouched() {
        let mut shift = shift(0);
        shift.record_sale(&[cash(i64::MAX)], Minor::ZERO).unwrap();
        assert_eq!(
            shift.record_sale(&[wallet("bKash", 1), cash(1)], Minor::ZERO),
            Err(ShiftError::Money(MoneyError::Overflow))
        );

        let report = shift.x_report().unwrap();
        assert_eq!(report.sales, 1, "a rejected sale is not counted");
        assert_eq!(
            report.tenders.len(),
            1,
            "and leaves no half-applied tender behind"
        );
        assert_eq!(report.cash_sales, Minor::new(i64::MAX));
    }
}
