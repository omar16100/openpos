//! Deciding what to sync next, and when to try again.
//!
//! The platform performs the request; this decides whether there should be one.
//! That split is the same one the rest of the crate keeps, and it matters more
//! here than anywhere: retry policy written in Dart and again in JavaScript is
//! retry policy that differs, and the difference only shows up as a shop whose
//! sales sat on a tablet for a day because one platform gave up quietly.
//!
//! Nothing here reads a clock or sleeps. It is handed the time and answers with
//! what to do now and how long to wait if the answer is nothing, so the same
//! decisions are testable without a network and identical on every target.
//!
//! # What it will not do
//!
//! It will not stop trying. A till with sales it cannot deliver keeps asking,
//! slower and slower, up to a ceiling, forever. Giving up is the one outcome
//! that is never right: the shop cannot tell that it happened, and the sales are
//! on a tablet nobody has backed up.

use crate::lease::DEFAULT_RENEWAL_THRESHOLD;

/// How long to wait after the first failure.
///
/// A second. Long enough not to hammer a server that is restarting, short
/// enough that a shop whose internet blinked does not notice.
pub const BASE_BACKOFF_MS: u64 = 1_000;

/// Longest wait between attempts.
///
/// Five minutes. A shop back online after an outage should not wait an hour to
/// discover it, and unbounded doubling reaches an hour in nine failures.
pub const MAX_BACKOFF_MS: u64 = 5 * 60 * 1_000;

/// How long to wait between successful rounds when there is nothing to do.
pub const IDLE_MS: u64 = 30 * 1_000;

/// Largest batch of sales to push at once.
///
/// Small on purpose. A shop draining a day's trading over mobile data wants each
/// attempt to be one a bad connection can finish; a batch that always fails
/// halfway delivers nothing at all, however many sales it carries.
pub const PUSH_BATCH: usize = 25;

/// Catalogue changes to ask for in one pull.
pub const PULL_LIMIT: u32 = 500;

/// What the platform should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Deliver up to this many sales, oldest first.
    Push { limit: usize },
    /// Ask for catalogue changes after this cursor.
    Pull { cursor: u64, limit: u32 },
    /// Ask for more receipt numbers.
    RenewLease { count: u32 },
    /// Ask for the shop's own details, for the top of a receipt.
    FetchShop,
    /// Ask who may stand at this till.
    FetchOperators,
    /// Drawers counted and closed that the shop has not been told about.
    PushShifts,
    /// Nothing to do. Come back in this many milliseconds.
    Wait { for_ms: u64 },
}

/// What the till knows about itself when it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    pub unsynced_sales: usize,
    /// Drawers counted and closed and not yet sent.
    pub unsent_shifts: usize,
    pub cursor: u64,
    pub receipt_numbers_left: u64,
    /// True when the last pull said more was waiting.
    pub more_to_pull: bool,
    /// False when the platform knows there is no network. A till that knows it
    /// is offline should not spend battery discovering that.
    pub online: bool,
}

/// How many receipt numbers to ask for.
///
/// A block that covers a long offline day, rather than one sized to what the
/// till has just used. Renewal happens when it can, not when it must.
pub const LEASE_BLOCK: u32 = 500;

/// How often to re-ask for things that change rarely: the shop's own details
/// and the list of people.
///
/// Ten minutes. Often enough that a cashier added this morning can sign in
/// before lunch, rare enough that it is not three requests every half minute
/// for data nobody touched.
pub const SETTINGS_REFRESH_MS: u64 = 10 * 60 * 1_000;

/// Whether something asked for at `last` is due again.
///
/// Never asked counts as due, which is what makes a freshly enrolled till fetch
/// anything at all.
const fn due(last: Option<u64>, now_ms: u64, every: u64) -> bool {
    match last {
        None => true,
        Some(at) => now_ms.saturating_sub(at) >= every,
    }
}

/// Decides what to do next, and how long to wait when the answer is nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Driver {
    failures: u32,
    /// When the next attempt may happen. Compared against the caller's clock,
    /// never against one of this type's own.
    not_before_ms: u64,
    /// When the shop's details and its people were last asked for.
    ///
    /// Recorded on a successful exchange rather than inferred from what the
    /// till ended up holding. A shop that has not added anybody yet answers
    /// with an empty list, and a driver that read that as "still does not know"
    /// would ask again immediately, forever, for as long as the shop had one
    /// person working in it.
    shop_at_ms: Option<u64>,
    operators_at_ms: Option<u64>,
    /// When the catalogue was last asked for, and whether it ever has been.
    ///
    /// Held because "more is waiting" is an answer only a previous pull can
    /// give. A driver that pulled only when told more was waiting would never
    /// pull at all on a freshly enrolled till, which is the one till that has
    /// nothing and needs everything.
    pulled_at_ms: Option<u64>,
}

impl Driver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consecutive failures since the last success.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// What to do at `now_ms`.
    ///
    /// The order is deliberate. Sales first, because they are the only thing
    /// here that exists nowhere else: a catalogue can be pulled again tomorrow
    /// and a lease can be asked for again, but a sale on a tablet that dies is
    /// gone. Then numbers, because running out of them degrades every later
    /// sale. The catalogue is last: a stale price is a real problem and a
    /// smaller one than either.
    #[must_use]
    pub fn next(&self, situation: &Situation, now_ms: u64) -> Next {
        if !situation.online {
            return Next::Wait {
                for_ms: self.backoff_ms(),
            };
        }
        if now_ms < self.not_before_ms {
            return Next::Wait {
                for_ms: self.not_before_ms.saturating_sub(now_ms),
            };
        }

        if situation.unsynced_sales > 0 {
            return Next::Push { limit: PUSH_BATCH };
        }
        if situation.receipt_numbers_left <= DEFAULT_RENEWAL_THRESHOLD {
            return Next::RenewLease { count: LEASE_BLOCK };
        }
        // Before the catalogue and after the numbers, for the same reason sales
        // come first: a counted drawer exists nowhere else, and a price can be
        // fetched again tomorrow.
        if situation.unsent_shifts > 0 {
            return Next::PushShifts;
        }
        // Before the catalogue. A till that can sell but prints receipts with
        // no shop on them is worse than one that waits a moment, and a customer
        // cannot take a nameless receipt back to anybody. Likewise a till with a
        // catalogue and nobody able to sign in can ring nothing needing a
        // permission, which is every refund and every drawer opening.
        if due(self.shop_at_ms, now_ms, SETTINGS_REFRESH_MS) {
            return Next::FetchShop;
        }
        if due(self.operators_at_ms, now_ms, SETTINGS_REFRESH_MS) {
            return Next::FetchOperators;
        }
        // Pull when the server said there was more, when this till has never
        // asked, or when it last asked long enough ago that a price could have
        // changed. The server does not push, so a till that stops asking stops
        // learning, and the first thing it fails to learn is that a price went
        // up this morning.
        if situation.more_to_pull || due(self.pulled_at_ms, now_ms, IDLE_MS) {
            return Next::Pull {
                cursor: situation.cursor,
                limit: PULL_LIMIT,
            };
        }

        Next::Wait {
            for_ms: self
                .pulled_at_ms
                .map_or(IDLE_MS, |at| IDLE_MS.saturating_sub(now_ms.saturating_sub(at))),
        }
    }

    /// Record that an attempt worked.
    pub fn succeeded(&mut self, now_ms: u64) {
        self.failures = 0;
        self.not_before_ms = now_ms;
    }

    /// Record that the shop's details were asked for, whatever came back.
    pub fn fetched_shop(&mut self, now_ms: u64) {
        self.shop_at_ms = Some(now_ms);
    }

    /// Record that the people were asked for, whatever came back.
    pub fn fetched_operators(&mut self, now_ms: u64) {
        self.operators_at_ms = Some(now_ms);
    }

    /// Record that the catalogue was asked for, whatever the answer was.
    ///
    /// Separate from `succeeded` because a pull that returns nothing is still a
    /// pull: a till that only counted pulls which changed something would ask
    /// again immediately, forever, in a shop whose prices are settled.
    pub fn pulled(&mut self, now_ms: u64) {
        self.pulled_at_ms = Some(now_ms);
    }

    /// Record that an attempt failed, and back off.
    ///
    /// The count saturates rather than wrapping, which matters: a till offline
    /// for a week accumulates a great many failures, and a counter that wrapped
    /// would take the wait back down to a second and hammer a server that is
    /// still not there.
    pub fn failed(&mut self, now_ms: u64) {
        self.failures = self.failures.saturating_add(1);
        self.not_before_ms = now_ms.saturating_add(self.backoff_ms());
    }

    /// How long the current failure count says to wait.
    ///
    /// Doubling, to a ceiling. No jitter, and that is a decision rather than an
    /// omission: jitter exists to stop a thousand clients retrying in lockstep,
    /// and these clients are a handful of tills per shop that did not start
    /// together. Adding randomness would mean this crate needs an entropy
    /// source, which it deliberately does not have.
    #[must_use]
    pub fn backoff_ms(&self) -> u64 {
        if self.failures == 0 {
            return 0;
        }
        let doublings = self.failures.saturating_sub(1).min(20);
        BASE_BACKOFF_MS
            .saturating_mul(1_u64.checked_shl(doublings).unwrap_or(u64::MAX))
            .min(MAX_BACKOFF_MS)
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

    /// A driver that has already asked for the things asked for once.
    fn settled() -> Driver {
        let mut driver = Driver::new();
        driver.fetched_shop(0);
        driver.fetched_operators(0);
        driver
    }

    fn idle() -> Situation {
        Situation {
            unsent_shifts: 0,
            unsynced_sales: 0,
            cursor: 7,
            receipt_numbers_left: 400,
            more_to_pull: false,
            online: true,
        }
    }

    #[test]
    fn sales_go_before_anything_else() {
        let driver = settled();
        let situation = Situation {
            unsynced_sales: 3,
            receipt_numbers_left: 0,
            more_to_pull: true,
            ..idle()
        };

        // Numbers and the catalogue can be asked for again tomorrow. A sale on a
        // tablet that dies is gone.
        assert_eq!(
            driver.next(&situation, 0),
            Next::Push { limit: PUSH_BATCH }
        );
    }

    #[test]
    fn numbers_go_before_the_catalogue() {
        let driver = settled();
        let situation = Situation {
            receipt_numbers_left: 10,
            more_to_pull: true,
            ..idle()
        };

        // Running out of numbers degrades every later sale; a stale price is a
        // smaller problem than an unnumbered receipt.
        assert_eq!(
            driver.next(&situation, 0),
            Next::RenewLease { count: LEASE_BLOCK }
        );
    }

    #[test]
    fn a_till_learns_what_shop_it_is_before_it_learns_what_it_sells() {
        let driver = Driver::new();
        // A receipt with no shop on it is one a customer cannot take back to
        // anybody, and a catalogue arriving first would let it sell anyway.
        assert_eq!(driver.next(&idle(), 0), Next::FetchShop);
    }

    #[test]
    fn a_till_learns_who_may_use_it_before_it_learns_what_it_sells() {
        let mut driver = Driver::new();
        driver.fetched_shop(0);
        // A catalogue arriving first would let it ring sales that nobody is
        // signed in for, and every refund would be refused with no way to fix
        // it from the counter.
        assert_eq!(driver.next(&idle(), 0), Next::FetchOperators);
    }

    #[test]
    fn a_shop_with_nobody_in_it_yet_does_not_ask_forever() {
        let mut driver = Driver::new();
        driver.fetched_shop(0);
        // The reply was an empty list, because nobody has been added. A driver
        // that read that as "still does not know" would ask again immediately
        // for as long as the shop had one person working in it.
        driver.fetched_operators(0);
        driver.succeeded(0);

        assert_ne!(driver.next(&idle(), 1_000), Next::FetchOperators);

        // And it does ask again later, so a cashier added this morning can sign
        // in before lunch. The shop falls due at the same moment and is asked
        // for first, as it is on a cold start.
        let later = SETTINGS_REFRESH_MS;
        assert_eq!(driver.next(&idle(), later), Next::FetchShop);
        driver.fetched_shop(later);
        assert_eq!(driver.next(&idle(), later), Next::FetchOperators);
    }

    #[test]
    fn a_till_that_has_never_pulled_pulls_before_it_waits() {
        let driver = settled();
        // A freshly enrolled till has nothing and needs everything, and nobody
        // has told it that more is waiting because nothing has asked yet.
        assert_eq!(
            driver.next(&idle(), 0),
            Next::Pull {
                cursor: 7,
                limit: PULL_LIMIT
            }
        );
    }

    #[test]
    fn an_idle_till_waits_and_then_asks_again() {
        let mut driver = settled();
        driver.pulled(0);
        driver.succeeded(0);

        // The server does not push, so a till that stops asking stops learning,
        // and the first thing it fails to learn is a price that went up.
        assert_eq!(driver.next(&idle(), 1_000), Next::Wait { for_ms: IDLE_MS - 1_000 });
        assert_eq!(
            driver.next(&idle(), IDLE_MS),
            Next::Pull {
                cursor: 7,
                limit: PULL_LIMIT
            }
        );
    }

    #[test]
    fn a_till_that_knows_it_is_offline_does_not_try() {
        let driver = settled();
        let situation = Situation {
            unsynced_sales: 9,
            online: false,
            ..idle()
        };
        assert!(matches!(driver.next(&situation, 0), Next::Wait { .. }));
    }

    #[test]
    fn failures_back_off_by_doubling_to_a_ceiling() {
        let mut driver = Driver::new();
        assert_eq!(driver.backoff_ms(), 0, "nothing has failed yet");

        driver.failed(0);
        assert_eq!(driver.backoff_ms(), 1_000);
        driver.failed(0);
        assert_eq!(driver.backoff_ms(), 2_000);
        driver.failed(0);
        assert_eq!(driver.backoff_ms(), 4_000);

        for _ in 0..30 {
            driver.failed(0);
        }
        assert_eq!(
            driver.backoff_ms(),
            MAX_BACKOFF_MS,
            "a shop back online should not wait an hour to find out"
        );
    }

    #[test]
    fn a_week_of_failures_does_not_wrap_the_wait_back_to_a_second() {
        let mut driver = Driver::new();
        // A till offline for a week accumulates a great many failures. A counter
        // that wrapped would take the wait back to a second and hammer a server
        // that is still not there.
        for _ in 0..100_000 {
            driver.failed(0);
        }
        assert_eq!(driver.backoff_ms(), MAX_BACKOFF_MS);
        assert!(driver.failures() > 0);
    }

    #[test]
    fn a_failure_holds_the_next_attempt_off_until_the_backoff_has_passed() {
        let mut driver = settled();
        let situation = Situation {
            unsynced_sales: 1,
            ..idle()
        };

        driver.failed(10_000);
        assert_eq!(
            driver.next(&situation, 10_500),
            Next::Wait { for_ms: 500 },
            "and it says how long, so a caller need not work it out"
        );
        assert_eq!(
            driver.next(&situation, 11_000),
            Next::Push { limit: PUSH_BATCH }
        );
    }

    #[test]
    fn one_success_clears_the_backoff_entirely() {
        let mut driver = settled();
        for _ in 0..10 {
            driver.failed(0);
        }
        driver.succeeded(50_000);

        assert_eq!(driver.failures(), 0);
        assert_eq!(driver.backoff_ms(), 0);
        assert_eq!(
            driver.next(
                &Situation {
                    unsynced_sales: 1,
                    ..idle()
                },
                50_000
            ),
            Next::Push { limit: PUSH_BATCH }
        );
    }

    #[test]
    fn it_never_decides_to_give_up() {
        let mut driver = settled();
        for _ in 0..1_000 {
            driver.failed(0);
        }
        // There is no variant for it, and there should not be: a shop cannot
        // tell that a till stopped trying, and the sales are on a tablet nobody
        // has backed up.
        let waited = driver.next(
            &Situation {
                unsynced_sales: 40,
                ..idle()
            },
            MAX_BACKOFF_MS.saturating_add(1),
        );
        assert_eq!(waited, Next::Push { limit: PUSH_BATCH });
    }

    #[test]
    fn a_pull_carries_the_cursor_the_till_actually_holds() {
        let mut driver = settled();
        driver.pulled(0);
        let situation = Situation {
            cursor: 91,
            more_to_pull: true,
            ..idle()
        };
        assert_eq!(
            driver.next(&situation, 0),
            Next::Pull {
                cursor: 91,
                limit: PULL_LIMIT
            }
        );
    }
}
