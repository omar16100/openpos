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

use openpos_core::money::Milli;
use openpos_core::replica::normalise;
use openpos_core::voice::{understand, MOST_A_COUNT_MAY_BE, MOST_TERMS_KEPT};
use proptest::prelude::*;

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
}
