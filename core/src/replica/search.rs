//! Offline catalogue search.
//!
//! A cashier types two or three characters and expects the item. There is no
//! server to ask, so the index has to be here, and it has to be cheap enough to
//! run on every keystroke on a low-end tablet.
//!
//! The index is a sorted vector of `(token, item index)` pairs. A prefix query is
//! a binary search for the lower bound plus a walk while the prefix still
//! matches. That beats a hash map for this job: prefixes fall out of the
//! ordering, it is one contiguous allocation rather than tens of thousands of
//! small ones, and the walk is sequential memory access.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Item, Replica};

/// Shortest token worth indexing. Whole words are indexed and matched by prefix
/// at query time; storing every prefix of every word would multiply the index
/// size for no gain, because cashiers refine by typing more rather than less.
const MIN_TOKEN_LEN: usize = 1;

/// Bengali digit zero. The ten that follow it are folded onto the Latin ones.
const BENGALI_ZERO: u32 = 0x09E6;

/// The two marks that hold a Bengali word together rather than break it.
///
/// U+09CD is the hasant, which is what makes a conjunct: without it "মিষ্টি" is
/// not one word but "মিষ" and "টি". U+09BC is the nukta, which is what makes ড়
/// out of ড. Neither carries the `Alphabetic` property, so `is_alphanumeric`
/// says no to both and every conjunct in the catalogue was being indexed as two
/// or three fragments.
///
/// Typed search survived that by accident, because it intersects its terms and
/// the fragments of one word co-occur on the same item. The Bangla test in this
/// file passed for that reason and proved nothing: it searched a word that was
/// split the same way on both sides.
const fn joins_a_word(ch: char) -> bool {
    matches!(ch, '\u{09BC}' | '\u{09CD}')
}

/// The three Bengali letters written both as one codepoint and as a letter plus
/// a nukta.
///
/// Unicode lists them as composition exclusions, so its own normal form is the
/// two-codepoint one and `NFC` will not put them back together. That leaves the
/// same word, rendered identically and indistinguishable to a shopkeeper, as two
/// different byte strings: "মুড়ি" written one way is not a prefix of "মুড়ি"
/// written the other, so a shop could type the name off its own shelf label and
/// be told there is no such item. Both spellings are folded to the same one here.
const fn splits_off_a_nukta(ch: char) -> Option<(char, char)> {
    match ch {
        '\u{09DC}' => Some(('\u{09A1}', '\u{09BC}')),
        '\u{09DD}' => Some(('\u{09A2}', '\u{09BC}')),
        '\u{09DF}' => Some(('\u{09AF}', '\u{09BC}')),
        _ => None,
    }
}

/// A Bengali digit as its Latin twin, if that is what this is.
fn latin_digit(ch: char) -> Option<char> {
    let offset = (ch as u32).checked_sub(BENGALI_ZERO)?;
    if offset > 9 {
        return None;
    }
    char::from_u32(('0' as u32).checked_add(offset)?)
}

/// Fold a string into comparable tokens.
///
/// ASCII is lowercased so "RICE" and "rice" match. Bangla has no case, but it
/// has three other things that decide whether two spellings of one word meet:
/// the marks that join a word, the two encodings of ড়ঢ়য়, and its own digits.
/// A catalogue here carries an English name with Latin digits beside a Bangla
/// name with Bengali ones, and without the last fold those are two separate
/// shops: "৫" never found "Rice Miniket 5kg" and "5" never found "মিনিকেট চাল
/// ৫ কেজি".
#[must_use]
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        // Invisible, and never the difference between two products. Dropped
        // rather than turned into a space, because a joiner sitting inside a
        // word is not a break in one.
        if matches!(ch, '\u{200C}' | '\u{200D}') {
            continue;
        }
        if let Some((base, nukta)) = splits_off_a_nukta(ch) {
            out.push(base);
            out.push(nukta);
        } else if let Some(digit) = latin_digit(ch) {
            out.push(digit);
        } else if ch.is_alphanumeric() || joins_a_word(ch) {
            for lowered in ch.to_lowercase() {
                out.push(lowered);
            }
        } else {
            out.push(' ');
        }
    }
    out
}

fn tokens_of(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().filter(|t| t.len() >= MIN_TOKEN_LEN)
}

/// Build the sorted token index for a whole catalogue.
pub(super) fn build_index(items: &[Item]) -> Vec<(Box<str>, u32)> {
    let mut pairs: Vec<(Box<str>, u32)> = Vec::with_capacity(items.len().saturating_mul(4));

    for (index, item) in items.iter().enumerate() {
        let Ok(position) = u32::try_from(index) else {
            // A catalogue beyond four billion items is not a till problem.
            break;
        };
        let mut push_tokens = |text: &str| {
            let normalised = normalise(text);
            for token in tokens_of(&normalised) {
                pairs.push((token.into(), position));
            }
        };
        push_tokens(&item.name_en);
        push_tokens(&item.name_bn);
        push_tokens(&item.code);
    }

    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// Run a query against the index.
///
/// Every term must match some token of the item, so terms narrow rather than
/// widen. Results keep catalogue order, which keeps the list stable as the
/// cashier types instead of reshuffling under their finger.
pub(super) fn run<'a>(replica: &'a Replica, query: &str, limit: usize) -> Vec<&'a Item> {
    let normalised = normalise(query);
    let mut terms = tokens_of(&normalised).peekable();
    if terms.peek().is_none() || limit == 0 {
        return Vec::new();
    }

    let mut matches: Option<Vec<u32>> = None;
    for term in terms {
        let mut hits = positions_with_prefix(&replica.tokens, term);
        hits.sort_unstable();
        hits.dedup();

        matches = Some(match matches {
            None => hits,
            Some(previous) => intersect(&previous, &hits),
        });

        if matches.as_ref().is_some_and(Vec::is_empty) {
            return Vec::new();
        }
    }

    matches
        .unwrap_or_default()
        .into_iter()
        .filter_map(|position| replica.items.get(position as usize))
        .filter(|item| item.active)
        .take(limit)
        .collect()
}

/// One item, and how much of what was asked for it accounts for.
#[derive(Debug, Clone, Copy)]
pub struct Weighted {
    /// Where the item sits in the catalogue.
    pub(super) position: u32,
    /// The weight of the terms it matched, added up.
    pub score: u32,
    /// How many distinct terms it matched.
    pub matched: u32,
    /// How many items share the rarest term it matched. One means that term
    /// belongs to this item alone, which is the strongest thing a query can say.
    pub commonest: u32,
}

/// What a set of spoken words came to when weighed against the catalogue.
#[derive(Debug, Clone)]
pub struct Weighed {
    /// Best first, then catalogue order.
    pub candidates: Vec<Weighted>,
    /// The weight of everything that was asked for, including the words no item
    /// has ever heard of. Those cost the most, which is the point: a word the
    /// catalogue does not know is the strongest evidence there is that the
    /// utterance was misheard, and treating it as free would make guessing free.
    pub demand: u32,
}

/// How much one term is worth, given how many items carry it.
///
/// A term on one item in twenty thousand is worth far more than one on four
/// thousand, and the fall between them is what stops "তেল" from deciding
/// anything on its own. Integer `ilog2` rather than a real logarithm, and not
/// because floats are slow: the same core runs on wasm32 and on aarch64, and two
/// builds that rounded a score differently would disagree about which item the
/// cashier meant. Nothing about money may depend on the target.
fn weight_of(items: u32, sharing: u32) -> u32 {
    let items = items.max(1);
    let sharing = sharing.clamp(1, items);
    // `checked_div` rather than a plain one because the workspace denies any
    // arithmetic that can misbehave, and a divisor being provably non-zero three
    // lines up is not something the lint can see. The `max(1)` after it keeps
    // `ilog2` off zero for the same reason.
    let share = items.checked_div(sharing).unwrap_or(1).max(1);
    1_u32.saturating_add(share.ilog2())
}

/// Weigh spoken words against the catalogue.
///
/// Unlike [`run`], a term that matches nothing does not empty the result. A
/// cashier who types narrows on purpose and means every word; a recogniser puts
/// words in that nobody said. So this scores rather than intersects, and pays
/// for the difference by counting the misses against the total.
pub(super) fn weigh(replica: &Replica, terms: &[&str], limit: usize) -> Weighed {
    let Ok(count) = u32::try_from(replica.items.len()) else {
        return Weighed {
            candidates: Vec::new(),
            demand: 0,
        };
    };
    let unknown = weight_of(count, 1);

    // One slot per item, rather than a map: a query touches a large share of
    // them, the array is one allocation, and the walk is sequential. Eighty
    // kilobytes for twenty thousand items, once per utterance, is not a cost
    // worth a hash for.
    let mut tally: Vec<Weighted> = (0..count)
        .map(|position| Weighted {
            position,
            score: 0,
            matched: 0,
            commonest: u32::MAX,
        })
        .collect();

    let mut demand = 0_u32;
    let mut seen: Vec<&str> = Vec::new();
    for term in terms {
        // A word said twice is one piece of evidence, not two.
        if seen.contains(term) {
            continue;
        }
        seen.push(term);

        let mut hits = positions_with_prefix(&replica.tokens, term);
        hits.sort_unstable();
        hits.dedup();
        // Only the goods the shop still sells count towards how common a word
        // is. The index carries withdrawn items so a refund can still find
        // them, and letting those decide would make a word look shared when the
        // only thing that carries it now is one item on one shelf.
        hits.retain(|position| {
            replica
                .items
                .get(*position as usize)
                .is_some_and(|item| item.active)
        });
        let Ok(sharing) = u32::try_from(hits.len()) else {
            continue;
        };
        if sharing == 0 {
            demand = demand.saturating_add(unknown);
            continue;
        }

        let weight = weight_of(count, sharing);
        demand = demand.saturating_add(weight);
        for position in hits {
            let Some(slot) = tally.get_mut(position as usize) else {
                continue;
            };
            slot.score = slot.score.saturating_add(weight);
            slot.matched = slot.matched.saturating_add(1);
            slot.commonest = slot.commonest.min(sharing);
        }
    }

    let mut candidates: Vec<Weighted> = tally
        .into_iter()
        .filter(|slot| slot.matched > 0)
        .filter(|slot| {
            replica
                .items
                .get(slot.position as usize)
                .is_some_and(|item| item.active)
        })
        .collect();
    // Best first, and catalogue order between equals, so a screen does not
    // reshuffle under a finger and two items that tie always tie the same way.
    candidates.sort_by(|a, b| b.score.cmp(&a.score).then(a.position.cmp(&b.position)));
    // Truncated last. Cutting before the sort would let the catalogue's own
    // order decide the winner, which is the shop's data entry deciding what the
    // cashier meant.
    candidates.truncate(limit);

    Weighed { candidates, demand }
}

/// Item positions whose token starts with `prefix`.
///
/// Collects every match before the caller caps the result, so a very broad
/// prefix on a large catalogue does proportionally more work: measured at 112 us
/// for a two-letter prefix over 20,000 items, against a 16 ms keystroke budget.
/// Bounded early exit would break catalogue ordering, and the headroom does not
/// justify it yet.
fn positions_with_prefix(tokens: &[(Box<str>, u32)], prefix: &str) -> Vec<u32> {
    let start = tokens.partition_point(|(token, _)| token.as_ref() < prefix);
    let mut found = Vec::new();
    for (token, position) in tokens.iter().skip(start) {
        if !token.starts_with(prefix) {
            break;
        }
        found.push(*position);
    }
    found
}

/// Intersection of two sorted, deduplicated lists.
fn intersect(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0_usize, 0_usize);
    while let (Some(a), Some(b)) = (left.get(i), right.get(j)) {
        match a.cmp(b) {
            core::cmp::Ordering::Equal => {
                out.push(*a);
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            core::cmp::Ordering::Less => i = i.saturating_add(1),
            core::cmp::Ordering::Greater => j = j.saturating_add(1),
        }
    }
    out
}

/// The token index, exposed for tests and for storage to snapshot alongside the
/// items if it ever becomes worth persisting rather than rebuilding.
impl Replica {
    #[must_use]
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use alloc::string::ToString;
    use alloc::vec;

    use super::super::tests::item;
    use super::super::{ItemDelta, Replica, DEFAULT_SEARCH_LIMIT};

    fn catalogue() -> Replica {
        Replica::from_items(vec![
            item(1, "SKU001", "Rice Miniket 5kg", "8690000000012"),
            item(2, "SKU002", "Rice Nazirshail 5kg", "8690000000029"),
            item(3, "SKU003", "Soybean Oil 2L", "8690000000036"),
            item(4, "SKU004", "Mustard Oil 500ml", "8690000000043"),
        ])
    }

    fn codes(items: &[&super::Item]) -> alloc::vec::Vec<alloc::string::String> {
        items.iter().map(|i| i.code.to_string()).collect()
    }

    #[test]
    fn finds_by_name_prefix() {
        let replica = catalogue();
        let found = replica.search("ric", DEFAULT_SEARCH_LIMIT);
        assert_eq!(codes(&found), vec!["SKU001", "SKU002"]);
    }

    #[test]
    fn narrows_as_the_cashier_types_more() {
        let replica = catalogue();
        assert_eq!(codes(&replica.search("oil", DEFAULT_SEARCH_LIMIT)), vec!["SKU003", "SKU004"]);
        assert_eq!(codes(&replica.search("oil mus", DEFAULT_SEARCH_LIMIT)), vec!["SKU004"]);
    }

    #[test]
    fn ignores_case_and_punctuation() {
        let replica = catalogue();
        assert_eq!(codes(&replica.search("RICE, min", DEFAULT_SEARCH_LIMIT)), vec!["SKU001"]);
    }

    #[test]
    fn finds_by_code() {
        let replica = catalogue();
        assert_eq!(codes(&replica.search("sku004", DEFAULT_SEARCH_LIMIT)), vec!["SKU004"]);
    }

    /// The same item, named in Bangla the way a back office would type it.
    fn named_in_bangla(seed: u128, code: &str, name_bn: &str) -> super::Item {
        let mut it = item(seed, code, "Placeholder", "8690000000000");
        it.name_bn = name_bn.into();
        it
    }

    /// A conjunct is one word.
    ///
    /// The hasant is not `Alphabetic`, so it used to become a space and
    /// "মিষ্টি" was indexed as "মিষ" and "টি". A shop typing the word it can see
    /// on the packet got nothing, because "মিষ্টি" as a query was split the same
    /// way and then required both fragments as separate prefixes of separate
    /// tokens.
    #[test]
    fn a_conjunct_is_one_word_and_not_two() {
        assert_eq!(
            super::normalise("মিষ্টি").split_whitespace().count(),
            1,
            "the hasant broke the word in half"
        );
        let replica = Replica::from_items(vec![named_in_bangla(10, "SWT1", "মিষ্টি দই ৫০০ গ্রাম")]);
        assert_eq!(codes(&replica.search("মিষ্টি", DEFAULT_SEARCH_LIMIT)), vec!["SWT1"]);
    }

    /// Two spellings of one word find the same item.
    ///
    /// ড় is written both as U+09DC and as ড plus a nukta, and Unicode excludes
    /// it from composition, so neither spelling is a prefix of the other and NFC
    /// does not reconcile them. A shop whose catalogue was typed on one keyboard
    /// and searched from another would be told its own stock does not exist.
    #[test]
    fn one_word_spelled_two_ways_finds_the_same_item() {
        let precomposed = "\u{09AE}\u{09C1}\u{09DC}\u{09BF}";
        let decomposed = "\u{09AE}\u{09C1}\u{09A1}\u{09BC}\u{09BF}";
        assert_ne!(precomposed, decomposed, "these are different bytes");
        assert_eq!(
            super::normalise(precomposed),
            super::normalise(decomposed),
            "and they must fold to one word"
        );

        let replica = Replica::from_items(vec![named_in_bangla(11, "MURI1", precomposed)]);
        assert_eq!(codes(&replica.search(decomposed, DEFAULT_SEARCH_LIMIT)), vec!["MURI1"]);
        assert_eq!(codes(&replica.search(precomposed, DEFAULT_SEARCH_LIMIT)), vec!["MURI1"]);
    }

    /// A catalogue carries an English name with Latin digits beside a Bangla one
    /// with Bengali digits. Until these were folded together they were two
    /// separate shops, and neither could see the other's numbers.
    #[test]
    fn bengali_and_latin_digits_are_the_same_number() {
        let replica = Replica::from_items(vec![named_in_bangla(
            12,
            "RICE5",
            "মিনিকেট চাল ৫ কেজি",
        )]);
        assert_eq!(
            codes(&replica.search("৫", DEFAULT_SEARCH_LIMIT)),
            vec!["RICE5"],
            "a Bengali digit must reach a name written with one"
        );
        assert_eq!(
            codes(&replica.search("5", DEFAULT_SEARCH_LIMIT)),
            vec!["RICE5"],
            "and a Latin digit must reach it too"
        );
    }

    /// A joiner is invisible and never tells two products apart, so it must not
    /// be read as the gap between two words.
    #[test]
    fn an_invisible_joiner_does_not_split_a_word() {
        assert_eq!(super::normalise("চা\u{200C}ল"), super::normalise("চাল"));
    }

    #[test]
    fn searches_bangla_names() {
        let replica = catalogue();
        // every sample item carries the same Bangla name, so this proves the
        // script is tokenised at all rather than dropped
        assert_eq!(replica.search("পণ্য", DEFAULT_SEARCH_LIMIT).len(), 4);
    }

    #[test]
    fn returns_nothing_for_an_empty_or_unmatched_query() {
        let replica = catalogue();
        assert!(replica.search("", DEFAULT_SEARCH_LIMIT).is_empty());
        assert!(replica.search("   ", DEFAULT_SEARCH_LIMIT).is_empty());
        assert!(replica.search("zzzz", DEFAULT_SEARCH_LIMIT).is_empty());
    }

    #[test]
    fn respects_the_limit() {
        let replica = catalogue();
        // all four items share this Bangla name, so the limit is what trims it
        assert_eq!(replica.search("পণ্য", DEFAULT_SEARCH_LIMIT).len(), 4);
        assert_eq!(replica.search("পণ্য", 2).len(), 2);
        assert_eq!(replica.search("পণ্য", 0).len(), 0);
    }

    #[test]
    fn hides_inactive_items() {
        let mut replica = catalogue();
        let mut retired = item(3, "SKU003", "Soybean Oil 2L", "8690000000036");
        retired.active = false;
        replica.apply([ItemDelta::Upsert(retired)]);
        assert_eq!(codes(&replica.search("oil", DEFAULT_SEARCH_LIMIT)), vec!["SKU004"]);
    }

    #[test]
    fn reindexes_after_a_delta_batch() {
        let mut replica = catalogue();
        replica.apply([ItemDelta::Upsert(item(5, "SKU005", "Red Lentil 1kg", "8690000000050"))]);
        assert_eq!(codes(&replica.search("lentil", DEFAULT_SEARCH_LIMIT)), vec!["SKU005"]);

        replica.apply([ItemDelta::Tombstone(crate::ids::Ulid::from_u128(5))]);
        assert!(replica.search("lentil", DEFAULT_SEARCH_LIMIT).is_empty());
    }
}
