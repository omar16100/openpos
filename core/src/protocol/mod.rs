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
pub const PROTOCOL_VERSION: u16 = 1;

/// Oldest protocol this build still answers. The server keeps one version of
/// slack so a till can be a release behind without being cut off mid-day.
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
    if requested < MINIMUM_PROTOCOL_VERSION || requested > PROTOCOL_VERSION {
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

/// Suspend somebody, or let them back in.
///
/// Its own request rather than a field on the upsert, because that one carries
/// the whole person including the derived PIN key, and the back office does not
/// have it: a PIN is hashed on the owner's device when it is set and never
/// leaves it. Asking an owner to retype somebody's PIN to suspend them is
/// asking them to know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetOperatorActiveRequest {
    pub protocol: u16,
    pub operator_id: u128,
    pub active: bool,
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
}

/// Set the shop's own details. Owner only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutShopRequest {
    pub protocol: u16,
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
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
