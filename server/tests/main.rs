//! Every test in this crate that is not beside the code it tests, in one binary.
//!
//! One file per concern still, and the names are the documentation: what is
//! different is that they are modules of one test binary rather than 7
//! separate ones.
//!
//! The reason is measured rather than assumed. A change to the core used to
//! relink every test binary in the workspace, about 21 MB each, and a full run
//! took the best part of an hour of which five seconds was testing. Linking is
//! the work, so the way to spend less of it is to link less.
//!
//! A new test file goes in `suite/` and gets a line here.

#[path = "suite/a_catalogue_row_stays_readable.rs"]
mod a_catalogue_row_stays_readable;
#[path = "suite/a_restore_makes_its_own_tables.rs"]
mod a_restore_makes_its_own_tables;
#[path = "suite/every_figure_filters.rs"]
mod every_figure_filters;
#[path = "suite/every_route_is_reachable.rs"]
mod every_route_is_reachable;

/// Nothing a sale can point at is ever deleted from under it, which a lookup
/// resolving names and an export reading lists both rest on.
#[path = "suite/nothing_a_sale_names_is_deleted.rs"]
mod nothing_a_sale_names_is_deleted;
#[path = "suite/export_import.rs"]
mod export_import;
#[path = "suite/postgres_repo.rs"]
mod postgres_repo;
#[path = "suite/till_sync.rs"]
mod till_sync;
