//! Integration tests for the top-level `run.shellCommandTimeoutSeconds` block
//! on [`AetherSettings`] being threaded into the MCP runtime's
//! `shell_command_timeout` (TASK-26-470). The setting bounds every shell
//! command a run starts (the `bash` tool of the built-in `coding` MCP server):
//! a command that outruns the configured limit is killed, its structured
//! result carries `killed: true` plus a "timed out after …" marker, and the
//! run continues rather than hanging.
//!
//! Two complementary assertions live here:
//! 1. Driving the production [`aether_core::mcp::McpBuilder`] (the same
//!    chain the CLI uses inside [`crate::runtime::RuntimeBuilder`]) with a
//!    500 ms limit and dispatching `coding__bash` with `sleep 5` observes
//!    the kill and the timeout marker on the first call, then continues to
//!    `exit 3` on the same live session — the second call returns
//!    `killed: false`, `exitCode: 3`, and a payload that does **not** name
//!    the marker. This is the run-continues-rather-than-hangs half of the
//!    done-when.
//! 2. The same timeout-bearing result, written into a JSON Lines transcript
//!    via [`aether_cli::transcript::JsonlTranscript`], carries the
//!    "timed out" marker in its `event.result` field — the
//!    transcript-names-it half of the done-when. The companion `exit 3`
//!    record round-trips through the same writer and the
//!    transcript-level assertion is its negative.
//!
//! Both halves use the same `bash` tool that the agent loop dispatches over
//! the MCP wire, so a regression in the coding server's timeout behaviour,
//! the MCP builder's `with_shell_command_timeout` plumbing, or the
//! transcript's `result` payload shows up as a failed assertion rather than
//! a silently-dropped marker.
//!
//! These tests do not start an agent or contact a provider: the goal is to
//! pin the contract that the headless CLI's `run` loop and the transcript
//! writer rely on when a per-command limit is configured.

use aether_cli::transcript::JsonlTranscript;
use aether_core::agent_spec::McpConfigSource;
use aether_core::events::{AgentEvent, ToolEvent};
use aether_core::mcp::mcp;
use futures::StreamExt;
use llm::ToolCallResult;
use mcp_servers::McpBuilderExt;
use mcp_utils::client::{CallToolOptions, CancellationToken, ToolCallEvent};
use std::error::Error;
use std::time::Duration;

type TestResult = Result<(), Box<dyn Error>>;

/// The single in-memory `coding` server the test wires into the manager. The
/// empty `args` list lets `CodingMcpArgs::from_args` accept the call as valid
/// while leaving every tool enabled (the limit under test is propagated through
/// the `bash` tool regardless of LSP/permission-mode settings). Mirrors
/// `tests/integration/shell_environment.rs` so the two tests stay symmetric.
const CODING_SERVER_SOURCE: &str = r#"{"servers":{"coding":{"type":"in-memory","args":[]}}}"#;

/// Substring the timed-out branch of `BashEnvironment::execute_command` writes
/// into the structured `output` field when the per-command timeout fires
/// (`Command timed out after {N}ms`). The substring is the marker the
/// transcript names the killed call by; asserting on its presence (vs. a
/// bare `killed: true` flag) keeps the test honest about distinguishing a
/// killed-by-timeout run from any other non-zero exit.
const TIMEOUT_MARKER: &str = "Command timed out after";

/// Spin up a production MCP manager with the configured per-command shell
/// timeout and a single in-memory `coding` server, mirroring the chain
/// [`crate::runtime::RuntimeBuilder`] uses inside the CLI but without the
/// agent / provider layers. The returned `McpSession` is the live handle
/// the test calls `coding__bash` through, exactly as the agent loop would
/// at run time.
async fn spawn_coding_with_timeout(
    workspace: &tempfile::TempDir,
    timeout: Duration,
) -> Result<(aether_core::mcp::McpSession, String), Box<dyn Error>> {
    let mut spawn = mcp(workspace.path())
        .with_shell_command_timeout(timeout)
        .with_builtin_servers()
        .from_mcp_config_sources(&[McpConfigSource::Json(CODING_SERVER_SOURCE.to_string())])?
        .spawn()
        .await?;

    let snapshot =
        spawn.block_until_ready().await.ok_or_else(|| -> Box<dyn Error> { "MCP bootstrap aborted".into() })?;
    let bash_tool = snapshot.tool_definitions().into_iter().find(|tool| tool.name == "coding__bash").ok_or_else(
        || -> Box<dyn Error> {
            format!("coding__bash must be in the snapshot; found: {:?}", snapshot.tool_definitions()).into()
        },
    )?;

    Ok((spawn, bash_tool.name))
}

/// Dispatch one `coding__bash` call over the live MCP session and return the
/// raw `rmcp::model::CallToolResult` the server returned. The harness timeout
/// (passed via `CallToolOptions::timeout`) is intentionally generous
/// compared to the per-command timeout (500 ms) so the test's outer wait
/// does not race the per-command kill; on the timed-out path the MCP layer
/// returns in well under 1 s with `killed: true`.
async fn call_bash(
    spawn: &mut aether_core::mcp::McpSession,
    tool_name: &str,
    command: &str,
) -> Result<rmcp::model::CallToolResult, Box<dyn Error>> {
    let request_arguments = serde_json::json!({
        "command": command,
        "description": "shell-command-timeout integration test",
    })
    .to_string();

    // Outer harness timeout is 30 s, matching `tests/integration/shell_environment.rs`.
    // The per-command kill fires at the configured 500 ms so the call returns
    // in well under that bound; a generous outer timeout keeps a regression
    // that disables the per-command kill from racing the harness into a
    // false failure.
    let options = CallToolOptions { timeout: Duration::from_secs(30), meta: None, cancel: CancellationToken::new() };
    let mut events = spawn.handle().call_model_visible(tool_name.to_string(), &request_arguments, options);

    while let Some(event) = events.next().await {
        match event {
            ToolCallEvent::Complete(Ok(result)) => {
                // Mirror `tests/integration/shell_environment.rs`: the bash tool
                // returns its `BashOutput` struct via `structured_content`, so
                // `result.structured_content` carries `{output, exitCode, killed}`
                // for our assertions. The full serialized result also lands in
                // the transcript via `convert_tool_result`'s YAML form (see
                // `aether_core::mcp::tool_bridge`).
                return Ok(result);
            }
            ToolCallEvent::Complete(Err(error)) => {
                return Err(format!("bash tool returned an error: {error}").into());
            }
            ToolCallEvent::TaskComplete { .. } => {
                return Err("coding__bash must complete inline, not via an MCP Task".into());
            }
            ToolCallEvent::Cancelled { .. } | ToolCallEvent::Progress(_) | ToolCallEvent::TaskCreated(_) => {
                // Ignore lifecycle events; we only act on the terminal
                // `Complete` / `TaskComplete` outcome.
            }
            ToolCallEvent::TaskStatus(_) => {}
        }
    }
    Err("MCP manager stopped before returning the bash result".into())
}

/// Pull the `killed` / `exitCode` / `output` triple out of a `BashOutput`
/// payload serialised by the MCP server's `structured_content`. The keys
/// are camelCase because `BashOutput` uses
/// `#[serde(rename_all = "camelCase")]` (see
/// `crates/mcp-servers/src/coding/tools/bash/mod.rs`), so the model-visible
/// shape is `{ output, exitCode, killed, _meta? }`.
fn bash_fields(result: &rmcp::model::CallToolResult) -> Result<(bool, i64, String), Box<dyn Error>> {
    let value = result.structured_content.as_ref().ok_or_else(|| -> Box<dyn Error> {
        format!("bash tool result must carry structured_content; got content: {:?}", result.content).into()
    })?;
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| -> Box<dyn Error> { format!("bash structured_content must carry an `output` string; got: {value}").into() })?
        .to_string();
    let killed = value
        .get("killed")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| -> Box<dyn Error> { format!("bash structured_content must carry a `killed` boolean; got: {value}").into() })?;
    let exit_code = value
        .get("exitCode")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| -> Box<dyn Error> { format!("bash structured_content must carry an `exitCode` integer; got: {value}").into() })?;
    Ok((killed, exit_code, output))
}

/// Driving the production [`aether_core::mcp::McpBuilder`] with
/// `with_shell_command_timeout(500 ms)` and dispatching `coding__bash` with
/// `sleep 5` observes the kill and the timeout marker; a follow-up `exit 3`
/// on the same live session continues normally with `killed: false` and
/// `exitCode: 3`. The contrast between the two calls is the run-continues
/// half of the done-when: a regression that hangs the session on the first
/// call fails the second-call assertion; a regression that fails to mark
/// the timed-out one fails the marker + `killed: true` assertion; a
/// regression that flips `killed` on a normal non-zero exit fails the
/// `exit 3` assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_timeout_kills_long_running_command_and_run_continues() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let (mut spawn, tool_name) = spawn_coding_with_timeout(&workspace, Duration::from_millis(500)).await?;

    // Call 1: `sleep 5` bounded at 500 ms. The kill path returns
    // `killed: true`, the `output` field carries the "timed out after" marker,
    // and `exitCode` is the sentinel `-1` the timed-out branch writes (the
    // process was killed before its real exit code could be observed).
    let timed_out = call_bash(&mut spawn, &tool_name, "sleep 5").await?;
    let (killed, exit_code, output) = bash_fields(&timed_out)?;
    assert!(
        killed,
        "the timed-out bash call must set `killed: true`; got structured_content: {:?}",
        timed_out.structured_content
    );
    assert_eq!(
        exit_code, -1,
        "the timed-out bash call must report exitCode -1 (killed before real exit); got structured_content: {:?}",
        timed_out.structured_content
    );
    assert!(
        output.contains(TIMEOUT_MARKER),
        "the timed-out bash call must name the timeout in `output`; output={output:?}"
    );

    // Call 2: `exit 3` on the same live session. The timeout only bounds
    // commands that outrun it; a 0-second non-zero exit completes well
    // inside the limit, so `killed` stays `false`, the exit code is
    // preserved verbatim, and `output` does not name the marker. This is
    // the explicit contrast the task asks for (distinct from a normal
    // non-zero exit) and the run-continues half of the done-when.
    let nonzero = call_bash(&mut spawn, &tool_name, "exit 3").await?;
    let (killed, exit_code, output) = bash_fields(&nonzero)?;
    assert!(
        !killed,
        "the `exit 3` bash call must set `killed: false`; got structured_content: {:?}",
        nonzero.structured_content
    );
    assert_eq!(
        exit_code, 3,
        "the `exit 3` bash call must report exitCode 3; got structured_content: {:?}",
        nonzero.structured_content
    );
    assert!(
        !output.contains(TIMEOUT_MARKER),
        "the `exit 3` bash call must not name the timeout marker; output={output:?}"
    );

    Ok(())
}

/// The timed-out result round-trips through a JSON Lines transcript and the
/// saved record names the kill. Mirrors
/// `tests/integration/transcript_jsonl.rs` (the `JsonlTranscript::create` /
/// `record` / `flush` pattern) so the transcript wiring the
/// `--transcript-jsonl` flag already uses is the same one this test
/// exercises. The companion `exit 3` record is the negative — its saved
/// `event.result` does not name the marker — so the transcript-level
/// assertion is also a contrast check rather than a single positive
/// observation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_timeout_marker_lands_in_jsonl_transcript_record() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let (mut spawn, tool_name) = spawn_coding_with_timeout(&workspace, Duration::from_millis(500)).await?;

    // Drive both calls the same way `shell_command_timeout_kills_…` does so
    // the transcript assertion sees the same payloads. Each call is wrapped
    // in its own `AgentEvent::Tool(ToolEvent::Result { .. })` event because that
    // is the shape `JsonlTranscript::record` accepts; the inner `ToolCallResult`
    // is what the bridge wrote into `result.result` for the agent loop. The
    // transcript writer serialises the event as JSON, so the on-disk line
    // carries the `command` / `description` for grep.
    let timed_out = call_bash(&mut spawn, &tool_name, "sleep 5").await?;
    let timed_out_yaml = structured_content_to_yaml(timed_out.structured_content.as_ref())?;
    let timed_out_event = AgentEvent::Tool(ToolEvent::Result {
        result: ToolCallResult {
            id: "tc-timed-out".to_string(),
            name: "coding__bash".to_string(),
            arguments: serde_json::json!({
                "command": "sleep 5",
                "description": "shell-command-timeout integration test",
            })
            .to_string(),
            result: timed_out_yaml,
        },
        result_meta: None,
    });

    let nonzero = call_bash(&mut spawn, &tool_name, "exit 3").await?;
    let nonzero_yaml = structured_content_to_yaml(nonzero.structured_content.as_ref())?;
    let nonzero_event = AgentEvent::Tool(ToolEvent::Result {
        result: ToolCallResult {
            id: "tc-nonzero".to_string(),
            name: "coding__bash".to_string(),
            arguments: serde_json::json!({
                "command": "exit 3",
                "description": "shell-command-timeout integration test",
            })
            .to_string(),
            result: nonzero_yaml,
        },
        result_meta: None,
    });

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("transcript.jsonl");
    let mut transcript = JsonlTranscript::create(&path)?;
    transcript.record(&timed_out_event)?;
    transcript.record(&nonzero_event)?;
    transcript.flush()?;

    let raw = std::fs::read_to_string(&path)?;
    let mut saw_marker = false;
    let mut saw_no_marker = false;
    for (index, line) in raw.lines().enumerate() {
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("line {index} is not valid JSON: {e}; line={line:?}"))?;
        // The transcript wraps every event under `event.event` (the outer
        // `event` is the `TranscriptRecord` envelope carrying `turn` / `type`,
        // the inner `event` is the serialized `AgentEvent` itself, and the
        // `ToolEvent::Result { result, .. }` payload sits under `event.event.result`).
        let result = value
            .get("event")
            .and_then(|event| event.get("event"))
            .and_then(|event| event.get("result"))
            .and_then(|result| result.get("result"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("line {index} is missing `event.event.event.result.result`: {value:?}"))?;
        if result.contains(TIMEOUT_MARKER) {
            saw_marker = true;
        } else {
            saw_no_marker = true;
        }
    }
    assert!(
        saw_marker,
        "the timed-out tool record must carry the \"timed out\" marker in the saved transcript; lines={raw:?}"
    );
    assert!(
        saw_no_marker,
        "the `exit 3` tool record must also be present in the saved transcript (with no timeout marker); lines={raw:?}"
    );

    Ok(())
}

/// Render an MCP `structured_content` value as the YAML form
/// [`aether_core::mcp::tool_bridge::convert_tool_result`] writes into
/// `ToolCallResult::result`. The bridge filters YAML candidates through a
/// round-trip so a structured value that round-trips losslessly ends up as
/// YAML and one that doesn't falls back to JSON; the round-trip always
/// succeeds for the bash tool (every field is a primitive). Using the
/// same shape keeps the transcript's `event.result` faithful to what the
/// production run would have recorded.
fn structured_content_to_yaml(value: Option<&serde_json::Value>) -> Result<String, Box<dyn Error>> {
    let Some(value) = value else {
        return Err("structured_content was missing on the bash result".into());
    };
    let yaml = serde_yml::to_string(value).map_err(|e| format!("structured_content failed to serialize: {e}"))?;
    if !serde_yml::from_str::<serde_json::Value>(&yaml).is_ok_and(|decoded| decoded == *value) {
        return Err(format!("structured_content YAML did not round-trip: yaml={yaml}").into());
    }
    Ok(yaml)
}