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
