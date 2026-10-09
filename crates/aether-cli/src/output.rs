use aether_core::events::{
    AgentEvent, CompactionOutcome, ContextEvent, LlmCallOutcome, MessageEvent, ModelEvent, ToolEvent, TurnEvent,
    TurnOutcome,
};
use llm::LlmCallPurpose;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Pretty,
    Json,
}

/// Accumulates the wall-clock duration of each turn observed on the agent
/// event stream. Callers feed `Instant::now()` to `begin` for every
/// `TurnEvent::Started` and to `end` for every `TurnEvent::Ended`; each matched
/// pair contributes one `Duration` to `self.durations`.
#[derive(Debug, Default, Clone)]
pub(crate) struct TurnTimings {
    durations: Vec<Duration>,
    pending_start: Option<Instant>,
}

impl TurnTimings {
    /// Record that a turn started at `now`. Any prior unmatched start is
    /// overwritten so a missing `Ended` does not leave dangling state.
    pub(crate) fn begin(&mut self, now: Instant) {
        self.pending_start = Some(now);
    }

    /// Record that a turn ended at `now`. Pushes the elapsed duration and
    /// clears the pending start. If no start is pending the call is a no-op
    /// so an unmatched `Ended` cannot poison later measurements.
    pub(crate) fn end(&mut self, now: Instant) {
        if let Some(start) = self.pending_start.take() {
            self.durations.push(now.saturating_duration_since(start));
        }
    }

    /// True when no turn duration has been recorded.
    pub(crate) fn is_empty(&self) -> bool {
        self.durations.is_empty()
    }

    /// Number of turn durations recorded so far. Test-only; not used in the
    /// production code path, so it is gated on `cfg(test)` to keep the lib
    /// build free of a dead-code warning.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.durations.len()
    }

    /// Sum of every recorded turn duration.
    pub(crate) fn total(&self) -> Duration {
        self.durations.iter().copied().sum()
    }

    /// Multi-line summary: one `Turn N took <duration>` line per turn,
    /// followed by a total line.
    pub(crate) fn summary(&self) -> String {
        use std::fmt::Write as _;
        let mut lines = String::new();
        for (index, duration) in self.durations.iter().enumerate() {
            let turn = index + 1;
            let _ = writeln!(lines, "Turn {turn} took {}", format_duration(*duration));
        }
        let count = self.durations.len();
        let total = format_duration(self.total());
        if count == 1 {
            let _ = write!(lines, "1 turn, total {total}");
        } else {
            let _ = write!(lines, "{count} turns, total {total}");
        }
        lines
    }
}

/// Render a `Duration` as seconds with millisecond precision, e.g. `1.500s`.
pub(crate) fn format_duration(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

/// Build the end-of-run turn summary. When `run_total` is `Some`, its formatted
/// duration is appended to the last line so the summary carries the whole-run
/// elapsed time (TASK-23-395) alongside the per-turn total (TASK-23-337).
pub(crate) fn turn_summary_body(timings: &TurnTimings, run_total: Option<Duration>) -> String {
    let mut body = timings.summary();
    if let Some(total) = run_total {
        use std::fmt::Write as _;
        let _ = write!(body, ", run took {}", format_duration(total));
    }
    body
}

/// Print the end-of-run turn summary. No-op when format is not `Text`.
pub(crate) fn print_turn_summary(format: OutputFormat, timings: &TurnTimings, run_total: Option<Duration>) {
    if !matches!(format, OutputFormat::Text) || timings.is_empty() {
        return;
    }
    println!("{}", turn_summary_body(timings, run_total));
}

/// Print a single agent event using the chosen format.
///
/// `retry_note` is optional context appended to the human-readable failure line
/// (e.g. the number of LLM retries observed before the turn ended). Machine
/// formats (`Pretty`, `Json`) ignore it because the underlying retry events are
/// already in the stream.
pub(crate) fn print_message(
    format: OutputFormat,
    message: &AgentEvent,
    retry_note: Option<&str>,
) -> Result<(), serde_json::Error> {
    match format {
        OutputFormat::Text => {
            if let Some(text) = format_text(message, retry_note) {
                if matches!(message, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } })) {
                    eprintln!("{text}");
                } else {
                    println!("{text}");
                }
            }
        }
        OutputFormat::Pretty => println!("{}", serde_json::to_string_pretty(message)?),
        OutputFormat::Json => println!("{}", serde_json::to_string(message)?),
    }

    Ok(())
}

fn format_text(message: &AgentEvent, retry_note: Option<&str>) -> Option<String> {
    match message {
        AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. }) => Some(chunk.clone()),
        AgentEvent::Message(MessageEvent::Thought { chunk, is_complete: true, .. }) => {
            Some(format!("Thought: {chunk}"))
        }
        AgentEvent::Tool(ToolEvent::Call { request, .. }) => {
            Some(format!("Tool call: {}({})", request.name, request.arguments))
        }
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => {
            Some(format!("Tool result [{}]: {}", result.name, result.result))
        }
        AgentEvent::Tool(ToolEvent::Error { error, .. }) => {
            Some(format!("Tool error [{}]: {}", error.name, error.error))
        }
        AgentEvent::Tool(ToolEvent::Refused { request, reason }) => {
            Some(format!("Tool refused [{}]: {reason}", request.name))
        }
        AgentEvent::Tool(ToolEvent::TaskStatus { request, task_id, status, status_message }) => Some(format!(
            "Task status [{}]: {} {}{}",
            request.name,
            task_id,
            status,
            status_message.as_deref().map(|message| format!(" - {message}")).unwrap_or_default()
        )),
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            Some(format!("Tool deferred [{}]: task {}", request.name, task_id))
        }
        AgentEvent::Tool(ToolEvent::TaskCompleted { request, task_id, result, .. }) => {
            Some(format!("Background task completed [{}]: {}: {}", request.name, task_id, result.result))
        }
        AgentEvent::Tool(ToolEvent::TaskFailed { request, task_id, error, .. }) => {
            Some(format!("Background task failed [{}]: {}: {}", request.name, task_id, error.error))
        }
        AgentEvent::Tool(ToolEvent::TaskCancelled { request, task_id, .. }) => {
            Some(format!("Background task cancelled [{}]: {task_id}", request.name))
        }
        AgentEvent::Turn(TurnEvent::Ended { outcome }) => Some(match outcome {
            TurnOutcome::Completed => "Done".to_string(),
            TurnOutcome::Cancelled => "Cancelled".to_string(),
            TurnOutcome::Failed { error } => match retry_note {
                Some(note) => format!("Error: {error} ({note})"),
                None => format!("Error: {error}"),
            },
            TurnOutcome::MaxTurnsReached { max_turns } => format!("Reached turn cap ({max_turns}); ending run"),
        }),
        AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts, .. }) => {
            Some(format!("Continuing ({attempt}/{max_attempts})..."))
        }
        AgentEvent::Turn(event @ TurnEvent::RetryScheduled { .. }) => event
            .retry_info()
            .map(|retry| format!("Retrying ({}/{}) in {}ms", retry.attempt, retry.max_attempts, retry.delay_ms)),
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            outcome: LlmCallOutcome::Failed { error, will_retry: true, .. },
            ..
        }) => Some(format!("LLM call failed (will retry): {error}")),
        AgentEvent::Model(ModelEvent::Switched { previous, new }) => {
            Some(format!("Model switched: {previous} -> {new}"))
        }
        AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message }) => {
            let bar = match total {
                Some(total) => format!("{progress}/{total}"),
                None => format!("{progress}"),
            };
            let suffix = message.as_deref().map(|message| format!(" - {message}")).unwrap_or_default();
            Some(format!("Tool progress [{}]: {bar}{suffix}", request.name))
        }
        AgentEvent::Tool(ToolEvent::DisplayUpdate { request, meta }) => {
            Some(format!("Tool progress [{}]: {} - {}", request.name, meta.display.title, meta.display.value))
        }
        AgentEvent::Tool(ToolEvent::SubAgentProgress { payload, .. }) => match &payload.event {
            AgentEvent::SessionUsage(_) => None,
            event => format_text(event, None)
                .map(|text| format!("Sub-agent {} [{}]: {text}", payload.agent_name, payload.task_id)),
        },
        AgentEvent::Context(ContextEvent::CompactionStarted { message_count, .. }) => {
            Some(format!("Context compaction started ({message_count} messages)"))
        }
        AgentEvent::Context(ContextEvent::CompactionEnded { outcome, .. }) => Some(match outcome {
            CompactionOutcome::Completed => "Context compaction completed".to_string(),
            CompactionOutcome::Failed { error } => format!("Context compaction failed: {error}"),
            CompactionOutcome::Cancelled => "Context compaction cancelled".to_string(),
        }),
        AgentEvent::Context(ContextEvent::CompactionResult { summary, messages_removed, .. }) => {
            Some(format!("Context compacted: {messages_removed} messages removed. {summary}"))
        }
        AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(format_context_usage(usage)),
        AgentEvent::Context(ContextEvent::Cleared) => Some("Context cleared".to_string()),
        AgentEvent::SessionUsage(usage) => Some(format_session_usage(usage)),
        AgentEvent::Turn(
            TurnEvent::Started { .. }
            | TurnEvent::LlmCallStarted { .. }
            | TurnEvent::LlmCallEnded {
                outcome:
                    LlmCallOutcome::Completed { .. }
                    | LlmCallOutcome::Cancelled
                    | LlmCallOutcome::Failed { will_retry: false, .. },
                ..
            },
        )
        | AgentEvent::Tool(
            ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. } | ToolEvent::CallUpdate { .. },
        )
        | AgentEvent::Message(MessageEvent::Text { .. } | MessageEvent::Thought { .. }) => None,
    }
}

fn format_context_usage(usage: &llm::ContextUsage) -> String {
    match (usage.context_limit, usage.usage_ratio) {
        (Some(limit), Some(ratio)) => {
            format!("Context: {} / {limit} tokens ({:.1}%)", usage.input_tokens, ratio * 100.0)
        }
        _ => format!("Context: {} tokens", usage.input_tokens),
    }
}

/// Tracks how many provider retries the CLI saw before the turn ended.
///
/// The headless event loop feeds [`AgentEvent`]s through [`RetryTracker::observe`].
/// Only `LlmCallPurpose::Chat` retries are counted; compaction retries are
/// excluded because the task scope ("retries it made for a provider error") is
/// the user-facing chat path. The last observed `LlmCallStarted` display name
/// is taken as "the failing provider".
#[derive(Default, Debug)]
pub(crate) struct RetryTracker {
    retries: u32,
    provider: Option<String>,
}

impl RetryTracker {
    /// Record one event. Each `RetryScheduled` chat event increments the
    /// counter; the `Started` event resets it because a new turn begins.
    pub(crate) fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Turn(TurnEvent::Started { .. }) => {
                self.retries = 0;
                self.provider = None;
            }
            AgentEvent::Turn(TurnEvent::LlmCallStarted { display_name, purpose, .. }) => {
                if matches!(purpose, LlmCallPurpose::Chat) {
                    self.provider = Some(display_name.clone());
                }
            }
            AgentEvent::Turn(TurnEvent::RetryScheduled { purpose: LlmCallPurpose::Chat, .. }) => {
                self.retries = self.retries.saturating_add(1);
            }
            _ => {}
        }
    }

    /// Render the note appended to the CLI failure line.
    pub(crate) fn failure_note(&self) -> String {
        match &self.provider {
            Some(provider) => format!("after {} retries on {provider}", self.retries),
            None => format!("after {} retries", self.retries),
        }
    }
}

/// Summary surfaced to integration tests so they can assert on the retry
/// count and the provider display name the CLI saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrySummary {
    pub retries: u32,
    pub failed: bool,
    pub provider: Option<String>,
}

/// Reduce a recorded event stream to the retry summary the CLI would render.
pub fn summarize_retries(events: &[AgentEvent]) -> RetrySummary {
    let mut tracker = RetryTracker::default();
    let mut failed = false;
    for event in events {
        tracker.observe(event);
        if let AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } }) = event {
            failed = true;
        }
    }
    RetrySummary { retries: tracker.retries, failed, provider: tracker.provider }
}

fn format_session_usage(usage: &llm::SessionUsageEvent) -> String {
    let call_cost =
        usage.estimated_cost.map_or_else(|| "unknown".to_string(), |cost| format!("${:.6}", cost.total_usd));
    let totals = &usage.totals;
    let cumulative_cost = if totals.is_fully_priced() {
        format!("estimated total: ${:.6}", totals.estimated_usd)
    } else {
        format!("known subtotal: ${:.6}, {} unpriced calls", totals.estimated_usd, totals.unpriced_calls)
    };
    format!(
        "Session usage #{} [{}]: {} in, {} out (call cost: {}, cumulative: {} tokens, {})",
        usage.sequence,
        usage.source.agent_name,
        usage.tokens.input_tokens,
        usage.tokens.output_tokens,
        call_cost,
        totals.tokens.total_tokens(),
        cumulative_cost,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::StreamState;

    #[test]
    fn format_text_formats_complete_text() {
        assert_eq!(
            format_text(&AgentEvent::text("id", "hello world", StreamState::Complete), None),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_text() {
        assert_eq!(format_text(&AgentEvent::text("id", "partial", StreamState::Partial), None), None);
    }

    #[test]
    fn format_text_formats_complete_thought() {
        assert_eq!(
            format_text(&AgentEvent::thought("id", "reasoning here", StreamState::Complete), None),
            Some("Thought: reasoning here".to_string())
        );
    }

    #[test]
    fn format_text_skips_incomplete_thought() {
        assert_eq!(format_text(&AgentEvent::thought("id", "partial", StreamState::Partial), None), None);
    }

    #[test]
    fn format_text_formats_tool_call() {
        let message = AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"cmd":"ls"}"#.to_string(),
            },
        });
        assert_eq!(format_text(&message, None), Some(r#"Tool call: bash({"cmd":"ls"})"#.to_string()));
    }

    #[test]
    fn format_text_skips_tool_call_updates() {
        let message =
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tc1".to_string(), chunk: "partial".to_string() });
        assert_eq!(format_text(&message, None), None);
    }

    #[test]
    fn format_text_formats_tool_result() {
        assert_eq!(format_text(&tool_result(), None), Some("Tool result [bash]: ok".to_string()));
    }

    #[test]
    fn format_text_formats_tool_error() {
        let message = AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: None,
                error: "not found".to_string(),
            },
        });
        assert_eq!(format_text(&message, None), Some("Tool error [bash]: not found".to_string()));
    }

    #[test]
    fn format_text_formats_turn_outcomes() {
        assert_eq!(
            format_text(
                &AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "boom".to_string() } }),
                None,
            ),
            Some("Error: boom".to_string())
        );
        assert_eq!(
            format_text(
                &AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "boom".to_string() } }),
                Some("after 3 retries on Fake LLM"),
            ),
            Some("Error: boom (after 3 retries on Fake LLM)".to_string())
        );
        assert_eq!(format_text(&AgentEvent::turn_ended(TurnOutcome::Cancelled), None), Some("Cancelled".to_string()));
        assert_eq!(format_text(&AgentEvent::turn_ended(TurnOutcome::Completed), None), Some("Done".to_string()));
    }

    #[test]
    fn format_text_formats_retry_events() {
        let started = AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "test".to_string(),
            attempt: 0,
            max_attempts: 3,
        });
        assert_eq!(format_text(&started, None), None);
        assert_eq!(format_text(&retry_scheduled(), None), Some("Retrying (1/3) in 10ms".to_string()));
        let retrying = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("overloaded", true),
        });
        assert_eq!(format_text(&retrying, None), Some("LLM call failed (will retry): overloaded".to_string()));
        let terminal = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: llm::LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("boom", false),
        });
        assert_eq!(format_text(&terminal, None), None);
    }

    #[test]
    fn format_text_formats_auto_continue_and_model_switch() {
        let continuing = AgentEvent::Turn(TurnEvent::AutoContinue {
            attempt: 2,
            max_attempts: 5,
            message_id: llm::MessageId::new(),
            content: vec![],
        });
        assert_eq!(format_text(&continuing, None), Some("Continuing (2/5)...".to_string()));
        let switched =
            AgentEvent::Model(ModelEvent::Switched { previous: "old-model".to_string(), new: "new-model".to_string() });
        assert_eq!(format_text(&switched, None), Some("Model switched: old-model -> new-model".to_string()));
    }

    #[test]
    fn format_text_formats_tool_progress() {
        assert_eq!(
            format_text(&tool_progress(50.0, Some(100.0), Some("halfway")), None),
            Some("Tool progress [bash]: 50/100 - halfway".to_string())
        );
        assert_eq!(format_text(&tool_progress(42.0, None, None), None), Some("Tool progress [bash]: 42".to_string()));
    }

    #[test]
    fn format_text_formats_context_events() {
        let started = AgentEvent::Context(ContextEvent::CompactionStarted {
            compaction_id: "compaction".into(),
            message_count: 42,
        });
        assert_eq!(format_text(&started, None), Some("Context compaction started (42 messages)".to_string()));
        let result = AgentEvent::Context(ContextEvent::CompactionResult {
            compaction_id: "compaction".into(),
            message_id: llm::MessageId::new(),
            summary: "summary here".to_string(),
            messages_removed: 10,
        });
        assert_eq!(
            format_text(&result, None),
            Some("Context compacted: 10 messages removed. summary here".to_string())
        );
        assert_eq!(format_text(&usage_update(), None), Some("Context: 100000 / 200000 tokens (50.0%)".to_string()));
        assert_eq!(format_text(&AgentEvent::Context(ContextEvent::Cleared), None), Some("Context cleared".to_string()));
    }

    #[test]
    fn turn_timings_records_each_turn_and_reports_the_sum() {
        let base = Instant::now();
        let mut timings = TurnTimings::default();
        assert!(timings.is_empty());
        assert_eq!(timings.len(), 0);
        assert_eq!(timings.total(), Duration::ZERO);

        // Turn 1: 1.250s
        timings.begin(base);
        timings.end(base + Duration::from_millis(1250));
        // Turn 2: 2.500s
        timings.begin(base + Duration::from_millis(1250));
        timings.end(base + Duration::from_millis(3750));

        assert_eq!(timings.len(), 2);
        assert_eq!(timings.total(), Duration::from_millis(3750));

        let summary = timings.summary();
        assert!(summary.contains("Turn 1 took "), "missing Turn 1 line: {summary}");
        assert!(summary.contains("Turn 2 took "), "missing Turn 2 line: {summary}");
        assert!(summary.contains(&format_duration(Duration::from_millis(3750))), "missing total in summary: {summary}");
        assert!(summary.contains("2 turns, total"), "missing plural total: {summary}");
    }

    #[test]
    fn turn_timings_ignores_unmatched_end() {
        let base = Instant::now();
        let mut timings = TurnTimings::default();
        timings.end(base + Duration::from_millis(500));
        assert!(timings.is_empty());
        assert_eq!(timings.total(), Duration::ZERO);
    }

    #[test]
    fn turn_summary_body_appends_run_total_to_last_line() {
        let base = Instant::now();
        let mut timings = TurnTimings::default();
        timings.begin(base);
        timings.end(base + Duration::from_millis(1_250));
        timings.begin(base + Duration::from_millis(1_250));
        timings.end(base + Duration::from_millis(3_750));
        // Run total (5s) intentionally differs from per-turn (3.75s) so a swap regression is caught.
        let run_total = Duration::from_millis(5_000);
        let body = turn_summary_body(&timings, Some(run_total));
        let run_total_str = format_duration(run_total);
        let last_line = body.lines().last().unwrap_or("");
        assert!(last_line.contains("2 turns, total"), "per-turn total missing: {last_line:?}");
        assert!(last_line.contains(&run_total_str), "whole-run total missing: {last_line:?}");
        assert!(last_line.ends_with(&run_total_str), "last line should end with whole-run total: {last_line:?}");
        assert!(last_line.contains("run took"), "whole-run label missing: {last_line:?}");
    }

    #[test]
    fn turn_timings_ignores_unmatched_start() {
        let base = Instant::now();
        let mut timings = TurnTimings::default();
        timings.begin(base);
        // No matching end: nothing recorded, pending start is dropped.
        assert!(timings.is_empty());
        assert_eq!(timings.total(), Duration::ZERO);
    }

    fn tool_result() -> AgentEvent {
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

    fn retry_scheduled() -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt: 1,
            max_attempts: 3,
            delay_ms: 10,
        })
    }

    fn usage_update() -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated {
            usage: llm::ContextUsage {
                input_tokens: 100_000.into(),
                context_limit: Some(200_000.into()),
                usage_ratio: Some(0.5),
            },
        })
    }

    fn llm_chat_started(display_name: &str) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: display_name.to_string(),
            attempt: 0,
            max_attempts: 3,
        })
    }

    fn llm_compaction_started(display_name: &str) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Compaction,
            model: llm::ModelIdentity::default(),
            display_name: display_name.to_string(),
            attempt: 0,
            max_attempts: 1,
        })
    }

    fn chat_retry(attempt: u32) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Chat,
            attempt,
            max_attempts: 3,
            delay_ms: 10,
        })
    }

    fn compaction_retry(attempt: u32) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: llm::LlmCallPurpose::Compaction,
            attempt,
            max_attempts: 1,
            delay_ms: 10,
        })
    }

    #[test]
    fn retry_tracker_resets_on_new_turn() {
        let mut tracker = RetryTracker::default();
        tracker.observe(&chat_retry(1));
        tracker.observe(&chat_retry(2));
        tracker.observe(&chat_retry(3));
        assert_eq!(tracker.failure_note(), "after 3 retries");
        tracker.observe(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }));
        assert_eq!(tracker.failure_note(), "after 0 retries");
    }

    #[test]
    fn retry_tracker_counts_only_chat_retries() {
        let mut tracker = RetryTracker::default();
        tracker.observe(&llm_chat_started("primary"));
        // chat retries should be counted
        tracker.observe(&chat_retry(1));
        tracker.observe(&chat_retry(2));
        // compaction retries should be ignored
        tracker.observe(&llm_compaction_started("primary"));
        tracker.observe(&compaction_retry(1));
        assert_eq!(tracker.failure_note(), "after 2 retries on primary");
    }

    #[test]
    fn retry_tracker_failure_note_renders_provider_when_known() {
        let mut tracker = RetryTracker::default();
        assert_eq!(tracker.failure_note(), "after 0 retries");
        tracker.observe(&llm_chat_started("Fake LLM"));
        tracker.observe(&chat_retry(1));
        assert_eq!(tracker.failure_note(), "after 1 retries on Fake LLM");
        tracker.observe(&chat_retry(2));
        tracker.observe(&chat_retry(3));
        assert_eq!(tracker.failure_note(), "after 3 retries on Fake LLM");
    }

    #[test]
    fn summarize_retries_reports_outcome() {
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_chat_started("Fake LLM"),
            chat_retry(1),
            chat_retry(2),
            AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error: "boom".to_string() } }),
        ];
        let summary = summarize_retries(&events);
        assert_eq!(summary.retries, 2);
        assert!(summary.failed);
        assert_eq!(summary.provider.as_deref(), Some("Fake LLM"));
    }

    #[test]
    fn summarize_retries_reports_completed_run() {
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_chat_started("Fake LLM"),
            chat_retry(1),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ];
        let summary = summarize_retries(&events);
        assert_eq!(summary.retries, 1);
        assert!(!summary.failed);
        assert_eq!(summary.provider.as_deref(), Some("Fake LLM"));
    }
}
