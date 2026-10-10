//! Integration tests for the `--config PATH` flag added to `aether`.
//!
//! Acceptance criteria are (a) `--config PATH` loads the named file instead
//! of the default user/project settings, (b) a path that does not exist fails
//! with a message that names it, and (c) the flag being absent preserves the
//! pre-existing default behaviour. `--dry-run` short-circuits the headless
//! loop before any session, MCP, or provider call (`headless/mod.rs:155`), so
//! these tests do not need stdin, network, or any pre-existing settings on
//! disk.
//!
//! The top-level placement — `--config` written before the subcommand, or
//! `aether --config <path>` with no subcommand at all — is covered by
//! `top_level_config_flag_names_a_missing_path` and
//! `bare_config_flag_names_a_missing_path` below. The eager
//! `SettingsSourceArgs::verify_explicit_source` check in `main()` reports a
//! missing path the same way the subcommand path already does.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// Write a settings document that names a single user-invocable agent. The
/// agent points at `PROMPT.md` (created alongside it) so prompt resolution
/// succeeds without dragging in a model-specific prompt directory.
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

/// `--config PATH` reads the named file and prints the agent it defines
/// instead of the default. Two distinct agent names confirm the run picks up
/// the file passed to the flag.
#[test]
fn config_flag_loads_the_named_file_instead_of_default() -> TestResult {
    let build_dir = tempfile::tempdir()?;
    let review_dir = tempfile::tempdir()?;
    let build_config = write_settings_with_agent(build_dir.path(), "build")?;
    let review_config = write_settings_with_agent(review_dir.path(), "review")?;

    for (config_path, expected_profile, label) in
        [(build_config.as_path(), "build", "build"), (review_config.as_path(), "review", "review")]
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
        command
            .arg("headless")
            .arg("--dry-run")
            .arg("--config")
            .arg(config_path)
            .arg("--cwd")
            .arg(build_dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "--dry-run must exit 0 for {label} config, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code(),
        );

        let profile_line = format!("profile: {expected_profile}");
        assert!(
            stdout.contains(&profile_line),
            "stdout must name `{expected_profile}` (from --config {label}); got:\n{stdout}",
        );
    }

    Ok(())
}

/// `--config PATH` for a path that does not exist must fail and the error
/// message must name the offending path.
#[test]
fn config_flag_names_a_missing_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("does-not-exist.json");
    let missing_for_assert = missing.clone();

    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("headless")
        .arg("--dry-run")
        .arg("--config")
        .arg(&missing)
        .arg("--cwd")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "--config with a missing path must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let expected = missing_for_assert.to_string_lossy().into_owned();
    assert!(stderr.contains(&expected), "stderr must name the missing path {expected:?}; got:\n{stderr}");

    Ok(())
}

/// Without `--config` the headless path must continue to read its existing
/// default. The pre-existing `--settings-json` flag covers that surface and
/// confirms the absent-flag branch is untouched by the new option.
#[test]
fn absent_config_flag_keeps_the_default() -> TestResult {
    let dir = tempfile::tempdir()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let endpoint = format!("http://{addr}");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--provider")
        .arg(format!("ollama.url={endpoint}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "absent --config must keep default behaviour, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stdout.contains("model: ollama:llama3.2"),
        "default path must still print the resolved model; got:\n{stdout}"
    );
    assert!(stdout.contains("profile: "), "default path must still print a profile line; got:\n{stdout}");

    Ok(())
}

/// `--config PATH` placed **before** the subcommand parses into the top-level
/// `Cli::settings_source`, which the run paths ignore. The new eager
/// `verify_explicit_source` check in `main()` must surface a missing path
/// instead of silently dropping it, with a message that names the path.
#[test]
fn top_level_config_flag_names_a_missing_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("does-not-exist.json");
    let missing_for_assert = missing.clone();

    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("--config")
        .arg(&missing)
        .arg("headless")
        .arg("--dry-run")
        .arg("--cwd")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "top-level --config with a missing path must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let expected = missing_for_assert.to_string_lossy().into_owned();
    assert!(stderr.contains(&expected), "stderr must name the missing path {expected:?}; got:\n{stderr}");

    Ok(())
}

/// Bare `aether --config <path>` (no subcommand) parses into the top-level
/// `Cli::settings_source` and would otherwise be ignored by the default TUI
/// path. The eager check must exit non-zero and name the path; this is
/// deterministic because the check runs before `run_default_command` and so
/// before the onboarding / TTY path is ever reached.
#[test]
fn bare_config_flag_names_a_missing_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("does-not-exist.json");
    let missing_for_assert = missing.clone();

    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command.arg("--config").arg(&missing).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "bare aether --config with a missing path must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let expected = missing_for_assert.to_string_lossy().into_owned();
    assert!(stderr.contains(&expected), "stderr must name the missing path {expected:?}; got:\n{stderr}");

    Ok(())
}

/// A directory passed as `--config` is "readable" as a path (it exists) but
/// cannot be parsed as a settings document, so the eager check must report
/// the path rather than silently swallowing the unreadable file.
#[test]
fn config_pointing_at_a_directory_names_the_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let directory_for_assert = dir.path().to_path_buf();

    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("--config")
        .arg(dir.path())
        .arg("headless")
        .arg("--dry-run")
        .arg("--cwd")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "--config pointing at a directory must exit non-zero, got {:?}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let expected = directory_for_assert.to_string_lossy().into_owned();
    assert!(stderr.contains(&expected), "stderr must name the path {expected:?}; got:\n{stderr}");

    Ok(())
}
