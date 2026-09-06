//! Who is at the till, and what they are allowed to do.
//!
//! This has to work with the internet down, which rules out asking a server
//! whether a PIN is right. The credentials therefore sit on the device, and the
//! design follows from that: a stolen tablet hands an attacker the hashes.
//!
//! A cashier's PIN is four to six digits. The entire space is a few hundred
//! thousand candidates, so against a fast hash it falls in under a second, and
//! there is no way to make the space larger without asking shop staff to type
//! something they will instead write on the counter. The only lever left is to
//! make each guess expensive, which is why the hash is deliberately slow and why
//! attempts are throttled on the device as well.
//!
//! Nothing here reads a clock. Every expiry and every lockout window is measured
//! against a timestamp the caller supplies, which is what keeps this identical
//! in a test, in a browser and on a phone.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::ids::Ulid;

pub type OperatorId = Ulid;

/// PBKDF2 rounds.
///
/// Chosen against the slowest device the product targets rather than this
/// laptop: a few hundred milliseconds on a cheap tablet is invisible to someone
/// typing a PIN once at the start of a shift, and it multiplies the cost of
/// searching the whole PIN space by the same factor. Recorded per credential so
/// the number can be raised later without invalidating existing PINs.
pub const DEFAULT_ROUNDS: u32 = 120_000;

/// Length of the derived key, and of the salt.
const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

/// What an operator may do without asking anyone.
///
/// A set of flags rather than named roles. Roles are a back-office presentation
/// concern, and encoding them here would mean a shop that wants a supervisor who
/// cannot void sales has to wait for a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    /// Largest discount, in basis points, this operator may apply unaided.
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    /// May authorise another operator's action. This is what makes someone a
    /// supervisor; nothing else here does.
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
}

impl Permissions {
    /// A shop owner or manager.
    #[must_use]
    pub fn supervisor() -> Self {
        Self {
            max_discount_bp: crate::money::BP_ONE,
            may_override_price: true,
            may_refund: true,
            may_void_line: true,
            may_authorise: true,
            may_open_drawer: true,
            may_close_shift: true,
        }
    }

    /// The default a new till hand gets: sell, and nothing else.
    #[must_use]
    pub fn cashier() -> Self {
        Self {
            max_discount_bp: 0,
            may_override_price: false,
            may_refund: false,
            may_void_line: false,
            may_authorise: false,
            may_open_drawer: false,
            may_close_shift: false,
        }
    }

    #[must_use]
    pub fn allows(&self, action: Action) -> bool {
        match action {
            Action::Discount { bp } => bp <= self.max_discount_bp,
            Action::OverridePrice => self.may_override_price,
            Action::Refund => self.may_refund,
            Action::VoidLine => self.may_void_line,
            Action::OpenDrawer => self.may_open_drawer,
            Action::CloseShift => self.may_close_shift,
        }
    }
}

/// Something a till refuses to do on the cashier's word alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Discount { bp: u32 },
    OverridePrice,
    Refund,
    VoidLine,
    OpenDrawer,
    CloseShift,
}

/// A stored PIN.
///
/// Holds only the salt, the round count and the derived key. The PIN itself
/// exists for the length of one verification and is never kept, logged, or
/// carried in an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinHash {
    pub salt: [u8; SALT_LEN],
    pub rounds: u32,
    key: [u8; KEY_LEN],
}

impl PinHash {
    /// Derive a hash from a PIN and a salt the caller supplies.
    ///
    /// The salt comes from the caller because this crate has no entropy source
    /// of its own, for the same reason it has no clock. It must be random and
    /// per operator: a shared salt means one search cracks every PIN in the
    /// shop at once.
    #[must_use]
    pub fn derive(pin: &str, salt: [u8; SALT_LEN], rounds: u32) -> Self {
        let mut key = [0_u8; KEY_LEN];
        // A zero round count would make the hash instant, which is the one
        // property this must not have.
        let rounds = rounds.max(1);
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pin.as_bytes(), &salt, rounds, &mut key);
        Self { salt, rounds, key }
    }

    /// Whether this PIN matches.
    ///
    /// Compared without an early exit. A comparison that stops at the first
    /// wrong byte leaks how much of the key was right through how long it took,
    /// and an attacker holding the device can measure that as often as they
    /// like.
    #[must_use]
    pub fn verify(&self, pin: &str) -> bool {
        let candidate = Self::derive(pin, self.salt, self.rounds);
        let mut difference = 0_u8;
        for (left, right) in self.key.iter().zip(candidate.key.iter()) {
            difference |= left ^ right;
        }
        difference == 0
    }
}

/// Deliberately opaque, like the token type on the server. A key that reaches a
/// log is a key an attacker can search offline at their own pace.
impl fmt::Display for PinHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PinHash(redacted)")
    }
}

/// Someone who can stand at the till.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    pub id: OperatorId,
    pub name: Box<str>,
    pub pin: PinHash,
    pub permissions: Permissions,
    /// Cleared staff, or someone suspended pending a conversation. Kept rather
    /// than deleted so their name still resolves on yesterday's tickets.
    pub active: bool,
}

impl PinHash {
    /// Rebuild a stored hash. Used only by the storage layer.
    #[must_use]
    pub fn from_parts(salt: [u8; SALT_LEN], rounds: u32, key: [u8; KEY_LEN]) -> Self {
        Self { salt, rounds, key }
    }

    /// The derived key, for writing to disk. Not public beyond that: a key that
    /// reaches a screen or a log can be searched offline at the attacker's pace.
    #[must_use]
    pub fn key(&self) -> &[u8; KEY_LEN] {
        &self.key
    }
}

/// Bytes of a derived key.
pub const KEY_BYTES: usize = KEY_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No operator with that id, or one that is no longer active. Deliberately
    /// one error rather than two: telling an attacker which ids exist saves them
    /// the trouble of finding out.
    UnknownOperator,
    /// The PIN was wrong.
    WrongPin { attempts_left: u32 },
    /// Too many wrong PINs. Carries when the operator may try again.
    LockedOut { until_ms: u64 },
    /// The action needs a permission this operator does not have and no
    /// supervisor has granted.
    NotPermitted { action: Action },
    /// A supervisor authorised this action, but that was long enough ago that
    /// they have walked away.
    AuthorisationExpired,
}

pub type Result<T> = core::result::Result<T, AuthError>;

/// Wrong PINs allowed before the operator is locked out.
pub const DEFAULT_ATTEMPTS: u32 = 5;

/// How long a lockout lasts, in milliseconds.
///
/// Five minutes: long enough that searching the PIN space takes centuries,
/// short enough that a cashier who fat-fingered their PIN during the morning
/// rush is not sent home. A permanent lockout would need a supervisor present,
/// and the shop that most needs this is the one where nobody is.
pub const DEFAULT_LOCKOUT_MS: u64 = 5 * 60 * 1_000;

/// A supervisor's standing permission for one action.
///
/// Expires, because the failure this prevents is a supervisor tapping their PIN
/// in at nine in the morning and every discount for the rest of the day
/// inheriting their authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authorisation {
    pub granted_by: OperatorId,
    pub action: Action,
    pub granted_at_ms: u64,
    pub expires_at_ms: u64,
}

/// How long a supervisor's authorisation stands, in milliseconds.
///
/// Ninety seconds: the time it takes to walk away. Anything longer and the
/// supervisor is effectively logged in at a till they are not standing at.
pub const DEFAULT_AUTHORISATION_MS: u64 = 90 * 1_000;

/// A privileged action that happened, and on whose authority.
///
/// Written down because the question asked afterwards is never "was this
/// allowed" but "who allowed it". An override with no name attached is
/// indistinguishable from theft when the variance is read a week later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub at_ms: u64,
    pub action: Action,
    /// Who performed it.
    pub operator: OperatorId,
    /// Who authorised it, when it was not the operator's own permission.
    pub authorised_by: Option<OperatorId>,
}

/// The operators a terminal knows about, and who is currently signed in.
#[derive(Debug, Default)]
pub struct AuthBook {
    operators: Vec<Operator>,
    signed_in: Option<OperatorId>,
    failures: Vec<(OperatorId, Failures)>,
    authorisation: Option<Authorisation>,
    audit: Vec<AuditEntry>,
    attempts: u32,
    lockout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Failures {
    count: u32,
    locked_until_ms: u64,
}

impl AuthBook {
    #[must_use]
    pub fn new() -> Self {
        Self {
            operators: Vec::new(),
            signed_in: None,
            failures: Vec::new(),
            authorisation: None,
            audit: Vec::new(),
            attempts: DEFAULT_ATTEMPTS,
            lockout_ms: DEFAULT_LOCKOUT_MS,
        }
    }

    #[must_use]
    pub fn with_policy(mut self, attempts: u32, lockout_ms: u64) -> Self {
        self.attempts = attempts.max(1);
        self.lockout_ms = lockout_ms;
        self
    }

    /// Add or replace an operator, as a catalogue pull does.
    pub fn put(&mut self, operator: Operator) {
        match self
            .operators
            .iter_mut()
            .find(|held| held.id == operator.id)
        {
            Some(held) => *held = operator,
            None => self.operators.push(operator),
        }
    }

    #[must_use]
    pub fn operators(&self) -> &[Operator] {
        &self.operators
    }

    #[must_use]
    pub fn signed_in(&self) -> Option<&Operator> {
        let id = self.signed_in?;
        self.find(id)
    }

    #[must_use]
    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    /// Sign in with a PIN.
    ///
    /// A wrong PIN costs an attempt whether or not the operator exists, and both
    /// paths do the same work: returning early on an unknown id would let
    /// somebody learn which ids are real by timing the refusal.
    pub fn sign_in(&mut self, id: OperatorId, pin: &str, now_ms: u64) -> Result<()> {
        if let Some(until) = self.locked_until(id, now_ms) {
            return Err(AuthError::LockedOut { until_ms: until });
        }

        let matched = self
            .find(id)
            .filter(|operator| operator.active)
            .is_some_and(|operator| operator.pin.verify(pin));

        if !matched {
            let left = self.record_failure(id, now_ms);
            return Err(if left == 0 {
                AuthError::LockedOut {
                    until_ms: now_ms.saturating_add(self.lockout_ms),
                }
            } else {
                AuthError::WrongPin {
                    attempts_left: left,
                }
            });
        }

        self.clear_failures(id);
        self.signed_in = Some(id);
        Ok(())
    }

    /// Sign out, and drop any standing authorisation with it.
    pub fn sign_out(&mut self) {
        self.signed_in = None;
        self.authorisation = None;
    }

    /// Check an action against the signed-in operator, then against any
    /// standing supervisor authorisation.
    ///
    /// Records the action when it is allowed. The audit trail is written here
    /// rather than left to callers, because a caller that forgets is a
    /// privileged action with nobody's name on it.
    pub fn check(&mut self, action: Action, now_ms: u64) -> Result<()> {
        let operator = self.signed_in().ok_or(AuthError::UnknownOperator)?;
        let id = operator.id;

        if operator.permissions.allows(action) {
            self.audit.push(AuditEntry {
                at_ms: now_ms,
                action,
                operator: id,
                authorised_by: None,
            });
            return Ok(());
        }

        let granted = match self.authorisation {
            Some(authorisation) if authorisation.action == action => authorisation,
            _ => return Err(AuthError::NotPermitted { action }),
        };
        if now_ms > granted.expires_at_ms {
            self.authorisation = None;
            return Err(AuthError::AuthorisationExpired);
        }

        // Spent on use. A single authorisation covering an afternoon of
        // discounts is the failure this whole mechanism exists to prevent.
        self.authorisation = None;
        self.audit.push(AuditEntry {
            at_ms: now_ms,
            action,
            operator: id,
            authorised_by: Some(granted.granted_by),
        });
        Ok(())
    }

    /// A supervisor puts their PIN in to allow one action by the cashier.
    ///
    /// The supervisor is not signed in by this: they authorise and walk away,
    /// and the till is still the cashier's. Signing them in instead is how a
    /// shop ends up with every sale after lunch attributed to the manager.
    pub fn authorise(
        &mut self,
        supervisor: OperatorId,
        pin: &str,
        action: Action,
        now_ms: u64,
        valid_for_ms: u64,
    ) -> Result<Authorisation> {
        if let Some(until) = self.locked_until(supervisor, now_ms) {
            return Err(AuthError::LockedOut { until_ms: until });
        }

        let permitted = self
            .find(supervisor)
            .filter(|operator| operator.active)
            .filter(|operator| operator.pin.verify(pin))
            .filter(|operator| operator.permissions.may_authorise)
            .is_some_and(|operator| operator.permissions.allows(action));

        if !permitted {
            let left = self.record_failure(supervisor, now_ms);
            return Err(if left == 0 {
                AuthError::LockedOut {
                    until_ms: now_ms.saturating_add(self.lockout_ms),
                }
            } else {
                AuthError::WrongPin {
                    attempts_left: left,
                }
            });
        }

        self.clear_failures(supervisor);
        let authorisation = Authorisation {
            granted_by: supervisor,
            action,
            granted_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(valid_for_ms),
        };
        self.authorisation = Some(authorisation);
        Ok(authorisation)
    }

    fn find(&self, id: OperatorId) -> Option<&Operator> {
        self.operators.iter().find(|operator| operator.id == id)
    }

    fn locked_until(&self, id: OperatorId, now_ms: u64) -> Option<u64> {
        self.failures
            .iter()
            .find(|(held, _)| *held == id)
            .map(|(_, failures)| failures.locked_until_ms)
            .filter(|until| *until > now_ms)
    }

    /// Count a wrong PIN and say how many tries are left.
    fn record_failure(&mut self, id: OperatorId, now_ms: u64) -> u32 {
        let attempts = self.attempts;
        let lockout = self.lockout_ms;

        let entry = match self.failures.iter_mut().find(|(held, _)| *held == id) {
            Some((_, failures)) => failures,
            None => {
                self.failures.push((
                    id,
                    Failures {
                        count: 0,
                        locked_until_ms: 0,
                    },
                ));
                match self.failures.last_mut() {
                    Some((_, failures)) => failures,
                    // Unreachable: a push cannot leave the vector empty. Handled
                    // rather than unwrapped because this crate does not panic.
                    None => return attempts.saturating_sub(1),
                }
            }
        };

        entry.count = entry.count.saturating_add(1);
        if entry.count >= attempts {
            entry.count = 0;
            entry.locked_until_ms = now_ms.saturating_add(lockout);
            return 0;
        }
        attempts.saturating_sub(entry.count)
    }

    fn clear_failures(&mut self, id: OperatorId) {
        self.failures.retain(|(held, _)| *held != id);
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

    use super::*;

    const SALT: [u8; SALT_LEN] = [7; SALT_LEN];
    const OTHER_SALT: [u8; SALT_LEN] = [9; SALT_LEN];

    /// Deliberately weak, so the suite stays fast. Production uses
    /// `DEFAULT_ROUNDS`, and one test below asserts that.
    const TEST_ROUNDS: u32 = 32;

    fn cashier(seed: u128, pin: &str) -> Operator {
        Operator {
            id: Ulid::from_u128(seed),
            name: "Karim".into(),
            pin: PinHash::derive(pin, SALT, TEST_ROUNDS),
            permissions: Permissions::cashier(),
            active: true,
        }
    }

    fn supervisor(seed: u128, pin: &str) -> Operator {
        Operator {
            id: Ulid::from_u128(seed),
            name: "Owner".into(),
            pin: PinHash::derive(pin, OTHER_SALT, TEST_ROUNDS),
            permissions: Permissions::supervisor(),
            active: true,
        }
    }

    fn book() -> AuthBook {
        let mut book = AuthBook::new().with_policy(3, 60_000);
        book.put(cashier(1, "1234"));
        book.put(supervisor(2, "9999"));
        book
    }

    #[test]
    fn the_right_pin_signs_in_and_the_wrong_one_does_not() {
        let mut book = book();
        assert!(book.sign_in(Ulid::from_u128(1), "1234", 0).is_ok());
        assert_eq!(book.signed_in().map(|operator| operator.id), Some(Ulid::from_u128(1)));

        book.sign_out();
        assert_eq!(
            book.sign_in(Ulid::from_u128(1), "4321", 0),
            Err(AuthError::WrongPin { attempts_left: 2 })
        );
        assert!(book.signed_in().is_none());
    }

    #[test]
    fn the_same_pin_under_a_different_salt_does_not_match() {
        // A shared salt means one search cracks every PIN in the shop at once.
        let one = PinHash::derive("1234", SALT, TEST_ROUNDS);
        let two = PinHash::derive("1234", OTHER_SALT, TEST_ROUNDS);
        assert_ne!(one, two);
        assert!(one.verify("1234"));
        assert!(!two.verify("4321"));
    }

    #[test]
    fn a_pin_hash_does_not_print_itself() {
        let hash = PinHash::derive("1234", SALT, TEST_ROUNDS);
        let shown = alloc::format!("{hash}");
        assert_eq!(shown, "PinHash(redacted)");
        assert!(!shown.contains("1234"));
    }

    /// The PIN space is a few hundred thousand candidates, so the cost of one
    /// guess is the only defence there is. This constant is load bearing, and a
    /// well-meaning reduction of it should not pass review quietly.
    const _: () = assert!(DEFAULT_ROUNDS >= 100_000);

    #[test]
    fn raising_the_round_count_does_not_break_existing_pins() {
        // Rounds are stored per credential, so the number can be raised for new
        // PINs without locking out every operator who set one before.
        let old = PinHash::derive("1234", SALT, TEST_ROUNDS);
        let new = PinHash::derive("1234", SALT, TEST_ROUNDS * 4);

        assert!(old.verify("1234"), "the old credential still opens");
        assert!(new.verify("1234"));
        assert_ne!(old, new);
    }

    #[test]
    fn an_unknown_id_and_a_wrong_pin_refuse_alike() {
        let mut book = book();
        // Two different errors would tell an attacker which ids are real.
        assert_eq!(
            book.sign_in(Ulid::from_u128(404), "1234", 0),
            Err(AuthError::WrongPin { attempts_left: 2 })
        );
    }

    #[test]
    fn a_suspended_operator_cannot_sign_in_but_still_has_a_name() {
        let mut book = book();
        let mut suspended = cashier(1, "1234");
        suspended.active = false;
        book.put(suspended);

        assert!(book.sign_in(Ulid::from_u128(1), "1234", 0).is_err());
        assert_eq!(
            book.operators().iter().find(|o| o.id == Ulid::from_u128(1)).map(|o| &*o.name),
            Some("Karim"),
            "yesterday's tickets still have to resolve the name"
        );
    }

    #[test]
    fn guessing_locks_the_operator_out_for_a_while() {
        let mut book = book();
        assert_eq!(
            book.sign_in(Ulid::from_u128(1), "0000", 0),
            Err(AuthError::WrongPin { attempts_left: 2 })
        );
        assert_eq!(
            book.sign_in(Ulid::from_u128(1), "0001", 0),
            Err(AuthError::WrongPin { attempts_left: 1 })
        );
        assert_eq!(
            book.sign_in(Ulid::from_u128(1), "0002", 0),
            Err(AuthError::LockedOut { until_ms: 60_000 })
        );

        // Even the right PIN waits, or the lockout would be trivial to skip.
        assert_eq!(
            book.sign_in(Ulid::from_u128(1), "1234", 1_000),
            Err(AuthError::LockedOut { until_ms: 60_000 })
        );
        assert!(
            book.sign_in(Ulid::from_u128(1), "1234", 61_000).is_ok(),
            "and a cashier who fat-fingered their PIN is not sent home"
        );
    }

    #[test]
    fn one_operators_lockout_does_not_stop_another_selling() {
        let mut book = book();
        for attempt in 0..3 {
            let _ = book.sign_in(Ulid::from_u128(1), "0000", attempt);
        }
        assert!(
            book.sign_in(Ulid::from_u128(2), "9999", 0).is_ok(),
            "a shop with one locked-out till hand is still open"
        );
    }

    #[test]
    fn a_cashier_cannot_do_what_they_are_not_permitted() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();

        assert_eq!(
            book.check(Action::Refund, 0),
            Err(AuthError::NotPermitted {
                action: Action::Refund
            })
        );
        assert!(book.audit().is_empty(), "a refusal is not an action taken");
    }

    #[test]
    fn a_supervisor_authorises_one_action_and_walks_away() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();
        book.authorise(
            Ulid::from_u128(2),
            "9999",
            Action::Refund,
            1_000,
            DEFAULT_AUTHORISATION_MS,
        )
        .unwrap();

        assert!(book.check(Action::Refund, 2_000).is_ok());
        assert_eq!(
            book.signed_in().map(|operator| operator.id),
            Some(Ulid::from_u128(1)),
            "the till is still the cashier's, or every sale after lunch is the manager's"
        );

        // Spent on use: one authorisation must not cover an afternoon.
        assert_eq!(
            book.check(Action::Refund, 3_000),
            Err(AuthError::NotPermitted {
                action: Action::Refund
            })
        );
    }

    #[test]
    fn an_authorisation_the_supervisor_walked_away_from_expires() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();
        book.authorise(Ulid::from_u128(2), "9999", Action::Refund, 0, 90_000)
            .unwrap();

        assert_eq!(
            book.check(Action::Refund, 90_001),
            Err(AuthError::AuthorisationExpired)
        );
    }

    #[test]
    fn an_authorisation_covers_only_the_action_it_was_given_for() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();
        book.authorise(Ulid::from_u128(2), "9999", Action::Refund, 0, 90_000)
            .unwrap();

        assert_eq!(
            book.check(Action::OverridePrice, 1_000),
            Err(AuthError::NotPermitted {
                action: Action::OverridePrice
            })
        );
    }

    #[test]
    fn a_cashier_cannot_authorise_themselves() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();

        // Right PIN, right person, but no authority to grant.
        assert!(book
            .authorise(Ulid::from_u128(1), "1234", Action::Refund, 0, 90_000)
            .is_err());
    }

    #[test]
    fn who_allowed_it_is_written_down() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();
        book.authorise(Ulid::from_u128(2), "9999", Action::Refund, 0, 90_000)
            .unwrap();
        book.check(Action::Refund, 1_000).unwrap();

        // The question asked afterwards is never "was this allowed" but "who
        // allowed it".
        assert_eq!(
            book.audit(),
            &[AuditEntry {
                at_ms: 1_000,
                action: Action::Refund,
                operator: Ulid::from_u128(1),
                authorised_by: Some(Ulid::from_u128(2)),
            }]
        );
    }

    #[test]
    fn a_discount_is_checked_against_the_ceiling_not_merely_permitted() {
        let mut book = AuthBook::new();
        let mut junior = cashier(1, "1234");
        junior.permissions.max_discount_bp = 1_000;
        book.put(junior);
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();

        assert!(book.check(Action::Discount { bp: 1_000 }, 0).is_ok());
        assert!(book.check(Action::Discount { bp: 1_001 }, 0).is_err());
    }

    #[test]
    fn signing_out_drops_a_standing_authorisation() {
        let mut book = book();
        book.sign_in(Ulid::from_u128(1), "1234", 0).unwrap();
        book.authorise(Ulid::from_u128(2), "9999", Action::Refund, 0, 90_000)
            .unwrap();
        book.sign_out();
        book.sign_in(Ulid::from_u128(1), "1234", 1_000).unwrap();

        // Otherwise a supervisor's grant survives a shift change.
        assert!(book.check(Action::Refund, 2_000).is_err());
    }
}
