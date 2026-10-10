//! Integration tests for the `aether --list-profiles` flag.
//!
//! `--list-profiles` enumerates every agent name declared in the loaded
//! settings, prints one name per line, and exits 0 — including when the
//! config defines zero agents. It must do so without starting a run, building
//! a session, or contacting any provider.
//!
//! These tests run the real `aether` binary against a temp settings file
//! (mirroring `config_flag.rs`); the flag uses the same `--config PATH`
//! source added by TASK-25-8.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Write a settings document that declares the given agent names. Each agent
/// points at `PROMPT.md` (created alongside it) so prompt resolution
/// succeeds without dragging in a model-specific prompt directory.
fn write_settings_with_agents(dir: &std::path::Path, names: &[&str]) -> Result<PathBuf, Box<dyn Error>> {
    fs::write(dir.join("PROMPT.md"), "Be helpful\n")?;
    let agents_json = names
        .iter()
        .map(|name| {
            format!(
                r#"{{
                    "name": "{name}",
                    "description": "{name} agent",
                    "model": "anthropic:claude-sonnet-4-5",
                    "userInvocable": true,
                    "prompts": ["PROMPT.md"]
                }}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let body = format!(
        r#"{{
            "credentialsStore": {{ "type": "memory" }},
            "agents": [{agents_json}]
        }}"#
    );
    let path = dir.join("settings.json");
    fs::write(&path, body)?;
    Ok(path)
}

/// `aether --list-profiles --config PATH` exits 0 and prints each configured
/// agent name exactly once, one per line, in config order.
#[test]
fn list_profiles_prints_one_name_per_line_and_exits_zero() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_agents(dir.path(), &["alpha", "beta"])?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
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
        "--list-profiles must exit 0 for a two-profile config, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["alpha", "beta"],
        "stdout must list each profile on its own line, in config order; got:\n{stdout}"
    );

    Ok(())
}

/// `aether --list-profiles --config PATH` exits 0 even when the config
/// declares zero agents, and emits no non-empty output lines.
#[test]
fn list_profiles_is_empty_and_exits_zero_when_config_defines_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_agents(dir.path(), &[])?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
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
        "--list-profiles with zero profiles must still exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let non_empty: Vec<&str> = stdout.lines().filter(|line| !line.trim().is_empty()).collect();
    assert!(
        non_empty.is_empty(),
        "stdout must contain no non-empty lines when no profiles are configured; got:\n{stdout}"
    );

    Ok(())
}
