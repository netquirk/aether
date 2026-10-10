use crate::error::CliError;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_core::core::{AgentBuilder, AgentDeps, AgentHandle, Prompt};
use aether_core::events::{AgentEvent, Command};
use aether_core::mcp::McpBuilder;
use aether_core::mcp::mcp;
use aether_core::mcp::{McpHandle, McpRuntime, McpSession};
use aether_project::ToolOutputSettings;
use aether_project::resolve_tool_output_cap;
use aether_project::tool_output_dir_from_env;
use aether_project::tool_output_max_bytes_from_env;
use llm::{ChatMessage, SessionUsageEvent, ToolDefinition};
use mcp_servers::McpBuilderExt;
use mcp_utils::client::{
    McpClientEvent, McpConnectionDetails, McpServer, OAuthHandlerFactory, ToolFilter, ToolMatcher,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::debug;

pub struct RuntimeBuilder {
    cwd: PathBuf,
    spec: AgentSpec,
    /// Top-level `toolOutput` block from the loaded settings. `None` means
    /// the settings file did not declare a cap; the CLI falls back to the
    /// 16 KiB default and env-var overrides.
    settings_tool_output: Option<ToolOutputSettings>,
    /// Extra environment variables injected into every shell command a run
    /// starts. Sourced from the top-level `shellEnvironment` block of the
    /// loaded settings; threaded into [`McpBuilder::with_shell_environment`]
    /// which merges them with the internal gateway socket inside
    /// [`spawn_mcp`].
    shell_environment: BTreeMap<String, String>,
    mcp_config_sources: Vec<McpConfigSource>,
    extra_mcp_servers: Vec<McpServer>,
    oauth_applicator: Option<Box<dyn FnOnce(McpBuilder) -> McpBuilder + Send>>,
    agent_deps: AgentDeps,
    usage_seed: Option<SessionUsageEvent>,
    /// Per-run tool denylist added on top of the agent's own `tools` block
    /// (TASK-25-123). Names go through [`ToolMatcher::name`] so the existing
    /// `with_tool_filter` deny list controls what reaches the model. Empty by
    /// default so pre-existing callers see no change.
    disable_tools: Vec<String>,
}

pub struct Runtime {
    pub agent_tx: Sender<Command>,
    pub agent_rx: Receiver<AgentEvent>,
    pub agent_handle: AgentHandle,
    pub event_rx: Receiver<McpClientEvent>,
    pub mcp_runtime: McpRuntime,
}

pub struct PromptInfo {
    pub spec: AgentSpec,
    pub tool_definitions: Vec<ToolDefinition>,
}

impl RuntimeBuilder {
    pub fn from_spec(cwd: PathBuf, spec: AgentSpec) -> Self {
        Self {
            cwd,
            spec,
            settings_tool_output: None,
            shell_environment: BTreeMap::new(),
            mcp_config_sources: Vec::new(),
            extra_mcp_servers: Vec::new(),
            oauth_applicator: None,
            agent_deps: AgentDeps::default(),
            usage_seed: None,
            disable_tools: Vec::new(),
        }
    }

    /// Set the top-level `toolOutput` block from the loaded settings. The CLI
    /// resolves the per-agent cap together with this block and the
    /// `AETHER_TOOL_OUTPUT_MAX_BYTES` / `PRAIRIE_TOOL_OUTPUT_DIR` env vars
    /// inside [`spawn_mcp`].
    pub fn settings_tool_output(mut self, tool_output: Option<ToolOutputSettings>) -> Self {
        self.settings_tool_output = tool_output;
        self
    }

    /// Set the top-level `shellEnvironment` block from the loaded settings.
    /// The CLI threads it into [`McpBuilder::with_shell_environment`] inside
    /// [`spawn_mcp`] so every shell command a run starts (the `bash` tool of
    /// the built-in `coding` MCP server) sees the configured variables
    /// merged over its process environment.
    pub fn shell_environment(mut self, vars: BTreeMap<String, String>) -> Self {
        self.shell_environment = vars;
        self
    }

    pub fn agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    /// Continue session usage totals from the last persisted usage event.
    pub fn resume_usage(mut self, last: SessionUsageEvent) -> Self {
        self.usage_seed = Some(last);
        self
    }

    /// Set MCP config source overrides. When non-empty, these completely
    /// replace any sources resolved from the agent's `AgentSpec`.
    pub fn mcp_sources(mut self, sources: Vec<McpConfigSource>) -> Self {
        self.mcp_config_sources = sources;
        self
    }

    pub fn extra_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.extra_mcp_servers = servers;
        self
    }

    pub fn oauth_handler_factory(mut self, factory: OAuthHandlerFactory) -> Self {
        self.oauth_applicator = Some(Box::new(|builder| builder.with_oauth_handler_factory(factory)));
        self
    }

    /// Per-run tool denylist (TASK-25-123). Each entry is matched against the
    /// model-facing tool name through [`ToolMatcher::name`]; an entry that
    /// does not match any tool is a silent no-op so unknown names fail
    /// gracefully. Names accumulate on top of the agent's configured `deny`
    /// list — a name already denied stays denied — and do not interact with
    /// the `allow` list (the surrounding `ToolFilter::is_tool_allowed` keeps
    /// that decision authoritative).
    pub fn disable_tools(mut self, names: Vec<String>) -> Self {
        self.disable_tools = names;
        self
    }

    pub async fn build(
        self,
        custom_prompt: Option<Prompt>,
        messages: Option<Vec<ChatMessage>>,
    ) -> Result<Runtime, CliError> {
        let deps = self.agent_deps.clone();
        let usage_seed = self.usage_seed.clone();
        let (spec, session) = self.spawn_mcp().await?;
        let mcp = session.handle().clone();

        let (agent_tx, agent_rx, agent_handle) = spawn_agent(&spec, &deps, mcp, Vec::new(), |mut agent_builder| {
            if let Some(last) = &usage_seed {
                agent_builder = agent_builder.resume_usage(last);
            }
            if let Some(prompt) = custom_prompt {
                agent_builder = agent_builder.system_prompt(prompt);
            }
            if let Some(msgs) = messages {
                agent_builder = agent_builder.messages(msgs);
            }
            agent_builder
        })
        .await?;
        let (mcp_runtime, event_rx) = session.connect_agent(agent_tx.clone()).await.split();

        Ok(Runtime { agent_tx, agent_rx, agent_handle, event_rx, mcp_runtime })
    }

    /// Spawn MCP, block until every server finishes its initial connection,
    /// then connect the agent to the session's filtered tools and MCP
    /// instructions before returning. Returns the live [`Runtime`] plus the
    /// bootstrap snapshot for callers that need the agent ready to use tools on
    /// its first turn.
    pub async fn build_ready(self, messages: Vec<ChatMessage>) -> Result<(Runtime, McpConnectionDetails), CliError> {
        let deps = self.agent_deps.clone();
        let usage_seed = self.usage_seed.clone();
        let (spec, mut session) = self.spawn_mcp().await?;
        let snapshot = session
            .block_until_ready()
            .await
            .ok_or_else(|| CliError::McpError("MCP bootstrap aborted before completion".to_string()))?;
        let mcp = session.handle().clone();

        let (agent_tx, agent_rx, agent_handle) = spawn_agent(&spec, &deps, mcp, Vec::new(), |mut agent_builder| {
            if let Some(last) = &usage_seed {
                agent_builder = agent_builder.resume_usage(last);
            }
            agent_builder.messages(messages)
        })
        .await?;
        let (mcp_runtime, event_rx) = session.connect_agent(agent_tx.clone()).await.split();

        Ok((Runtime { agent_tx, agent_rx, agent_handle, event_rx, mcp_runtime }, snapshot))
    }

    pub async fn build_prompt_info(self) -> Result<PromptInfo, CliError> {
        let (spec, mut session) = self.spawn_mcp().await?;
        let details = session
            .block_until_ready()
            .await
            .ok_or_else(|| CliError::McpError("MCP bootstrap aborted before completion".to_string()))?;
        let filtered_tools = details.tool_definitions();
        Ok(PromptInfo { spec, tool_definitions: filtered_tools })
    }

    async fn spawn_mcp(self) -> Result<(AgentSpec, McpSession), CliError> {
        let deps = self.agent_deps.clone();
        let mut filter = self.spec.tools.clone();
        apply_disabled_tools(&mut filter, &self.disable_tools);
        let mut builder = mcp(&self.cwd).with_tool_filter(filter);

        if let Some(apply_oauth) = self.oauth_applicator {
            builder = apply_oauth(builder);
        }

        builder = builder.with_agent_deps(deps).with_shell_environment(self.shell_environment).with_builtin_servers();

        if !self.extra_mcp_servers.is_empty() {
            builder = builder.with_servers(self.extra_mcp_servers);
        }

        let mcp_config_sources: Vec<McpConfigSource> = if self.mcp_config_sources.is_empty() {
            self.spec.mcp_config_sources.clone()
        } else {
            self.mcp_config_sources
        };

        if !mcp_config_sources.is_empty() {
            debug!("Loading MCP configs from: {:?}", mcp_config_sources);
            builder =
                builder.from_mcp_config_sources(&mcp_config_sources).map_err(|e| CliError::McpError(e.to_string()))?;
        }

        let tool_output_cap = resolve_tool_output_cap(
            &self.cwd,
            self.settings_tool_output.as_ref(),
            self.spec.tool_output.as_ref(),
            tool_output_max_bytes_from_env(),
            tool_output_dir_from_env(),
        );
        builder = builder.with_tool_output_cap(tool_output_cap);

        let spawn = builder.spawn().await.map_err(|e| CliError::McpError(e.to_string()))?;
        Ok((self.spec, spawn))
    }
}

/// Merge the per-run `--disable-tool NAME` denylist into the agent's own
/// `ToolFilter` (TASK-25-123). Each entry is wrapped in
/// [`ToolMatcher::name`] so the resulting deny list applies through the same
/// glob-aware machinery [`ToolFilter::is_tool_allowed`] already uses.
/// `pub(crate)` so the integration test path can also exercise it without
/// fetching the names out of the in-memory `with_tool_filter` plumbing.
pub(crate) fn apply_disabled_tools(filter: &mut ToolFilter, names: &[String]) {
    filter.deny.extend(names.iter().cloned().map(ToolMatcher::name));
}

async fn spawn_agent(
    spec: &AgentSpec,
    deps: &AgentDeps,
    mcp: McpHandle,
    tool_definitions: Vec<ToolDefinition>,
    configure: impl FnOnce(AgentBuilder) -> AgentBuilder,
) -> Result<(Sender<Command>, Receiver<AgentEvent>, AgentHandle), CliError> {
    let builder = AgentBuilder::from_spec(spec, vec![], deps)
        .await
        .map_err(|error| CliError::AgentError(error.to_string()))?
        .tools(mcp, tool_definitions);

    configure(builder).spawn().await.map_err(|error| CliError::AgentError(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ToolDefinition;
    use serde_json::json;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "test tool", json!({"type": "object"}))
    }

    #[test]
    fn apply_disabled_tools_adds_name_matchers_to_deny() {
        // The helper appends one `ToolMatcher::Name` per input string; an
        // empty filter plus two names is the canonical wiring check. We don't
        // inspect `filter.deny` directly (it is the deny that matters at
        // runtime, not a particular equality); `is_tool_allowed` is the
        // observable contract downstream code relies on.
        let mut filter = ToolFilter::default();
        apply_disabled_tools(&mut filter, &["coding__bash".to_string(), "coding__read_file".to_string()]);

        assert_eq!(filter.deny.len(), 2);
        assert!(!filter.is_tool_allowed(&tool("coding__bash")));
        assert!(!filter.is_tool_allowed(&tool("coding__read_file")));
        // The helper preserves any pre-existing deny entries.
        let mut filter = ToolFilter { deny: vec![ToolMatcher::name("preexisting")], ..ToolFilter::default() };
        apply_disabled_tools(&mut filter, &["coding__bash".to_string()]);
        assert_eq!(filter.deny.len(), 2);
        assert!(!filter.is_tool_allowed(&tool("preexisting")));
        assert!(!filter.is_tool_allowed(&tool("coding__bash")));
    }

    #[test]
    fn apply_disabled_tools_empty_keeps_filter_intact() {
        // The empty path is a no-op so a run without `--disable-tool` does
        // not see any denylist changes. Baseline allows every tool.
        let mut filter = ToolFilter::default();
        apply_disabled_tools(&mut filter, &[]);
        assert!(filter.is_tool_allowed(&tool("anything")));
        assert!(filter.deny.is_empty());
    }

    #[test]
    fn apply_disabled_tools_unknown_name_is_silent_no_op() {
        // A name that matches no tool leaves the allow-check intact for the
        // tools that do exist; the test pins the no-op-on-unknown contract
        // so callers don't have to special-case it.
        let mut filter = ToolFilter::default();
        apply_disabled_tools(&mut filter, &["does-not-exist".to_string()]);
        assert!(filter.is_tool_allowed(&tool("coding__bash")));
    }

    #[test]
    fn apply_disabled_tools_leaves_sibling_tools_allowed() {
        // Withholding one tool must not affect the rest of the model's view.
        let mut filter = ToolFilter::default();
        apply_disabled_tools(&mut filter, &["coding__bash".to_string()]);
        assert!(!filter.is_tool_allowed(&tool("coding__bash")));
        assert!(filter.is_tool_allowed(&tool("coding__read_file")));
        assert!(filter.is_tool_allowed(&tool("coding__grep")));
    }
}
