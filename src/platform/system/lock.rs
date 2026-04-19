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

impl std::fmt::Debug for FreezeLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FreezeLock").finish()
    }
}

impl FreezeLock {
    /// Attempt to acquire the exclusive lock using the default XDG_RUNTIME_DIR path.
    ///
    /// Returns `Ok(FreezeLock)` when the lock is acquired, or
    /// `Err(AppError::FreezeLockBusy)` if another instance is already running.
    pub fn acquire() -> Result<Self> {
        Self::acquire_at(lock_path()?)
    }

    /// Attempt to acquire the exclusive lock at an explicit path.
    ///
    /// Prefer [`acquire`] in production code. This variant exists to allow
    /// tests to supply a temporary path without touching environment variables.
    fn acquire_at(path: PathBuf) -> Result<Self> {
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
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| {
        AppError::Other(
            "XDG_RUNTIME_DIR is not set. This variable is required for IPC and lock files in Wayland environments.".to_string(),
        )
    })?;
    Ok(PathBuf::from(runtime_dir).join(LOCK_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freeze_lock_exclusive() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let lock_file = temp_dir.path().join(LOCK_FILE_NAME);

        // First acquisition should succeed
        let lock1 = FreezeLock::acquire_at(lock_file.clone());
        assert!(
            lock1.is_ok(),
            "First lock acquisition failed: {:?}",
            lock1.err()
        );

        // Second acquisition should fail with FreezeLockBusy
        let lock2 = FreezeLock::acquire_at(lock_file.clone());
        match lock2 {
            Err(AppError::FreezeLockBusy) => {}
            _ => panic!("Expected AppError::FreezeLockBusy, got {:?}", lock2),
        }

        // After dropping lock1, lock3 should succeed
        drop(lock1);
        let lock3 = FreezeLock::acquire_at(lock_file);
        assert!(
            lock3.is_ok(),
            "Lock acquisition failed after drop: {:?}",
            lock3.err()
        );
    }
}
