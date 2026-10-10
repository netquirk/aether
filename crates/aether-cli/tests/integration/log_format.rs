//! Integration tests for the `aether headless --log-format <FORMAT>` flag
//! (TASK-25-99).
//!
//! The flag accepts one of `text` (the default, unchanged) or `json` (one
//! JSON object per line, each carrying at least `level`, `message`, and
//! `timestamp`). It is wired into the same `setup_tracing` call the existing
//! `--log-level` and `--log-file` flags feed, so the settings-load path's
//! `tracing::warn!` for an unrecognised top-level key is the cheapest canary
//! that proves the format actually changed the run's log stream. The
//! `headless --dry-run` short-circuit runs the same load + filter path but
//! skips session/MCP/provider work, so the tests stay deterministic and
//! network-free.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

const UNKNOWN_KEY: &str = "zzLogFormatTestUnknown";

/// Write a settings document that includes a deliberately unknown
/// top-level key. The `tracing::warn!` emitted when that key is recognised
/// (or, here, not) is the canary used to prove `--log-format` actually
/// changes the run's log stream, not just parses the flag.
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

/// Run `aether headless --dry-run` with the given extra flags and return
/// the captured stdout, stderr, and exit status. Centralised so each test
/// below only has to vary the `--log-format` value.
fn run_headless_dry_run(
    dir: &std::path::Path,
    config_path: &std::path::Path,
    log_format: Option<&str>,
) -> Result<(String, String, std::process::ExitStatus), Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--config")
        .arg(config_path)
        .arg("--cwd")
        .arg(dir)
        .arg("--model")
        .arg("ollama:llama3.2");
    if let Some(format) = log_format {
        command.arg("--log-format").arg(format);
    }
    let output = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((stdout, stderr, output.status))
}

/// `aether headless --dry-run --log-format json …` must emit the
/// settings-load warning as a single JSON object on stderr, with
/// top-level `level`, `message`, and `timestamp` fields. This proves
/// three things in one run:
///   1. the flag is wired into `setup_tracing` (the line is JSON, not text),
///   2. the JSON object has the three keys the task's "Done when" requires,
///   3. the run shape is preserved (stdout still reports the model and the
///      exit status is 0).
#[test]
fn json_format_emits_one_json_object_per_line() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let (stdout, stderr, status) = run_headless_dry_run(dir.path(), &config_path, Some("json"))?;

    assert!(
        status.success(),
        "--log-format json must not change the run's exit status, got {status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under --log-format json; got:\n{stdout}"
    );

    // Split the canary line out of stderr. The settings-load warning is the
    // one observable `tracing::warn!` a `--dry-run` produces, so any
    // non-empty stderr line is the canary (or a duplicate of it). The JSON
    // formatter is required to put one object on one line, so each line
    // must be a self-contained JSON value.
    let canary_lines: Vec<&str> = stderr.lines().filter(|line| line.contains(UNKNOWN_KEY)).collect();
    assert!(
        !canary_lines.is_empty(),
        "stderr must carry the settings-load warning about {UNKNOWN_KEY:?}; got:\n{stderr}"
    );

    for line in &canary_lines {
        let value: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|error| {
            panic!(
                "every stderr line carrying the canary must be valid JSON: {error} in {line:?}\nfull stderr:\n{stderr}"
            )
        });
        let object =
            value.as_object().unwrap_or_else(|| panic!("each canary line must be a JSON object; got: {line:?}"));

        assert!(object.contains_key("level"), "object must carry a top-level `level` key: {line:?}");
        assert!(object.contains_key("message"), "object must carry a top-level `message` key: {line:?}");
        assert!(object.contains_key("timestamp"), "object must carry a top-level `timestamp` key: {line:?}");

        let level = object["level"].as_str().unwrap_or_else(|| panic!("`level` must be a string; got {line:?}"));
        assert_eq!(
            level.eq_ignore_ascii_case("WARN"),
            true,
            "settings-load warning must be a WARN-level event; got {level:?} in {line:?}"
        );

        let message = object["message"].as_str().unwrap_or_else(|| panic!("`message` must be a string; got {line:?}"));
        assert!(
            message.contains(UNKNOWN_KEY),
            "message must name the unknown key {UNKNOWN_KEY:?}; got {message:?} in {line:?}"
        );

        let timestamp =
            object["timestamp"].as_str().unwrap_or_else(|| panic!("`timestamp` must be a string; got {line:?}"));
        assert!(!timestamp.is_empty(), "`timestamp` must be non-empty; got {line:?}");
    }

    Ok(())
}

/// Absent `--log-format` keeps the pre-existing `fmt` layer output. The
/// settings-load warning must reach stderr in plain text — that is, the
/// canary line must NOT be a parseable JSON object and must still mention
/// `WARN` and the unknown key. This is the regression anchor for the
/// "default output does not change" requirement.
#[test]
fn default_format_stays_text() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let (stdout, stderr, status) = run_headless_dry_run(dir.path(), &config_path, None)?;

    assert!(
        status.success(),
        "absent --log-format must not change the exit status, got {status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under the default format; got:\n{stdout}"
    );

    let canary = stderr
        .lines()
        .find(|line| line.contains(UNKNOWN_KEY))
        .unwrap_or_else(|| panic!("stderr must carry the settings-load warning about {UNKNOWN_KEY:?}; got:\n{stderr}"));

    let parsed: Result<serde_json::Value, _> = serde_json::from_str(canary);
    assert!(parsed.is_err(), "the default format must NOT emit JSON; got a parseable canary line: {canary:?}");
    assert!(canary.contains("WARN"), "default text format must still surface the WARN level; got: {canary:?}");
    assert!(canary.contains(UNKNOWN_KEY), "default text format must still name the unknown key; got: {canary:?}");

    Ok(())
}

/// `--log-format text` is explicitly accepted and behaves the same as the
/// absent-flag path: the canary is plain text, not JSON. This documents
/// the explicit-override escape hatch for any operator that wants to
/// force the text format regardless of defaults.
#[test]
fn explicit_text_format_matches_default() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let (stdout, stderr, status) = run_headless_dry_run(dir.path(), &config_path, Some("text"))?;

    assert!(
        status.success(),
        "--log-format text must not change the exit status, got {status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("model: ollama:llama3.2"),
        "stdout must still report the resolved model under --log-format text; got:\n{stdout}"
    );

    let canary = stderr
        .lines()
        .find(|line| line.contains(UNKNOWN_KEY))
        .unwrap_or_else(|| panic!("stderr must carry the settings-load warning about {UNKNOWN_KEY:?}; got:\n{stderr}"));
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(canary);
    assert!(parsed.is_err(), "--log-format text must NOT emit JSON; got a parseable canary line: {canary:?}");
    assert!(canary.contains("WARN"), "--log-format text must still surface the WARN level; got: {canary:?}");

    Ok(())
}

/// An unknown value is rejected at parse time. clap uses exit code 2 for
/// argument errors and the diagnostic must name the rejected input and
/// list the allowed set so the caller can correct it.
#[test]
fn rejects_unknown_log_format_value() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--log-format")
        .arg("xml")
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
        !output.status.success(),
        "--log-format xml must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code()
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap uses exit code 2 for argument errors; got {:?}\nstderr:\n{stderr}",
        output.status.code()
    );
    assert!(stderr.contains("xml"), "stderr must name the rejected value `xml`; got:\n{stderr}");
    for allowed in ["text", "json"] {
        assert!(stderr.contains(allowed), "stderr must list `{allowed}` as an accepted value; got:\n{stderr}");
    }

    Ok(())
}

/// `--log-format json` composes with `--log-file PATH` (TASK-25-48): the
/// JSON formatter writes to the same path the text format would have, so
/// a single run can be redirected to a file in either format. This is
/// the cheapest end-to-end check that the two flags wire into the same
/// `setup_tracing` call and do not interact destructively.
#[test]
fn json_format_composes_with_log_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_unknown_key(dir.path())?;
    let log_path = dir.path().join("run.log");
    assert!(!log_path.exists(), "log path must not exist before the run");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--log-level")
        .arg("warn")
        .arg("--log-format")
        .arg("json")
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
        "combining --log-format json with --log-file must succeed; got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    let log_contents = fs::read_to_string(&log_path)?;
    assert!(
        !stderr.contains(UNKNOWN_KEY),
        "--log-file must redirect the warning *away from* stderr even under --log-format json; got:\n{stderr}"
    );

    let canary = log_contents.lines().find(|line| line.contains(UNKNOWN_KEY)).unwrap_or_else(|| {
        panic!("the log file must contain the settings-load warning about {UNKNOWN_KEY:?}; got:\n{log_contents}")
    });

    let value: serde_json::Value = serde_json::from_str(canary)
        .unwrap_or_else(|error| panic!("the log file line must be a valid JSON object under --log-format json: {error} in {canary:?}\nfull log:\n{log_contents}"));
    let object =
        value.as_object().unwrap_or_else(|| panic!("the log file line must be a JSON object; got: {canary:?}"));
    assert!(object.contains_key("level"), "JSON log line must carry `level`: {canary:?}");
    assert!(object.contains_key("message"), "JSON log line must carry `message`: {canary:?}");
    assert!(object.contains_key("timestamp"), "JSON log line must carry `timestamp`: {canary:?}");

    Ok(())
}
