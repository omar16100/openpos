//! Every shape that travels, written down, so one cannot change quietly.
//!
//! The wire has the same disease the disk had. A field appended to a request
//! makes every body an older build sends undecodable, because postcard is
//! positional: the server does not see a missing field, it sees rubbish and
//! says "malformed". A field appended to a reply does the same in the other
//! direction, and the shop is looking at an error where its trail should be.
//!
//! The protocol's own rule is to bump `PROTOCOL_VERSION`, freeze the old shape
//! and decode both. That rule is written at the top of the module, and it was
//! walked past twice this month: once when three refusals gained figures, and
//! once when the trail gained the receipt a reprint was of. Both times a person
//! caught it, and one of those times it was a reviewer rather than the author.
//!
//! So this file writes down what every shape looks like, and fails when one
//! moves. It fails on legitimate changes too, which is the point: the message
//! is what to do, and the record beside it is what a reviewer reads to see what
//! moved.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

/// Where the record lives, beside the tests that read it.
const RECORD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/protocol_shapes.txt");

/// Where a fresh one is written when they disagree, for copying over after
/// somebody has looked at it.
const FRESH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/protocol_shapes.txt");

/// Read the shapes out of the protocol's own source.
///
/// The source rather than the types, because a type cannot be asked what its
/// fields are called and the names are half of what makes a body positional.
/// Doc comments, attributes and blank lines are dropped: what is left is the
/// order and the types, which is exactly what the bytes depend on.
fn shapes() -> BTreeMap<String, String> {
    let source = include_str!("../../src/protocol/mod.rs");
    let mut found = BTreeMap::new();
    let mut name: Option<String> = None;
    let mut fields: Vec<String> = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("pub struct ")
            .or_else(|| trimmed.strip_prefix("pub enum "))
        {
            // A tuple struct or a unit struct says everything on one line.
            let head = rest.split_whitespace().next().unwrap_or_default();
            let named = head.trim_end_matches(['{', '(', ';', '<']).to_owned();
            if trimmed.ends_with('{') {
                name = Some(named);
                fields.clear();
            } else {
                found.insert(named, trimmed.to_owned());
            }
            continue;
        }
        if trimmed == "}" {
            if let Some(held) = name.take() {
                found.insert(held, fields.join(","));
            }
            fields.clear();
            continue;
        }
        if name.is_none() || trimmed.is_empty() || trimmed.starts_with("///") {
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with("#[") {
            continue;
        }
        // A struct field, or an enum variant with its payload.
        fields.push(
            trimmed
                .trim_end_matches(',')
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    found
}

/// The record as it is written: one shape per line, its name, a tab, its
/// fields. Plain text because it is read by whoever is reviewing the change,
/// and a diff of it is the whole point.
fn written_down(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(shape, fields)| (shape.to_owned(), fields.to_owned()))
        .collect()
}

#[test]
fn no_shape_that_travels_changes_without_being_written_down() {
    let now = shapes();
    let held = written_down(&std::fs::read_to_string(RECORD).expect("the record of what these shapes were"));

    let mut moved: Vec<String> = Vec::new();
    for (shape, fields) in &now {
        match held.get(shape) {
            None => moved.push(format!("{shape} is new")),
            Some(before) if before != fields => moved.push(format!("{shape} changed")),
            Some(_) => {}
        }
    }
    for shape in held.keys() {
        if !now.contains_key(shape) {
            moved.push(format!("{shape} is gone"));
        }
    }

    if !moved.is_empty() {
        let fresh: String = now
            .iter()
            .map(|(shape, fields)| format!("{shape}\t{fields}\n"))
            .collect();
        let _ = std::fs::write(FRESH, fresh);
    }

    assert!(
        moved.is_empty(),
        "these shapes travel between a till and a shop, and one of them has moved:\n  {}\n\n\
         postcard is positional, so a field added to a request makes every body an older build \
         sends undecodable, and a field added to a reply does the same to a build reading it. \
         If this is a shape a build in the field can send or read: raise PROTOCOL_VERSION, freeze \
         the shape as it was under its old name, and decode both at whichever end reads it. Then \
         copy target/protocol_shapes.txt over core/tests/protocol_shapes.txt in the same commit, \
         so the next person can see in the diff exactly what moved.\n\n\
         Read the frozen copy against the old line in that diff, field by field and in order. \
         The fields of a frozen copy are usually written out by hand, the compiler cannot help \
         with the order because a `From` impl assigns by name and compiles either way, and the \
         encoding is positional: a copy with two fields swapped is a till reading a shop's \
         address as its BIN, with nothing anywhere to notice. That happened here, and the diff \
         of this record is what caught it.",
        moved.join("\n  ")
    );
}

#[test]
fn the_record_is_about_the_protocol_this_build_speaks() {
    // A guard on the guard: a record written against a different version says
    // nothing about this one, and would pass while every shape in it was wrong.
    let held = written_down(&std::fs::read_to_string(RECORD).expect("the record"));
    assert!(
        held.contains_key("PushRequest") && held.contains_key("AllowedEntry"),
        "the record does not name the shapes this protocol is made of, so it is not this \
         protocol's record"
    );
    assert_eq!(
        openpos_core::protocol::PROTOCOL_VERSION,
        12,
        "the version the record above was taken against. Raising it is right and expected; raise \
         it here too, in the same commit as the frozen copy of whatever shape changed."
    );
}
