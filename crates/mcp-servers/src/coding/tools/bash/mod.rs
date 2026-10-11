use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::env::current_exe;
use std::os::unix::process::CommandExt;
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

use crate::coding::error::BashError;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, truncate};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BashInput {
    /// The command to execute
    pub command: String,
    /// Optional timeout in milliseconds (max 600000)
    pub timeout: Option<u64>,
    /// Clear, concise description of what this command does in 5-10 words
    pub description: Option<String>,
    /// Set to true to run this command in the background
    #[serde(alias = "run_in_background")]
    pub run_in_background: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BashOutput {
    /// Combined stdout and stderr output
    pub output: String,
    /// Exit code of the command
    pub exit_code: i32,
    /// Whether the command was killed due to timeout
    pub killed: bool,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Immutable environment overrides inherited by Bash commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BashEnvironment {
    vars: BTreeMap<String, String>,
    /// Upper bound on a single command's wall time, applied in addition to
    /// whatever deadline the caller passes per call. Set by
    /// [`with_command_timeout`](Self::with_command_timeout).
    command_timeout: Option<Duration>,
}

impl BashEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(name.into(), value.into());
        self
    }

    /// Set an environment-level ceiling on a single command's wall time. A
    /// call that supplies its own deadline is bounded by the smaller of the
    /// two; a call that does not is bounded by this one alone.
    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = Some(timeout);
        self
    }

    /// Read the environment-level ceiling set via
    /// [`with_command_timeout`](Self::with_command_timeout).
    pub fn command_timeout(&self) -> Option<Duration> {
        self.command_timeout
    }

    pub fn with_path_prepend(self, directory: impl AsRef<Path>) -> Self {
        let directory = directory.as_ref().to_string_lossy().into_owned();
        let current = std::env::var("PATH").unwrap_or_default();
        let mut entries = vec![directory.clone()];
        entries.extend(current.split(':').filter(|entry| *entry != directory).map(str::to_owned));
        self.with_var("PATH", entries.join(":"))
    }

    pub fn with_current_exe_dir_on_path(self) -> Self {
        let Some(directory) = current_exe().ok().and_then(|path| path.parent().map(Path::to_path_buf)) else {
            return self;
        };
        self.with_path_prepend(directory)
    }

    pub fn vars(&self) -> &BTreeMap<String, String> {
        &self.vars
    }
}

pub fn validate_args(args: &BashInput) -> Result<(), BashError> {
    if args.command.trim() == "rm" {
        return Err(BashError::Forbidden("Deleting files with a bare 'rm' is not allowed".to_string()));
    }

    if args.timeout.is_some_and(|timeout_ms| timeout_ms > 600_000) {
        return Err(BashError::TimeoutTooLarge);
    }

    Ok(())
}

/// Runs `args.command` to completion. The effective deadline is the smaller
/// of `args.timeout` (caller-supplied, in milliseconds) and the environment's
/// [`BashEnvironment::command_timeout`]; either being `None` falls back to
/// the one that is set, and both `None` means no timeout.
pub async fn execute_command(
    args: BashInput,
    cwd: Option<&Path>,
    environment: &BashEnvironment,
) -> Result<BashOutput, BashError> {
    validate_args(&args)?;

    let call_timeout = args.timeout.map(Duration::from_millis);
    let timeout = match (call_timeout, environment.command_timeout()) {
        (Some(call), Some(env)) => Some(call.min(env)),
        (call, env) => call.or(env),
    };

    let mut command = Command::new("bash");
    command.arg("-c").arg(&args.command);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.envs(environment.vars());
    command.as_std_mut().process_group(0);

    let child = command
        .spawn()
        .map_err(|error| BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() })?;
    let mut process_group = ProcessGroupGuard::new(child.id());
    let mut child_task = tokio::spawn(child.wait_with_output());

    let output = match timeout {
        Some(timeout) => {
            tokio::select! {
                result = &mut child_task => result,
                () = tokio::time::sleep(timeout) => {
                    process_group.kill();
                    let _ = tokio::time::timeout(Duration::from_secs(5), &mut child_task).await;
                    let display_meta = ToolDisplayMeta::new(
                        "Ran",
                        format!("{} (exit -1, timed out)", truncate(&args.command, 40)),
                    );
                    return Ok(BashOutput {
                        output: format!("Command timed out after {}ms", timeout.as_millis()),
                        exit_code: -1,
                        killed: true,
                        meta: Some(display_meta.into()),
                    });
                }
            }
        }
        None => child_task.await,
    };
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Err(BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() });
        }
    };
    let output =
        output.map_err(|error| BashError::SpawnFailed { command: args.command.clone(), reason: error.to_string() })?;
    process_group.disarm();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);
    let display_meta = ToolDisplayMeta::new("Ran", format!("{} (exit {exit_code})", truncate(&args.command, 40)));

    Ok(BashOutput { output: format!("{stdout}{stderr}"), exit_code, killed: false, meta: Some(display_meta.into()) })
}

/// Kills an entire process group on drop unless disarmed after a clean exit.
///
/// `Child::kill`/`kill_on_drop` signal only the direct child (`bash`), leaving
/// its spawned commands orphaned; killing the process group covers the tree.
/// Disarming matters because the kernel may recycle the group id once reaped.
struct ProcessGroupGuard {
    pgid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pgid: Option<u32>) -> Self {
        Self { pgid }
    }

    fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            unsafe {
                libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn environment_overrides_are_passed_to_bash() {
        let environment = BashEnvironment::new().with_var("AETHER_TEST_VALUE", "present");
        let output = execute_command(
            BashInput { command: "printf '%s' \"$AETHER_TEST_VALUE\"".into(), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();
        assert_eq!(output.output, "present");
    }

    #[tokio::test]
    async fn command_timeout_kills_a_long_running_command() {
        let environment = BashEnvironment::new().with_command_timeout(Duration::from_millis(200));
        let started = Instant::now();
        let output = execute_command(
            BashInput { command: "sleep 5".into(), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert!(output.killed, "long-running command should be marked killed");
        assert_eq!(output.exit_code, -1);
        assert!(
            output.output.contains("timed out"),
            "timed-out marker should appear in output, got {:?}",
            output.output
        );
        assert!(
            output.meta.as_ref().map(|meta| format!("{meta:?}").contains("timed out")).unwrap_or(false),
            "display meta should mention the timed-out status"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "command should die well under 5s, elapsed = {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn nonzero_exit_is_not_marked_killed() {
        let environment = BashEnvironment::new().with_command_timeout(Duration::from_millis(200));
        let output = execute_command(
            BashInput { command: "exit 3".into(), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();

        assert!(!output.killed, "a non-zero exit must not be flagged as killed");
        assert_eq!(output.exit_code, 3);
        assert!(!output.output.contains("timed out"));
    }

    #[tokio::test]
    async fn environment_timeout_caps_a_longer_call_timeout() {
        let environment = BashEnvironment::new().with_command_timeout(Duration::from_millis(200));
        let started = Instant::now();
        let output = execute_command(
            BashInput { command: "sleep 5".into(), timeout: Some(30_000), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert!(output.killed, "environment timeout must override a longer call timeout");
        assert_eq!(output.exit_code, -1);
        assert!(output.output.contains("timed out"));
        assert!(
            elapsed < Duration::from_secs(2),
            "command should die near the 200ms environment timeout, elapsed = {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn call_timeout_caps_a_longer_environment_timeout() {
        let environment = BashEnvironment::new().with_command_timeout(Duration::from_secs(10));
        let started = Instant::now();
        let output = execute_command(
            BashInput { command: "sleep 5".into(), timeout: Some(150), ..Default::default() },
            None,
            &environment,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert!(output.killed, "a shorter call timeout must still bound the command");
        assert_eq!(output.exit_code, -1);
        assert!(output.output.contains("timed out"));
        assert!(
            elapsed < Duration::from_secs(2),
            "command should die near the 150ms call timeout, elapsed = {elapsed:?}"
        );
    }
}
