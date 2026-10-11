//! Integration tests for the `--log-max-bytes` rotation flag (TASK-26-92).
//!
//! `--log-max-bytes N` pairs with `--log-file` to keep a long-running aether
//! process from filling the disk with one unbounded log: once a write crosses
//! `N` bytes, the live file is renamed to `<stem>.1` (overwriting any
//! previous rotated sibling — exactly one previous file is kept, mirroring
//! the `--transcript-max-bytes` contract), a fresh empty file is opened at
//! the original `--log-file` path, and one line naming the new file is
//! printed to stderr so an operator tailing the terminal can follow the log
//! across rotations.
//!
//! The tests reuse the `headless --dry-run --log-level warn` harness the
//! `--log-file` tests already exercise, but with a deliberately tiny
//! `--log-max-bytes` so even a single `tracing::warn!` from the settings
//! loader crosses the threshold and a rotation fires. Each test asserts:
//!
//!   1. `--log-file` is written to (proves the redirect reaches the
//!      rotating writer, not stderr),
//!   2. the rotated sibling exists (proves rotation actually fired),
//!   3. the stderr stream contains a line naming the new file (proves the
//!      notice was emitted and names the freshly opened file, not the
//!      rotated-away sibling).
//!
//! The `no_max_keeps_single_file_behaviour` test is the negative case: with
//! `--log-max-bytes` omitted the run shape is preserved (single file, no
//! rotated sibling) so callers that never pass the flag see no change.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Settings document with two deliberately-unknown keys. Every unknown key
/// produces one `tracing::warn!` during settings load, so two keys give us
/// at least two records flowing through the redirected log layer — enough
/// headroom to drive a rotation even with conservative log levels.
const UNKNOWN_KEY_ONE: &str = "zzLogRotationTestUnknownOne";
const UNKNOWN_KEY_TWO: &str = "zzLogRotationTestUnknownTwo";

fn write_settings_with_unknown_keys(dir: &std::path::Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = dir.join("settings.json");
    let body = format!(
        r#"{{
            "credentialsStore": {{ "type": "memory" }},
            "agents": [],
            "{UNKNOWN_KEY_ONE}": true,
            "{UNKNOWN_KEY_TWO}": true
        }}"#
    );
    fs::write(&path, body)?;
    Ok(path)
}

/// Helper to spawn the headless `--dry-run` binary with `--log-file` and
/// optional `--log-max-bytes` arguments. Returns the captured `output` so
/// individual tests can pick which assertions matter for the rotation
/// contract.
fn run_dry_run(
    log_path: &std::path::Path,
    log_max_bytes: Option<&str>,
    config_path: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<std::process::Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command.arg("headless").arg("--dry-run").arg("--log-level").arg("warn").arg("--log-file").arg(log_path);
    if let Some(max) = log_max_bytes {
        command.arg("--log-max-bytes").arg(max);
    }
    command
        .arg("--config")
        .arg(config_path)
        .arg("--cwd")
        .arg(cwd)
        .arg("--model")
        .arg("ollama:llama3.2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command.output()?)
}

/// `--log-file PATH --log-max-bytes 1` (any record crosses the threshold)
/// must rotate the live file to `<stem>.1` and print a line to stderr
/// naming the freshly-opened file. Proves the rotation pipeline end-to-end.
#[test]
fn rotation_moves_live_log_to_sibling_and_prints_notice_naming_new_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_keys(dir.path())?;
    let log_path = dir.path().join("run.log");
    let rotated_path = dir.path().join("run.1");
    assert!(!log_path.exists(), "log path must not exist before the run for the test to be meaningful");
    assert!(!rotated_path.exists(), "rotated sibling must not exist before the run");

    let output = run_dry_run(&log_path, Some("1"), &config_path, dir.path())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "--log-max-bytes must not change the run's exit status, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    // The live file was redirected to the operator's path. With
    // `log-max-bytes = 1`, every record triggers a rotation immediately and
    // the freshly-opened live file ends up empty between records — the
    // contract is that the operator's path stays open for new writes, not
    // that it always holds the most recent record. The rotated sibling is
    // where the actual content lives.
    assert!(log_path.exists(), "--log-file must still create the live log path: {log_path:?}");

    // The rotated sibling exists — proves rotation actually fired — and
    // holds at least one of the warnings the settings loader emitted for
    // the unknown keys.
    assert!(rotated_path.exists(), "rotation must rename the live log to <stem>.1; expected {rotated_path:?}");
    let rotated_contents = fs::read_to_string(&rotated_path)?;
    assert!(
        rotated_contents.contains(UNKNOWN_KEY_ONE) || rotated_contents.contains(UNKNOWN_KEY_TWO),
        "the rotated sibling must hold at least one of the settings-load warnings; got:\n{rotated_contents}"
    );

    // The stderr notice names the *new* file (the freshly-opened one at
    // the operator's `--log-file` path), not the rotated-away sibling.
    let rendered_new_path = log_path.to_string_lossy().into_owned();
    let rendered_rotated_path = rotated_path.to_string_lossy().into_owned();
    let expected_prefix = "aether: log rotated; writing to ".to_string();
    let rotation_line_count = stderr.lines().filter(|line| line.starts_with(&expected_prefix)).count();
    assert!(
        rotation_line_count >= 1,
        "stderr must contain at least one rotation notice naming the new file; got:\n{stderr}"
    );
    assert!(
        stderr.contains(&rendered_new_path),
        "the rotation notice must name the freshly-opened log path {rendered_new_path:?}; got:\n{stderr}"
    );
    assert!(
        !stderr.contains(&format!("writing to {rendered_rotated_path}")),
        "the rotation notice must NOT name the rotated-away sibling {rendered_rotated_path:?}; got:\n{stderr}"
    );

    Ok(())
}

/// `--log-max-bytes` omitted (the default) must leave the run shape
/// unchanged: a single `--log-file` receives every log line, no rotated
/// sibling exists, no rotation notice is emitted. Guards against a future
/// regression that quietly enables rotation when none was requested.
#[test]
fn no_max_keeps_single_file_behaviour() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_keys(dir.path())?;
    let log_path = dir.path().join("run.log");
    let rotated_path = dir.path().join("run.1");
    assert!(!log_path.exists(), "log path must not exist before the run");

    let output = run_dry_run(&log_path, None, &config_path, dir.path())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the run must still succeed without --log-max-bytes; got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(log_path.exists(), "--log-file must still create the live log path");
    let live_contents = fs::read_to_string(&log_path)?;
    assert!(
        live_contents.contains(UNKNOWN_KEY_ONE) && live_contents.contains(UNKNOWN_KEY_TWO),
        "the live log must capture every warning when rotation is disabled; got:\n{live_contents}"
    );

    assert!(
        !rotated_path.exists(),
        "no rotated sibling must exist when --log-max-bytes is omitted; got unexpected {rotated_path:?}"
    );

    assert!(
        !stderr.contains("aether: log rotated"),
        "no rotation notice must fire when --log-max-bytes is omitted; got:\n{stderr}"
    );

    Ok(())
}

/// `--log-max-bytes 0` (the explicit "off" sentinel) must be treated the
/// same as `None`. Matches the `max_bytes = 0` convention the
/// `--transcript-max-bytes` flag already uses so operators who pass a
/// literal zero don't accidentally enable rotation via the explicit form.
#[test]
fn max_bytes_zero_disables_rotation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_keys(dir.path())?;
    let log_path = dir.path().join("run.log");
    let rotated_path = dir.path().join("run.1");

    let output = run_dry_run(&log_path, Some("0"), &config_path, dir.path())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "--log-max-bytes 0 must not change the run's exit status, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(!rotated_path.exists(), "--log-max-bytes 0 must not rotate; got unexpected sibling {rotated_path:?}");
    assert!(
        !stderr.contains("aether: log rotated"),
        "--log-max-bytes 0 must not emit a rotation notice; got:\n{stderr}"
    );

    let live_contents = fs::read_to_string(&log_path)?;
    assert!(
        live_contents.contains(UNKNOWN_KEY_ONE) && live_contents.contains(UNKNOWN_KEY_TWO),
        "the live log must capture every warning when rotation is disabled by 0; got:\n{live_contents}"
    );

    Ok(())
}
