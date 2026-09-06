//! What actually goes on disk.
//!
//! These types are deliberately separate from the ones the till works with. The
//! encoding is positional, so adding a field to [`crate::replica::Item`] would
//! silently turn every snapshot ever written into garbage. Keeping a wire type
//! per schema version means a field can be added to the domain freely, and the
//! disk format changes only when somebody writes a new version and a converter.
//!
//! The rule that follows: **never derive `Serialize` on a domain type.** If a
//! struct is used by the cart, the replica or the ledger, it does not appear
//! here; a mirror of it does.
//!
//! Disk formats need backward compatibility only, because new code reads old
//! bytes. The sync wire additionally needs forward compatibility, since an old
//! till must parse a newer server's replies, so it lives elsewhere and evolves on
//! its own schedule. Sharing one struct between disk and network is the decision
//! that hurts at month twelve, when a network change forces a disk migration.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::cart::{CartLine, Tender, TenderKind, Ticket};
use crate::domain::{Discount, PriceMode};
use crate::ids::Ulid;
use crate::money::{Bp, Milli, Minor};
use crate::replica::Item;

/// Schema carried in the frame header for a snapshot payload.
pub const SNAPSHOT_SCHEMA: u16 = 1;
/// Schema carried in the frame header for a committed sale.
pub const SALE_SCHEMA: u16 = 1;
/// Schema carried in the frame header for a batch of catalogue changes.
pub const DELTAS_SCHEMA: u16 = 1;
/// Schema carried in the frame header for a sync acknowledgement watermark.
pub const ACK_SCHEMA: u16 = 1;
/// Schema carried in the frame header for a receipt number block.
pub const LEASE_SCHEMA: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The bytes did not decode as the schema claimed.
    Malformed,
    /// Written by a newer version than this build understands. Reported rather
    /// than guessed at: a till that misreads a snapshot sells at the wrong price.
    UnsupportedSchema { schema: u16 },
    /// A value decoded but is not valid, such as a tax rate above 100 percent.
    OutOfRange,
}

pub type Result<T> = core::result::Result<T, WireError>;

// ---------------------------------------------------------------------------
// Version 1
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotV1 {
    /// Server sequence this snapshot is current as of. Stored with the items so
    /// a cold start knows where to resume pulling without a separate file that
    /// could disagree with the catalogue it describes.
    pub cursor: u64,
    pub items: Vec<ItemV1>,
}

/// Catalogue changes pulled from the server, as persisted.
///
/// Written to the replica log before being applied in memory, so a reboot
/// between the two replays them rather than losing them. The cursor advances
/// only once this frame is durable: otherwise a price the cashier already saw
/// could revert after a power cut, having been pulled but never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltasV1 {
    /// Server sequence after applying this batch.
    pub cursor: u64,
    pub upserts: Vec<ItemV1>,
    pub tombstones: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV1 {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    /// True when the shelf price already contains VAT.
    pub price_inclusive: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscountV1 {
    None,
    RateBp(u32),
    AmountMinor(i64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TenderKindV1 {
    Cash,
    Wallet(String),
    Card,
    Credit,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenderV1 {
    pub kind: TenderKindV1,
    pub amount_minor: i64,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV1 {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
}

/// A sale, as committed.
///
/// The totals are stored rather than recomputed on read. They are what the
/// customer was actually charged and what the receipt in their pocket says; the
/// server revalidates them with the same arithmetic when the sale syncs, and a
/// disagreement is a finding rather than something to silently correct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketV1 {
    pub id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub customer: Option<u128>,
    pub lines: Vec<LineV1>,
    pub ticket_discount: DiscountV1,
    pub tenders: Vec<TenderV1>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
}

/// Everything one sale changes, in a single payload.
///
/// This is the unit the journal commits atomically. Splitting it would allow a
/// crash between the ticket and the lease, producing either a receipt number
/// consumed with no sale behind it, or a number reissued after reboot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleCommitV1 {
    pub ticket: TicketV1,
    /// The lease position after this sale took its number, so a reboot resumes
    /// where the terminal actually stopped rather than where it last synced.
    pub lease_next: Option<u64>,
    pub lease_epoch: Option<u64>,
    /// Stock movements this sale caused, as item id and signed milli-units.
    pub stock: Vec<(u128, i64)>,
}

/// How far the server has confirmed, recorded in the critical log itself.
///
/// Written rather than deleting the sales it covers, because deleting from the
/// front of a log means rewriting it, and a crash mid-rewrite would take the
/// unacknowledged tail with it. A watermark is one append; the sales it covers
/// are dropped later, all at once, when nothing is left outstanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncAckV1 {
    /// Highest journal sequence the server has confirmed, contiguously.
    pub through_sequence: u64,
}

/// A block of receipt numbers granted by the server, as persisted.
///
/// Stored so a terminal that reboots offline resumes numbering where it actually
/// stopped. Recovering the block from the server instead would mean a till that
/// cannot print a numbered receipt until it next reaches the network, which is
/// the moment it is least likely to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseGrantV1 {
    pub terminal: u128,
    pub epoch: u64,
    pub prefix: String,
    pub first: u64,
    pub last: u64,
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encode a receipt number block.
pub fn encode_lease(lease: &LeaseGrantV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(lease).map_err(|_| WireError::Malformed)
}

/// Decode a receipt number block written under `schema`.
pub fn decode_lease(schema: u16, bytes: &[u8]) -> Result<LeaseGrantV1> {
    match schema {
        LEASE_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

/// Encode an acknowledgement watermark.
pub fn encode_ack(ack: &SyncAckV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(ack).map_err(|_| WireError::Malformed)
}

/// Decode an acknowledgement watermark written under `schema`.
pub fn decode_ack(schema: u16, bytes: &[u8]) -> Result<SyncAckV1> {
    match schema {
        ACK_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

/// Encode a catalogue snapshot, current as of `cursor`.
pub fn encode_snapshot(items: &[Item], cursor: u64) -> Result<Vec<u8>> {
    let snapshot = SnapshotV1 {
        cursor,
        items: items.iter().map(ItemV1::from_domain).collect(),
    };
    postcard::to_allocvec(&snapshot).map_err(|_| WireError::Malformed)
}

/// Decode a catalogue snapshot written under `schema`.
///
/// Old schemas are decoded by keeping their struct definitions and a converter,
/// never by trying to read new bytes with an old decoder. When a version is
/// retired, the boot path rewrites the snapshot in the current format, which caps
/// how many decoder generations stay alive.
pub fn decode_snapshot(schema: u16, bytes: &[u8]) -> Result<(Vec<Item>, u64)> {
    match schema {
        SNAPSHOT_SCHEMA => {
            let snapshot: SnapshotV1 =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            let cursor = snapshot.cursor;
            let items = snapshot
                .items
                .into_iter()
                .map(ItemV1::into_domain)
                .collect::<Result<Vec<_>>>()?;
            Ok((items, cursor))
        }
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

/// Encode a batch of catalogue changes for the replica log.
pub fn encode_deltas(deltas: &ItemDeltasV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(deltas).map_err(|_| WireError::Malformed)
}

/// Decode a batch of catalogue changes written under `schema`.
pub fn decode_deltas(schema: u16, bytes: &[u8]) -> Result<ItemDeltasV1> {
    match schema {
        DELTAS_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

/// Encode one committed sale.
pub fn encode_sale(sale: &SaleCommitV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(sale).map_err(|_| WireError::Malformed)
}

/// Decode one committed sale written under `schema`.
pub fn decode_sale(schema: u16, bytes: &[u8]) -> Result<SaleCommitV1> {
    match schema {
        SALE_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl ItemV1 {
    #[must_use]
    pub fn from_domain(item: &Item) -> Self {
        Self {
            id: item.id.to_u128(),
            code: item.code.to_string(),
            name_en: item.name_en.to_string(),
            name_bn: item.name_bn.to_string(),
            unit: item.unit.to_string(),
            price_minor: item.price.get(),
            cost_minor: item.cost.get(),
            vat_bp: item.vat_rate.get(),
            price_inclusive: matches!(item.price_mode, PriceMode::Inclusive),
            barcodes: item.barcodes.iter().map(ToString::to_string).collect(),
            on_hand_milli: item.on_hand.get(),
            active: item.active,
        }
    }

    pub fn into_domain(self) -> Result<Item> {
        Ok(Item {
            id: Ulid::from_u128(self.id),
            code: self.code.into_boxed_str(),
            name_en: self.name_en.into_boxed_str(),
            name_bn: self.name_bn.into_boxed_str(),
            unit: self.unit.into_boxed_str(),
            price: Minor::new(self.price_minor),
            cost: Minor::new(self.cost_minor),
            vat_rate: Bp::vat(self.vat_bp).map_err(|_| WireError::OutOfRange)?,
            price_mode: if self.price_inclusive {
                PriceMode::Inclusive
            } else {
                PriceMode::Exclusive
            },
            barcodes: self
                .barcodes
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
            on_hand: Milli::new(self.on_hand_milli),
            active: self.active,
        })
    }
}

impl DiscountV1 {
    #[must_use]
    pub fn from_domain(discount: Discount) -> Self {
        match discount {
            Discount::None => Self::None,
            Discount::Rate(rate) => Self::RateBp(rate.get()),
            Discount::Amount(amount) => Self::AmountMinor(amount.get()),
        }
    }

    pub fn into_domain(self) -> Result<Discount> {
        Ok(match self {
            Self::None => Discount::None,
            Self::RateBp(bp) => {
                Discount::Rate(Bp::new(bp).map_err(|_| WireError::OutOfRange)?)
            }
            Self::AmountMinor(amount) => Discount::Amount(Minor::new(amount)),
        })
    }
}

impl TenderKindV1 {
    #[must_use]
    pub fn from_domain(kind: &TenderKind) -> Self {
        match kind {
            TenderKind::Cash => Self::Cash,
            TenderKind::Wallet(name) => Self::Wallet(name.to_string()),
            TenderKind::Card => Self::Card,
            TenderKind::Credit => Self::Credit,
            TenderKind::Other(name) => Self::Other(name.to_string()),
        }
    }

    #[must_use]
    pub fn into_domain(self) -> TenderKind {
        match self {
            Self::Cash => TenderKind::Cash,
            Self::Wallet(name) => TenderKind::Wallet(name.into_boxed_str()),
            Self::Card => TenderKind::Card,
            Self::Credit => TenderKind::Credit,
            Self::Other(name) => TenderKind::Other(name.into_boxed_str()),
        }
    }
}

impl TenderV1 {
    #[must_use]
    pub fn from_domain(tender: &Tender) -> Self {
        Self {
            kind: TenderKindV1::from_domain(&tender.kind),
            amount_minor: tender.amount.get(),
            reference: tender.reference.as_ref().map(ToString::to_string),
        }
    }

    #[must_use]
    pub fn into_domain(self) -> Tender {
        Tender {
            kind: self.kind.into_domain(),
            amount: Minor::new(self.amount_minor),
            reference: self.reference.map(String::into_boxed_str),
        }
    }
}

impl LineV1 {
    #[must_use]
    pub fn from_domain(line: &CartLine) -> Self {
        Self {
            item_id: line.item_id.to_u128(),
            code: line.code.to_string(),
            name: line.name.to_string(),
            unit_price_minor: line.unit_price.get(),
            qty_milli: line.qty.get(),
            discount: DiscountV1::from_domain(line.discount),
            vat_bp: line.vat_rate.get(),
            price_inclusive: matches!(line.price_mode, PriceMode::Inclusive),
        }
    }

    pub fn into_domain(self) -> Result<CartLine> {
        Ok(CartLine {
            item_id: Ulid::from_u128(self.item_id),
            code: self.code.into_boxed_str(),
            name: self.name.into_boxed_str(),
            unit_price: Minor::new(self.unit_price_minor),
            qty: Milli::new(self.qty_milli),
            discount: self.discount.into_domain()?,
            vat_rate: Bp::vat(self.vat_bp).map_err(|_| WireError::OutOfRange)?,
            price_mode: if self.price_inclusive {
                PriceMode::Inclusive
            } else {
                PriceMode::Exclusive
            },
        })
    }
}

impl TicketV1 {
    #[must_use]
    pub fn from_domain(ticket: &Ticket, receipt_epoch: Option<u64>) -> Self {
        Self {
            id: ticket.id.to_u128(),
            terminal: ticket.terminal.to_u128(),
            rung_at_ms: ticket.rung_at_ms,
            receipt_no: ticket.receipt_no.as_ref().map(ToString::to_string),
            receipt_epoch,
            customer: ticket.customer.map(Ulid::to_u128),
            lines: ticket.lines.iter().map(LineV1::from_domain).collect(),
            ticket_discount: DiscountV1::from_domain(ticket.ticket_discount),
            tenders: ticket.tenders.iter().map(TenderV1::from_domain).collect(),
            net_minor: ticket.totals.net_total.get(),
            vat_minor: ticket.totals.vat_total.get(),
            discount_minor: ticket.totals.discount_total.get(),
            total_minor: ticket.totals.total.get(),
            change_minor: ticket.change.get(),
            overrides: ticket.overrides.iter().map(ToString::to_string).collect(),
        }
    }

    /// Rebuild the lines and tenders of a stored sale.
    ///
    /// Totals are not reconstructed into a [`crate::domain::TicketTotals`] here:
    /// the stored figures are evidence of what was charged, and recomputing them
    /// on read would quietly paper over a disagreement instead of surfacing it.
    /// The server compares stored against recomputed and raises a repair item.
    pub fn lines_and_tenders(self) -> Result<(Vec<CartLine>, Vec<Tender>)> {
        let lines = self
            .lines
            .into_iter()
            .map(LineV1::into_domain)
            .collect::<Result<Vec<_>>>()?;
        let tenders = self.tenders.into_iter().map(TenderV1::into_domain).collect();
        Ok((lines, tenders))
    }
}

/// Build the payload for one committed sale.
#[must_use]
pub fn sale_commit(ticket: &Ticket, receipt_epoch: Option<u64>, lease_next: Option<u64>) -> SaleCommitV1 {
    let stock = ticket
        .lines
        .iter()
        .map(|line| (line.item_id.to_u128(), line.qty.get().saturating_neg()))
        .collect();

    SaleCommitV1 {
        ticket: TicketV1::from_domain(ticket, receipt_epoch),
        lease_next,
        lease_epoch: receipt_epoch,
        stock,
    }
}

/// Convenience for tests and for the boot path: the boxed string a wire type
/// hands back.
#[must_use]
pub fn boxed(text: &str) -> Box<str> {
    text.into()
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
    use crate::cart::{Cart, CartLimits};
    use crate::money::Milli;

    fn item() -> Item {
        Item {
            id: Ulid::from_u128(1),
            code: boxed("SKU001"),
            name_en: boxed("Rice Miniket 5kg"),
            name_bn: boxed("মিনিকেট চাল ৫ কেজি"),
            unit: boxed("Nos"),
            price: Minor::new(43_000),
            cost: Minor::new(38_000),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            barcodes: vec![boxed("8690000000012")],
            on_hand: Milli::new(40_000),
            active: true,
        }
    }

    #[test]
    fn a_snapshot_round_trips_including_bangla() {
        let items = vec![item()];
        let bytes = encode_snapshot(&items, 77).unwrap();
        let (restored, cursor) = decode_snapshot(SNAPSHOT_SCHEMA, &bytes).unwrap();
        assert_eq!(restored, items);
        assert_eq!(cursor, 77, "a snapshot knows where to resume pulling");
        assert_eq!(&*restored[0].name_bn, "মিনিকেট চাল ৫ কেজি");
    }

    #[test]
    fn refuses_a_schema_it_does_not_understand() {
        let bytes = encode_snapshot(&[item()], 0).unwrap();
        assert_eq!(
            decode_snapshot(99, &bytes),
            Err(WireError::UnsupportedSchema { schema: 99 })
        );
    }

    #[test]
    fn refuses_bytes_that_are_not_a_snapshot() {
        assert_eq!(
            decode_snapshot(SNAPSHOT_SCHEMA, b"not postcard at all"),
            Err(WireError::Malformed)
        );
    }

    #[test]
    fn rejects_an_out_of_range_tax_rate_rather_than_selling_at_it() {
        let mut wire = ItemV1::from_domain(&item());
        wire.vat_bp = 20_000;
        let snapshot = SnapshotV1 { cursor: 0, items: vec![wire] };
        let bytes = postcard::to_allocvec(&snapshot).unwrap();
        assert_eq!(
            decode_snapshot(SNAPSHOT_SCHEMA, &bytes),
            Err(WireError::OutOfRange)
        );
    }

    #[test]
    fn a_sale_round_trips_with_its_tenders_and_totals() {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(), Milli::new(2_000)).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Wallet(boxed("bKash")),
            amount: Minor::new(50_000),
            reference: Some(boxed("TRX99")),
        });
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(50_000),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(9), Ulid::from_u128(7), 1_788_600_000_000)
            .unwrap();

        let payload = sale_commit(&ticket, Some(3), Some(124));
        let bytes = encode_sale(&payload).unwrap();
        let restored = decode_sale(SALE_SCHEMA, &bytes).unwrap();

        assert_eq!(restored, payload);
        assert_eq!(restored.ticket.total_minor, ticket.totals.total.get());
        assert_eq!(restored.lease_next, Some(124));
        // Stock moves out by what was sold.
        assert_eq!(restored.stock, vec![(1_u128, -2_000_i64)]);

        let (lines, tenders) = restored.ticket.lines_and_tenders().unwrap();
        assert_eq!(lines, ticket.lines);
        assert_eq!(tenders, ticket.tenders);
    }

    #[test]
    fn a_snapshot_of_a_realistic_catalogue_stays_small() {
        let items: Vec<Item> = (0..1_000)
            .map(|index| {
                let mut copy = item();
                copy.id = Ulid::from_u128(index + 1);
                copy
            })
            .collect();
        let bytes = encode_snapshot(&items, 0).unwrap();
        // Roughly 130 bytes an item with two scripts of names. Twenty thousand
        // items therefore lands near 2.6 MB, comfortably inside what a cheap
        // tablet hydrates in well under the cold start budget.
        let per_item = bytes.len() / items.len();
        assert!(per_item < 200, "{per_item} bytes an item is larger than expected");
    }
}
