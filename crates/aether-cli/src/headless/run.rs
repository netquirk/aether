use aether_core::core::{AgentDeps, Prompt};
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use aether_core::mcp::McpHandle;
use aether_telemetry::TelemetryRuntime;
use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{Instant as TokioInstant, sleep_until};
use tracing::{error, info};

use crate::file_changes::FileChanges;
use crate::log_level::{LogLevel, resolve as resolve_log_level};
use crate::run_timeout::{RunTimeoutWatch, TIMEOUT_EXIT_CODE};
use crate::telemetry::build_telemetry_runtime;
use crate::transcript::JsonlTranscript;
use crate::workspace::warn_if_not_a_repository;

use super::error::CliError;
use super::{CliEventKind, RunConfig};
use crate::output::{
    OutputFormat, ProviderWaitTracker, RetryTracker, TurnTimings, print_message, print_provider_wait, print_run_usage,
    print_turn_summary,
};
use crate::progress::{ToolProgressReporter, tool_progress_update};
use crate::provider_stall::ProviderStallWatch;
use crate::run_usage::RunUsage;
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::{expand_slash_command, parse_slash_command};

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    let log_file = config.log_file.clone();
    setup_tracing(resolve_log_level(config.log_level, config.verbose), config.log_file.as_deref(), config.log_format)
        .map_err(|source| match log_file {
        Some(path) => CliError::LogFileOpen { path, source },
        None => CliError::IoError(source),
    })?;
    // Record which provider and model the run is about to answer so the line
    // appears in whatever log path `--log-file` (TASK-25-48) selected. The
    // record sits before `warn_if_not_a_repository` and the agent/MCP build so
    // it is the first thing each run writes about itself, even when the
    // resolved spec later resolves to an alloy list (the first entry is the
    // model in use; `AlloyedModelProvider` only swaps providers on failure).
    let (provider, model_id) = run_model_identity(&config.spec.model);
    info!(provider = %provider, model_id = %model_id, "run starting");
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
        .settings_tool_output(config.settings_tool_output.clone())
        .shell_environment(config.shell_environment.clone())
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

    // Open the JSON Lines transcript writer up front so a bad path fails
    // the run before any provider call. A successful open is also the
    // promise the file is on disk; a mid-run write error is reported but
    // does not fail the run.
    let mut transcript = match &config.transcript_jsonl {
        Some(path) => {
            Some(JsonlTranscript::create_with_max_bytes(path, config.transcript_max_bytes).map_err(CliError::IoError)?)
        }
        None => None,
    };

    let (exit_code, changes, summary) = stream_output(
        agent.agent_rx,
        config.output,
        &config.events,
        run_started_at,
        config.provider_stall_warn,
        config.quiet,
        io::stderr(),
        transcript.as_mut(),
        config.timeout,
    )
    .await;
    print_run_summary(config.output, &summary);

    let timed_out = exit_code == ExitCode::from(TIMEOUT_EXIT_CODE);
    drop(agent.agent_tx);
    if timed_out {
        // The headless loop returned because the run deadline passed; an
        // in-flight provider call is the most likely reason it never
        // returned a turn outcome. Call [`AgentHandle::abort`] so the
        // background task unwinds instead of being awaited indefinitely,
        // then wait briefly for it to acknowledge the cancel. The
        // bounded wait matches the spirit of the caller-capped timeout:
        // a `--timeout` is meant to cap the entire run, not just the
        // event loop.
        agent.agent_handle.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), agent.agent_handle.await_completion()).await;
    } else {
        agent.agent_handle.await_completion().await;
    }

    if config.output == OutputFormat::Text {
        println!("{}", changes.summary());
    }

    Ok(exit_code)
}

fn print_run_summary(format: OutputFormat, summary: &RunSummary) {
    // Only text mode gets the aggregate line; machine formats already have
    // every event on the stream.
    if matches!(format, OutputFormat::Text) {
        println!("{}", summary.line());
    }
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

// `stream_output` takes 8 positional parameters: the addition of the
// caller-capped run timeout (TASK-25-18) brought it from 7 to 8. Bundling
// them into a config struct would obscure the call sites without reducing
// total surface area, so silence the threshold-crossing lint.
#[allow(clippy::too_many_arguments)]
async fn stream_output<W: io::Write>(
    mut rx: mpsc::Receiver<AgentEvent>,
    format: OutputFormat,
    events: &[CliEventKind],
    run_started_at: Instant,
    provider_stall_warn: Option<Duration>,
    quiet: bool,
    progress_writer: W,
    mut transcript: Option<&mut JsonlTranscript>,
    run_timeout: Option<Duration>,
) -> (ExitCode, FileChanges, RunSummary) {
    let mut tracker = RetryTracker::default();
    // Wall-clock timing of every turn seen on the stream, independent of the
    // `--events` filter so a filtered run still reports how long it ran.
    let mut timings = TurnTimings::default();
    // Wall-clock time the agent spent blocked on provider responses. Like
    // `timings`/`usage`, fed every event so a `--events`-filtered run still
    // reports the figure.
    let mut provider_wait = ProviderWaitTracker::default();
    // Per-model token totals across every SessionUsage event, also
    // independent of the `--events` filter so the end-of-run summary is
    // always complete.
    let mut usage = RunUsage::default();
    let mut changes = FileChanges::default();
    let mut summary = RunSummary::default();
    let mut exit_code = ExitCode::SUCCESS;
    // Live stderr progress line naming the tool currently executing. Like
    // `timings`/`usage`, it is fed every event so a `--events`-filtered run
    // still updates the status line. When `--quiet` is set the reporter is
    // still constructed (so the post-loop `clear()` calls stay safe) but
    // `apply()` is skipped below, so no progress bytes reach `progress_writer`.
    // Resolved once per run from the `NO_COLOR` environment variable via
    // [`crate::color::color_enabled`]: when colour is off the reporter writes
    // plain newline-terminated lines and never emits ANSI / cursor-control
    // sequences, regardless of `--quiet`.
    let color = crate::color::color_enabled();
    let mut progress = ToolProgressReporter::new(progress_writer, color);
    // Live one-line stall warning for provider calls that exceed
    // `provider_stall_warn`. Fed the same events as `provider_wait`. The
    // select loop below races `rx.recv()` against the watch's deadline so a
    // hung call prints the warning once instead of hanging silently.
    // `--quiet` intentionally does not gate the stall warning: it is a
    // problem-reporting line the task asks to keep.
    let mut stall = ProviderStallWatch::new(provider_stall_warn, io::stderr());
    // Caller-capped wall-clock run timeout (TASK-25-18). Unlike the stall
    // watch, this one fires on the run, not per-LLM-call: the headless
    // event loop races the run deadline against the stall deadline and
    // `rx.recv()`, and the earliest of the three wakes the loop. When the
    // run deadline is the earliest the loop exits through the timeout
    // branch, leaving the post-loop bookkeeping in a state that returns
    // `TIMEOUT_EXIT_CODE` instead of `FAILURE`/`SUCCESS`. The sink is
    // `io::stderr()` (same as the stall warning) so machine `--output json`
    // streams stay clean.
    let mut timeout = RunTimeoutWatch::new(run_timeout, run_started_at, io::stderr());

    loop {
        // Race the next event against the stall deadline and the caller-capped
        // run deadline. The earliest of the three wakes the loop; when none of
        // them is due (no in-flight call, no timeout set, or the timeout has
        // already fired) the loop falls back to the plain `rx.recv()` path
        // with no timer overhead. Extracted to keep `stream_output` under the
        // `clippy::too_many_lines` pedantic threshold.
        let next = await_with_deadlines(&mut rx, &mut stall, &mut timeout).await;
        let maybe_event = match next {
            NextEvent::Received(event) => event,
            NextEvent::TimedOut => {
                return finish_run_timed_out(
                    &mut progress,
                    format,
                    &timings,
                    &provider_wait,
                    &usage,
                    &mut transcript,
                    run_started_at,
                    changes,
                    summary,
                );
            }
            NextEvent::Loop => continue,
        };
        let Some(event) = maybe_event else { break };
        let msg: &AgentEvent = &event;
        if let AgentEvent::SessionUsage(sample) = msg {
            usage.record(sample);
        }

        // Capture the note for failed turns *before* we update the tracker
        // with the event we are about to print. Observing the `Ended` event
        // for a failed turn does not change the count or provider, so the
        // value is identical before and after the observation.
        let note = match msg {
            AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } }) => Some(tracker.failure_note()),
            _ => None,
        };
        tracker.observe(msg);

        // Counts for the end-of-run summary; independent of `--events` so a
        // filtered run still reports how many turns and tool calls happened.
        summary.record(msg);

        // Update the live stderr progress line (Text mode only — Json/Pretty
        // are machine-readable and must not be polluted with control codes).
        // Fed every event, like `timings`/`usage`, so a filtered run still
        // reflects the tool currently executing. `--quiet` short-circuits the
        // `apply()` call so no progress bytes reach `progress_writer`; the
        // end-of-loop `clear()` calls below stay no-ops because the
        // reporter is never set active in quiet mode.
        if !quiet
            && matches!(format, OutputFormat::Text)
            && let Some(update) = tool_progress_update(msg)
            && let Err(error) = progress.apply(update)
        {
            eprintln!("Failed to write tool progress: {error}");
        }

        match msg {
            AgentEvent::Turn(TurnEvent::Started { .. }) => timings.begin(Instant::now()),
            AgentEvent::Turn(TurnEvent::Ended { .. }) => timings.end(Instant::now()),
            AgentEvent::Turn(TurnEvent::LlmCallStarted { .. } | TurnEvent::LlmCallEnded { .. }) => {
                // Sample `now` once so the live stall warning and the per-call
                // wait tracker agree on the elapsed time.
                let now = Instant::now();
                provider_wait.observe(msg, now);
                stall.observe(msg, now);
            }
            _ => {}
        }

        if let Some(meta) = tool_result_meta(msg) {
            changes.record(meta);
        }

        // Write the transcript line before we print: a bad disk shows up
        // here, not as a corrupt half-line on stdout. The writer ignores
        // events with no CLI event kind (streaming fragments, CallUpdate).
        if let Some(t) = transcript.as_mut()
            && let Err(error) = t.record(msg)
        {
            eprintln!("Failed to write transcript: {error}");
        }

        if should_emit(msg, events)
            && let Err(error) = print_message(format, msg, note.as_deref())
        {
            eprintln!("Failed to serialize headless event: {error}");
            // Clear the live progress line so we do not leave a half-written
            // status row on stderr if the loop exits early.
            let _ = progress.clear();
            if let Some(t) = transcript.as_mut() {
                let _ = t.flush();
            }
            return (ExitCode::FAILURE, changes, summary);
        }

        if let Some(outcome) = msg.turn_outcome() {
            exit_code = match outcome {
                TurnOutcome::Failed { .. } => ExitCode::FAILURE,
                TurnOutcome::Completed | TurnOutcome::Cancelled | TurnOutcome::MaxTurnsReached { .. } => {
                    ExitCode::SUCCESS
                }
            };
            break;
        }
    }

    let run_total_elapsed = run_started_at.elapsed();
    // Erase the live progress line so it does not bleed into the turn/run
    // summary printed just below; clear() is a no-op when no line is active.
    let _ = progress.clear();
    print_turn_summary(format, &timings, Some(run_total_elapsed));
    print_provider_wait(format, &provider_wait);
    print_run_usage(format, &usage);
    if let Some(t) = transcript.as_mut() {
        let _ = t.flush();
    }
    (exit_code, changes, summary)
}

/// Wrap the post-loop bookkeeping (clear the live progress line, print the
/// per-turn / run / provider-wait / usage summaries, flush the transcript)
/// the timeout branch shares with the natural loop end. Returns the distinct
/// exit code so `stream_output`'s single call site stays uniform.
#[allow(clippy::too_many_arguments)]
fn finish_run_timed_out<W: io::Write>(
    progress: &mut ToolProgressReporter<W>,
    format: OutputFormat,
    timings: &TurnTimings,
    provider_wait: &ProviderWaitTracker,
    usage: &RunUsage,
    transcript: &mut Option<&mut JsonlTranscript>,
    run_started_at: Instant,
    changes: FileChanges,
    summary: RunSummary,
) -> (ExitCode, FileChanges, RunSummary) {
    let run_total_elapsed = run_started_at.elapsed();
    // Erase the live progress line so it does not bleed into the turn/run
    // summary printed just below; clear() is a no-op when no line is active.
    let _ = progress.clear();
    print_turn_summary(format, timings, Some(run_total_elapsed));
    print_provider_wait(format, provider_wait);
    print_run_usage(format, usage);
    if let Some(t) = transcript.as_mut() {
        let _ = t.flush();
    }
    (ExitCode::from(TIMEOUT_EXIT_CODE), changes, summary)
}

/// Outcome of one iteration of `stream_output`'s timer race.
///
/// The headless event loop arms a stall deadline (when a provider call is
/// in flight and not yet warned about) and a run deadline (when `--timeout`
/// is set); both can be unset, in which case the loop falls back to a plain
/// `recv()`. Extracted into an enum so the helper that owns the timer race
/// can talk to the loop without an `Option`/`Result` dance.
enum NextEvent {
    /// `rx.recv()` produced an event (or `None` if the channel closed). The
    /// payload is boxed so the bare variants (`TimedOut` / `Loop`) do not
    /// enlarge the enum by the size of [`AgentEvent`].
    Received(Option<Box<AgentEvent>>),
    /// The caller-capped run deadline fired; the loop must exit with the
    /// distinct timeout exit code.
    TimedOut,
    /// A watch latched (the stall warning fired) without reaching the run
    /// deadline; the loop should re-arm the timers and `continue`.
    Loop,
}

/// Wait for the next agent event while racing both watches' deadlines.
///
/// Falls back to `rx.recv().await` when neither deadline is set. When the
/// earliest deadline is the stall one, `warn_if_stalled` latches the watch
/// and the helper returns [`NextEvent::Loop`] so the caller can `continue`
/// without re-reading. When the earliest deadline is the run timeout,
/// `expire_if_due` writes the timeout line and the helper returns
/// [`NextEvent::TimedOut`]; the caller exits the loop with
/// [`crate::run_timeout::TIMEOUT_EXIT_CODE`].
#[allow(clippy::too_many_arguments)]
/// Either a channel event (or `None` if the channel closed) or one of the
/// two deadlines woke the loop. Used to keep the `tokio::select!` arms in
/// `await_with_deadlines` structurally homogeneous; without it the
/// "deadline fired" arms need a sentinel payload to satisfy the macro's
/// type constraint.
enum RaceOutcome {
    /// `rx.recv()` produced an event (or `None` if the channel closed).
    EventReceived(Option<Box<AgentEvent>>),
    /// The caller-capped run deadline fired; the loop must exit with the
    /// distinct timeout exit code.
    TimedOut,
    /// The provider-stall deadline fired; the loop should re-arm the
    /// timers and `continue`.
    StallWarned,
}

async fn await_with_deadlines<W: io::Write, T: io::Write>(
    rx: &mut mpsc::Receiver<AgentEvent>,
    stall: &mut ProviderStallWatch<W>,
    timeout: &mut RunTimeoutWatch<T>,
) -> NextEvent {
    fn received(opt: Option<AgentEvent>) -> RaceOutcome {
        RaceOutcome::EventReceived(opt.map(Box::new))
    }

    fn try_poll_after<W: io::Write, T: io::Write>(
        stall: &mut ProviderStallWatch<W>,
        timeout: &mut RunTimeoutWatch<T>,
        now: Instant,
    ) -> Option<RaceOutcome> {
        if timeout.expire_if_due(now).unwrap_or(false) {
            return Some(RaceOutcome::TimedOut);
        }
        if stall.warn_if_stalled(now).unwrap_or(false) {
            return Some(RaceOutcome::StallWarned);
        }
        None
    }

    let stall_d = stall.next_deadline();
    let timeout_d = timeout.deadline();
    let outcome = match (stall_d, timeout_d) {
        (None, None) => received(rx.recv().await),
        (Some(stall_d), None) => {
            let tokio_deadline = TokioInstant::from_std(stall_d);
            tokio::select! {
                biased;
                () = sleep_until(tokio_deadline) => {
                    if let Err(error) = stall.warn_if_stalled(Instant::now()) {
                        eprintln!("Failed to write provider stall warning: {error}");
                    }
                    RaceOutcome::StallWarned
                }
                maybe = rx.recv() => received(maybe),
            }
        }
        (None, Some(timeout_d)) => {
            let tokio_deadline = TokioInstant::from_std(timeout_d);
            tokio::select! {
                biased;
                () = sleep_until(tokio_deadline) => {
                    if let Err(error) = timeout.expire_if_due(Instant::now()) {
                        eprintln!("Failed to write run timeout line: {error}");
                    }
                    RaceOutcome::TimedOut
                }
                maybe = rx.recv() => received(maybe),
            }
        }
        (Some(stall_d), Some(timeout_d)) => {
            if stall_d <= timeout_d {
                let tokio_deadline = TokioInstant::from_std(stall_d);
                tokio::select! {
                    biased;
                    () = sleep_until(tokio_deadline) => {
                        let now = Instant::now();
                        if let Some(race) = try_poll_after(stall, timeout, now) {
                            race
                        } else {
                            // Race resolved without either firing (e.g. an
                            // event landed between the timer arm and the
                            // wakeup). Fall through to a plain recv().
                            received(rx.recv().await)
                        }
                    }
                    maybe = rx.recv() => received(maybe),
                }
            } else {
                let tokio_deadline = TokioInstant::from_std(timeout_d);
                tokio::select! {
                    biased;
                    () = sleep_until(tokio_deadline) => {
                        if let Some(race) = try_poll_after(stall, timeout, Instant::now()) {
                            race
                        } else {
                            received(rx.recv().await)
                        }
                    }
                    maybe = rx.recv() => received(maybe),
                }
            }
        }
    };
    match outcome {
        RaceOutcome::EventReceived(event) => NextEvent::Received(event),
        RaceOutcome::StallWarned => NextEvent::Loop,
        RaceOutcome::TimedOut => NextEvent::TimedOut,
    }
}

/// Per-run tally of turns started and tool calls made by `stream_output`.
/// Counts come from `AgentEvent` matches (not `TurnTimings`/`FileChanges`)
/// so an unmatched `TurnEvent::Ended` or any `Tool(ToolEvent::Call)` is still
/// reflected.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RunSummary {
    turns: u32,
    tool_calls: u32,
}

impl RunSummary {
    fn record(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Turn(TurnEvent::Started { .. }) => self.turns = self.turns.saturating_add(1),
            AgentEvent::Tool(ToolEvent::Call { .. }) => self.tool_calls = self.tool_calls.saturating_add(1),
            _ => {}
        }
    }

    fn line(&self) -> String {
        format!(
            "Run finished: {} {}, {} {}",
            self.turns,
            turn_label(self.turns),
            self.tool_calls,
            tool_call_label(self.tool_calls),
        )
    }
}

fn turn_label(count: u32) -> &'static str {
    if count == 1 { "turn" } else { "turns" }
}

fn tool_call_label(count: u32) -> &'static str {
    if count == 1 { "tool call" } else { "tool calls" }
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

pub(crate) fn event_kind(msg: &AgentEvent) -> Option<CliEventKind> {
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
        AgentEvent::Tool(ToolEvent::Refused { .. }) => Some(CliEventKind::ToolRefused),
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

/// Split the resolved model spec into the `(provider, model_id)` pair the
/// run-start log line names (TASK-25-119). The spec is canonical
/// (`provider:model`) or an alloy list (`p1:m1,p2:m2`); the first entry is
/// the model in use, mirroring `ModelProviderParser::parse` which threads the
/// first identity through `AlloyedModelProvider` as the default and only
/// swaps providers on a failed call. When the spec entry parses as a
/// catalogued `LlmModel` the answer comes from the catalog's own
/// `provider()` / `model_id()` accessors; otherwise the raw `provider:model`
/// split is the fallback so unknown or local-only specs still surface a
/// sensible `(provider, model_id)` pair in the log.
fn run_model_identity(model_spec: &str) -> (String, String) {
    let first = model_spec.split(',').next().unwrap_or(model_spec);
    let first = first.trim();
    match first.parse::<llm::LlmModel>() {
        Ok(model) => (model.provider().to_string(), model.model_id().into_owned()),
        Err(_) => {
            let (provider, model_id) = first.split_once(':').unwrap_or(("", first));
            (provider.to_string(), model_id.to_string())
        }
    }
}

pub(crate) fn setup_tracing(
    level: LogLevel,
    log_file: Option<&Path>,
    format: crate::log_format::LogFormat,
) -> std::io::Result<()> {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::fmt::writer::BoxMakeWriter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::new(level.directive());
    // Converge the two writers onto a single `BoxMakeWriter` so the
    // `format` match below can use a single concrete writer type. The
    // writer's behaviour matches the pre-existing `setup_tracing` exactly:
    // missing parent directories still fail the run; the file is opened
    // in append mode (TASK-25-48) so an operator's existing log is
    // preserved across runs.
    let writer = match log_file {
        None => BoxMakeWriter::new(io::stderr),
        Some(path) => {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            BoxMakeWriter::new(file)
        }
    };
    // Gate the `Text` layer's ANSI on the same `NO_COLOR` signal the live
    // progress line honours: a `NO_COLOR=1` run emits log records to
    // stderr in plain text, matching the rest of the colour-free output.
    // When the operator has redirected the log to a file (TASK-25-48), or
    // selected the JSON format (TASK-25-99), ANSI is unconditionally
    // disabled so the on-disk / on-stderr log is a stable format.
    let ansi = log_file.is_none() && crate::color::color_enabled();

    match format {
        // The JSON formatter (TASK-25-99) emits one JSON object per line
        // with top-level `timestamp`, `level`, `message`, and `target`
        // fields. `flatten_event(true)` hoists the message up to the
        // top level instead of nesting it under a `fields` key, so the
        // emitted object matches the task's "level / message / timestamp"
        // contract directly. A separate `match` arm keeps the two layer
        // types (text vs json) from having to unify into one.
        crate::log_format::LogFormat::Json => {
            let layer =
                fmt::layer().json().flatten_event(true).with_writer(writer).with_ansi(false).with_filter(filter);
            let _ = tracing_subscriber::registry().with(layer).try_init();
        }
        crate::log_format::LogFormat::Text => {
            let layer = fmt::layer().with_writer(writer).with_ansi(ansi).with_filter(filter);
            let _ = tracing_subscriber::registry().with(layer).try_init();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use aether_core::events::StreamState;

    use super::*;
    use llm::ContextUsage;

    /// Process-global lock borrowed from [`crate::color::tests::ENV_LOCK`]:
    /// every test in the crate that mutates `NO_COLOR` shares it so no two
    /// run in parallel. Declared `&'static Mutex<()>` so it's a single
    /// crate-wide instance the static reference borrows.
    use crate::color::tests::ENV_LOCK as NO_COLOR_LOCK;
    static ENV_LOCK: &std::sync::Mutex<()> = &NO_COLOR_LOCK;

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
    fn run_model_identity_splits_canonical_spec() {
        // Canonical `provider:model` form goes through the catalogued
        // `LlmModel` parser, so the answer must use the catalog's own
        // `provider()` / `model_id()` accessors rather than the raw split.
        assert_eq!(run_model_identity("ollama:llama3.2"), ("ollama".to_string(), "llama3.2".to_string()));
    }

    #[test]
    fn run_model_identity_picks_first_entry_in_alloy_spec() {
        // Alloy specs are stored verbatim as comma-separated entries; the
        // first entry is the model in use (see
        // `ModelProviderParser::parse` which threads the first identity
        // through `AlloyedModelProvider` as the default), so the log line
        // names that one regardless of how many follow.
        assert_eq!(
            run_model_identity("anthropic:claude-sonnet-4-5,ollama:llama3.2"),
            ("anthropic".to_string(), "claude-sonnet-4-5".to_string())
        );
    }

    #[test]
    fn run_model_identity_trims_whitespace_around_first_entry() {
        // `ModelProviderParser::parse` trims each entry before parsing, and
        // the resolved spec on `AgentSpec` reflects that trim. The helper
        // re-trims defensively so a future caller that hands it a less-clean
        // string still gets the right pair in the log line.
        assert_eq!(
            run_model_identity(" ollama:llama3.2 ,anthropic:claude-sonnet-4-5"),
            ("ollama".to_string(), "llama3.2".to_string())
        );
    }

    #[test]
    fn run_model_identity_falls_back_to_raw_split_for_unknown_specs() {
        // Custom / dynamic specs that the catalog parser rejects still need a
        // `(provider, model_id)` pair in the log; the raw `split(':')`
        // fallback keeps that observable working without forcing the run to
        // fail before any provider work happens.
        assert_eq!(run_model_identity("custom:my-local-model"), ("custom".to_string(), "my-local-model".to_string()));
        assert_eq!(run_model_identity("plain-no-colon"), ("".to_string(), "plain-no-colon".to_string()));
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
            (
                AgentEvent::Tool(ToolEvent::Refused {
                    request: llm::ToolCallRequest {
                        id: "tc1".to_string(),
                        name: "bash".to_string(),
                        arguments: "{}".to_string(),
                    },
                    reason: "no shell access".to_string(),
                }),
                CliEventKind::ToolRefused,
            ),
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
        let (code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &filter, Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 0);
    }

    #[tokio::test]
    async fn stream_output_failed_turn_exits_with_failure() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::turn_ended(TurnOutcome::Failed { error: "boom".to_string() })).await.unwrap();
        let (code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(changes.total(), 0);
    }

    #[tokio::test]
    async fn stream_output_reports_two_file_changes() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(tool_result_with_file_diff("created.rs", None, Some("new"))).await.unwrap();
        tx.send(tool_result_with_file_diff("edited.rs", Some("old"), Some("new"))).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
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
        let (code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(changes.total(), 0);
        assert!(changes.summary().contains("Files changed: 0"));
    }

    /// Feeds a `SessionUsage` payload (the carrier for the provider's
    /// per-call token counts) followed by `turn_ended`, and asserts the
    /// stream completes successfully. The end-of-run usage block is rendered
    /// once, after the loop, by `print_run_usage`; the count assertion lives
    /// in `run_usage::tests::render_text_carries_provider_reported_token_counts`
    /// where the same payload is fed and the rendered text is checked byte
    /// for byte. Driving `stream_output` here proves the event flows through
    /// the same path a real run takes (per-event recording + the post-loop
    /// render) without regressing to a pre-loop or duplicate print.
    #[tokio::test]
    async fn stream_output_passes_session_usage_event_through_to_end_of_run() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(session_usage_event(1, llm::TokenUsage::new(11, 22))).await.unwrap();
        tx.send(session_usage_event(2, llm::TokenUsage::new(33, 44))).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        drop(tx);

        let (code, _changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// Companion to the above: a turn that ends without any `SessionUsage`
    /// event must still complete cleanly, and `print_run_usage` must be a
    /// no-op for the empty case (its `is_empty()` short-circuit). The empty
    /// guarantee is asserted in `run_usage::tests::run_usage_empty_has_no_summary`
    /// at the unit level; this test covers the path through `stream_output`.
    #[tokio::test]
    async fn stream_output_with_no_session_usage_completes_cleanly() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        drop(tx);

        let (code, _changes, summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(summary, RunSummary { turns: 1, tool_calls: 0 });
    }

    fn session_usage_event(seq: u64, tokens: llm::TokenUsage) -> AgentEvent {
        AgentEvent::SessionUsage(llm::testing::session_usage_event(seq, tokens))
    }

    #[tokio::test]
    async fn stream_output_counts_task_completed_file_diffs() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(task_completed_with_file_diff("removed.rs", Some("old"), None)).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let (code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
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
        let (_code, changes, _summary) =
            stream_output(rx, OutputFormat::Text, &filter, Instant::now(), None, false, io::sink(), None, None).await;
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
        let (code, _changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn stream_output_counts_turns_and_tool_calls() {
        let (tx, rx) = mpsc::channel(8);
        // Two turns and two tool calls, interleaved with their results. The
        // second turn emits only a tool result (no `Call`) so the tool count
        // stays at 2 and the second turn is "passive".
        tx.send(AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await.unwrap();
        tx.send(tool_call_msg()).await.unwrap();
        tx.send(tool_result_msg()).await.unwrap();
        tx.send(tool_call_msg()).await.unwrap();
        tx.send(tool_result_msg()).await.unwrap();
        tx.send(AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await.unwrap();
        tx.send(tool_result_msg()).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        drop(tx);

        let (code, _changes, summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, io::sink(), None, None).await;

        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(summary, RunSummary { turns: 2, tool_calls: 2 });
        // The summary is a single line so downstream tooling can grep for it
        // without false positives from the per-turn timing block above.
        let line = summary.line();
        assert_eq!(line, "Run finished: 2 turns, 2 tool calls");
        assert_eq!(line.lines().count(), 1);
    }

    fn execution_started(tool: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id: "tc1".to_string(), tool_name: tool.to_string() })
    }

    #[tokio::test]
    async fn stream_output_quiet_suppresses_tool_progress_line() {
        // Not quiet: the live line is written (contains the ⏺ glyph, bytes e2 8f ba).
        let (tx, rx) = mpsc::channel(4);
        tx.send(execution_started("bash")).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let mut sink = Vec::new();
        stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, &mut sink, None, None).await;
        assert!(sink.windows(3).any(|w| w == b"\xe2\x8f\xba"), "non-quiet must draw the progress line: {sink:?}");

        // Quiet: no progress bytes at all.
        let (tx, rx) = mpsc::channel(4);
        tx.send(execution_started("bash")).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let mut sink = Vec::new();
        stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, true, &mut sink, None, None).await;
        assert!(sink.is_empty(), "quiet must not write progress bytes: {sink:?}");
    }

    /// RAII guard that captures the pre-test value of `NO_COLOR` and restores
    /// it on drop. The `Drop` impl calls `unsafe` `env::{set_var, remove_var}`
    /// because edition 2024 marks those `unsafe` (process-global mutation).
    struct EnvGuard(std::option::Option<std::ffi::OsString>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe {
                    std::env::set_var("NO_COLOR", value);
                },
                None => unsafe {
                    std::env::remove_var("NO_COLOR");
                },
            }
        }
    }

    /// Drive `stream_output` with one synthetic `ExecutionStarted`+`turn_ended`
    /// sequence, returning every byte the progress writer received. Used by
    /// [`stream_output_no_color_emits_plain_progress_line`] to assert
    /// byte-level guarantees from both colour states.
    async fn drain_progress_sink() -> Vec<u8> {
        let (tx, rx) = mpsc::channel(4);
        tx.send(execution_started("bash")).await.unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();
        let mut sink = Vec::new();
        stream_output(rx, OutputFormat::Text, &[], Instant::now(), None, false, &mut sink, None, None).await;
        sink
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_output_no_color_emits_plain_progress_line() {
        // The lock below is a process-global; we hold it across the
        // `stream_output` future because the colour decision reads
        // `NO_COLOR` once at run start, and another test could mutate the
        // environment between the env write and the future's read on a
        // multi-threaded runtime. With `current_thread` the only awaits are
        // on the same task as the holder, so no other task can race for the
        // mutex; the allow documents that the test is intentionally
        // single-threaded rather than relying on the runtime default.
        #[allow(clippy::await_holding_lock)]
        async fn body() {
            // The colour-state decision is read once at the top of
            // `stream_output`, so every nested test below restores `NO_COLOR`
            // to its prior value on the way out (even on panic) before the
            // next test runs. Same convention as `with_env` in
            // `crates/aether-core/tests/mcp/config_parser_tests.rs`.
            let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let prior = std::env::var_os("NO_COLOR");

            // With `NO_COLOR=1` the progress line must still appear (so a
            // watcher on stderr can see tool names) but only as plain text:
            // no `0x1b`, no `\r`, the tool name on a single terminated line.
            unsafe {
                std::env::set_var("NO_COLOR", "1");
            }
            let restore = EnvGuard(prior.clone());
            let sink = drain_progress_sink().await;
            assert!(!sink.contains(&0x1b), "NO_COLOR=1 run must not contain any 0x1b escape byte: {sink:?}");
            assert!(!sink.contains(&b'\r'), "NO_COLOR=1 run must not contain carriage returns: {sink:?}");
            assert!(
                sink.windows(b"\xe2\x8f\xba".len()).any(|w| w == b"\xe2\x8f\xba"),
                "tool name should still be on the plain-text line: {sink:?}"
            );
            drop(restore);

            // With `NO_COLOR` absent the in-place colourful form returns.
            unsafe {
                std::env::remove_var("NO_COLOR");
            }
            let restore = EnvGuard(prior.clone());
            let sink = drain_progress_sink().await;
            assert!(sink.contains(&0x1b), "colour run must contain at least one 0x1b escape byte: {sink:?}");
            assert!(
                sink.windows(b"\x1b[K".len()).any(|w| w == b"\x1b[K"),
                "colour run must use the in-place replacement form (`\\x1b[K` to erase to EOL): {sink:?}"
            );
            assert!(
                sink.windows(b"\r\x1b[2K".len()).any(|w| w == b"\r\x1b[2K"),
                "colour run must end with the full-line clear escape: {sink:?}"
            );
            assert!(
                sink.starts_with(b"\r"),
                "colour run should start with the carriage-return that overwrites the previous tool: {sink:?}"
            );
            drop(restore);
        }

        body().await;
    }

    #[tokio::test]
    async fn stream_output_terminates_with_stall_threshold_configured() {
        // A short threshold still terminates when a turn ends: the select
        // loop must break on the `turn_ended` event, not the deadline branch.
        let (tx, rx) = mpsc::channel(4);
        tx.send(AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: llm::LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: "primary".to_string(),
            attempt: 0,
            max_attempts: 3,
        }))
        .await
        .unwrap();
        tx.send(AgentEvent::turn_ended(TurnOutcome::Completed)).await.unwrap();

        let threshold = Duration::from_millis(50);
        let (code, _changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], Instant::now(), Some(threshold), false, io::sink(), None, None)
                .await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// The expiry path that TASK-25-18 tests. The channel is held open (the
    /// sender is kept alive) so `rx.recv()` would otherwise block forever;
    /// `run_started_at` is set in the past so the run deadline is already due
    /// on the first iteration. The `biased` `sleep_until` arm fires without
    /// any real time elapsing, the `RunTimeoutWatch` writes its line, and the
    /// loop returns the distinct timeout exit code instead of hanging.
    #[tokio::test(start_paused = true)]
    async fn stream_output_expires_with_timeout_exit_code() {
        // Future date arithmetic: subtract a large-but-representable duration
        // from the current `Instant` so the deadline has already passed even on
        // a freshly-initialised runtime. 32-bit platforms support a span of
        // ~136 years, so any combination of `<limit> + 1s` is well under the
        // ceiling.
        let (_keep_tx, rx) = mpsc::channel::<AgentEvent>(1);
        let limit = Duration::from_secs(30);
        let started = Instant::now()
            .checked_sub(limit + Duration::from_secs(1))
            .expect("`Instant - 31s` must remain representable on the test runtime");
        let (code, _changes, _summary) =
            stream_output(rx, OutputFormat::Text, &[], started, None, false, io::sink(), None, Some(limit)).await;
        assert_eq!(
            code,
            ExitCode::from(crate::run_timeout::TIMEOUT_EXIT_CODE),
            "loop must exit with the distinct timeout exit code"
        );
        assert_ne!(code, ExitCode::FAILURE, "timeout exit code must differ from generic failure");
        assert_ne!(code, ExitCode::SUCCESS, "timeout exit code must differ from a successful run");
    }

    #[test]
    fn run_summary_line_pluralises_counts() {
        // Zero, one, and many cases for both counts catch singular/plural
        // regressions independently.
        assert_eq!(RunSummary::default().line(), "Run finished: 0 turns, 0 tool calls");
        assert_eq!(RunSummary { turns: 1, tool_calls: 1 }.line(), "Run finished: 1 turn, 1 tool call",);
        assert_eq!(RunSummary { turns: 1, tool_calls: 3 }.line(), "Run finished: 1 turn, 3 tool calls",);
        assert_eq!(RunSummary { turns: 4, tool_calls: 1 }.line(), "Run finished: 4 turns, 1 tool call",);
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
