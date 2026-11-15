//! The shell-command ceiling the `run` settings block configures.
//!
//! `RunSettings::shell_command_timeout` is the public seam every runtime reads:
//! the CLI headless runner resolves it and hands it to the MCP runtime, which
//! bounds every shell command the `bash` tool of the built-in `coding` server
//! starts.

use std::time::Duration;

use aether_project::RunSettings;

fn run_settings(seconds: serde_json::Value) -> RunSettings {
    serde_json::from_value(serde_json::json!({ "shellCommandTimeoutSeconds": seconds }))
        .expect("the run settings parse")
}

#[test]
fn the_shell_command_timeout_is_read_from_the_run_settings() {
    let settings = run_settings(serde_json::json!(30));
    assert_eq!(settings.shell_command_timeout(), Some(Duration::from_secs(30)));
}

#[test]
fn an_unset_or_zero_shell_command_timeout_leaves_commands_unbounded() {
    assert_eq!(run_settings(serde_json::Value::Null).shell_command_timeout(), None);
    assert_eq!(RunSettings::default().shell_command_timeout(), None);
    assert_eq!(run_settings(serde_json::json!(0)).shell_command_timeout(), None);
}
