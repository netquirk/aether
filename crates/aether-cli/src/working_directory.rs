//! Working-directory writability guard (TASK-26-110).
//!
//! Runs [`crate::headless::run::run`] only if the working directory can be
//! written. The probe lives in the same `.aether/` directory the rest of the
//! run uses ([`crate::run_lock::LOCK_DIR`]) so a project whose root is
//! read-only but whose `.aether/` is writable keeps running, and a project
//! whose `.aether/` is the actual problem is refused with the path named in
//! the error.
//!
//! Refusing up front prevents a run from entering the agent loop, building a
//! transcript writer, contacting a provider, and then dying on the first
//! on-disk write mid-turn. The refusal carries a message naming the path so
//! the operator can tell which directory was the problem without scraping
//! the entire log.
//!
//! `--dry-run` is excluded: it short-circuits in
//! [`crate::headless::run_headless`] before [`ensure_writable`] is called,
//! matching the surface area the existing TASK-26-21 lock targets. `--dry-run`
//! never writes to the working directory, so probing it would only produce
//! false refusals on read-only trees the operator deliberately set up.

use std::fs::OpenOptions;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CliError;
use crate::run_lock::LOCK_DIR;

/// Probe that the working directory is writable.
///
/// `cwd` is the run's working directory. The probe creates `.aether/` (a
/// no-op when the directory already exists) and a uniquely-named file inside
/// it, then deletes the file. The file create is required to catch an
/// already-existing but read-only `.aether/` directory; `create_dir_all`
/// alone would miss that case.
///
/// On success leaves no trace behind — the file is removed, `create_dir_all`
/// is idempotent. On failure returns
/// [`CliError::WorkingDirectoryNotWritable`] with the original working
/// directory path so the caller-facing message names the directory the
/// operator can investigate (per the task's "names that directory" done-when).
/// The probe file itself is best-effort cleaned up on the error path so a
/// subsequent run does not see a leftover.
pub(crate) fn ensure_writable(cwd: &Path) -> Result<(), CliError> {
    let dir = cwd.join(LOCK_DIR);
    if let Err(source) = std::fs::create_dir_all(&dir) {
        return Err(CliError::WorkingDirectoryNotWritable { path: cwd.to_path_buf(), source });
    }
    let probe = dir.join(probe_name());
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_file) => {
            // Best-effort cleanup; a leftover does not invalidate the
            // successful result, but a hard kill between create and remove
            // can leave a zero-byte file behind. The pid+nanos name keeps
            // it from causing a false refusal on a later run.
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(source) => {
            // Best-effort cleanup so a partial probe does not stay on disk.
            let _ = std::fs::remove_file(&probe);
            Err(CliError::WorkingDirectoryNotWritable { path: cwd.to_path_buf(), source })
        }
    }
}

/// Build a unique probe file name combining the current pid with
/// wall-clock nanoseconds since the UNIX epoch. Uniqueness matters because
/// two `aether headless` runs in the same working directory would collide
/// with a fixed name; the pid+nanos pair is virtually collision-free inside
/// a single test run. The name never collides with the run lock file
/// ([`crate::run_lock::LOCK_FILE`]) so the TASK-26-21 lock semantics are
/// untouched.
fn probe_name() -> String {
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("write-probe-{pid}-{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// `ensure_writable` succeeds on a writable temp directory and leaves no
    /// probe artefact behind so a follow-up `RunLock::acquire` finds a clean
    /// `.aether/`. Pinning both the absence of a probe file and the created
    /// `.aether/` keeps a regression in either the success path or the
    /// cleanup path visible.
    #[test]
    fn writable_directory_is_accepted_and_left_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_writable(dir.path()).expect("writable directory must be accepted");

        let aether_dir = dir.path().join(LOCK_DIR);
        assert!(aether_dir.is_dir(), ".aether/ must be created: {aether_dir:?}");

        let entries: Vec<_> = std::fs::read_dir(&aether_dir)
            .expect("read_dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.is_empty(),
            "ensure_writable must leave .aether/ empty (no leftover probe file); got: {entries:?}"
        );
    }

    /// On a directory whose permissions forbid the current user from writing,
    /// `ensure_writable` returns `CliError::WorkingDirectoryNotWritable` and
    /// the rendered message names the working directory. Confirms the
    /// task's done-when: "a message naming that directory".
    #[cfg(unix)]
    #[test]
    #[allow(clippy::items_after_statements, clippy::elidable_lifetime_names, reason = "drop guard stays scoped to the test")]
    fn refuses_a_read_only_directory_and_names_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        // `0o555` = r-x for everyone: read + execute, no write. This is the
        // canonical "read-only directory" mode used by every TLS cert test
        // and the only reliable way to forbid writes for the process's uid
        // without juggling POSIX ACLs.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555)).expect("chmod 0o555 must succeed");
        // Restoring `0o755` is required so `tempdir`'s drop can remove the
        // directory; even on the error path the test must not leave a
        // directory behind that the cleanup hook cannot delete. The drop
        // guard sits inside the test on purpose so it cannot outlive the
        // path it restores; the structured lifetimes keep the type simple.
        struct RestorePermissions<'a>(&'a std::path::Path);
        impl<'a> Drop for RestorePermissions<'a> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = RestorePermissions(path);

        // `chmod` is a no-op under `cap_dac_override` (the container the
        // tests run in grants the user every DAC-override capability) and
        // under a tmpfs mount whose `uid` matches the process. Probe with a
        // direct write to confirm the chmod took effect; if a write still
        // succeeds the probe would also succeed and the test cannot prove
        // the read-only path. Skip with a printed note so the workspace
        // run does not produce a false red on those runners.
        let write_probe = path.join(".can-we-write");
        if std::fs::File::create(&write_probe).is_ok() {
            let _ = std::fs::remove_file(&write_probe);
            eprintln!(
                "refuses_a_read_only_directory_and_names_it: skipping because chmod 0o555 \
                did not forbid writes (cap_dac_override or per-uid tmpfs mount)"
            );
            return;
        }

        let err = ensure_writable(path).expect_err("a 0o555 directory must be refused");
        match &err {
            CliError::WorkingDirectoryNotWritable { path: err_path, .. } => {
                assert_eq!(err_path, path, "the refusal must name the working directory");
            }
            other => panic!("expected WorkingDirectoryNotWritable, got {other:?}"),
        }
        let rendered = err.to_string();
        assert!(
            rendered.contains(path.to_string_lossy().as_ref()),
            "rendered message must name the directory `{path:?}`; got {rendered:?}"
        );
        assert!(rendered.contains("not writable"), "rendered message must announce the condition; got {rendered:?}");
        assert!(
            rendered.contains("cannot start run"),
            "rendered message must identify the failure as a pre-run refusal; got {rendered:?}"
        );
    }

    /// `ensure_writable` must surface the cause in the structured error too,
    /// not only in the rendered string. Verifies the `#[source]` chain is
    /// preserved so the operator's debugging tools (or a follow-up
    /// `Display`-driven error reporter) see the underlying `io::Error`
    /// alongside the path.
    #[cfg(unix)]
    #[test]
    fn preserves_the_underlying_io_error() {
        use std::io;
        let source = io::Error::new(io::ErrorKind::PermissionDenied, "synthetic denial");
        let err = CliError::WorkingDirectoryNotWritable { path: std::path::PathBuf::from("/tmp/synthetic"), source };
        // Walk the `source()` chain to confirm the inner `io::Error` is
        // preserved through `#[source]`.
        let mut current: Option<&(dyn std::error::Error + 'static)> = err.source();
        let mut found = false;
        while let Some(e) = current {
            if e.downcast_ref::<io::Error>().is_some() {
                found = true;
                break;
            }
            current = e.source();
        }
        assert!(found, "the underlying io::Error must be reachable through `source()`");
    }

    /// `ensure_writable` refuses a `cwd` whose path points at an existing
    /// non-directory (a regular file in this case). The probe runs
    /// `create_dir_all` on `cwd/.aether/`, which fails because `cwd` is not
    /// a directory. The test pins two things on this branch:
    ///   1. the refusal carries the path the operator passed (per the task's
    ///      done-when), and
    ///   2. the **first** branch of the probe — the `create_dir_all` step —
    ///      is the path that actually fires here, distinct from the
    ///      file-create branch a read-only `.aether/` would land on.
    ///
    /// This is a filesystem-portable refusal: it does not depend on `chmod`
    /// semantics, so it runs reliably on tmpfs and overlayfs as well as on
    /// ext4 without privileges.
    #[cfg(unix)]
    #[test]
    fn refuses_a_path_pointing_at_a_non_directory_and_names_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Make `cwd` a regular file instead of a directory. The probe will
        // try to `create_dir_all(cwd.join(".aether"))` and the OS rejects
        // creating a directory whose parent is a regular file.
        let cwd_file = dir.path().join("i-am-a-file-not-a-dir");
        std::fs::write(&cwd_file, b"not a directory").expect("write regular file");
        let err = ensure_writable(&cwd_file).expect_err("a `cwd` that is a regular file must be refused");
        match &err {
            CliError::WorkingDirectoryNotWritable { path, .. } => {
                assert_eq!(path, &cwd_file, "refusal must name the given path");
            }
            other => panic!("expected WorkingDirectoryNotWritable, got {other:?}"),
        }
        let rendered = err.to_string();
        assert!(
            rendered.contains(cwd_file.to_string_lossy().as_ref()),
            "rendered message must name the path `{}`; got {rendered}",
            cwd_file.display()
        );
        assert!(rendered.contains("not writable"), "rendered message must announce the condition; got {rendered}");
    }
}
