//! Who is calling.
//!
//! Before this existed, a request said which shop and which terminal it was in
//! its own body, and the server believed it. Anyone who guessed a pair of
//! identifiers could push sales into a shop's ledger or pull its price list.
//!
//! Now a terminal presents a bearer token issued at enrolment, and the server
//! derives the tenant and terminal from the token rather than from the body.
//! The body's opinion about who it is no longer matters, which is the point: a
//! caller can no longer claim an identity it cannot prove.

use std::fmt;

use sha2::{Digest, Sha256};

/// A terminal's credential, in the clear. Only ever exists twice: once when it
/// is generated, and once in the reply that hands it to the device. It is never
/// stored, logged, or included in an error.
pub struct Token(String);

impl Token {
    /// Mint a fresh token.
    ///
    /// 256 bits from the operating system's generator. There is no counter, no
    /// timestamp and no identifier in it: a token that encodes anything about
    /// its owner tells an attacker something about the shop it belongs to.
    #[must_use]
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);

        let mut text = String::with_capacity(64);
        for byte in bytes {
            use fmt::Write;
            // Ignoring the result is safe: writing to a String cannot fail, and
            // the alternative would be an error path that can never be taken.
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// What actually gets stored.
    #[must_use]
    pub fn hash(&self) -> TokenHash {
        TokenHash::of(&self.0)
    }
}

/// Deliberately opaque. A token that appears in a log file is a token that has
/// to be revoked, and logs are the easiest place for one to end up.
impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(redacted)")
    }
}

/// The stored form of a token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenHash(Vec<u8>);

impl TokenHash {
    /// Hash a presented token so it can be compared with what is stored.
    #[must_use]
    pub fn of(token: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        Self(hasher.finalize().to_vec())
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

/// A short code a person can read off a screen and type into a tablet.
///
/// Roughly forty bits, which would be far too little for a long-lived
/// credential and is fine for one that is single use and expires in minutes. The
/// alphabet omits I, L, O and U, so a code read aloud over a phone cannot be
/// misheard as a different valid code.
pub struct EnrolmentCode(String);

const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LENGTH: usize = 8;

impl EnrolmentCode {
    #[must_use]
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0_u8; CODE_LENGTH];
        rand::rng().fill_bytes(&mut bytes);

        let mut text = String::with_capacity(CODE_LENGTH);
        for byte in bytes {
            // Five bits per character, taken by masking rather than by a
            // remainder. Reducing a random byte modulo an alphabet size that
            // does not divide 256 makes the early symbols likelier than the
            // late ones, which quietly costs entropy. Thirty-two divides 256
            // exactly, so masking is unbiased and says so at a glance.
            let index = usize::from(byte & 0x1F);
            let symbol = CODE_ALPHABET.get(index).copied().unwrap_or(b'0');
            text.push(char::from(symbol));
        }
        Self(text)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    #[must_use]
    pub fn hash(&self) -> TokenHash {
        TokenHash::of(&normalise_code(&self.0))
    }

    /// Hash a code as typed by a person, tolerating the mistakes people make.
    #[must_use]
    pub fn hash_of(typed: &str) -> TokenHash {
        TokenHash::of(&normalise_code(typed))
    }
}

impl fmt::Debug for EnrolmentCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnrolmentCode(redacted)")
    }
}

/// Fold the ways a person mistypes a code onto the value that was issued.
///
/// Case is ignored, spaces and dashes are dropped because people group
/// characters when copying them, and the letters excluded from the alphabet are
/// mapped to the digits they resemble. Someone reading a code aloud says "oh"
/// for zero, and the tablet at the other end should not care.
fn normalise_code(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            'U' => 'V',
            other => other,
        })
        .collect()
}

/// A caller that has proved which terminal it is.
///
/// Handlers take this instead of reading identifiers out of the request body.
/// The type is the enforcement: a handler cannot act on a tenant it was not
/// given, because there is no other way to obtain one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller {
    pub tenant: u128,
    pub terminal: u128,
    /// What this device may be used for. A different question from what the
    /// person signing in at the till may do, which `openpos_core::auth`
    /// answers: a tablet left on a counter is a risk whoever is logged in.
    pub role: Role,
}

/// What a credential is allowed to be used for.
///
/// Two, because two is what the shops this is for actually have. Finer
/// permissions belong to the operator signing in, not to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Role {
    /// Rings sales and syncs. Nothing else.
    #[default]
    Till = 1,
    /// A till, and the back office as well.
    Owner = 2,
}

impl Role {
    /// Read a role back off the wire or out of a column.
    ///
    /// Anything unrecognised reads as `Till`. A role from a newer build than
    /// this one must not be guessed upward: reading an unknown number as the
    /// more powerful role is how a rolling upgrade grants access nobody
    /// granted.
    #[must_use]
    pub fn from_i16(value: i16) -> Self {
        match value {
            2 => Self::Owner,
            _ => Self::Till,
        }
    }

    #[must_use]
    pub fn as_i16(self) -> i16 {
        self as i16
    }

    /// Whether this role covers everything `needed` covers.
    #[must_use]
    pub fn covers(self, needed: Self) -> bool {
        self >= needed
    }
}

/// Pull a bearer token out of an Authorization header.
///
/// Returns `None` for anything malformed rather than trying to be helpful. A
/// server that accepts near-misses teaches clients to send them.
#[must_use]
pub fn bearer(header: Option<&str>) -> Option<&str> {
    let value = header?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token)
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

    #[test]
    fn tokens_are_long_and_never_repeat() {
        let first = Token::generate();
        let second = Token::generate();

        assert_eq!(first.as_str().len(), 64, "256 bits as hex");
        assert_ne!(first.as_str(), second.as_str());
        assert!(first.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hashing_is_stable_and_distinguishes_tokens() {
        let token = Token::generate();
        assert_eq!(token.hash(), TokenHash::of(token.as_str()));
        assert_ne!(token.hash(), Token::generate().hash());
        assert_eq!(token.hash().as_bytes().len(), 32);
    }

    #[test]
    fn a_token_does_not_print_itself() {
        let token = Token::generate();
        let shown = format!("{token:?}");
        assert_eq!(shown, "Token(redacted)");
        assert!(
            !shown.contains(token.as_str()),
            "a token in a log is a token that must be revoked"
        );
    }

    #[test]
    fn enrolment_codes_are_short_and_unambiguous() {
        let code = EnrolmentCode::generate();
        assert_eq!(code.as_str().len(), 8, "short enough to read off a screen");
        assert!(
            code.as_str().chars().all(|c| !"ILOU".contains(c)),
            "the ambiguous letters must not appear: {}",
            code.as_str()
        );
    }

    #[test]
    fn a_mistyped_code_still_matches_what_was_issued() {
        let code = EnrolmentCode::generate();
        let spaced = format!("{} {}", &code.as_str()[..4], &code.as_str()[4..]);

        assert_eq!(EnrolmentCode::hash_of(&spaced), code.hash(), "grouping");
        assert_eq!(
            EnrolmentCode::hash_of(&code.as_str().to_lowercase()),
            code.hash(),
            "case"
        );
        // Somebody reading "0" aloud as "oh", and the typist believing them.
        assert_eq!(EnrolmentCode::hash_of("0O0O0O0O"), EnrolmentCode::hash_of("00000000"));
        assert_eq!(EnrolmentCode::hash_of("1I1L"), EnrolmentCode::hash_of("1111"));
    }

    #[test]
    fn an_enrolment_code_does_not_print_itself() {
        let code = EnrolmentCode::generate();
        assert_eq!(format!("{code:?}"), "EnrolmentCode(redacted)");
    }

    #[test]
    fn reads_a_bearer_header() {
        assert_eq!(bearer(Some("Bearer abc123")), Some("abc123"));
        assert_eq!(bearer(Some("bearer abc123")), Some("abc123"));
    }

    #[test]
    fn refuses_anything_that_is_not_one() {
        assert_eq!(bearer(None), None);
        assert_eq!(bearer(Some("abc123")), None, "no scheme");
        assert_eq!(bearer(Some("Basic abc123")), None, "wrong scheme");
        assert_eq!(bearer(Some("Bearer ")), None, "no token");
        assert_eq!(bearer(Some("Bearer    ")), None, "whitespace is not a token");
    }
}
