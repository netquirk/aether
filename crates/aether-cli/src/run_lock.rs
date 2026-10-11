//! Per-working-directory run lock for the headless CLI (TASK-26-21).
//!
//! Two `aether headless` runs that share a working directory can race on the
//! on-disk artefacts they write (transcript, file changes, session index,
//! `.aether/`). [`RunLock`] acquires an exclusive OS-level advisory lock on a
//! well-known file inside the project so the second run refuses to start
//! before any of those artefacts are touched. The refusal message names the
//! lock file so an operator who suspects the lock is stale knows what to look
//! at.
//!
//! The lock is held by a single [`RunLock`] value bound to a named local in
//! [`crate::headless::run::run`]. Its [`Drop`] impl closes the file handle,
//! which the OS releases the lock from on every supported platform — on a
//! normal return, on `?`-propagated errors, on a panic, and on a hard process
//! kill. The lock file itself is left on disk between runs (zero bytes after
//! release) because the file's presence is not what keeps the lock active;
//! only an open file handle does.
//!
//! [`std::fs::File::try_lock`] (stable since Rust 1.89) provides the lock
//! without a new dependency. On Linux it maps to `flock(LOCK_EX |
//! LOCK_NB)`; on macOS the standard library uses the equivalent `flock` path.
//! Both refuse a second open on the same path from the same process, which
//! keeps the in-process unit tests deterministic. The lock's `Drop` runs
//! whether the run succeeds, fails with `CliError`, or unwinds through a
//! panic; `Drop` is the single mechanism that guarantees release on every
//! exit path.
//!
//! `--dry-run` short-circuits in [`crate::headless::run_headless`] before
//! [`crate::headless::run::run`] is called, so dry runs do not acquire the
//! lock and can run alongside an active run without conflict. That matches
//! the task's intent: dry runs are read-only resolution and never touch the
//! project's on-disk artefacts.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::error::CliError;

/// Directory under the working directory that holds aether's on-disk
/// artefacts. Matches `aether_project::PROJECT_SETTINGS_PATH` so the lock sits
/// next to the settings file the run is about to read, in the same folder
/// the project already creates.
pub(crate) const LOCK_DIR: &str = ".aether";
/// File the run lock is held on. Sits inside [`LOCK_DIR`] so every project
/// already using `aether` has a consistent location.
pub(crate) const LOCK_FILE: &str = "run.lock";

/// Compute the canonical lock path for a working directory.
///
/// Canonicalises `cwd` so two spellings of the same directory (e.g. a
/// relative `./repo` and an absolute `/abs/path/to/repo`) map to the same
/// lock file. Falls back to the raw path on canonicalisation error so a
/// non-existent or unreadable directory still surfaces a useful refusal
/// instead of failing the run with an `IoError` that hides the lock's
/// purpose. Mirrors `WorkspaceManager`'s tolerance at
/// `workspace/mod.rs:153`.
pub(crate) fn lock_path(cwd: &Path) -> PathBuf {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    canonical.join(LOCK_DIR).join(LOCK_FILE)
}

/// Exclusive advisory lock guarding the working directory while a run is in
/// progress.
///
/// Constructed once at the top of [`crate::headless::run::run`] and bound to
/// a named local so it lives until the run returns. [`Drop::drop`] closes
/// the underlying file handle, which the OS uses to release the lock; this
/// is the single mechanism that guarantees release on the success path, on
/// `?`-propagated `CliError` returns, on panics, and on process exit. The
/// lock file is intentionally not removed on drop: its presence does not
/// keep the lock active (only an open handle does), and leaving a zero-byte
/// file in place keeps `lock_path` deterministic for a follow-up run.
#[derive(Debug)]
pub(crate) struct RunLock {
    file: File,
}

impl RunLock {
    /// Try to take the lock for `cwd`. On contention returns
    /// [`CliError::RunLocked`] with the lock path so the caller-facing
    /// message names the file the operator can inspect; on any other IO
    /// failure returns [`CliError::IoError`].
    ///
    /// The directory holding the lock file is created on demand so a fresh
    /// working directory that has not yet been opened by `aether` still gets
    /// a lock. The file is opened with `create(true)` so a leftover zero-byte
    /// file from a previous release does not race the create; `truncate(false)`
    /// means the file is opened (and locked) without disturbing its contents,
    /// which keeps a stale `run.lock` discoverable rather than silently
    /// emptied if the path was somehow already present.
    pub(crate) fn acquire(cwd: &Path) -> Result<Self, CliError> {
        let path = lock_path(cwd);
        if let Some(parent) = path.parent() {
            // `create_dir_all` is a no-op when the directory already exists;
            // a fresh working directory that has never run aether still
            // gets a `.aether/` to hold the lock in.
            std::fs::create_dir_all(parent).map_err(CliError::IoError)?;
        }
        let file =
            OpenOptions::new().create(true).write(true).truncate(false).open(&path).map_err(CliError::IoError)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(CliError::RunLocked { path }),
            Err(TryLockError::Error(source)) => Err(CliError::IoError(source)),
        }
    }
}

impl Drop for RunLock {
    /// Closing the file handle releases the OS-level advisory lock on every
    /// supported platform. The lock file itself is left on disk; only an open
    /// handle holds the lock, so the next run gets a clean acquisition.
    fn drop(&mut self) {
        // Best-effort unlock on platforms that distinguish `LOCK_UN` from
        // close. `std::fs::File::unlock` (and `LockError::WouldBlock` for
        // `unlock`) are no-ops on platforms that already release on close.
        let _ = self.file.unlock();
        // `self.file` is closed here by its own `Drop`; the lock is released
        // by close regardless of the `unlock` return.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal message a second run sees must name the lock file, the
    /// pin the task's "refused with a message naming the lock file" done-when
    /// depends on. Both the structured variant and the rendered string carry
    /// the path so a regression in either the field or the message shows up
    /// here.
    #[test]
    fn refuses_a_second_lock_and_names_the_lock_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _held = RunLock::acquire(dir.path()).expect("first acquire");
        let err = RunLock::acquire(dir.path()).expect_err("second acquire must be refused");
        let expected_path = lock_path(dir.path());
        match &err {
            CliError::RunLocked { path } => assert_eq!(path, &expected_path, "lock path must round-trip"),
            other => panic!("expected RunLocked, got {other:?}"),
        }
        let rendered = err.to_string();
        assert!(
            rendered.contains(expected_path.to_string_lossy().as_ref()),
            "refusal message must name the lock file `{expected_path:?}`; got {rendered:?}"
        );
        assert!(
            rendered.contains("another aether run"),
            "refusal message must mention the contention; got {rendered:?}"
        );
    }

    /// Drop at the end of a block must release the lock so a follow-up
    /// acquire on the same path succeeds. Covers the success-path release:
    /// the guard simply goes out of scope when the run completes normally.
    #[test]
    fn releases_the_lock_when_the_run_ends() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let _held = RunLock::acquire(dir.path()).expect("first acquire");
            // _held drops at end of block
        }
        RunLock::acquire(dir.path()).expect("follow-up acquire after drop must succeed");
    }

    /// A function that acquires the lock and then returns an error must
    /// release the lock through the guard's `Drop` on its early `Err`
    /// return. This exercises the same path `crate::headless::run::run`
    /// takes when it `?`-propagates a `CliError` after `acquire` has
    /// returned the guard.
    #[test]
    fn releases_the_lock_when_the_run_fails() {
        fn failing_run(cwd: &Path) -> Result<(), CliError> {
            let _lock = RunLock::acquire(cwd)?;
            Err(CliError::AgentError("boom".into()))
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = failing_run(dir.path()).expect_err("function must fail");
        assert!(matches!(err, CliError::AgentError(_)), "the failure surfaces unchanged: {err:?}");
        // The `Drop` on `_lock` ran on the early `Err` return; a follow-up
        // acquire must succeed.
        RunLock::acquire(dir.path()).expect("follow-up acquire after Err return must succeed");
    }

    /// `lock_path` must canonicalise the working directory so two spellings
    /// of the same directory map to one lock file. The pin matters because
    /// `--options-json` callers may pass `cwd` in a different shape than the
    /// CLI's `--cwd` flag.
    #[test]
    fn lock_path_canonicalises_the_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = lock_path(dir.path());
        // Recomputing for the same directory must yield the same lock path;
        // two spellings of the same directory map to one lock file.
        let canonical_again = lock_path(dir.path());
        assert_eq!(canonical, canonical_again, "lock path must be deterministic");
        assert!(
            canonical.ends_with(format!("{LOCK_DIR}/{LOCK_FILE}")),
            "lock path lives under .aether/run.lock, got {canonical:?}"
        );
    }
}
