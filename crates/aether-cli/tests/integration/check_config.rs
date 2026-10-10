//! Integration tests for the `aether --check-config` flag.
//!
//! `--check-config` exercises the same settings load the subcommands use
//! (defaults or the file named by `--config`/`--settings-file`/the inline
//! `--settings-json`) and exits 0 when the load succeeds. When the loaded
//! document is malformed it prints the load error to stderr and exits
//! non-zero. The flag must do so without starting a run, building a session,
//! or contacting any provider, mirroring the `--list-profiles` pattern.
//!
//! These tests run the real `aether` binary against a temp settings file
//! (mirroring `config_flag.rs` and `list_profiles.rs`).

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Write a settings document that declares a single user-invocable agent.
/// The agent points at `PROMPT.md` (created alongside it) so prompt
/// resolution succeeds without dragging in a model-specific prompt
/// directory. Mirrors `write_settings_with_agent` in `config_flag.rs`.
fn write_settings_with_agent(dir: &std::path::Path, agent_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    fs::write(dir.join("PROMPT.md"), "Be helpful\n")?;
    let settings_path = dir.join("settings.json");
    let body = format!(
        r#"{{
            "credentialsStore": {{ "type": "memory" }},
            "agents": [
                {{
                    "name": "{agent_name}",
                    "description": "{agent_name} agent",
                    "model": "anthropic:claude-sonnet-4-5",
                    "userInvocable": true,
                    "prompts": ["PROMPT.md"]
                }}
            ]
        }}"#
    );
    fs::write(&settings_path, body)?;
    Ok(settings_path)
}

/// `aether --check-config --config PATH` exits 0 on a config that loads
/// cleanly. The success message is printed to stdout so tooling can see it.
#[test]
fn check_config_exits_zero_for_a_valid_config() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_agent(dir.path(), "alpha")?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("--check-config")
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
        "--check-config must exit 0 for a valid config, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(stdout.contains("Configuration is valid"), "stdout must confirm the config loaded; got:\n{stdout}");

    Ok(())
}

/// `aether --check-config --config PATH` exits non-zero on malformed JSON
/// and prints the parse error (named by `SettingsError::ParseError` in
/// `aether-project/src/error.rs`) to stderr. The message must include the
/// `Failed to parse settings file` prefix so users can locate the failure
/// without scraping the rest of the output.
#[test]
fn check_config_prints_the_error_and_exits_non_zero_for_a_malformed_config() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("settings.json");
    fs::write(&config_path, "{ this is not json")?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("--check-config")
        .arg("--config")
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "--check-config with malformed JSON must exit non-zero, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(
        !stderr.trim().is_empty(),
        "stderr must carry the error message for a malformed config; got empty stderr; stdout:\n{stdout}"
    );

    assert!(
        stderr.contains("Failed to parse settings file"),
        "stderr must contain `Failed to parse settings file` for a malformed config; got:\n{stderr}"
    );

    Ok(())
}
