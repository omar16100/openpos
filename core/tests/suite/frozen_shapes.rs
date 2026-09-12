//! A legacy shape must not be built out of a shape that keeps growing.
//!
//! Every persisted struct in `wire.rs` is read positionally, so a legacy copy
//! kept to read old bytes must not name a type that is still gaining fields.
//! Bend that once and the next field added anywhere silently changes what those
//! bytes claim to be: the symptom is a till that comes back after an upgrade
//! unable to read its own standing state, with the day's unsent sales and the
//! parked baskets inside it.
//!
//! The byte fixtures in `bytes_from_before.rs` catch that after the fact, on
//! whichever shapes somebody remembered to freeze. This catches it in the
//! source, on all of them, for the shapes that have actually been growing.
//!
//! Source scanning rather than types, because the property is about how the
//! file is written and Rust cannot say "this struct may only name frozen
//! structs". Same tactic as `every_figure_filters` on the SQL, for the same
//! reason.

// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// The shapes that have grown a field since they were first written, and so
/// must never be named by a legacy copy.
///
/// A short list on purpose. When a field is added to any other shape in
/// `wire.rs`, the shape belongs here and a frozen copy of it belongs in that
/// file, pointed at by every legacy struct written before the change. Adding a
/// name here is the cheap half of that work; this test is what makes somebody
/// do the other half.
const STILL_GROWING: &[&str] = &[
    "ShopV1",
    "ItemV1",
    "LineV1",
    "CustomerV1",
    "HeldTicketV1",
    "HeldTicketsV1",
    "TicketV1",
    "SaleCommitV1",
    "AllowedV1",
];

#[test]
fn no_legacy_shape_is_built_out_of_a_shape_that_keeps_growing() {
    let source = include_str!("../../src/storage/wire.rs");

    let mut looking_at: Option<String> = None;
    let mut faults: Vec<String> = Vec::new();

    for (number, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            let name = rest
                .split([' ', '{', '<'])
                .next()
                .unwrap_or_default()
                .to_owned();
            looking_at = name.ends_with("Legacy").then_some(name);
            continue;
        }
        if trimmed == "}" {
            looking_at = None;
            continue;
        }

        let Some(holder) = looking_at.as_deref() else {
            continue;
        };
        let Some(field) = trimmed.strip_prefix("pub ") else {
            continue;
        };
        let Some((_, declared)) = field.split_once(':') else {
            continue;
        };
        // Every identifier in the type, so `Vec<Option<ItemV1>>` is checked at
        // every level rather than only at the outside.
        for word in declared.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !STILL_GROWING.contains(&word) {
                continue;
            }
            faults.push(format!(
                "wire.rs:{}: {holder} holds {word}, which is still growing. Freeze a copy of \
                 {word} as it stands and point {holder} at that, or the next field added to \
                 {word} changes what these bytes claim to be.",
                number.saturating_add(1)
            ));
        }
    }

    assert!(
        faults.is_empty(),
        "a legacy shape may not name a shape that keeps growing:\n  {}",
        faults.join("\n  ")
    );
}

/// And the list above is not decoration: every name on it must be a shape this
/// file actually holds, so a rename cannot quietly empty the rule out.
#[test]
fn every_shape_named_as_still_growing_is_a_shape_that_exists() {
    let source = include_str!("../../src/storage/wire.rs");
    for name in STILL_GROWING {
        assert!(
            source.contains(&format!("pub struct {name} {{")),
            "{name} is listed as still growing and no longer exists in wire.rs: the rule now \
             covers nothing, which is worse than not having it"
        );
    }
}
