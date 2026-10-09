//! Integration tests for the top-level `shellEnvironment` block on
//! [`AetherSettings`] being inherited by every shell command a run starts —
//! specifically the `bash` tool of the built-in `coding` MCP server.
//!
//! Two assertions live here:
//! 1. The block deserialises into [`AetherSettings::shell_environment`] with
//!    the same key/value pair the user wrote.
//! 2. The same variable reaches the bash process when a `coding__bash` tool
//!    call is dispatched through the production MCP manager the CLI would build
//!    during a run. The expected harness-shaped flow uses
//!    [`aether_core::mcp::McpBuilder`] directly with a single, model-visible
//!    `coding` in-memory server, exactly the same wiring
//!    [`crate::runtime::RuntimeBuilder`] uses inside the CLI.
//!
//! These tests do not start an agent or contact a provider: the goal is to
//! pin the contract that runs (and any future harness built on
//! [`McpBuilder`]) rely on when a custom env var is needed by a `bash`
//! invocation.

use aether_core::agent_spec::McpConfigSource;
use aether_core::mcp::mcp;
use aether_project::AetherSettings;
use futures::StreamExt;
use mcp_servers::McpBuilderExt;
use mcp_utils::client::{CallToolOptions, CancellationToken, ToolCallEvent};
use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

/// One environment variable key used by the test. Chosen to be unique enough
/// that it cannot collide with anything else that might be present in the
/// process environment.
const ENV_VAR_NAME: &str = "AETHER_TASK24_TEST_VALUE";

/// The value bound to [`ENV_VAR_NAME`] in the test's settings block and the
/// string the bash command asserts against.
const ENV_VAR_VALUE: &str = "from-config";

type TestResult = Result<(), Box<dyn Error>>;

/// The single in-memory `coding` server the test wires into the manager. The
/// empty `args` list lets `CodingMcpArgs::from_args` accept the call as valid
/// while leaving every tool enabled (the variable under test is propagated
/// through the `bash` tool regardless of LSP/permission-mode settings).
const CODING_SERVER_SOURCE: &str = r#"{"servers":{"coding":{"type":"in-memory","args":[]}}}"#;

/// Parsing the top-level `shellEnvironment` block on
/// [`AetherSettings`] surfaces the configured keys on the struct the CLI
/// captures before resolving an agent. Without this, nothing downstream
/// ([`crate::runtime::RuntimeBuilder::shell_environment`] or the in-memory
/// MCP plumbing in [`aether_core::mcp::McpBuilder`]) would receive the
/// caller's intent.
#[test]
fn shell_environment_block_deserializes_into_settings() -> TestResult {
    let json = format!(
        r#"{{ "agents": [], "shellEnvironment": {{ "{name}": "{value}" }} }}"#,
        name = ENV_VAR_NAME,
        value = ENV_VAR_VALUE
    );
    let settings = AetherSettings::try_from(json.as_str())?;

    assert_eq!(
        settings.shell_environment.len(),
        1,
        "exactly the one declared variable must be parsed; got: {:?}",
        settings.shell_environment
    );
    assert_eq!(
        settings.shell_environment.get(ENV_VAR_NAME).map(String::as_str),
        Some(ENV_VAR_VALUE),
        "the parsed value must match the JSON exactly"
    );

    Ok(())
}

/// Driving the production [`McpBuilder`] with the same chain the CLI uses
/// (`with_shell_environment` → `with_builtin_servers` →
/// `from_mcp_config_sources`) and dispatching `coding__bash` over the MCP
/// wire must let bash observe the configured variable. This is the only
/// end-to-end proof that the variable actually reaches a child shell
/// process; unit tests on `BashEnvironment` cannot substitute it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bash_tool_inherits_configured_shell_environment() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let configured = BTreeMap::from([(ENV_VAR_NAME.to_string(), ENV_VAR_VALUE.to_string())]);

    let mut spawn = mcp(workspace.path())
        .with_shell_environment(configured)
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

    // Bash command reads the variable, prints it with `printf` so the output
    // is exactly the value (no trailing newline / whitespace / colour
    // codes that would complicate the assertion). `%s` accepts the value
    // without interpreting `%` placeholders.
    let request_arguments = serde_json::json!({
        "command": format!("printf '%s' \"${name}\"", name = ENV_VAR_NAME),
        "description": "shell-environment integration test",
    })
    .to_string();

    let options = CallToolOptions { timeout: Duration::from_secs(30), meta: None, cancel: CancellationToken::new() };
    let mut events = spawn.handle().call_model_visible(bash_tool.name, &request_arguments, options);

    while let Some(event) = events.next().await {
        match event {
            ToolCallEvent::Complete(Ok(result)) => {
                let structured = result.structured_content.as_ref().ok_or_else(|| -> Box<dyn Error> {
                    format!("bash must return structured_content; got content: {:?}", result.content).into()
                })?;
                let output =
                    structured.get("output").and_then(|value| value.as_str()).ok_or_else(|| -> Box<dyn Error> {
                        format!("bash structured_content must carry an `output` string; got: {structured:?}").into()
                    })?;
                assert_eq!(
                    output, ENV_VAR_VALUE,
                    "the bash tool must observe the configured env var; got stdout/stderr: {output:?}"
                );
                assert_eq!(
                    structured.get("exitCode").and_then(|value| value.as_i64()),
                    Some(0),
                    "the bash command must exit 0; structured_content: {structured:?}"
                );
                return Ok(());
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
