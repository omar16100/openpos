//! Every word on a paper can be supplied by the shop, and the list is frozen.
//!
//! The three papers this crate lays out (a receipt, a drawer slip, an account
//! page) are read by a customer, by whoever counts the drawer, and by a
//! neighbour settling up. A shop whose screens speak Bangla and whose paper
//! speaks English is the shop's own till disagreeing with its own receipt.
//!
//! So every label goes through `Words::word(key, english)`, and the keys are
//! frozen here and written out to `apps/shared/paper_words.json` for the screens
//! to translate against. Same arrangement as `refusal_codes.rs`, for the same
//! reason: the JavaScript cannot read Rust, and a list somebody copied by hand
//! goes stale the first time a label is added.
//!
//! Source scanning rather than anything cleverer, because the property is about
//! what is written in the file.

// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::collections::BTreeSet;

/// Every key a paper asks for. Adding a label is adding a line here and a word
/// for it in `apps/shared/words.js`, which the JavaScript test enforces.
const EVERY_KEY: &[&str] = &[
    "account.in_credit",
    "account.name",
    "account.nothing_on_it",
    "account.owing",
    "account.title",
    "drawer.cash_in",
    "drawer.cash_out",
    "drawer.checked_by",
    "drawer.counted",
    "drawer.counted_by",
    "drawer.counted_title",
    "drawer.exactly_right",
    "drawer.not_in_the_till",
    "drawer.opening_float",
    "drawer.over_by",
    "drawer.printed_by",
    "drawer.sales",
    "drawer.short_by",
    "drawer.should_hold",
    "drawer.so_far_title",
    "drawer.till",
    "paper.a_copy",
    "paper.printed",
    "receipt.against",
    "receipt.buyer_bin",
    "receipt.card",
    "receipt.cash",
    "receipt.change",
    "receipt.customer",
    "receipt.date",
    "receipt.discount",
    "receipt.line_discount",
    "receipt.net",
    "receipt.number",
    "receipt.on",
    "receipt.on_account",
    "receipt.refund_title",
    "receipt.served_by",
    "receipt.thank_you",
    "receipt.to_be_assigned",
    "receipt.total",
    "receipt.vat",
    "receipt.vat_in_all",
];

/// The keys the source actually asks for, read out of the calls themselves.
fn asked_for() -> BTreeSet<String> {
    let source = include_str!("../../src/receipt/mod.rs");
    let mut found = BTreeSet::new();
    for (at, _) in source.match_indices(".word(\"") {
        let rest = &source[at + ".word(\"".len()..];
        if let Some(end) = rest.find('"') {
            found.insert(rest[..end].to_owned());
        }
    }
    found
}

#[test]
fn every_label_on_a_paper_can_be_supplied() {
    let asked = asked_for();
    for key in &asked {
        assert!(
            EVERY_KEY.contains(&key.as_str()),
            "{key} is asked for by a paper and is not in the frozen list. A screen translates \
             against that list, so a label missing from it is one nobody can put into their own \
             language."
        );
    }
    for key in EVERY_KEY {
        assert!(
            asked.contains(*key),
            "{key} is frozen and no paper asks for it: take it out, or somebody translates a \
             label that is never printed"
        );
    }
}

#[test]
fn the_screens_are_handed_the_same_list() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/shared/paper_words.json");
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut written = String::from("[\n");
    for (at, key) in EVERY_KEY.iter().enumerate() {
        written.push_str("  \"");
        written.push_str(key);
        written.push('"');
        if at + 1 < EVERY_KEY.len() {
            written.push(',');
        }
        written.push('\n');
    }
    written.push_str("]\n");

    if held.trim() != written.trim() {
        std::fs::write(path, &written).expect("apps/shared/paper_words.json is writable");
        panic!(
            "apps/shared/paper_words.json did not match the frozen list and has been rewritten. \
             Run the tests again, and give every new key words in apps/shared/words.js"
        );
    }
}
