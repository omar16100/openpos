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
use crate::domain::{Discount, PriceMode, VatBase};
use crate::ids::Ulid;
use crate::money::{Bp, Milli, Minor};
use crate::replica::Item;

/// Schema carried in the frame header for a snapshot payload.
/// Bumped when the tax base became a per-item choice. Version 1 is still read;
/// see `ItemV1Legacy`.
pub const SNAPSHOT_SCHEMA: u16 = 2;
/// Schema carried in the frame header for a committed sale.
pub const SALE_SCHEMA: u16 = 2;

/// The sale format as version 1 wrote it, read and converted.
///
/// A sale rung before a shop could say what it sold a thing by. Every one of
/// them meant pieces.
pub const SALE_SCHEMA_V1: u16 = 1;
/// Schema carried in the frame header for a batch of catalogue changes.
/// Bumped alongside the snapshot, for the same reason.
pub const DELTAS_SCHEMA: u16 = 2;
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
}

/// Every ticket currently parked.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldTicketsV1 {
    pub tickets: Vec<HeldTicketV1>,
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

pub const TERMINAL_SCHEMA: u16 = 3;

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
}

/// A drawer counted and closed, waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftV1 {
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

/// The standing state as version 2 wrote it, read and converted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateV2Legacy {
    pub leases: Vec<LeaseGrantV1>,
    pub held: HeldTicketsV1,
    pub unnumbered: u64,
    #[serde(default)]
    pub operators: Vec<OperatorV1>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub shop: Option<ShopV1>,
}

impl From<TerminalStateV2Legacy> for TerminalStateV1 {
    fn from(old: TerminalStateV2Legacy) -> Self {
        Self {
            leases: old.leases,
            held: old.held,
            unnumbered: old.unnumbered,
            operators: old.operators,
            token: old.token,
            shop: old.shop,
            // A device from before drawers were sent. Whatever it closed is on
            // its own paper and nowhere else, and this build cannot invent it.
            unsent_shifts: Vec::new(),
        }
    }
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
    pub held: HeldTicketsV1,
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
            held: old.held,
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
            }),
            unsent_shifts: Vec::new(),
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
        let salt: [u8; SALT_LEN] = self
            .salt
            .try_into()
            .map_err(|_| WireError::OutOfRange)?;
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
        TERMINAL_SCHEMA_V2 => postcard::from_bytes::<TerminalStateV2Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        TERMINAL_SCHEMA_V1 => postcard::from_bytes::<TerminalStateV1Legacy>(bytes)
            .map(Into::into)
            .map_err(|_| WireError::Malformed),
        other => Err(WireError::UnsupportedSchema { schema: other }),
    }
}

pub const SHIFT_SCHEMA: u16 = 1;

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
    },
}

/// Encode a drawer event.
pub fn encode_shift_event(event: &ShiftEventV1) -> Result<Vec<u8>> {
    postcard::to_allocvec(event).map_err(|_| WireError::Malformed)
}

/// Decode a drawer event written under `schema`.
pub fn decode_shift_event(schema: u16, bytes: &[u8]) -> Result<ShiftEventV1> {
    match schema {
        SHIFT_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
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

/// Decode one committed sale written under `schema`.
pub fn decode_sale(schema: u16, bytes: &[u8]) -> Result<SaleCommitV1> {
    match schema {
        SALE_SCHEMA => postcard::from_bytes(bytes).map_err(|_| WireError::Malformed),
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
            vat_on_undiscounted: matches!(line.vat_base, VatBase::Undiscounted),
            unit: line.unit.to_string(),
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
            held: HeldTicketsV1::default(),
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
            unsent_shifts: alloc::vec![],
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
            }),
        };
        let bytes = encode_terminal_state(&state).expect("it encodes");

        let read = decode_terminal_state(TERMINAL_SCHEMA, &bytes).expect("and decodes");
        assert_eq!(read.shop.expect("a shop").wallets, alloc::vec!["bKash"]);
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
