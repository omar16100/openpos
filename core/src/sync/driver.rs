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
    /// Ask who the shop lets buy on account.
    FetchCustomers,
    /// Ask where the shop's settings stand, as one number.
    CheckSettings,
    /// Ask what each of them owes.
    FetchBalances,
    /// Ask what the shop believes is on the shelves, for a window of the
    /// catalogue.
    ///
    /// A window rather than everything, because the answer is one figure per
    /// item and a shop with a long catalogue would be asking for a megabyte
    /// every few minutes to enforce a rule about a dozen of them. Successive
    /// asks move along, so every item is refreshed within a lap.
    FetchStock { from: usize, limit: usize },
    /// Take a fresh credential, before the one in hand expires.
    RenewCredential,
    /// Drawers counted and closed that the shop has not been told about.
    PushShifts,
    /// Privileged actions this device allowed that the shop has not been told
    /// about.
    PushAllowed,
    /// Items this till wrote down at the counter that the shop has not got.
    PushItems,
    /// Say what the drawer standing open right now holds.
    ReportDrawer,
    /// Nothing to do. Come back in this many milliseconds.
    Wait { for_ms: u64 },
}

/// What the till knows about itself when it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    pub unsynced_sales: usize,
    /// Drawers counted and closed and not yet sent.
    pub unsent_shifts: usize,
    /// Privileged actions allowed and not yet sent.
    pub unsent_allowed: usize,
    /// Items this till wrote down and the shop has not got.
    pub unsent_items: usize,
    /// True while a drawer is open on this till.
    pub drawer_open: bool,
    /// When this device's credential was taken, by its own clock, and how long
    /// the shop says one lasts. Zero for a device enrolled by a build that did
    /// not write it down, which renews at the next opportunity.
    pub credential_taken_at_ms: u64,
    pub credential_lifetime_ms: u64,
    /// False before a device has a credential at all, when there is nothing to
    /// renew and nothing to sync.
    pub enrolled: bool,
    /// True when the shop has written anybody down as buying on account. A shop
    /// that has not has no balances to ask for.
    pub has_customers: bool,
    /// True when this shop has asked its tills to do something about the shelf.
    ///
    /// Only then is stock worth fetching: it is one figure per item and the
    /// catalogue's own copy never moves, so a shop that does nothing with it
    /// should not pay for it every few minutes.
    pub watches_stock: bool,
    /// How many items this till holds, so the window can move along and wrap.
    pub items: usize,
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

/// How often to tell the shop what an open drawer holds.
///
/// Two minutes. A drawer left open overnight and wiped in the morning used to
/// take its whole summary with it; this bounds that loss to the last couple of
/// minutes of a shift nobody closed. Rare enough to be one small request while a
/// till is otherwise idle, and it is the last thing tried before waiting.
pub const DRAWER_REPORT_MS: u64 = 2 * 60 * 1_000;

/// How long a device runs on one credential before taking a fresh one.
///
/// Thirty days, or a third of whatever the shop says one lasts, whichever is
/// sooner. The shop's own policy is a year, so a till that renews monthly is
/// eleven months clear of the moment it would otherwise stop working, and a
/// shop that shortens the policy is followed without a new build.
///
/// This is the step that had no client at all: the route existed, the reply
/// carried the lifetime so a device could decide, and nothing ever asked. Every
/// device would have stopped one year after it was enrolled.
pub const RENEW_CREDENTIAL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// How often to ask what people owe.
///
/// More often than the list of names, because a name is written down once and a
/// balance moves every time somebody takes a bag of rice, possibly at another
/// till. Five minutes is close enough that a cashier answering "how much do I
/// owe" across the counter is not badly wrong, and far enough apart that it is
/// not a request a minute for a number nobody asked for.
pub const BALANCES_REFRESH_MS: u64 = 5 * 60 * 1_000;

/// How often a till asks what the shelves hold.
///
/// Only for a shop that has asked its tills to warn or refuse. Between asks the
/// figure moves for this terminal's own sales, which it applies itself; what it
/// misses is another till's, and five minutes of another till is the error a
/// shop accepts when it turns the rule on.
pub const STOCK_REFRESH_MS: u64 = 5 * 60 * 1_000;

/// How many items one ask covers.
///
/// The server answers one query per item, so this is a bound on both sides. A
/// shop of two hundred lines is refreshed whole every five minutes; one of two
/// thousand takes fifty, which is worth saying out loud rather than discovering.
pub const STOCK_WINDOW: usize = 200;

/// Whether the credential in hand is old enough to replace.
///
/// A device that never wrote down when it took its credential renews at the
/// next opportunity: one request, and then it knows.
fn credential_due(situation: &Situation, now_ms: u64) -> bool {
    if situation.credential_taken_at_ms == 0 {
        return true;
    }
    let window = if situation.credential_lifetime_ms == 0 {
        RENEW_CREDENTIAL_MS
    } else {
        (situation.credential_lifetime_ms / 3).min(RENEW_CREDENTIAL_MS)
    };
    now_ms.saturating_sub(situation.credential_taken_at_ms) >= window
}

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
    /// When an open drawer was last reported.
    drawer_at_ms: Option<u64>,
    /// When the people who buy on account were last asked for.
    customers_at_ms: Option<u64>,
    /// When what they owe was last asked for.
    balances_at_ms: Option<u64>,
    /// When the settings counter was last asked for, and where it stood.
    settings_at_ms: Option<u64>,
    settings_seq: Option<u64>,
    /// When the shelves were last asked about, and where in the catalogue the
    /// next ask starts.
    stock_at_ms: Option<u64>,
    stock_from: usize,
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

        // Before anything else that needs a credential, because everything
        // does. A device renewing while its credential still works costs one
        // request; one that waits until it stops is a shop with a tablet nobody
        // can enrol without the owner's device in front of them.
        if situation.enrolled && credential_due(situation, now_ms) {
            return Next::RenewCredential;
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
        // Beside the drawers and for the same reason: who allowed what exists
        // nowhere but this device until the shop has it, and a tablet that is
        // lost or wiped takes it with it. A price can be fetched again
        // tomorrow; this cannot be reconstructed by anybody.
        if situation.unsent_allowed > 0 {
            return Next::PushAllowed;
        }
        // Before the catalogue and before the lists, because the sales already
        // sent name these: a shop reading its own takings should be able to
        // look up what was sold. Small, and there are only ever a handful.
        if situation.unsent_items > 0 {
            return Next::PushItems;
        }
        // Cheap, and often, because it is what makes the three expensive ones
        // rare. A cashier being locked out in a hurry reaches a till in the time
        // this takes rather than in the ten minutes the lists take.
        if due(self.settings_at_ms, now_ms, IDLE_MS) {
            return Next::CheckSettings;
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
        // With the people, and for the same reason: a sale on account written
        // with the line down needs the name on the device already.
        if due(self.customers_at_ms, now_ms, SETTINGS_REFRESH_MS) {
            return Next::FetchCustomers;
        }
        // And what they owe, more often, but only where the shop has written
        // anybody down: a shop that sells on account against typed names has no
        // balances to ask for.
        if situation.has_customers && due(self.balances_at_ms, now_ms, BALANCES_REFRESH_MS) {
            return Next::FetchBalances;
        }
        // What the shelves hold, for the shop that has asked its tills to do
        // something about it. After the records and before the catalogue: a
        // stale figure warns or refuses wrongly, which is a queue waiting, and
        // a stale price is a price.
        if situation.watches_stock
            && situation.items > 0
            && due(self.stock_at_ms, now_ms, STOCK_REFRESH_MS)
        {
            return Next::FetchStock {
                from: self.stock_from.min(situation.items.saturating_sub(1)),
                limit: STOCK_WINDOW,
            };
        }
        // Pull when the server said there was more, when this till has never
        // asked, or when it last asked long enough ago that a price could have
        // changed. The server does not push, so a till that stops asking stops
        // learning, and the first thing it fails to learn is that a price went
        // up this morning.
        // After everything that is a record, because this is a position: the
        // counted drawer is what the shop keeps, and this only bounds how much
        // of an open one is lost with a device.
        if situation.drawer_open && due(self.drawer_at_ms, now_ms, DRAWER_REPORT_MS) {
            return Next::ReportDrawer;
        }
        if situation.more_to_pull || due(self.pulled_at_ms, now_ms, IDLE_MS) {
            return Next::Pull {
                cursor: situation.cursor,
                limit: PULL_LIMIT,
            };
        }

        Next::Wait {
            for_ms: self.pulled_at_ms.map_or(IDLE_MS, |at| {
                IDLE_MS.saturating_sub(now_ms.saturating_sub(at))
            }),
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

    /// Record that the account customers were asked for, whatever came back.
    pub fn fetched_customers(&mut self, now_ms: u64) {
        self.customers_at_ms = Some(now_ms);
    }

    /// Record where the shop's settings counter stands.
    ///
    /// A number that has moved means the people, the shop or the account
    /// customers changed, so all three are due again: a till that has to
    /// re-read one may as well re-read all of them, and this is the only moment
    /// it can know. A till that has never asked takes the first answer as its
    /// mark rather than as a change, or every till would re-read everything the
    /// first time it looked.
    pub fn settings_seq(&mut self, seq: u64, now_ms: u64) {
        self.settings_at_ms = Some(now_ms);
        let moved = self.settings_seq.is_some_and(|held| held != seq);
        self.settings_seq = Some(seq);
        if moved {
            self.shop_at_ms = None;
            self.operators_at_ms = None;
            self.customers_at_ms = None;
        }
    }

    /// Record that the balances were asked for, whatever came back.
    pub fn fetched_balances(&mut self, now_ms: u64) {
        self.balances_at_ms = Some(now_ms);
    }

    /// Record that a window of stock was asked for, and move along.
    ///
    /// The window advances whatever came back, and wraps at the end of the
    /// catalogue. A window that could not be fetched is one lap behind rather
    /// than blocking the ones after it.
    pub fn fetched_stock(&mut self, now_ms: u64, items: usize) {
        self.stock_at_ms = Some(now_ms);
        let next = self.stock_from.saturating_add(STOCK_WINDOW);
        self.stock_from = if next >= items { 0 } else { next };
    }

    /// Record that the open drawer was reported.
    pub fn reported_drawer(&mut self, now_ms: u64) {
        self.drawer_at_ms = Some(now_ms);
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
        driver.fetched_customers(0);
        // A till that has asked once and been told nothing has changed since.
        driver.settings_seq(1, 0);
        driver
    }

    fn idle() -> Situation {
        Situation {
            unsent_shifts: 0,
            unsent_allowed: 0,
            drawer_open: false,
            has_customers: false,
            unsent_items: 0,
            // A shop that does nothing about the shelf, which is every shop
            // until one says otherwise. The tests that care say so themselves.
            watches_stock: false,
            items: 20,
            enrolled: true,
            // A credential taken a moment ago, so nothing here is about to
            // renew: the tests that care about renewal say so themselves.
            credential_taken_at_ms: 1,
            credential_lifetime_ms: 0,
            unsynced_sales: 0,
            cursor: 7,
            receipt_numbers_left: 400,
            more_to_pull: false,
            online: true,
        }
    }

    /// A shop that does nothing about the shelf is not asked about it.
    ///
    /// One figure per item, every few minutes, to enforce a rule nobody set: a
    /// till doing that is spending a shop's line on nothing.
    #[test]
    fn a_shop_that_ignores_the_shelf_is_never_asked_what_is_on_it() {
        let mut driver = settled();
        let mut asked = 0;
        for now_ms in (0..40 * 60 * 1_000).step_by(60_000) {
            if matches!(driver.next(&idle(), now_ms), Next::FetchStock { .. }) {
                asked += 1;
            }
            driver.succeeded(now_ms);
        }
        assert_eq!(asked, 0, "nothing asked for a rule nobody set");
    }

    /// A shop that has is, and the window moves along the catalogue.
    #[test]
    fn a_shop_that_watches_the_shelf_is_asked_a_window_at_a_time() {
        let watching = Situation {
            watches_stock: true,
            items: 450,
            ..idle()
        };
        let mut driver = settled();

        let first = driver.next(&watching, 0);
        assert_eq!(
            first,
            Next::FetchStock {
                from: 0,
                limit: STOCK_WINDOW
            }
        );
        driver.fetched_stock(0, 450);
        driver.succeeded(0);

        // Not again until it is due, whatever else is idle.
        assert!(!matches!(
            driver.next(&watching, 60_000),
            Next::FetchStock { .. }
        ));

        // And when it is, the next window along, then the last, then round.
        // The settings counter is cheap and often, so it is answered at each of
        // these before the question being asked here is the next one.
        let later = STOCK_REFRESH_MS + 1;
        driver.settings_seq(1, later);
        assert_eq!(
            driver.next(&watching, later),
            Next::FetchStock {
                from: 200,
                limit: STOCK_WINDOW
            }
        );
        driver.fetched_stock(later, 450);
        let second = later + STOCK_REFRESH_MS + 1;
        driver.settings_seq(1, second);
        // Ten minutes have gone by, so the lists are due as well: answered here
        // for the same reason as the settings, since they come first.
        driver.fetched_shop(second);
        driver.fetched_operators(second);
        driver.fetched_customers(second);
        assert_eq!(
            driver.next(&watching, later + STOCK_REFRESH_MS + 1),
            Next::FetchStock {
                from: 400,
                limit: STOCK_WINDOW
            }
        );
        driver.fetched_stock(second, 450);
        let third = later + 2 * STOCK_REFRESH_MS + 2;
        driver.settings_seq(1, third);
        driver.fetched_shop(third);
        driver.fetched_operators(third);
        driver.fetched_customers(third);
        assert_eq!(
            driver.next(&watching, later + 2 * STOCK_REFRESH_MS + 2),
            Next::FetchStock {
                from: 0,
                limit: STOCK_WINDOW
            },
            "and back to the start of the catalogue"
        );
    }

    #[test]
    fn a_credential_is_replaced_before_it_expires_rather_than_after() {
        let driver = settled();
        let taken = 1_788_600_000_000_u64;
        let fresh = Situation {
            credential_taken_at_ms: taken,
            // A device that has renewed once knows what the shop's policy is.
            credential_lifetime_ms: 365 * 24 * 60 * 60 * 1_000,
            ..idle()
        };

        // A month in, with eleven left. The whole point is to be nowhere near
        // the edge: a device that waits until the credential stops working is a
        // shop with a dead tablet and nobody able to enrol it without the
        // owner's device in front of them.
        assert_ne!(driver.next(&fresh, taken + 1_000), Next::RenewCredential);
        assert_eq!(
            driver.next(&fresh, taken + RENEW_CREDENTIAL_MS),
            Next::RenewCredential
        );

        // And ahead of sending sales, because sending needs a credential and
        // renewing while the old one still works costs one request.
        let selling = Situation {
            unsynced_sales: 40,
            ..fresh
        };
        assert_eq!(
            driver.next(&selling, taken + RENEW_CREDENTIAL_MS),
            Next::RenewCredential
        );
    }

    #[test]
    fn a_device_that_never_wrote_down_when_it_was_enrolled_renews_at_once() {
        let driver = settled();
        // What every device upgrading from a build that did not write it down
        // looks like. One request, and then it knows.
        let unknown = Situation {
            credential_taken_at_ms: 0,
            ..idle()
        };
        assert_eq!(driver.next(&unknown, 1_000), Next::RenewCredential);

        // A device with no credential at all has nothing to renew: it is
        // waiting for somebody to read an enrolment code onto it.
        let bare = Situation {
            enrolled: false,
            credential_taken_at_ms: 0,
            ..idle()
        };
        assert_ne!(driver.next(&bare, 1_000), Next::RenewCredential);
    }

    #[test]
    fn a_shorter_policy_is_followed_without_a_new_build() {
        let driver = settled();
        let taken = 1_000_u64;
        // A shop that shortens its credential life to a week. A third of it,
        // rather than the thirty days a longer policy would allow.
        let week = 7 * 24 * 60 * 60 * 1_000;
        let short = Situation {
            credential_taken_at_ms: taken,
            credential_lifetime_ms: week,
            ..idle()
        };
        assert_ne!(driver.next(&short, taken + week / 4), Next::RenewCredential);
        assert_eq!(driver.next(&short, taken + week / 3), Next::RenewCredential);
    }

    #[test]
    fn a_change_to_the_people_makes_all_three_lists_due_again() {
        let mut driver = settled();
        driver.pulled(0);

        // Nothing has moved, so nothing else is asked for. This is the case
        // that has to be cheap, because it is every cycle of every quiet day.
        assert_eq!(driver.next(&idle(), IDLE_MS), Next::CheckSettings);
        driver.settings_seq(1, IDLE_MS);
        assert!(matches!(driver.next(&idle(), IDLE_MS), Next::Pull { .. }));

        // Somebody is suspended. The counter moves, and the till re-reads the
        // people, the shop and the account customers: it cannot tell which of
        // them changed, and re-reading one it did not need is cheaper than
        // three counters and a chance to forget to move one.
        let after = IDLE_MS * 2;
        assert_eq!(driver.next(&idle(), after), Next::CheckSettings);
        driver.settings_seq(2, after);
        assert_eq!(driver.next(&idle(), after), Next::FetchShop);
        driver.fetched_shop(after);
        assert_eq!(driver.next(&idle(), after), Next::FetchOperators);
        driver.fetched_operators(after);
        assert_eq!(driver.next(&idle(), after), Next::FetchCustomers);
    }

    #[test]
    fn a_till_that_has_never_asked_takes_the_first_answer_as_its_mark() {
        let mut driver = Driver::new();
        driver.fetched_shop(0);
        driver.fetched_operators(0);
        driver.fetched_customers(0);
        driver.pulled(0);

        // Otherwise every till would re-read everything the first time it
        // looked, which is the opposite of what asking for one number is for.
        assert_eq!(driver.next(&idle(), 1_000), Next::CheckSettings);
        driver.settings_seq(7, 1_000);
        assert!(matches!(driver.next(&idle(), 1_000), Next::Wait { .. }));
    }

    #[test]
    fn what_people_owe_is_asked_for_only_where_anybody_is_written_down() {
        let mut driver = settled();
        driver.pulled(0);

        // A shop that sells on account against names typed at the till has no
        // balances to ask for, and asking would be a request every five minutes
        // for an empty list.
        assert!(matches!(driver.next(&idle(), 1_000), Next::Wait { .. }));

        let with_names = Situation {
            has_customers: true,
            ..idle()
        };
        assert_eq!(driver.next(&with_names, 1_000), Next::FetchBalances);

        // And not again straight away: a name is written down once and a
        // balance moves, but not once a second.
        driver.fetched_balances(1_000);
        assert!(matches!(driver.next(&with_names, 2_000), Next::Wait { .. }));
        let later = 1_000 + BALANCES_REFRESH_MS;
        assert_eq!(driver.next(&with_names, later), Next::CheckSettings);
        driver.settings_seq(1, later);
        assert_eq!(driver.next(&with_names, later), Next::FetchBalances);
    }

    #[test]
    fn an_open_drawer_is_reported_after_everything_that_is_a_record() {
        let driver = settled();
        let open = Situation {
            drawer_open: true,
            ..idle()
        };

        // Nothing else is waiting, so the drawer is what is left to say. A till
        // wiped in the morning with a drawer left open used to take the whole
        // summary with it; this bounds that to the last couple of minutes.
        assert_eq!(driver.next(&open, 0), Next::ReportDrawer);

        // But never ahead of a sale, which exists nowhere else. This is a
        // position: the counted drawer is the record, and it is pushed on its
        // own when somebody closes it.
        let selling = Situation {
            unsynced_sales: 1,
            ..open
        };
        assert!(matches!(driver.next(&selling, 0), Next::Push { .. }));
        let counted = Situation {
            unsent_shifts: 1,
            ..open
        };
        assert_eq!(driver.next(&counted, 0), Next::PushShifts);

        // And what it allowed goes next, for the same reason the drawer does:
        // it exists nowhere but this device until the shop has it.
        let allowed = Situation {
            unsent_shifts: 0,
            unsent_allowed: 2,
            ..counted
        };
        assert_eq!(driver.next(&allowed, 0), Next::PushAllowed);
    }

    #[test]
    fn an_open_drawer_is_not_reported_every_time_round() {
        let mut driver = settled();
        let open = Situation {
            drawer_open: true,
            ..idle()
        };
        driver.reported_drawer(0);

        // A quiet afternoon is not a reason to send the same figure a hundred
        // times. The catalogue is asked for instead, and the drawer waits.
        assert!(matches!(driver.next(&open, 1_000), Next::Pull { .. }));
        driver.pulled(1_000);
        assert!(matches!(driver.next(&open, 2_000), Next::Wait { .. }));
        assert_eq!(driver.next(&open, DRAWER_REPORT_MS), Next::CheckSettings);
        driver.settings_seq(1, DRAWER_REPORT_MS);
        assert_eq!(driver.next(&open, DRAWER_REPORT_MS), Next::ReportDrawer);
    }

    #[test]
    fn a_till_with_no_drawer_open_says_nothing_about_one() {
        let mut driver = settled();
        driver.pulled(0);
        assert!(matches!(driver.next(&idle(), 1_000), Next::Wait { .. }));
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
        assert_eq!(driver.next(&situation, 0), Next::Push { limit: PUSH_BATCH });
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
        let mut driver = Driver::new();
        // A till that has never asked anything asks the cheap question first
        // and takes the answer as its mark rather than as a change.
        assert_eq!(driver.next(&idle(), 0), Next::CheckSettings);
        driver.settings_seq(1, 0);
        // A receipt with no shop on it is one a customer cannot take back to
        // anybody, and a catalogue arriving first would let it sell anyway.
        assert_eq!(driver.next(&idle(), 0), Next::FetchShop);
    }

    #[test]
    fn a_till_learns_who_may_use_it_before_it_learns_what_it_sells() {
        let mut driver = Driver::new();
        driver.settings_seq(1, 0);
        driver.fetched_shop(0);
        // A catalogue arriving first would let it ring sales that nobody is
        // signed in for, and every refund would be refused with no way to fix
        // it from the counter.
        assert_eq!(driver.next(&idle(), 0), Next::FetchOperators);
    }

    #[test]
    fn a_shop_with_nobody_in_it_yet_does_not_ask_forever() {
        let mut driver = Driver::new();
        driver.settings_seq(1, 0);
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
        // The cheap question falls due first, as it does on every cycle.
        assert_eq!(driver.next(&idle(), later), Next::CheckSettings);
        driver.settings_seq(1, later);
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
        assert_eq!(
            driver.next(&idle(), 1_000),
            Next::Wait {
                for_ms: IDLE_MS - 1_000
            }
        );
        // One cheap question first, on the same cadence: whether the people,
        // the shop or the account customers moved. Then the catalogue.
        assert_eq!(driver.next(&idle(), IDLE_MS), Next::CheckSettings);
        driver.settings_seq(1, IDLE_MS);
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
