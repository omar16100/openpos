//! The network protocol, shared by till and server.
//!
//! Deliberately separate from the on-disk types in [`crate::storage::wire`].
//! Disk needs backward compatibility, because new code reads old bytes. The
//! network needs that *and* forward compatibility, because an old till has to
//! keep working against a newer server. Sharing one struct would mean a protocol
//! change forcing a disk migration, and a disk change breaking the wire.
//!
//! Forward compatibility here comes from negotiation rather than from a
//! self-describing format. Every request states the protocol version it speaks,
//! and the server answers in that version or refuses. postcard is positional and
//! cannot tolerate an unexpected field, so guessing is not an option; saying
//! which dialect you speak is cheap and leaves no ambiguity.
//!
//! A till two weeks offline, syncing into a server that has been upgraded twice,
//! is the case this exists for.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// Protocol this build speaks.
///
/// Bumped when a request or a reply changes shape, which is not the same as
/// adding a route. These bodies are positional: a field added to a struct makes
/// every older body undecodable, so the version is what tells the two sides
/// which shape they are looking at. Version 2 added who counted a drawer.
pub const PROTOCOL_VERSION: u16 = 2;

/// Oldest protocol this build still answers. The server keeps one version of
/// slack so a till can be a release behind without being cut off mid-day.
///
/// Slack is not free: every shape that changed since then needs a legacy struct
/// here and a branch where it is read, the same way the storage layer keeps one.
/// Version 1 differs in one shape, the closed drawer, and that is below.
pub const MINIMUM_PROTOCOL_VERSION: u16 = 1;

/// Why a request could not be served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolError {
    /// The caller speaks a version this server no longer supports, or one from
    /// the future. Carries the range so the client can report something useful
    /// rather than "sync failed".
    UnsupportedVersion {
        requested: u16,
        minimum: u16,
        current: u16,
    },
    /// The terminal is not enrolled for this tenant, or not enrolled at all.
    UnknownTerminal,
    /// The payload did not decode.
    Malformed,
    /// No credential, or one the server does not recognise.
    ///
    /// Appended rather than inserted: these are encoded positionally, so
    /// reordering the variants would make an older till read one refusal as
    /// another.
    Unauthenticated,
    /// Too many attempts in too short a time. Carries when to try again, so a
    /// client waits rather than hammering.
    TooManyAttempts { retry_after_seconds: u64 },
    /// The credential is genuine but is not for this. A till may ring sales and
    /// sync; it may not reprice the shop.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    NotPermitted,
}

/// Check a request's version before doing anything else with it.
pub fn negotiate(requested: u16) -> Result<u16, ProtocolError> {
    if !(MINIMUM_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&requested) {
        return Err(ProtocolError::UnsupportedVersion {
            requested,
            minimum: MINIMUM_PROTOCOL_VERSION,
            current: PROTOCOL_VERSION,
        });
    }
    Ok(requested)
}

// ---------------------------------------------------------------------------
// Push: sales from a till to the server
// ---------------------------------------------------------------------------

/// One sale, forwarded exactly as it was committed.
///
/// The payload is the bytes already on the terminal's disk, not a re-encoding.
/// Re-encoding risks sending something subtly different from what is stored and
/// from what the receipt in the customer's hand says, and that difference would
/// only ever show up in a dispute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleEnvelope {
    pub id: u128,
    /// Schema of the payload, so the server picks the right decoder rather than
    /// assuming the newest.
    pub schema: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sales: Vec<SaleEnvelope>,
}

/// Why the server would not accept a sale as it stands.
///
/// None of these reject the sale outright. It happened: goods left the shop and
/// money changed hands. The server records it and raises a repair item, because
/// refusing it would mean the only copy is on a tablet that might not survive
/// the week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineReason {
    /// Stored totals disagree with what the shared arithmetic recomputes. Should
    /// be impossible, since both sides run the same crate, so it means tampering
    /// or corruption rather than a rounding difference.
    TotalsMismatch {
        stored_minor: i64,
        recomputed_minor: i64,
    },
    /// Another sale already carries this receipt number under this epoch. The
    /// usual cause is a terminal restored from a backup or a cloned tablet.
    DuplicateReceiptNumber { receipt_no: String },
    /// The payload could not be decoded under the schema it claimed.
    Undecodable,
    /// Carried in by hand from a device that could not send it: a till whose
    /// terminal the shop deleted, or bytes read back out of a torn log. The
    /// sale is stored and somebody is asked to look at it, because the ordinary
    /// path is a credential and this one is a person with a file.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older device read one reason as another.
    CarriedIn,
}

/// Sales handed to the shop by somebody carrying them, rather than sent.
///
/// The one way out for a device that cannot sync: its terminal was deleted, or
/// it has to be re-enrolled as another and its outbox is the only record of
/// goods that left the shop. Owner only, because the credential that would
/// ordinarily prove where these came from is exactly what is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptSalesRequest {
    pub protocol: u16,
    /// The terminal that rang them, as the device believes itself to be. Kept
    /// as it is even where the shop no longer lists that terminal: it is what
    /// the receipts say, and a sale filed under the wrong till is a sale nobody
    /// can find again.
    pub terminal: u128,
    pub sales: Vec<SaleEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptSalesResponse {
    pub protocol: u16,
    /// Now safely stored, including ones already here from an earlier attempt.
    /// Safe for the device to be wiped once it has seen these.
    pub adopted: Vec<u128>,
    /// Stored, and waiting for somebody to look. Every carried sale is, by the
    /// fact of being carried; this names anything wrong with them beyond that.
    pub needing_attention: Vec<Quarantined>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantined {
    pub id: u128,
    pub reason: QuarantineReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub protocol: u16,
    /// Sales now safely stored, including ones already present from an earlier
    /// attempt. A till may drop these from its outbox.
    pub accepted: Vec<u128>,
    /// Stored, but needing a human. Also safe for the till to drop: the server
    /// has them.
    pub quarantined: Vec<Quarantined>,
}

impl PushResponse {
    /// Everything the till may now consider delivered.
    #[must_use]
    pub fn settled(&self) -> Vec<u128> {
        let mut ids = self.accepted.clone();
        ids.extend(self.quarantined.iter().map(|item| item.id));
        ids
    }
}

// ---------------------------------------------------------------------------
// Pull: catalogue changes from the server to a till
// ---------------------------------------------------------------------------

/// An item as it travels over the network.
///
/// A near-twin of the on-disk shape, and deliberately not the same type. They
/// evolve on different schedules, and the day they need to differ is the day
/// sharing one would have hurt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWire {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    /// True when VAT is charged on the price before discounts.
    ///
    /// Appended, never inserted: these encode positionally, and a field placed
    /// in the middle would make an older till read a barcode as a boolean.
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

/// An item as version 1 of the catalogue format wrote it.
///
/// Kept only to read what version 1 wrote, and never written. postcard is
/// positional, so the field added for the tax base cannot be read out of these
/// bytes: without this, every catalogue row stored before that change would
/// stop decoding, and every till in every shop would stop pulling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWireV1 {
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

impl ItemWireV1 {
    /// Every item written before the tax base was a choice was taxed the
    /// ordinary way, because that was the only way there was.
    #[must_use]
    pub fn into_current(self) -> ItemWire {
        ItemWire {
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
pub struct PullRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// Server sequence the till already has.
    pub cursor: u64,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullResponse {
    pub protocol: u16,
    /// Sequence after applying this batch.
    pub cursor: u64,
    pub upserts: Vec<ItemWire>,
    pub tombstones: Vec<u128>,
    /// True when more changes are waiting, so the till knows to ask again rather
    /// than assuming it is current.
    pub more: bool,
}

// ---------------------------------------------------------------------------
// Enrolment: a new device trading a short code for a real credential
// ---------------------------------------------------------------------------

/// The one request that carries no credential, because it is how a device gets
/// one. It states no tenant and no terminal either: both are read from the code,
/// so a device cannot enrol itself into a shop it was not invited to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolRequest {
    pub protocol: u16,
    /// As typed by a person. The server normalises before comparing.
    pub code: String,
}

/// Ask for a code that will enrol a new device.
///
/// The terminal id is minted by the asking device, as sale ids and count ids
/// are. Identity is created where the work happens, so nothing waits on a
/// server to be allowed to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCodeRequest {
    pub protocol: u16,
    pub terminal_id: u128,
    /// What the shop calls this device. Printed in the terminal health list, so
    /// "the one by the door" beats a uuid.
    pub label: String,
    /// 1 till, 2 owner. A caller may not ask for more than it holds.
    pub role: i16,
    pub valid_for_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCodeResponse {
    pub protocol: u16,
    /// Shown once, to be read onto the new device. Never retrievable again:
    /// only its hash is kept.
    pub code: String,
    pub terminal_id: u128,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolResponse {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// Shown to the device once and never retrievable again.
    pub token: String,
}

// ---------------------------------------------------------------------------
// Operators: the people who stand at a till
// ---------------------------------------------------------------------------

/// A person, as they travel.
///
/// The PIN is not here and never is. What crosses is a salt, a round count and
/// a derived key, computed on the owner's device by the same code the till uses
/// to verify: the PIN itself does not go over the network, and a copy of this
/// message is worth no more than a copy of the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorWire {
    pub id: u128,
    pub name: String,
    pub pin_salt: Vec<u8>,
    pub pin_rounds: u32,
    pub pin_key: Vec<u8>,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutOperatorRequest {
    pub protocol: u16,
    pub operator: OperatorWire,
}

/// Change a person, except their PIN.
///
/// Its own request rather than a field on the upsert, because that one carries
/// the whole person including the derived PIN key, and the back office does not
/// have it: a PIN is hashed on the owner's device when it is set and never
/// leaves it. Asking an owner to retype somebody's PIN to correct their name,
/// or to take the drawer away from them, is asking them to know it.
///
/// One request rather than one per field. Suspending and renaming are the same
/// act from here - changing what can be changed without the PIN - and two
/// routes for that would be two places to forget the owner check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmendOperatorRequest {
    pub protocol: u16,
    pub operator_id: u128,
    pub name: String,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

/// Give somebody a new PIN.
///
/// Its own request, carrying a credential and nothing else, because amending a
/// person carries no credential and that is the point of it. Two acts, two
/// shapes, and neither can be used to do the other by leaving a field out.
///
/// The key is derived on the owner's device by the same code the till checks it
/// with, so the PIN itself never travels and this is worth nothing to somebody
/// who reads it off the wire without also having the PIN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetOperatorPinRequest {
    pub protocol: u16,
    pub operator_id: u128,
    pub pin_salt: Vec<u8>,
    /// Carried per person, so raising the cost later does not lock out
    /// everybody who set a PIN before.
    pub pin_rounds: u32,
    pub pin_key: Vec<u8>,
}

/// Ask for the people who may stand at this till.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorsRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorsResponse {
    pub protocol: u16,
    pub operators: Vec<OperatorWire>,
}

// ---------------------------------------------------------------------------
// Shop details: what goes at the top of a receipt
// ---------------------------------------------------------------------------

/// Ask for the shop's own details.
///
/// A separate exchange rather than a field added to the pull, because appending
/// to a reply everything already speaks is a protocol version bump and this is
/// not worth one. A new path costs an old till nothing: it simply never asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopResponse {
    pub protocol: u16,
    pub name: String,
    /// Absent rather than empty when the shop has none. A receipt omits what is
    /// missing; it does not print a label with nothing after it.
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// The wallets this shop takes, by the name a report should read.
    ///
    /// Appended, never inserted: these encode positionally, and a field placed
    /// in the middle would make an older till read a phone number as a list.
    ///
    /// Set here rather than typed at a till, because a shop that takes two will
    /// otherwise type both names all day, and one typo makes a third that then
    /// has its own line in every report and reconciles against nothing.
    #[serde(default)]
    pub wallets: Vec<String>,
}

/// Set the shop's own details. Owner only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutShopRequest {
    pub protocol: u16,
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// Appended, never inserted, for the same reason as on the response.
    #[serde(default)]
    pub wallets: Vec<String>,
}

// ---------------------------------------------------------------------------
// Purchasing: who the shop buys from, and what arrived
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierWire {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    /// Business Identification Number. Optional because most neighbourhood
    /// suppliers do not have one, and a required field would be filled with
    /// zeros.
    pub bin: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutSupplierRequest {
    pub protocol: u16,
    pub supplier: SupplierWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuppliersRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuppliersResponse {
    pub protocol: u16,
    pub suppliers: Vec<SupplierWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLineWire {
    pub item_id: u128,
    pub qty_milli: i64,
    /// What this delivery cost per unit, which is what a margin is measured
    /// against rather than the item's standing cost.
    pub unit_cost_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveGoodsRequest {
    pub protocol: u16,
    /// Minted on the device, so a delivery survives a dropped reply and can be
    /// resent without being booked twice.
    pub id: u128,
    pub supplier_id: Option<u128>,
    /// The supplier's own invoice or challan number.
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub note: Option<String>,
    pub lines: Vec<ReceiptLineWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveGoodsResponse {
    pub protocol: u16,
    /// False when this delivery was already booked. Not an error: a retry after
    /// a dropped reply is normal, and the caller needs to know it was recognised
    /// rather than silently counted again.
    pub recorded: bool,
    /// What each received item now holds, as the server computes it.
    pub on_hand: Vec<OnHandEntry>,
}

// ---------------------------------------------------------------------------
// Stock counts: asserting what the shelf holds
// ---------------------------------------------------------------------------

/// One counted line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountedItem {
    /// Minted on the device, so a count survives a dropped reply and can be
    /// resent without being recorded twice.
    pub id: u128,
    pub item_id: u128,
    pub counted_milli: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCountRequest {
    pub protocol: u16,
    /// Device clock at the moment of counting, shared by every line in one
    /// count. It decides which sales the count should already reflect, and is
    /// never used to order counts between terminals.
    pub counted_at_ms: u64,
    pub note: Option<String>,
    pub lines: Vec<CountedItem>,
}

/// A drawer that has been counted and closed.
///
/// Pushed rather than kept on the device. The whole point of counting a drawer
/// is that somebody who was not at the till reconciles it, and until this
/// existed a cashier counted, the till worked out the variance, and the owner
/// had to take their word for both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWire {
    pub id: u128,
    pub terminal: u128,
    /// Who counted it, and what they were called at the time. The name is
    /// carried rather than looked up, because somebody who has since left the
    /// shop is still the person this variance belongs to.
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
    /// What the drawer should have held.
    pub expected_cash_minor: i64,
    /// What was in it.
    pub counted_cash_minor: i64,
    /// Counted less expected. Negative is short, which is a fact to report
    /// rather than an error: a shift that could not be closed short would be
    /// closed dishonestly instead.
    pub variance_minor: i64,
}

/// A closed drawer as version 1 sent one, before it said who counted it.
///
/// Kept so a till a release behind can still hand over the drawer it counted.
/// Losing that is losing the only record that a cashier counted and the till
/// agreed, which is the whole reason it is pushed rather than kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWireV1 {
    pub id: u128,
    pub terminal: u128,
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

impl From<ClosedShiftWireV1> for ClosedShiftWire {
    fn from(old: ClosedShiftWireV1) -> Self {
        Self {
            id: old.id,
            terminal: old.terminal,
            // A drawer counted by a till that did not write down who counted it.
            // The shop knows it happened and cannot be told by whom.
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

impl From<ClosedShiftWire> for ClosedShiftWireV1 {
    fn from(new: ClosedShiftWire) -> Self {
        // The name is dropped rather than translated: a version 1 reader has
        // nowhere to put it and would misread the bytes if it were sent.
        Self {
            id: new.id,
            terminal: new.terminal,
            opened_at_ms: new.opened_at_ms,
            closed_at_ms: new.closed_at_ms,
            opening_float_minor: new.opening_float_minor,
            sales: new.sales,
            cash_sales_minor: new.cash_sales_minor,
            non_cash_sales_minor: new.non_cash_sales_minor,
            cash_in_minor: new.cash_in_minor,
            cash_out_minor: new.cash_out_minor,
            expected_cash_minor: new.expected_cash_minor,
            counted_cash_minor: new.counted_cash_minor,
            variance_minor: new.variance_minor,
        }
    }
}

/// Drawers pushed by a version 1 till.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsRequestV1 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shifts: Vec<ClosedShiftWireV1>,
}

/// Drawers read by a version 1 back office.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponseV1 {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWireV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shifts: Vec<ClosedShiftWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsResponse {
    pub protocol: u16,
    /// Shifts the server now holds, including ones it already had: a till may
    /// drop these. A repeat is ordinary, not an error, because a dropped reply
    /// is the usual reason a till sends one twice.
    pub accepted: Vec<u128>,
}

/// Cut a device off.
///
/// What a shop needs the moment a tablet is lost or stolen: every credential
/// that terminal holds stops working. The terminal itself stays, because its
/// sales are still its sales and a shop investigating a theft wants to see that
/// a device existed rather than an absence.
///
/// The device is not wiped and cannot be: it may be holding sales nobody else
/// has, and if it is ever recovered those are read off it and carried in by
/// hand. Cutting it off is what stops it doing anything new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeTerminalRequest {
    pub protocol: u16,
    pub terminal: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeTerminalResponse {
    pub protocol: u16,
    /// How many credentials were withdrawn. Zero is an ordinary answer: a
    /// device enrolled and never used, or one already cut off.
    pub withdrawn: u32,
}

/// Ask what sold over a period.
///
/// The question a shop asks before it orders: what moved, and how much of it.
/// Answered from the stock movements each sale wrote rather than from its
/// payload, because those are already the server's own reading of the lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
    pub limit: u32,
}

/// How much of one item left the shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldWire {
    pub item_id: u128,
    /// Positive is what left the shop. A period with more returns than sales of
    /// one thing shows negative, which is a fact worth seeing.
    pub qty_milli: i64,
    pub sales: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldResponse {
    pub protocol: u16,
    /// Most sold first. Names are not here: the device asking already holds the
    /// catalogue, and sending them again would be the same strings on every
    /// report for the life of the shop.
    pub rows: Vec<SoldWire>,
}

/// Ask what the shop owes its suppliers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingRequest {
    pub protocol: u16,
}

/// What the shop owes one supplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingWire {
    pub supplier_id: u128,
    pub name: String,
    /// Positive is owed by the shop. Negative means it has paid ahead, which
    /// happens and is worth showing rather than hiding.
    pub owed_minor: i64,
    pub deliveries: u32,
    pub since_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingResponse {
    pub protocol: u16,
    pub owing: Vec<SupplierOwingWire>,
}

/// Ask which catalogue changes never reached the tills.
///
/// A change this build cannot read is passed over and the cursor moves on, so a
/// shop can lose a price change and have no way to find out. This is the way to
/// find out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangesRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// One change every till has passed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangeWire {
    pub seq: u64,
    pub item_id: u128,
    /// The schema its payload was written under: one number naming the build
    /// that wrote it, which is what somebody needs to know to fix it.
    pub schema: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangesResponse {
    pub protocol: u16,
    pub changes: Vec<UnreadableChangeWire>,
}

/// Ask what passed between the shop and one supplier over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierStatementRequest {
    pub protocol: u16,
    pub supplier_id: u128,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// One line of that: goods in, or money out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierEntryWire {
    pub at_ms: u64,
    /// True when goods came in, false when money went out.
    pub delivered: bool,
    /// Positive either way: what arrived, or what was handed over.
    pub amount_minor: i64,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierStatementResponse {
    pub protocol: u16,
    /// Oldest first, and a delivery before a payment made in the same moment:
    /// goods arrive and are then paid for.
    pub entries: Vec<SupplierEntryWire>,
    /// What the period ends owing, so the paper and the total agree without the
    /// screen adding the lines up itself.
    pub owed_minor: i64,
}

/// Record money paid to a supplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaySupplierRequest {
    pub protocol: u16,
    /// Minted by whoever recorded it, so a resent one is not counted twice.
    pub id: u128,
    pub supplier_id: u128,
    /// What was handed over. Positive.
    pub amount_minor: i64,
    pub paid_at_ms: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaySupplierResponse {
    pub protocol: u16,
    /// False when this payment was already recorded, which is ordinary: a
    /// dropped reply is the usual reason one is sent twice.
    pub paid: bool,
    /// What the shop owes them now, from the ledger rather than from the
    /// screen's own arithmetic.
    pub owed_minor: i64,
}

/// Ask what was sold at each tax rate over a period.
///
/// The figure a shop needs for its monthly return, which until now lived only
/// inside the sale payloads: answering it meant decoding every ticket of the
/// month, the most expensive way to answer a question asked twelve times a year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// What was sold at one rate, and the tax on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRowWire {
    /// Basis points, so fifteen percent is 1500 and a rate that changes next
    /// year is a different row rather than a rewrite of this one.
    pub vat_bp: u32,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub sales: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatResponse {
    pub protocol: u16,
    /// Smallest rate first. Refunds carry their own sign and subtract, which is
    /// what a return wants.
    pub rows: Vec<VatRowWire>,
    /// How much of that figure comes from sales still waiting on somebody to
    /// look at them. Counted rather than removed: a duplicate receipt
    /// over-declares and a sale nobody has looked at may be either, and this is
    /// a number a shop signs its name to. The machine says how much is
    /// uncertain; the person filing decides.
    pub waiting_sales: u64,
    pub waiting_vat_minor: i64,
}

/// Ask what a day looked like.
///
/// The question an owner asks once, at closing: what was sold, what came back,
/// what the drawers held against what they should have, and what went on
/// account rather than into the till. Answered in one call because it is one
/// question, and from the headers and the two ledgers rather than by decoding
/// tickets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayRequest {
    pub protocol: u16,
    /// Inclusive, by the clock of whoever rang the sale. A shop's day ends when
    /// it closes, not at midnight.
    pub from_ms: u64,
    pub to_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayResponse {
    pub protocol: u16,
    pub sales: u64,
    pub total_minor: i64,
    pub refunds: u64,
    pub refunded_minor: i64,
    /// Drawers counted and closed in the period, and how they came out.
    pub drawers_counted: u32,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
    /// Put on somebody's account, taken off it, and struck off without money.
    /// Three numbers, because money the shop was given and money it gave up are
    /// not the same thing and a day that nets to zero because one balanced the
    /// other is a day somebody should look at.
    pub charged_minor: i64,
    pub paid_minor: i64,
    pub written_off_minor: i64,
    pub tills: Vec<TillTakings>,
}

/// Somebody the shop lets buy on account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerWire {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
}

/// Ask who the shop lets buy on account.
///
/// A till's route as well as the back office's, like the people who may sign
/// in: a sale on account is written with the internet down, so the names have
/// to be on the device before they are wanted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomersRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomersResponse {
    pub protocol: u16,
    pub customers: Vec<CustomerWire>,
}

/// Ask what each of them owes.
///
/// Its own route rather than a field on the customer list, because the two move
/// at different speeds: a name is written down once and a balance changes every
/// time somebody takes a bag of rice. A till asking for names every few minutes
/// to learn a number would be asking the shop to send the same list all day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancesRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

/// What one person owes, as the shop's book stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceWire {
    pub customer: u128,
    /// Positive is owed to the shop.
    pub owed_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancesResponse {
    pub protocol: u16,
    /// Only the people who owe something. A shop with two hundred names on the
    /// list and four of them owing sends four numbers.
    pub balances: Vec<BalanceWire>,
}

/// Add or correct somebody who buys on account. Owner only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCustomerRequest {
    pub protocol: u16,
    pub customer: CustomerWire,
}

/// What a till has open right now.
///
/// Sent while a drawer is open rather than only when it closes, which was the
/// only moment the shop ever heard about one. A till left open overnight and
/// wiped in the morning took its whole takings summary with it, and nobody
/// could ask which tills still had a drawer open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDrawerRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shift: u128,
    pub opened_at_ms: u64,
    /// The till's clock when it said this.
    pub at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDrawerResponse {
    pub protocol: u16,
}

/// Ask which tills have a drawer open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawersRequest {
    pub protocol: u16,
}

/// A drawer somebody has open, as that till last said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawerWire {
    pub terminal: u128,
    pub shift: u128,
    pub opened_at_ms: u64,
    /// When the till last said this. Four hours ago and four minutes ago are
    /// different things and only the shop can say which matters.
    pub reported_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawersResponse {
    pub protocol: u16,
    pub drawers: Vec<OpenDrawerWire>,
}

/// Ask for the drawers a shop has closed lately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsRequest {
    pub protocol: u16,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponse {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWire>,
}

// ---------------------------------------------------------------------------
// What people owe the shop
// ---------------------------------------------------------------------------

/// Ask who owes the shop money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// What one person owes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwingWire {
    /// The folded name, which is what a payment is recorded against. Sent so a
    /// screen names the same person the server did, rather than folding a
    /// display name again and hoping the two agree.
    pub person_key: String,
    pub person_name: String,
    /// Positive is owed to the shop. Negative means they are in credit, which
    /// is worth showing rather than hiding.
    pub owed_minor: i64,
    /// When the oldest entry still in this balance was made.
    pub since_ms: u64,
    pub last_at_ms: u64,
    pub entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedResponse {
    pub protocol: u16,
    pub owing: Vec<OwingWire>,
}

/// Take money off what somebody owes, or strike a debt off without money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakePaymentRequest {
    pub protocol: u16,
    /// Minted by whoever took the payment, so a resent one is not counted
    /// twice. A payment counted twice is money the shop believes it has been
    /// given and has not.
    pub id: u128,
    pub person_key: String,
    /// What to call them if this is the first entry under that key.
    pub person_name: String,
    /// What was handed over, or what is being struck off. Positive either way.
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: Option<String>,
    /// True when no money changed hands: a sale rung twice by a till restored
    /// from a backup, goods brought back, an argument settled. It needs a note,
    /// and it is never added in with money the shop was actually given.
    pub written_off: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakePaymentResponse {
    pub protocol: u16,
    /// False when this payment was already recorded, which is ordinary: a
    /// dropped reply is the usual reason one is sent twice.
    pub taken: bool,
    /// What they owe now, so a screen shows the truth rather than its own
    /// arithmetic.
    pub owed_minor: i64,
}

/// Ask what makes up one person's balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRequest {
    pub protocol: u16,
    pub person_key: String,
    pub limit: u32,
}

/// One line of somebody's account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntryWire {
    /// The sale that created the debt, or the payment that reduced it.
    pub source_id: u128,
    pub is_sale: bool,
    /// True when it came off the account without money changing hands.
    pub written_off: bool,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountResponse {
    pub protocol: u16,
    pub entries: Vec<AccountEntryWire>,
}

/// What one till took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillTakings {
    pub terminal: u128,
    pub sales: u64,
    pub total_minor: i64,
    /// Sales in this period the server has quarantined. Carried per till,
    /// because one till producing all of them is a different problem from every
    /// till producing one.
    pub needing_attention: u64,
}

/// Ask what has been delivered lately.
///
/// Newest first and capped, because the question a shop asks is "what came in
/// this week" rather than "everything since we opened", and the answer to the
/// second would be a page nobody can read on a tablet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// One line of a delivery, as it comes back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredLineWire {
    pub item_id: u128,
    pub qty_milli: i64,
    pub unit_cost_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryWire {
    pub id: u128,
    pub supplier_id: Option<u128>,
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub lines: Vec<DeliveredLineWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesResponse {
    pub protocol: u16,
    pub deliveries: Vec<DeliveryWire>,
}

/// Ask what a set of items is believed to hold.
///
/// A separate question from the catalogue, because a sale is not a catalogue
/// change: the figure on an item record is whatever it was when somebody last
/// edited that item, and a screen that shows it as stock shows a number that
/// never moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandRequest {
    pub protocol: u16,
    /// Empty means everything the shop sells. A shop with ten thousand lines
    /// asks for the page it is looking at instead.
    pub item_ids: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandResponse {
    pub protocol: u16,
    pub on_hand: Vec<OnHandEntry>,
}

/// What one item is now believed to hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandEntry {
    pub item_id: u128,
    pub qty_milli: i64,
    /// When it was last counted, if ever. Absent means the figure is a running
    /// total resting on no count, which a shop should be told.
    pub counted_at_ms: Option<u64>,
    /// Sales rung before the last count but which reached the server after it.
    /// Not included in `qty_milli`, because nobody can say whether the person
    /// counting saw those goods.
    pub unreconciled_milli: i64,
    pub unreconciled_sales: u32,
}

/// Stock leaving or entering for a reason that is neither a sale nor a
/// delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectStockRequest {
    pub protocol: u16,
    pub id: u128,
    pub item_id: u128,
    /// Signed: negative for goods gone, positive for a count that was under.
    pub qty_milli: i64,
    /// Why. Required, and refused when blank: an unexplained correction is
    /// indistinguishable from theft when the variance is read a month later.
    pub reason: String,
    pub occurred_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectStockResponse {
    pub protocol: u16,
    /// False when this correction was already recorded. A retry after a dropped
    /// reply is normal and is not an error.
    pub recorded: bool,
    pub on_hand: Option<OnHandEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCountResponse {
    pub protocol: u16,
    /// The figure for each counted item after the count, so the device shows
    /// what the server concluded rather than what it asserted.
    pub on_hand: Vec<OnHandEntry>,
}

// ---------------------------------------------------------------------------
// Renewal: a credential that would otherwise run out
// ---------------------------------------------------------------------------

/// Ask for a fresh credential, authenticated with the current one.
///
/// Carries no identity, like every other authenticated request: the server takes
/// the tenant and terminal from the token presented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewResponse {
    pub protocol: u16,
    /// Shown once. The device must store it before acting on this reply.
    pub token: String,
    /// Seconds until the new credential expires, so a till can decide when to
    /// ask again rather than each build hard-coding the server's policy.
    pub expires_in_seconds: u64,
    /// Seconds the old credential keeps working.
    ///
    /// Not zero, and that is the point: if this reply is lost, the device still
    /// holds only the old token, and revoking it immediately would strand a till
    /// with no way to authenticate and no way to ask again.
    pub previous_valid_for_seconds: u64,
}

// ---------------------------------------------------------------------------
// Lease: receipt numbers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// How many numbers the till wants. The server may grant fewer.
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseResponse {
    pub protocol: u16,
    /// Bumped when the server believes this terminal was replaced or restored,
    /// so numbers issued under the old epoch remain attributable.
    pub epoch: u64,
    pub prefix: String,
    pub first: u64,
    pub last: u64,
}

// ---------------------------------------------------------------------------
// Back office: the shop owner rather than the till
// ---------------------------------------------------------------------------
//
// These carry a tenant and a terminal like every other authenticated request,
// and for the same reason: the server compares both against the credential and
// refuses a mismatch, so a console configured against the wrong shop is told so
// instead of being quietly served somebody else's numbers.
//
// Times are milliseconds since the Unix epoch, matching `rung_at_ms` on a sale.
// Deliberately not a formatted string: the back office renders in the shop's own
// locale, and a server that pre-formats forces its own.

/// Ask for the sales that need a human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairQueueRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// How many entries to return. The server clamps this: a shop with a broken
    /// till can accumulate thousands, and a page nobody can download is a queue
    /// nobody can work through.
    pub limit: u32,
}

/// One sale waiting on a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairEntry {
    pub id: u128,
    /// Absent when the till sold without a leased block, or when the payload
    /// could not be decoded far enough to find one.
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    /// When the server received it, not when it was rung up. The gap between the
    /// two is how long the till was offline, which is usually the story.
    pub received_at_ms: u64,
    /// Prose, written for the person deciding what to do about the sale.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairQueueResponse {
    pub protocol: u16,
    pub entries: Vec<RepairEntry>,
}

/// Mark one quarantined sale as dealt with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRepairRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sale: u128,
    /// What the shop decided, kept beside the sale. The queue is worked by a
    /// person months before anyone asks why a total was wrong, and an entry that
    /// disappears without a note leaves that question unanswerable.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRepairResponse {
    pub protocol: u16,
    /// False when the sale was already resolved, or is not in the queue at all.
    /// The two are one answer because acting on either is the same: reload the
    /// queue and look again.
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

/// One terminal, as support sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthEntry {
    pub terminal: u128,
    pub label: String,
    /// Bumped when the server believes the device was replaced or restored. A
    /// jump here explains duplicate receipt numbers further down the queue.
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    /// When the server last heard this terminal sync. `None` for a device that
    /// has not been heard from since the column existed, which is not the same
    /// as a device that has never synced, and is not worth pretending otherwise.
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    /// Unresolved quarantined sales from this terminal. One till producing all
    /// of them is a device fault; every till producing some is a release fault.
    pub open_repairs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthResponse {
    pub protocol: u16,
    pub terminals: Vec<TerminalHealthEntry>,
}

/// Create or replace one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertItemRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub item: ItemWire,
}

/// Withdraw one item. The id travels alone: tills need a tombstone, not a copy
/// of what was deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteItemRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub item: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueEditResponse {
    pub protocol: u16,
    /// Sequence this edit was recorded at. A till that has already pulled past
    /// it is current; one behind it has work to do.
    pub cursor: u64,
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
    fn accepts_the_versions_it_speaks() {
        assert_eq!(negotiate(PROTOCOL_VERSION), Ok(PROTOCOL_VERSION));
    }

    #[test]
    fn refuses_a_version_from_the_future_with_something_useful_to_say() {
        assert_eq!(
            negotiate(99),
            Err(ProtocolError::UnsupportedVersion {
                requested: 99,
                minimum: MINIMUM_PROTOCOL_VERSION,
                current: PROTOCOL_VERSION,
            })
        );
    }

    #[test]
    fn refuses_a_version_that_has_been_retired() {
        assert!(negotiate(0).is_err());
    }

    #[test]
    fn a_till_may_drop_everything_the_server_settled() {
        let response = PushResponse {
            protocol: PROTOCOL_VERSION,
            accepted: vec![1, 2],
            quarantined: vec![Quarantined {
                id: 3,
                reason: QuarantineReason::Undecodable,
            }],
        };
        // Quarantined sales are stored too. Holding them on the till would mean
        // the only copy sits on a tablet nobody has backed up.
        assert_eq!(response.settled(), vec![1, 2, 3]);
    }

    #[test]
    fn requests_and_responses_round_trip() {
        let request = PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 42,
            terminal: 7,
            sales: vec![SaleEnvelope {
                id: 900,
                schema: 1,
                payload: vec![1, 2, 3],
            }],
        };
        let bytes = postcard::to_allocvec(&request).unwrap();
        let restored: PushRequest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored, request);
    }

    #[test]
    fn a_terminal_never_heard_from_survives_the_wire_as_absent() {
        // postcard encodes an Option as a tag, so the difference between "never
        // synced" and "synced at time zero" is preserved rather than collapsing
        // into an epoch timestamp that would read as 1970 in the back office.
        let response = TerminalHealthResponse {
            protocol: PROTOCOL_VERSION,
            terminals: vec![TerminalHealthEntry {
                terminal: 7,
                label: alloc::string::String::from("Counter"),
                epoch: 1,
                enrolled_at_ms: 1_788_600_000_000,
                last_seen_ms: None,
                sales: 0,
                open_repairs: 0,
            }],
        };
        let bytes = postcard::to_allocvec(&response).unwrap();
        let restored: TerminalHealthResponse = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored, response);
        assert_eq!(restored.terminals[0].last_seen_ms, None);
    }
}
