//! # platform::system::lock
//!
//! Exclusive process-level lock for freeze mode.
//!
//! Uses a BSD flock (`LOCK_EX | LOCK_NB`) on a file under `$XDG_RUNTIME_DIR`
//! via `nix::fcntl::Flock`. The lock is automatically released when
//! [`FreezeLock`] is dropped (file descriptor closed), so no explicit unlock
//! call is needed.

use std::{fs::OpenOptions, path::PathBuf};

use nix::fcntl::{Flock, FlockArg};

use crate::domain::error::{AppError, Result};

const LOCK_FILE_NAME: &str = "hyprcrop-freeze.lock";

/// RAII guard that holds the exclusive freeze lock for its lifetime.
pub struct FreezeLock {
    _flock: Flock<std::fs::File>,
}

impl FreezeLock {
    /// Attempt to acquire the exclusive lock.
    ///
    /// Returns `Ok(FreezeLock)` when the lock is acquired, or
    /// `Err(AppError::FreezeLockBusy)` if another instance is already running.
    pub fn acquire() -> Result<Self> {
        let path = lock_path()?;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| AppError::FileSystem(path.clone(), e))?;

        Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map(|_flock| Self { _flock })
            .map_err(|(_, e)| match e {
                nix::errno::Errno::EWOULDBLOCK => AppError::FreezeLockBusy,
                _ => AppError::Other(format!("flock failed on {}: {e}", path.display())),
            })
    }
}

fn lock_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map_err(|e| AppError::Other(format!("XDG_RUNTIME_DIR is not set: {e}")))?;
    Ok(PathBuf::from(runtime_dir).join(LOCK_FILE_NAME))
}
