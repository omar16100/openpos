//! Every test in this crate that is not beside the code it tests, in one binary.
//!
//! One file today, and the shape is the same as the other two crates on purpose:
//! a new test file goes in `suite/` and gets a line here, rather than becoming
//! another binary to link. See `core/tests/main.rs` for the measurement.

#[path = "suite/nothing_the_shop_sends_is_dropped.rs"]
mod nothing_the_shop_sends_is_dropped;
