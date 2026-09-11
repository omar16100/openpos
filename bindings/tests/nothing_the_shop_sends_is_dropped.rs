//! Every field a reply carries reaches the shape a screen is handed, or is
//! written down here as not carried.
//!
//! Six times this month a lower layer did something careful and the last hop
//! threw it away. Twice it was exactly this: a reply carried a field, the
//! bindings' own copy of that row did not have it, so the reply decoded, the
//! field vanished, and nothing anywhere complained. The trail carried which
//! receipt a reprint was of and the screen showed none. The return carried
//! which kind of nothing a zero-rated line is and the screen printed every line
//! as a percentage, which is the one distinction that screen exists for. Both
//! were found by a person looking at a screen, weeks apart.
//!
//! The pairs are not written down: they are read out of the code that does the
//! work. An arm decodes one reply and builds the rows a screen holds, so the
//! reply's rows and the shapes built in that arm belong together by
//! construction, and a pair that went stale would be a pair that no longer
//! compiles.
//!
//! What this cannot see is a field carried into the bindings and then not
//! rendered. That is the next hop and it has its own tests. What it closes is
//! the hop where a field silently ceases to exist.
//!
//! The half-way state needs nothing from here: a field taken off one of these
//! shapes while the code still fills it in does not compile. What compiles, and
//! what happened twice, is a field that arrives and is never mentioned at all.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

/// Fields a reply carries that the screens are deliberately not given, and why.
///
/// Every line is a claim that a shopkeeper loses nothing by it. They are ids
/// the screen has no use for, and figures nobody has asked for yet.
const NOT_CARRIED: &[(&str, &str, &str)] = &[
    (
        "AllowedEntry",
        "action",
        "carried as `kind`, because the screen says it in the shop's own words and the number is \
         what it looks the words up by",
    ),
    (
        "AllowedEntry",
        "operator",
        "the name is carried and the id is not: the trail is read by a person, and somebody since \
         renamed or gone from the shop is still who that entry belongs to",
    ),
    (
        "AllowedEntry",
        "authorised_by",
        "the same, for whoever allowed it. The name at the time is what a shop reads",
    ),
    (
        "ClosedShiftWire",
        "closed_by",
        "the name goes with it and the id does not, for the reason the trail's does",
    ),
    (
        "RepairEntry",
        "held_for",
        "carried as `kind` and `parts`, which is the reason itself and the figures in it, so a \
         screen can say it in the shop's language rather than showing an English sentence",
    ),
    (
        "OpenDrawerWire",
        "shift",
        "which drawer session it is. The screen shows the till, when it opened and what it holds, \
         and nothing it offers is per session",
    ),
    (
        "ReceiptGapWire",
        "epoch",
        "which run of numbers the gap is in. The screen shows the numbers either side of it, which \
         is what an inspector asks about; the epoch is how the shop's own query grouped them",
    ),
    (
        "TerminalHealthEntry",
        "terminal",
        "carried as `id`, in the text a person can read back rather than as the number the shop \
         stores: it is what every other device on that screen is named by",
    ),
    (
        "TerminalHealthEntry",
        "epoch",
        "how many times that till has been given a fresh block of numbers. A shop reads the list \
         to see which devices are alive, and this is not that",
    ),
];

/// Replies whose rows are turned into what a screen holds somewhere other than
/// the arm that decodes them, and where that is.
///
/// Named rather than skipped, because "somewhere else does it" is exactly what
/// a dropped field looks like from here.
const CONVERTED_ELSEWHERE: &[(&str, &str)] = &[
    ("PullResponse", "WireItem::from_wire, with the catalogue"),
    ("ItemNowResponse", "WireItem::from_wire"),
    ("TillItemsResponse", "WireItem::from_wire"),
    ("OperatorsResponse", "people_from, into the till's own operators"),
    ("CustomersResponse", "into the till's own customers"),
    ("BalancesResponse", "into the till's own balances"),
    ("RepairQueueResponse", "mapped with held_for beside it"),
    ("PushResponse", "PushResponse::settled, into what the outbox may drop"),
    ("AdoptSalesResponse", "the same, for sales carried in by hand"),
    ("ReceiveGoodsResponse", "the screen asks the shelf again, which is the same answer"),
];

/// Struct name to its field names, from a Rust source.
fn structs(source: &str) -> BTreeMap<String, Vec<String>> {
    let mut found = BTreeMap::new();
    let mut name: Option<String> = None;
    let mut fields: Vec<String> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            if trimmed.ends_with('{') {
                name = Some(
                    rest.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches('{')
                        .to_owned(),
                );
                fields.clear();
            }
            continue;
        }
        if trimmed == "}" {
            if let Some(held) = name.take() {
                found.insert(held, core::mem::take(&mut fields));
            }
            continue;
        }
        if name.is_none() {
            continue;
        }
        if let Some(field) = trimmed.strip_prefix("pub ")
            && let Some((named, _)) = field.split_once(':') {
                fields.push(named.to_owned());
            }
    }
    found
}

/// The types each struct's fields are written as, which is how one shape says
/// it carries another.
fn field_types(source: &str) -> BTreeMap<String, Vec<String>> {
    let mut found = BTreeMap::new();
    let mut name: Option<String> = None;
    let mut types: Vec<String> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            if trimmed.ends_with('{') {
                name = Some(
                    rest.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches('{')
                        .to_owned(),
                );
                types.clear();
            }
            continue;
        }
        if trimmed == "}" {
            if let Some(held) = name.take() {
                found.insert(held, core::mem::take(&mut types));
            }
            continue;
        }
        if name.is_none() {
            continue;
        }
        if let Some(field) = trimmed.strip_prefix("pub ")
            && let Some((_, written_as)) = field.split_once(':') {
                types.push(written_as.trim().trim_end_matches(',').to_owned());
            }
    }
    found
}

/// Whether a type is written in terms of this shape.
///
/// Whole words only. `Vec<SupplierOwingWire>` does not carry an `OwingWire`,
/// and a substring match said it did: four fields of a shape that is read
/// perfectly well somewhere else were called dropped, which is how a guard
/// teaches people to ignore it.
fn names(written_as: &str, shape: &str) -> bool {
    written_as
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == shape)
}

/// One arm of the match that turns replies into what a screen holds.
struct Arm {
    reply: String,
    mirrors: Vec<String>,
}

fn arms() -> Vec<Arm> {
    let bindings = include_str!("../src/sync.rs");
    let mut found = Vec::new();
    for piece in bindings.split("\n        Exchange::") {
        let Some(at) = piece.find("protocol::") else {
            continue;
        };
        let reply: String = piece[at + "protocol::".len()..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !reply.ends_with("Response") {
            continue;
        }
        // Every row shape this arm builds by hand. The `.map(|one| Row {` is
        // the shape of the work: one row in, one row out.
        let mut mirrors = Vec::new();
        let mut from = 0_usize;
        while let Some(spot) = piece[from..].find(".map(|") {
            let start = from + spot;
            let rest = &piece[start..];
            if let Some(brace) = rest.find(" {") {
                let head = &rest[..brace];
                if let Some(named) = head.rsplit(['|', ' ']).find(|word| {
                    word.chars().next().is_some_and(char::is_uppercase)
                        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
                }) {
                    mirrors.push(named.to_owned());
                }
            }
            from = start + ".map(|".len();
        }
        found.push(Arm { reply, mirrors });
    }
    found
}

#[test]
fn every_field_a_reply_carries_reaches_the_shape_a_screen_holds() {
    let wire = structs(include_str!("../../core/src/protocol/mod.rs"));
    let wire_types = field_types(include_str!("../../core/src/protocol/mod.rs"));
    let held = structs(include_str!("../src/sync.rs"));

    // Every arm that reads a reply, gathered by the reply: the same one is read
    // in two places when a till and the back office both ask, and each builds
    // the rows it needs. What matters is whether a field reaches a screen
    // anywhere, not whether both readers wanted it.
    let mut by_reply: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for arm in arms() {
        by_reply
            .entry(arm.reply)
            .or_default()
            .extend(arm.mirrors);
    }

    let mut dropped: Vec<String> = Vec::new();
    for (reply, mirrors) in &by_reply {
        let arm = Arm {
            reply: reply.clone(),
            mirrors: mirrors.clone(),
        };
        let Some(reply_types) = wire_types.get(&arm.reply) else {
            continue;
        };
        // The row shapes this reply carries inside it.
        let carried: Vec<&String> = wire
            .keys()
            .filter(|other| {
                *other != &arm.reply
                    && reply_types.iter().any(|written_as| names(written_as, other))
            })
            .collect();
        if carried.is_empty() {
            continue;
        }
        if arm.mirrors.is_empty() {
            assert!(
                CONVERTED_ELSEWHERE
                    .iter()
                    .any(|(reply, _)| *reply == arm.reply),
                "{} carries rows and this arm builds none of them. Say where that work happens in \
                 CONVERTED_ELSEWHERE, because from here a converter somewhere else and a field \
                 quietly dropped look the same.",
                arm.reply
            );
            continue;
        }

        let carried_names: Vec<&String> = arm
            .mirrors
            .iter()
            .filter_map(|mirror| held.get(mirror))
            .flatten()
            .collect();

        for shape in carried {
            for field in wire.get(shape).into_iter().flatten() {
                if carried_names.contains(&field) {
                    continue;
                }
                // An id the screen holds by its own name: the wire says
                // `item_id` as a number and the screen holds `item` as the
                // text a person can read back.
                if let Some(stem) = field.strip_suffix("_id")
                    && carried_names.iter().any(|held| held.as_str() == stem) {
                        continue;
                    }
                // A name carried instead of the id it belongs to.
                if carried_names
                    .iter()
                    .any(|held| held.as_str() == format!("{field}_name"))
                {
                    continue;
                }
                if NOT_CARRIED
                    .iter()
                    .any(|(named, left, _)| named == shape && left == field)
                {
                    continue;
                }
                dropped.push(format!("{shape}.{field} (in the {} arm)", arm.reply));
            }
        }
    }
    dropped.sort();
    dropped.dedup();

    assert!(
        dropped.is_empty(),
        "the shop sends these and the shape a screen is handed has no room for them, so they stop \
         here and nothing complains:\n  {}\n\nCarry it, or write it down in NOT_CARRIED with what \
         a shopkeeper loses by it. Two of these went unnoticed for weeks: which receipt a reprint \
         was of, and whether a line was zero rated or exempt.",
        dropped.join("\n  ")
    );
}

#[test]
fn what_is_written_down_is_about_shapes_that_exist() {
    // A reason kept against a field that has been renamed is a reason nobody is
    // applying, and it would hide the next field that really is dropped.
    let wire = structs(include_str!("../../core/src/protocol/mod.rs"));
    for (shape, field, why) in NOT_CARRIED {
        let fields = wire
            .get(*shape)
            .unwrap_or_else(|| panic!("{shape} is written down here and no longer exists"));
        assert!(
            fields.iter().any(|held| held == field),
            "{shape} no longer has {field}, so this is stale: {why}"
        );
    }
    for (reply, where_it_happens) in CONVERTED_ELSEWHERE {
        assert!(
            wire.contains_key(*reply),
            "{reply} is written down as converted by {where_it_happens} and no longer exists"
        );
    }
}
