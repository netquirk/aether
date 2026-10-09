//! Integration tests for `aether headless --system-prompt-file`.
//!
//! `--system-prompt-file` loads the run's system prompt from a file. A missing
//! or unreadable file must fail the run with a clear error that names the
//! path, and combining the new flag with the existing `--system-prompt` must
//! be rejected. These tests stop at config resolution, so they never start a
//! session or contact a provider.

use std::error::Error;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

const SETTINGS_JSON: &str = r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#;

/// `aether headless --system-prompt-file <path>` with a path that does not
/// exist must fail the run with a non-zero exit and an error that names the
/// path. This must fail at config resolution (before any provider call), so
/// the test does not need network access.
#[test]
fn headless_rejects_missing_system_prompt_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("no-such-prompt.txt");
    let missing_text = missing.to_string_lossy().into_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--settings-json")
        .arg(SETTINGS_JSON)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--system-prompt-file")
        .arg(&missing)
        .arg("hi")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "missing --system-prompt-file must fail; got exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(stderr.contains("failed to read system prompt"), "stderr must announce the read failure: {stderr}");
    assert!(stderr.contains(&missing_text), "stderr must name the path {missing_text:?}: {stderr}");

    Ok(())
}

/// Passing both `--system-prompt` and `--system-prompt-file` is rejected by
/// clap before any config resolution happens, so the test only has to assert
/// a non-zero exit and that the conflict message names both flags.
#[test]
fn headless_rejects_both_system_prompt_and_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let prompt_file = dir.path().join("prompt.txt");
    std::fs::write(&prompt_file, "you are a helpful agent")?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--settings-json")
        .arg(SETTINGS_JSON)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--system-prompt")
        .arg("inline prompt")
        .arg("--system-prompt-file")
        .arg(&prompt_file)
        .arg("hi")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "both --system-prompt and --system-prompt-file must be rejected; got exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stderr.contains("--system-prompt") && stderr.contains("--system-prompt-file"),
        "stderr must name both flags in the conflict message: {stderr}"
    );

    Ok(())
}

/// `aether headless` reached through `--options-json` with a `systemPromptFile`
/// pointing at a missing path must also fail and name the path, mirroring the
/// flag-path failure mode for harness callers that always pass
/// `--options-json`.
#[test]
fn headless_rejects_missing_system_prompt_file_via_options_json() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("missing.json");
    let missing_text = missing.to_string_lossy().into_owned();
    let cwd_text = dir.path().to_string_lossy().into_owned();
    let options = serde_json::json!({
        "prompt": "hi",
        "settings": {
            "credentialsStore": {"type": "memory"},
            "agents": [],
        },
        "systemPromptFile": &missing_text,
        "cwd": &cwd_text,
    })
    .to_string();

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--options-json")
        .arg(&options)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "missing systemPromptFile via --options-json must fail; got exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(stderr.contains("failed to read system prompt"), "stderr must announce the read failure: {stderr}");
    assert!(stderr.contains(&missing_text), "stderr must name the path {missing_text:?}: {stderr}");

    Ok(())
}
