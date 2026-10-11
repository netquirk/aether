//! Integration tests for `aether headless --session-id ID` (TASK-26-163).
//!
//! Two `aether headless` runs holding the same `--session-id` cannot
//! simultaneously open the session transcript they would clobber. The
//! `--session-id ID` flag carries the id, and the lock sits under
//! `$AETHER_HOME/sessions/<id>.lock` (`SessionStore`-style central layout).
//! The `cwd_not_writable.rs` pair of tests pins the pre-existing cwd-lock
//! semantics; this file pins the new session-id lock as a separate axis so a
//! regression in one cannot mask a regression in the other.
//!
//! The end-to-end test holds the lock from the test process itself
//! (`fs::OpenOptions::create().read(true).write(true)` +
//! `try_lock`) before spawning the binary; the binary refuses with
//! `SessionLocked` because the file is already locked. The release criterion
//! (the lock must come back when the run ends) is pinned by the unit tests
//! in `src/session_lock.rs`; the integration test focuses on the "names
//! the session id" contract the task's done-when requires.

use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Inline `--settings-json` block an `aether headless` run can reach the
/// lock check with. Carries only the credential-store knob and an empty
/// agents list, matching the offline canary every other "reaches the lock
/// before any session work" test relies on (e.g. `cwd_not_writable.rs`).
const SETTINGS_JSON: &str = r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#;

/// RAII guard that owns an open file handle whose underlying OS-level
/// advisory lock is released as soon as `Drop` runs. The lifetime of the
/// lock is tied to the lifetime of this guard, so the test can scope the
/// "another run is holding the lock" simulation to a single block: the
/// guard drops at the end of the block, the OS releases the lock, and a
/// follow-up `acquire` would see it as free. The integration test does
/// not exercise that release (the `src/session_lock.rs` unit tests pin
/// it instead); the guard still drops here so the test cleans up after
/// itself even when an assertion fails.
struct SessionLockGuard {
    file: File,
    #[allow(dead_code, reason = "kept for diagnosis on assertion failure")]
    path: PathBuf,
}

impl SessionLockGuard {
    /// Acquire the lock at `<home>/sessions/<session_id>.lock`,
    /// mirroring `SessionLock::lock_path`. The implementation must
    /// `create(true)` so a fresh `aether home` that has never run aether
    /// still gets a `sessions/` directory and a lock file at the path
    /// the production guard expects.
    fn acquire(home: &Path, session_id: &str) -> std::io::Result<Self> {
        let dir = home.join("sessions");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{session_id}.lock"));
        let file = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&path)?;
        file.try_lock().map_err(|error| std::io::Error::other(format!("try_lock failed: {error}")))?;
        Ok(Self { file, path })
    }
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        // Best-effort `unlock`; the close on drop is the actual release
        // pathway on every platform `File::try_lock` runs on today.
        let _ = self.file.unlock();
    }
}

/// `aether headless --session-id <id>` started while the same id is held
/// by a live run must exit non-zero and print a message that names the
/// session id, matching the task's done-when verbatim:
///
/// > starting a run whose session id is already held by a live run fails
/// > with an error naming the session id
///
/// The test process takes the lock before spawning the binary, so the
/// refusal originates from the real `SessionLock::acquire` code path and
/// exercises the on-disk contract (`<AETHER_HOME>/sessions/<id>.lock`)
/// without requiring a long-running sibling process.
#[cfg(unix)]
#[test]
#[allow(clippy::significant_drop_tightening, reason = "the guard must outlive the child process")]
fn session_id_held_by_a_live_run_is_refused_with_a_name() -> TestResult {
    let home = tempfile::tempdir()?;
    let cwd = tempfile::tempdir()?;
    let settings = cwd.path().join("settings.json");
    std::fs::create_dir_all(cwd.path().join(".aether"))?;
    fs::write(&settings, SETTINGS_JSON)?;

    // Hold the lock for `test-session` from the test process so the
    // child sees it as already held when it tries to acquire.
    let _held = SessionLockGuard::acquire(home.path(), "test-session")?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--cwd")
        .arg(cwd.path())
        .arg("--config")
        .arg(&settings)
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--session-id")
        .arg("test-session")
        .env("AETHER_HOME", home.path())
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "aether headless run with a held --session-id must exit non-zero, got {:?}\n\
stdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(stderr.contains("test-session"), "stderr must name the held session id 'test-session'; got:\n{stderr}");
    assert!(
        stderr.contains("another aether run"),
        "stderr must announce the contention as 'another aether run'; got:\n{stderr}"
    );

    Ok(())
}

/// A separate session id must not contend. Two child processes run with
/// different `--session-id`s; the held one (`session-a`) must surface
/// the session-lock refusal, and the free one (`session-b`) must not.
/// They share `cwd` on purpose so the cwd-lock message would show up for
/// both runs first if the test were ever masking the session-axis
/// somehow — that asymmetry is what makes this test a useful negative.
#[cfg(unix)]
#[test]
#[allow(clippy::significant_drop_tightening, reason = "the guard must outlive the child process")]
fn different_session_ids_do_not_contend() -> TestResult {
    let home = tempfile::tempdir()?;
    let cwd = tempfile::tempdir()?;
    let settings = cwd.path().join("settings.json");
    std::fs::create_dir_all(cwd.path().join(".aether"))?;
    fs::write(&settings, SETTINGS_JSON)?;

    // Hold the first session id so it is taken, but leave the second
    // free. Both child processes use a `--provider` override pointing
    // at an unbound local port so the provider call fails fast without
    // waiting for a network timeout.
    let _held = SessionLockGuard::acquire(home.path(), "session-a")?;

    let stderr_a = String::from_utf8_lossy(
        &Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("headless")
            .arg("--cwd")
            .arg(cwd.path())
            .arg("--config")
            .arg(&settings)
            .arg("--model")
            .arg("ollama:llama3.2")
            .arg("--provider")
            .arg("ollama.url=http://127.0.0.1:1")
            .arg("--session-id")
            .arg("session-a")
            .arg("--timeout")
            .arg("5s")
            .env("AETHER_HOME", home.path())
            .arg("hello")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?
            .stderr,
    )
    .into_owned();

    let stderr_b = String::from_utf8_lossy(
        &Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("headless")
            .arg("--cwd")
            .arg(cwd.path())
            .arg("--config")
            .arg(&settings)
            .arg("--model")
            .arg("ollama:llama3.2")
            .arg("--provider")
            .arg("ollama.url=http://127.0.0.1:1")
            .arg("--session-id")
            .arg("session-b")
            .arg("--timeout")
            .arg("5s")
            .env("AETHER_HOME", home.path())
            .arg("hello")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?
            .stderr,
    )
    .into_owned();

    assert!(
        stderr_a.contains("another aether run") && stderr_a.contains("session-a"),
        "the held session id 'session-a' must surface the session-lock refusal; got:\n{stderr_a}",
    );
    assert!(
        !stderr_b.contains("another aether run"),
        "a session id with no other holder must not surface the session-lock refusal; got:\n{stderr_b}",
    );
    assert!(
        !stderr_b.contains("session-a"),
        "a different session id must not be reported as held by 'session-a'; got:\n{stderr_b}",
    );

    Ok(())
}

/// `--session-id` with no other run holding the id must proceed past the
/// guard and surface a refused provider call rather than the session
/// lock. The `--provider` override points at an unbound local port so
/// the run fails fast. The test asserts the on-disk contract alongside
/// the negative refusal: even when the run later fails for an unrelated
/// reason, the lock file is created on disk (the guard creates before
/// the run's work) and the run's stderr does not announce the
/// session-lock refusal.
#[cfg(unix)]
#[test]
fn unused_session_id_does_not_fire_the_lock() -> TestResult {
    let home = tempfile::tempdir()?;
    let cwd = tempfile::tempdir()?;
    let settings = cwd.path().join("settings.json");
    std::fs::create_dir_all(cwd.path().join(".aether"))?;
    fs::write(&settings, SETTINGS_JSON)?;

    // `--provider` override at an unbound local port keeps the run
    // short: connection refused without any network backoff. The
    // session-id lock must not fire on this run because no other run
    // holds `free-session`.
    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--cwd")
        .arg(cwd.path())
        .arg("--config")
        .arg(&settings)
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--provider")
        .arg("ollama.url=http://127.0.0.1:1")
        .arg("--session-id")
        .arg("free-session")
        .arg("--timeout")
        .arg("5s")
        .env("AETHER_HOME", home.path())
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The session-id guard must not have fired: the only refusal we
    // expect is from the provider HTTP call (connection refused).
    assert!(
        !stderr.contains("another aether run"),
        "an unused --session-id must not fire the session-lock refusal; got:\n{stderr}",
    );
    assert!(
        !stderr.contains("--session-id must not"),
        "an in-spec --session-id must not be rejected at parse time; got:\n{stderr}",
    );

    // The lock file the guard creates must exist on disk. By the time
    // we look the run has already released the lock (so the file is
    // unheld but still on disk), which matches `SessionLock`'s
    // "presence on disk ≠ lock active" contract.
    let lock_path = home.path().join("sessions").join("free-session.lock");
    assert!(
        lock_path.exists(),
        "the session-lock file `{lock_path:?}` must exist after a run that acquired and released the guard; \
stderr was:\n{stderr}",
    );

    Ok(())
}

/// Reject a `--session-id` that contains a path separator at parse time
/// so a CLI caller cannot reach the lock directory under a different
/// name. The test asserts the refusal surfaces through the binary's
/// stderr and exits non-zero before any provider call.
#[cfg(unix)]
#[test]
fn path_traversal_session_id_is_refused() -> TestResult {
    let home = tempfile::tempdir()?;
    let cwd = tempfile::tempdir()?;
    let settings = cwd.path().join("settings.json");
    std::fs::create_dir_all(cwd.path().join(".aether"))?;
    fs::write(&settings, SETTINGS_JSON)?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--cwd")
        .arg(cwd.path())
        .arg("--config")
        .arg(&settings)
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--session-id")
        .arg("../escape")
        .env("AETHER_HOME", home.path())
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "a malicious --session-id must exit non-zero; got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(stderr.contains("--session-id"), "rejection must name --session-id; got:\n{stderr}");

    Ok(())
}
