//! Integration tests for `aether --disable-tool NAME` (TASK-25-123).
//!
//! `--disable-tool NAME` (repeatable) drops the named tool from the model-visible
//! set exposed by a run. Exercising it through `aether show-prompt --list-tools`
//! is the deterministic offline test seam: that command renders, by its own doc
//! comment, "the same set the agent exposes to the model during a run" through
//! the identical `RuntimeBuilder` code path the headless run uses, so an
//! assertion on its output pinpoints the same tool set a real run would see.
//! No provider is contacted.

use std::error::Error;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

/// Inline MCP block containing the built-in `coding` server. Reused from
/// `list_tools.rs` so both files exercise the same offline tool surface.
const SETTINGS_WITH_CODING: &str = r#"{
  "credentialsStore": {"type": "memory"},
  "prompts": [{"type": "text", "text": "inspect-only agent for the disable-tool integration test"}],
  "agents": [
    {
      "name": "Inspector",
      "description": "Inspect-only agent for the disable-tool integration test",
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

/// Run `aether show-prompt --list-tools --settings-json <json> --cwd <dir> [extra args]`
/// and capture the resulting stdout/stderr/exit status. Helper to keep the four
/// integration test bodies focused on their assertions instead of shelling-out
/// boilerplate.
fn run_list_tools(extra_args: &[&str], cwd: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("show-prompt")
        .arg("--list-tools")
        .arg("--settings-json")
        .arg(SETTINGS_WITH_CODING)
        .arg("--cwd")
        .arg(cwd)
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawning aether must succeed")
}

/// Baseline: `aether show-prompt --list-tools` exposes `coding__bash` from the
/// inline `coding` server so the withheld test below can prove the tool is
/// absent only because of `--disable-tool`, not because the baseline didn't
/// include it.
#[test]
fn disable_tool_baseline_lists_coding_bash() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = run_list_tools(&[], dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "baseline --list-tools must exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stdout.contains("coding__bash"),
        "baseline must list coding__bash (otherwise the withheld assertion below is meaningless); got:\n{stdout}"
    );

    Ok(())
}

/// `aether show-prompt --list-tools --disable-tool coding__bash` exposes the
/// inline `coding` server's tools EXCEPT `coding__bash`, which the model can
/// no longer see; other coding tools stay present. This is the core
/// acceptance check for TASK-25-123: passing `--disable-tool NAME` makes
/// the run's tool list omit NAME.
#[test]
fn disable_tool_withheld_removes_named_tool_from_list() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = run_list_tools(&["--disable-tool", "coding__bash"], dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "--disable-tool must not fail the run, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    // Each non-empty line is one model-visible tool name (the same guarantee
    // `list_tools_prints_one_name_per_line_and_exits_zero` pins); we re-assert
    // it here so a regression in the output format also flags this test.
    let lines: Vec<&str> = stdout.lines().filter(|line| !line.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "stdout must list at least one tool after --disable-tool; got:\n{stdout}");

    for line in &lines {
        assert!(
            !line.chars().any(char::is_whitespace),
            "every non-empty line must be a single tool name with no whitespace; got {line:?} in:\n{stdout}"
        );
    }

    // The withheld tool is no longer in the model-visible set. The check
    // operates on whole lines (not substrings) so a future tool name that
    // *contains* `coding__bash` would still pass; today there is none, and
    // the list is line-delimited.
    let matches: Vec<&&str> = lines.iter().filter(|line| **line == "coding__bash").collect();
    assert!(
        matches.is_empty(),
        "--disable-tool coding__bash must remove the named tool from the list; stdout:\n{stdout}"
    );

    // Sanity guard: the inline coding server exposes more than just bash, so
    // the withheld run should still see other coding tools. Both
    // `coding__read_file` and `coding__grep` are part of the same built-in
    // server as the withheld tool; asserting they survive makes "withhold X
    // only" observable on a single line.
    assert!(lines.contains(&"coding__read_file"), "withheld run must still expose sibling tools; stdout:\n{stdout}");
    assert!(lines.contains(&"coding__grep"), "withheld run must still expose sibling tools; stdout:\n{stdout}");

    Ok(())
}

/// `--disable-tool NAME` is repeatable (mirrors the upstream clap `Vec<String>`
/// convention). Two withheld names drop exactly those two lines and leave the
/// rest intact. This is the contract a wrapper script can rely on when it has
/// to blanket-disable two tools for one run.
#[test]
fn disable_tool_repeats_drop_each_named_tool_once() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = run_list_tools(&["--disable-tool", "coding__bash", "--disable-tool", "coding__read_file"], dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "two --disable-tool arguments must not fail the run, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    let lines: Vec<&str> = stdout.lines().filter(|line| !line.trim().is_empty()).collect();
    assert!(!lines.contains(&"coding__bash"), "repeatable --disable-tool must drop coding__bash; stdout:\n{stdout}");
    assert!(
        !lines.contains(&"coding__read_file"),
        "repeatable --disable-tool must drop coding__read_file; stdout:\n{stdout}"
    );
    // Coding sibling still present so an over-eager filter doesn't shadow
    // the rest of the set.
    assert!(lines.contains(&"coding__grep"), "withholding bash + read_file must still expose grep; stdout:\n{stdout}");

    Ok(())
}

/// Unknown names are a silent no-op (TASK-25-123: "Unknown name is a silent
/// no-op"; the design rides on `ToolFilter::deny`'s existing semantics).
/// Exiting 0 and leaving the rest of the list intact makes the flag safe to
/// pass from an operator wrapper that wants to apply a script-supplied list.
#[test]
fn disable_tool_unknown_name_is_no_op_and_exits_zero() -> TestResult {
    let dir = tempfile::tempdir()?;

    let output = run_list_tools(&["--disable-tool", "does-not-exist-anywhere"], dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "unknown --disable-tool must not fail the run, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(
        stdout.contains("coding__bash"),
        "unknown --disable-tool must not touch the baseline list; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("coding__read_file"),
        "unknown --disable-tool must not touch the baseline list; stdout:\n{stdout}"
    );

    Ok(())
}
