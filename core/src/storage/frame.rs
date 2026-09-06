//! The on-disk frame protocol.
//!
//! Everything openpos persists is a sequence of self-describing frames: a fixed
//! header, then an opaque payload. The header carries enough to detect the four
//! things that actually go wrong in the field, rather than the ones that are
//! pleasant to imagine:
//!
//! - a write torn by a tablet losing power mid-append
//! - a file from a newer version of the app, after a downgrade
//! - a backup restored onto a different terminal, or a cloned tablet image
//! - silent corruption on cheap eMMC
//!
//! Recovery truncates at the first frame that fails to verify. A torn tail costs
//! the sale that was mid-commit, which is the one the cashier is still holding;
//! it must never poison the frames before it, and it must never resurrect half a
//! ticket.
//!
//! This lives in the core, not in the platform backends, so it is written once
//! and property-tested against a fault-injecting mock. The alternative is
//! reimplementing crash safety in Dart and again in JavaScript, in the two places
//! the test suite cannot reach.

use alloc::vec::Vec;

/// `OPFR`, so a stray file is identifiable with `head -c 4`.
const MAGIC: [u8; 4] = *b"OPFR";

/// Bumped only when the header layout itself changes, which should be close to
/// never. Payload evolution is carried by `schema` instead.
pub const FRAME_FORMAT_VERSION: u16 = 1;

/// Header bytes preceding every payload.
pub const HEADER_LEN: usize = 60;

/// Which store a frame belongs to.
///
/// The two stores have different lifecycles and must never share a log: the
/// replica cache is truncated at every checkpoint, while the critical store is
/// truncated only once sales have been acknowledged by the server. One log
/// serving both means a checkpoint silently deletes unsynced sales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Store {
    /// Business evidence: tickets, tenders, outbox, lease book, shift and cash.
    /// Loss here is money and legal exposure.
    Critical = 1,
    /// The catalogue replica and its delta log. Rebuildable from the server.
    ReplicaCache = 2,
}

impl Store {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Critical),
            2 => Some(Self::ReplicaCache),
            _ => None,
        }
    }
}

/// What a payload holds. The frame layer does not interpret payloads; this only
/// tells the reader which decoder to hand the bytes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadKind {
    /// A whole sale committed atomically: ticket, lease-after state, shift and
    /// cash movement, outbox entry. One frame, so frame atomicity is transaction
    /// atomicity.
    SaleCommit = 1,
    /// Catalogue changes pulled from the server.
    ItemDeltas = 2,
    /// A full replica snapshot.
    Snapshot = 3,
    /// Lease block granted by the server.
    LeaseGrant = 4,
    /// A drawer opening, cash crossing it, or the count that closes it. Events
    /// rather than a shift snapshot: the sales themselves are already frames in
    /// this log, so replaying it in order is what reconstitutes a shift, and a
    /// snapshot could only ever disagree with them.
    ShiftEvent = 5,
    /// A print was attempted, and how it went. Recorded after the barrier, never
    /// blocking it.
    PrintAttempt = 6,
    /// Outbox entries acknowledged by the server.
    SyncAck = 7,
    /// Retired. Parked tickets briefly lived in the critical log, until it
    /// became clear that emptying that log on a full acknowledgement would take
    /// them with it. They live in the standing-state blob now. The number stays
    /// reserved so it can never come to mean something else.
    HeldTickets = 8,
    /// The terminal's standing state, written only into a blob slot.
    TerminalState = 9,
}

impl PayloadKind {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::SaleCommit),
            2 => Some(Self::ItemDeltas),
            3 => Some(Self::Snapshot),
            4 => Some(Self::LeaseGrant),
            5 => Some(Self::ShiftEvent),
            6 => Some(Self::PrintAttempt),
            7 => Some(Self::SyncAck),
            8 => Some(Self::HeldTickets),
            9 => Some(Self::TerminalState),
            _ => None,
        }
    }
}

/// Everything before the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub store: Store,
    pub kind: PayloadKind,
    /// Version of the payload's own schema, so decoders for N-1 and N-2 can be
    /// selected without guessing.
    pub schema: u16,
    /// Version of the app that wrote it, for diagnosis rather than dispatch.
    pub producer: u16,
    /// Which shop this belongs to. Present so a restored backup or a cloned
    /// tablet image is detected on read instead of silently syncing as somebody
    /// else's terminal.
    pub tenant: u128,
    pub terminal: u128,
    /// Core-owned, persisted, monotonic. Never a wall clock: the core has no
    /// clock, and device clocks are the thing that cannot be trusted.
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than a header, so nothing can be said about them.
    Truncated { needed: usize, found: usize },
    /// Not a frame at all: wrong magic, or reading at the wrong offset.
    BadMagic,
    /// Written by a future version of the frame layout.
    UnsupportedFormat { version: u16 },
    /// A byte that should name a store or payload kind did not.
    UnknownDiscriminant,
    /// Header parsed but the payload is not all there.
    IncompletePayload { declared: usize, found: usize },
    /// Checksum mismatch: the bytes changed after they were written.
    ChecksumMismatch { expected: u32, actual: u32 },
    /// A frame from another shop or another terminal.
    WrongOwner { tenant: u128, terminal: u128 },
}

/// Append one frame to `out`.
pub fn encode(header: &FrameHeader, payload: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FRAME_FORMAT_VERSION.to_le_bytes());
    out.push(header.store as u8);
    out.push(header.kind as u8);
    out.extend_from_slice(&header.schema.to_le_bytes());
    out.extend_from_slice(&header.producer.to_le_bytes());
    out.extend_from_slice(&header.tenant.to_le_bytes());
    out.extend_from_slice(&header.terminal.to_le_bytes());
    out.extend_from_slice(&header.sequence.to_le_bytes());
    let declared = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&declared.to_le_bytes());

    // Checksum covers the header written so far plus the payload, so a corrupted
    // length or terminal id is caught rather than trusted.
    let checksum_at = out.len();
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(payload);

    let mut digest = Crc32::new();
    digest.update(out.get(start..checksum_at).unwrap_or_default());
    digest.update(payload);
    let checksum = digest.finish().to_le_bytes();
    if let Some(slot) = out.get_mut(checksum_at..checksum_at.saturating_add(4)) {
        slot.copy_from_slice(&checksum);
    }
}

/// One frame read back out of a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub header: FrameHeader,
    pub payload: &'a [u8],
    /// Total bytes this frame occupies, header included.
    pub len: usize,
}

/// Decode the frame at the start of `bytes`.
pub fn decode(bytes: &[u8]) -> Result<Frame<'_>, FrameError> {
    if bytes.len() < HEADER_LEN {
        return Err(FrameError::Truncated {
            needed: HEADER_LEN,
            found: bytes.len(),
        });
    }
    let magic = bytes.get(0..4).unwrap_or_default();
    if magic != MAGIC {
        return Err(FrameError::BadMagic);
    }
    let format = read_u16(bytes, 4);
    if format != FRAME_FORMAT_VERSION {
        return Err(FrameError::UnsupportedFormat { version: format });
    }

    let store = bytes
        .get(6)
        .and_then(|b| Store::from_byte(*b))
        .ok_or(FrameError::UnknownDiscriminant)?;
    let kind = bytes
        .get(7)
        .and_then(|b| PayloadKind::from_byte(*b))
        .ok_or(FrameError::UnknownDiscriminant)?;

    let header = FrameHeader {
        store,
        kind,
        schema: read_u16(bytes, 8),
        producer: read_u16(bytes, 10),
        tenant: read_u128(bytes, 12),
        terminal: read_u128(bytes, 28),
        sequence: read_u64(bytes, 44),
    };

    let declared = read_u32(bytes, 52) as usize;
    let stored_checksum = read_u32(bytes, 56);
    let total = HEADER_LEN.saturating_add(declared);
    if bytes.len() < total {
        return Err(FrameError::IncompletePayload {
            declared,
            found: bytes.len().saturating_sub(HEADER_LEN),
        });
    }

    let payload = bytes.get(HEADER_LEN..total).unwrap_or_default();
    let mut digest = Crc32::new();
    digest.update(bytes.get(0..56).unwrap_or_default());
    digest.update(payload);
    let actual = digest.finish();
    if actual != stored_checksum {
        return Err(FrameError::ChecksumMismatch {
            expected: stored_checksum,
            actual,
        });
    }

    Ok(Frame {
        header,
        payload,
        len: total,
    })
}

/// What a scan of a whole log found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan<'a> {
    /// Frames that verified, in order.
    pub frames: Vec<Frame<'a>>,
    /// Bytes covered by those frames. Anything after this is unreadable and the
    /// file should be truncated here before the next append, so a torn tail is
    /// not carried forward forever.
    pub valid_len: usize,
    /// Why the scan stopped, if it stopped early.
    pub stopped_by: Option<FrameError>,
}

impl Scan<'_> {
    /// Whether anything was discarded.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.stopped_by.is_none()
    }
}

/// Read every frame until one fails to verify.
///
/// Stopping at the first bad frame rather than trying to resynchronise is
/// deliberate. Scanning forward for the next magic could land inside a payload
/// that happens to contain those four bytes and manufacture a frame that was
/// never written, which for a ticket log means inventing a sale.
#[must_use]
pub fn scan(bytes: &[u8]) -> Scan<'_> {
    let mut frames = Vec::new();
    let mut offset = 0_usize;

    loop {
        let rest = bytes.get(offset..).unwrap_or_default();
        if rest.is_empty() {
            return Scan {
                frames,
                valid_len: offset,
                stopped_by: None,
            };
        }
        match decode(rest) {
            Ok(frame) => {
                offset = offset.saturating_add(frame.len);
                frames.push(frame);
            }
            Err(error) => {
                return Scan {
                    frames,
                    valid_len: offset,
                    stopped_by: Some(error),
                };
            }
        }
    }
}

/// Reject frames belonging to another shop or terminal.
///
/// Called on read after a restore. A cloned tablet image otherwise syncs as the
/// terminal it was copied from, which duplicates receipt numbers under the same
/// lease epoch: exactly the failure the lease design exists to prevent.
pub fn check_owner(header: &FrameHeader, tenant: u128, terminal: u128) -> Result<(), FrameError> {
    if header.tenant != tenant || header.terminal != terminal {
        return Err(FrameError::WrongOwner {
            tenant: header.tenant,
            terminal: header.terminal,
        });
    }
    Ok(())
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    let slice = bytes.get(at..at.saturating_add(2)).unwrap_or_default();
    u16::from_le_bytes(slice.try_into().unwrap_or([0; 2]))
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    let slice = bytes.get(at..at.saturating_add(4)).unwrap_or_default();
    u32::from_le_bytes(slice.try_into().unwrap_or([0; 4]))
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    let slice = bytes.get(at..at.saturating_add(8)).unwrap_or_default();
    u64::from_le_bytes(slice.try_into().unwrap_or([0; 8]))
}

fn read_u128(bytes: &[u8], at: usize) -> u128 {
    let slice = bytes.get(at..at.saturating_add(16)).unwrap_or_default();
    u128::from_le_bytes(slice.try_into().unwrap_or([0; 16]))
}

/// CRC-32, IEEE polynomial, table driven.
///
/// Hand-rolled rather than pulled in as a dependency: it is twenty lines, it must
/// work identically on three targets, and the table is built at compile time so
/// a 4.7 MB snapshot is checksummed in single-digit milliseconds. A bitwise
/// implementation would be an order of magnitude slower on the cold-start path.
struct Crc32 {
    state: u32,
}

const CRC_TABLE: [u32; 256] = build_crc_table();

// Const evaluation runs at compile time, so an overflow or an out-of-bounds here
// is a build failure rather than a fault at a till. The lint guards against
// runtime panics, which this function cannot produce.
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
const fn build_crc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0_usize;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

impl Crc32 {
    const fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            let index = ((self.state ^ u32::from(*byte)) & 0xFF) as usize;
            let entry = CRC_TABLE.get(index).copied().unwrap_or(0);
            self.state = entry ^ (self.state >> 8);
        }
    }

    const fn finish(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
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

    fn header(sequence: u64) -> FrameHeader {
        FrameHeader {
            store: Store::Critical,
            kind: PayloadKind::SaleCommit,
            schema: 1,
            producer: 1,
            tenant: 42,
            terminal: 7,
            sequence,
        }
    }

    fn log_of(count: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for sequence in 0..count {
            let payload = alloc::format!("sale {sequence}");
            encode(&header(sequence), payload.as_bytes(), &mut out);
        }
        out
    }

    #[test]
    fn known_crc_matches_the_standard() {
        // The canonical CRC-32 check value for "123456789".
        let mut digest = Crc32::new();
        digest.update(b"123456789");
        assert_eq!(digest.finish(), 0xCBF4_3926);
    }

    #[test]
    fn round_trips_a_frame() {
        let mut out = Vec::new();
        encode(&header(9), b"a sale", &mut out);
        assert_eq!(out.len(), HEADER_LEN + 6);

        let frame = decode(&out).unwrap();
        assert_eq!(frame.header, header(9));
        assert_eq!(frame.payload, b"a sale");
        assert_eq!(frame.len, out.len());
    }

    #[test]
    fn round_trips_an_empty_payload() {
        let mut out = Vec::new();
        encode(&header(1), b"", &mut out);
        let frame = decode(&out).unwrap();
        assert_eq!(frame.payload, b"");
    }

    #[test]
    fn reads_a_whole_log_in_order() {
        let bytes = log_of(5);
        let scan = scan(&bytes);
        assert!(scan.is_clean());
        assert_eq!(scan.valid_len, bytes.len());
        let sequences: Vec<u64> = scan.frames.iter().map(|f| f.header.sequence).collect();
        assert_eq!(sequences, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_torn_tail_costs_only_the_last_frame() {
        // A tablet dies partway through appending the fifth sale.
        let bytes = log_of(5);
        let whole_four = scan(&bytes).frames[3];
        let cut = bytes.len() - 3;
        let torn = &bytes[..cut];

        let scan = scan(torn);
        assert_eq!(scan.frames.len(), 4, "the four committed sales survive");
        assert!(!scan.is_clean());
        assert!(scan.valid_len < torn.len(), "the tail is discarded");
        assert_eq!(scan.frames[3], whole_four);
    }

    #[test]
    fn truncating_at_any_byte_never_invents_a_frame() {
        let bytes = log_of(4);
        for cut in 0..bytes.len() {
            let scan = scan(&bytes[..cut]);
            // Every frame returned must be one that was genuinely written, and
            // the valid prefix can never claim more bytes than were given.
            assert!(scan.valid_len <= cut);
            for (position, frame) in scan.frames.iter().enumerate() {
                assert_eq!(frame.header.sequence, position as u64);
            }
        }
    }

    #[test]
    fn detects_a_flipped_bit_in_the_payload() {
        let mut bytes = Vec::new();
        encode(&header(1), b"one thousand taka", &mut bytes);
        bytes[HEADER_LEN + 2] ^= 0b0000_1000;

        match decode(&bytes) {
            Err(FrameError::ChecksumMismatch { .. }) => {}
            other => panic!("silent corruption went undetected: {other:?}"),
        }
    }

    #[test]
    fn detects_a_corrupted_header() {
        let mut bytes = Vec::new();
        encode(&header(1), b"one thousand taka", &mut bytes);
        // Corrupt the terminal id, which a naive checksum over the payload alone
        // would happily accept.
        bytes[30] ^= 0xFF;
        assert!(matches!(
            decode(&bytes),
            Err(FrameError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn rejects_bytes_that_are_not_a_frame() {
        assert_eq!(
            decode(b"short"),
            Err(FrameError::Truncated { needed: HEADER_LEN, found: 5 })
        );
        let junk = vec![0_u8; HEADER_LEN + 4];
        assert_eq!(decode(&junk), Err(FrameError::BadMagic));
    }

    #[test]
    fn rejects_a_future_frame_format() {
        let mut bytes = Vec::new();
        encode(&header(1), b"x", &mut bytes);
        bytes[4..6].copy_from_slice(&99_u16.to_le_bytes());
        assert_eq!(
            decode(&bytes),
            Err(FrameError::UnsupportedFormat { version: 99 })
        );
    }

    #[test]
    fn rejects_a_frame_from_another_terminal() {
        let mut bytes = Vec::new();
        encode(&header(1), b"x", &mut bytes);
        let frame = decode(&bytes).unwrap();

        assert_eq!(check_owner(&frame.header, 42, 7), Ok(()));
        // a restored backup, or a cloned tablet image
        assert_eq!(
            check_owner(&frame.header, 42, 8),
            Err(FrameError::WrongOwner { tenant: 42, terminal: 7 })
        );
    }
}
