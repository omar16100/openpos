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

use crate::auth::{self, Operator, Permissions, PinHash, SALT_LEN};
use crate::cart::{CartLine, Direction, Tender, TenderKind, Ticket};
use crate::domain::{Discount, PriceMode, Supply, VatBase};
use crate::ids::Ulid;
use crate::money::{Bp, Milli, Minor};
use crate::replica::Item;

/// Schema carried in the frame header for a snapshot payload.
/// Bumped when the tax base became a per-item choice, and again when a shop
/// could say a thing was exempt. Both older versions are still read; see
/// `ItemV1Legacy` and `ItemV2Legacy`.
pub const SNAPSHOT_SCHEMA: u16 = 4;
/// The snapshot as it was written before a shop could say a thing was exempt.
pub const SNAPSHOT_SCHEMA_V2: u16 = 2;
/// The snapshot as it was written before a shop could sort its shelves.
pub const SNAPSHOT_SCHEMA_V3: u16 = 3;
/// Schema carried in the frame header for a committed sale.
pub const SALE_SCHEMA: u16 = 5;

/// The sale format as it was written before a sale said who rang it.
///
/// Every sale this product has ever committed is one of these. A shop upgrading
/// has an outbox of them and a log of them, and both are read by the shapes
/// below.
pub const SALE_SCHEMA_V4: u16 = 4;

/// The sale format as it was written before a line could be exempt.
///
/// Sitting in an outbox waiting to be sent, or being reprinted from the log
/// months later. Read and converted, never written.
pub const SALE_SCHEMA_V2: u16 = 2;

/// The sale format as it was written before what the shop paid travelled with
/// the sale.
pub const SALE_SCHEMA_V3: u16 = 3;

/// The sale format as version 1 wrote it, read and converted.
///
/// A sale rung before a shop could say what it sold a thing by. Every one of
/// them meant pieces.
pub const SALE_SCHEMA_V1: u16 = 1;
/// Schema carried in the frame header for a batch of catalogue changes.
/// Bumped alongside the snapshot, for the same reason.
pub const DELTAS_SCHEMA: u16 = 4;
/// The catalogue batch as it was written before a shop could say a thing was
/// exempt.
pub const DELTAS_SCHEMA_V2: u16 = 2;
/// The catalogue batch as it was written before a shop could sort its shelves.
pub const DELTAS_SCHEMA_V3: u16 = 3;
/// Schema carried in the frame header for a sync acknowledgement watermark.
pub const ACK_SCHEMA: u16 = 1;
/// Schema carried in the frame header for a receipt number block.
pub const LEASE_SCHEMA: u16 = 1;
/// Schema carried in the frame header for the set of parked tickets.

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

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Malformed => f.write_str("the stored bytes did not decode"),
            Self::UnsupportedSchema { schema } => {
                write!(f, "schema {schema} is newer than this build reads")
            }
            Self::OutOfRange => f.write_str("a stored value is outside the range it may hold"),
        }
    }
}

impl core::error::Error for WireError {}

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
/// The version 1 shapes, for reading what version 1 wrote. Never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotV1Legacy {
    pub cursor: u64,
    pub items: Vec<ItemV1Legacy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltasV1Legacy {
    pub cursor: u64,
    pub upserts: Vec<ItemV1Legacy>,
    pub tombstones: Vec<u128>,
}

/// The snapshot and the catalogue batch as they were written before a shop
/// could say a thing was exempt. Frozen copies, for the reason every other one
/// in this file exists: they held the current item by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotV2Legacy {
    pub cursor: u64,
    pub items: Vec<ItemV2Legacy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltasV2Legacy {
    pub cursor: u64,
    pub upserts: Vec<ItemV2Legacy>,
    pub tombstones: Vec<u128>,
}

/// The snapshot and the catalogue batch as they were written before a shop
/// could sort its shelves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotV3Legacy {
    pub cursor: u64,
    pub items: Vec<ItemV3Legacy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltasV3Legacy {
    pub cursor: u64,
    pub upserts: Vec<ItemV3Legacy>,
    pub tombstones: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltasV1 {
    /// Server sequence after applying this batch.
    pub cursor: u64,
    pub upserts: Vec<ItemV1>,
    pub tombstones: Vec<u128>,
}

/// An item as version 1 wrote it, kept only to read what version 1 wrote.
///
/// postcard is positional, so a field added to the current shape cannot be read
/// out of these bytes and this struct cannot be edited. It exists to be decoded
/// and converted, never to be written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV1Legacy {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

impl ItemV1Legacy {
    /// Every item written before the tax base was a choice was taxed the
    /// ordinary way, because that was the only way there was.
    #[must_use]
    fn into_current(self) -> ItemV1 {
        ItemV1 {
            id: self.id,
            code: self.code,
            name_en: self.name_en,
            name_bn: self.name_bn,
            unit: self.unit,
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: false,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            supply: 0,
            category: String::new(),
        }
    }
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
    /// True when VAT is charged on the price before discounts, so a discount
    /// comes out of the shop's margin and the tax does not move.
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    /// Standard rated, zero rated or exempt, as the number `Supply` is stored
    /// as. Appended, because these bytes are read positionally and every item
    /// on every device was written before this field existed.
    #[serde(default)]
    pub supply: u8,
    /// What the shop calls this kind of thing. Empty for the ones nobody has
    /// sorted. Appended, like everything before it.
    #[serde(default)]
    pub category: String,
}

/// An item as it was written before the shop could sort its shelves.
///
/// Frozen, for the reason every other copy in this file is: the shapes below
/// hold the current item by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV3Legacy {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    #[serde(default)]
    pub supply: u8,
}

impl From<ItemV3Legacy> for ItemV1 {
    fn from(old: ItemV3Legacy) -> Self {
        Self {
            id: old.id,
            code: old.code,
            name_en: old.name_en,
            name_bn: old.name_bn,
            unit: old.unit,
            price_minor: old.price_minor,
            cost_minor: old.cost_minor,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            barcodes: old.barcodes,
            on_hand_milli: old.on_hand_milli,
            active: old.active,
            supply: old.supply,
            // Nobody sorted the shelves in a build that could not.
            category: String::new(),
        }
    }
}

/// An item as it was written before a shop could say a thing was exempt.
///
/// A frozen copy, and the reason for it is the one this file keeps learning:
/// the legacy shapes above hold `ItemV1` by name, so a field added to the
/// current item silently changes what those old shapes claim to be, and every
/// standing state written by a shipped build stops decoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV2Legacy {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

impl From<ItemV2Legacy> for ItemV1 {
    fn from(old: ItemV2Legacy) -> Self {
        Self {
            id: old.id,
            code: old.code,
            name_en: old.name_en,
            name_bn: old.name_bn,
            unit: old.unit,
            price_minor: old.price_minor,
            cost_minor: old.cost_minor,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            barcodes: old.barcodes,
            on_hand_milli: old.on_hand_milli,
            active: old.active,
            // Everything written before the distinction existed was sold at
            // whatever rate it carried, which is the standard treatment.
            supply: 0,
            category: String::new(),
        }
    }
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
    /// Frozen with the price, so the server revalidating this sale charges the
    /// tax the till charged rather than the tax the item carries today.
    pub vat_on_undiscounted: bool,
    /// What it was sold by. Frozen for the same reason: an item re-measured
    /// from kilos to litres must not change what last week's receipt says.
    pub unit: String,
    /// Standard, zero rated or exempt, frozen with the price. What a line was
    /// on the day is what that day's return declares, whatever the shop
    /// reclassifies the item as afterwards. Appended.
    #[serde(default)]
    pub supply: u8,
    /// What the shop paid for one of these, frozen with the price, so what a
    /// day made stays what it made when the supplier's price moves. Zero where
    /// the shop has never said. Appended.
    #[serde(default)]
    pub cost_minor: i64,
}

/// An item and a parked basket exactly as they stand today, frozen.
///
/// Not because anything has changed yet, but because the standing states below
/// hold them and the rule this file lives by is that a legacy shape names only
/// frozen shapes. Every time that rule was bent, the next field added silently
/// changed a struct kept to read bytes nobody can rewrite, and a shop woke up
/// to a till that could not open its own ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV4Legacy {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    #[serde(default)]
    pub supply: u8,
    #[serde(default)]
    pub category: String,
}

impl From<ItemV4Legacy> for ItemV1 {
    fn from(old: ItemV4Legacy) -> Self {
        Self {
            id: old.id,
            code: old.code,
            name_en: old.name_en,
            name_bn: old.name_bn,
            unit: old.unit,
            price_minor: old.price_minor,
            cost_minor: old.cost_minor,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            barcodes: old.barcodes,
            on_hand_milli: old.on_hand_milli,
            active: old.active,
            supply: old.supply,
            category: old.category,
        }
    }
}

/// A line as it stands today, frozen for the reason above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV4Legacy {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub unit: String,
    #[serde(default)]
    pub supply: u8,
    #[serde(default)]
    pub cost_minor: i64,
}

impl From<LineV4Legacy> for LineV1 {
    fn from(old: LineV4Legacy) -> Self {
        Self {
            item_id: old.item_id,
            code: old.code,
            name: old.name,
            unit_price_minor: old.unit_price_minor,
            qty_milli: old.qty_milli,
            discount: old.discount,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            unit: old.unit,
            supply: old.supply,
            cost_minor: old.cost_minor,
        }
    }
}

/// Parked baskets as they stand today, frozen for the reason above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV4Legacy {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV4Legacy>,
    pub ticket_discount: DiscountV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV4Legacy {
    pub tickets: Vec<HeldTicketV4Legacy>,
}

impl From<HeldTicketsV4Legacy> for HeldTicketsV1 {
    fn from(old: HeldTicketsV4Legacy) -> Self {
        Self {
            tickets: old
                .tickets
                .into_iter()
                .map(|one| HeldTicketV1 {
                    id: one.id,
                    held_at_ms: one.held_at_ms,
                    customer: one.customer,
                    label: one.label,
                    lines: one.lines.into_iter().map(Into::into).collect(),
                    ticket_discount: one.ticket_discount,
                    // Nothing this old wrote down what a supervisor had allowed
                    // on a basket, or which way round it was.
                    overrides: alloc::vec::Vec::new(),
                    refund: false,
                    refund_of: None,
                })
                .collect(),
        }
    }
}

/// A line as it was written before the cost was frozen onto it.
///
/// Frozen, for the reason every copy in this file is: the ticket and the parked
/// baskets hold `LineV1` by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV3Legacy {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub unit: String,
    #[serde(default)]
    pub supply: u8,
}

impl From<LineV3Legacy> for LineV1 {
    fn from(old: LineV3Legacy) -> Self {
        Self {
            item_id: old.item_id,
            code: old.code,
            name: old.name,
            unit_price_minor: old.unit_price_minor,
            qty_milli: old.qty_milli,
            discount: old.discount,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            unit: old.unit,
            supply: old.supply,
            // Rung before the shop's own cost travelled with the sale. Nothing
            // is known about what that one cost, and zero says so: a margin
            // report counts those apart rather than calling them free.
            cost_minor: 0,
        }
    }
}

/// A line as it was written before a shop could say a thing was exempt.
///
/// Frozen, for the reason `ItemV2Legacy` is: the ticket and the parked baskets
/// hold `LineV1` by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV2Legacy {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub unit: String,
}

impl From<LineV2Legacy> for LineV1 {
    fn from(old: LineV2Legacy) -> Self {
        Self {
            item_id: old.item_id,
            code: old.code,
            name: old.name,
            unit_price_minor: old.unit_price_minor,
            qty_milli: old.qty_milli,
            discount: old.discount,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            unit: old.unit,
            supply: 0,
            cost_minor: 0,
        }
    }
}

/// A line as version 1 of the sale format wrote one, without the unit.
///
/// Kept to read what version 1 wrote and never written. postcard is positional,
/// so without this every sale committed before this change would stop decoding:
/// the outbox could not send them and the receipt could not reprint them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV1Legacy {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
}

impl From<LineV1Legacy> for LineV1 {
    fn from(old: LineV1Legacy) -> Self {
        Self {
            item_id: old.item_id,
            code: old.code,
            name: old.name,
            unit_price_minor: old.unit_price_minor,
            qty_milli: old.qty_milli,
            discount: old.discount,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            // A sale rung before the shop could say what it sold a thing by.
            // Pieces is what every one of them meant.
            unit: String::from("Nos"),
            supply: 0,
            cost_minor: 0,
        }
    }
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
    /// Who was signed in when this was rung, as an id. Appended.
    ///
    /// The id rather than the name, so the name resolves as the person is
    /// called now: an operator record is kept rather than deleted for exactly
    /// this. `None` where nobody was signed in, which is a real state of a till
    /// that never refuses a sale for want of one.
    #[serde(default)]
    pub operator: Option<u128>,
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
    /// For a refund, the receipt it reverses, when the customer had it.
    ///
    /// A refund is recognisable from its negative total, but the paper it
    /// reverses is not recoverable from anything else, and it is the first thing
    /// asked for when a refund is questioned later.
    pub refund_of: Option<String>,
}

/// A ticket as it was written before it said who rang it.
///
/// Its lines are the copy frozen above rather than the live shape: a frozen
/// shape that names a live one changes with it and stops reading the bytes it
/// was kept for. That has already happened once in this product, on the wire
/// rather than here, and went unnoticed for seventy-four commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketV4Legacy {
    pub id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub customer: Option<u128>,
    pub lines: Vec<LineV4Legacy>,
    pub ticket_discount: DiscountV1,
    pub tenders: Vec<TenderV1>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
}

/// A sale committed before it said who rang it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleCommitV4Legacy {
    pub ticket: TicketV4Legacy,
    pub lease_next: Option<u64>,
    pub lease_epoch: Option<u64>,
    pub stock: Vec<(u128, i64)>,
    pub refund_of: Option<String>,
}

/// A ticket as it was written before the cost travelled with the sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketV3Legacy {
    pub id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub customer: Option<u128>,
    pub lines: Vec<LineV3Legacy>,
    pub ticket_discount: DiscountV1,
    pub tenders: Vec<TenderV1>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
}

/// A sale as it was written before the cost travelled with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleCommitV3Legacy {
    pub ticket: TicketV3Legacy,
    pub lease_next: Option<u64>,
    pub lease_epoch: Option<u64>,
    pub stock: Vec<(u128, i64)>,
    pub refund_of: Option<String>,
}

impl From<SaleCommitV3Legacy> for SaleCommitV1 {
    fn from(old: SaleCommitV3Legacy) -> Self {
        let ticket = old.ticket;
        Self {
            ticket: TicketV1 {
                id: ticket.id,
                terminal: ticket.terminal,
                rung_at_ms: ticket.rung_at_ms,
                receipt_no: ticket.receipt_no,
                receipt_epoch: ticket.receipt_epoch,
                customer: ticket.customer,
                lines: ticket.lines.into_iter().map(Into::into).collect(),
                ticket_discount: ticket.ticket_discount,
                tenders: ticket.tenders,
                net_minor: ticket.net_minor,
                vat_minor: ticket.vat_minor,
                discount_minor: ticket.discount_minor,
                total_minor: ticket.total_minor,
                change_minor: ticket.change_minor,
                overrides: ticket.overrides,
                // Rung by a build that did not ask who was at the till.
                operator: None,
            },
            lease_next: old.lease_next,
            lease_epoch: old.lease_epoch,
            stock: old.stock,
            refund_of: old.refund_of,
        }
    }
}

/// Parked baskets as they were written before the cost travelled with a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV3Legacy {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV3Legacy>,
    pub ticket_discount: DiscountV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV3Legacy {
    pub tickets: Vec<HeldTicketV3Legacy>,
}

impl From<HeldTicketsV3Legacy> for HeldTicketsV1 {
    fn from(old: HeldTicketsV3Legacy) -> Self {
        Self {
            tickets: old
                .tickets
                .into_iter()
                .map(|one| HeldTicketV1 {
                    id: one.id,
                    held_at_ms: one.held_at_ms,
                    customer: one.customer,
                    label: one.label,
                    lines: one.lines.into_iter().map(Into::into).collect(),
                    ticket_discount: one.ticket_discount,
                    // Nothing this old wrote down what a supervisor had allowed
                    // on a basket, or which way round it was.
                    overrides: alloc::vec::Vec::new(),
                    refund: false,
                    refund_of: None,
                })
                .collect(),
        }
    }
}

/// A ticket as it was written before a line could be exempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketV2Legacy {
    pub id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub customer: Option<u128>,
    pub lines: Vec<LineV2Legacy>,
    pub ticket_discount: DiscountV1,
    pub tenders: Vec<TenderV1>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
}

/// A sale as it was written before a line could be exempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleCommitV2Legacy {
    pub ticket: TicketV2Legacy,
    pub lease_next: Option<u64>,
    pub lease_epoch: Option<u64>,
    pub stock: Vec<(u128, i64)>,
    pub refund_of: Option<String>,
}

impl From<SaleCommitV2Legacy> for SaleCommitV1 {
    fn from(old: SaleCommitV2Legacy) -> Self {
        let ticket = old.ticket;
        Self {
            ticket: TicketV1 {
                id: ticket.id,
                terminal: ticket.terminal,
                rung_at_ms: ticket.rung_at_ms,
                receipt_no: ticket.receipt_no,
                receipt_epoch: ticket.receipt_epoch,
                customer: ticket.customer,
                lines: ticket.lines.into_iter().map(Into::into).collect(),
                ticket_discount: ticket.ticket_discount,
                tenders: ticket.tenders,
                net_minor: ticket.net_minor,
                vat_minor: ticket.vat_minor,
                discount_minor: ticket.discount_minor,
                total_minor: ticket.total_minor,
                change_minor: ticket.change_minor,
                overrides: ticket.overrides,
                // Rung by a build that did not ask who was at the till.
                operator: None,
            },
            lease_next: old.lease_next,
            lease_epoch: old.lease_epoch,
            stock: old.stock,
            refund_of: old.refund_of,
        }
    }
}

/// A ticket as version 1 wrote one, whose lines carry no unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketV1Legacy {
    pub id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub customer: Option<u128>,
    pub lines: Vec<LineV1Legacy>,
    pub ticket_discount: DiscountV1,
    pub tenders: Vec<TenderV1>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
}

/// A sale as version 1 wrote one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleCommitV1Legacy {
    pub ticket: TicketV1Legacy,
    pub lease_next: Option<u64>,
    pub lease_epoch: Option<u64>,
    pub stock: Vec<(u128, i64)>,
    pub refund_of: Option<String>,
}

impl From<SaleCommitV1Legacy> for SaleCommitV1 {
    fn from(old: SaleCommitV1Legacy) -> Self {
        let ticket = old.ticket;
        Self {
            ticket: TicketV1 {
                id: ticket.id,
                terminal: ticket.terminal,
                rung_at_ms: ticket.rung_at_ms,
                receipt_no: ticket.receipt_no,
                receipt_epoch: ticket.receipt_epoch,
                customer: ticket.customer,
                lines: ticket.lines.into_iter().map(Into::into).collect(),
                ticket_discount: ticket.ticket_discount,
                tenders: ticket.tenders,
                net_minor: ticket.net_minor,
                vat_minor: ticket.vat_minor,
                discount_minor: ticket.discount_minor,
                total_minor: ticket.total_minor,
                change_minor: ticket.change_minor,
                overrides: ticket.overrides,
                // Rung by a build that did not ask who was at the till.
                operator: None,
            },
            lease_next: old.lease_next,
            lease_epoch: old.lease_epoch,
            stock: old.stock,
            refund_of: old.refund_of,
        }
    }
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

/// One ticket a cashier put aside, as persisted.
///
/// A parked basket is not money yet, but it is a customer standing at the
/// counter. Losing it to a flat battery means re-scanning everything in front of
/// them, so it is written down like anything else that would be painful to
/// reconstruct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV1 {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV1>,
    pub ticket_discount: DiscountV1,
    /// What was waived on this basket before it was parked, in the words that
    /// go on the paper.
    ///
    /// Appended, never inserted. Without it a basket a supervisor approved came
    /// back with the approval gone: the price was still the approved one and
    /// the line saying who allowed it was not on the receipt, which is the one
    /// line a shop reads when it asks why this price differs from the shelf.
    #[serde(default)]
    pub overrides: Vec<String>,
    /// True when this was parked as a refund, and the receipt it is against.
    ///
    /// A parked refund came back as a sale with negative lines on it, because
    /// nothing on the way in said which direction it was. That is money going
    /// the wrong way with nothing on the screen to say so.
    #[serde(default)]
    pub refund: bool,
    #[serde(default)]
    pub refund_of: Option<String>,
}

/// Every ticket currently parked.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV1 {
    pub tickets: Vec<HeldTicketV1>,
}

/// A parked basket as every build up to schema 15 wrote it: before it carried
/// what a supervisor had allowed on it, or which way round it was.
///
/// Frozen for the reason every copy in this file is. postcard is positional, so
/// without this the parked baskets on a device upgrading are read a field short
/// and the whole standing state fails with them, taking the day's unsent sales
/// as well.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV5Legacy {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV4Legacy>,
    pub ticket_discount: DiscountV1,
}

impl From<HeldTicketV5Legacy> for HeldTicketV1 {
    fn from(old: HeldTicketV5Legacy) -> Self {
        Self {
            id: old.id,
            held_at_ms: old.held_at_ms,
            customer: old.customer,
            label: old.label,
            lines: old.lines.into_iter().map(Into::into).collect(),
            ticket_discount: old.ticket_discount,
            // That build did not write down what was allowed on this basket,
            // and nothing may invent it: the receipt says what the device knew.
            overrides: alloc::vec::Vec::new(),
            // Nor which way round it was. Every basket parked by a build before
            // this one was parked as a sale, because the screen offered no
            // other kind: a refund could be parked, and came back as a sale,
            // which is the defect this field exists to fix rather than a state
            // anybody chose.
            refund: false,
            refund_of: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV5Legacy {
    pub tickets: Vec<HeldTicketV5Legacy>,
}

impl From<HeldTicketsV5Legacy> for HeldTicketsV1 {
    fn from(old: HeldTicketsV5Legacy) -> Self {
        Self {
            tickets: old.tickets.into_iter().map(Into::into).collect(),
        }
    }
}

/// Parked baskets as they were written before a line could be exempt.
///
/// Frozen and never written. Every standing state below holds the parked
/// baskets by name, so a field added to a line changes all of them at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV2Legacy {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV2Legacy>,
    pub ticket_discount: DiscountV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV2Legacy {
    pub tickets: Vec<HeldTicketV2Legacy>,
}

impl From<HeldTicketsV2Legacy> for HeldTicketsV1 {
    fn from(old: HeldTicketsV2Legacy) -> Self {
        Self {
            tickets: old
                .tickets
                .into_iter()
                .map(|one| HeldTicketV1 {
                    id: one.id,
                    held_at_ms: one.held_at_ms,
                    customer: one.customer,
                    label: one.label,
                    lines: one.lines.into_iter().map(Into::into).collect(),
                    ticket_discount: one.ticket_discount,
                    // Nothing this old wrote down what a supervisor had allowed
                    // on a basket, or which way round it was.
                    overrides: alloc::vec::Vec::new(),
                    refund: false,
                    refund_of: None,
                })
                .collect(),
        }
    }
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

pub const TERMINAL_SCHEMA: u16 = 21;

/// What version 20 wrote: no record of a PIN got wrong.
///
/// A device on that build held its lockouts in memory, so closing the tab
/// cleared them. Read and carried forward with none, which is what it had: a
/// device coming from there has nobody locked out, and the five attempts start
/// again, which is the same thing that build did on every reload.
pub const TERMINAL_SCHEMA_V20: u16 = 20;

/// What version 19 wrote: a shop with no say over which languages it offers.
///
/// Read and carried forward with the say left empty, which means every language
/// this build has: that is what a device coming from that build was doing, and
/// it goes on doing it until the shop says otherwise.
pub const TERMINAL_SCHEMA_V19: u16 = 19;

/// What version 18 wrote: everything this build writes except the open drawer.
///
/// A device on that build keeps its drawer in the critical log and nowhere
/// else, which is why the log could not be dropped under one. Read and carried
/// forward with no drawer, which is the truth about it: the log it came with
/// still holds the frames, and the replay finds them.
pub const TERMINAL_SCHEMA_V18: u16 = 18;

/// What version 17 wrote: a record of whether the device had been round the
/// shelf. Written for a day. The figures that record is about live in the
/// catalogue snapshot, which is only rewritten when the delta log has grown, so
/// a sweep reaches the disk by luck or not at all: a till came back from a
/// reload saying it had been round the shelf and holding the catalogue's own
/// figures, which are usually zero, and its shop's rule refused everything
/// scanned at it. Going round is a thing this run has done, so it is not
/// written down at all now.
pub const TERMINAL_SCHEMA_V17: u16 = 17;

/// What version 16 wrote. It kept no record of whether the device had been
/// round the shelf, so a till upgrading from it says nothing about the shelf
/// until it has been round once, which is the safe end of that question.
pub const TERMINAL_SCHEMA_V16: u16 = 16;

/// What version 15 wrote. Its parked baskets say nothing about what a
/// supervisor allowed on them, or which way round they are.
pub const TERMINAL_SCHEMA_V15: u16 = 15;

/// What every build up to 14 wrote. Its trail entries carry no receipt.
pub const TERMINAL_SCHEMA_V14: u16 = 14;

/// The standing state as version 1 wrote it.
///
/// Kept only to read what version 1 wrote, and never written. postcard is
/// positional and does not honour a serde default for a field that is simply
/// absent from the bytes, so without this every till in every shop would fail to
/// read its own leases, its parked sales and its credential the first time it
/// started on a build that knew about wallets.
pub const TERMINAL_SCHEMA_V1: u16 = 1;

/// The standing state as version 2 wrote it: wallets, but no drawer waiting to
/// be sent. Kept for the same reason as version 1.
pub const TERMINAL_SCHEMA_V2: u16 = 2;

/// The standing state as version 3 wrote it: a drawer waiting to be sent, but
/// no record of who counted it.
pub const TERMINAL_SCHEMA_V3: u16 = 3;

/// The version before the people who buy on account were held on the device.
pub const TERMINAL_SCHEMA_V4: u16 = 4;

/// The version before a device wrote down when its credential was issued.
pub const TERMINAL_SCHEMA_V5: u16 = 5;

/// The version before a device kept what it allowed until the shop had it.
pub const TERMINAL_SCHEMA_V6: u16 = 6;

/// The version before a shop could say what to do when a basket asks for more
/// than the shelf holds.
pub const TERMINAL_SCHEMA_V7: u16 = 7;

/// The version before a till could sell something the shop had never heard of.
pub const TERMINAL_SCHEMA_V8: u16 = 8;

/// The version before a till could write down somebody who buys on account.
pub const TERMINAL_SCHEMA_V9: u16 = 9;

/// The version before a shop could say a thing was zero rated or exempt.
pub const TERMINAL_SCHEMA_V10: u16 = 10;

/// The version before a shop could sort its shelves into its own categories.
pub const TERMINAL_SCHEMA_V11: u16 = 11;

/// The version before what the shop paid travelled with a line, so its parked
/// baskets carry no cost.
pub const TERMINAL_SCHEMA_V12: u16 = 12;

/// The version before a shop could cap what somebody owes it.
pub const TERMINAL_SCHEMA_V13: u16 = 13;

/// An operator as stored on the device.
///
/// The PIN travels and rests as a derived key with its salt and round count, so
/// a terminal never holds anything that can be turned back into the digits a
/// cashier types. The round count travels with it rather than being a constant,
/// so raising the cost later does not lock out everyone who set a PIN before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorV1 {
    pub id: u128,
    pub name: String,
    pub salt: Vec<u8>,
    pub rounds: u32,
    pub key: Vec<u8>,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

/// What a terminal owns independently of the sales it has yet to deliver.
///
/// Held in a blob slot rather than the critical log because that log is emptied
/// the moment the server confirms everything in it, which is the ordinary end
/// of a trading day. Numbers already leased and baskets already parked must
/// outlive that, or a shop that synced last night opens tomorrow with nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV1 {
    /// Blocks still in hand, active first, each at the position it had reached.
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV1,
    /// Sales that closed with no number available and are still waiting for one.
    pub unnumbered: u64,
    /// Who may stand at this till. Held on the device because the whole point
    /// is that a cashier can sign in with the internet down.
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    /// The credential this terminal syncs with.
    ///
    /// Kept beside the ledger rather than wherever a platform finds convenient,
    /// because it belongs to the same thing: wiping the till wipes the
    /// credential, and a device restored from another terminal's files is
    /// already refused by the owner check rather than arriving with a working
    /// token for a shop it is not part of.
    #[serde(default)]
    pub token: Option<String>,
    /// The shop's own details, for the top of a receipt. Held here because a
    /// receipt is printed with the internet down, so they have to be on the
    /// device before they are wanted.
    #[serde(default)]
    pub shop: Option<ShopV1>,
    /// Drawers counted and closed and not yet sent to the shop.
    ///
    /// Here rather than in the log because the log is truncated when every sale
    /// in it has been acknowledged, and a counted drawer that went with it is
    /// an accountability record nobody can reconstruct: the cashier counted, the
    /// till agreed, and then neither of them can prove it.
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    /// The people who buy on account. Held on the device for the same reason
    /// the operators are: a cashier writes a sale to somebody's account with
    /// the internet down, and a name typed from memory is how one Karim ends up
    /// paying for another Karim's rice.
    #[serde(default)]
    pub customers: Vec<CustomerV1>,
    /// When this device's credential was issued, and how long the shop said one
    /// lasts. Held so a till can renew before it expires rather than stopping
    /// dead a year after it was enrolled, and held across restarts so a tablet
    /// switched off every night does not renew every morning.
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    /// Privileged actions this device allowed and the shop has not been told
    /// about.
    ///
    /// Here rather than in the log for the reason the counted drawers are: the
    /// log is emptied when every sale in it has been acknowledged, and an
    /// override that went with it is an accountability record nobody can
    /// reconstruct. The question asked afterwards is never "was this allowed"
    /// but "who allowed it", and a device that answered that only until its
    /// next drain was answering nobody.
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1>,
    /// How many privileged actions this device has allowed, ever.
    ///
    /// Its own counter rather than a clock: two of them in one millisecond are
    /// possible on a fast device, and the shop has to be able to tell one from
    /// the other when it stores them. Never reset, because a number that starts
    /// again is a number that collides with what the shop already holds.
    #[serde(default)]
    pub allowed_seq: u64,
    /// Items a till wrote down itself, and the shop has not got.
    ///
    /// A delivery arrives during an outage and its barcode is in nobody's
    /// catalogue. A till that could only say "no such item" would lose the sale
    /// and the shop would sell it off the paper, so the till writes the item
    /// down and sells it. Here rather than in the log for the reason the counted
    /// drawers are: the log is emptied when its sales are acknowledged, and an
    /// item that went with it is a sale in the shop's books naming something
    /// nobody can look up.
    #[serde(default)]
    pub unsent_items: Vec<ItemV1>,
    /// People a till wrote down itself, and the shop has not got.
    ///
    /// Somebody buys on account who is in nobody's list yet. Writing them down
    /// at the till is what keeps two people with one name apart: a sale against
    /// a typed name is added up against the spelling, and the second Karim ends
    /// up paying for the first one's rice.
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV1>,
    /// The drawer that is open, when the log under it has been dropped.
    ///
    /// Absent on a device whose log still holds its drawer, which is every
    /// device that has not yet had every sale acknowledged. See `OpenDrawerV1`.
    #[serde(default)]
    pub open_drawer: Option<OpenDrawerV1>,
    /// PINs got wrong on this device, per person. Appended, never inserted.
    #[serde(default)]
    pub wrong_pins: Vec<WrongPinsV1>,
}

/// A privileged action a device allowed, waiting to be sent.
///
/// Written down because the question asked afterwards is never "was this
/// allowed" but "who allowed it". An override with nobody's name on it is
/// indistinguishable from theft when the variance is read a week later.
/// One privileged action, as every build up to schema 14 wrote it.
///
/// Frozen. `AllowedV1` gained the receipt a reprint was of, and postcard is
/// positional: without this copy, every trail entry a device is still holding
/// would be read one field short and the whole standing state would fail with
/// it, taking the day's unsent sales and the parked baskets. What the shop
/// already holds is safe either way; these are the ones only the device has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedV1Legacy {
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
}

impl From<AllowedV1Legacy> for AllowedV1 {
    fn from(old: AllowedV1Legacy) -> Self {
        Self {
            seq: old.seq,
            at_ms: old.at_ms,
            action: old.action,
            bp: old.bp,
            operator: old.operator,
            operator_name: old.operator_name,
            authorised_by: old.authorised_by,
            authorised_by_name: old.authorised_by_name,
            // Nothing written before this knew which receipt a reprint was of.
            receipt_no: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedV1 {
    /// This device's own count, so the shop can tell two identical actions in
    /// one millisecond apart and store each exactly once.
    pub seq: u64,
    pub at_ms: u64,
    /// 1 discount, 2 price override, 3 refund, 4 void a line, 5 open the
    /// drawer, 6 close the drawer. Numbers rather than the enum, because these
    /// bytes outlive the build that wrote them and a variant appended in the
    /// middle would turn a refund into a void.
    pub action: u8,
    /// Basis points, for a discount. Zero for everything else.
    pub bp: u32,
    /// Who did it, and what they were called at the time. The name is copied
    /// for the reason the drawer's is: somebody since renamed or gone from the
    /// shop still has to be the person this belongs to.
    pub operator: u128,
    pub operator_name: String,
    /// Who allowed it, when it was not the operator's own permission. Zero when
    /// nobody had to: the cashier's own ceiling covered it.
    pub authorised_by: u128,
    pub authorised_by_name: String,
    /// The receipt a reprint was of, when the entry is one.
    ///
    /// `None` for every other kind, and for anything written before this
    /// existed. A reprint is a second piece of paper somebody can hand over,
    /// and the question a shop asks afterwards is which one.
    pub receipt_no: Option<String>,
}

/// When a credential was issued and how long one lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialV1 {
    /// Device clock when this credential was taken. Only ever compared with
    /// this device's own clock, which is the one thing it can trust about time.
    pub taken_at_ms: u64,
    /// What the shop said a credential lasts, in milliseconds. Zero until the
    /// device has renewed once and been told: a freshly enrolled device knows
    /// only that it has one.
    pub lifetime_ms: u64,
}

/// Somebody the shop lets buy on account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerV1 {
    pub id: u128,
    pub name: String,
    /// How the shop chases them. Optional, because a shop that knows exactly
    /// who "Karim, flat 3" is should not be stopped by a form.
    pub phone: Option<String>,
    /// False when the shop has stopped letting them buy on account. Kept rather
    /// than deleted: what they already owe does not stop being owed.
    pub active: bool,
    /// Their Business Identification Number, when the buyer is a business.
    ///
    /// Costs nothing to carry now and is what a tax invoice here has to name:
    /// a shop selling to another business writes it on the paper. Appended,
    /// never inserted, like every field before it.
    #[serde(default)]
    pub bin: Option<String>,
    /// The most the shop will let them owe at once, in poisha. Zero is no
    /// limit, which is what every shop has until it says otherwise.
    ///
    /// A shop that sells on account all day and never says stop is a shop
    /// whose cash is on somebody else's shelf. Appended, never inserted.
    #[serde(default)]
    pub limit_minor: i64,
}

/// Somebody who buys on account, as version 14 wrote them: with a cap on what
/// they may owe, and nothing after it.
///
/// A copy rather than the live shape, for the reason every copy in this file
/// exists: the standing states below hold a customer by name, so the next field
/// added to the live one silently changes what version 14's bytes claim to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerV4Legacy {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub limit_minor: i64,
}

impl From<CustomerV4Legacy> for CustomerV1 {
    fn from(old: CustomerV4Legacy) -> Self {
        Self {
            id: old.id,
            name: old.name,
            phone: old.phone,
            active: old.active,
            bin: old.bin,
            limit_minor: old.limit_minor,
        }
    }
}

/// Somebody who buys on account, as written before the shop could cap what
/// they owe.
///
/// Frozen, for the reason every copy in this file is: the standing states hold
/// the current customer by name, so a field added to it changes all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerV3Legacy {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
    #[serde(default)]
    pub bin: Option<String>,
}

impl From<CustomerV3Legacy> for CustomerV1 {
    fn from(old: CustomerV3Legacy) -> Self {
        Self {
            id: old.id,
            name: old.name,
            phone: old.phone,
            active: old.active,
            bin: old.bin,
            // No shop that could not say had said.
            limit_minor: 0,
        }
    }
}

/// Somebody who buys on account, as written before the buyer could have a BIN.
///
/// Referenced by every standing state before version 10, which is why it exists
/// separately rather than those pointing at the current shape: a legacy struct
/// that quietly grows a field with the current one stops reading the bytes it
/// was kept for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerV2Legacy {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
}

impl From<CustomerV2Legacy> for CustomerV1 {
    fn from(old: CustomerV2Legacy) -> Self {
        Self {
            id: old.id,
            name: old.name,
            phone: old.phone,
            active: old.active,
            // Nobody was ever asked for one.
            bin: None,
            limit_minor: 0,
        }
    }
}

/// A drawer counted and closed, waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftV1 {
    pub id: u128,
    /// Who counted it, and what they were called at the time.
    ///
    /// The name is copied rather than looked up later, for the reason a price
    /// on a line is: somebody who has since left the shop, or been renamed,
    /// still has to be the person this variance belongs to.
    pub closed_by: u128,
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

/// A drawer that is still open, written down so the log under it can go.
///
/// While the log is there, an open drawer is the frames in it: the opening, the
/// cash that moved, and every sale rung under it. Replaying them is what makes
/// the drawer figure and the sales figure agree by construction.
///
/// The log is emptied once the shop has taken every sale in it, and it cannot
/// be emptied under a drawer that lives nowhere else: a shop that never counts
/// its drawer never lets a byte go, which is about 145 KB of every thousand
/// sales. So this is written at the moment the log is dropped, and it carries
/// the sequence it was folded through: the next boot starts from it and replays
/// only what came after. The same shape as the catalogue's snapshot and its
/// delta log, and it keeps what the replay gave, because this is a checkpoint
/// of the replay rather than a second opinion about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawerV1 {
    pub id: u128,
    pub terminal: u128,
    pub opened_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    /// Gross under each kind, which is what an X report shows. Kept per kind
    /// rather than as cash and not-cash, because the report says which wallet.
    pub tenders: Vec<DrawerTenderV1>,
    /// Every non-sale movement, in the order it happened. The audit trail a
    /// variance is read against, and the reason it is here rather than summed:
    /// a drawer short by five hundred with a drop of five hundred in it is a
    /// different evening from one with no movements at all.
    pub movements: Vec<DrawerMovementV1>,
    /// The log sequence this was folded through. Frames at or below it are
    /// already in the figures above, so a boot skips them: that is what makes
    /// a crash between writing this and dropping the log cost nothing.
    pub folded_through: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawerTenderV1 {
    pub kind: TenderKindV1,
    pub amount_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawerMovementV1 {
    /// True for money in, false for money out. The direction is stored rather
    /// than a signed amount so a reader that ignores it cannot silently turn a
    /// drop into a top-up.
    pub inward: bool,
    pub amount_minor: i64,
    pub reason: String,
    pub at_ms: u64,
}

/// A drawer as version 3 wrote one, before it recorded who counted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftV3Legacy {
    pub id: u128,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

impl From<ClosedShiftV3Legacy> for ClosedShiftV1 {
    fn from(old: ClosedShiftV3Legacy) -> Self {
        Self {
            id: old.id,
            // A drawer counted before the till wrote down who counted it. The
            // shop knows it happened and cannot be told by whom.
            closed_by: 0,
            closed_by_name: String::new(),
            opened_at_ms: old.opened_at_ms,
            closed_at_ms: old.closed_at_ms,
            opening_float_minor: old.opening_float_minor,
            sales: old.sales,
            cash_sales_minor: old.cash_sales_minor,
            non_cash_sales_minor: old.non_cash_sales_minor,
            cash_in_minor: old.cash_in_minor,
            cash_out_minor: old.cash_out_minor,
            expected_cash_minor: old.expected_cash_minor,
            counted_cash_minor: old.counted_cash_minor,
            variance_minor: old.variance_minor,
        }
    }
}

/// Somebody who buys on account, as schema 15 wrote them.
///
/// Identical to version 14's, because nothing about a customer changed in 15
/// or 16. A copy all the same, for the reason every copy in this file exists:
/// the next field added to the live shape would silently change what version
/// 15's bytes claim to be.
pub type CustomerV5Legacy = CustomerV4Legacy;

/// One trail entry as schema 15 wrote it: with the receipt a reprint was of,
/// which is what 15 added, and nothing after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedV5Legacy {
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
    #[serde(default)]
    pub receipt_no: Option<String>,
}

impl From<AllowedV5Legacy> for AllowedV1 {
    fn from(old: AllowedV5Legacy) -> Self {
        Self {
            seq: old.seq,
            at_ms: old.at_ms,
            action: old.action,
            bp: old.bp,
            operator: old.operator,
            operator_name: old.operator_name,
            authorised_by: old.authorised_by,
            authorised_by_name: old.authorised_by_name,
            receipt_no: old.receipt_no,
        }
    }
}

/// An item a till wrote down, as schema 15 wrote it.
///
/// Identical to version 14's for the same reason the customer is.
pub type ItemV5Legacy = ItemV4Legacy;


// --- the shapes as schema 16 wrote them ------------------------------------
//
// Copies of what a parked basket, a line on one, a person who buys on account,
// a trail entry and an item a till wrote down looked like on the day schema 17
// was minted. Identical to the live ones today, and that is the point: the live
// ones keep growing, and a legacy state that named them would quietly change
// what schema 16's bytes claim to be the next time one of them gains a field.
// `core/tests/frozen_shapes.rs` is what makes this a rule rather than a habit.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineV6Legacy {
    pub item_id: u128,
    pub code: String,
    pub name: String,
    pub unit_price_minor: i64,
    pub qty_milli: i64,
    pub discount: DiscountV1,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub unit: String,
    #[serde(default)]
    pub supply: u8,
    #[serde(default)]
    pub cost_minor: i64,
}

impl From<LineV6Legacy> for LineV1 {
    fn from(old: LineV6Legacy) -> Self {
        Self {
            item_id: old.item_id,
            code: old.code,
            name: old.name,
            unit_price_minor: old.unit_price_minor,
            qty_milli: old.qty_milli,
            discount: old.discount,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            unit: old.unit,
            supply: old.supply,
            cost_minor: old.cost_minor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTicketV6Legacy {
    pub id: u128,
    pub held_at_ms: u64,
    pub customer: Option<u128>,
    pub label: String,
    pub lines: Vec<LineV6Legacy>,
    pub ticket_discount: DiscountV1,
    #[serde(default)]
    pub overrides: Vec<String>,
    #[serde(default)]
    pub refund: bool,
    #[serde(default)]
    pub refund_of: Option<String>,
}

impl From<HeldTicketV6Legacy> for HeldTicketV1 {
    fn from(old: HeldTicketV6Legacy) -> Self {
        Self {
            id: old.id,
            held_at_ms: old.held_at_ms,
            customer: old.customer,
            label: old.label,
            lines: old.lines.into_iter().map(Into::into).collect(),
            ticket_discount: old.ticket_discount,
            overrides: old.overrides,
            refund: old.refund,
            refund_of: old.refund_of,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV6Legacy {
    pub tickets: Vec<HeldTicketV6Legacy>,
}

impl From<HeldTicketsV6Legacy> for HeldTicketsV1 {
    fn from(old: HeldTicketsV6Legacy) -> Self {
        Self {
            tickets: old.tickets.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerV6Legacy {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub limit_minor: i64,
}

impl From<CustomerV6Legacy> for CustomerV1 {
    fn from(old: CustomerV6Legacy) -> Self {
        Self {
            id: old.id,
            name: old.name,
            phone: old.phone,
            active: old.active,
            bin: old.bin,
            limit_minor: old.limit_minor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedV6Legacy {
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
    pub receipt_no: Option<String>,
}

impl From<AllowedV6Legacy> for AllowedV1 {
    fn from(old: AllowedV6Legacy) -> Self {
        Self {
            seq: old.seq,
            at_ms: old.at_ms,
            action: old.action,
            bp: old.bp,
            operator: old.operator,
            operator_name: old.operator_name,
            authorised_by: old.authorised_by,
            authorised_by_name: old.authorised_by_name,
            receipt_no: old.receipt_no,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemV6Legacy {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    #[serde(default)]
    pub supply: u8,
    #[serde(default)]
    pub category: String,
}

impl From<ItemV6Legacy> for ItemV1 {
    fn from(old: ItemV6Legacy) -> Self {
        Self {
            id: old.id,
            code: old.code,
            name_en: old.name_en,
            name_bn: old.name_bn,
            unit: old.unit,
            price_minor: old.price_minor,
            cost_minor: old.cost_minor,
            vat_bp: old.vat_bp,
            price_inclusive: old.price_inclusive,
            vat_on_undiscounted: old.vat_on_undiscounted,
            barcodes: old.barcodes,
            on_hand_milli: old.on_hand_milli,
            active: old.active,
            supply: old.supply,
            category: old.category,
        }
    }
}

/// The standing state as schema 20 wrote it: no record of a PIN got wrong.
///
/// Frozen because postcard is positional. A device coming from that build kept
/// its lockouts in memory and lost them whenever the tab closed, so the field
/// it lacks is one a boot has nothing to say about: nobody is locked out, and
/// the attempts start again exactly as they did there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV20Legacy {
    pub leases: Vec<LeaseGrantV1>,
    /// The frozen copies rather than the growing ones, for the reason the copy
    /// below says at length.
    pub held: HeldTicketsV6Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV4Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV6Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV6Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub open_drawer: Option<OpenDrawerV1>,
}

impl From<TerminalStateV20Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV20Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            open_drawer: old.open_drawer,
            // Nobody is locked out, which is what that build had every time a
            // tab was closed.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as schema 19 wrote it: a shop with no languages on it.
///
/// Frozen because postcard is positional. Everything else about it is what this
/// build writes, so the one thing a boot has to supply is the say a shop had no
/// way to make, and there is only one honest value for that: nothing said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV19Legacy {
    pub leases: Vec<LeaseGrantV1>,
    /// The frozen copies rather than the growing ones, for the reason the copy
    /// below says at length: the same bytes today is exactly why.
    pub held: HeldTicketsV6Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV6Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV6Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub open_drawer: Option<OpenDrawerV1>,
}

impl From<TerminalStateV19Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV19Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            open_drawer: old.open_drawer,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as schema 17 wrote it.
///
/// One field longer than 16 and than 18: whether the device had been round the
/// shelf. It was written for a day, and the day it was written it was found to
/// outlive what it was about. The figures it claimed live in the catalogue
/// snapshot, which is rewritten only when the delta log has grown, so a shelf
/// sweep reaches the disk by luck: a till reloaded came back saying it knew the
/// shelf and holding the catalogue's own figures, which are zero, and refused
/// every sale in a shop whose rule says refuse.
///
/// The standing state as schema 18 wrote it: no open drawer in it.
///
/// Frozen because postcard is positional. A device coming from that build has
/// its drawer in the log, so the field it lacks is the one a boot does not need
/// from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV18Legacy {
    pub leases: Vec<LeaseGrantV1>,
    /// The frozen copies rather than the growing ones. They are the same bytes
    /// today, and that is exactly why naming the growing ones here would be the
    /// mistake: the next field added to a parked basket or a person would
    /// silently change what these bytes claim to be.
    pub held: HeldTicketsV6Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV6Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV6Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV6Legacy>,
}

impl From<TerminalStateV18Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV18Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// Read and dropped, because what it says is not a thing this build believes
/// about a device on the strength of what an older one wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV17Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV6Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV6Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV6Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV6Legacy>,
    /// Whether that build thought this device had been round the shelf.
    #[serde(default)]
    pub shelf_swept: bool,
}

impl From<TerminalStateV17Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV17Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as schema 16 wrote it.
///
/// Frozen for one reason: the device gained a record of whether it had been
/// round the shelf. Everything else is the same shape, and the nested types are
/// the live ones for the same reason version 15 names live operators: they did
/// not change in this version, and a copy that is not a copy of anything is a
/// copy that goes stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV16Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV6Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV6Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV6Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV6Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV6Legacy>,
}

impl From<TerminalStateV16Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV16Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // The build that wrote this kept no such record, so the honest
            // answer is that this device may never have been round the shelf.
            // It goes round once and says so; until then the shelf rules say
            // nothing, which is the end of that question a shop can live with.
            //
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as schema 15 wrote it.
///
/// Frozen for one reason: a parked basket gained what a supervisor allowed on
/// it and which way round it is. Everything else here is the same, and it is
/// copied rather than shared because a legacy struct built out of a shape that
/// keeps growing is not frozen at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV15Legacy {
    /// Blocks still in hand, active first, each at the position it had reached.
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV5Legacy,
    /// Sales that closed with no number available and are still waiting for one.
    pub unnumbered: u64,
    /// Who may stand at this till. Held on the device because the whole point
    /// is that a cashier can sign in with the internet down.
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    /// The credential this terminal syncs with.
    ///
    /// Kept beside the ledger rather than wherever a platform finds convenient,
    /// because it belongs to the same thing: wiping the till wipes the
    /// credential, and a device restored from another terminal's files is
    /// already refused by the owner check rather than arriving with a working
    /// token for a shop it is not part of.
    #[serde(default)]
    pub token: Option<String>,
    /// The shop's own details, for the top of a receipt. Held here because a
    /// receipt is printed with the internet down, so they have to be on the
    /// device before they are wanted.
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    /// Drawers counted and closed and not yet sent to the shop.
    ///
    /// Here rather than in the log because the log is truncated when every sale
    /// in it has been acknowledged, and a counted drawer that went with it is
    /// an accountability record nobody can reconstruct: the cashier counted, the
    /// till agreed, and then neither of them can prove it.
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    /// The people who buy on account. Held on the device for the same reason
    /// the operators are: a cashier writes a sale to somebody's account with
    /// the internet down, and a name typed from memory is how one Karim ends up
    /// paying for another Karim's rice.
    #[serde(default)]
    pub customers: Vec<CustomerV5Legacy>,
    /// When this device's credential was issued, and how long the shop said one
    /// lasts. Held so a till can renew before it expires rather than stopping
    /// dead a year after it was enrolled, and held across restarts so a tablet
    /// switched off every night does not renew every morning.
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    /// Privileged actions this device allowed and the shop has not been told
    /// about.
    ///
    /// Here rather than in the log for the reason the counted drawers are: the
    /// log is emptied when every sale in it has been acknowledged, and an
    /// override that went with it is an accountability record nobody can
    /// reconstruct. The question asked afterwards is never "was this allowed"
    /// but "who allowed it", and a device that answered that only until its
    /// next drain was answering nobody.
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV5Legacy>,
    /// How many privileged actions this device has allowed, ever.
    ///
    /// Its own counter rather than a clock: two of them in one millisecond are
    /// possible on a fast device, and the shop has to be able to tell one from
    /// the other when it stores them. Never reset, because a number that starts
    /// again is a number that collides with what the shop already holds.
    #[serde(default)]
    pub allowed_seq: u64,
    /// Items a till wrote down itself, and the shop has not got.
    ///
    /// A delivery arrives during an outage and its barcode is in nobody's
    /// catalogue. A till that could only say "no such item" would lose the sale
    /// and the shop would sell it off the paper, so the till writes the item
    /// down and sells it. Here rather than in the log for the reason the counted
    /// drawers are: the log is emptied when its sales are acknowledged, and an
    /// item that went with it is a sale in the shop's books naming something
    /// nobody can look up.
    #[serde(default)]
    pub unsent_items: Vec<ItemV5Legacy>,
    /// People a till wrote down itself, and the shop has not got.
    ///
    /// Somebody buys on account who is in nobody's list yet. Writing them down
    /// at the till is what keeps two people with one name apart: a sale against
    /// a typed name is added up against the spelling, and the second Karim ends
    /// up paying for the first one's rice.
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV5Legacy>,
}

impl From<TerminalStateV15Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV15Legacy) -> Self {
        Self {
            leases: old.leases,
            // The only thing that changed in this version. Everything else is
            // carried across as it stands.
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            // Nothing about a person, an item or the trail changed in this
            // version, only what a parked basket carries. The copies are still
            // copies: a legacy state naming a live type is the trap this file
            // exists to avoid.
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as schema 14 wrote it.
///
/// Frozen for one reason: the trail's entries gained the receipt a reprint was
/// of. Everything else here is the same, and it is copied rather than shared
/// because a legacy struct built out of a shape that keeps growing is not
/// frozen at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV14Legacy {
    /// Blocks still in hand, active first, each at the position it had reached.
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV4Legacy,
    /// Sales that closed with no number available and are still waiting for one.
    pub unnumbered: u64,
    /// Who may stand at this till. Held on the device because the whole point
    /// is that a cashier can sign in with the internet down.
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    /// The credential this terminal syncs with.
    ///
    /// Kept beside the ledger rather than wherever a platform finds convenient,
    /// because it belongs to the same thing: wiping the till wipes the
    /// credential, and a device restored from another terminal's files is
    /// already refused by the owner check rather than arriving with a working
    /// token for a shop it is not part of.
    #[serde(default)]
    pub token: Option<String>,
    /// The shop's own details, for the top of a receipt. Held here because a
    /// receipt is printed with the internet down, so they have to be on the
    /// device before they are wanted.
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    /// Drawers counted and closed and not yet sent to the shop.
    ///
    /// Here rather than in the log because the log is truncated when every sale
    /// in it has been acknowledged, and a counted drawer that went with it is
    /// an accountability record nobody can reconstruct: the cashier counted, the
    /// till agreed, and then neither of them can prove it.
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    /// The people who buy on account. Held on the device for the same reason
    /// the operators are: a cashier writes a sale to somebody's account with
    /// the internet down, and a name typed from memory is how one Karim ends up
    /// paying for another Karim's rice.
    #[serde(default)]
    pub customers: Vec<CustomerV4Legacy>,
    /// When this device's credential was issued, and how long the shop said one
    /// lasts. Held so a till can renew before it expires rather than stopping
    /// dead a year after it was enrolled, and held across restarts so a tablet
    /// switched off every night does not renew every morning.
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    /// Privileged actions this device allowed and the shop has not been told
    /// about.
    ///
    /// Here rather than in the log for the reason the counted drawers are: the
    /// log is emptied when every sale in it has been acknowledged, and an
    /// override that went with it is an accountability record nobody can
    /// reconstruct. The question asked afterwards is never "was this allowed"
    /// but "who allowed it", and a device that answered that only until its
    /// next drain was answering nobody.
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    /// How many privileged actions this device has allowed, ever.
    ///
    /// Its own counter rather than a clock: two of them in one millisecond are
    /// possible on a fast device, and the shop has to be able to tell one from
    /// the other when it stores them. Never reset, because a number that starts
    /// again is a number that collides with what the shop already holds.
    #[serde(default)]
    pub allowed_seq: u64,
    /// Items a till wrote down itself, and the shop has not got.
    ///
    /// A delivery arrives during an outage and its barcode is in nobody's
    /// catalogue. A till that could only say "no such item" would lose the sale
    /// and the shop would sell it off the paper, so the till writes the item
    /// down and sells it. Here rather than in the log for the reason the counted
    /// drawers are: the log is emptied when its sales are acknowledged, and an
    /// item that went with it is a sale in the shop's books naming something
    /// nobody can look up.
    #[serde(default)]
    pub unsent_items: Vec<ItemV4Legacy>,
    /// People a till wrote down itself, and the shop has not got.
    ///
    /// Somebody buys on account who is in nobody's list yet. Writing them down
    /// at the till is what keeps two people with one name apart: a sale against
    /// a typed name is added up against the spelling, and the second Karim ends
    /// up paying for the first one's rice.
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV4Legacy>,
}
impl From<TerminalStateV14Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV14Legacy) -> Self {
        Self {
            leases: old.leases,
            // Nothing about a basket, an item or a person changed in this
            // version, only what the trail records about a reprint. The copies
            // are still copies: a legacy state naming a live type is the trap
            // this file exists to avoid.
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 5 wrote it, before a device wrote down when
/// its credential was issued.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV5Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV2Legacy>,
}

/// The standing state as version 13 wrote it: everything but a cap on what
/// anybody may owe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV13Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV4Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV3Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV4Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV3Legacy>,
}

impl From<TerminalStateV13Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV13Legacy) -> Self {
        Self {
            leases: old.leases,
            // Nothing changed about a line or a basket in this version, and
            // the shapes are still frozen copies: a legacy state naming a live
            // type is the trap this file exists to avoid.
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 12 wrote it: everything but what the shop paid,
/// so a basket parked before the upgrade carries no cost on its lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV12Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV3Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV3Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV4Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV3Legacy>,
}

impl From<TerminalStateV12Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV12Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            // Nothing changed about an item in this version, only about a
            // line, and the copy is frozen for the reason every other one is.
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 11 wrote it: everything but the shop's own
/// categories, so the items a till wrote down carry no sorting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV11Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV3Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV3Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV3Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV3Legacy>,
}

impl From<TerminalStateV11Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV11Legacy) -> Self {
        Self {
            leases: old.leases,
            // A line has never carried the shop's sorting, only what was sold
            // and at what, so this version's baskets are the pre-cost shape.
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 10 wrote it: everything but the kind of supply
/// an item is, so its parked baskets and its written-down items carry no answer
/// to that question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV10Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV3Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV2Legacy>,
    #[serde(default)]
    pub unsent_customers: Vec<CustomerV3Legacy>,
}

impl From<TerminalStateV10Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV10Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            unsent_customers: old.unsent_customers.into_iter().map(Into::into).collect(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 9 wrote it: everything but a buyer's BIN, and
/// everything but the people a till wrote down itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV9Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV2Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
    #[serde(default)]
    pub unsent_items: Vec<ItemV2Legacy>,
}

impl From<TerminalStateV9Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV9Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: old.unsent_items.into_iter().map(Into::into).collect(),
            // A device upgrading has written nobody down, because the build it
            // was running could not.
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 8 wrote it: everything but the items a till
/// wrote down itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV8Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV3Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV2Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
}

impl From<TerminalStateV8Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV8Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            // A device upgrading has written no items down, because the build
            // it was running could not.
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 7 wrote it: everything but what a shop wants
/// done about the shelf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV7Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV2Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
    #[serde(default)]
    pub unsent_allowed: Vec<AllowedV1Legacy>,
    #[serde(default)]
    pub allowed_seq: u64,
}

impl From<TerminalStateV7Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV7Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            unsent_allowed: old.unsent_allowed.into_iter().map(Into::into).collect(),
            allowed_seq: old.allowed_seq,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 6 wrote it: everything but what the device
/// allowed and has not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV6Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
    #[serde(default)]
    pub customers: Vec<CustomerV2Legacy>,
    #[serde(default)]
    pub credential: Option<CredentialV1>,
}

impl From<TerminalStateV6Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV6Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            credential: old.credential,
            // A device upgraded in the middle of a day. What it allowed before
            // now is gone: it was only ever in memory, and inventing entries
            // for it would be worse than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

impl From<TerminalStateV5Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV5Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            customers: old.customers.into_iter().map(Into::into).collect(),
            // A device that never wrote down when its credential was taken. It
            // renews at the next opportunity rather than guessing, which costs
            // one request and buys a year.
            credential: None,
            // Nothing was kept about what this device allowed: it was only
            // ever in memory before, and inventing entries would be worse
            // than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 4 wrote it, before the shop's account
/// customers were held on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV4Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV1>,
}

impl From<TerminalStateV4Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV4Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts,
            credential: None,
            // A device that has not been told who buys on account yet. It will
            // be at the next sync, and until then a cashier types the name as
            // they always did.
            customers: Vec::new(),
            // Nothing was kept about what this device allowed: it was only
            // ever in memory before, and inventing entries would be worse
            // than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 3 wrote it, read and converted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV3Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
    #[serde(default)]
    pub unsent_shifts: Vec<ClosedShiftV3Legacy>,
}

impl From<TerminalStateV3Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV3Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            unsent_shifts: old.unsent_shifts.into_iter().map(Into::into).collect(),
            customers: Vec::new(),
            credential: None,
            // Nothing was kept about what this device allowed: it was only
            // ever in memory before, and inventing entries would be worse
            // than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// The standing state as version 2 wrote it, read and converted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV2Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV2Legacy>,
}

impl From<TerminalStateV2Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV2Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop.map(Into::into),
            // A device from before drawers were sent. Whatever it closed is on
            // its own paper and nowhere else, and this build cannot invent it.
            unsent_shifts: Vec::new(),
            customers: Vec::new(),
            credential: None,
            // Nothing was kept about what this device allowed: it was only
            // ever in memory before, and inventing entries would be worse
            // than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

/// A PIN somebody got wrong, and whether it locked them out.
///
/// Written down because a lockout held only in memory is a lockout anybody
/// holding the device can clear by closing the tab: five wrong guesses, reload,
/// five more. What is left then is the rounds, which is a few hundred
/// milliseconds a guess on a cheap tablet, and a four digit PIN is ten thousand
/// of those.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrongPinsV1 {
    pub operator: u128,
    pub count: u32,
    /// When the lockout runs out, by the device's own clock. Zero when nobody
    /// is locked out and only the count stands.
    pub locked_until_ms: u64,
}

/// A shop as it appears on its own receipts, plus what it takes money by.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopV1 {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// The wallets this shop takes. Held on the device with everything else a
    /// till needs before it can sell: a cashier taking bKash with the line down
    /// should be offered the name rather than made to spell it.
    pub wallets: Vec<String>,
    /// What to do when a basket asks for more of something than the shop
    /// believes it has: nothing, say so, or refuse it. A number rather than the
    /// enum, so a device reading a rule a later build added is not stopped by
    /// a variant it has never heard of.
    ///
    /// Appended, never inserted: these encode positionally, and a field placed
    /// in the middle would make an older till read a wallet list as a rule.
    #[serde(default)]
    pub stock_rule: u8,
    /// The languages this shop offers its own staff, by the codes the screens
    /// use: `en`, `bn`. Empty means every language this build has, which is
    /// what every shop that has never said otherwise means.
    ///
    /// Held on the device because a screen has to draw itself with the internet
    /// down, and the language it draws itself in is not a thing to go and ask
    /// about. Appended, never inserted, like the wallets and the rule above it.
    #[serde(default)]
    pub languages: Vec<String>,
}

/// A shop as schema 20 wrote it: the same fields this build writes, frozen.
///
/// Byte for byte what `ShopV1` holds today, and that is exactly why it exists
/// separately. The standing state version 20 wrote names this rather than the
/// growing one, so the next field added to a shop cannot quietly change what
/// those bytes claim to be.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopV4Legacy {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub wallets: Vec<String>,
    #[serde(default)]
    pub stock_rule: u8,
    #[serde(default)]
    pub languages: Vec<String>,
}

impl From<ShopV4Legacy> for ShopV1 {
    fn from(old: ShopV4Legacy) -> Self {
        Self {
            name: old.name,
            bin: old.bin,
            address: old.address,
            phone: old.phone,
            wallets: old.wallets,
            stock_rule: old.stock_rule,
            languages: old.languages,
        }
    }
}

/// A shop as it was written before a shop could say which languages it offers.
///
/// Referenced by every standing state from version 8 to version 19, which all
/// wrote the shop with a stock rule and no languages. Frozen for the reason the
/// one below it is frozen, and it is the same mistake either way: a legacy copy
/// naming a growing shape stops reading the bytes it was kept for, and the
/// symptom is a till that cannot read its own standing state after an upgrade,
/// with the day's unsent sales inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopV3Legacy {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub wallets: Vec<String>,
    #[serde(default)]
    pub stock_rule: u8,
}

impl From<ShopV3Legacy> for ShopV1 {
    fn from(old: ShopV3Legacy) -> Self {
        Self {
            name: old.name,
            bin: old.bin,
            address: old.address,
            phone: old.phone,
            wallets: old.wallets,
            stock_rule: old.stock_rule,
            // A shop that never said means every language there is, which is
            // what it had before this existed.
            languages: Vec::new(),
        }
    }
}

/// A shop as it was written before a shop could say what to do about the shelf.
///
/// Referenced by every standing state before version 8, which is why it exists
/// separately rather than those pointing at the current shape: a legacy struct
/// that quietly grows a field with the current one is a legacy struct that
/// stops reading the bytes it was kept for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopV2Legacy {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub wallets: Vec<String>,
}

impl From<ShopV2Legacy> for ShopV1 {
    fn from(old: ShopV2Legacy) -> Self {
        Self {
            name: old.name,
            bin: old.bin,
            address: old.address,
            phone: old.phone,
            wallets: old.wallets,
            // A shop that was never asked gets the rule that keeps a till
            // selling. Turning it on is a statement that the figures mean
            // something, and nobody has made it.
            stock_rule: 0,
            // Nor was it ever asked which languages it offers, which means all
            // of them, which is what it had.
            languages: Vec::new(),
        }
    }
}

/// A shop as version 1 wrote one, without the wallets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopV1Legacy {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
}

/// The standing state as version 1 wrote it, read and then converted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV1Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV2Legacy,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV1Legacy>,
}

impl From<TerminalStateV1Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV1Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held.into(),
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            // A shop that has never been told which wallets it takes takes
            // none, and the till falls back to letting a cashier name one.
            shop: old.shop.map(|shop| ShopV1 {
                name: shop.name,
                bin: shop.bin,
                address: shop.address,
                phone: shop.phone,
                wallets: Vec::new(),
                stock_rule: 0,
                languages: Vec::new(),
            }),
            unsent_shifts: Vec::new(),
            customers: Vec::new(),
            credential: None,
            // Nothing was kept about what this device allowed: it was only
            // ever in memory before, and inventing entries would be worse
            // than the hole.
            unsent_allowed: Vec::new(),
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            // Its drawer is in the log it came with.
            open_drawer: None,
            // Nobody is locked out, which is what that build had.
            wrong_pins: Vec::new(),
        }
    }
}

impl OperatorV1 {
    #[must_use]
    pub fn from_domain(operator: &Operator) -> Self {
        Self {
            id: operator.id.to_u128(),
            name: operator.name.to_string(),
            salt: operator.pin.salt.to_vec(),
            rounds: operator.pin.rounds,
            key: operator.pin.key().to_vec(),
            max_discount_bp: operator.permissions.max_discount_bp,
            may_override_price: operator.permissions.may_override_price,
            may_refund: operator.permissions.may_refund,
            may_void_line: operator.permissions.may_void_line,
            may_authorise: operator.permissions.may_authorise,
            may_open_drawer: operator.permissions.may_open_drawer,
            may_close_shift: operator.permissions.may_close_shift,
            active: operator.active,
        }
    }

    pub fn into_domain(self) -> Result<Operator> {
        // A salt or key of the wrong length means the record was truncated or
        // written by something that is not this format. Padding it out would
        // produce a credential that verifies against nothing and looks like a
        // forgotten PIN rather than a corrupt file.
        let salt: [u8; SALT_LEN] = self.salt.try_into().map_err(|_| WireError::OutOfRange)?;
        let key: [u8; auth::KEY_BYTES] = self.key.try_into().map_err(|_| WireError::OutOfRange)?;

        Ok(Operator {
            id: Ulid::from_u128(self.id),
            name: self.name.into_boxed_str(),
            pin: PinHash::from_parts(salt, self.rounds, key),
            permissions: Permissions {
                max_discount_bp: self.max_discount_bp,
                may_override_price: self.may_override_price,
                may_refund: self.may_refund,
                may_void_line: self.may_void_line,
                may_authorise: self.may_authorise,
                may_open_drawer: self.may_open_drawer,
                may_close_shift: self.may_close_shift,
            },
            active: self.active,
        })
    }
}

/// Encode the terminal's standing state.
pub fn encode_terminal_state(state: &TerminalStateV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(state).map_err(|_| WireError::Malformed)
}

/// Decode the terminal's standing state written under `schema`.
pub fn decode_terminal_state(schema: u16, bytes: &[u8]) -> Result<TerminalStateV1> {
    match schema {
        TERMINAL_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V20 => postcard::from_bytes::<TerminalStateV20Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V19 => postcard::from_bytes::<TerminalStateV19Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V18 => postcard::from_bytes::<TerminalStateV18Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V17 => postcard::from_bytes::<TerminalStateV17Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V16 => postcard::from_bytes::<TerminalStateV16Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V15 => postcard::from_bytes::<TerminalStateV15Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V14 => postcard::from_bytes::<TerminalStateV14Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V13 => postcard::from_bytes::<TerminalStateV13Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V12 => postcard::from_bytes::<TerminalStateV12Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V11 => postcard::from_bytes::<TerminalStateV11Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V10 => postcard::from_bytes::<TerminalStateV10Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V9 => postcard::from_bytes::<TerminalStateV9Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V8 => postcard::from_bytes::<TerminalStateV8Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V7 => postcard::from_bytes::<TerminalStateV7Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V6 => postcard::from_bytes::<TerminalStateV6Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V5 => postcard::from_bytes::<TerminalStateV5Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V4 => postcard::from_bytes::<TerminalStateV4Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V3 => postcard::from_bytes::<TerminalStateV3Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V2 => postcard::from_bytes::<TerminalStateV2Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V1 => postcard::from_bytes::<TerminalStateV1Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

pub const SHIFT_SCHEMA: u16 = 2;
/// Version 1 wrote a count with no name on it.
pub const SHIFT_SCHEMA_V1: u16 = 1;

/// Something that happened to a drawer.
///
/// Variants are encoded positionally, so new ones are appended and never
/// reordered: an older build reading a reordered log would read a cash drop as
/// a shift opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShiftEventV1 {
    Opened {
        id: u128,
        terminal: u128,
        opening_float_minor: i64,
        at_ms: u64,
    },
    CashMoved {
        /// True for money in, false for money out. The direction is stored
        /// rather than a signed amount so a reader that ignores it cannot
        /// silently turn a drop into a top-up.
        inward: bool,
        amount_minor: i64,
        reason: String,
        at_ms: u64,
    },
    Closed {
        counted_cash_minor: i64,
        at_ms: u64,
        /// Who counted it, in the frame rather than only in the record built
        /// from it. The record is written a moment after this frame is durable,
        /// and a device that dies in between is rebuilt from what is here: a
        /// count with nobody's name on it is half an accountability record.
        counted_by: u128,
        counted_by_name: String,
    },
}

/// Drawer events as version 1 wrote them: a count with nobody's name on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShiftEventV1Legacy {
    Opened {
        id: u128,
        terminal: u128,
        opening_float_minor: i64,
        at_ms: u64,
    },
    CashMoved {
        inward: bool,
        amount_minor: i64,
        reason: String,
        at_ms: u64,
    },
    Closed {
        counted_cash_minor: i64,
        at_ms: u64,
    },
}

impl From<ShiftEventV1Legacy> for ShiftEventV1 {
    fn from(old: ShiftEventV1Legacy) -> Self {
        match old {
            ShiftEventV1Legacy::Opened {
                id,
                terminal,
                opening_float_minor,
                at_ms,
            } => Self::Opened {
                id,
                terminal,
                opening_float_minor,
                at_ms,
            },
            ShiftEventV1Legacy::CashMoved {
                inward,
                amount_minor,
                reason,
                at_ms,
            } => Self::CashMoved {
                inward,
                amount_minor,
                reason,
                at_ms,
            },
            // Nobody, because the build that wrote it did not ask. Empty rather
            // than a name invented here: a count attributed to the wrong person
            // is worse than one attributed to nobody.
            ShiftEventV1Legacy::Closed {
                counted_cash_minor,
                at_ms,
            } => Self::Closed {
                counted_cash_minor,
                at_ms,
                counted_by: 0,
                counted_by_name: String::new(),
            },
        }
    }
}

/// Encode a drawer event.
pub fn encode_shift_event(event: &ShiftEventV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(event).map_err(|_| WireError::Malformed)
}

/// Decode a drawer event written under `schema`.
pub fn decode_shift_event(schema: u16, bytes: &[u8]) -> Result<ShiftEventV1> {
    match schema {
        SHIFT_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        SHIFT_SCHEMA_V1 => postcard::from_bytes::<ShiftEventV1Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

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
        // Written before a shop could sort its shelves.
        SNAPSHOT_SCHEMA_V3 => {
            let snapshot: SnapshotV3Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            let cursor = snapshot.cursor;
            let items = snapshot
                .items
                .into_iter()
                .map(|item| ItemV1::from(item).into_domain())
                .collect::<Result<Vec<_>>>()?;
            Ok((items, cursor))
        }
        // Written before a shop could say a thing was exempt.
        SNAPSHOT_SCHEMA_V2 => {
            let snapshot: SnapshotV2Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            let cursor = snapshot.cursor;
            let items = snapshot
                .items
                .into_iter()
                .map(|item| ItemV1::from(item).into_domain())
                .collect::<Result<Vec<_>>>()?;
            Ok((items, cursor))
        }
        1 => {
            let snapshot: SnapshotV1Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            let cursor = snapshot.cursor;
            let items = snapshot
                .items
                .into_iter()
                .map(|item| item.into_current().into_domain())
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
        // Written before a shop could sort its shelves.
        DELTAS_SCHEMA_V3 => {
            let legacy: ItemDeltasV3Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            Ok(ItemDeltasV1 {
                cursor: legacy.cursor,
                upserts: legacy.upserts.into_iter().map(Into::into).collect(),
                tombstones: legacy.tombstones,
            })
        }
        // Written before a shop could say a thing was exempt.
        DELTAS_SCHEMA_V2 => {
            let legacy: ItemDeltasV2Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            Ok(ItemDeltasV1 {
                cursor: legacy.cursor,
                upserts: legacy.upserts.into_iter().map(Into::into).collect(),
                tombstones: legacy.tombstones,
            })
        }
        1 => {
            let legacy: ItemDeltasV1Legacy =
                postcard::from_bytes(bytes).map_err(|_| WireError::Malformed)?;
            Ok(ItemDeltasV1 {
                cursor: legacy.cursor,
                upserts: legacy
                    .upserts
                    .into_iter()
                    .map(ItemV1Legacy::into_current)
                    .collect(),
                tombstones: legacy.tombstones,
            })
        }
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

/// Encode one committed sale.
pub fn encode_sale(sale: &SaleCommitV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(sale).map_err(|_| WireError::Malformed)
}

impl From<TicketV4Legacy> for TicketV1 {
    fn from(old: TicketV4Legacy) -> Self {
        Self {
            id: old.id,
            terminal: old.terminal,
            rung_at_ms: old.rung_at_ms,
            receipt_no: old.receipt_no,
            receipt_epoch: old.receipt_epoch,
            customer: old.customer,
            lines: old.lines.into_iter().map(Into::into).collect(),
            ticket_discount: old.ticket_discount,
            tenders: old.tenders,
            net_minor: old.net_minor,
            vat_minor: old.vat_minor,
            discount_minor: old.discount_minor,
            total_minor: old.total_minor,
            change_minor: old.change_minor,
            overrides: old.overrides,
            // Nobody. The sale was rung by a build that did not ask, so the
            // paper and the shop's records say so rather than guessing at the
            // person who happens to be at the till when it is read back.
            operator: None,
        }
    }
}

impl From<SaleCommitV4Legacy> for SaleCommitV1 {
    fn from(old: SaleCommitV4Legacy) -> Self {
        Self {
            ticket: old.ticket.into(),
            lease_next: old.lease_next,
            lease_epoch: old.lease_epoch,
            stock: old.stock,
            refund_of: old.refund_of,
        }
    }
}

/// Decode one committed sale written under `schema`.
pub fn decode_sale(schema: u16, bytes: &[u8]) -> Result<SaleCommitV1> {
    match schema {
        SALE_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
        // A sale committed before it said who rang it.
        SALE_SCHEMA_V4 => postcard::from_bytes::<SaleCommitV4Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        // A sale committed before the cost travelled with it.
        SALE_SCHEMA_V3 => postcard::from_bytes::<SaleCommitV3Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        // A sale committed before a line could be exempt.
        SALE_SCHEMA_V2 => postcard::from_bytes::<SaleCommitV2Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        // A sale committed before the unit existed. Still in an outbox waiting
        // to be sent, or being reprinted from the log months later.
        SALE_SCHEMA_V1 => postcard::from_bytes::<SaleCommitV1Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
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
            vat_on_undiscounted: matches!(item.vat_base, VatBase::Undiscounted),
            barcodes: item.barcodes.iter().map(ToString::to_string).collect(),
            on_hand_milli: item.on_hand.get(),
            active: item.active,
            supply: item.supply.as_u8(),
            category: item.category.to_string(),
        }
    }

    pub fn into_domain(self) -> Result<Item> {
        // Rejected at the boundary, alongside the VAT range, because one bad
        // catalogue row reaches every till in the shop. An item priced below
        // zero pays the customer to take it, and nothing further down the money
        // path would call that an error.
        if self.price_minor < 0 || self.cost_minor < 0 {
            return Err(WireError::OutOfRange);
        }
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
            vat_base: if self.vat_on_undiscounted {
                VatBase::Undiscounted
            } else {
                VatBase::Discounted
            },
            barcodes: self
                .barcodes
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
            on_hand: Milli::new(self.on_hand_milli),
            active: self.active,
            supply: Supply::from_u8(self.supply),
            category: self.category.into_boxed_str(),
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
            Self::RateBp(bp) => Discount::Rate(Bp::new(bp).map_err(|_| WireError::OutOfRange)?),
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
            vat_on_undiscounted: matches!(line.vat_base, VatBase::Undiscounted),
            unit: line.unit.to_string(),
            supply: line.supply.as_u8(),
            cost_minor: line.cost.get(),
        }
    }

    pub fn into_domain(self) -> Result<CartLine> {
        Ok(CartLine {
            unit: self.unit.into_boxed_str(),
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
            vat_base: if self.vat_on_undiscounted {
                VatBase::Undiscounted
            } else {
                VatBase::Discounted
            },
            supply: Supply::from_u8(self.supply),
            cost: Minor::new(self.cost_minor),
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
            operator: ticket.operator.map(Ulid::to_u128),
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
        let tenders = self
            .tenders
            .into_iter()
            .map(TenderV1::into_domain)
            .collect();
        Ok((lines, tenders))
    }
}

/// Build the payload for one committed sale.
#[must_use]
pub fn sale_commit(
    ticket: &Ticket,
    receipt_epoch: Option<u64>,
    lease_next: Option<u64>,
) -> SaleCommitV1 {
    // Stock moves opposite to the line: a sale of one takes one off the shelf, a
    // refund of one puts it back, and both fall out of negating the quantity.
    //
    // Summed per item rather than emitted per line. One item legitimately
    // appears on two lines when the second carries a discount or a price
    // override, and the server keys a movement on the sale and the item, so a
    // second entry for the same pair was silently discarded: the ledger recorded
    // one unit of rice leaving when three did, permanently, and the shrinkage
    // report accused staff of the difference.
    let mut stock: Vec<(u128, i64)> = Vec::with_capacity(ticket.lines.len());
    for line in &ticket.lines {
        let id = line.item_id.to_u128();
        let movement = line.qty.get().saturating_neg();
        match stock.iter_mut().find(|(existing, _)| *existing == id) {
            Some((_, total)) => *total = total.saturating_add(movement),
            None => stock.push((id, movement)),
        }
    }

    let refund_of = match &ticket.direction {
        Direction::Sale => None,
        Direction::Refund { original_receipt } => {
            original_receipt.as_ref().map(ToString::to_string)
        }
    };

    SaleCommitV1 {
        ticket: TicketV1::from_domain(ticket, receipt_epoch),
        lease_next,
        lease_epoch: receipt_epoch,
        stock,
        refund_of,
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

    #[test]
    fn a_sale_committed_before_units_existed_still_decodes() {
        // One sitting unsent in an outbox when the device upgrades, or being
        // reprinted from the log months later. postcard is positional, so
        // without the version 1 shape this sale becomes unreadable: the till
        // cannot send it and cannot reprint it, and it is the only copy.
        let old = SaleCommitV1Legacy {
            ticket: TicketV1Legacy {
                id: 900,
                terminal: 7,
                rung_at_ms: 1_788_600_000_000,
                receipt_no: Some(alloc::string::String::from("T1-000100")),
                receipt_epoch: Some(1),
                customer: None,
                lines: alloc::vec![LineV1Legacy {
                    item_id: 1,
                    code: alloc::string::String::from("RICE5"),
                    name: alloc::string::String::from("Rice Miniket 5kg"),
                    unit_price_minor: 43_000,
                    qty_milli: 1_000,
                    discount: DiscountV1::None,
                    vat_bp: 1_500,
                    price_inclusive: false,
                    vat_on_undiscounted: false,
                }],
                ticket_discount: DiscountV1::None,
                tenders: alloc::vec![],
                net_minor: 43_000,
                vat_minor: 6_450,
                discount_minor: 0,
                total_minor: 49_450,
                change_minor: 0,
                overrides: alloc::vec![],
            },
            lease_next: Some(101),
            lease_epoch: Some(1),
            stock: alloc::vec![(1, -1_000)],
            refund_of: None,
        };
        let bytes = postcard::to_allocvec(&old).expect("version one encodes");

        let read = decode_sale(SALE_SCHEMA_V1, &bytes).expect("and still decodes");

        assert_eq!(read.ticket.receipt_no.as_deref(), Some("T1-000100"));
        assert_eq!(read.ticket.total_minor, 49_450);
        assert_eq!(read.ticket.lines.len(), 1);
        // A sale rung before a shop could say what it sold a thing by. Every one
        // of them meant pieces.
        assert_eq!(read.ticket.lines[0].unit, "Nos");
        assert_eq!(read.stock, alloc::vec![(1, -1_000)]);
    }

    #[test]
    fn a_drawer_counted_by_version_one_still_reads_with_nobody_named() {
        // A tablet upgrading with yesterday's count still in its log. postcard
        // is positional, so the two fields appended to the count are not absent
        // in these bytes, they are whatever follows: read as the current shape
        // this is a decode failure, and a decode failure here is a device that
        // will not open its own ledger.
        let old = ShiftEventV1Legacy::Closed {
            counted_cash_minor: 79_000,
            at_ms: 1_788_600_000_000,
        };
        let bytes = postcard::to_allocvec(&old).expect("version one encodes");

        let read = decode_shift_event(SHIFT_SCHEMA_V1, &bytes).expect("and still decodes");

        match read {
            ShiftEventV1::Closed {
                counted_cash_minor,
                at_ms,
                counted_by,
                counted_by_name,
            } => {
                assert_eq!(counted_cash_minor, 79_000, "what was in the drawer");
                assert_eq!(at_ms, 1_788_600_000_000);
                // Nobody, because the build that wrote it did not ask.
                assert_eq!(counted_by, 0);
                assert!(counted_by_name.is_empty());
            }
            other => panic!("a count read back as {other:?}"),
        }
    }

    #[test]
    fn a_drawer_opened_by_version_one_still_reads() {
        let old = ShiftEventV1Legacy::Opened {
            id: 80,
            terminal: 7,
            opening_float_minor: 200_000,
            at_ms: 1_788_600_000_000,
        };
        let bytes = postcard::to_allocvec(&old).expect("version one encodes");

        assert_eq!(
            decode_shift_event(SHIFT_SCHEMA_V1, &bytes).expect("and still decodes"),
            ShiftEventV1::Opened {
                id: 80,
                terminal: 7,
                opening_float_minor: 200_000,
                at_ms: 1_788_600_000_000,
            }
        );
    }

    #[test]
    fn standing_state_written_by_version_one_still_reads() {
        // A till upgrading to a build that knows about wallets. postcard is
        // positional and does not honour a serde default for a field that is
        // simply absent, so without the legacy shape every till in every shop
        // would fail to read its own leases, its parked sales and its
        // credential the first time it started on this build.
        let old = TerminalStateV1Legacy {
            leases: alloc::vec![LeaseGrantV1 {
                terminal: 7,
                epoch: 1,
                prefix: alloc::string::String::from("T1"),
                first: 100,
                last: 599,
            }],
            held: HeldTicketsV2Legacy::default(),
            unnumbered: 2,
            operators: alloc::vec![],
            token: Some(alloc::string::String::from("a-credential")),
            shop: Some(ShopV1Legacy {
                name: alloc::string::String::from("Karim General Store"),
                bin: Some(alloc::string::String::from("001234567-0101")),
                address: None,
                phone: None,
            }),
        };
        let bytes = postcard::to_allocvec(&old).expect("version one encodes");

        let read = decode_terminal_state(TERMINAL_SCHEMA_V1, &bytes).expect("and still decodes");

        assert_eq!(read.leases.len(), 1);
        assert_eq!(read.leases[0].last, 599, "the numbers it had left");
        assert_eq!(read.unnumbered, 2);
        assert_eq!(read.token.as_deref(), Some("a-credential"));
        let shop = read.shop.expect("the shop it prints at the top");
        assert_eq!(shop.name, "Karim General Store");
        // A shop that was never told which wallets it takes takes none, and the
        // till falls back to letting a cashier name one.
        assert!(shop.wallets.is_empty());
    }

    #[test]
    fn standing_state_written_now_carries_the_wallets() {
        let state = TerminalStateV1 {
            customers: vec![],
            credential: None,
            unsent_shifts: alloc::vec![],
            open_drawer: None,
            unsent_allowed: alloc::vec![],
            allowed_seq: 0,
            unsent_items: Vec::new(),
            unsent_customers: Vec::new(),
            leases: alloc::vec![],
            held: HeldTicketsV1::default(),
            unnumbered: 0,
            operators: alloc::vec![],
            token: None,
            shop: Some(ShopV1 {
                name: alloc::string::String::from("Karim General Store"),
                bin: None,
                address: None,
                phone: None,
                wallets: alloc::vec![alloc::string::String::from("bKash")],
                stock_rule: 2,
                languages: alloc::vec![alloc::string::String::from("bn")],
            }),
            wrong_pins: alloc::vec![WrongPinsV1 {
                operator: 91,
                count: 2,
                locked_until_ms: 0,
            }],
        };
        let bytes = encode_terminal_state(&state).expect("it encodes");

        let read = decode_terminal_state(TERMINAL_SCHEMA, &bytes).expect("and decodes");
        let shop = read.shop.expect("a shop");
        assert_eq!(shop.wallets, alloc::vec!["bKash"]);
        assert_eq!(shop.stock_rule, 2, "and what it does about the shelf");
    }

    /// A device holding what the build before this one wrote.
    ///
    /// The shop grew a field, so every standing state before version 8 has to
    /// be read through the shape it was written in. postcard is positional: read
    /// as the current shape, a version 7 shop is a decode failure, and a decode
    /// failure here is a till that will not open its own ledger.
    #[test]
    fn standing_state_written_by_version_seven_still_reads() {
        let old = TerminalStateV7Legacy {
            leases: alloc::vec![LeaseGrantV1 {
                terminal: 7,
                epoch: 1,
                prefix: alloc::string::String::from("T1"),
                first: 100,
                last: 599,
            }],
            held: HeldTicketsV2Legacy::default(),
            unnumbered: 0,
            operators: alloc::vec![],
            token: Some(alloc::string::String::from("a-credential")),
            shop: Some(ShopV2Legacy {
                name: alloc::string::String::from("Karim General Store"),
                bin: Some(alloc::string::String::from("001234567-0101")),
                address: None,
                phone: None,
                wallets: alloc::vec![alloc::string::String::from("bKash")],
            }),
            unsent_shifts: alloc::vec![],
            customers: alloc::vec![],
            credential: None,
            unsent_allowed: alloc::vec![],
            allowed_seq: 4,
        };
        let bytes = postcard::to_allocvec(&old).expect("version seven encodes");

        let read = decode_terminal_state(TERMINAL_SCHEMA_V7, &bytes).expect("and still decodes");

        assert_eq!(read.leases.len(), 1, "the numbers it had left");
        assert_eq!(read.token.as_deref(), Some("a-credential"));
        assert_eq!(read.allowed_seq, 4, "and what it had allowed");
        let shop = read.shop.expect("the shop it prints at the top");
        assert_eq!(shop.wallets, alloc::vec!["bKash"]);
        // A shop that was never asked what to do about the shelf does nothing,
        // which is what it was doing.
        assert_eq!(shop.stock_rule, 0);
    }

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
            vat_base: VatBase::Discounted,
            barcodes: vec![boxed("8690000000012")],
            on_hand: Milli::new(40_000),
            active: true,
            supply: crate::domain::Supply::Standard,
            category: "".into(),
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

    /// What the shop sorts a thing under reaches a till and comes back whole.
    ///
    /// Both directions matter: the till shows it beside an item, and a sale
    /// rung from a catalogue this device restored from its own disk has to be
    /// the same item it pulled.
    #[test]
    fn what_a_shop_sorts_a_thing_under_survives_the_disk() {
        let mut sorted = item();
        sorted.category = "Rice".into();
        sorted.supply = crate::domain::Supply::Exempt;

        let bytes = encode_snapshot(&[sorted.clone()], 5).unwrap();
        let (restored, _) = decode_snapshot(SNAPSHOT_SCHEMA, &bytes).unwrap();
        assert_eq!(&*restored[0].category, "Rice");
        assert_eq!(restored[0].supply, crate::domain::Supply::Exempt);

        // And a catalogue written by the build before either existed reads as
        // sorted under nothing and taxed the ordinary way, which is what those
        // builds meant.
        let older = postcard::to_allocvec(&SnapshotV3Legacy {
            cursor: 5,
            items: alloc::vec![ItemV3Legacy {
                id: 1,
                code: alloc::string::String::from("RICE5"),
                name_en: alloc::string::String::from("Rice Miniket 5kg"),
                name_bn: alloc::string::String::from("Rice Miniket 5kg"),
                unit: alloc::string::String::from("Nos"),
                price_minor: 43_000,
                cost_minor: 38_000,
                vat_bp: 1_500,
                price_inclusive: false,
                vat_on_undiscounted: false,
                barcodes: alloc::vec![alloc::string::String::from("8690000000001")],
                on_hand_milli: 40_000,
                active: true,
                supply: 1,
            }],
        })
        .unwrap();
        let (read, _) = decode_snapshot(SNAPSHOT_SCHEMA_V3, &older).unwrap();
        assert!(read[0].category.is_empty(), "nobody sorted it");
        assert_eq!(
            read[0].supply,
            crate::domain::Supply::ZeroRated,
            "and what that build did say is kept"
        );
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
        let snapshot = SnapshotV1 {
            cursor: 0,
            items: vec![wire],
        };
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
        assert!(
            per_item < 200,
            "{per_item} bytes an item is larger than expected"
        );
    }
}
