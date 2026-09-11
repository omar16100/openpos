//! A restore is the one command that runs on a machine which has never held
//! the shop.
//!
//! Everything else here starts on a machine that has been serving: the tables
//! are there because the server made them on the way up. A restore is the
//! opposite by definition. It is a rented box after the old one died, a
//! replacement tablet, or somebody checking that last night's file is worth the
//! disk it is on. On that machine there are no tables, and for a while the
//! answer to a restore was one word, `Backend`, on the one morning a shop is
//! trying to get its life back.
//!
//! Read rather than run, because the wiring lives in `main.rs`, which is a
//! binary and cannot be called from a test. What is being guarded is the order
//! of two lines, and a test that reads them is worth more than no test at all:
//! the live walk that found this is in `todo.md`, and this is what stops it
//! coming back.

const MAIN: &str = include_str!("../src/main.rs");

/// The stretch between asking for the database URL and connecting with it.
fn before_the_database_is_opened() -> &'static str {
    let asked = MAIN
        .find("let url = std::env::var(\"OPENPOS_DATABASE_URL\")")
        .expect("the one-shot commands ask for a database URL");
    let opened = MAIN[asked..]
        .find("PgRepo::connect(")
        .expect("and then open it");
    &MAIN[asked..asked + opened]
}

#[test]
fn a_restore_makes_its_tables_before_it_writes_to_them() {
    let before = before_the_database_is_opened();
    assert!(
        before.contains("PgRepo::migrate("),
        "a restore onto a machine that has never held this shop finds no tables. \
         Migrating has to happen before the connection is used, the way serving \
         has always done it, or the shop gets a one-word refusal on the morning \
         it is restoring a backup"
    );
    assert!(
        before.contains("Asked::Import"),
        "and only for a restore: an export or an enrolment code run against \
         somebody else's database has no business making tables in it"
    );
    assert!(
        before.contains("OPENPOS_ADMIN_DATABASE_URL"),
        "with the admin URL, because the app role is not allowed to make tables"
    );
}

#[test]
fn a_refused_restore_says_what_to_do_about_it() {
    let arm = MAIN
        .find("import_tenant(")
        .map(|at| &MAIN[at..(at + 900).min(MAIN.len())])
        .expect("a restore writes the bundle in");
    assert!(
        !arm.contains("format!(\"{error:?}\")"),
        "the database refusing a restore used to print the name of the layer \
         that refused it. The person reading it has a file and a machine that is \
         not their old one, and `Backend` tells them nothing they can act on"
    );
    assert!(
        arm.contains("OPENPOS_ADMIN_DATABASE_URL"),
        "what it usually is, is the tables not being there, so the message says \
         which variable to give and that nothing has been written yet"
    );
}
