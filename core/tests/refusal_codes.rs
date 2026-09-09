//! Every refusal a till can give has a stable name, and the list is frozen.
//!
//! The words a refusal carries are English, and they are what a log and a
//! developer read. A shop in Bangladesh has a cashier reading the screen, and
//! the moment something is refused is exactly the moment they need it in their
//! own language: a screen that matched on the English sentence to translate it
//! would go quiet the day somebody improved the wording.
//!
//! So each refusal carries a code, and the codes are frozen here and written
//! out to `apps/shared/refusals.json` for the screens to key a dictionary on.
//! One list, in one place, and two tests that keep it honest: this one, which
//! fails when a code appears or disappears without the list being changed, and
//! the JavaScript one, which fails when the dictionary does not cover it.

// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeSet;

use openpos_core::auth::{Action, AuthError};
use openpos_core::cart::CartError;
use openpos_core::money::{Minor, MoneyError};
use openpos_core::shift::ShiftError;
use openpos_core::till::TillError;

/// Every code a screen has to be able to say. Adding one here is the cheap half
/// of adding a refusal; the other half is a word for it in every language the
/// shop screens speak, which the JavaScript test enforces.
const EVERY_CODE: &[&str] = &[
    "authorisation-expired",
    "beyond-their-limit",
    "change-from-a-promise",
    "discount-above-ceiling",
    "drawer-already-closed",
    "drawer-still-open",
    "empty-basket",
    "journal",
    "locked-out",
    "mixed-sale-and-return",
    "money",
    "more-than-the-shelf-holds",
    "nameless-customer",
    "nameless-item",
    "nameless-operator",
    "nameless-shop",
    "negative-amount",
    "negative-price",
    "no-barcode-to-find-it-by",
    "no-longer-sold",
    "no-open-shift",
    "no-reason",
    "no-such-held-ticket",
    "no-such-line",
    "not-permitted",
    "nothing-to-hold",
    "price-override-not-allowed",
    "refund-not-settled",
    "sync",
    "ticket-in-progress",
    "underpaid",
    "unknown-barcode",
    "unknown-customer",
    "unknown-operator",
    "wire",
    "write-it-against-them",
    "wrong-pin",
];

/// One of every refusal there is, built so the codes come from the code rather
/// than from a second list somebody typed.
fn one_of_each() -> Vec<TillError> {
    vec![
        TillError::UnknownBarcode,
        TillError::NoLongerSold,
        TillError::NothingToHold,
        TillError::NoSuchHeldTicket,
        TillError::TicketInProgress,
        TillError::NoOpenShift,
        TillError::NamelessShop,
        TillError::NamelessItem,
        TillError::NamelessCustomer,
        TillError::NoBarcodeToFindItBy,
        TillError::NamelessOperator,
        TillError::UnknownCustomer,
        TillError::MoreThanTheShelfHolds {
            name: "Rice".to_owned(),
            on_hand_milli: 1_000,
            wanted_milli: 2_000,
        },
        TillError::BeyondTheirLimit {
            name: "Karim".to_owned(),
            owed_minor: 100,
            owed_as_of_ms: 1,
            limit_minor: 200,
            wanted_minor: 300,
        },
        TillError::WriteItAgainstThem {
            name: "Karim".to_owned(),
        },
        TillError::Cart(CartError::NoSuchLine { index: 0 }),
        TillError::Cart(CartError::Empty),
        TillError::Cart(CartError::MixedSaleAndReturn),
        TillError::Cart(CartError::RefundNotSettled {
            outstanding: Minor::new(1),
        }),
        TillError::Cart(CartError::DiscountAboveCeiling {
            requested: 1,
            ceiling: 0,
        }),
        TillError::Cart(CartError::PriceOverrideNotAllowed),
        TillError::Cart(CartError::NegativePrice {
            price: Minor::new(-1),
        }),
        TillError::Cart(CartError::Underpaid {
            short_by: Minor::new(1),
        }),
        TillError::Cart(CartError::ChangeFromAPromise {
            over_by: Minor::new(1),
            cash: Minor::new(0),
        }),
        TillError::Cart(CartError::Money(MoneyError::Overflow)),
        TillError::Auth(AuthError::UnknownOperator),
        TillError::Auth(AuthError::WrongPin { attempts_left: 1 }),
        TillError::Auth(AuthError::LockedOut { until_ms: 1 }),
        TillError::Auth(AuthError::NotPermitted {
            action: Action::Refund,
        }),
        TillError::Auth(AuthError::AuthorisationExpired),
        TillError::Shift(ShiftError::AlreadyClosed { closed_at_ms: 1 }),
        TillError::Shift(ShiftError::StillOpen),
        TillError::Shift(ShiftError::NegativeAmount {
            amount: Minor::new(-1),
        }),
        TillError::Shift(ShiftError::NoReason),
        TillError::Shift(ShiftError::Money(MoneyError::Overflow)),
    ]
}

#[test]
fn every_refusal_carries_a_code_from_the_frozen_list() {
    for refusal in one_of_each() {
        let code = refusal.code();
        assert!(
            EVERY_CODE.contains(&code),
            "{refusal:?} answers {code}, which is not in the frozen list. A screen keys its \
             words on these, so a new one needs adding here and translating in \
             apps/shared/words.js"
        );
        assert!(
            !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "{code} is not a stable key: lower case and hyphens, because it is read by \
             JavaScript and by people"
        );
    }
}

#[test]
fn the_frozen_list_holds_nothing_that_no_refusal_gives() {
    // The other direction, or the list fills up with codes nothing can produce
    // and every screen carries words for refusals that cannot happen.
    //
    // The three wrapped kinds are the exception and are named: a journal, a sync
    // or a wire failure is one code covering a family of faults a cashier can do
    // nothing about, and building one of those here would mean a broken store.
    let given: std::collections::BTreeSet<&str> =
        one_of_each().iter().map(TillError::code).collect();
    for code in EVERY_CODE {
        assert!(
            given.contains(code) || matches!(*code, "journal" | "sync" | "wire"),
            "{code} is frozen and nothing produces it"
        );
    }
}

#[test]
fn the_screens_are_handed_the_same_list() {
    // Written out rather than kept in two places. The JavaScript cannot read
    // Rust, and a dictionary keyed on a list somebody copied by hand is a
    // dictionary that goes stale the first time a refusal is added.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/shared/refusals.json");
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines = String::from("[\n");
    for (at, code) in EVERY_CODE.iter().enumerate() {
        lines.push_str("  \"");
        lines.push_str(code);
        lines.push('"');
        if at + 1 < EVERY_CODE.len() {
            lines.push(',');
        }
        lines.push('\n');
    }
    lines.push_str("]\n");

    if held.trim() != lines.trim() {
        std::fs::write(path, &lines).expect("apps/shared/refusals.json is writable");
        panic!(
            "apps/shared/refusals.json did not match the frozen list and has been rewritten. \
             Run the tests again, and give every new code words in apps/shared/words.js"
        );
    }
}

/// Every code a refusal from the shop's own server carries. Adding one is the
/// cheap half; the other half is a word for it in every language, which
/// `apps/shared/words.test.js` enforces against the same written-out list.
const EVERY_SERVER_CODE: &[&str] = &[
    "barcode-in-use",
    "device-needs-updating",
    "device-not-permitted",
    "item-has-history",
    "malformed",
    "not-a-price",
    "stale",
    "too-many-attempts",
    "unauthenticated",
    "unknown-terminal",
];

/// One of every refusal the server can give.
fn one_of_each_server() -> Vec<openpos_core::protocol::ProtocolError> {
    use openpos_core::protocol::ProtocolError as Refusal;

    vec![
        Refusal::UnsupportedVersion {
            requested: 1,
            minimum: 2,
            current: 3,
        },
        Refusal::UnknownTerminal,
        Refusal::Malformed,
        Refusal::Unauthenticated,
        Refusal::TooManyAttempts {
            retry_after_seconds: 30,
        },
        Refusal::NotPermitted,
        Refusal::Stale,
        Refusal::BarcodeInUse {
            barcode: "8901234567890".to_owned(),
        },
        Refusal::ItemHasHistory,
        Refusal::NotAPrice {
            said: "a tax rate of 150% is not a rate".to_owned(),
        },
    ]
}

/// Every variant of `ProtocolError`, read out of the enum itself.
///
/// `one_of_each_server()` below is written by hand, and a list written by hand
/// is a list somebody forgets. Adding a variant, giving it a code and a
/// sentence, and not adding it here leaves `server_refusals.json` unaware of
/// it, the dictionary unaware of it, and every screen showing the English at
/// the moment the shop is being refused something.
///
/// Source scanning rather than anything cleverer, for the same reason
/// `paper_words.rs` does it: the property is about what is written in the file.
fn every_variant_written_down() -> BTreeSet<String> {
    let source = include_str!("../src/protocol/mod.rs");
    let at = source
        .find("pub enum ProtocolError {")
        .expect("the enum is in this file");
    let body = &source[at..];
    let end = body.find("\n}\n").expect("the enum ends");
    let mut found = BTreeSet::new();
    let mut depth = 0_i32;
    for line in body[..end].lines().skip(1) {
        let trimmed = line.trim();
        // Only the outermost level names a variant; a braced variant's fields
        // are indented inside it.
        if depth == 0
            && let Some(name) = trimmed.split(['{', ',', '(']).next()
            && !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
        {
            found.insert(name.trim().to_owned());
        }
        depth += i32::try_from(trimmed.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(trimmed.matches('}').count()).unwrap_or(0);
    }
    found
}

#[test]
fn the_server_list_holds_one_of_every_variant_there_is() {
    let written = every_variant_written_down();
    assert!(
        written.len() >= 10,
        "the scan found {} variants, which is not the enum: it has been reformatted and this \
         test is no longer reading it",
        written.len()
    );
    let built: BTreeSet<String> = one_of_each_server()
        .iter()
        .map(|refusal| {
            let shown = alloc_debug(refusal);
            shown
                .split([' ', '{', '('])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    for name in &written {
        assert!(
            built.contains(name),
            "ProtocolError::{name} exists and one_of_each_server() does not build one, so its \
             code is never checked against the frozen list and no screen has words for it"
        );
    }
}

/// A variant's name, as Debug writes it.
fn alloc_debug(refusal: &openpos_core::protocol::ProtocolError) -> String {
    format!("{refusal:?}")
}

#[test]
fn every_refusal_the_server_gives_carries_a_code_from_its_frozen_list() {
    // The server was the last place in this system that could only speak
    // English. A save built on a stale copy, a barcode another item holds, an
    // item the shop has traded, a rate no till could price: exactly the moments
    // an owner needs their own language, and the only ones that did not have it.
    let given: std::collections::BTreeSet<&str> =
        one_of_each_server().iter().map(|e| e.code()).collect();
    for refusal in one_of_each_server() {
        let code = refusal.code();
        assert!(
            EVERY_SERVER_CODE.contains(&code),
            "{refusal:?} answers {code}, which is not in the frozen list. A screen keys its words \
             on these, so a new one needs adding here and translating in apps/shared/words.js"
        );
        assert!(
            !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "{code} is not a stable key: lower case and hyphens, because it is read by \
             JavaScript and by people"
        );
    }
    for code in EVERY_SERVER_CODE {
        assert!(given.contains(code), "{code} is frozen and nothing produces it");
    }
    // The two lists share a dictionary, so a name used twice would have one set
    // of words serving two different refusals: a till's "not permitted" is a
    // cashier who may not do that, and the server's is a device that may not.
    for code in EVERY_SERVER_CODE {
        assert!(
            !EVERY_CODE.contains(code),
            "{code} names both a till's refusal and the server's, and they share one dictionary"
        );
    }
}

#[test]
fn the_screens_are_handed_the_servers_list_too() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../apps/shared/server_refusals.json"
    );
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines = String::from("[\n");
    for (at, code) in EVERY_SERVER_CODE.iter().enumerate() {
        lines.push_str("  \"");
        lines.push_str(code);
        lines.push('"');
        if at + 1 < EVERY_SERVER_CODE.len() {
            lines.push(',');
        }
        lines.push('\n');
    }
    lines.push_str("]\n");

    if held.trim() != lines.trim() {
        std::fs::write(path, &lines).expect("apps/shared/server_refusals.json is writable");
        panic!(
            "apps/shared/server_refusals.json did not match the frozen list and has been \
             rewritten. Run the tests again, and give every new code words in apps/shared/words.js"
        );
    }
}

/// Every number a till can write into its trail, and the JavaScript gets the
/// list.
///
/// The trail is what an owner reads when they want to know what happened at a
/// counter that evening, and a screen turns each number into a phrase. A number
/// with no phrase falls back to the English sentence the till sent, which is
/// how action twelve, a sale written against somebody already past what they
/// may owe, read as English in a Bangla shop from the day it was added.
///
/// Nothing else could have caught it: these are asked for by number rather than
/// by name, so the test that scans the screens for keys cannot see them.
///
/// Numbers are never reused. A shop's stored trail is read under this list, so
/// a number that changes meaning is last year's evenings quietly saying
/// something else.
const EVERY_TRAIL_CODE: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

#[test]
fn the_screens_are_handed_every_number_a_trail_can_hold() {
    // Read out of the source rather than listed twice: the mapping from an
    // action to a number is in `till.rs`, and so are the numbers written
    // directly for the things that are not permissions.
    let source = include_str!("../src/till.rs");
    let mut found: BTreeSet<u8> = BTreeSet::new();
    for (at, _) in source.match_indices("=> (") {
        let rest = &source[at + "=> (".len()..];
        if let Some(end) = rest.find(',')
            && let Ok(code) = rest[..end].trim().trim_end_matches("_u8").parse::<u8>()
        {
            found.insert(code);
        }
    }
    for (at, _) in source.match_indices("write_down_allowed(") {
        let rest = &source[at + "write_down_allowed(".len()..];
        // The second argument, when it is a number written there and then.
        if let Some(end) = rest.find(')')
            && let Some(second) = rest[..end].split(',').nth(1)
            && let Ok(code) = second.trim().parse::<u8>()
        {
            found.insert(code);
        }
    }
    // The three the PIN path writes by hand.
    for (at, _) in source.match_indices("(true, _) => ") {
        let rest = &source[at + "(true, _) => ".len()..];
        if let Some(end) = rest.find(',')
            && let Ok(code) = rest[..end].trim().parse::<u8>()
        {
            found.insert(code);
        }
    }
    for pattern in ["(false, true) => ", "(false, false) => "] {
        for (at, _) in source.match_indices(pattern) {
            let rest = &source[at + pattern.len()..];
            if let Some(end) = rest.find(',')
                && let Ok(code) = rest[..end].trim().parse::<u8>()
            {
                found.insert(code);
            }
        }
    }

    assert!(
        found.len() >= 10,
        "the scan found {} trail numbers, which is not the file: it has been written another way \
         and this test is no longer reading it",
        found.len()
    );
    for code in &found {
        assert!(
            EVERY_TRAIL_CODE.contains(code),
            "a till can write {code} into its trail and it is not in the frozen list, so no screen \
             has a phrase for it and a shop reads the English fallback"
        );
    }

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/shared/trail_codes.json");
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut written = String::from("[\n");
    for (at, code) in EVERY_TRAIL_CODE.iter().enumerate() {
        written.push_str("  ");
        written.push_str(&code.to_string());
        if at + 1 < EVERY_TRAIL_CODE.len() {
            written.push(',');
        }
        written.push('\n');
    }
    written.push_str("]\n");
    if held.trim() != written.trim() {
        std::fs::write(path, &written).expect("apps/shared/trail_codes.json is writable");
        panic!(
            "apps/shared/trail_codes.json did not match the frozen list and has been rewritten. \
             Run the tests again, and give every new number a phrase in apps/shared/words.js"
        );
    }
}

/// A quarantine reason is stored in the shop's database, so its bytes are
/// frozen too.
///
/// The server writes the reason itself beside the sentence, as postcard, which
/// is positional: a variant inserted rather than appended would make every row
/// already in a shop's database decode as a different reason. The screen would
/// then say, in the shop's own language and with conviction, the wrong thing
/// about why a sale is being held.
///
/// The hex is a record, not something generated. If one of these fails, either
/// somebody reordered the enum, in which case put it back, or the encoding
/// changed, in which case every shop's stored reasons need reading under the
/// old shape first.
#[test]
fn the_bytes_a_shop_already_holds_still_say_what_they_said() {
    use openpos_core::protocol::QuarantineReason as Why;

    let frozen: &[(&str, Why)] = &[
        (
            "00b6cc02e8cc02",
            Why::TotalsMismatch {
                stored_minor: 21_275,
                recomputed_minor: 21_300,
            },
        ),
        (
            "010954312d303030313030",
            Why::DuplicateReceiptNumber {
                receipt_no: "T1-000100".to_owned(),
            },
        ),
        ("02", Why::Undecodable),
        ("03", Why::CarriedIn),
        (
            "0480bcf886873480e9da8b8734",
            Why::ClockOutOfRange {
                rung_at_ms: 1_788_600_000_000,
                received_at_ms: 1_788_610_000_000,
            },
        ),
    ];

    for (hex, reason) in frozen {
        let written = postcard::to_allocvec(reason).expect("a reason encodes");
        let said = written
            .iter()
            .map(|byte| alloc_hex(*byte))
            .collect::<String>();
        assert_eq!(
            &said.as_str(),
            hex,
            "{reason:?} encodes differently than the bytes a shop already holds"
        );

        let read: Why = postcard::from_bytes(&written).expect("and decodes");
        assert_eq!(&read, reason);
    }
}

fn alloc_hex(byte: u8) -> String {
    format!("{byte:02x}")
}
