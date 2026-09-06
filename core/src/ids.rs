//! Identifiers.
//!
//! Every entity a terminal creates gets a ULID at creation, on the device, with
//! no network involved. Identity never waits for a server; only presentation
//! (the receipt number) does, and that comes from a lease.
//!
//! Stored as a `u128` rather than a string: 16 bytes instead of 26 plus a heap
//! allocation, hashed in one word, and comparable without a memcmp. A 20,000 item
//! catalogue holds tens of thousands of these, and the till is memory-bound
//! before it is CPU-bound.

use alloc::string::String;
use core::fmt;

/// Crockford base32, the ULID alphabet: no I, L, O or U, so a handwritten ID
/// cannot be misread.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ENCODED_LEN: usize = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ulid(u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UlidError {
    WrongLength { len: usize },
    InvalidCharacter { at: usize },
}

impl fmt::Display for UlidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength { len } => write!(f, "a ULID is 26 characters, got {len}"),
            Self::InvalidCharacter { at } => write!(f, "invalid character at position {at}"),
        }
    }
}

impl core::error::Error for UlidError {}

impl Ulid {
    pub const NIL: Self = Self(0);

    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn to_u128(self) -> u128 {
        self.0
    }

    /// Build from a millisecond timestamp and 80 bits of randomness.
    ///
    /// The caller supplies both, because the core has no clock and no entropy
    /// source of its own: those are platform concerns, and injecting them keeps
    /// this crate deterministic and testable.
    #[must_use]
    pub const fn from_parts(timestamp_ms: u64, randomness: u128) -> Self {
        let time_bits = ((timestamp_ms as u128) & 0x0000_FFFF_FFFF_FFFF) << 80;
        let random_bits = randomness & 0x0000_0000_0000_FFFF_FFFF_FFFF_FFFF_FFFF;
        Self(time_bits | random_bits)
    }

    /// Milliseconds since the Unix epoch, as encoded at creation.
    ///
    /// Useful for debugging and for ordering within one terminal. It is not a
    /// trustworthy business clock: device clocks skew, which is why ledger
    /// ordering uses the server sequence instead.
    #[must_use]
    pub const fn timestamp_ms(self) -> u64 {
        (self.0 >> 80) as u64
    }

    #[must_use]
    pub fn encode(self) -> String {
        let mut out = String::with_capacity(ENCODED_LEN);
        // Most significant group first, five bits at a time.
        for position in (0..ENCODED_LEN).rev() {
            let shift = position.saturating_mul(5);
            let index = ((self.0 >> shift) & 0x1F) as usize;
            let symbol = ALPHABET.get(index).copied().unwrap_or(b'0');
            out.push(char::from(symbol));
        }
        out
    }

    pub fn decode(text: &str) -> Result<Self, UlidError> {
        let bytes = text.as_bytes();
        if bytes.len() != ENCODED_LEN {
            return Err(UlidError::WrongLength { len: bytes.len() });
        }
        let mut value: u128 = 0;
        for (position, byte) in bytes.iter().enumerate() {
            let digit = decode_symbol(*byte).ok_or(UlidError::InvalidCharacter { at: position })?;
            value = (value << 5) | u128::from(digit);
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Ulid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

fn decode_symbol(byte: u8) -> Option<u8> {
    // Crockford decoding is case-insensitive and treats I and L as 1, O as 0.
    let upper = byte.to_ascii_uppercase();
    match upper {
        b'I' | b'L' => return Some(1),
        b'O' => return Some(0),
        _ => {}
    }
    ALPHABET
        .iter()
        .position(|candidate| *candidate == upper)
        .and_then(|index| u8::try_from(index).ok())
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use super::*;

    #[test]
    fn round_trips_through_text() {
        let id = Ulid::from_parts(1_788_600_000_000, 0x0123_4567_89AB_CDEF_0123);
        let encoded = id.encode();
        assert_eq!(encoded.len(), 26);
        assert_eq!(Ulid::decode(&encoded), Ok(id));
    }

    #[test]
    fn keeps_the_timestamp_readable() {
        let id = Ulid::from_parts(1_788_600_000_000, 42);
        assert_eq!(id.timestamp_ms(), 1_788_600_000_000);
    }

    #[test]
    fn sorts_by_creation_time() {
        let earlier = Ulid::from_parts(1_788_600_000_000, u128::MAX);
        let later = Ulid::from_parts(1_788_600_000_001, 0);
        assert!(earlier < later, "ULIDs must order by time, not by randomness");
    }

    #[test]
    fn accepts_crockford_ambiguities() {
        // A shopkeeper reading an ID aloud says "oh" for zero and "eye" for one.
        let canonical = Ulid::decode("00000000000000000000000000").unwrap();
        assert_eq!(Ulid::decode("OOOOOOOOOOOOOOOOOOOOOOOOOO"), Ok(canonical));
        let ones = Ulid::decode("0000000000000000000000000I").unwrap();
        assert_eq!(ones, Ulid::from_u128(1));
        assert_eq!(Ulid::decode("0000000000000000000000000l"), Ok(ones));
    }

    #[test]
    fn rejects_malformed_text() {
        assert_eq!(Ulid::decode("TOOSHORT"), Err(UlidError::WrongLength { len: 8 }));
        assert_eq!(
            Ulid::decode("0000000000000000000000000U"),
            Err(UlidError::InvalidCharacter { at: 25 })
        );
    }
}
