//! Integration tests for the `aether --log-file PATH` flag (TASK-25-48).
//!
//! The flag redirects the headless run's tracing log away from stderr and
//! onto a file the operator names. The settings-load path emits a
//! `tracing::warn!` for unrecognised top-level keys, so under
//! `--log-level warn` (and above) that warning is the cheapest canary that
//! proves lines flow through the redirected layer instead of through the
//! terminal. The `headless --dry-run` short-circuit runs the same load +
//! filter path but skips session/MCP/provider work, so the tests stay
//! deterministic and network-free.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

const UNKNOWN_KEY: &str = "zzLogFileTestUnknown";

/// Write a settings document that includes a deliberately unknown
/// top-level key. The `tracing::warn!` emitted when that key is recognised
/// (or, here, not) is the canary used to prove `--log-file` actually
/// redirects the run's log stream, not just parses the flag.
fn write_settings_with_unknown_key(dir: &std::path::Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = dir.join("settings.json");
    let body = format!(
        r#"{{
            "credentialsStore": {{ "type": "memory" }},
            "agents": [],
            "{UNKNOWN_KEY}": true
        }}"#
    );
    fs::write(&path, body)?;
    Ok(path)
}

/// `aether headless --dry-run --log-level warn --log-file PATH …` must write
/// the settings-load warning about the unknown key to PATH instead of
/// stderr. This proves two things in one run:
///   1. the redirect reaches `setup_tracing` (the line lands in PATH), and
///   2. the redirect actually replaces stderr (the same line is *absent*
///      from stderr — just parsing the flag would still print to stderr).
#[test]
fn headless_run_log_is_written_to_the_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let log_path = dir.path().join("run.log");
    assert!(!log_path.exists(), "log path must not exist before the run for the test to be meaningful");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--log-file")
        .arg(&log_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "--log-file must not change the run's exit status, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stdout.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under --log-file; got:\n{stdout}"
    );

    let log_contents = fs::read_to_string(&log_path)?;
    assert!(
        log_contents.contains(UNKNOWN_KEY),
        "--log-file must redirect the settings-load warning about {UNKNOWN_KEY:?} into the file; got:\n{log_contents}"
    );
    assert!(!stderr.contains(UNKNOWN_KEY), "--log-file must redirect the warning *away from* stderr; got:\n{stderr}");

    Ok(())
}

/// Pointing `--log-file` at a path that does not yet exist must create the
/// file (the task says "creating PATH if missing") and still write the run
/// log to it. Parent directories are *not* created — opening the path fails
/// the run — so the test lives inside a `tempdir` whose parent is
/// guaranteed to exist.
#[test]
fn missing_log_file_path_is_created() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let log_path = dir.path().join("fresh.log");
    assert!(!log_path.exists(), "log path must not exist before the run");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--log-file")
        .arg(&log_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "creating a missing log path must succeed; got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(log_path.exists(), "--log-file must create PATH if missing; path={log_path:?}");
    let contents = fs::read_to_string(&log_path)?;
    assert!(
        contents.contains(UNKNOWN_KEY),
        "the freshly-created log must still capture the run's warning; got:\n{contents}"
    );

    Ok(())
}

/// Pointing `--log-file` at a path whose parent directory does not exist
/// must fail the run before the agent starts, and the diagnostic must name
/// the offending path so the operator can correct it. This documents the
/// "no parent-dir creation" boundary so a future change cannot silently
/// start creating parent directories.
#[test]
fn invalid_log_file_path_fails_the_run() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    // `nested/dir/that/does/not/exist/run.log` — the parent chain is
    // intentionally absent so the open fails.
    let log_path =
        dir.path().join("nested").join("dir").join("that").join("does").join("not").join("exist").join("run.log");
    assert!(!log_path.parent().expect("path has a parent").exists(), "test prerequisite: parent must not exist");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--log-file")
        .arg(&log_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let rendered_path = log_path.to_string_lossy().into_owned();

    assert!(
        !output.status.success(),
        "opening a log path whose parent is missing must fail the run; got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stderr.contains(&rendered_path),
        "the diagnostic must name the offending log path {rendered_path:?}; got:\n{stderr}"
    );

    Ok(())
}
