//! Verifies the `-C/--cwd` flag on `aether acp` selects the directory the
//! session is rooted at, and that paths reported on the `initialize`
//! response's `_meta["contextbridge/aether"]["remote"]["cwd"]` reflect it.
//! When the flag is absent the current working directory is used as today.
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

const MINIMAL_SETTINGS_JSON: &str = r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#;

#[test]
fn acp_cwd_roots_the_session_at_the_given_directory() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let expected_cwd = workspace.path().canonicalize()?;
    let response = run_acp_initialize(acp_cwd_command(log_dir.path(), &workspace.path()))?;
    assert_remote_cwd(&response, &expected_cwd)
}

#[test]
fn acp_without_cwd_uses_the_process_working_directory() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let expected_cwd = std::env::current_dir()?.canonicalize()?;
    let response = run_acp_initialize(acp_command(log_dir.path()))?;
    assert_remote_cwd(&response, &expected_cwd)
}

fn acp_cwd_command(log_dir: &std::path::Path, workspace: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("acp")
        .arg("--cwd")
        .arg(workspace)
        .arg("--log-dir")
        .arg(log_dir)
        .arg("--settings-json")
        .arg(MINIMAL_SETTINGS_JSON)
        .stderr(Stdio::null());
    command
}

fn acp_command(log_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("acp")
        .arg("--log-dir")
        .arg(log_dir)
        .arg("--settings-json")
        .arg(MINIMAL_SETTINGS_JSON)
        .stderr(Stdio::null());
    command
}

fn run_acp_initialize(mut command: Command) -> TestResult<serde_json::Value> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("child stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("child stdout"))?;

    stdin.write_all(initialize_line()?.as_bytes())?;
    stdin.flush()?;

    let mut response = String::new();
    BufReader::new(stdout).read_line(&mut response)?;

    let _ = child.kill();
    let _ = child.wait();

    let value: serde_json::Value = serde_json::from_str(&response)?;
    Ok(value)
}

fn assert_remote_cwd(response: &serde_json::Value, expected_cwd: &std::path::Path) -> TestResult {
    let remote = response.pointer(&format!("/result/_meta/{POINTER_NAMESPACE}/remote")).ok_or_else(|| {
        let rendered = serde_json::to_string_pretty(response).unwrap_or_default();
        std::io::Error::other(format!("missing _meta remote block; response:\n{rendered}"))
    })?;
    let reported = remote
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing remote.cwd string"))?;
    let reported = PathBuf::from(reported);
    assert_eq!(
        reported.canonicalize().unwrap_or(reported),
        expected_cwd,
        "remote.cwd should match the requested workspace: {response}"
    );
    Ok(())
}

const POINTER_NAMESPACE: &str = "contextbridge~1aether";

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
