//! Integration tests for the per-working-directory run lock (TASK-26-21).
//!
//! `aether headless` holds an OS-level advisory lock on
//! `<working directory>/.aether/run.lock` while a run is in progress, so two
//! runs in one directory cannot clobber the artefacts they write (transcript,
//! file changes, `.aether/`). Two properties of that lock are pinned here
//! through the public CLI:
//!
//!   * the lock is the lock on an open handle, not the file's presence. Every
//!     run leaves a zero-byte `run.lock` behind, so a stale file in a working
//!     directory must never turn the next run away, and
//!   * a run takes its lock in the directory it was told to work in. A run
//!     started through `--options-json` uses the `-C/--cwd` flag when the
//!     payload carries no `cwd` of its own, so two such runs do not fight
//!     over one directory.
//!
//! Both tests stop at the telemetry check: an inline `--options-json` payload
//! with an invalid `traceparent` makes the run fail right after it took the
//! lock, with no provider call and no network access.

use std::error::Error;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

/// A `traceparent` the telemetry layer rejects. The run fails with it right
/// after it takes the working-directory lock, which is exactly the point the
/// tests below need to observe.
const INVALID_TRACEPARENT: &str = "00-00000000000000000000000000000000-0123456789abcdef-01";

/// Build the `--options-json` payload for a run that stops at the telemetry
/// check. `cwd` is left out so the payload does not name a working directory
/// of its own.
fn options_payload() -> String {
    serde_json::json!({
        "prompt": "hello",
        "settings": {
            "agents": [],
            "telemetry": {"otlp": {"endpoint": "http://127.0.0.1:1"}},
        },
        "traceContext": {"traceparent": INVALID_TRACEPARENT},
    })
    .to_string()
}

/// A zero-byte `.aether/run.lock` left behind by an earlier run must not
/// refuse the next run. The file is what every run leaves on disk; only an
/// open handle holds the lock, so a stale artefact in the working directory
/// is harmless. The run must get past the lock and fail later, at the
/// telemetry check, instead of being turned away with "another aether run is
/// already using this working directory".
#[test]
fn a_stale_empty_run_lock_does_not_refuse_the_run() -> TestResult {
    let dir = tempfile::tempdir()?;
    let aether_dir = dir.path().join(".aether");
    std::fs::create_dir_all(&aether_dir)?;
    let lock = aether_dir.join("run.lock");
    std::fs::write(&lock, b"")?;
    assert_eq!(std::fs::metadata(&lock)?.len(), 0, "the stale lock file must be empty");

    let options = serde_json::json!({
        "prompt": "hello",
        "settings": {
            "agents": [],
            "telemetry": {"otlp": {"endpoint": "http://127.0.0.1:1"}},
        },
        "traceContext": {"traceparent": INVALID_TRACEPARENT},
        "cwd": dir.path().to_string_lossy(),
    })
    .to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--options-json")
        .arg(&options)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("another aether run"),
        "a stale, empty lock file must not refuse the run; stderr: {stderr}"
    );
    assert!(
        stderr.contains("invalid traceparent"),
        "the run must start and reach the telemetry check; stderr: {stderr}"
    );
    Ok(())
}

/// A run started through `--options-json` must work in the directory the
/// `-C/--cwd` flag names when the payload carries no `cwd` of its own. The
/// run's own lock file is the evidence: it is created inside the directory
/// the run started in, so two runs started with different `--cwd` flags take
/// two different locks instead of refusing each other.
#[test]
fn headless_options_json_run_starts_in_the_cwd_flag_directory() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--options-json")
        .arg(options_payload())
        .arg("--cwd")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid traceparent"),
        "the run must start and reach the telemetry check; stderr: {stderr}"
    );
    assert!(
        dir.path().join(".aether").join("run.lock").exists(),
        "the run must take its lock in the directory `--cwd` named; stderr: {stderr}"
    );
    Ok(())
}
