use super::agent::{AgentConfig, AutoContinue, RetryConfig};
use super::repetition_config::RepetitionConfig;
use super::tool_policy::ToolPolicy;
use crate::agent_spec::AgentSpec;
use crate::context::{CompactionConfig, SessionUsageTracker};
use crate::core::{Agent, AgentDeps, Prompt, PromptCache, Result};
use crate::events::{AgentEvent, AgentObserver, Command};
use crate::mcp::McpHandle;
use llm::parser::ModelProviderParser;
use llm::{ChatMessage, Context, ModelSettings, SessionUsageEvent, StreamingModelProvider, ToolDefinition};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

/// Handle for communicating with a running Agent
pub struct AgentHandle {
    handle: JoinHandle<()>,
}

impl AgentHandle {
    /// Abort the agent task immediately.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Returns `true` if the agent task has finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Wait for the agent task to complete.
    pub async fn await_completion(self) {
        let _ = self.handle.await;
    }
}

pub struct AgentBuilder {
    llm: Arc<dyn StreamingModelProvider>,
    prompts: Vec<Prompt>,
    tool_definitions: Vec<ToolDefinition>,
    initial_messages: Vec<ChatMessage>,
    mcp: Option<McpHandle>,
    channel_capacity: usize,
    tool_timeout: Duration,
    compaction_config: Option<CompactionConfig>,
    max_auto_continues: u32,
    require_tool_call: bool,
    retry_config: RetryConfig,
    repetition_config: RepetitionConfig,
    context_window: Option<u32>,
    max_turns: Option<u32>,
    model_settings: ModelSettings,
    observers: Vec<Box<dyn AgentObserver>>,
    session_usage: SessionUsageTracker,
    session_affinity_key: String,
    tool_policy: Arc<dyn ToolPolicy>,
}

impl AgentBuilder {
    pub fn new(llm: Arc<dyn StreamingModelProvider>) -> Self {
        Self {
            llm,
            prompts: Vec::new(),
            tool_definitions: Vec::new(),
            initial_messages: Vec::new(),
            mcp: None,
            channel_capacity: 1000,
            tool_timeout: Duration::from_mins(60),
            compaction_config: Some(CompactionConfig::default()),
            max_auto_continues: 3,
            require_tool_call: false,
            retry_config: RetryConfig::default(),
            repetition_config: RepetitionConfig::default(),
            context_window: None,
            max_turns: None,
            model_settings: ModelSettings::default(),
            observers: Vec::new(),
            session_usage: SessionUsageTracker::new("agent"),
            session_affinity_key: uuid::Uuid::new_v4().to_string(),
            tool_policy: Arc::new(super::tool_policy::AllowAllTools),
        }
    }

    /// Create a builder from a resolved `AgentSpec`.
    ///
    /// The LLM provider is derived from `spec.model` via `ModelProviderParser`.
    /// `base_prompts` are prepended before the spec's own prompts.
    pub async fn from_spec(spec: &AgentSpec, base_prompts: Vec<Prompt>, deps: &AgentDeps) -> Result<Self> {
        let parser = ModelProviderParser::default().with_provider_connections(spec.provider_connections.clone());
        let parser = match deps.oauth_credential_store.clone() {
            Some(store) => parser.with_codex_provider(store),
            None => parser,
        };
        let (provider, _) = parser.parse(&spec.model).await?;
        let mut builder = Self::new(Arc::from(provider))
            .context_window(spec.context_window)
            .max_turns(spec.max_turns)
            .model_settings(spec.model_settings.clone())
            .session_usage(SessionUsageTracker::new(&spec.name));

        if let Some(key) = &deps.session_affinity_key {
            builder = builder.session_affinity_key(key.clone());
        }
        if let Some(observer) = deps.observer(&spec.name) {
            builder = builder.observer(observer);
        }

        for prompt in base_prompts {
            builder = builder.system_prompt(prompt);
        }

        for prompt in &spec.prompts {
            builder = builder.system_prompt(prompt.clone());
        }

        Ok(builder)
    }

    /// Add a prompt to the system prompt.
    ///
    /// Multiple prompts are concatenated with double newlines.
    pub fn system_prompt(mut self, prompt: Prompt) -> Self {
        self.prompts.push(prompt);
        self
    }

    pub fn tools(mut self, mcp: McpHandle, tools: Vec<ToolDefinition>) -> Self {
        self.tool_definitions = tools;
        self.mcp = Some(mcp);
        self
    }

    /// Set the timeout for tool execution
    ///
    /// If a tool does not return a result within this duration, it will be marked as failed
    /// and the agent will continue processing.
    ///
    /// Default: 60 minutes
    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    /// Configure context compaction settings.
    ///
    /// By default, agents automatically compact context when token usage exceeds
    /// 85% of the context window, preventing overflow during long-running tasks.
    ///
    /// # Examples
    /// ```ignore
    /// // Custom threshold
    /// agent(llm).compaction(CompactionConfig::with_threshold(0.9))
    ///
    /// // Disable compaction entirely
    /// agent(llm).compaction(CompactionConfig::disabled())
    ///
    /// // Full customization
    /// agent(llm).compaction(
    ///     CompactionConfig::with_threshold(0.85)
    ///         .keep_recent_tool_results(3)
    ///         .min_messages(20)
    /// )
    /// ```
    pub fn compaction(mut self, config: CompactionConfig) -> Self {
        self.compaction_config = Some(config);
        self
    }

    /// Disable context compaction entirely.
    ///
    /// Overflow errors from the model will be surfaced directly to callers.
    pub fn disable_compaction(mut self) -> Self {
        self.compaction_config = None;
        self
    }

    /// Configure the maximum number of auto-continue attempts.
    ///
    /// When the LLM stops without making tool calls, the agent may inject a
    /// continuation prompt and restart the LLM stream for resumable stop
    /// reasons (for example, token length limits).
    ///
    /// This setting limits how many times the agent will attempt to continue
    /// before giving up and ending the turn with [`TurnEvent::Ended`](crate::events::TurnEvent::Ended).
    ///
    /// Default: 3
    ///
    /// # Example
    /// ```ignore
    /// // Allow up to 5 auto-continue attempts
    /// agent(llm).max_auto_continues(5)
    ///
    /// // Disable auto-continue entirely
    /// agent(llm).max_auto_continues(0)
    /// ```
    pub fn max_auto_continues(mut self, max: u32) -> Self {
        self.max_auto_continues = max;
        self
    }

    /// Require the model to make at least one tool call before ending a turn.
    ///
    /// When enabled, a turn that ends with no tool call, no queued user input,
    /// and no `EndTurn` declaration from the provider is treated as a "dead
    /// turn": the first occurrence shares the existing auto-continue budget and
    /// nudges the model once via [`AgentBuilder::max_auto_continues`]; the next
    /// occurrence fails the turn with
    /// `TurnOutcome::Failed { error: "turn ended without a tool call" }`.
    ///
    /// Off by default so existing chat-style agents (where an end-turn reply
    /// without a tool call is normal) keep working unchanged.
    ///
    /// # Example
    /// ```ignore
    /// // Require a tool call on every turn, allowing one nudge before failing
    /// agent(llm).require_tool_call(true).max_auto_continues(1)
    /// ```
    pub fn require_tool_call(mut self, require: bool) -> Self {
        self.require_tool_call = require;
        self
    }

    /// Configure retry behavior for transient LLM provider failures.
    pub fn retry(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Configure the repetition detector.
    ///
    /// When the same assistant output (text and/or tool calls) is observed
    /// `config.max_repeats` times in a row within one turn, the turn is
    /// ended with [`TurnEvent::Ended`](crate::events::TurnEvent::Ended)
    /// carrying a `TurnOutcome::Failed` error that names the repetition.
    /// Setting the threshold to `0` (or using [`RepetitionConfig::disabled`])
    /// turns the detector off. Default: 3.
    pub fn repetition_config(mut self, config: RepetitionConfig) -> Self {
        self.repetition_config = config;
        self
    }

    /// Convenience for `repetition_config(RepetitionConfig { max_repeats })`.
    pub fn repetition_limit(mut self, max_repeats: u32) -> Self {
        self.repetition_config = RepetitionConfig { max_repeats };
        self
    }

    /// Override the effective model context window in tokens.
    pub fn context_window(mut self, context_window: Option<u32>) -> Self {
        self.context_window = context_window;
        self
    }

    /// Cap the number of LLM chat turns in a single run.
    ///
    /// When `Some(n)`, the agent ends the run cleanly with
    /// [`TurnOutcome::MaxTurnsReached`](crate::events::TurnOutcome::MaxTurnsReached)
    /// once `n` chat turns have been started. `None` (the default) leaves the
    /// run unbounded.
    pub fn max_turns(mut self, max: Option<u32>) -> Self {
        self.max_turns = max;
        self
    }

    /// Set the sampling controls (`temperature`, `top_p`, `max_tokens`) applied to
    /// every model call this agent makes.
    pub fn model_settings(mut self, model_settings: ModelSettings) -> Self {
        self.model_settings = model_settings;
        self
    }

    pub fn session_affinity_key(mut self, key: impl Into<String>) -> Self {
        self.session_affinity_key = key.into();
        self
    }

    /// Pre-populate the context with conversation history (e.g. from a restored session).
    ///
    /// These messages are inserted after the system prompt.
    pub fn messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.initial_messages = messages;
        self
    }

    /// Attach an observer of the agent's event stream.
    pub fn observer(mut self, observer: Box<dyn AgentObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    /// Record usage under `tracker`, which names this agent in usage events.
    pub fn session_usage(mut self, tracker: SessionUsageTracker) -> Self {
        self.session_usage = tracker;
        self
    }

    /// Install a [`ToolPolicy`] consulted before every tool call the model requests.
    /// The default [`AllowAllTools`](super::tool_policy::AllowAllTools) refuses nothing.
    pub fn tool_policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self {
        self.tool_policy = policy;
        self
    }

    /// Continue session totals from the last persisted usage event, e.g. when
    /// resuming a session.
    pub fn resume_usage(mut self, last: &SessionUsageEvent) -> Self {
        self.session_usage.resume_from(last);
        self
    }

    pub async fn spawn(self) -> Result<(Sender<Command>, Receiver<AgentEvent>, AgentHandle)> {
        let mut prompt_cache = PromptCache::new(self.prompts);
        let system_content = prompt_cache.render().await?;
        let mut messages = Vec::new();

        if !system_content.is_empty() {
            messages.push(ChatMessage::system(system_content));
        }

        messages.extend(self.initial_messages);
        let (command_tx, command_rx) = mpsc::channel::<Command>(self.channel_capacity);
        let (message_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(self.channel_capacity);
        let mut context = Context::new(messages, self.tool_definitions);
        context.set_model_settings(self.model_settings);
        context.set_session_affinity_key(Some(self.session_affinity_key));

        let config = AgentConfig {
            llm: self.llm,
            context,
            mcp: self.mcp,
            tool_timeout: self.tool_timeout,
            compaction_config: self.compaction_config,
            auto_continue: AutoContinue::new(self.max_auto_continues),
            retry_config: self.retry_config,
            repetition: self.repetition_config,
            context_window: self.context_window,
            max_turns: self.max_turns,
            prompt_cache,
            observers: self.observers,
            session_usage: self.session_usage,
            tool_policy: self.tool_policy,
            require_tool_call: self.require_tool_call,
        };

        let agent = Agent::new(config, command_rx, message_tx);
        let agent_handle = tokio::spawn(agent.run());

        Ok((command_tx, agent_event_rx, AgentHandle { handle: agent_handle }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_spec::AgentSpecExposure;
    use llm::ProviderConnectionOverrides;
    use mcp_utils::client::ToolFilter;

    #[tokio::test]
    async fn test_agent_handle_is_finished() {
        let handle = AgentHandle { handle: tokio::spawn(async {}) };
        handle.await_completion().await;
    }

    #[tokio::test]
    async fn test_agent_handle_abort() {
        let handle = AgentHandle { handle: tokio::spawn(std::future::pending::<()>()) };
        assert!(!handle.is_finished());
        handle.abort();
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn system_prompt_preserves_add_order() {
        let builder = AgentBuilder::new(Arc::new(llm::testing::FakeLlmProvider::new(vec![])))
            .system_prompt(Prompt::text("first"))
            .system_prompt(Prompt::text("second"))
            .system_prompt(Prompt::text("third"));

        let rendered = Prompt::build_all(&builder.prompts).await.unwrap();

        assert_eq!(rendered, "first\n\nsecond\n\nthird");
    }

    #[tokio::test]
    async fn from_spec_applies_context_window_and_model_settings() {
        let settings = ModelSettings { temperature: Some(0.0), max_tokens: Some(128), ..Default::default() };
        let spec = AgentSpec {
            name: "alloy".to_string(),
            description: "alloy".to_string(),
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            reasoning_effort: None,
            model_settings: settings.clone(),
            context_window: Some(200_000),
            max_turns: Some(7),
            prompts: vec![],
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        };

        let dependencies = AgentDeps::default().with_session_affinity_key("conversation-123");
        let builder = AgentBuilder::from_spec(&spec, vec![], &dependencies).await.unwrap();

        assert_eq!(builder.context_window, Some(200_000));
        assert_eq!(builder.max_turns, Some(7));
        assert_eq!(builder.model_settings, settings);
        assert_eq!(builder.session_affinity_key, "conversation-123");
    }

    #[tokio::test]
    async fn from_spec_accepts_alloy_model_specs() {
        let spec = AgentSpec {
            name: "alloy".to_string(),
            description: "alloy".to_string(),
            model: "ollama:llama3.2,llamacpp:local".to_string(),
            reasoning_effort: None,
            model_settings: ModelSettings::default(),
            context_window: None,
            max_turns: None,
            prompts: vec![],
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        };

        let builder = AgentBuilder::from_spec(&spec, vec![], &AgentDeps::default()).await;
        assert!(builder.is_ok());
    }
}
