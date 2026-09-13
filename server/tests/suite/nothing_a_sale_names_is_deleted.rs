//! Nothing a sale can point at is ever deleted from under it.
//!
//! A sale names a terminal, the person who rang it, the person who bought it,
//! and a delivery names a supplier. None of those rows is ever removed: a
//! person who leaves the shop is made inactive, a terminal that is lost is
//! revoked, a customer whose account is stopped keeps what they owe. Every one
//! of those is a row a sale still points at, and a shop asked "who rang this"
//! about a sale from March is asking about somebody who left in April.
//!
//! Two things rest on that, and neither is obvious from the code that relies
//! on them.
//!
//! A lookup resolves the operator's and the buyer's names when it runs, so the
//! paper says what the shop calls them now. A `delete` would make that paper
//! say nobody, silently, for every sale they ever touched.
//!
//! And an export reads the ledgers as of a cut and the lists as they stand,
//! which is deliberate, because a restore wants the lists current. That mixture
//! is only harmless while the lists cannot lose a row between the two moments.
//! With a `delete` in the shop, a backup taken while somebody was tidying the
//! staff list would restore sales naming people it does not hold.
//!
//! So the rule is checked here rather than remembered. The one table that is
//! deleted from is `open_drawer`, which is a drawer that has since been counted
//! and written down as a closed one: it is a list of what is open, and closed is
//! closed.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// The tables a sale, a delivery or a drawer points at by id.
const POINTED_AT: &[&str] = &[
    "terminal",
    "operator",
    "customer",
    "supplier",
    "sale",
    "account_entry",
    "closed_shift",
    "stock_movement",
    "catalogue_change",
];

/// What may be deleted, with the reason, because a rule with no exceptions
/// written down is a rule somebody works around in silence.
///
/// `open_drawer` is the list of drawers standing open. A drawer that has been
/// counted is written down as a closed one and taken out of this list, and an
/// open list still showing a drawer somebody counted an hour ago is a list an
/// owner learns to ignore. Nothing points at a row in it: the closed record
/// carries its own id.
const MAY_BE_DELETED: &[&str] = &["open_drawer"];

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

#[test]
fn no_query_deletes_a_row_a_sale_can_point_at() {
    let mut faults = Vec::new();
    for (path, source) in every_source() {
        let lowered = source.to_lowercase();
        for (at, _) in lowered.match_indices("delete from") {
            let rest = lowered[at.saturating_add("delete from".len())..].trim_start();
            let table: String = rest
                .chars()
                .take_while(|one| one.is_alphanumeric() || *one == '_')
                .collect();
            if MAY_BE_DELETED.contains(&table.as_str()) {
                continue;
            }
            if POINTED_AT.contains(&table.as_str()) {
                faults.push(format!("{path}: deletes from {table}"));
            }
        }
    }

    assert!(
        faults.is_empty(),
        "a sale points at these rows by id and reads their names when somebody asks, so deleting \
         one makes every paper that named it say nobody, quietly. Stop the account, revoke the \
         device, mark the person inactive: those keep the row. If a row really must go, say why \
         beside the query and name the table in MAY_BE_DELETED here.\n  {}",
        faults.join("\n  ")
    );
}

/// A migration must not do it either.
#[test]
fn no_migration_drops_one_either() {
    let at = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut faults = Vec::new();
    for entry in std::fs::read_dir(&at).expect("the migrations are there").flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let lowered = text.to_lowercase();
        for table in POINTED_AT {
            if lowered.contains(&format!("drop table {table}"))
                || lowered.contains(&format!("drop table if exists {table}"))
            {
                faults.push(format!("{}: drops {table}", entry.path().display()));
            }
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n  "));
}
