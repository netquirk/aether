//! Integration tests for the ACP trace-message logging (TASK-25-467).
//!
//! `--log-level trace` on `aether acp …` installs a per-line debug callback
//! on the stdio ACP transport, so every JSON-RPC message the server sends
//! and receives is recorded in the per-day log file as a single line
//! naming direction + method. At lower log levels the callback is never
//! installed, so neither `acp recv …` nor `acp send …` ever appear.
//!
//! The tests below are the done-when check the task names: a trace run
//! logs the exchange, and a non-trace run does not. Each test spawns the
//! real `aether` binary over stdio (modelled on `acp_stdio.rs`), sends a
//! serde-built `initialize` request, then reads every log file the
//! `daily(log_dir, "aether-acp.log")` writer produces in
//! `crates/aether-cli/src/acp/mod.rs::setup_logging`.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest};
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Run a single `initialize` exchange against a freshly-spawned `aether acp`
/// subprocess. The child writes nothing on its own after the response, so
/// the caller can `kill` it and still have a complete log file.
///
/// Note: dropping `stdin` here shuts down the server's read half *before*
/// the response is read, which races with the server's shutdown path and
/// surfaces as an EOF. Mirror `acp_stdio.rs::assert_serves_initialize`
/// instead — keep stdin open until after the response is read, then
/// `kill` the child. The server's `clean_disconnect` outcome is
/// irrelevant to this test because we only care what was logged.
fn run_initialize(log_dir: &Path, level: &str) -> TestResult<()> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("acp")
        .arg("--log-dir")
        .arg(log_dir)
        .arg("--log-level")
        .arg(level)
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("child stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("child stdout"))?;

    stdin.write_all(initialize_line()?.as_bytes())?;
    stdin.flush()?;

    let mut response = String::new();
    BufReader::new(stdout).read_line(&mut response)?;
    assert_initialize_response(&response)?;

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Concatenate every `aether-acp.log*` file in `log_dir` into a single
/// string. `tracing_appender::rolling::daily` appends a date suffix to
/// file names, so a sweep across the directory is the robust way to
/// observe what was written. Each file's contents are joined with a
/// newline so a match cannot accidentally straddle two files.
fn read_log_dir(log_dir: &Path) -> TestResult<String> {
    let mut combined = String::new();
    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("aether-acp.log") {
            continue;
        }
        combined.push_str(&fs::read_to_string(entry.path())?);
        combined.push('\n');
    }
    Ok(combined)
}

#[test]
fn trace_logs_each_acp_message_with_its_method() -> TestResult {
    // Acceptance criterion A: each ACP message aether sends and receives
    // is logged as one line naming its method when --log-level trace is
    // set. The `acp recv initialize` sentinel is only emitted by
    // `log_acp_message` when the server's incoming `initialize` line is
    // processed; `acp send response` is only emitted when the outgoing
    // `initialize` response is framed. Both are produced by our
    // instrumentation — not the vendored crate's own tracing events.
    let log_dir = tempfile::tempdir()?;
    run_initialize(log_dir.path(), "trace")?;

    let body = read_log_dir(log_dir.path())?;
    assert!(body.contains("acp recv initialize"), "trace run should log the incoming `initialize` frame, got:\n{body}");
    assert!(body.contains("acp send response"), "trace run should log the outgoing response frame, got:\n{body}");

    Ok(())
}

#[test]
fn lower_log_levels_log_no_acp_messages() -> TestResult {
    // Acceptance criterion B: at every level below `trace` the debug
    // callback is never installed, so no `acp recv …` / `acp send …`
    // trace line is ever written. We exercise the highest non-trace
    // level (`debug`) so the filter actually accepts the run's own
    // tracing events too; only our own emission is absent.
    let log_dir = tempfile::tempdir()?;
    run_initialize(log_dir.path(), "debug")?;

    let body = read_log_dir(log_dir.path())?;
    assert!(!body.contains("acp recv "), "debug run must not log incoming ACP messages, got:\n{body}");
    assert!(!body.contains("acp send "), "debug run must not log outgoing ACP messages, got:\n{body}");

    Ok(())
}

fn initialize_line() -> TestResult<String> {
    let params = serde_json::to_value(InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test", "1")))?;
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": params,
        "id": 1
    });
    Ok(format!("{}\n", serde_json::to_string(&line)?))
}

fn assert_initialize_response(line: &str) -> TestResult {
    let response: serde_json::Value = serde_json::from_str(line)?;
    assert_eq!(response["id"], serde_json::json!(1), "response should echo the request id: {response}");
    assert_eq!(response["result"]["protocolVersion"], 2);
    assert_eq!(response["result"]["info"]["name"], "Aether");
    Ok(())
}
