//! The Bangla a counter is spoken in.
//!
//! Tables rather than code, because which words a shop uses is a fact about the
//! shop and not about this crate: a dialect that says "পিছ" where another says
//! "পিস" should be a line added here, not a function rewritten.
//!
//! Every table is matched against the output of [`crate::replica::normalise`],
//! so the words below are written the way that leaves them: Bengali digits
//! already folded to Latin ones, conjuncts whole, and the two spellings of ড়ঢ়য়
//! already reconciled. Writing them any other way would mean entries that can
//! never match, which is the quietest kind of wrong.
//!
//! What is deliberately absent is as important as what is here. There is no
//! table of the eighty-nine irregular words for eleven to ninety-nine. A numeral
//! this file does not know is not guessed at: it stays an ordinary search term,
//! matches nothing, and the utterance ends up with no quantity, which is the
//! answer a till should give when it does not know. Numbers the recogniser
//! writes as digits are read as digits and need no table at all.

/// What a word is, once it has been looked up.
///
/// Everything not in a table is a [`Sense::Word`], which is to say a piece of
/// the thing the cashier is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sense {
    /// A whole number, from a word or from digits.
    Number(u32),
    /// Says the number in front of it counts separate things: "তিনটা", "দুই পিস".
    Counter,
    /// A weight or a volume. Refused, not guessed at: see [`super::Refusal`].
    Measure,
    /// Money. "একশ টাকার চাল" is a hundred taka of rice, not a hundred of it.
    Money,
    /// Half, one and a half, two and a half. Refused for the same reason.
    Fraction,
    /// A set whose size is a fact about the item, not about the language.
    Set,
    /// Politeness and grammar. Dropped, because an unmatched term is evidence
    /// the utterance was not understood and these would be false evidence.
    Filler,
    /// Part of what is being asked for.
    Word,
}

/// Numerals this file is willing to read.
///
/// One to twenty and the tens, which is what a person counting goods off a shelf
/// says out loud, plus the hundreds and the thousand. The large ones earn their
/// place by being refusable: "একশ" reaching [`Sense::Number`] is what lets a till
/// say a hundred is too many to be a count, where leaving it out would let it
/// pass silently as a word.
const NUMERALS: &[(&str, u32)] = &[
    ("এক", 1),
    ("দুই", 2),
    ("দু", 2),
    ("তিন", 3),
    ("চার", 4),
    ("পাঁচ", 5),
    ("ছয়", 6),
    ("সাত", 7),
    ("আট", 8),
    ("নয়", 9),
    ("দশ", 10),
    ("এগারো", 11),
    ("বারো", 12),
    ("তেরো", 13),
    ("চৌদ্দ", 14),
    ("পনেরো", 15),
    ("ষোলো", 16),
    ("সতেরো", 17),
    ("আঠারো", 18),
    ("উনিশ", 19),
    ("বিশ", 20),
    ("ত্রিশ", 30),
    ("চল্লিশ", 40),
    ("পঞ্চাশ", 50),
    ("ষাট", 60),
    ("সত্তর", 70),
    ("আশি", 80),
    ("নব্বই", 90),
    ("একশ", 100),
    ("একশো", 100),
    ("দুইশ", 200),
    ("দুশো", 200),
    ("তিনশ", 300),
    ("চারশ", 400),
    ("চারশো", 400),
    ("পাঁচশ", 500),
    ("পাঁচশো", 500),
    ("হাজার", 1_000),
];

/// The endings that turn a numeral into a count of things: "তিন" becomes
/// "তিনটা". Bangla writes them onto the number, so a token has to be split
/// before either half can be recognised.
///
/// Longest first, because "খানা" and "খান" both appear and stripping the shorter
/// one first would leave an "া" behind and make the numeral unrecognisable.
///
/// "টো" is here rather than in the numerals: "দুটো" is two with the counter
/// already written on, and listing it as a plain numeral made it a bare number
/// that the till then refused to count, which is the opposite of what the word
/// means.
const COUNTER_SUFFIXES: &[&str] =
    &["গুলো", "গুলা", "খানা", "খানি", "খান", "টা", "টি", "টে", "টো"];

/// Words that stand alone and say the number counts things.
const COUNTERS: &[&str] = &[
    "পিস", "পিছ", "প্যাকেট", "প্যাকেটে", "বোতল", "কৌটা", "কার্টন", "বক্স", "বস্তা", "টুকরা", "টুকরো",
];

/// Weights and volumes.
///
/// Refused rather than read, because turning one into a quantity needs a fact
/// about the item that this system has never recorded: `Item.unit` is prose, so
/// nothing says whether a thing is sold by the kilo or by the packet, nor how
/// many grams are in one of it. Five hundred grams against a five hundred gram
/// packet and against loose goods are a thousand times apart.
const MEASURES: &[&str] = &[
    "কেজি",
    "কিলো",
    "কিলোগ্রাম",
    "গ্রাম",
    "গ্ৰাম",
    "লিটার",
    "লিটাৰ",
    "মিলি",
    "মিলিলিটার",
    "এমএল",
    "ml",
    "kg",
    "মণ",
    "সের",
    "ছটাক",
];

/// Money. A shop is asked for a hundred taka of something at least as often as
/// for a hundred of it, and the two differ by the price.
const MONEY: &[&str] = &["টাকা", "টাকার", "টাকায়", "টেকা", "টেকার", "পয়সা"];

/// Halves and quarters.
///
/// Refused for the same reason as a measure, and with one of its own: সাড়ে and
/// পৌনে are modifiers rather than amounts, so "সাড়ে তিন" is three and a half and
/// reading either word alone gives the wrong number rather than no number.
const FRACTIONS: &[&str] = &[
    "আধা", "আধ", "হাফ", "পোয়া", "সোয়া", "দেড়", "দেড়শ", "আড়াই", "আড়াইশ", "সাড়ে", "পৌনে",
];

/// Sets whose size is a property of how the shop entered the item.
///
/// A হালি is four, and whether four eggs is four of the item or one of it
/// depends on whether the shop sells eggs by the egg or by the hali. Nothing
/// here can tell, so nothing here decides.
const SETS: &[&str] = &["হালি", "হালী", "ডজন", "ডজনে", "জোড়া"];

/// Politeness, address and the verbs of asking.
///
/// Dropped before anything is searched for. They have to go: an unmatched term
/// is the strongest evidence there is that an utterance was not understood, and
/// a cashier saying "ভাই" is not evidence of anything.
const FILLERS: &[&str] = &[
    "দাও", "দেও", "দেন", "দিন", "দে", "দিবেন", "নাও", "নেন", "নিব", "আর", "একটু", "ভাই", "ভাইয়া",
    "আপা", "আপু", "মামা", "চাচা", "প্লিজ", "দয়া", "করে", "তো", "একদম", "লাগবে", "চাই", "আছে",
    "আছেনি", "কি", "একটুখানি",
];

/// Read a token as a whole number of digits, if that is all it is.
///
/// The recogniser writes numbers either way, and [`crate::replica::normalise`]
/// has already folded Bengali digits onto Latin ones, so this covers both
/// scripts and every value, which is why the word table above can stop at a
/// thousand without leaving a hole.
fn digits(token: &str) -> Option<u32> {
    if token.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for ch in token.chars() {
        let digit = ch.to_digit(10)?;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

fn numeral(token: &str) -> Option<u32> {
    NUMERALS
        .iter()
        .find(|(word, _)| *word == token)
        .map(|(_, value)| *value)
}

/// Split a token into the numeral it starts with and the counter stuck to it,
/// so "তিনটা" is three of something rather than a word nothing has heard of.
fn numeral_with_counter(token: &str) -> Option<u32> {
    for suffix in COUNTER_SUFFIXES {
        // Not `?`: a suffix that does not match is the ordinary case, and
        // giving up on the first one would mean only "গুলো" was ever tried.
        let Some(stem) = token.strip_suffix(suffix) else {
            continue;
        };
        if let Some(value) = numeral(stem) {
            return Some(value);
        }
        if let Some(value) = digits(stem) {
            return Some(value);
        }
    }
    None
}

/// What one token means, and whether a counter was stuck to it.
///
/// The second half of the pair is what tells "তিন চাল" from "তিনটা চাল": both
/// carry the number three, and only the second says the three counts things.
#[must_use]
pub fn sense_of(token: &str) -> (Sense, bool) {
    if let Some(value) = digits(token) {
        return (Sense::Number(value), false);
    }
    if let Some(value) = numeral(token) {
        return (Sense::Number(value), false);
    }
    if let Some(value) = numeral_with_counter(token) {
        return (Sense::Number(value), true);
    }
    // Order matters below only in that no word appears in two tables. Checked by
    // a test, because a word that did would be read as whichever list came first
    // and the reason would be invisible.
    if MONEY.contains(&token) {
        return (Sense::Money, false);
    }
    if FRACTIONS.contains(&token) {
        return (Sense::Fraction, false);
    }
    if SETS.contains(&token) {
        return (Sense::Set, false);
    }
    if MEASURES.contains(&token) {
        return (Sense::Measure, false);
    }
    if COUNTERS.contains(&token) {
        return (Sense::Counter, false);
    }
    if FILLERS.contains(&token) {
        return (Sense::Filler, false);
    }
    (Sense::Word, false)
}

/// Every table, for the tests that hold them to their invariants.
#[cfg(test)]
#[must_use]
pub(super) const fn tables() -> [(&'static str, &'static [&'static str]); 6] {
    [
        ("money", MONEY),
        ("fractions", FRACTIONS),
        ("sets", SETS),
        ("measures", MEASURES),
        ("counters", COUNTERS),
        ("fillers", FILLERS),
    ]
}

/// The numerals, for the same reason.
#[cfg(test)]
#[must_use]
pub(super) fn numerals() -> &'static [(&'static str, u32)] {
    NUMERALS
}
