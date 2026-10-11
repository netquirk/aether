//! Per-session-id run lock for the headless CLI (TASK-26-163).
//!
//! Two `aether headless` runs that share a session id would race on the same
//! session transcript under [`$AETHER_HOME/sessions/<id>.jsonl`](crate).
//! The TASK-26-21 [`RunLock`](crate::run_lock::RunLock) catches a second run
//! sharing a working directory, but a run can clobber a session written by a
//! run in a *different* working directory; this guard closes that hole by
//! keying on the session id itself.
//!
//! The lock is an exclusive OS-level advisory lock on a well-known file
//! inside `$AETHER_HOME/sessions/`. A second acquisition of the same id is
//! refused before any agent build or transcript write. The refusal message
//! names the held session id and the lock file so an operator who suspects
//! the lock is stale knows exactly which session to release.
//!
//! The lock sits on a [`SessionLock`] value bound to a named local in
//! [`crate::headless::run::run`]. Its [`Drop`] impl closes the file handle,
//! which the OS releases the lock from on every supported platform — on a
//! normal return, on `?`-propagated errors, on a panic, and on a hard
//! process kill. The lock file itself is left on disk between runs (zero
//! bytes after release) because the file's presence is not what keeps the
//! lock active; only an open file handle does. Once the run completes the
//! same session id is available again for a fresh run.
//!
//! `std::fs::File::try_lock` (stable since Rust 1.89) provides the lock
//! without a new dependency. On Linux it maps to `flock(LOCK_EX | LOCK_NB)`;
//! on macOS the standard library uses the equivalent `flock` path. Both
//! refuse a second open on the same path from the same process, which
//! keeps the in-process unit tests deterministic. `Drop` runs whether the
//! run succeeds, fails with `CliError`, or unwinds through a panic; `Drop`
//! is the single mechanism that guarantees release on every exit path.
//!
//! `--dry-run` short-circuits in [`crate::headless::run_headless`] before
//! [`crate::headless::run::run`] is called, so dry runs do not acquire the
//! lock and can run alongside an active run without conflict. That matches
//! the session lock's intent: dry runs are read-only resolution and never
//! touch a session transcript.
//!
//! `aether_home()` may resolve to `None` on a host without `AETHER_HOME` or
//! `HOME` set. In that case `acquire` returns `Ok(None)` and emits a
//! [`tracing::warn!`]; the run proceeds without a session-id guard so
//! headless behaviour on home-less hosts is preserved. The docstring's
//! "lock is released when the run ends" guarantee applies once the guard
//! is taken; on a home-less host there is simply no guard to release.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use tracing::warn;
use utils::settings::aether_home;

use crate::error::CliError;

/// Directory under `aether_home` that holds both the session transcripts and
/// these locks. Matches the `SessionStore` location
/// (`crates/aether-sessions/src/store/mod.rs`) so the lock sits next to the
/// file it is meant to protect.
pub(crate) const SESSION_LOCK_DIR: &str = "sessions";
/// File the session lock is held on. The `<session_id>.lock` suffix avoids
/// any collision with the existing `<session_id>.jsonl` transcripts; the
/// extension can never be a transcript file.
pub(crate) const SESSION_LOCK_EXT: &str = "lock";

/// Compute the canonical lock path for a session id inside `dir`.
///
/// Centralised so the integration test (which holds the test lock on the
/// same path the CLI expects) and the production call site agree on the
/// exact spelling — a discrepancy would let a regression pass the unit
/// tests and fail the binary in production.
pub(crate) fn lock_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.{SESSION_LOCK_EXT}"))
}

/// Reject session ids that would resolve outside `dir` once joined into a
/// lock path. Empty, `.`, `..`, and any id containing a path separator
/// (`/` or `\`) all fall through to a [`CliError::ConflictingArgs`] with a
/// message naming the offending value so the operator sees the refusal
/// immediately at parse time. Empty ids are a no-op the rest of the code
/// already short-circuits on; the dot-doublet and separators are the
/// path-traversal defence.
pub(crate) fn validate_session_id(session_id: &str) -> Result<(), CliError> {
    if session_id.is_empty() {
        return Err(CliError::ConflictingArgs("--session-id must not be empty".to_string()));
    }
    if session_id == "." || session_id == ".." {
        return Err(CliError::ConflictingArgs(format!(
            "--session-id must not be {session_id:?}: a path-traversal defence forbids it"
        )));
    }
    if session_id.contains('/') || session_id.contains('\\') {
        return Err(CliError::ConflictingArgs(format!(
            "--session-id must not contain a path separator: {session_id:?}"
        )));
    }
    Ok(())
}

/// Exclusive advisory lock guarding a session id while a run is in progress.
///
/// Constructed once at the top of [`crate::headless::run::run`] and bound
/// to a named local so it lives until the run returns. [`Drop::drop`]
/// closes the underlying file handle, which the OS uses to release the
/// lock; this is the single mechanism that guarantees release on the
/// success path, on `?`-propagated `CliError` returns, on panics, and on
/// process exit. The lock file is intentionally not removed on drop: its
/// presence does not keep the lock active (only an open handle does), and
/// leaving a zero-byte file in place keeps `lock_path` deterministic for a
/// follow-up run.
#[derive(Debug)]
pub(crate) struct SessionLock {
    // Wrapped in `Option` so the `Drop` impl can run `unlock()` first and
    // then drop the handle in a single block; `File` itself is not
    // constructible from a default, and the handle drops via its own
    // `Drop` either way.
    file: File,
}

impl SessionLock {
    /// Try to take the lock for `session_id` under the current aether
    /// home. Returns:
    ///
    /// - `Ok(Some(guard))` when the lock is held by this process.
    /// - `Ok(None)` when `aether_home()` is `None` (home-less host).
    ///   The run proceeds without a guard so behaviour on such hosts is
    ///   preserved; a [`tracing::warn!`] records the bypass.
    /// - `Err(CliError::SessionLocked { session_id, path })` on
    ///   contention. The rendered message names the held id and the lock
    ///   file the operator can inspect.
    /// - `Err(CliError::ConflictingArgs)` when the id is malformed (empty,
    ///   `.`, `..`, contains a path separator).
    /// - `Err(CliError::IoError)` on any other IO failure (e.g. the home
    ///   directory cannot be created).
    ///
    /// Validates the id, resolves the home, then defers to
    /// [`Self::acquire_in`] for the actual file work. The directory
    /// holding the lock file is created on demand so a fresh `aether
    /// home` that has not yet been opened still gets a `sessions/`
    /// directory; the file is opened with `create(true)` so a leftover
    /// zero-byte file from a previous release does not race the create.
    pub(crate) fn acquire(session_id: &str) -> Result<Option<Self>, CliError> {
        validate_session_id(session_id)?;
        let dir = if let Some(home) = aether_home() {
            home.join(SESSION_LOCK_DIR)
        } else {
            warn!(
                session_id = %session_id,
                "aether_home unavailable: skipping --session-id lock (HOME/AETHER_HOME unset); \
                a second run holding the same session id may clobber this run's transcript"
            );
            return Ok(None);
        };
        Self::acquire_in(&dir, session_id).map(Some)
    }

    /// Acquire the lock for `session_id` under the directory `dir` (used
    /// directly by tests; the call site goes through `acquire`). Mirrors
    /// [`RunLock::acquire`](crate::run_lock::RunLock::acquire) so the two
    /// locks read identically to a future operator looking at the crate.
    fn acquire_in(dir: &Path, session_id: &str) -> Result<Self, CliError> {
        // `create_dir_all` is a no-op when the directory already exists;
        // a fresh `aether home` that has never run aether still gets a
        // `sessions/` folder to hold the lock in.
        std::fs::create_dir_all(dir).map_err(CliError::IoError)?;
        let path = lock_path(dir, session_id);
        let file =
            OpenOptions::new().create(true).write(true).truncate(false).open(&path).map_err(CliError::IoError)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(CliError::SessionLocked { session_id: session_id.into(), path }),
            Err(TryLockError::Error(source)) => Err(CliError::IoError(source)),
        }
    }
}

impl Drop for SessionLock {
    /// Closing the file handle releases the OS-level advisory lock on
    /// every supported platform. The lock file itself is left on disk;
    /// only an open handle holds the lock, so the next run gets a clean
    /// acquisition.
    fn drop(&mut self) {
        // Best-effort unlock on platforms that distinguish `LOCK_UN`
        // from close. `std::fs::File::unlock` is a no-op on platforms
        // that already release on close (Linux, macOS today).
        let _ = self.file.unlock();
        // `self.file` is closed here by its own `Drop`; the lock is
        // released by close regardless of the `unlock` return.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal message a second run sees must name the session id, the
    /// pin the task's "refused with an error naming the session id"
    /// done-when depends on. Both the structured variant and the
    /// rendered string carry the id so a regression in either the field
    /// or the message shows up here.
    #[test]
    fn refuses_a_second_lock_and_names_the_session_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _held = SessionLock::acquire_in(dir.path(), "shared").expect("first acquire");
        let err = SessionLock::acquire_in(dir.path(), "shared").expect_err("second acquire must be refused");
        let expected_path = lock_path(dir.path(), "shared");
        match &err {
            CliError::SessionLocked { session_id, path } => {
                assert_eq!(session_id, "shared", "session id must round-trip in the variant");
                assert_eq!(path, &expected_path, "lock path must round-trip in the variant");
            }
            other => panic!("expected SessionLocked, got {other:?}"),
        }
        let rendered = err.to_string();
        assert!(rendered.contains("shared"), "refusal message must name the session id 'shared'; got {rendered:?}");
        assert!(
            rendered.contains("another aether run"),
            "refusal message must mention the contention; got {rendered:?}"
        );
        assert!(
            rendered.contains(expected_path.to_string_lossy().as_ref()),
            "refusal message must name the lock file `{expected_path:?}`; got {rendered:?}"
        );
    }

    /// Two different session ids must not contend — the lock is keyed on
    /// the id, not the directory, so two sessions under the same home
    /// can be active in parallel. This guard pins the "different code
    /// path" axis that distinguishes TASK-26-163 from TASK-26-21.
    #[test]
    fn different_session_ids_do_not_contend() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _first = SessionLock::acquire_in(dir.path(), "first").expect("first acquires");
        SessionLock::acquire_in(dir.path(), "second").expect("second session must not contend with first");
    }

    /// Drop at the end of a block must release the lock so a follow-up
    /// acquire on the same session id succeeds. Covers the success-path
    /// release: the guard simply goes out of scope when the run
    /// completes normally.
    #[test]
    fn releases_the_lock_when_the_run_ends() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let _held = SessionLock::acquire_in(dir.path(), "shared").expect("first acquire");
            // _held drops at end of block
        }
        SessionLock::acquire_in(dir.path(), "shared").expect("follow-up acquire after drop must succeed");
    }

    /// A function that acquires the lock and then returns an error must
    /// release the lock through the guard's `Drop` on its early `Err`
    /// return. This exercises the same path
    /// [`crate::headless::run::run`] takes when it `?`-propagates a
    /// `CliError` after `acquire` has returned the guard.
    #[test]
    fn releases_the_lock_when_the_run_fails() {
        fn failing_run(dir: &Path, session_id: &str) -> Result<(), CliError> {
            let _lock = SessionLock::acquire_in(dir, session_id)?;
            Err(CliError::AgentError("boom".into()))
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = failing_run(dir.path(), "shared").expect_err("function must fail");
        assert!(matches!(err, CliError::AgentError(_)), "the failure surfaces unchanged: {err:?}");
        // The `Drop` on `_lock` ran on the early `Err` return; a
        // follow-up acquire must succeed on the same id.
        SessionLock::acquire_in(dir.path(), "shared").expect("follow-up acquire after Err return must succeed");
    }

    /// `validate_session_id` must reject the four shapes that would let a
    /// caller escape the lock directory: empty, `.`, `..`, and any id
    /// containing a `/` or `\` separator. All four surface as
    /// `ConflictingArgs` so the refusal reads as a CLI misuse rather
    /// than an IO error.
    #[test]
    fn invalid_session_id_is_rejected() {
        for bad in ["", ".", "..", "a/b", r"a\b", "nested/path"] {
            let err = validate_session_id(bad).expect_err(&format!("{bad:?} must be rejected"));
            match err {
                CliError::ConflictingArgs(_) => {}
                other => panic!("expected ConflictingArgs for {bad:?}, got {other:?}"),
            }
            let rendered = err.to_string();
            assert!(
                rendered.contains("--session-id"),
                "rejection message must name --session-id for {bad:?}; got {rendered:?}"
            );
        }
    }

    /// `lock_path` joins the session id and the `lock` extension under
    /// the supplied directory; this is what both the production call site
    /// and the integration test depend on, so a regression shows up here
    /// and in the integration test together.
    #[test]
    fn lock_path_uses_lock_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = lock_path(dir.path(), "abc");
        assert_eq!(path, dir.path().join("abc.lock"));
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some(SESSION_LOCK_EXT),
            "lock path extension must be {SESSION_LOCK_EXT:?}, got {path:?}"
        );
    }
}
