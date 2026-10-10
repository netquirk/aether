use crate::context::{
    CompactionConfig, CompactionError, CompactionResult, Compactor, SessionUsageTracker, TokenTracker,
};
use crate::core::PromptCache;
use crate::core::prompt_cache_key::derive_prompt_cache_key;
use crate::core::queued_input::QueuedInput;
use crate::core::repetition_config::{RepetitionConfig, RepetitionTracker, iteration_signature};
pub use crate::core::retry_config::RetryConfig;
use crate::core::tool_execution::{ToolAbortPolicy, ToolExecutionUpdate, ToolExecutions};
pub use crate::core::tool_policy::ToolPolicy;
use crate::events::{
    AgentCommand, AgentEvent, AgentObserver, Command, CompactionId, CompactionOutcome, ContextEvent, LlmCallOutcome,
    ModelEvent, StreamState, TaskOutcome, ToolEvent, TraceContext, TurnEvent, TurnOutcome, UserCommand,
    refusal_context_message,
};
use crate::mcp::McpHandle;
use futures::Stream;
use llm::{
    AssistantReasoning, ChatMessage, Context, EncryptedReasoningContent, LlmCallPurpose, LlmError, LlmModel,
    LlmResponse, MessageId, ModelIdentity, StopReason, StreamingModelProvider, TokenUsage, ToolCallError,
    ToolCallRequest, ToolCallResult,
};
use mcp_utils::client::{CallToolError, CallToolOptions, ToolCallEvent};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::ReceiverStream;

/// Internal event type for merging LLM and tool result streams
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum StreamEvent {
    LlmRequestStarted { attempt: u32 },
    Llm(Result<LlmResponse, LlmError>),
    ToolExecution(ToolCallEvent),
    Command(Command),
    InputClosed,
    Compaction(Result<CompactionResult, CompactionError>),
}

type EventStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

/// Keys for the merged stream map. Tool-call IDs come from providers, so the
/// typed key keeps them from colliding with reserved streams.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StreamKey {
    Input,
    Llm,
    Compaction,
    Tool(String),
}

pub(crate) struct AgentConfig {
    pub llm: Arc<dyn StreamingModelProvider>,
    pub context: Context,
    pub mcp: Option<McpHandle>,
    pub tool_timeout: Duration,
    pub compaction_config: Option<CompactionConfig>,
    pub auto_continue: AutoContinue,
    pub retry_config: RetryConfig,
    pub repetition: RepetitionConfig,
    pub context_window: Option<u32>,
    /// Optional cap on the number of LLM chat turns in a single run. When
    /// `Some(n)`, the run ends cleanly with
    /// [`TurnOutcome::MaxTurnsReached`](crate::events::TurnOutcome::MaxTurnsReached)
    /// once `n` chat turns have started. `None` means unbounded.
    pub max_turns: Option<u32>,
    pub prompt_cache: PromptCache,
    pub observers: Vec<Box<dyn AgentObserver>>,
    pub session_usage: SessionUsageTracker,
    pub tool_policy: Arc<dyn ToolPolicy>,
    /// When `true`, a turn that ends with no tool call and no `EndTurn`
    /// declaration is treated as a "dead turn" and reused with the existing
    /// auto-continue budget: first occurrence nudges the model once, the next
    /// occurrence fails the turn with `"turn ended without a tool call"`.
    pub require_tool_call: bool,
}

pub struct Agent {
    llm: Arc<dyn StreamingModelProvider>,
    context: Context,
    mcp: Option<McpHandle>,
    message_tx: mpsc::Sender<AgentEvent>,
    observers: Vec<Box<dyn AgentObserver>>,
    streams: StreamMap<StreamKey, EventStream>,
    tool_timeout: Duration,
    tool_policy: Arc<dyn ToolPolicy>,
    token_tracker: TokenTracker,
    compaction_config: Option<CompactionConfig>,
    auto_continue: AutoContinue,
    retry_config: RetryConfig,
    repetition: RepetitionTracker,
    tool_executions: ToolExecutions,
    pending_inputs: VecDeque<QueuedInput>,
    queued_inputs: VecDeque<QueuedInput>,
    context_window: Option<u32>,
    /// Optional cap on the number of LLM chat turns in a single run.
    max_turns: Option<u32>,
    /// Number of LLM chat turns already started in the current run. Reset on
    /// every `begin_turn`.
    turns_started: u32,
    prompt_cache: PromptCache,
    turn_active: bool,
    llm_call_active: bool,
    active_compaction: Option<CompactionId>,
    active_model: Option<LlmModel>,
    session_usage: SessionUsageTracker,
    /// Resumes after a mid-stream cut since the last cleanly finished model
    /// call. Shares `retry_config.max_attempts` with plain retries.
    stream_resumes: u32,
    /// Set when an interrupted call is being resumed: the continuation note
    /// that goes into the conversation once the partial iteration completes.
    pending_resume_note: Option<String>,
    require_tool_call: bool,
}

impl Agent {
    pub(crate) fn new(
        config: AgentConfig,
        command_rx: mpsc::Receiver<Command>,
        message_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let mut streams: StreamMap<StreamKey, EventStream> = StreamMap::new();
        let input_stream = ReceiverStream::new(command_rx)
            .map(StreamEvent::Command)
            .chain(futures::stream::once(async { StreamEvent::InputClosed }));
        streams.insert(StreamKey::Input, Box::pin(input_stream));

        let context_limit = config.context_window.or_else(|| config.llm.context_window());

        let mut tool_executions = ToolExecutions::default();
        if let Some(handle) = &config.mcp {
            tool_executions.set_tool_output_cap(handle.tool_output_cap_arc());
        }

        Self {
            llm: config.llm,
            context: config.context,
            mcp: config.mcp,
            message_tx,
            observers: config.observers,
            streams,
            tool_timeout: config.tool_timeout,
            tool_policy: config.tool_policy,
            token_tracker: TokenTracker::new(context_limit),
            compaction_config: config.compaction_config,
            auto_continue: config.auto_continue,
            retry_config: config.retry_config,
            repetition: RepetitionTracker::new(config.repetition),
            tool_executions,
            pending_inputs: VecDeque::new(),
            queued_inputs: VecDeque::new(),
            context_window: config.context_window,
            max_turns: config.max_turns,
            turns_started: 0,
            prompt_cache: config.prompt_cache,
            turn_active: false,
            llm_call_active: false,
            active_compaction: None,
            active_model: None,
            session_usage: config.session_usage,
            stream_resumes: 0,
            pending_resume_note: None,
            require_tool_call: config.require_tool_call,
        }
    }

    pub fn current_model_display_name(&self) -> String {
        self.llm.display_name()
    }

    /// Get a reference to the token tracker
    pub fn token_tracker(&self) -> &TokenTracker {
        &self.token_tracker
    }

    pub async fn run(mut self) {
        let mut state = IterationState::default();
        let mut input_closed = false;
        self.emit_tool_definitions().await;

        while let Some((stream_key, event)) = self.streams.next().await {
            match event {
                StreamEvent::Command(Command::UserCommand(UserCommand::Cancel)) => {
                    self.on_user_cancel(&mut state).await;
                }

                StreamEvent::Command(Command::UserCommand(UserCommand::ClearContext)) => {
                    self.on_user_clear_context(&mut state).await;
                }

                StreamEvent::Command(Command::UserCommand(UserCommand::Text { message_id, content })) => {
                    if self.is_busy() {
                        self.queued_inputs.push_back(QueuedInput::User { message_id, content });
                    } else {
                        self.begin_turn(QueuedInput::User { message_id, content }, &mut state).await;
                    }
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::SwitchModel(new_provider))) => {
                    self.on_switch_model(new_provider).await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::UpdateTools(tools))) => {
                    self.context.set_tools(tools);
                    self.emit_tool_definitions().await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::UpdateMcpInstructions { server, body })) => {
                    self.on_update_instruction(server, body).await;
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::SetReasoningEffort(effort))) => {
                    self.context.set_reasoning_effort(effort.unwrap_or_default());
                }

                StreamEvent::Command(Command::AgentCommand(AgentCommand::ReplaceConversation(messages))) => {
                    self.on_replace_conversation(messages, &mut state).await;
                }

                StreamEvent::InputClosed => {
                    input_closed = true;
                }

                StreamEvent::LlmRequestStarted { attempt } => {
                    self.begin_chat_call(attempt).await;
                }

                StreamEvent::Llm(llm_event) => {
                    self.on_llm_event(llm_event, &mut state).await;
                }

                StreamEvent::ToolExecution(tool_event) => {
                    let StreamKey::Tool(tool_id) = stream_key else {
                        unreachable!("tool events must come from a tool stream")
                    };
                    self.on_tool_execution_event(tool_id, tool_event, &mut state).await;
                }

                StreamEvent::Compaction(result) => {
                    self.on_compaction_complete(result).await;
                }
            }

            if state.is_complete(self.tool_executions.has_foreground())
                && let Some(id) = state.current_message_id.take()
            {
                let iteration = std::mem::take(&mut state);
                self.on_iteration_complete(id, iteration).await;
            }

            if input_closed && !self.turn_active && !self.is_busy() {
                if self.tool_executions.is_empty() {
                    break;
                }
                self.abort_in_flight_work(ToolAbortPolicy::CancelAll).await;
            }
        }

        tracing::debug!("Agent task shutting down - input channel closed");
    }

    async fn on_iteration_complete(&mut self, id: MessageId, iteration: IterationState) {
        let IterationState {
            message_content,
            reasoning_summary_text,
            encrypted_reasoning,
            tool_calls,
            completed_tool_calls,
            stop_reason,
            ..
        } = iteration;
        let has_tool_calls = !completed_tool_calls.is_empty();
        let has_content = !message_content.is_empty() || !reasoning_summary_text.is_empty() || has_tool_calls;
        let has_queued_input = !self.queued_inputs.is_empty();
        let declares_end_turn = matches!(stop_reason, Some(StopReason::EndTurn));
        let missing_tool_call = self.require_tool_call && !has_tool_calls && !has_queued_input && !declares_end_turn;
        let should_auto_continue = self.auto_continue.should_continue(stop_reason.as_ref(), missing_tool_call);

        if has_content {
            let reasoning = AssistantReasoning::from_parts(reasoning_summary_text.clone(), encrypted_reasoning);
            self.context.push_assistant_turn(id.clone(), &message_content, reasoning, completed_tool_calls);

            self.emit(AgentEvent::text(&id, &message_content, StreamState::Complete)).await;

            if !reasoning_summary_text.is_empty() {
                self.emit(AgentEvent::thought(&id, &reasoning_summary_text, StreamState::Complete)).await;
            }
        }

        let signature = iteration_signature(&message_content, &reasoning_summary_text, &tool_calls);

        // A queued user input is progress: the model has been told something
        // new, so any previous repetition streak is moot.
        // A mid-stream resume is also a continuation, not a new attempt at
        // the same prompt: skip the repetition check so a stuck cut can
        // still consume the configured retry budget instead of being aborted
        // early as a "repetition loop".
        if self.pending_resume_note.is_some() {
            // The partial text is the cut's tail of an in-progress reply; the
            // resume will re-issue the same context, so the next iteration's
            // signature would be identical and trip the detector unfairly.
        } else if has_queued_input {
            self.repetition.reset();
        } else if self.repetition.observe(signature.as_deref()) {
            let observed = self.repetition.count();
            tracing::warn!(observed, "repetition loop detected; ending turn");
            let error = format!(
                "repetition loop detected: the model emitted identical assistant output/tool call {observed} times in a row; stopped the turn"
            );
            self.auto_continue.reset();
            self.finish_turn(TurnOutcome::Failed { error }).await;
            return;
        }

        if let Some(note) = self.pending_resume_note.take() {
            self.inject_resume_note(note).await;
            self.start_next_turn().await;
            return;
        }

        if has_queued_input || has_tool_calls {
            self.auto_continue.reset();
            self.start_next_turn().await;
        } else if should_auto_continue {
            self.auto_continue.advance();
            tracing::info!(
                "LLM stopped with {:?}, auto-continuing (attempt {}/{})",
                stop_reason,
                self.auto_continue.count,
                self.auto_continue.max
            );

            self.inject_continuation_prompt(stop_reason.as_ref()).await;
            self.start_next_turn().await;
        } else if missing_tool_call {
            tracing::debug!(
                "LLM ended a turn without a tool call after exhausting the auto-continue budget; failing turn"
            );
            self.auto_continue.reset();
            self.finish_turn(TurnOutcome::Failed { error: "turn ended without a tool call".to_string() }).await;
        } else {
            tracing::debug!("LLM completed turn with stop reason: {:?}", stop_reason);
            self.auto_continue.reset();
            self.finish_turn(TurnOutcome::Completed).await;
        }
    }

    async fn start_next_turn(&mut self) {
        debug_assert!(self.pending_inputs.is_empty());
        self.pending_inputs.append(&mut self.queued_inputs);
        if self.compaction_needed() {
            self.begin_compaction().await;
        } else {
            self.start_chat_turn().await;
        }
    }

    async fn start_chat_turn(&mut self) {
        // Enforce the per-run turn cap before issuing the LLM call so the
        // tool-call loop cannot run unbounded. Abort any in-flight work
        // (foreground tools would have retired before this method is reached
        // from `on_iteration_complete`; this guards the compaction path too).
        if let Some(max) = self.max_turns
            && self.turns_started >= max
        {
            tracing::info!(max_turns = max, "Turn cap reached; ending run");
            self.abort_in_flight_work(ToolAbortPolicy::CancelAll).await;
            self.pending_inputs.clear();
            self.queued_inputs.clear();
            self.auto_continue.reset();
            self.finish_turn(TurnOutcome::MaxTurnsReached { max_turns: max }).await;
            return;
        }
        self.turns_started += 1;
        self.commit_pending_inputs().await;
        self.start_llm_stream(None, 0).await;
    }

    async fn on_user_cancel(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work(ToolAbortPolicy::PreserveBackgroundAcknowledgements).await;
        self.commit_pending_inputs().await;
        self.queued_inputs.retain(|input| matches!(input, QueuedInput::TaskOutcome(_)));
        self.commit_queued_inputs().await;
        *state = IterationState::default();
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn discard_in_flight_work(&mut self, state: &mut IterationState) {
        self.abort_in_flight_work(ToolAbortPolicy::CancelAll).await;
        self.pending_inputs.clear();
        self.queued_inputs.clear();
        self.auto_continue.reset();
        self.repetition.reset();
        *state = IterationState::default();
    }

    async fn on_user_clear_context(&mut self, state: &mut IterationState) {
        self.discard_in_flight_work(state).await;
        self.context.clear_conversation();
        self.token_tracker.reset_current_usage();
        self.emit(AgentEvent::Context(ContextEvent::Cleared)).await;
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn on_replace_conversation(&mut self, messages: Vec<ChatMessage>, state: &mut IterationState) {
        self.discard_in_flight_work(state).await;
        self.context.replace_conversation(messages);
        self.emit(self.context_usage_message()).await;
        self.finish_turn(TurnOutcome::Cancelled).await;
    }

    async fn begin_turn(&mut self, input: QueuedInput, state: &mut IterationState) {
        *state = IterationState::default();
        self.auto_continue.reset();
        self.repetition.reset();
        self.turns_started = 0;
        self.stream_resumes = 0;
        self.pending_resume_note = None;
        self.turn_active = true;
        let content = input.content_blocks();
        self.emit(AgentEvent::Turn(TurnEvent::Started { content })).await;
        self.queued_inputs.push_back(input);
        self.start_next_turn().await;
    }

    async fn enqueue_task_outcome(&mut self, outcome: TaskOutcome, state: &mut IterationState) {
        let input = QueuedInput::TaskOutcome(Box::new(outcome));
        if self.is_busy() {
            self.queued_inputs.push_back(input);
        } else {
            self.begin_turn(input, state).await;
        }
    }

    async fn on_update_instruction(&mut self, server: String, body: Option<String>) {
        self.prompt_cache.update_mcp_instruction(server, body);
        match self.prompt_cache.render().await {
            Ok(content) => self.context.set_system_content(content),
            Err(e) => tracing::warn!("Failed to rebuild system prompt after instructions update: {e}"),
        }
    }

    async fn on_switch_model(&mut self, new_provider: Box<dyn StreamingModelProvider>) {
        let previous = self.llm.display_name();
        let new_context_limit = self.context_window.or_else(|| new_provider.context_window());
        self.llm = Arc::from(new_provider);
        self.token_tracker.reset_current_usage();
        self.token_tracker.set_context_limit(new_context_limit);
        let new = self.llm.display_name();
        self.emit(AgentEvent::Model(ModelEvent::Switched { previous, new })).await;

        self.emit(self.context_usage_message()).await;
    }

    async fn start_llm_stream(&mut self, delay: Option<Duration>, attempt: u32) {
        self.refresh_prompt_cache_key();
        self.streams.remove(&StreamKey::Llm);
        let stream: EventStream = match delay {
            None => {
                self.begin_chat_call(attempt).await;
                Box::pin(self.llm.stream_response(&self.context).map(StreamEvent::Llm))
            }
            Some(delay) => {
                self.emit(AgentEvent::Turn(TurnEvent::RetryScheduled {
                    purpose: LlmCallPurpose::Chat,
                    attempt,
                    max_attempts: self.retry_config.max_attempts,
                    delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                }))
                .await;
                let llm = Arc::clone(&self.llm);
                let context = self.context.clone();
                Box::pin(async_stream::stream! {
                    sleep(delay).await;
                    yield StreamEvent::LlmRequestStarted { attempt };
                    let mut inner = llm.stream_response(&context);
                    while let Some(item) = inner.next().await {
                        yield StreamEvent::Llm(item);
                    }
                })
            }
        };
        self.streams.insert(StreamKey::Llm, stream);
    }

    async fn on_llm_error(&mut self, error: LlmError, state: &mut IterationState) {
        let attempts_used = state.retry_attempt + self.stream_resumes;
        let will_retry = error.is_retryable() && attempts_used < self.retry_config.max_attempts;
        let outcome = LlmCallOutcome::from_llm_error(&error, will_retry);
        let error_message = error.to_string();
        self.finish_chat_call(outcome).await;

        if !will_retry {
            self.finish_turn(TurnOutcome::Failed { error: error_message }).await;
            return;
        }

        if self.retry_config.resume_partial && state.has_partial_output() {
            self.resume_interrupted_call(&error, state).await;
            return;
        }

        state.retry_attempt += 1;
        let delay = self.retry_config.compute_delay(state.retry_attempt);

        tracing::warn!(
            attempt = state.retry_attempt,
            max_attempts = self.retry_config.max_attempts,
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            error = %error,
            "Retrying LLM request after transient failure"
        );

        self.tool_executions.retire_foreground();
        self.start_llm_stream(Some(delay), state.retry_attempt).await;
    }

    /// Keep what an interrupted call already produced instead of re-sending
    /// the request from scratch. The partial text and reasoning are committed
    /// as the assistant's turn, tool calls that finished streaming keep
    /// running and their results are kept, and once they settle the model is
    /// asked to continue from the cut. A tool call whose arguments were still
    /// streaming never ran; its partial arguments are replayed in the note so
    /// the model can issue it again in full.
    async fn resume_interrupted_call(&mut self, error: &LlmError, state: &mut IterationState) {
        self.stream_resumes += 1;
        let attempt = state.retry_attempt + self.stream_resumes;

        tracing::warn!(
            attempt,
            max_attempts = self.retry_config.max_attempts,
            kept_text_bytes = state.message_content.len(),
            kept_tool_calls = state.started_tool_calls,
            cut_tool_calls = state.streaming_tool_calls.len(),
            error = %error,
            "Resuming LLM response after mid-stream interruption"
        );

        self.emit(AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: LlmCallPurpose::Chat,
            attempt,
            max_attempts: self.retry_config.max_attempts,
            delay_ms: 0,
        }))
        .await;

        // Drop the cut stream so nothing it still yields lands on this call.
        self.streams.remove(&StreamKey::Llm);
        let cut_calls = std::mem::take(&mut state.streaming_tool_calls);
        self.pending_resume_note = Some(resume_note(state, &cut_calls));
        // Partial reasoning is not kept: a thinking block cut before its
        // signature is rejected by providers that verify one.
        state.reasoning_summary_text.clear();
        state.encrypted_reasoning = None;
        state.llm_done = true;
        state.stop_reason = None;
    }

    async fn inject_resume_note(&mut self, note: String) {
        let message_id = MessageId::new();
        let content = vec![llm::ContentBlock::text(note)];
        self.context.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        self.emit(AgentEvent::Turn(TurnEvent::AutoContinue {
            attempt: self.stream_resumes,
            max_attempts: self.retry_config.max_attempts,
            message_id,
            content,
        }))
        .await;
    }

    fn is_busy(&self) -> bool {
        self.streams.contains_key(&StreamKey::Llm)
            || self.streams.contains_key(&StreamKey::Compaction)
            || self.tool_executions.has_foreground()
    }

    async fn abort_in_flight_work(&mut self, tool_policy: ToolAbortPolicy) {
        if self.llm_call_active {
            self.finish_chat_call(LlmCallOutcome::Cancelled).await;
        }
        if self.streams.remove(&StreamKey::Compaction).is_some() {
            let compaction_id = self.active_compaction.take().expect("active compaction stream has an identity");
            self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Compaction,
                outcome: LlmCallOutcome::Cancelled,
            }))
            .await;
            self.emit(AgentEvent::Context(ContextEvent::CompactionEnded {
                compaction_id,
                outcome: CompactionOutcome::Cancelled,
            }))
            .await;
        }
        self.streams.remove(&StreamKey::Llm);
        for tool_id in self.tool_executions.abort(&tool_policy) {
            self.streams.remove(&StreamKey::Tool(tool_id));
        }
    }

    /// Inject a continuation prompt when the LLM stops due to a resumable reason.
    async fn inject_continuation_prompt(&mut self, stop_reason: Option<&StopReason>) {
        let reason = stop_reason.map_or_else(|| "Unknown".to_string(), |reason| format!("{reason:?}"));
        let message_id = MessageId::new();
        let content = vec![llm::ContentBlock::text(format!(
            "<system-notification>The LLM API stopped with reason '{reason}'. Continue from where you left off and finish your task.</system-notification>"
        ))];
        self.context.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        self.emit(AgentEvent::Turn(TurnEvent::AutoContinue {
            attempt: self.auto_continue.count,
            max_attempts: self.auto_continue.max,
            message_id,
            content,
        }))
        .await;
    }

    async fn on_llm_event(&mut self, result: Result<LlmResponse, LlmError>, state: &mut IterationState) {
        use LlmResponse::{
            Done, EncryptedReasoning, Error, Reasoning, Start, Text, ToolRequestArg, ToolRequestComplete,
            ToolRequestStart, Usage,
        };

        let response = match result {
            Ok(response) => response,
            Err(e) => {
                self.on_llm_error(e, state).await;
                return;
            }
        };

        match response {
            Start => state.on_llm_start(MessageId::new()),

            Text { chunk } => {
                self.handle_llm_text(chunk, state).await;
            }

            Reasoning { chunk } => {
                state.reasoning_summary_text.push_str(&chunk);
                if let Some(id) = state.current_message_id.clone() {
                    self.emit(AgentEvent::thought(&id, &chunk, StreamState::Partial)).await;
                }
            }

            EncryptedReasoning { id, content } => {
                if let Some(model) = self.active_model.clone() {
                    state.encrypted_reasoning = Some(EncryptedReasoningContent { id, model, content });
                }
            }

            ToolRequestStart { id, name } => {
                state.streaming_tool_calls.push(ToolCallRequest {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: String::new(),
                });
                let request = ToolCallRequest { id, name, arguments: String::new() };
                self.emit(AgentEvent::Tool(ToolEvent::Call { request })).await;
            }

            ToolRequestArg { id, chunk } => {
                if let Some(call) = state.streaming_tool_calls.iter_mut().find(|call| call.id == id) {
                    call.arguments.push_str(&chunk);
                }
                self.emit(AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: id, chunk })).await;
            }

            ToolRequestComplete { tool_call } => {
                state.streaming_tool_calls.retain(|call| call.id != tool_call.id);
                state.started_tool_calls += 1;
                self.handle_tool_completion(tool_call, state).await;
            }

            Done { stop_reason } => {
                self.stream_resumes = 0;
                state.llm_done = true;
                state.stop_reason = stop_reason;
                self.finish_chat_call(LlmCallOutcome::Completed {
                    stop_reason: state.stop_reason.clone(),
                    usage: state.call_usage.take(),
                })
                .await;
            }

            Error { message } => {
                self.finish_chat_call(LlmCallOutcome::failed(message.clone(), false)).await;
                self.finish_turn(TurnOutcome::Failed { error: message }).await;
            }

            Usage { tokens: sample } => {
                self.handle_llm_usage(sample, state).await;
            }
        }
    }

    async fn handle_llm_text(&mut self, chunk: String, state: &mut IterationState) {
        state.message_content.push_str(&chunk);

        if let Some(id) = state.current_message_id.clone() {
            self.emit(AgentEvent::text(&id, &chunk, StreamState::Partial)).await;
        }
    }

    async fn handle_tool_completion(&mut self, tool_call: ToolCallRequest, state: &mut IterationState) {
        state.record_tool_call(&tool_call);
        let tool_id = tool_call.id.clone();

        if let Some(reason) = self.tool_policy.refuse(&tool_call) {
            let reason_for_event = reason.clone();
            self.record_tool_refusal(tool_call.clone(), reason, state).await;
            self.emit(AgentEvent::Tool(ToolEvent::Refused { request: tool_call, reason: reason_for_event })).await;
            return;
        }

        let cancel = self.tool_executions.start(tool_call.clone());
        tracing::debug!("Tool execution started: {} ({})", tool_call.name, tool_id);
        self.emit(AgentEvent::Tool(ToolEvent::ExecutionStarted {
            tool_id: tool_id.clone(),
            tool_name: tool_call.name.clone(),
        }))
        .await;

        let Some(mcp) = self.mcp.clone() else {
            let stream = futures::stream::once(async {
                StreamEvent::ToolExecution(ToolCallEvent::Complete(Err(CallToolError::Unavailable {
                    message: "MCP runtime is not available".to_string(),
                })))
            });
            self.streams.insert(StreamKey::Tool(tool_id), Box::pin(stream));
            return;
        };

        let trace_context = self.observers.iter().find_map(|observer| observer.tool_trace_context(&tool_id));
        let options = CallToolOptions {
            timeout: self.tool_timeout,
            meta: trace_context.as_ref().map(TraceContext::to_meta),
            cancel,
        };
        let stream =
            mcp.call_model_visible(tool_call.name, &tool_call.arguments, options).map(StreamEvent::ToolExecution);
        self.streams.insert(StreamKey::Tool(tool_id), Box::pin(stream));
    }

    async fn record_tool_refusal(&mut self, request: ToolCallRequest, reason: String, state: &mut IterationState) {
        let refusal_text = format!("Tool call `{name}` was refused by policy: {reason}", name = request.name,);
        state.completed_tool_calls.push(Ok(ToolCallResult {
            id: request.id.clone(),
            name: request.name.clone(),
            arguments: request.arguments.clone(),
            result: refusal_text,
        }));
        self.context.add_message(refusal_context_message(&request, &reason));
    }

    async fn handle_llm_usage(&mut self, sample: TokenUsage, state: &mut IterationState) {
        state.call_usage = Some(sample);
        self.token_tracker.record_usage(sample);
        let ratio_pct = self.token_tracker.usage_ratio().map(|r| r * 100.0);
        let remaining = self.token_tracker.tokens_remaining();
        tracing::debug!(?sample, ?ratio_pct, ?remaining, "Token usage");

        self.emit(self.context_usage_message()).await;
        self.emit_session_usage(LlmCallPurpose::Chat, sample).await;
    }

    async fn emit_session_usage(&mut self, purpose: LlmCallPurpose, tokens: TokenUsage) {
        let model = ModelIdentity::of(self.active_model.as_ref());
        let event = self.session_usage.record(purpose, model, tokens);
        self.emit(AgentEvent::SessionUsage(event)).await;
    }

    fn context_usage_message(&self) -> AgentEvent {
        AgentEvent::Context(ContextEvent::UsageUpdated { usage: self.token_tracker.snapshot().clone() })
    }

    fn compaction_needed(&self) -> bool {
        self.compaction_config.as_ref().is_some_and(|config| {
            self.token_tracker.needs_compaction(self.context.estimated_token_count(), config.threshold)
        })
    }

    async fn begin_compaction(&mut self) {
        tracing::info!("Starting context compaction - {} messages", self.context.message_count());
        let compaction_id = CompactionId::new();
        self.active_compaction = Some(compaction_id.clone());
        self.emit(AgentEvent::Context(ContextEvent::CompactionStarted {
            compaction_id,
            message_count: self.context.message_count(),
        }))
        .await;
        let started = self.begin_llm_call(LlmCallPurpose::Compaction, 0);
        self.emit(started).await;

        let compactor = Compactor::new(self.llm.clone());
        let context = self.context.clone();
        let stream: EventStream =
            Box::pin(futures::stream::once(async move { StreamEvent::Compaction(compactor.compact(context).await) }));
        self.streams.insert(StreamKey::Compaction, stream);
    }

    async fn on_compaction_complete(&mut self, result: Result<CompactionResult, CompactionError>) {
        let compaction_id = self.active_compaction.take().expect("completed compaction has an identity");
        if let Ok(result) = &result
            && let Some(usage) = result.usage
        {
            self.emit_session_usage(LlmCallPurpose::Compaction, usage).await;
        }
        let outcome = match &result {
            Ok(result) => LlmCallOutcome::Completed { stop_reason: None, usage: result.usage },
            Err(e) => LlmCallOutcome::failed(e.to_string(), false),
        };
        self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Compaction, outcome })).await;

        match result {
            Ok(result) => {
                tracing::info!("Context compacted: {} messages removed", result.messages_removed);
                let message_id = MessageId::new();
                self.context = self.context.with_compacted_summary(message_id.clone(), &result.summary);
                self.token_tracker.reset_current_usage();
                self.emit(AgentEvent::Context(ContextEvent::CompactionResult {
                    compaction_id: compaction_id.clone(),
                    message_id,
                    summary: result.summary,
                    messages_removed: result.messages_removed,
                }))
                .await;
                self.emit(AgentEvent::Context(ContextEvent::CompactionEnded {
                    compaction_id,
                    outcome: CompactionOutcome::Completed,
                }))
                .await;
            }
            Err(e) => {
                tracing::warn!("Context compaction failed: {e}");
                self.emit(AgentEvent::Context(ContextEvent::CompactionEnded {
                    compaction_id,
                    outcome: CompactionOutcome::Failed { error: e.to_string() },
                }))
                .await;
            }
        }

        self.start_chat_turn().await;
    }

    async fn on_tool_execution_event(&mut self, tool_id: String, event: ToolCallEvent, state: &mut IterationState) {
        match self.tool_executions.on_event(&tool_id, event) {
            ToolExecutionUpdate::Event(event) => {
                if let ToolEvent::SubAgentProgress { payload, .. } = &event
                    && let AgentEvent::SessionUsage(child) = &payload.event
                {
                    let folded = self.session_usage.record_child(&payload.task_id, child.clone());
                    self.emit(AgentEvent::SessionUsage(folded)).await;
                }
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::Completed { result, event } => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                state.completed_tool_calls.push(result);
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::TaskCreated { result, event } => {
                state.completed_tool_calls.push(Ok(result));
                self.emit(AgentEvent::Tool(event)).await;
            }
            ToolExecutionUpdate::TaskCompleted(outcome) => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                self.enqueue_task_outcome(outcome, state).await;
            }
            ToolExecutionUpdate::TaskCancelled(outcome) => {
                self.streams.remove(&StreamKey::Tool(tool_id));
                self.record_task_outcome(outcome).await;
            }
            ToolExecutionUpdate::Retired => {
                self.streams.remove(&StreamKey::Tool(tool_id));
            }
            ToolExecutionUpdate::Ignored => {
                tracing::debug!(%tool_id, "Ignoring unexpected tool execution event");
            }
        }
    }

    async fn record_task_outcome(&mut self, outcome: TaskOutcome) {
        self.context.add_message(outcome.context_message());
        self.emit(AgentEvent::Tool(outcome.into())).await;
    }

    fn refresh_prompt_cache_key(&mut self) {
        let key = derive_prompt_cache_key(self.llm.as_ref(), &self.context);
        self.context.set_prompt_cache_key(Some(key));
    }

    async fn commit_pending_inputs(&mut self) {
        let inputs = std::mem::take(&mut self.pending_inputs);
        self.commit_inputs(inputs).await;
    }

    async fn commit_queued_inputs(&mut self) {
        let inputs = std::mem::take(&mut self.queued_inputs);
        self.commit_inputs(inputs).await;
    }

    async fn commit_inputs(&mut self, inputs: VecDeque<QueuedInput>) {
        for input in inputs {
            match input {
                QueuedInput::User { message_id, content } => {
                    self.context.add_message(ChatMessage::user_with_id(message_id, content));
                }
                QueuedInput::TaskOutcome(outcome) => self.record_task_outcome(*outcome).await,
            }
        }
    }

    async fn emit_tool_definitions(&mut self) {
        let tools = self.context.tools().clone();
        if !tools.is_empty() {
            self.emit(AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools })).await;
        }
    }

    async fn emit(&mut self, message: AgentEvent) {
        for observer in &mut self.observers {
            observer.on_event(&message);
        }

        if let Err(e) = self.message_tx.send(message).await {
            tracing::warn!("Failed to send agent message: {e:?}");
        }
    }

    async fn finish_turn(&mut self, outcome: TurnOutcome) {
        if std::mem::take(&mut self.turn_active) {
            self.emit(AgentEvent::turn_ended(outcome)).await;
        }
    }

    async fn begin_chat_call(&mut self, attempt: u32) {
        self.llm_call_active = true;
        let started = self.begin_llm_call(LlmCallPurpose::Chat, attempt);
        if let Some(system_prompt) = self.context.system_content() {
            for observer in &mut self.observers {
                observer.on_system_prompt(system_prompt);
            }
        }
        self.emit(started).await;
    }

    async fn finish_chat_call(&mut self, outcome: LlmCallOutcome) {
        if std::mem::take(&mut self.llm_call_active) {
            self.emit(AgentEvent::Turn(TurnEvent::LlmCallEnded { purpose: LlmCallPurpose::Chat, outcome })).await;
        }
    }

    fn begin_llm_call(&mut self, purpose: LlmCallPurpose, attempt: u32) -> AgentEvent {
        self.active_model = self.llm.model();
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose,
            model: ModelIdentity::of(self.active_model.as_ref()),
            display_name: self.llm.display_name(),
            attempt,
            max_attempts: self.retry_config.max_attempts,
        })
    }
}

pub(crate) struct AutoContinue {
    max: u32,
    count: u32,
}

impl AutoContinue {
    pub(crate) fn new(max: u32) -> Self {
        Self { max, count: 0 }
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn should_continue(&self, stop_reason: Option<&StopReason>, missing_tool_call: bool) -> bool {
        let resumable_stop = matches!(stop_reason, Some(StopReason::Length));
        (resumable_stop || missing_tool_call) && self.count < self.max
    }

    fn advance(&mut self) {
        self.count += 1;
    }
}

/// Longest partial tool-call argument replayed in a resume note.
const MAX_REPLAYED_ARGUMENT_BYTES: usize = 16 * 1024;

fn resume_note(state: &IterationState, cut_calls: &[ToolCallRequest]) -> String {
    let mut note = String::from(
        "<system-notification>Your previous response was cut off by a network interruption before it finished.",
    );
    if !state.message_content.trim().is_empty() {
        note.push_str(" Everything you wrote before the cut is kept above.");
    }
    if state.started_tool_calls > 0 {
        note.push_str(" The tool calls that were complete before the cut ran, and their results are above.");
    }
    for call in cut_calls {
        let arguments = truncate_to_boundary(&call.arguments, MAX_REPLAYED_ARGUMENT_BYTES);
        let _ = write!(
            note,
            "\n\nYou were in the middle of a `{}` tool call when the cut happened, so it did NOT run. Its arguments up to the cut were:\n```\n{}\n```\nIssue that call again with its complete arguments.",
            call.name, arguments
        );
    }
    note.push_str(
        "\n\nContinue exactly where you stopped. Do not repeat text you already wrote and do not redo work that is already done.</system-notification>",
    );
    note
}

fn truncate_to_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Debug, Default)]
struct IterationState {
    current_message_id: Option<MessageId>,
    message_content: String,
    reasoning_summary_text: String,
    encrypted_reasoning: Option<EncryptedReasoningContent>,
    /// Requested tool calls for this iteration, captured in the order the
    /// model emitted them. Used by the repetition detector to fingerprint
    /// re-issued tool calls across iterations; the `id` is intentionally
    /// excluded from the signature since providers assign fresh ids per call.
    tool_calls: Vec<ToolCallRequest>,
    completed_tool_calls: Vec<Result<ToolCallResult, ToolCallError>>,
    llm_done: bool,
    stop_reason: Option<StopReason>,
    retry_attempt: u32,
    call_usage: Option<TokenUsage>,
    /// Tool calls whose arguments are still streaming in this call.
    streaming_tool_calls: Vec<ToolCallRequest>,
    /// Tool calls of this call that finished streaming and were started.
    started_tool_calls: usize,
}

impl IterationState {
    fn on_llm_start(&mut self, message_id: MessageId) {
        self.current_message_id = Some(message_id);
        self.message_content.clear();
        self.reasoning_summary_text.clear();
        self.encrypted_reasoning = None;
        self.stop_reason = None;
        self.call_usage = None;
        self.tool_calls.clear();
        self.streaming_tool_calls.clear();
        self.started_tool_calls = 0;
    }

    /// Whether an interrupted call already produced something worth keeping.
    /// A cut before any visible output (none, or reasoning only) is retried
    /// from scratch.
    fn has_partial_output(&self) -> bool {
        self.current_message_id.is_some()
            && (!self.message_content.trim().is_empty()
                || self.started_tool_calls > 0
                || !self.streaming_tool_calls.is_empty())
    }

    fn is_complete(&self, has_foreground_tools: bool) -> bool {
        self.llm_done && !has_foreground_tools
    }

    fn record_tool_call(&mut self, request: &ToolCallRequest) {
        self.tool_calls.push(request.clone());
    }
}
