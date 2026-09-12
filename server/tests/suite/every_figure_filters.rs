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
//!
//! The scan reads text, which gives it three ways to be walked past, and each
//! one has its own test below rather than a note saying it is a known
//! weakness. SQL could move to a file this never opens, so every source under
//! `server/src` is read rather than one named file. A query could be built
//! instead of written, and text cannot see what a `format!` produces, so a
//! query whose SQL is not a literal is refused outright. And the table has
//! more than one spelling, so the ones Postgres accepts are matched too.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

/// Every source the server is built from, read as text at run time.
///
/// Read from the directory rather than named one by one, because a named list
/// is a list that does not include the file somebody adds next month. That file
/// would hold the eleventh query, and this whole test would pass without ever
/// having opened it.
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
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut found,
    );
    assert!(
        found.len() > 5,
        "found {} sources under server/src, which is not the crate: this test is reading the \
         wrong directory and would pass without checking anything",
        found.len()
    );
    found
}

/// Every source, as one body of text.
fn all_of_it() -> String {
    every_source()
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_query_that_reads_the_sales_filters_them_or_says_why_not() {
    let mut unmarked = Vec::new();
    for sql in string_literals(&all_of_it()) {
        if !reads_the_sales(&sql) {
            continue;
        }
        let low = sql.to_lowercase();
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
    for sql in string_literals(&all_of_it()) {
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


/// Whether a query reads the sales, in any spelling Postgres accepts.
///
/// `from sale` was the whole test, and it is one of four ways to name that
/// table. A schema prefix and a quoted identifier are both ordinary SQL and
/// both walked straight past a rule about a shop's money.
///
/// `sale_line` and `sale_vat` match too, and that is deliberate: they are joined
/// to the sale they belong to, so a figure read from them has the same duty. A
/// false positive here costs somebody one line of thought; a false negative
/// costs a shop a duplicate in its takings for ever.
fn reads_the_sales(sql: &str) -> bool {
    let low = sql.to_lowercase();
    ["from", "join"].iter().any(|word| {
        ["sale", "public.sale", "\"sale\""]
            .iter()
            .any(|table| low.contains(&alloc_pair(word, table)))
    })
}

/// `from` and `sale` with one space, which is how the formatter writes it.
fn alloc_pair(word: &str, table: &str) -> String {
    format!("{word} {table}")
}

/// Every way a query reaches the database, so a new one cannot be missed.
const HOW_SQL_IS_SENT: &[&str] = &[
    "sqlx::query(",
    "sqlx::query_as(",
    "sqlx::query_scalar(",
    "sqlx::query_as::",
    "sqlx::query_scalar::",
    "sqlx::raw_sql(",
];

#[test]
fn every_query_is_written_rather_than_built() {
    // The scan above reads text, and text cannot see what a `format!` produces:
    // a query assembled at run time could name the sales, skip the filter, and
    // pass this file without ever appearing in it. The same hole hides a worse
    // one, because SQL built by concatenation is where an injection lives.
    //
    // So the rule is stricter than the scan needs: what reaches the database is
    // a literal, always. Every value goes through a bind.
    let mut built = Vec::new();
    for (name, source) in every_source() {
        for how in HOW_SQL_IS_SENT {
            for (at, _) in source.match_indices(how) {
                let rest = source[at + how.len()..].trim_start();
                // A turbofish call takes its argument after the generic.
                let rest = match rest.strip_prefix("<") {
                    Some(after) => match after.find(">(") {
                        Some(end) => after[end + 2..].trim_start(),
                        None => rest,
                    },
                    None => rest,
                };
                // The queries in this crate explain themselves in Rust comments
                // above their own SQL, which is where those comments belong.
                let rest = past_comments(rest);
                if !rest.starts_with('"') {
                    let shown: String = rest.chars().take(60).collect();
                    built.push(format!("{name}: {shown}"));
                }
            }
        }
    }

    assert!(
        built.is_empty(),
        "these send SQL that is not a written literal. A query built at run time is invisible to \
         the rule above, so it can read a shop's sales without the filter and nothing here would \
         say so, and it is also where an injection lives. Write the SQL and bind the \
         values:\n  {}",
        built.join("\n  ")
    );
}

#[test]
fn the_scan_can_still_see_every_query_the_server_sends() {
    // The list above is how SQL leaves this crate. A new way of sending it,
    // added and not listed, is a query nothing on this page checks.
    let source = all_of_it();
    let sent = HOW_SQL_IS_SENT
        .iter()
        .map(|how| source.matches(how).count())
        .sum::<usize>();
    let mentions = source.matches("sqlx::query").count() + source.matches("raw_sql").count();
    assert!(
        sent >= mentions,
        "the server sends SQL {mentions} times and this test knows about {sent} of them: a way of \
         reaching the database has been added and is not in HOW_SQL_IS_SENT"
    );
}


/// Whatever comes after any Rust line comments at the front of `text`.
fn past_comments(text: &str) -> &str {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("//") {
        rest = match after.find('\n') {
            Some(end) => after[end + 1..].trim_start(),
            None => "",
        };
    }
    rest
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
    let reads = string_literals(&all_of_it())
        .into_iter()
        .filter(|sql| reads_the_sales(sql))
        .count();
    assert!(
        reads > 15,
        "only {reads} queries read the sales, which means this scan is not reading the file"
    );
}
