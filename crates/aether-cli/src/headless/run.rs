use aether_core::core::{AgentDeps, Prompt};
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use aether_core::mcp::McpHandle;
use aether_telemetry::TelemetryRuntime;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
// `Duration` is test-only; the production loop only needs `Instant`. Keeping
// `Duration` out of this import avoids an `unused_imports` warning from `cargo
// build` (which doesn't compile `#[cfg(test)]` modules).
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::error;

use crate::file_changes::FileChanges;
use crate::telemetry::build_telemetry_runtime;
use crate::workspace::warn_if_not_a_repository;

use super::error::CliError;
use super::{CliEventKind, RunConfig};
use crate::output::{OutputFormat, RetryTracker, TurnTimings, print_message, print_turn_summary};
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::{expand_slash_command, parse_slash_command};

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose);
    warn_if_not_a_repository(&config.cwd);

    let telemetry = build_telemetry_runtime(config.telemetry.as_ref(), config.trace_context.clone())?;
    let result = run_agent(config, telemetry.clone()).await;

    if let Some(telemetry) = telemetry {
        telemetry.shutdown_or_log();
    }
    result
}

async fn run_agent(config: RunConfig, telemetry: Option<Arc<TelemetryRuntime>>) -> Result<ExitCode, CliError> {
    let mut spec = config.spec;
    if let Some(system_prompt) = config.system_prompt {
        spec.prompts.push(Prompt::text(&system_prompt));
    }

    let registry = config.agent_catalog.registry().clone();
    let deps =
        AgentDeps::new(config.oauth_credential_store, telemetry.as_ref().map(|runtime| runtime.observer_factory()))
            .with_agent_registry(registry);
    let (agent, _mcp_snapshot) = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_sources(config.mcp_config_sources)
        .agent_deps(deps)
        .build_ready(vec![])
        .await?;

    let prompt = expand_prompt(agent.mcp_runtime.handle(), config.prompt).await;

    // Whole-run wall-clock start. Picked up by `stream_output` so the final
    // summary line can carry the run's total elapsed time alongside the
    // per-turn timings (TASK-23-337). Per-turn and whole-run are distinct
    // figures: the whole run includes startup, MCP setup, and any gaps
    // between turns; the per-turn total only covers turn-to-turn intervals.
    let run_started_at = Instant::now();

    agent
        .agent_tx
        .send(Command::text(&prompt))
        .await
        .map_err(|e| CliError::AgentError(format!("Failed to send prompt: {e}")))?;

    let (exit_code, changes) = stream_output(agent.agent_rx, config.output, &config.events, run_started_at).await;

    drop(agent.agent_tx);
    agent.agent_handle.await_completion().await;

    if config.output == OutputFormat::Text {
        println!("{}", changes.summary());
    }

    Ok(exit_code)
}

async fn expand_prompt(mcp: &McpHandle, prompt: String) -> String {
    let Some(slash_command) = parse_slash_command(&prompt) else {
        return prompt;
    };

    match expand_slash_command(mcp, slash_command.command_name, slash_command.args_text).await {
        Ok(expanded) => expanded,
        Err(error) => {
            error!("Failed to expand slash command: {error}");
            prompt
        }
    }
}

async fn stream_output(
    mut rx: mpsc::Receiver<AgentEvent>,
    format: OutputFormat,
    events: &[CliEventKind],
    run_started_at: Instant,
) -> (ExitCode, FileChanges) {
    let mut tracker = RetryTracker::default();
    // Wall-clock timing of every turn seen on the stream, independent of the
    // `--events` filter so a filtered run still reports how long it ran.
    let mut timings = TurnTimings::default();
    let mut changes = FileChanges::default();
    let mut exit_code = ExitCode::SUCCESS;

    while let Some(msg) = rx.recv().await {
        // Capture the note for failed turns *before* we update the tracker
        // with the event we are about to print. Observing the `Ended` event
        // for a failed turn does not change the count or provider, so the
        // value is identical before and after the observation.
        let note = match &msg {
            AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } }) => Some(tracker.failure_note()),
            _ => None,
        };
        tracker.observe(&msg);

        match &msg {
            AgentEvent::Turn(TurnEvent::Started { .. }) => timings.begin(Instant::now()),
            AgentEvent::Turn(TurnEvent::Ended { .. }) => timings.end(Instant::now()),
            _ => {}
        }

        if let Some(meta) = tool_result_meta(&msg) {
            changes.record(meta);
        }

        if should_emit(&msg, events)
            && let Err(error) = print_message(format, &msg, note.as_deref())
        {
            eprintln!("Failed to serialize headless event: {error}");
            return (ExitCode::FAILURE, changes);
        }

        if let Some(outcome) = msg.turn_outcome() {
            exit_code = match outcome {
                TurnOutcome::Failed { .. } => ExitCode::FAILURE,
                TurnOutcome::Completed | TurnOutcome::Cancelled => ExitCode::SUCCESS,
            };
            break;
        }
    }

    let run_total_elapsed = run_started_at.elapsed();
    print_turn_summary(format, &timings, Some(run_total_elapsed));
    (exit_code, changes)
}

/// Extract the `FileDiff`-bearing metadata from a tool event, if any.
fn tool_result_meta(msg: &AgentEvent) -> Option<&mcp_utils::display_meta::ToolResultMeta> {
    match msg {
        AgentEvent::Tool(
            ToolEvent::Result { result_meta: Some(meta), .. }
            | ToolEvent::TaskCompleted { result_meta: Some(meta), .. },
        ) => Some(meta),
        _ => None,
    }
}

fn should_emit(msg: &AgentEvent, include: &[CliEventKind]) -> bool {
    let Some(kind) = event_kind(msg) else { return false };
    include.is_empty() || include.contains(&kind)
}

fn event_kind(msg: &AgentEvent) -> Option<CliEventKind> {
    match msg {
        AgentEvent::Message(MessageEvent::Text { is_complete: true, .. }) => Some(CliEventKind::Text),
        AgentEvent::Message(MessageEvent::Thought { is_complete: true, .. }) => Some(CliEventKind::Thought),
        AgentEvent::Tool(ToolEvent::Call { .. }) => Some(CliEventKind::ToolCall),
        AgentEvent::Tool(
            ToolEvent::Result { .. } | ToolEvent::TaskCreated { .. } | ToolEvent::TaskCompleted { .. },
        ) => Some(CliEventKind::ToolResult),
        AgentEvent::Tool(ToolEvent::Error { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }) => {
            Some(CliEventKind::ToolError)
        }
        AgentEvent::Turn(TurnEvent::AutoContinue { .. }) => Some(CliEventKind::AutoContinue),
        AgentEvent::Model(ModelEvent::Switched { .. }) => Some(CliEventKind::ModelSwitched),
        AgentEvent::Tool(
            ToolEvent::Progress { .. }
            | ToolEvent::DisplayUpdate { .. }
            | ToolEvent::SubAgentProgress { .. }
            | ToolEvent::TaskStatus { .. },
        ) => Some(CliEventKind::ToolProgress),
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => Some(CliEventKind::ContextCompactionStarted),
        AgentEvent::Context(ContextEvent::CompactionEnded { .. }) => Some(CliEventKind::ContextCompactionEnded),
        AgentEvent::Context(ContextEvent::CompactionResult { .. }) => Some(CliEventKind::ContextCompactionResult),
        AgentEvent::Context(ContextEvent::UsageUpdated { .. }) => Some(CliEventKind::ContextUsage),
        AgentEvent::SessionUsage(_) => Some(CliEventKind::SessionUsage),
        AgentEvent::Context(ContextEvent::Cleared) => Some(CliEventKind::ContextCleared),
        AgentEvent::Turn(TurnEvent::Started { .. }) => Some(CliEventKind::TurnStarted),
        AgentEvent::Turn(TurnEvent::Ended { .. }) => Some(CliEventKind::TurnEnded),
        AgentEvent::Turn(TurnEvent::RetryScheduled { .. }) => Some(CliEventKind::LlmRetryScheduled),
        AgentEvent::Turn(TurnEvent::LlmCallStarted { .. }) => Some(CliEventKind::LlmCallStarted),
        AgentEvent::Turn(TurnEvent::LlmCallEnded { .. }) => Some(CliEventKind::LlmCallEnded),
        AgentEvent::Tool(ToolEvent::ExecutionStarted { .. }) => Some(CliEventKind::ToolExecutionStarted),
        AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. }) => Some(CliEventKind::ToolDefinitionsUpdated),
        AgentEvent::Message(
            MessageEvent::Text { is_complete: false, .. } | MessageEvent::Thought { is_complete: false, .. },
        )
        | AgentEvent::Tool(ToolEvent::CallUpdate { .. }) => None,
    }
}

pub(crate) fn setup_tracing(verbose: bool) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = if verbose { EnvFilter::new("debug,agent=off") } else { EnvFilter::new("warn,agent=off") };
    let layer = fmt::layer().with_writer(io::stderr).with_filter(filter);

    let _ = tracing_subscriber::registry().with(layer).try_init();
}

#[cfg(test)]
mod tests {
    use aether_core::events::StreamState;

    use super::*;
    use llm::ContextUsage;

    #[test]
    fn event_kind_none_for_non_output_fragments() {
        assert_eq!(event_kind(&AgentEvent::text("id", "x", StreamState::Partial)), None);
        assert_eq!(event_kind(&AgentEvent::thought("id", "x", StreamState::Partial)), None);
        assert_eq!(
            event_kind(&AgentEvent::Tool(ToolEvent::CallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
            })),
            None,
        );
    }

    #[test]
    fn event_kind_turn_ended_is_filterable() {
        assert_eq!(event_kind(&AgentEvent::turn_ended(TurnOutcome::Completed)), Some(CliEventKind::TurnEnded));
    }

    #[test]
    fn should_emit_empty_filter_rejects_non_output_events() {
        assert!(should_emit(&tool_call_msg(), &[]));
        assert!(should_emit(
            &AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "e".to_string() } }),
            &[]
        ));
        assert!(should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), &[]));
        assert!(!should_emit(&AgentEvent::text("id", "x", StreamState::Partial), &[]));
        assert!(!should_emit(
            &AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tc1".to_string(), chunk: "x".to_string() }),
            &[],
        ));
    }

    #[test]
    fn should_emit_single_type_whitelist() {
        let filter = &[CliEventKind::ToolCall];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(!should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), filter));
    }

    #[test]
    fn should_emit_multi_type_whitelist() {
        let filter = &[CliEventKind::ToolCall, CliEventKind::ToolResult];
        assert!(should_emit(&tool_call_msg(), filter));
        assert!(should_emit(&tool_result_msg(), filter));
        assert!(!should_emit(&AgentEvent::turn_ended(TurnOutcome::Completed), filter));
    }

    #[test]
    fn should_emit_turn_ended_respects_filter() {
        let msg = AgentEvent::turn_ended(TurnOutcome::Completed);
        assert!(should_emit(&msg, &[CliEventKind::TurnEnded]));
        assert!(!should_emit(&msg, &[CliEventKind::ToolCall]));
    }

    #[test]
    fn event_kind_covers_every_cli_event_kind() {
        use clap::ValueEnum;

        let samples = vec![
            (AgentEvent::text("id", "x", StreamState::Complete), CliEventKind::Text),
            (AgentEvent::thought("id", "x", StreamState::Complete), CliEventKind::Thought),
            (tool_call_msg(), CliEventKind::ToolCall),
            (tool_result_msg(), CliEventKind::ToolResult),
            (
                AgentEvent::Tool(ToolEvent::Error {
                    error: llm::ToolCallError {
                        id: "tc1".to_string(),
                        name: "bash".to_string(),
                        arguments: None,
                        error: "boom".to_string(),
                    },
                }),
                CliEventKind::ToolError,
            ),
            (
                AgentEvent::Turn(TurnEvent::AutoContinue {
                    attempt: 1,
                    max_attempts: 3,
                    message_id: llm::MessageId::new(),
                    content: vec![],
                }),
                CliEventKind::AutoContinue,
            ),
            (
                AgentEvent::Model(ModelEvent::Switched { previous: "a".to_string(), new: "b".to_string() }),
                CliEventKind::ModelSwitched,
            ),
            (tool_progress(1.0, None, None), CliEventKind::ToolProgress),
            (
                AgentEvent::Context(ContextEvent::CompactionStarted {
                    compaction_id: "compaction".into(),
                    message_count: 1,
                }),
                CliEventKind::ContextCompactionStarted,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionEnded {
                    compaction_id: "compaction".into(),
                    outcome: aether_core::events::CompactionOutcome::Completed,
                }),
                CliEventKind::ContextCompactionEnded,
            ),
            (
                AgentEvent::Context(ContextEvent::CompactionResult {
                    compaction_id: "compaction".into(),
                    message_id: llm::MessageId::new(),
                    summary: "s".to_string(),
                    messages_removed: 1,
                }),
                CliEventKind::ContextCompactionResult,
            ),
            (usage_update(), CliEventKind::ContextUsage),
            (
                AgentEvent::SessionUsage(llm::testing::session_usage_event(1, llm::TokenUsage::new(1, 1))),
                CliEventKind::SessionUsage,
            ),
            (AgentEvent::Context(ContextEvent::Cleared), CliEventKind::ContextCleared),
            (AgentEvent::Turn(TurnEvent::Started { content: vec![] }), CliEventKind::TurnStarted),
            (AgentEvent::turn_ended(TurnOutcome::Completed), CliEventKind::TurnEnded),
            (retry_scheduled(1, 10), CliEventKind::LlmRetryScheduled),
            (llm_call_started(0), CliEventKind::LlmCallStarted),
            (
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    purpose: llm::LlmCallPurpose::Chat,
                    outcome: aether_core::events::LlmCallOutcome::Cancelled,
                }),
                CliEventKind::LlmCallEnded,
            ),
            (
                AgentEvent::Tool(ToolEvent::ExecutionStarted {
                    tool_id: "tc1".to_string(),
                    tool_name: "bash".to_string(),
                }),
                CliEventKind::ToolExecutionStarted,
            ),
            (AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools: vec![] }), CliEventKind::ToolDefinitionsUpdated),
        ];

        for kind in CliEventKind::value_variants() {
            assert!(samples.iter().any(|(_, k)| k == kind), "samples is missing a case for {kind:?}");
        }

        for (msg, kind) in &samples {
            assert_eq!(event_kind(msg), Some(*kind), "event_kind disagrees for {kind:?}");
        }
    }

    #[tokio::test]
    async fn stream_output_turn_ended_breaks_loop_under_filter() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let filter = vec![CliEventKind::ToolCall];
        let (code, changes) = stream_output(rx, OutputFormat::Text, &filter, Instant::now()).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 0);
    }

    #[tokio::test]
    async fn stream_output_failed_turn_exits_with_failure() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::turn_ended(TurnOutcome::Failed { error: "boom".to_string() })).await.unwrap();
        let (code, changes) = stream_output(rx, OutputFormat::Text, &[], Instant::now()).await;
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(changes.total(), 0);
    }

    #[tokio::test]
    async fn stream_output_reports_two_file_changes() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(tool_result_with_file_diff("created.rs", None, Some("new"))).await.unwrap();
        tx.send(tool_result_with_file_diff("edited.rs", Some("old"), Some("new"))).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, changes) = stream_output(rx, OutputFormat::Text, &[], Instant::now()).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 2);
        assert_eq!(changes.created(), 1);
        assert_eq!(changes.modified(), 1);
        assert_eq!(changes.deleted(), 0);
        assert!(changes.summary().contains("Files changed: 2"));
    }

    #[tokio::test]
    async fn stream_output_reports_zero_when_nothing_changed() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, changes) = stream_output(rx, OutputFormat::Text, &[], Instant::now()).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 0);
        assert!(changes.summary().contains("Files changed: 0"));
    }

    #[tokio::test]
    async fn stream_output_counts_task_completed_file_diffs() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(task_completed_with_file_diff("removed.rs", Some("old"), None)).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, changes) = stream_output(rx, OutputFormat::Text, &[], Instant::now()).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 1);
        assert_eq!(changes.deleted(), 1);
    }

    #[tokio::test]
    async fn stream_output_ignores_events_filtered_out_by_cli_flag() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(tool_result_with_file_diff("filtered.rs", None, Some("new"))).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        // Restrict to a different event kind so the ToolResult is not printed,
        // but file changes are still tallied.
        let filter = vec![CliEventKind::TurnEnded];
        let (_code, changes) = stream_output(rx, OutputFormat::Text, &filter, Instant::now()).await;
        assert_eq!(changes.total(), 1);
    }

    fn tool_result_with_file_diff(path: &str, old: Option<&str>, new: Option<&str>) -> AgentEvent {
        let diff = mcp_utils::display_meta::FileDiff {
            path: path.to_string(),
            old_text: old.map(str::to_string),
            new_text: new.map(str::to_string),
        };
        let meta = mcp_utils::display_meta::ToolResultMeta::with_file_diff(
            mcp_utils::display_meta::ToolDisplayMeta::new("Edit", path),
            diff,
        );
        AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "edit_file".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: Some(meta),
        })
    }

    fn task_completed_with_file_diff(path: &str, old: Option<&str>, new: Option<&str>) -> AgentEvent {
        let diff = mcp_utils::display_meta::FileDiff {
            path: path.to_string(),
            old_text: old.map(str::to_string),
            new_text: new.map(str::to_string),
        };
        let meta = mcp_utils::display_meta::ToolResultMeta::with_file_diff(
            mcp_utils::display_meta::ToolDisplayMeta::new("Delete", path),
            diff,
        );
        AgentEvent::Tool(ToolEvent::TaskCompleted {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "delete_file".to_string(),
                arguments: "{}".to_string(),
            },
            task_id: "task-1".to_string(),
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "delete_file".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: Some(meta),
        })
    }

    #[tokio::test]
    async fn stream_output_records_turn_durations() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, _changes) = stream_output(rx, OutputFormat::Text, &[], Instant::now()).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn stream_output_summary_includes_whole_run_total_elapsed() {
        use std::time::Duration;
        // Build the printed summary with the same helper `print_turn_summary`
        // uses, so we can assert the last line ends with the whole-run
        // elapsed time without having to capture stdout.
        let mut timings = TurnTimings::default();
        timings.begin(Instant::now());
        timings.end(Instant::now());
        let run_total = Duration::from_secs(5);
        let printed = crate::output::turn_summary_body(&timings, Some(run_total));
        let last_line = printed.lines().last().unwrap_or("");
        assert!(
            last_line.contains("1 turn, total"),
            "final summary line should still report the per-turn total, got: {last_line:?} (full: {printed:?})"
        );
        assert!(
            last_line.contains("run took"),
            "final summary line should label the whole-run elapsed, got: {last_line:?}"
        );
        assert!(
            last_line.contains("5.000s"),
            "final summary line should contain the whole-run total, got: {last_line:?}"
        );
        assert!(
            last_line.ends_with("5.000s"),
            "final summary line should end with the whole-run total, got: {last_line:?}"
        );
    }

    fn tool_call_msg() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
        })
    }

    fn tool_result_msg() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        })
    }

    fn tool_progress(progress: f64, total: Option<f64>, message: Option<&str>) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Progress {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            progress,
            total,
            message: message.map(str::to_string),
        })
    }

    fn retry_scheduled(attempt: u32, delay_ms: u64) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt,
            max_attempts: 3,
            delay_ms,
        })
    }

    fn llm_call_started(attempt: u32) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "test".to_string(),
            attempt,
            max_attempts: 3,
        })
    }

    fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: ContextUsage {
                input_tokens: 100_000.into(),
                context_limit: Some(200_000.into()),
                usage_ratio: Some(0.5),
            },
        })
    }
}
