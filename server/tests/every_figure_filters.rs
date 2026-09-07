//! Every figure ignores a sale the shop struck out, or says why it does not.
//!
//! Striking out a sale takes it out of the takings, the tax, what sold, the
//! shelf and the account book. Ten queries carry that filter today, and the
//! eleventh, written next month by somebody who has never read this file, is
//! one line away from counting a duplicate for ever.
//!
//! So the rule is checked here rather than remembered. A query that reads the
//! sales either filters them or carries `-- every sale:` and a reason, beside
//! the query, where the next person is already looking.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// The Postgres store, read as text. Its own source is the thing under test.
const STORE: &str = include_str!("../src/pg.rs");

#[test]
fn a_query_that_reads_the_sales_filters_them_or_says_why_not() {
    let mut unmarked = Vec::new();
    for sql in string_literals(STORE) {
        let low = sql.to_lowercase();
        if !(low.contains("from sale") || low.contains("join sale")) {
            continue;
        }
        // The filter itself, in either direction: a figure that skips a
        // struck-out sale, or the account book's predicate that finds one.
        if low.contains("resolution_kept is not false") || low.contains("resolution_kept is false")
        {
            continue;
        }
        // Or a deliberate exemption, written where the query is.
        if low.contains("-- every sale:") {
            continue;
        }
        unmarked.push(sql.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    assert!(
        unmarked.is_empty(),
        "these read the sales without ignoring the ones the shop struck out. Either add the \
         filter, or say why every sale belongs in this one with a line starting `-- every sale:` \
         inside the query:\n  {}",
        unmarked.join("\n  ")
    );
}

#[test]
fn an_exemption_is_written_so_that_postgres_reads_it_as_a_comment() {
    // A comment in SQL ends at the newline. An exemption whose second line
    // forgot its own dashes is a sentence the database tries to run, and it
    // says so with an error that names nothing: this was written wrong twice
    // in the hour this file was added.
    let mut broken = Vec::new();
    for sql in string_literals(STORE) {
        if !sql.contains("-- every sale:") {
            continue;
        }
        let mut in_comment = false;
        for line in sql.lines().map(str::trim) {
            if line.starts_with("--") {
                in_comment = true;
                continue;
            }
            if in_comment {
                // The first line after the comment block has to be the
                // statement itself.
                let starts_a_statement = ["select", "insert", "update", "with", "delete"]
                    .iter()
                    .any(|word| line.to_lowercase().starts_with(word));
                if !line.is_empty() && !starts_a_statement {
                    broken.push(format!("{line} ...in: {}", one_line(&sql)));
                }
                break;
            }
        }
    }

    assert!(
        broken.is_empty(),
        "these lines sit inside an exemption and are not comments, so Postgres will try to run \
         them:\n  {}",
        broken.join("\n  ")
    );
}

/// One line of a query, for an error message somebody has to read.
fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every double-quoted literal in a Rust source file.
///
/// Crude on purpose: it is looking for SQL, and SQL is the only thing in that
/// file long enough to contain `from sale`. Escapes are stepped over so a
/// literal containing a quote does not end the scan early.
fn string_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut lit = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    // Whatever it escapes is not a closing quote.
                    let _ = chars.next();
                    lit.push(' ');
                }
                '"' => break,
                other => lit.push(other),
            }
        }
        found.push(lit);
    }
    found
}

#[test]
fn the_scan_finds_the_queries_it_is_supposed_to_be_checking() {
    // A rule that silently matched nothing would pass for ever. This is the
    // same mistake as a property test that gates its own inputs, and it has
    // already been made once in this codebase.
    let reads = string_literals(STORE)
        .into_iter()
        .filter(|sql| {
            let low = sql.to_lowercase();
            low.contains("from sale") || low.contains("join sale")
        })
        .count();
    assert!(
        reads > 15,
        "only {reads} queries read the sales, which means this scan is not reading the file"
    );
}
