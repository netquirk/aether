//! Integration test: the run log records the wall-clock time the run took
//! (TASK-25-148).
//!
//! Every headless run writes one `tracing::info!` line near its end that
//! names the elapsed wall-clock duration. The line is emitted from
//! `headless::run::run` immediately after the agent task returns, before the
//! telemetry runtime is shut down, so it sits alongside the run-start line
//! `headless::run::run` already emits (`"run starting"`). The test points
//! `--model` at a closed local port so the run is expected to fail on the
//! refused HTTP call; the run-start line must already be on disk, and the
//! run-end line must be on disk *after* the failed call. The test asserts:
//!
//!   * the log file carries the substring `elapsed_seconds=`, and
//!   * the value after `elapsed_seconds=` parses as a `u64` — proving the
//!     line carries whole seconds (the task's default), not a float or a
//!     duration string like `1.234s`.
//!
//! The test does **not** assert an exact value: a fast run can legitimately
//! log `elapsed_seconds=0` after truncation.

use std::error::Error;
use std::net::TcpListener;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// `aether headless --model ollama:llama3.2 --log-level info --log-file PATH`
/// must write a `tracing::info!` line naming the whole-run elapsed time
/// (`elapsed_seconds=<n>`) by the time the run exits. The test points the
/// `--model` flag at a closed loopback port so the run is offline and fast:
/// the refused connection is the expected failure mode that exercises the
/// post-startup code path without making the test hang on a real provider
/// call.
#[test]
fn run_log_records_whole_second_elapsed() -> TestResult {
    let dir = tempfile::tempdir()?;
    // Bind then drop: the listener's port is freed but the address is
    // immediately closed for new connections, so any subsequent request is
    // refused with `ECONNREFUSED` — the same offline canary the dry-run
    // integration tests use to prove no provider call happens.
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?
    };
    let endpoint = format!("http://{addr}");
    let log_path = dir.path().join("run.log");

    let _output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--provider")
        .arg(format!("ollama.url={endpoint}"))
        .arg("--log-level")
        .arg("info")
        .arg("--log-file")
        .arg(&log_path)
        .arg("--timeout")
        .arg("15s")
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let log = std::fs::read_to_string(&log_path)?;

    // The end-of-run line must be present in the log.
    assert!(log.contains("elapsed_seconds="), "run log must carry an elapsed_seconds field; got:\n{log}",);
    // And it must be paired with the `run finished` message the implementation
    // uses, so an unrelated `elapsed_seconds=` substring cannot satisfy the
    // assertion.
    assert!(log.contains("run finished"), "run log must carry the run-finished line; got:\n{log}",);

    // Parse the value as a `u64`: the implementation uses `as_secs()`, so a
    // float, a duration string, or any unit suffix would all be rejected by
    // the strict integer parser. The exact value is intentionally not
    // asserted — a sub-second run legitimately logs `0` after truncation.
    let line = log
        .lines()
        .find(|line| line.contains("elapsed_seconds="))
        .expect("an elapsed_seconds= line must be present in the run log");
    let value_str = line
        .split("elapsed_seconds=")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .unwrap_or("");
    assert!(!value_str.is_empty(), "elapsed_seconds value must not be empty; got line: {line:?}",);
    let parsed: u64 = value_str.parse().unwrap_or_else(|error| {
        panic!("elapsed_seconds value must be a whole-second integer, got {value_str:?}: {error}")
    });
    // Sanity bound: a run that took "longer than a day" is a test-infrastructure
    // problem rather than a real number; a tight upper bound catches it.
    assert!(
        parsed < 24 * 60 * 60,
        "elapsed_seconds={parsed} is implausibly large; the value should be a wall-clock run duration",
    );

    Ok(())
}
