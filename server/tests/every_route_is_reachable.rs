//! Every route the server serves can be reached from a device, and every
//! request a device can build has a route.
//!
//! Four defects this month were the same defect: a rule written, tested,
//! shipped, and reachable by nobody. An item could not be deleted because no
//! screen asked; the price check refused rows nothing could send; the repair
//! queue carried a reason no screen could read. Each was found by walking, one
//! at a time, weeks after it was written, and each looked finished in the
//! commit that added it because the tests exercised the handler directly.
//!
//! A handler with a test and no caller is the most convincing kind of dead
//! code: it is green, it is covered, and it does nothing for a shop. So the
//! chain from a screen to a route is checked as a property of the source rather
//! than re-audited by hand every few weeks.
//!
//! The chain has three links and this file holds the first. `sync.rs` builds
//! every request a device sends, so a route nothing in it posts to is a route
//! nothing can reach. The second link, that every request the bindings can
//! build is asked for by a screen, is in `apps/shared/admin_requests.test.js`,
//! against a list this file writes out: the JavaScript cannot read Rust, and a
//! list copied by hand goes stale the first time a request is added.
//!
//! Source scanning rather than anything cleverer, for the same reason
//! `frozen_shapes.rs` scans `wire.rs`: the property is about what is written in
//! the file.

// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::collections::BTreeSet;

/// The router, as written.
const ROUTER: &str = include_str!("../src/http.rs");

/// Every request a device builds and posts.
const BINDINGS: &str = include_str!("../../bindings/src/sync.rs");

/// Routes nothing on a device posts to, and why that is right.
///
/// One entry, and it needs a reason next to it rather than a place on a list:
/// this is exactly the list that grows quietly until it is the whole router.
const REACHED_BY_SOMETHING_ELSE: &[(&str, &str)] = &[
    (
        "/health",
        "asked by whatever is watching the server, which is not a device and holds no credential",
    ),
    (
        "/v1/back-office/receipt",
        "a back office built before the counter could ask this question still posts to it. The \
         route this build's devices use is /v1/receipt, which answers the same thing without \
         asking whether the device is the back office: a customer comes back with a receipt to \
         the counter, not to the desk",
    ),
];

/// Every path the router serves.
///
/// The whitespace between `.route(` and the path is skipped rather than
/// assumed away. A long route is wrapped onto its own line by the formatter,
/// and the first version of this scan matched `.route("` exactly: it silently
/// stopped seeing three routes, which is a reachability test that has quietly
/// stopped checking reachability.
fn routes() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (at, _) in ROUTER.match_indices(".route(") {
        let rest = &ROUTER[at + ".route(".len()..];
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = rest.find('"') {
            found.insert(rest[..end].to_owned());
        }
    }
    found
}

/// Every path something on a device posts to.
fn posted_to() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (at, _) in BINDINGS.match_indices("\"/v1/") {
        let rest = &BINDINGS[at + 1..];
        if let Some(end) = rest.find('"') {
            found.insert(rest[..end].to_owned());
        }
    }
    found
}

#[test]
fn every_route_the_server_serves_can_be_reached_from_a_device() {
    let posted = posted_to();
    let excused: BTreeSet<&str> = REACHED_BY_SOMETHING_ELSE
        .iter()
        .map(|(path, _)| *path)
        .collect();

    for route in routes() {
        if excused.contains(route.as_str()) {
            continue;
        }
        assert!(
            posted.contains(&route),
            "{route} is served and nothing in bindings/src/sync.rs posts to it, so no device can \
             reach it. Either a screen is missing, which is the defect this test exists for, or it \
             is reached by something that is not a device and belongs in \
             REACHED_BY_SOMETHING_ELSE with the reason written down"
        );
    }
}

#[test]
fn every_request_a_device_builds_has_a_route_to_arrive_at() {
    // The other direction. A device posting somewhere the server does not serve
    // gets a 404 that says nothing, and the shop is told the shop is down.
    let served = routes();
    for path in posted_to() {
        assert!(
            served.contains(&path),
            "a device posts to {path} and the server serves no such route: the shop would answer \
             404 and the screen would say it could not reach the shop"
        );
    }
}

#[test]
fn nothing_is_excused_that_no_longer_exists() {
    // Or the list keeps its reasons for routes that were deleted, and the next
    // person reads a justification for something that is not there.
    let served = routes();
    for (path, why) in REACHED_BY_SOMETHING_ELSE {
        assert!(
            served.contains(*path),
            "{path} is excused from needing a caller ({why}) and the server does not serve it"
        );
    }
}

/// The list of requests a device can build, written out for the JavaScript.
///
/// The second link in the chain: a request the bindings can build and no screen
/// asks for is the same dead code one layer up, and it is the layer where all
/// four of this month's defects actually lived.
#[test]
fn the_screens_are_handed_the_list_of_requests() {
    let mut names: Vec<String> = Vec::new();
    let at = BINDINGS
        .find("pub enum AdminRequest {")
        .expect("the back office's requests are an enum in this file");
    let body = &BINDINGS[at..];
    let end = body.find("\n}\n").expect("the enum ends");
    let mut depth = 0_i32;
    for line in body[..end].lines().skip(1) {
        let trimmed = line.trim();
        // Only the outermost level names a variant; a braced variant's fields
        // are indented inside it.
        if depth == 0
            && let Some(name) = trimmed.split(['{', ',', '(']).next()
            && !name.is_empty()
            && name.chars().next().is_some_and(char::is_uppercase)
        {
            names.push(snake(name.trim()));
        }
        depth += i32::try_from(trimmed.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(trimmed.matches('}').count()).unwrap_or(0);
    }
    names.sort();
    assert!(
        names.len() > 30,
        "the scan found {} requests, which is not the enum: it has been reformatted and this test \
         is no longer reading it",
        names.len()
    );

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../apps/shared/admin_requests.json"
    );
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut written = String::from("[\n");
    for (at, name) in names.iter().enumerate() {
        written.push_str("  \"");
        written.push_str(name);
        written.push('"');
        if at + 1 < names.len() {
            written.push(',');
        }
        written.push('\n');
    }
    written.push_str("]\n");

    if held.trim() != written.trim() {
        std::fs::write(path, &written).expect("apps/shared/admin_requests.json is writable");
        panic!(
            "apps/shared/admin_requests.json did not match the requests a device can build and has \
             been rewritten. Run the tests again, and give every new one a screen that asks for it"
        );
    }
}

/// The list of commands a till can be given, written out for the JavaScript.
///
/// The third link. A command is not a request to the shop: it is what a screen
/// asks its own till to do, and the same dead-code question applies to it. Nine
/// of these are reached by something other than a screen, and
/// `apps/shared/till_commands.test.js` holds that list with a reason beside
/// each, because nine commands nobody calls is a fact worth having to write
/// down rather than one to discover by grepping in a year.
#[test]
fn the_screens_are_handed_the_list_of_commands() {
    let source = include_str!("../../bindings/src/lib.rs");
    let names = variants_of(source, "pub enum Command {");
    assert!(
        names.len() > 30,
        "the scan found {} commands, which is not the enum: it has been reformatted and this test \
         is no longer reading it",
        names.len()
    );
    write_out(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../apps/shared/till_commands.json"
        ),
        &names,
        "apps/shared/till_commands.json",
    );
}

/// The outermost variant names of an enum, in the source as written.
fn variants_of(source: &str, opener: &str) -> Vec<String> {
    let at = source.find(opener).expect("the enum is in this file");
    let body = &source[at..];
    let end = body.find("\n}\n").expect("the enum ends");
    let mut names = Vec::new();
    let mut depth = 0_i32;
    for line in body[..end].lines().skip(1) {
        let trimmed = line.trim();
        // Only the outermost level names a variant; a braced variant's fields
        // are indented inside it.
        if depth == 0
            && let Some(name) = trimmed.split(['{', ',', '(']).next()
            && !name.is_empty()
            && name.chars().next().is_some_and(char::is_uppercase)
        {
            names.push(snake(name.trim()));
        }
        depth += i32::try_from(trimmed.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(trimmed.matches('}').count()).unwrap_or(0);
    }
    names.sort();
    names
}

/// Write a list out, and fail loudly the first time so nobody misses it.
fn write_out(path: &str, names: &[String], shown: &str) {
    let held = std::fs::read_to_string(path).unwrap_or_default();
    let mut written = String::from("[\n");
    for (at, name) in names.iter().enumerate() {
        written.push_str("  \"");
        written.push_str(name);
        written.push('"');
        if at + 1 < names.len() {
            written.push(',');
        }
        written.push('\n');
    }
    written.push_str("]\n");

    if held.trim() != written.trim() {
        std::fs::write(path, &written).unwrap_or_else(|_| panic!("{shown} is writable"));
        panic!(
            "{shown} did not match what the source says and has been rewritten. Run the tests \
             again, and account for every new entry"
        );
    }
}

/// `ItemNow` as the wire writes it: `item_now`.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (at, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && at != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
