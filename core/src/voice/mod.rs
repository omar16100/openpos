//! What the cashier said, turned into something a till can act on.
//!
//! The microphone, the recogniser and the model are the platform's business.
//! This takes the text they produce and decides what it means, for the same
//! reason every other decision is in this crate: a rule written twice is a rule
//! that differs, and the difference would show up as a browser and a tablet
//! ringing different baskets from the same sentence.
//!
//! # What this refuses to do
//!
//! It never applies a quantity. It offers one, and only for the shape of
//! utterance where the answer is not in doubt: a plain count of separate things,
//! with a word saying so. Everything else that carries a number comes back
//! refused, with the reason in words a shop can act on.
//!
//! That is not caution for its own sake. A recogniser gets somewhere between one
//! word in six and one word in three wrong on read speech, and a counter is
//! noisier than read speech. The cost of a proposal a cashier ignores is a
//! glance. The cost of a quantity applied wrongly is the price difference, a
//! wrong tax position, a stock figure that walks away from the shelf, and an
//! owner who stops believing the till. Those are not the same size, so the rules
//! below are not balanced between them.
//!
//! # What it cannot do, and why the reason is not speech
//!
//! Weights and volumes are refused, and no better recogniser would fix it.
//! Turning "five hundred grams" into a quantity needs a fact about the item that
//! this system has never recorded: `Item.unit` is prose, defaulted to `"Nos"`
//! and typed by hand, so nothing says whether a thing is sold by the kilo or by
//! the packet, nor how many grams are in one of it. Against a five hundred gram
//! packet the answer is one; against loose goods it is five hundred. A thousand
//! times apart, and nothing here can tell which.

mod bangla;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::money::Milli;
use crate::replica::normalise;

pub use bangla::Sense;

/// The largest count this will offer.
///
/// Ninety-nine of one thing is already a delivery rather than a sale, and every
/// number above it is far more likely to be money, a weight, or a digit heard
/// wrong. Refusing at the top of the range costs a cashier two keystrokes on the
/// rare real case and stops the common unreal one.
pub const MOST_A_COUNT_MAY_BE: u32 = 99;

/// How many words are worth searching on.
///
/// A cashier asking for something says a few words. A recogniser fed a fan, a
/// street and half a conversation produces a paragraph, and there is no reason
/// to let the length of that paragraph decide how long the till thinks for.
pub const MOST_TERMS_KEPT: usize = 12;

/// Why a number that was said did not become a quantity.
///
/// Carried rather than swallowed, because "it did not understand" and "it
/// understood and will not guess" look identical on a screen that is not told
/// them apart, and they need different things from the cashier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A hundred taka of rice, not a hundred bags of it.
    Money,
    /// A weight or a volume, which needs a fact about the item nobody recorded.
    Measure,
    /// Half, one and a half, two and a half.
    Fraction,
    /// A hali, a dozen, a pair: how many that is depends on how the shop entered
    /// the item, not on the language.
    Set,
    /// More than one number, and nothing says which one counts.
    Several,
    /// A number with no word saying what it counts.
    Bare { count: u32 },
    /// A count larger than this will offer.
    TooMany { count: u32 },
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Money => f.write_str(
                "that sounded like an amount of money rather than a number of things, so the \
                 quantity is left at one",
            ),
            Self::Measure => f.write_str(
                "a weight or a volume was said, and this till has not been told how much of an \
                 item one of it is, so the quantity is left at one",
            ),
            Self::Fraction => f.write_str(
                "part of a unit was said, and the quantity is left at one rather than rounded",
            ),
            Self::Set => f.write_str(
                "how many that is depends on how the shop entered the item, so the quantity is \
                 left at one",
            ),
            Self::Several => f.write_str(
                "more than one number was said and nothing says which one is the quantity, so it \
                 is left at one",
            ),
            Self::Bare { count } => write!(
                f,
                "{count} was said with nothing saying what it counts, so the quantity is left at \
                 one"
            ),
            Self::TooMany { count } => write!(
                f,
                "{count} is more than this till will put on a line without being asked twice, so \
                 the quantity is left at one"
            ),
        }
    }
}

impl core::error::Error for Refusal {}

/// A transcript, read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Understood {
    /// The words worth looking the item up by, folded the way the catalogue
    /// index is folded so the two can meet.
    pub terms: Vec<Box<str>>,
    /// Words dropped as politeness or grammar. Kept so a screen can show what
    /// it threw away: a cashier who cannot see that has no way to learn what
    /// the till listens to.
    pub ignored: Vec<Box<str>>,
    /// How many, when the till is willing to say. Never applied here.
    pub count: Option<u32>,
    /// Why there is no count, when a number was said and refused.
    pub refused: Option<Refusal>,
}

impl Understood {
    /// The count as a quantity, or one of the thing.
    ///
    /// One is the answer whenever the till would not say, which is the same
    /// answer a scan gives and the one a cashier can correct in a single press.
    #[must_use]
    pub fn quantity(&self) -> Milli {
        match self.count {
            Some(count) => Milli::new(i64::from(count).saturating_mul(1_000)),
            None => Milli::ONE,
        }
    }

    /// Whether there is anything here to look an item up by.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// One token, once it has been read.
struct Token {
    text: String,
    sense: Sense,
    /// Whether a counter was written onto the number itself, as in "তিনটা".
    counted: bool,
}

/// Read a transcript.
///
/// Total by construction: a recogniser emits whatever it likes, including
/// nothing, including a paragraph, including scripts this shop does not use, and
/// none of that is an error. An utterance with nothing in it comes back empty
/// rather than refused.
#[must_use]
pub fn understand(transcript: &str) -> Understood {
    let folded = normalise(transcript);
    let tokens: Vec<Token> = folded
        .split_whitespace()
        .take(MOST_TERMS_KEPT)
        .map(|text| {
            let (sense, counted) = bangla::sense_of(text);
            Token {
                text: String::from(text),
                sense,
                counted,
            }
        })
        .collect();

    let (count, refused) = read_the_count(&tokens);

    // A number that became the quantity is not also a word to search on: asking
    // the catalogue for "3" would pull in every item with a 3 anywhere in its
    // name. A number that was refused stays, because then it is part of what was
    // being described, which is exactly the case of "মিনিকেট চাল ৫ কেজি".
    let quantity_taken = count.is_some();
    let mut terms = Vec::new();
    let mut ignored = Vec::new();
    for token in tokens {
        match token.sense {
            Sense::Filler => ignored.push(token.text.into_boxed_str()),
            Sense::Number(_) if quantity_taken => ignored.push(token.text.into_boxed_str()),
            _ => terms.push(token.text.into_boxed_str()),
        }
    }

    Understood {
        terms,
        ignored,
        count,
        refused,
    }
}

/// Decide the count, or the reason there is not one.
///
/// Reads the whole utterance before deciding anything. A rule that looked only
/// at the front would take the three in "তিন কেজি চাল" and never see the কেজি
/// that makes it unanswerable.
fn read_the_count(tokens: &[Token]) -> (Option<u32>, Option<Refusal>) {
    let says = |wanted: &Sense| tokens.iter().any(|token| token.sense == *wanted);

    // Read before the search for a number, because these two carry an amount on
    // their own. "আধা কেজি চিনি" is half a kilo of sugar and "হালি ডিম" is four
    // eggs, and neither sentence contains a numeral. Looking for a number first
    // would answer "nothing was said about how many" for an utterance that said
    // it plainly, and the screen would offer one of something the cashier never
    // asked for one of.
    if says(&Sense::Fraction) {
        return (None, Some(Refusal::Fraction));
    }
    if says(&Sense::Set) {
        return (None, Some(Refusal::Set));
    }

    let mut numbers = tokens.iter().filter_map(|token| match token.sense {
        Sense::Number(value) => Some((value, token.counted)),
        _ => None,
    });
    let Some((value, counted_on_the_number)) = numbers.next() else {
        // Nothing was said about how many, which is not a refusal. Every scan
        // the till has ever taken is this case.
        return (None, None);
    };
    if numbers.next().is_some() {
        return (None, Some(Refusal::Several));
    }

    // Whereas these two say nothing on their own: a "কেজি" with no number in
    // front of it is a word out of an item's name, not a claim about how many.
    // So they are read only once a number has been found.
    if says(&Sense::Money) {
        return (None, Some(Refusal::Money));
    }
    if says(&Sense::Measure) {
        return (None, Some(Refusal::Measure));
    }

    if !counted_on_the_number && !says(&Sense::Counter) {
        return (None, Some(Refusal::Bare { count: value }));
    }
    if value == 0 || value > MOST_A_COUNT_MAY_BE {
        return (None, Some(Refusal::TooMany { count: value }));
    }
    (Some(value), None)
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use alloc::string::ToString;
    use alloc::vec::Vec;

    use super::{understand, Refusal, MOST_A_COUNT_MAY_BE};
    use crate::money::Milli;

    fn terms(said: &str) -> Vec<alloc::string::String> {
        understand(said)
            .terms
            .iter()
            .map(|t| t.to_string())
            .collect()
    }

    /// The shape the whole feature exists for.
    #[test]
    fn three_packets_of_rice_is_three() {
        let heard = understand("তিন প্যাকেট চাল");
        assert_eq!(heard.count, Some(3));
        assert_eq!(heard.quantity(), Milli::new(3_000));
        assert_eq!(terms("তিন প্যাকেট চাল"), ["প্যাকেট", "চাল"]);
    }

    /// Bangla writes the counter onto the number, and a till that only knew the
    /// separated form would miss the way most people say it.
    #[test]
    fn a_counter_stuck_to_the_number_still_counts() {
        assert_eq!(understand("তিনটা চাল").count, Some(3));
        assert_eq!(understand("দুটো সাবান").count, Some(2));
        assert_eq!(understand("৫টি ডিম").count, Some(5));
    }

    /// The demo catalogue's own first item, read off the packet.
    ///
    /// "মিনিকেট চাল ৫ কেজি" is the name of a five kilo bag. A till that read the
    /// five as a quantity would ring five bags at 2,150 for a customer buying
    /// one at 430, on the most ordinary sentence in the shop.
    #[test]
    fn reading_a_packet_aloud_does_not_multiply_it() {
        let heard = understand("মিনিকেট চাল ৫ কেজি");
        assert_eq!(heard.count, None, "five bags for a customer buying one");
        assert_eq!(heard.refused, Some(Refusal::Measure));
        assert_eq!(heard.quantity(), Milli::ONE);
    }

    /// And the number stays a word to search on, because in that sentence it is
    /// part of the name: dropping it would lose the only thing separating a five
    /// kilo bag from a two kilo one.
    #[test]
    fn a_refused_number_is_still_something_to_look_up_by() {
        assert!(terms("মিনিকেট চাল ৫ কেজি").contains(&"5".to_string()));
    }

    /// A hundred taka of rice is not a hundred bags of rice. This is how a large
    /// part of a Bangladeshi counter actually asks for things.
    #[test]
    fn a_hundred_taka_of_rice_is_not_a_hundred_of_it() {
        let heard = understand("একশ টাকার চাল");
        assert_eq!(heard.count, None);
        assert_eq!(heard.refused, Some(Refusal::Money));
    }

    /// A weight is refused, and the reason is not the recogniser: nothing in
    /// this system says how much of an item one of it is.
    #[test]
    fn a_weight_is_refused_rather_than_guessed_at() {
        assert_eq!(understand("দুই কেজি চিনি").refused, Some(Refusal::Measure));
        assert_eq!(understand("৫০০ গ্রাম চা").refused, Some(Refusal::Measure));
    }

    /// Fractions are modifiers, not amounts: "সাড়ে তিন" is three and a half, so
    /// reading either word alone gives a wrong number rather than none.
    #[test]
    fn part_of_a_unit_is_refused() {
        assert_eq!(understand("আধা কেজি চিনি").refused, Some(Refusal::Fraction));
        assert_eq!(understand("সাড়ে তিন কেজি চাল").refused, Some(Refusal::Fraction));
        assert_eq!(understand("দেড় লিটার তেল").refused, Some(Refusal::Fraction));
    }

    /// A hali is four eggs, and whether four eggs is four of the item or one of
    /// it is a fact about how the shop entered it.
    #[test]
    fn a_set_is_refused_because_its_size_is_the_shops_business() {
        assert_eq!(understand("দুই হালি ডিম").refused, Some(Refusal::Set));
        assert_eq!(understand("এক ডজন ডিম").refused, Some(Refusal::Set));
    }

    #[test]
    fn two_numbers_say_nothing_about_which_is_the_quantity() {
        assert_eq!(understand("দুইটা চাল ৫ কেজি").refused, Some(Refusal::Several));
    }

    #[test]
    fn a_number_counting_nothing_is_refused() {
        assert_eq!(understand("৩ চাল").refused, Some(Refusal::Bare { count: 3 }));
    }

    #[test]
    fn a_count_above_the_ceiling_is_refused() {
        let over = MOST_A_COUNT_MAY_BE + 1;
        let said = alloc::format!("{over}টা চাল");
        assert_eq!(understand(&said).refused, Some(Refusal::TooMany { count: over }));
        let at = alloc::format!("{MOST_A_COUNT_MAY_BE}টা চাল");
        assert_eq!(understand(&at).count, Some(MOST_A_COUNT_MAY_BE));
    }

    /// Nothing said about how many is not a refusal. It is every scan the till
    /// has ever taken.
    #[test]
    fn saying_no_number_is_not_a_refusal() {
        let heard = understand("মিনিকেট চাল");
        assert_eq!(heard.count, None);
        assert_eq!(heard.refused, None);
        assert_eq!(heard.quantity(), Milli::ONE);
    }

    /// Politeness is not evidence. It has to go before anything is searched for,
    /// because a word the catalogue has never seen is the strongest signal there
    /// is that the till misheard, and "ভাই" would be a false one.
    #[test]
    fn politeness_is_set_aside_and_shown() {
        let heard = understand("ভাই একটু চাল দাও");
        assert_eq!(terms("ভাই একটু চাল দাও"), ["চাল"]);
        assert_eq!(
            heard
                .ignored
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>(),
            ["ভাই", "একটু", "দাও"]
        );
    }

    /// A recogniser emits whatever it likes, and none of it is an error.
    #[test]
    fn an_empty_or_meaningless_transcript_is_not_a_refusal() {
        for said in ["", "   ", "\u{200d}", "৳৳৳", "???"] {
            let heard = understand(said);
            assert!(heard.is_empty(), "{said:?} should carry nothing to look up");
            assert_eq!(heard.count, None);
            assert_eq!(heard.refused, None);
            assert_eq!(heard.quantity(), Milli::ONE);
        }
    }

    /// A paragraph from a hot mic must not decide how long the till thinks for.
    #[test]
    fn a_rambling_transcript_is_bounded() {
        let long = "চাল ".repeat(500);
        assert!(understand(&long).terms.len() <= super::MOST_TERMS_KEPT);
    }

    /// Whatever else is refused, the quantity a caller gets is one, and one is
    /// always safe: it is what a scan gives.
    #[test]
    fn a_refusal_always_leaves_one() {
        for said in [
            "একশ টাকার চাল",
            "দুই কেজি চিনি",
            "আধা কেজি চিনি",
            "দুই হালি ডিম",
            "দুইটা চাল ৫ কেজি",
            "৩ চাল",
            "৫০০টা চাল",
        ] {
            let heard = understand(said);
            assert!(heard.refused.is_some(), "{said:?} should be refused");
            assert_eq!(heard.count, None);
            assert_eq!(heard.quantity(), Milli::ONE, "{said:?}");
            assert!(!heard.refused.unwrap().to_string().is_empty());
        }
    }

    /// No word may sit in two tables. One that did would be read as whichever
    /// list is checked first, and the reason would be invisible in the source.
    #[test]
    fn no_word_belongs_to_two_tables() {
        let tables = super::bangla::tables();
        for (index, (name, words)) in tables.iter().enumerate() {
            for word in words.iter() {
                for (other_name, other) in tables.iter().skip(index + 1) {
                    assert!(
                        !other.contains(word),
                        "{word} is in both {name} and {other_name}"
                    );
                }
                assert!(
                    super::bangla::numerals().iter().all(|(n, _)| n != word),
                    "{word} is in {name} and is also a numeral"
                );
            }
        }
    }

    /// Every table entry must survive the folding its input has already been
    /// through, or it is an entry that can never match anything.
    #[test]
    fn every_word_in_every_table_is_already_folded() {
        for (name, words) in super::bangla::tables() {
            for word in words {
                assert_eq!(
                    crate::replica::normalise(word).trim(),
                    *word,
                    "{word} in {name} is not written the way normalise leaves it"
                );
            }
        }
        for (word, _) in super::bangla::numerals() {
            assert_eq!(crate::replica::normalise(word).trim(), *word, "numeral {word}");
        }
    }
}
