//! Integration tests for the `aether --log-level <LEVEL>` flag (TASK-25-41).
//!
//! The flag accepts one of `error`, `warn`, `info`, or `debug` and is wired
//! into the headless subcommand's tracing filter. It is also accepted
//! before the subcommand (`aether --log-level debug headless …`); both
//! placements must end up in the same `HeadlessArgs` shape. An unknown value
//! is rejected at parse time with a diagnostic that names the offending
//! input and lists the allowed set.
//!
//! The settings-load path emits a `tracing::warn!` for unrecognised
//! top-level keys, so `--log-level error` is the cheapest way to observe
//! the filter at work: the warning is silenced under `error` and reappears
//! under `warn`/`info`/`debug`. The `headless --dry-run` short-circuit
//! (`crates/aether-cli/src/headless/mod.rs`) runs the same load + filter
//! path but skips session/MCP/provider work, which keeps the tests
//! deterministic and network-free.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

const UNKNOWN_KEY: &str = "zzLogLevelTestUnknown";

/// Write a settings document that includes a deliberately unknown
/// top-level key. The `tracing::warn!` emitted when that key is recognised
/// (or, here, not) is the canary used to prove `--log-level` is wired into
/// the actual run-time filter, not just a parser that the binary ignores.
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

/// `aether --log-level <LEVEL> --list-profiles --config PATH` must exit 0
/// for every value the help text advertises. `--list-profiles` reads the
/// same settings the run would and exits without starting a session, so
/// the test exercises the flag's parse and merge into the `Cli` struct
/// without needing a network round-trip.
#[test]
fn accepts_each_log_level_before_the_subcommand() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;

    for level in ["error", "warn", "info", "debug", "trace"] {
        let output = Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("--log-level")
            .arg(level)
            .arg("--list-profiles")
            .arg("--config")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "--log-level {level} must exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code(),
        );
    }

    Ok(())
}

/// The same flag placed after the subcommand (`aether headless --log-level …`)
/// must behave identically. This guards the merge path in `main.rs` that
/// copies the top-level value into `HeadlessArgs` when the subcommand did
/// not see it itself. `headless --dry-run` short-circuits before any
/// session/MCP/provider work, so the test stays network-free.
#[test]
fn accepts_each_log_level_after_the_subcommand() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;

    for level in ["error", "warn", "info", "debug", "trace"] {
        let output = Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("headless")
            .arg("--dry-run")
            .arg("--log-level")
            .arg(level)
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
            "headless --log-level {level} must exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code(),
        );
    }

    Ok(())
}

/// The flag's effect on the run's logging is observable: settings load
/// emits `tracing::warn!` for unknown top-level keys, so
/// `--log-level error` must silence the message and `--log-level warn` (or
/// higher) must re-emit it. We use two distinct values to confirm the
/// level actually drives the filter and is not simply passing through
/// unchanged.
#[test]
fn log_level_filters_settings_load_warnings() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;

    let run = |level: &str| -> Result<(String, String), Box<dyn Error>> {
        let output = Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("headless")
            .arg("--dry-run")
            .arg("--log-level")
            .arg(level)
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
        assert!(output.status.success(), "--log-level {level} must exit 0, got {:?}", output.status.code(),);
        Ok((String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned()))
    };

    let (stdout_error, stderr_error) = run("error")?;
    assert!(
        !stderr_error.contains(UNKNOWN_KEY),
        "--log-level error must silence the settings-load warning about {UNKNOWN_KEY:?}, got:\n{stderr_error}"
    );
    assert!(
        stdout_error.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under --log-level error; got:\n{stdout_error}"
    );

    let (stdout_warn, stderr_warn) = run("warn")?;
    assert!(
        stderr_warn.contains(UNKNOWN_KEY),
        "--log-level warn must re-emit the settings-load warning naming {UNKNOWN_KEY:?}, got:\n{stderr_warn}"
    );
    assert!(
        stdout_warn.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under --log-level warn; got:\n{stdout_warn}"
    );

    Ok(())
}

/// An unknown value is rejected at parse time. clap uses exit code 2 for
/// argument errors and the diagnostic must name the rejected input and
/// list the allowed set so the caller can correct it.
#[test]
fn rejects_unknown_log_level_value() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("--log-level")
        .arg("bogus")
        .arg("--list-profiles")
        .arg("--config")
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "--log-level bogus must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap uses exit code 2 for argument errors; got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(stderr.contains("bogus"), "stderr must name the rejected value `bogus`; got:\n{stderr}");
    for allowed in ["error", "warn", "info", "debug", "trace"] {
        assert!(stderr.contains(allowed), "stderr must list `{allowed}` as an accepted value; got:\n{stderr}");
    }

    Ok(())
}
