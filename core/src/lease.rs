//! Receipt numbers, leased from the server in blocks.
//!
//! Identity and presentation are deliberately separate. A ticket's identity is a
//! ULID minted on the device, so a sale never waits for a network. Its receipt
//! number, the thing printed on paper and quoted in a dispute, comes from a block
//! the server leased to this terminal in advance.
//!
//! Terminal-owned sequences were considered and rejected: a terminal restored
//! from a backup, or with its storage cleared, restarts its counter and reissues
//! numbers that already exist. The server can only notice afterwards, by which
//! time the customer has walked out with the receipt. A lease moves that
//! detection before the sale instead of after it, and an epoch lets the server
//! recognise a terminal that came back from the dead.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

use crate::cart::TerminalId;

/// Numbers left before the till starts asking for a new block. Renewal is
/// opportunistic and happens whenever the terminal is online, so this only has
/// to cover a plausible offline stretch: a few hundred sales is a long day.
pub const DEFAULT_RENEWAL_THRESHOLD: u64 = 100;

/// How many digits a receipt number is padded to. Fixed width keeps printed
/// receipts and spreadsheets sorting correctly.
const SEQUENCE_WIDTH: usize = 6;

/// A block of receipt numbers granted to one terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub terminal: TerminalId,
    /// Bumped by the server whenever it reissues numbers to a terminal it
    /// believes was replaced or restored. A ticket carries the epoch it was
    /// numbered under, so a collision is attributable rather than mysterious.
    pub epoch: u64,
    /// Printed before the sequence, usually a short terminal name.
    pub prefix: Box<str>,
    /// Next number to hand out.
    pub next: u64,
    /// Last number in the block, inclusive.
    pub last: u64,
}

impl Lease {
    #[must_use]
    pub fn new(terminal: TerminalId, epoch: u64, prefix: &str, first: u64, last: u64) -> Self {
        Self {
            terminal,
            epoch,
            prefix: prefix.into(),
            next: first,
            last,
        }
    }

    /// How many numbers remain unissued.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.last.saturating_add(1).saturating_sub(self.next)
    }

    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }

    /// Whether the till should ask for another block next time it is online.
    #[must_use]
    pub fn needs_renewal(&self, threshold: u64) -> bool {
        self.remaining() <= threshold
    }

    /// Take the next number, or `None` if the block is used up.
    pub fn consume(&mut self) -> Option<ReceiptNumber> {
        if self.is_exhausted() {
            return None;
        }
        let sequence = self.next;
        self.next = self.next.saturating_add(1);
        Some(ReceiptNumber {
            epoch: self.epoch,
            sequence,
            text: format_receipt(&self.prefix, sequence),
        })
    }
}

/// A number actually issued to a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptNumber {
    pub epoch: u64,
    pub sequence: u64,
    /// What gets printed, for example `T1-000123`.
    pub text: String,
}

fn format_receipt(prefix: &str, sequence: u64) -> String {
    format!("{prefix}-{sequence:0SEQUENCE_WIDTH$}", SEQUENCE_WIDTH = SEQUENCE_WIDTH)
}

/// The terminal's supply of receipt numbers: one active block, and optionally
/// the next one already in hand.
///
/// Holding a reserve is what lets a till cross a block boundary while offline.
/// Renewing only at exhaustion would mean the first sale after the last number
/// is unnumbered, which is exactly when a shop is busiest.
#[derive(Debug, Clone, Default)]
pub struct LeaseBook {
    active: Option<Lease>,
    reserve: Option<Lease>,
    /// Sales closed with no number available, awaiting a block to number them.
    unnumbered: u64,
}

impl LeaseBook {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn active(&self) -> Option<&Lease> {
        self.active.as_ref()
    }

    /// Sales that closed with no number available.
    ///
    /// They are still valid sales, still synced, still counted. The back office
    /// numbers them when a block arrives. A till that refused to sell here would
    /// be worse than one that sells and reconciles.
    #[must_use]
    pub fn unnumbered(&self) -> u64 {
        self.unnumbered
    }

    /// Accept a block from the server. The first goes active, the next is held
    /// in reserve, and anything beyond that replaces the reserve.
    pub fn grant(&mut self, lease: Lease) {
        match self.active {
            None => self.active = Some(lease),
            Some(_) => self.reserve = Some(lease),
        }
    }

    /// Total numbers in hand across both blocks.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        let active = self.active.as_ref().map_or(0, Lease::remaining);
        let reserve = self.reserve.as_ref().map_or(0, Lease::remaining);
        active.saturating_add(reserve)
    }

    /// Whether to ask for another block at the next opportunity.
    #[must_use]
    pub fn needs_renewal(&self, threshold: u64) -> bool {
        self.remaining() <= threshold
    }

    /// Take the next receipt number, promoting the reserve when the active block
    /// runs out. Returns `None` only when the terminal has genuinely run dry.
    pub fn consume(&mut self) -> Option<ReceiptNumber> {
        if let Some(number) = self.active.as_mut().and_then(Lease::consume) {
            return Some(number);
        }
        // Active block is spent; promote the reserve and try once more.
        if self.reserve.is_some() {
            self.active = self.reserve.take();
            if let Some(number) = self.active.as_mut().and_then(Lease::consume) {
                return Some(number);
            }
        }
        self.unnumbered = self.unnumbered.saturating_add(1);
        None
    }

    /// Clear the record of unnumbered sales once the back office has numbered
    /// them.
    pub fn clear_unnumbered(&mut self) {
        self.unnumbered = 0;
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use super::*;
    use crate::ids::Ulid;

    fn terminal() -> TerminalId {
        Ulid::from_u128(7)
    }

    fn block(first: u64, last: u64) -> Lease {
        Lease::new(terminal(), 1, "T1", first, last)
    }

    #[test]
    fn issues_padded_numbers_in_order() {
        let mut lease = block(100, 102);
        assert_eq!(lease.consume().map(|n| n.text), Some("T1-000100".into()));
        assert_eq!(lease.consume().map(|n| n.text), Some("T1-000101".into()));
        assert_eq!(lease.consume().map(|n| n.text), Some("T1-000102".into()));
        assert!(lease.consume().is_none(), "the block is spent");
    }

    #[test]
    fn reports_what_is_left() {
        let mut lease = block(100, 599);
        assert_eq!(lease.remaining(), 500);
        lease.consume();
        assert_eq!(lease.remaining(), 499);
        assert!(!lease.needs_renewal(100));

        let nearly_spent = block(595, 599);
        assert!(nearly_spent.needs_renewal(DEFAULT_RENEWAL_THRESHOLD));
    }

    #[test]
    fn crosses_a_block_boundary_using_the_reserve() {
        let mut book = LeaseBook::new();
        book.grant(block(100, 101));
        book.grant(block(200, 201));
        assert_eq!(book.remaining(), 4);

        assert_eq!(book.consume().map(|n| n.sequence), Some(100));
        assert_eq!(book.consume().map(|n| n.sequence), Some(101));
        // active block spent, reserve takes over without a gap in service
        assert_eq!(book.consume().map(|n| n.sequence), Some(200));
        assert_eq!(book.consume().map(|n| n.sequence), Some(201));
        assert_eq!(book.remaining(), 0);
    }

    #[test]
    fn keeps_selling_when_the_numbers_run_out() {
        let mut book = LeaseBook::new();
        book.grant(block(100, 100));
        assert!(book.consume().is_some());

        // A till with no numbers left still takes the money. The sale is real;
        // only its presentation is missing.
        assert!(book.consume().is_none());
        assert!(book.consume().is_none());
        assert_eq!(book.unnumbered(), 2);

        book.grant(block(300, 399));
        assert_eq!(book.consume().map(|n| n.sequence), Some(300));
        book.clear_unnumbered();
        assert_eq!(book.unnumbered(), 0);
    }

    #[test]
    fn carries_the_epoch_onto_every_number() {
        let mut lease = Lease::new(terminal(), 4, "T2", 1, 9);
        let issued = lease.consume().unwrap();
        assert_eq!(issued.epoch, 4);
        assert_eq!(issued.text, "T2-000001");
    }

    #[test]
    fn an_empty_book_issues_nothing() {
        let mut book = LeaseBook::new();
        assert!(book.consume().is_none());
        assert_eq!(book.unnumbered(), 1);
        assert!(book.needs_renewal(DEFAULT_RENEWAL_THRESHOLD));
    }
}
