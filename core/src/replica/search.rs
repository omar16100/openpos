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

/// Fold a string into comparable tokens.
///
/// ASCII is lowercased so "RICE" and "rice" match. Bangla has no case, so it
/// passes through unchanged; the tokens are still split on whitespace and
/// punctuation, which is what makes "চাল" findable inside a longer name.
#[must_use]
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects
    )]

    use alloc::string::ToString;
    use alloc::vec;

    use super::super::tests::item;
    use super::super::{DEFAULT_SEARCH_LIMIT, ItemDelta, Replica};

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
        assert_eq!(
            codes(&replica.search("oil", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU003", "SKU004"]
        );
        assert_eq!(
            codes(&replica.search("oil mus", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU004"]
        );
    }

    #[test]
    fn ignores_case_and_punctuation() {
        let replica = catalogue();
        assert_eq!(
            codes(&replica.search("RICE, min", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU001"]
        );
    }

    #[test]
    fn finds_by_code() {
        let replica = catalogue();
        assert_eq!(
            codes(&replica.search("sku004", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU004"]
        );
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
        assert_eq!(
            codes(&replica.search("oil", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU004"]
        );
    }

    #[test]
    fn reindexes_after_a_delta_batch() {
        let mut replica = catalogue();
        replica.apply([ItemDelta::Upsert(item(
            5,
            "SKU005",
            "Red Lentil 1kg",
            "8690000000050",
        ))]);
        assert_eq!(
            codes(&replica.search("lentil", DEFAULT_SEARCH_LIMIT)),
            vec!["SKU005"]
        );

        replica.apply([ItemDelta::Tombstone(crate::ids::Ulid::from_u128(5))]);
        assert!(replica.search("lentil", DEFAULT_SEARCH_LIMIT).is_empty());
    }
}
