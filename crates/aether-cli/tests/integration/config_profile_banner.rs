//! Integration test: the run-start banner names the config file and the
//! (non-default) profile the run is using (TASK-26-858 / Part 3).
//!
//! A run writes a single `eprintln!` line at the start of `crates/aether-cli/src/headless/run.rs`
//! that names the settings file and the selected profile, so an operator can
//! see which configuration a run actually used at a glance. The line is
//! emitted before the agent/MCP/provider build, so the integration test points
//! `--provider ollama.url=http://<dead-addr>` at a closed loopback port; the
//! run is expected to fail on the refused provider call, but the banner must
//! already be on stderr before that happens. The test asserts stderr contains
//! the absolute config path AND the literal string `profile 'beta'` (a
//! non-default profile, since `alpha` is the first user-invocable agent in
//! the config) with the path and stderr echoed in the failure messages so a
//! regression is easy to triage.

use std::error::Error;
use std::fs;
use std::net::TcpListener;
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

/// `aether headless --config PATH --agent beta --cwd DIR --provider ollama.url=http://<dead-addr> --timeout 15s hello`
/// must write a single `aether: loaded config … (profile 'beta')` line to
/// stderr before the refused provider call. The config declares `alpha` first
/// (the default profile) then `beta`, so selecting `beta` via `--agent` is the
/// non-default case the task calls out.
#[test]
fn run_start_names_config_file_and_non_default_profile() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config_path = write_settings_with_agents(dir.path(), &["alpha", "beta"])?;

    // Bind then drop: the listener's port is freed but the address is
    // immediately closed for new connections, so any subsequent request is
    // refused with `ECONNREFUSED` — the same offline canary the dry-run
    // integration tests use to prove no provider call happens.
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?
    };
    let endpoint = format!("http://{addr}");

    let _output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--config")
        .arg(&config_path)
        .arg("--agent")
        .arg("beta")
        .arg("--cwd")
        .arg(dir.path())
        .arg("--provider")
        .arg(format!("ollama.url={endpoint}"))
        .arg("--timeout")
        .arg("15s")
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stderr = String::from_utf8_lossy(&_output.stderr);
    let config_display = config_path.display().to_string();

    assert!(stderr.contains(&config_display), "stderr must name the config path {config_display:?}; got:\n{stderr}");
    assert!(stderr.contains("profile 'beta'"), "stderr must name the non-default profile 'beta'; got:\n{stderr}");

    Ok(())
}
