//! Durable storage in the browser, over OPFS sync access handles.
//!
//! This is the backend the browser till actually runs on, and it is the reason
//! the core's storage trait is synchronous. A sync access handle has real
//! `read`, `write`, `truncate` and `flush` methods that return values rather
//! than promises, so a sale commit is a straight line with no suspension point
//! in the middle where a second scan could arrive.
//!
//! Sync access handles exist only in a worker. That is not a preference: the
//! browser refuses to create one on the main thread. So the till runs in a
//! dedicated worker and the UI talks to it by message, which is the arrangement
//! the architecture assumed and this file is where that assumption gets paid
//! for.
//!
//! Opening the handles is asynchronous and happens in JavaScript, before this
//! type exists. Everything after that is synchronous, which is what the durable
//! commit path needs.

use alloc_shim::Vec;
use openpos_core::storage::backend::{Backend, BackendError, Blob, Result};
use openpos_core::storage::frame::Store;
use wasm_bindgen::prelude::*;
use web_sys::{FileSystemReadWriteOptions, FileSystemSyncAccessHandle};

mod alloc_shim {
    pub use std::vec;
    pub use std::vec::Vec;
}

use std::format;
use std::string::{String, ToString};

/// Every file the till keeps, in the order the JS side hands them over.
///
/// Named rather than positional in the JavaScript, because a caller that swaps
/// two handles by accident would give a shop its snapshot in place of its sales
/// ledger, and nothing downstream would notice until a reboot.
pub const FILE_NAMES: [&str; 7] = [
    "critical.log",
    "replica.log",
    "snapshot-a.bin",
    "snapshot-b.bin",
    "terminal-a.bin",
    "terminal-b.bin",
    "salvage.bin",
];

/// Sizes of the handles as the browser reports them, for diagnostics.
#[must_use]
pub fn sizes(handles: &js_sys::Array) -> Vec<f64> {
    (0..handles.length())
        .map(|index| {
            handles
                .get(index)
                .dyn_into::<FileSystemSyncAccessHandle>()
                .ok()
                .and_then(|handle| handle.get_size().ok())
                .unwrap_or(-1.0)
        })
        .collect()
}

/// Read the critical log through the backend and describe what came back, for
/// telling a broken read apart from a broken write.
#[must_use]
pub fn peek_critical(handles: &js_sys::Array) -> String {
    let Some(backend) = OpfsBackend::from_handles(handles) else {
        return String::from("could not take the handles");
    };
    match backend.read_log(Store::Critical) {
        Ok(bytes) => {
            let head: Vec<String> = bytes.iter().take(8).map(|b| b.to_string()).collect();
            format!("read {} bytes, first8=[{}]", bytes.len(), head.join(","))
        }
        Err(error) => format!("read failed: {error}"),
    }
}

/// Exercise every operation the storage contract promises, and name the first
/// one that does not hold.
///
/// Worth keeping rather than deleting once this file worked. A till that will
/// not open is the worst thing that happens to a shop, and the difference
/// between "this browser will not flush" and "this ledger is corrupt" decides
/// whether somebody restores a backup or buys a different tablet. It writes only
/// to the salvage slot, which holds nothing a running till depends on.
#[must_use]
pub fn self_test(handles: &js_sys::Array) -> String {
    let Some(mut backend) = OpfsBackend::from_handles(handles) else {
        return String::from("could not take the handles");
    };

    let probe: &[u8] = b"openpos storage self test";
    if let Err(error) = backend.write_blob(Blob::Salvage, probe) {
        return format!("write_blob failed: {error}");
    }
    match backend.read_blob(Blob::Salvage) {
        Ok(bytes) if bytes == probe => {}
        Ok(bytes) => {
            return format!("read_blob returned {} bytes, expected {}", bytes.len(), probe.len())
        }
        Err(error) => return format!("read_blob failed: {error}"),
    }
    if let Err(error) = backend.flush() {
        return format!("flush failed: {error}");
    }
    if let Err(error) = backend.write_blob(Blob::Salvage, b"") {
        return format!("write_blob of nothing failed: {error}");
    }
    match backend.read_blob(Blob::Salvage) {
        Ok(bytes) if bytes.is_empty() => {}
        Ok(bytes) => return format!("an emptied blob read back {} bytes", bytes.len()),
        Err(error) => return format!("read_blob after emptying failed: {error}"),
    }
    String::from("ok")
}

/// Storage backed by OPFS.
pub struct OpfsBackend {
    critical: FileSystemSyncAccessHandle,
    replica: FileSystemSyncAccessHandle,
    snapshot_a: FileSystemSyncAccessHandle,
    snapshot_b: FileSystemSyncAccessHandle,
    terminal_a: FileSystemSyncAccessHandle,
    terminal_b: FileSystemSyncAccessHandle,
    salvage: FileSystemSyncAccessHandle,
}

impl OpfsBackend {
    /// Take ownership of seven already-opened handles, in `FILE_NAMES` order.
    ///
    /// Returns `None` rather than opening fewer files than it needs. A till
    /// missing one of these is a till that cannot keep a promise it will make
    /// to a customer within the minute.
    #[must_use]
    pub fn from_handles(handles: &js_sys::Array) -> Option<Self> {
        if handles.length() as usize != FILE_NAMES.len() {
            return None;
        }
        let at = |index: u32| handles.get(index).dyn_into::<FileSystemSyncAccessHandle>().ok();
        Some(Self {
            critical: at(0)?,
            replica: at(1)?,
            snapshot_a: at(2)?,
            snapshot_b: at(3)?,
            terminal_a: at(4)?,
            terminal_b: at(5)?,
            salvage: at(6)?,
        })
    }

    fn blob_handle(&self, blob: Blob) -> &FileSystemSyncAccessHandle {
        match blob {
            Blob::SnapshotA => &self.snapshot_a,
            Blob::SnapshotB => &self.snapshot_b,
            Blob::TerminalA => &self.terminal_a,
            Blob::TerminalB => &self.terminal_b,
            Blob::Salvage => &self.salvage,
        }
    }

    fn log_handle(&self, store: Store) -> &FileSystemSyncAccessHandle {
        match store {
            Store::Critical => &self.critical,
            Store::ReplicaCache => &self.replica,
        }
    }

    /// Read a whole file.
    ///
    /// A missing or empty file reads as no bytes rather than as an error,
    /// because a shop's first morning legitimately has neither a snapshot nor a
    /// ledger and must still open.
    fn read_all(handle: &FileSystemSyncAccessHandle) -> Result<Vec<u8>> {
        let size = handle.get_size().map_err(|_| BackendError::Io)?;
        // `get_size` returns a double. A file this large cannot exist here, and
        // truncating one silently would hand back a prefix of a shop's ledger
        // as though it were the whole thing.
        if !size.is_finite() || size < 0.0 || size > f64::from(u32::MAX) {
            return Err(BackendError::Io);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let len = size as usize;

        let mut buffer = alloc_shim::vec![0_u8; len];
        if len == 0 {
            return Ok(buffer);
        }

        // The browser fills a buffer it owns, and the bytes are copied into the
        // wasm heap afterwards. Handing it a view over wasm memory instead is
        // the shape that looks obvious and is not safe: growing the heap during
        // the call detaches the view the browser is writing into.
        let scratch = js_sys::Uint8Array::new_with_length(
            u32::try_from(len).map_err(|_| BackendError::Io)?,
        );

        // The offset is always stated, never left to default.
        //
        // A handle carries an implicit position that a write advances, so a read
        // issued after a write starts where that write finished and returns
        // nothing. Every read here wants the whole file from the beginning, and
        // saying so costs a line. Found in Chrome: a blob written and read back
        // in the same breath came back empty, and a till would have booted with
        // an empty ledger rather than refusing to boot at all.
        let options = FileSystemReadWriteOptions::new();
        options.set_at(0.0);
        let read = handle
            .read_with_js_u8_array_and_options(&scratch, &options)
            .map_err(|_| BackendError::Io)?;
        scratch.copy_to(&mut buffer);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let read = read as usize;
        // A short read means the file changed under us or the handle is not
        // what it claimed. Reporting it beats returning a truncated ledger that
        // recovery would then treat as a torn tail and cut back further.
        if read != len {
            return Err(BackendError::Io);
        }
        Ok(buffer)
    }

    fn write_at(handle: &FileSystemSyncAccessHandle, at: f64, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let options = FileSystemReadWriteOptions::new();
        options.set_at(at);

        // Copied into a buffer the browser owns, for the same reason the read
        // path does: a view over the wasm heap can be detached out from under
        // the call.
        let scratch = js_sys::Uint8Array::new_with_length(
            u32::try_from(bytes.len()).map_err(|_| BackendError::Io)?,
        );
        scratch.copy_from(bytes);
        let written = handle
            .write_with_js_u8_array_and_options(&scratch, &options)
            .map_err(|_| BackendError::Io)?;

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let written = written as usize;
        // A partial write is the torn write the frame checksums exist to catch,
        // but there is no reason to let it pass silently here: the caller is
        // told the commit failed and will roll back.
        if written != bytes.len() {
            return Err(BackendError::Io);
        }
        Ok(())
    }
}

impl Backend for OpfsBackend {
    fn read_blob(&self, blob: Blob) -> Result<Vec<u8>> {
        Self::read_all(self.blob_handle(blob))
    }

    fn write_blob(&mut self, blob: Blob, bytes: &[u8]) -> Result<()> {
        let handle = self.blob_handle(blob);
        // Truncate first, so a shorter payload cannot leave the tail of a longer
        // one behind it. The A/B slot scheme is what makes this safe to do in
        // place: the slot being overwritten is never the one in use.
        handle.truncate_with_u32(0).map_err(|_| BackendError::Io)?;
        Self::write_at(handle, 0.0, bytes)
    }

    fn read_log(&self, store: Store) -> Result<Vec<u8>> {
        Self::read_all(self.log_handle(store))
    }

    fn append_log(&mut self, store: Store, bytes: &[u8]) -> Result<()> {
        let handle = self.log_handle(store);
        let end = handle.get_size().map_err(|_| BackendError::Io)?;
        Self::write_at(handle, end, bytes)
    }

    fn truncate_log(&mut self, store: Store, len: usize) -> Result<()> {
        let handle = self.log_handle(store);
        let len = u32::try_from(len).map_err(|_| BackendError::Io)?;
        handle.truncate_with_u32(len).map_err(|_| BackendError::Io)
    }

    fn flush(&mut self) -> Result<()> {
        // Every handle, not just the one last written. The trait's barrier is
        // global: a sale commit writes the critical log, and a checkpoint writes
        // a blob and then truncates a log, and the ordering guarantees in the
        // journal assume one flush covers all of it. Flushing only the last file
        // touched would make those guarantees false in exactly the crash the
        // journal was written to survive.
        for handle in [
            &self.critical,
            &self.replica,
            &self.snapshot_a,
            &self.snapshot_b,
            &self.terminal_a,
            &self.terminal_b,
            &self.salvage,
        ] {
            handle.flush().map_err(|_| BackendError::Io)?;
        }
        Ok(())
    }
}
