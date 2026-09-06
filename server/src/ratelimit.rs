//! Making guesses expensive.
//!
//! An enrolment code is eight characters from a 32 symbol alphabet, so forty
//! bits. That is ample against a person and thin against a machine allowed to
//! try continuously. Single use and a short expiry already bound the damage; a
//! limit on attempts is what makes the arithmetic hopeless rather than merely
//! unattractive.
//!
//! A fixed window rather than a token bucket, deliberately. The bucket is the
//! better shape for smoothing legitimate bursty traffic, and enrolment has no
//! legitimate bursty traffic: a shop enrols a tablet, then does not do it again
//! for months. A counter that resets every minute is easier to reason about and
//! has one obvious failure mode, which is that an attacker gets a fresh budget
//! at the top of each window. Against forty bits that is irrelevant.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Attempts allowed per key per window.
///
/// Ten is generous for the real case, where a code is typed once and works, and
/// ruinous for guessing: at ten a minute, half the code space takes something
/// like a hundred thousand years.
pub const DEFAULT_ATTEMPTS: u32 = 10;

/// How long a window lasts.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Over the limit. Carries how long until the window resets, so the caller
    /// can tell the client when to come back instead of leaving it to guess.
    Deny { retry_after: Duration },
}

/// Counts attempts per key over a fixed window.
#[derive(Debug)]
pub struct RateLimiter {
    windows: Mutex<HashMap<String, Window>>,
    attempts: u32,
    window: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    count: u32,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_ATTEMPTS, DEFAULT_WINDOW)
    }
}

impl RateLimiter {
    #[must_use]
    pub fn new(attempts: u32, window: Duration) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            attempts,
            window,
        }
    }

    /// Record an attempt and say whether it is allowed.
    ///
    /// Uses a monotonic clock, so a system clock stepping backwards, which does
    /// happen on cheap hardware syncing time for the first time, cannot hand an
    /// attacker an unlimited budget.
    pub fn check(&self, key: &str) -> Decision {
        let now = Instant::now();
        let Ok(mut windows) = self.windows.lock() else {
            // A poisoned lock means a thread panicked mid-update. Failing closed
            // is right here: refusing enrolments for a moment is a nuisance,
            // while failing open removes the only limit on guessing.
            return Decision::Deny {
                retry_after: self.window,
            };
        };

        // Drop windows that have aged out, so a long-running server does not
        // accumulate an entry per address that ever probed it.
        windows.retain(|_, entry| now.duration_since(entry.started) < self.window);

        let entry = windows.entry(key.to_owned()).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(entry.started) >= self.window {
            *entry = Window {
                started: now,
                count: 0,
            };
        }

        if entry.count >= self.attempts {
            return Decision::Deny {
                retry_after: self.window.saturating_sub(now.duration_since(entry.started)),
            };
        }
        entry.count = entry.count.saturating_add(1);
        Decision::Allow
    }

    /// How many keys are currently being tracked, for diagnostics.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.windows.lock().map(|windows| windows.len()).unwrap_or(0)
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

    #[test]
    fn allows_up_to_the_limit_then_refuses() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));

        for attempt in 0..3 {
            assert_eq!(limiter.check("1.2.3.4"), Decision::Allow, "attempt {attempt}");
        }
        assert!(matches!(
            limiter.check("1.2.3.4"),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn says_when_to_come_back() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        limiter.check("1.2.3.4");

        let Decision::Deny { retry_after } = limiter.check("1.2.3.4") else {
            panic!("the second attempt must be refused");
        };
        assert!(retry_after <= Duration::from_secs(60));
        assert!(retry_after > Duration::from_secs(50), "not an unhelpfully small hint");
    }

    #[test]
    fn one_clients_guessing_does_not_lock_out_another() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        limiter.check("attacker");
        limiter.check("attacker");
        assert!(matches!(limiter.check("attacker"), Decision::Deny { .. }));

        // The shop enrolling a tablet at the same moment is unaffected.
        assert_eq!(limiter.check("the shop"), Decision::Allow);
    }

    #[test]
    fn a_new_window_restores_the_budget() {
        let limiter = RateLimiter::new(1, Duration::from_millis(30));
        assert_eq!(limiter.check("1.2.3.4"), Decision::Allow);
        assert!(matches!(limiter.check("1.2.3.4"), Decision::Deny { .. }));

        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(limiter.check("1.2.3.4"), Decision::Allow, "the window rolled over");
    }

    #[test]
    fn stale_keys_do_not_accumulate() {
        let limiter = RateLimiter::new(5, Duration::from_millis(20));
        for index in 0..50 {
            limiter.check(&format!("prober {index}"));
        }
        assert_eq!(limiter.tracked(), 50);

        std::thread::sleep(Duration::from_millis(30));
        limiter.check("someone else");
        assert_eq!(
            limiter.tracked(),
            1,
            "a server probed for months must not grow a map of every address"
        );
    }
}
