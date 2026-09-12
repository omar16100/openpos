//! Every test in this crate that is not beside the code it tests, in one binary.
//!
//! One file per concern still, and the names are the documentation: what is
//! different is that they are modules of one test binary rather than 14
//! separate ones.
//!
//! The reason is measured rather than assumed. A change to this crate used to
//! relink every one of those binaries, about 21 MB each, and a full run of the
//! workspace took the best part of an hour of which five seconds was testing.
//! Linking is the work, so the way to spend less of it is to link less.
//!
//! A new test file goes in `suite/` and gets a line here. Nothing else about
//! writing one changes: `cargo test -p openpos-core suite::what_it_is_called`
//! runs one of them.

#[path = "suite/a_cheap_tablet.rs"]
mod a_cheap_tablet;
#[path = "suite/bytes_from_before.rs"]
mod bytes_from_before;
#[path = "suite/drawer_properties.rs"]
mod drawer_properties;
#[path = "suite/frozen_shapes.rs"]
mod frozen_shapes;
#[path = "suite/paper_words.rs"]
mod paper_words;
#[path = "suite/pricing_properties.rs"]
mod pricing_properties;
#[path = "suite/protocol_shapes.rs"]
mod protocol_shapes;
#[path = "suite/refusal_codes.rs"]
mod refusal_codes;
#[path = "suite/salvage.rs"]
mod salvage;
#[path = "suite/schema_labels.rs"]
mod schema_labels;
#[path = "suite/storage_recovery.rs"]
mod storage_recovery;
#[path = "suite/upgrade.rs"]
mod upgrade;
#[path = "suite/voice_properties.rs"]
mod voice_properties;
#[path = "suite/what_this_build_writes.rs"]
mod what_this_build_writes;
