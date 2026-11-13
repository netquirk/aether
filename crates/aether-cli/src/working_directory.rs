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
//!
//! The module also owns [`resolve_run_cwd`], the rule that decides *which*
//! directory a run uses: the `cwd` key of an `--options-json` payload when it
//! carries one, otherwise the `-C/--cwd` flag.

use std::path::PathBuf;

use crate::error::CliError;

/// Resolve the working directory a run starts in.
///
/// Two places can name it: the `cwd` key of the `--options-json` payload and
/// the `-C/--cwd` flag. The payload wins when it carries one; otherwise the
/// flag decides (its default is `.`, the process's directory). The ACP
/// command resolves its session root the same way (`resolve_session_root` in
/// `crate::acp`), so both commands agree on what a caller asked for.
///
/// The result is canonicalized so every later step — the run lock, the
/// transcript, the workspace boundary — sees one absolute spelling of the
/// directory. A path that does not exist fails the run with a
/// [`CliError::IoError`] before any artefact is written.
pub(crate) fn resolve_run_cwd(payload_cwd: Option<PathBuf>, flag_cwd: PathBuf) -> Result<PathBuf, CliError> {
    payload_cwd.unwrap_or(flag_cwd).canonicalize().map_err(CliError::IoError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload's `cwd` wins over the `-C/--cwd` flag, and the flag is the
    /// fallback when the payload carries none. This is the rule the headless
    /// `--options-json` path and the ACP path share, so a caller that names a
    /// working directory always gets that directory.
    #[test]
    fn the_payload_cwd_wins_and_the_flag_is_the_fallback() {
        let payload_dir = tempfile::tempdir().expect("tempdir");
        let flag_dir = tempfile::tempdir().expect("tempdir");

        let from_payload = resolve_run_cwd(Some(payload_dir.path().to_path_buf()), flag_dir.path().to_path_buf())
            .expect("payload cwd");
        assert_eq!(
            from_payload,
            payload_dir.path().canonicalize().expect("canonical payload dir"),
            "the payload's cwd must win over the flag"
        );

        let from_flag = resolve_run_cwd(None, flag_dir.path().to_path_buf()).expect("flag cwd");
        assert_eq!(
            from_flag,
            flag_dir.path().canonicalize().expect("canonical flag dir"),
            "the flag must be the fallback when the payload carries no cwd"
        );
    }

    /// A working directory that does not exist fails the run before any
    /// artefact is written, with the underlying IO error preserved.
    #[test]
    fn a_missing_working_directory_fails_the_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("no-such-directory");
        let err = resolve_run_cwd(None, missing).expect_err("a missing directory must fail");
        assert!(matches!(err, CliError::IoError(_)), "the run must fail with an IO error; got {err:?}");
    }
}
