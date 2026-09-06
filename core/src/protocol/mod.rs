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
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolResponse {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// Shown to the device once and never retrievable again.
    pub token: String,
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
}
