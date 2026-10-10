//! Integration test: the run log names the provider and model at run start
//! (TASK-25-119).
//!
//! A run writes one `tracing::info!` line near its start naming the provider
//! and the model id the run is about to answer. The line is emitted before
//! the agent/MCP/provider build so the integration test points the `--model`
//! flag at a closed local port; the run is expected to fail on the refused
//! HTTP call, but the run-start line must already be on disk before that
//! happens. The test asserts the file contains `provider=ollama` and
//! `model_id=llama3.2` within the first few lines (i.e. the record is "near
//! the start", not buried after provider/turn output) so the deliverable is
//! observable in the existing run-log path with the existing `--log-level`
//! flag and no new flag.

use std::error::Error;
use std::net::TcpListener;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// `aether headless --model ollama:llama3.2 --log-level info --log-file PATH`
/// must write a single `tracing::info!` line near the start of PATH that
/// names the provider (`ollama`) and the model id (`llama3.2`). The line is
/// emitted before any provider call, so the run is aimed at a closed
/// loopback port to keep the test offline and fast — the refused connection
/// is the expected failure mode that exercises the post-startup code path
/// without making the test hang on a real provider call.
#[test]
fn run_start_log_names_provider_and_model() -> TestResult {
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
    assert!(log.contains("provider=ollama"), "run log must name the provider; got:\n{log}");
    assert!(log.contains("model_id=llama3.2"), "run log must name the model id; got:\n{log}");

    // The line must be near the start, not buried after provider/turn output.
    // Use a small bound so the assertion still catches a regression where a
    // future change pushes the record behind many other entries.
    let line_index = log
        .lines()
        .position(|line| line.contains("model_id=llama3.2"))
        .expect("model line must be present in the run log");
    assert!(line_index <= 3, "model line must be near the start of the run log (line index {line_index}); got:\n{log}");

    Ok(())
}
