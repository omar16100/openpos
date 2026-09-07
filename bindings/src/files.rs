//! A till's store as ordinary files in a directory.
//!
//! The browser has OPFS and the server has Postgres. Everything else that could
//! run a till, an Android tablet through the C ABI, a desktop build, a support
//! tool that wants to open a device's store on a laptop, has a filesystem and
//! nothing else, and until now the only backend it could use was the one whose
//! type name says nothing survives a reload.
//!
//! One directory per terminal, one file per blob and per log. The layout is
//! deliberately dull: a person with a shop's tablet in their hand and no
//! openpos build should be able to see what is there, and copy it.
//!
//! # What `flush` promises
//!
//! `Backend::flush` is the barrier a receipt is printed on, so what it means
//! here is worth stating rather than assuming. It calls `sync_data` on every
//! file this backend has written and then on the directory, which on Linux and
//! on Android's ext4 or f2fs is a real barrier: the write survives the power
//! going out.
//!
//! On macOS `fsync` asks the drive to write its cache and does not wait for the
//! platters, and the call that does, `F_FULLFSYNC`, needs `libc` and unsafe,
//! which this workspace forbids outside the C ABI. macOS is where this is
//! developed and not where it is deployed, and saying so is better than a
//! comment claiming a promise the code does not keep.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use openpos_core::storage::backend::{Backend, BackendError, Blob, Result};
use openpos_core::storage::frame::Store;

/// A till's store, as files.
#[derive(Debug)]
pub struct FileBackend {
    home: PathBuf,
    /// Held open, because a sale appends to one of these and reopening a file
    /// per sale is a syscall on the one path that must stay short.
    critical: File,
    replica: File,
}

impl FileBackend {
    /// Open, creating the directory and the logs if this is a first morning.
    ///
    /// # Errors
    /// When the directory cannot be made, or a log cannot be opened.
    pub fn open(home: &Path) -> Result<Self> {
        std::fs::create_dir_all(home).map_err(|_| BackendError::Io)?;
        Ok(Self {
            home: home.to_path_buf(),
            critical: append_to(&log_path(home, Store::Critical))?,
            replica: append_to(&log_path(home, Store::ReplicaCache))?,
        })
    }

    fn log(&mut self, store: Store) -> &mut File {
        match store {
            Store::Critical => &mut self.critical,
            Store::ReplicaCache => &mut self.replica,
        }
    }

    /// Reopen a log after it has been cut back, so the append handle is not
    /// still pointing past the new end.
    fn reopen(&mut self, store: Store) -> Result<()> {
        let opened = append_to(&log_path(&self.home, store))?;
        match store {
            Store::Critical => self.critical = opened,
            Store::ReplicaCache => self.replica = opened,
        }
        Ok(())
    }
}

impl Backend for FileBackend {
    fn read_blob(&self, blob: Blob) -> Result<Vec<u8>> {
        read_whole(&blob_path(&self.home, blob))
    }

    fn write_blob(&mut self, blob: Blob, bytes: &[u8]) -> Result<()> {
        // Written beside and renamed over, because a blob is replaced whole and
        // a torn half of one is a terminal that has forgotten its receipt
        // numbers and its parked baskets. A rename within one directory is
        // atomic: readers see the old file or the new one.
        let path = blob_path(&self.home, blob);
        let mut scratch = path.clone();
        scratch.set_extension("writing");
        {
            let mut file = File::create(&scratch).map_err(|_| BackendError::Io)?;
            file.write_all(bytes).map_err(|_| BackendError::Io)?;
            file.sync_data().map_err(|_| BackendError::Io)?;
        }
        std::fs::rename(&scratch, &path).map_err(|_| BackendError::Io)
    }

    fn read_log(&self, store: Store) -> Result<Vec<u8>> {
        read_whole(&log_path(&self.home, store))
    }

    fn append_log(&mut self, store: Store, bytes: &[u8]) -> Result<()> {
        let file = self.log(store);
        file.write_all(bytes).map_err(|_| BackendError::Io)
    }

    fn truncate_log(&mut self, store: Store, len: usize) -> Result<()> {
        let path = log_path(&self.home, store);
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|_| BackendError::Io)?;
        file.set_len(len as u64).map_err(|_| BackendError::Io)?;
        file.sync_data().map_err(|_| BackendError::Io)?;
        // The append handle still believes the old end. Reopened rather than
        // seeked, so the next write cannot land in the hole a cut left.
        self.reopen(store)
    }

    fn flush(&mut self) -> Result<()> {
        self.critical.sync_data().map_err(|_| BackendError::Io)?;
        self.replica.sync_data().map_err(|_| BackendError::Io)?;
        // And the directory, or a file created since the last barrier can lose
        // its own name in a power cut while its contents survive.
        let home = File::open(&self.home).map_err(|_| BackendError::Io)?;
        home.sync_all().map_err(|_| BackendError::Io)
    }
}

/// Where a blob lives. Named for what it is, so a person looking at the
/// directory can tell.
fn blob_path(home: &Path, blob: Blob) -> PathBuf {
    home.join(match blob {
        Blob::SnapshotA => "snapshot-a.bin",
        Blob::SnapshotB => "snapshot-b.bin",
        Blob::TerminalA => "terminal-a.bin",
        Blob::TerminalB => "terminal-b.bin",
        Blob::Salvage => "salvage.bin",
    })
}

fn log_path(home: &Path, store: Store) -> PathBuf {
    home.join(match store {
        Store::Critical => "critical.log",
        Store::ReplicaCache => "replica.log",
    })
}

fn append_to(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| BackendError::Io)
}

/// A missing file reads as empty, which is what a first morning looks like.
fn read_whole(path: &Path) -> Result<Vec<u8>> {
    match File::open(path) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|_| BackendError::Io)?;
            Ok(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(BackendError::Io),
    }
}
