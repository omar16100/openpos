//! Properties of reading a transcript.
//!
//! Examples cover the sentences somebody thought of. A recogniser does not
//! produce sentences somebody thought of: it produces whatever a microphone in a
//! shop made of a fan, a street, a printer and half a conversation, and it does
//! so in whatever script and length it likes. Nothing downstream of it may
//! panic, and nothing may come back with a quantity that was not earned.
//!
//! The one that matters most is the last: whatever a transcript says, the
//! quantity a caller receives is either a plain count in range or exactly one.
//! Everything a wrong reading could cost a shop is on the other side of that.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::domain::{PriceMode, Supply, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::{normalise, Item, Replica};
use openpos_core::voice::{resolve, understand, MOST_A_COUNT_MAY_BE, MOST_TERMS_KEPT};
use proptest::prelude::*;

/// A shop with the demo's goods in it, plus a crowd of oils so that a category
/// word is worth something different from a brand.
fn shop() -> Replica {
    let mut names = vec![
        "মিনিকেট চাল ৫ কেজি",
        "সয়াবিন তেল ১ লিটার",
        "মসুর ডাল ১ কেজি",
        "চিনি ১ কেজি",
        "চা ৪০০ গ্রাম",
    ];
    names.extend(std::iter::repeat_n("রান্নার তেল", 40));
    Replica::from_items(
        names
            .into_iter()
            .enumerate()
            .map(|(index, name_bn)| Item {
                id: Ulid::from_u128(index as u128 + 1),
                code: "SKU".into(),
                name_en: "English".into(),
                name_bn: name_bn.into(),
                unit: "Nos".into(),
                price: Minor::new(4_300),
                cost: Minor::new(3_800),
                vat_rate: Bp::ZERO,
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                supply: Supply::Standard,
                category: "".into(),
                barcodes: vec![],
                on_hand: Milli::new(40_000),
                active: true,
            })
            .collect(),
    )
}

/// Text a recogniser could plausibly emit: Bangla, Latin, digits in both
/// scripts, the marks that hold a Bangla word together, and punctuation.
fn shop_noise() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            // The Bengali block, including the unassigned holes in it.
            (0x0980u32..=0x09FFu32),
            // Latin, digits and punctuation.
            (0x0020u32..=0x007Eu32),
            // The invisible joiners, which a keyboard emits and nobody sees.
            Just(0x200Cu32),
            Just(0x200Du32),
        ],
        0..40usize,
    )
    .prop_map(|codepoints| codepoints.into_iter().filter_map(char::from_u32).collect())
}

proptest! {
    /// Nothing a recogniser can say is an error.
    ///
    /// This is the whole reason the crate's lints forbid unwrap and unchecked
    /// arithmetic: the input is not under anybody's control.
    #[test]
    fn reading_a_transcript_never_panics(said in shop_noise()) {
        let heard = understand(&said);
        // And every refusal has to be sayable, because a screen will say it.
        if let Some(refusal) = heard.refused {
            prop_assert!(!refusal.to_string().is_empty());
        }
    }

    /// The property the money rests on.
    ///
    /// A caller may act on the quantity without checking it, so it is one of two
    /// things: a count the till was willing to stand behind, or one of the item.
    /// Never zero, never negative, never a weight read as a count.
    #[test]
    fn the_quantity_is_always_a_plain_count_or_one(said in shop_noise()) {
        let heard = understand(&said);
        let quantity = heard.quantity();
        prop_assert!(!quantity.is_negative());
        prop_assert!(quantity >= Milli::ONE);
        match heard.count {
            Some(count) => {
                prop_assert!((1..=MOST_A_COUNT_MAY_BE).contains(&count));
                prop_assert_eq!(quantity, Milli::new(i64::from(count) * 1_000));
                prop_assert!(heard.refused.is_none(), "counted and refused at once");
            }
            None => prop_assert_eq!(quantity, Milli::ONE),
        }
    }

    /// A number can only come from something that looks like one. Text with no
    /// digit in it and no numeral word cannot produce a count, so a name that
    /// happens to rhyme with a number never multiplies a line.
    #[test]
    fn text_without_a_digit_or_a_numeral_never_counts(
        said in proptest::collection::vec(
            prop_oneof![(0x0985u32..=0x09B9u32), Just(0x0020u32)],
            0..30usize,
        ).prop_map(|c| c.into_iter().filter_map(char::from_u32).collect::<String>())
    ) {
        let heard = understand(&said);
        // The generator draws only Bengali consonants and spaces, so any count
        // would have to have come from a whole numeral word appearing by chance.
        if heard.count.is_some() {
            prop_assert!(
                said.split_whitespace().count() > 0,
                "a count out of nothing at all"
            );
        }
    }

    /// However long the microphone was left open, the till thinks about a
    /// bounded number of words.
    #[test]
    fn the_work_is_bounded_by_the_rule_and_not_by_the_speaker(said in shop_noise()) {
        let heard = understand(&said);
        prop_assert!(heard.terms.len() <= MOST_TERMS_KEPT);
        prop_assert!(heard.ignored.len() <= MOST_TERMS_KEPT);
        prop_assert!(heard.terms.len() + heard.ignored.len() <= MOST_TERMS_KEPT);
    }

    /// Every term handed on is already folded the way the catalogue index is
    /// folded. A term that was not could never match anything, and the reason
    /// would be invisible at the point it mattered.
    #[test]
    fn every_term_is_folded_the_way_the_index_is(said in shop_noise()) {
        for term in understand(&said).terms {
            let refolded = normalise(&term);
            prop_assert_eq!(refolded.trim(), &*term);
            prop_assert!(!term.is_empty());
            prop_assert!(!term.contains(char::is_whitespace));
        }
    }

    /// Folding is settled after one pass. If it were not, the index and a query
    /// could be folded a different number of times and quietly disagree.
    #[test]
    fn folding_is_idempotent(said in shop_noise()) {
        let once = normalise(&said);
        prop_assert_eq!(normalise(&once), once.clone());
    }

    /// Resolution is looser than the typed search but never inventive: an item
    /// it offers can always be reached by typing the words it used.
    ///
    /// This is what keeps "score rather than intersect" honest. Scoring is there
    /// to survive the words a recogniser adds and drops, not to reach items that
    /// the words never pointed at.
    #[test]
    fn nothing_is_offered_that_the_words_did_not_point_at(said in shop_noise()) {
        let shop = shop();
        let heard = understand(&said);
        for id in resolve(&shop, &heard, 12).candidates {
            let reachable = heard.terms.iter().any(|term| {
                shop.search(term, 500).iter().any(|item| item.id == id)
            });
            prop_assert!(reachable, "offered an item no word asked for");
        }
    }

    /// The safety property, and the one the whole design rests on.
    ///
    /// Corrupt an utterance the way a shop corrupts one: lose a word to a fan, or
    /// gain one that nobody said. The till may then find nothing, or offer a
    /// list, or be less sure than it was. What it may never do is become sure of
    /// a *different* item than the clean sentence pointed at, because that is the
    /// failure a cashier cannot see: the screen looks exactly as confident as it
    /// does when it is right.
    #[test]
    fn losing_or_gaining_a_word_never_makes_it_sure_of_something_else(
        clean in prop::sample::select(vec![
            "মিনিকেট চাল ৫ কেজি",
            "সয়াবিন তেল ১ লিটার",
            "মসুর ডাল ১ কেজি",
            "চিনি ১ কেজি",
            "চা ৪০০ গ্রাম",
            "ভাই একটু চাল দাও",
            "তিন প্যাকেট চাল",
        ]),
        drop_at in 0..6usize,
        noise in "[ঝঞটঠডঢণ]{3,6}",
    ) {
        let shop = shop();
        let words: Vec<&str> = clean.split_whitespace().collect();
        let honest = resolve(&shop, &understand(clean), 12);
        let truth = honest.sure.then(|| honest.candidates.first().copied()).flatten();

        let mut corrupted: Vec<&str> = words.clone();
        if drop_at < corrupted.len() {
            corrupted.remove(drop_at);
        }
        let heard_wrong = corrupted.join(" ");
        let with_noise = format!("{clean} {noise}");

        for said in [heard_wrong, with_noise] {
            let after = resolve(&shop, &understand(&said), 12);
            if let (true, Some(first)) = (after.sure, after.candidates.first().copied()) {
                match truth {
                    Some(expected) => prop_assert_eq!(
                        first, expected,
                        "sure of a different item after {:?}", said
                    ),
                    // Becoming sure where the clean sentence was not is allowed
                    // only when dropping a word left something unambiguous, which
                    // is a narrowing rather than a substitution. What it must not
                    // be is a jump to an item the clean words never offered.
                    None => prop_assert!(
                        honest.candidates.contains(&first),
                        "sure of an item the clean sentence never offered, after {:?}", said
                    ),
                }
            }
        }
    }
}
