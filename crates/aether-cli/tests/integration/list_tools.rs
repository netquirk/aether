//! Integration tests for `aether show-prompt --list-tools`.
//!
//! `--list-tools` must print one line per model-visible tool name (the same
//! set the agent exposes to the model during a run) and exit 0. It must do so
//! without building the system prompt and without contacting any provider.

use std::error::Error;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

/// Inline MCP block containing the built-in `coding` server (model-visible,
/// no args). This is the smallest inline config that produces a non-empty
/// model-visible tool list without contacting a provider.
const SETTINGS_WITH_CODING: &str = r#"{
  "credentialsStore": {"type": "memory"},
  "prompts": [{"type": "text", "text": "inspect-only agent for the list-tools integration test"}],
  "agents": [
    {
      "name": "Inspector",
      "description": "Inspect-only agent for the list-tools integration test",
      "model": "anthropic:claude-sonnet-4-5",
      "userInvocable": true,
      "mcps": [
        {
          "type": "inline",
          "servers": {
            "coding": {
              "type": "in-memory",
              "args": []
            }
          }
        }
      ]
    }
  ]
}"#;

/// `aether show-prompt --list-tools` exits 0, prints one model-visible tool
/// name per non-empty line, and never renders the system prompt or the stats
/// block that the default `show-prompt` invocation produces. The list must
/// come from the same `tool_definitions()` registry a normal run uses, so the
/// well-known builtin coding tool name `coding__bash` must appear in the
/// output (the model-facing name the agent exposes to the model).
#[test]
fn list_tools_prints_one_name_per_line_and_exits_zero() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("show-prompt")
        .arg("--list-tools")
        .arg("--settings-json")
        .arg(SETTINGS_WITH_CODING)
        .arg("--cwd")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "--list-tools must exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let lines: Vec<&str> = stdout.lines().filter(|line| !line.trim().is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "stdout must list at least one tool (the inline `coding` server is configured); got:\n{stdout}"
    );

    for line in &lines {
        assert!(
            !line.chars().any(char::is_whitespace),
            "every non-empty line must be a single tool name with no whitespace; got {line:?} in:\n{stdout}"
        );
    }

    assert!(
        stdout.contains("coding__bash"),
        "stdout must list the model-facing builtin `coding__bash` from the inline `coding` server; got:\n{stdout}"
    );

    assert!(
        !stdout.contains("--- Tools ("),
        "--list-tools must not render the `--- Tools (...) ---` block from the default `show-prompt` path; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Prompt chars"),
        "--list-tools must not render the stats block from the default `show-prompt` path; got:\n{stdout}"
    );

    Ok(())
}
