//! A ladder of protocol branches has to climb.
//!
//! A route answers an older caller on the shape that caller can read, and it
//! does that with a run of `if protocol < N` branches, oldest first. Written in
//! any other order the branches below the first match are dead: a caller
//! speaking 13 falls into the branch for anything under 16 and is answered with
//! a body carrying two fields it has never heard of.
//!
//! That is not a wrong answer in the ordinary sense. These bodies are postcard,
//! which is positional and has no field names in it, so a list of sales with an
//! extra trailing field on each one is read as a list of different length
//! carrying different figures. The symptom is a back office that shows nonsense
//! for a receipt somebody is holding, or shows nothing.
//!
//! It happened here. The receipt route grew a branch for protocol 16 on 13
//! September 2026 and the branch was put above the one for 14, which had been
//! correct until that moment. Nothing failed, because every build in the room
//! spoke the newest version. This test is what would have failed.

// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// Every `.rs` under the server's `src`.
fn every_source() -> Vec<(String, String)> {
    fn walk(at: &std::path::Path, into: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|kind| kind == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                into.push((path.display().to_string(), text));
            }
        }
    }

    let mut found = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(!found.is_empty(), "no server source was read at all");
    found
}

/// The version in `if protocol < 16 {`, when a line is one.
fn version_asked_about(line: &str) -> Option<u16> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("if protocol < ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[test]
fn every_run_of_protocol_branches_goes_oldest_first() {
    let mut faults = Vec::new();

    for (path, source) in every_source() {
        // Per function, because two routes in one file have nothing to say to
        // each other and each has its own ladder.
        let mut last: Option<(u16, usize)> = None;
        for (number, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("pub(crate) async fn ")
                || trimmed.starts_with("pub(super) async fn ")
            {
                last = None;
                continue;
            }
            let Some(version) = version_asked_about(line) else {
                continue;
            };
            if let Some((before, at)) = last
                && version <= before
            {
                faults.push(format!(
                    "{path}:{} answers protocol < {version} below the branch for < {before} on \
                     line {}, so nothing reaches it: a caller that old falls into the newer \
                     branch and is sent a body with fields it cannot read",
                    number.saturating_add(1),
                    at.saturating_add(1),
                ));
            }
            last = Some((version, number));
        }
    }

    assert!(faults.is_empty(), "{}", faults.join("\n  "));
}

/// And the scan is reading what it thinks it is.
///
/// A test that finds nothing because it is looking in the wrong place passes
/// for ever. These ladders are the whole subject, so the scan says how many it
/// found and stops if the answer is none.
#[test]
fn the_scan_finds_the_ladders_it_is_about() {
    let found: usize = every_source()
        .iter()
        .map(|(_, source)| source.lines().filter(|line| version_asked_about(line).is_some()).count())
        .sum();
    assert!(
        found >= 8,
        "the scan found {found} protocol branches in the server, which is not this server: \
         they have been written another way and this test is no longer reading them"
    );
}
