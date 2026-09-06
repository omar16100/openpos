//! The till behind a plain C ABI.
//!
//! Four functions, and one of them does all the work. Dart's FFI, Kotlin's JNI
//! and anything else that speaks C can call these with no code generator in the
//! build, which is the point: a generator is a dependency that has to keep
//! working across two toolchains it does not control, on the one path a shop
//! cannot do without.
//!
//! The whole surface is `openpos_till_run`, which takes a JSON command and
//! returns a JSON view. Adding an operation changes neither this file nor
//! anything the platform side has to regenerate: the command shape is defined
//! once, in `openpos-bindings`, and both platforms parse the same replies.
//!
//! # Safety
//!
//! Every function here is `unsafe` because it takes pointers from another
//! language, and the caller has to keep three rules that no type system spanning
//! the boundary can enforce:
//!
//! 1. A handle from `openpos_till_open_memory` is used until it is passed to
//!    `openpos_till_close`, and never afterwards.
//! 2. A string returned by this library is freed with `openpos_string_free`, and
//!    never with the platform's own allocator: it was allocated by Rust.
//! 3. A handle is not used from two threads at once. The till is a single
//!    cashier's, and serialising is the platform's job.
//!
//! Every one of them is checked where checking is possible: a null pointer is
//! answered rather than dereferenced, and a string that is not valid UTF-8 is
//! refused rather than assumed.

use std::ffi::{c_char, CStr, CString};

use openpos_bindings::TillHandle;

/// An opaque till. The platform holds the pointer and nothing else.
pub struct OpenposTill {
    inner: TillHandle,
}

/// Open a till whose data lives only in memory.
///
/// Nothing it holds survives the process. The Android build will open one on the
/// device's own store instead; this exists so a platform can prove its binding
/// works before any of that is wired up, which is a thing worth being able to do
/// separately.
///
/// Returns null when the identifiers are not valid ids.
///
/// # Safety
/// `tenant` and `terminal` must be null, or valid C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openpos_till_open_memory(
    tenant: *const c_char,
    terminal: *const c_char,
) -> *mut OpenposTill {
    // SAFETY: the caller promises these are null or terminated strings, and
    // `borrow` checks for null itself rather than trusting that half.
    let (Some(tenant), Some(terminal)) = (unsafe { borrow(tenant) }, unsafe { borrow(terminal) })
    else {
        return std::ptr::null_mut();
    };
    match TillHandle::open_in_memory(tenant, terminal) {
        Some(inner) => Box::into_raw(Box::new(OpenposTill { inner })),
        None => std::ptr::null_mut(),
    }
}

/// Carry out one command and describe the till afterwards.
///
/// `request` is a JSON command, the reply is a JSON view, and the caller frees
/// the reply with `openpos_string_free`. A null handle or an unreadable request
/// is answered with a view carrying an error, never with a null: a caller that
/// has to check for null before parsing has two failure paths to get right, and
/// the one it forgets is the one that matters.
///
/// # Safety
/// `till` must be a live handle from this library, `request` a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openpos_till_run(
    till: *mut OpenposTill,
    request: *const c_char,
) -> *mut c_char {
    // SAFETY: as above. A request that is null, or not text, is answered rather
    // than read.
    let Some(request) = (unsafe { borrow(request) }) else {
        return hand_out(refusal("the command was not readable text"));
    };
    // SAFETY: the caller promises a live handle. Null is checked rather than
    // trusted, because a null handle is the mistake a platform actually makes.
    let Some(till) = (unsafe { till.as_mut() }) else {
        return hand_out(refusal("the till handle was null"));
    };
    hand_out(till.inner.run_json(request))
}

/// Release a string this library returned.
///
/// # Safety
/// `text` must be a pointer this library returned and has not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openpos_string_free(text: *mut c_char) {
    if text.is_null() {
        return;
    }
    // SAFETY: the caller promises this came from `hand_out`, which built it with
    // `CString::into_raw`.
    drop(unsafe { CString::from_raw(text) });
}

/// Close a till and release it.
///
/// Committed sales are already durable; this only releases memory. A till whose
/// commits were not yet durable would be a till that lied when it let a receipt
/// print, which is the one thing the storage layer is built to prevent.
///
/// # Safety
/// `till` must be a handle from this library that has not already been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openpos_till_close(till: *mut OpenposTill) {
    if till.is_null() {
        return;
    }
    // SAFETY: the caller promises this came from `openpos_till_open_memory`.
    drop(unsafe { Box::from_raw(till) });
}

/// Borrow a C string as Rust text, or nothing if it is null or not UTF-8.
///
/// # Safety
/// `text` must be null or a valid C string.
unsafe fn borrow<'a>(text: *const c_char) -> Option<&'a str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: checked non-null, and the caller promises a terminated string.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

/// Hand a string to the caller, who frees it with `openpos_string_free`.
fn hand_out(text: String) -> *mut c_char {
    // A view is JSON built by this crate and holds no interior nul. Should one
    // ever appear, an error the caller can read beats a null it may not check.
    match CString::new(text) {
        Ok(text) => text.into_raw(),
        Err(_) => CString::new(r#"{"error":"the reply could not be represented"}"#)
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut()),
    }
}

fn refusal(message: &str) -> String {
    format!(r#"{{"lines":[],"error":{message:?}}}"#)
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use super::*;

    /// Drive the library exactly as a platform would: through pointers.
    fn call(till: *mut OpenposTill, request: &str) -> String {
        let request = CString::new(request).unwrap();
        let reply = unsafe { openpos_till_run(till, request.as_ptr()) };
        assert!(!reply.is_null(), "a reply is never null");
        let text = unsafe { CStr::from_ptr(reply) }.to_str().unwrap().to_owned();
        unsafe { openpos_string_free(reply) };
        text
    }

    fn open() -> *mut OpenposTill {
        let tenant = CString::new("0000000000000000000000002A").unwrap();
        let terminal = CString::new("00000000000000000000000007").unwrap();
        let till = unsafe { openpos_till_open_memory(tenant.as_ptr(), terminal.as_ptr()) };
        assert!(!till.is_null(), "a till opens");
        till
    }

    #[test]
    fn a_sale_rings_through_the_c_boundary() {
        let till = open();

        let applied = call(
            till,
            r#"{"op":"apply_items","items":[{"id":"00000000000000000000000001",
               "code":"RICE5","name":"Rice Miniket 5kg","price_minor":43000,"vat_bp":1500,
               "price_inclusive":false,"barcodes":["8690000000001"],"on_hand_milli":40000}]}"#,
        );
        assert!(applied.contains("\"error\":null"), "{applied}");

        let scanned = call(
            till,
            r#"{"op":"scan","barcode":"8690000000001","qty_milli":2000}"#,
        );
        // The same figures the native suite and the browser both produce, which
        // is the only thing that makes one core worth having.
        assert!(scanned.contains("\"net_minor\":86000"), "{scanned}");
        assert!(scanned.contains("\"vat_minor\":12900"), "{scanned}");
        assert!(scanned.contains("\"total_minor\":98900"), "{scanned}");

        let paid = call(till, r#"{"op":"add_cash","amount_minor":100000}"#);
        assert!(paid.contains("\"change_minor\":1100"), "{paid}");

        unsafe { openpos_till_close(till) };
    }

    #[test]
    fn a_null_handle_is_answered_rather_than_dereferenced() {
        // A caller that must check for null before parsing has two failure paths
        // to get right, and the one it forgets is the one that matters.
        let reply = call(std::ptr::null_mut(), r#"{"op":"view"}"#);
        assert!(reply.contains("the till handle was null"), "{reply}");
    }

    #[test]
    fn an_unreadable_command_is_refused_with_a_reason() {
        let till = open();
        let reply = call(till, "{not json");
        assert!(reply.contains("could not read that command"), "{reply}");
        unsafe { openpos_till_close(till) };
    }

    #[test]
    fn a_bad_identifier_gives_null_rather_than_a_till_that_is_not_one() {
        let bad = CString::new("not-a-ulid").unwrap();
        let till = unsafe { openpos_till_open_memory(bad.as_ptr(), bad.as_ptr()) };
        assert!(till.is_null());

        // And null pointers in, rather than a crash.
        let till = unsafe { openpos_till_open_memory(std::ptr::null(), std::ptr::null()) };
        assert!(till.is_null());
    }

    #[test]
    fn freeing_and_closing_nothing_is_harmless() {
        // A platform's teardown path runs after a failed open more often than
        // anybody plans for.
        unsafe { openpos_string_free(std::ptr::null_mut()) };
        unsafe { openpos_till_close(std::ptr::null_mut()) };
    }
}
