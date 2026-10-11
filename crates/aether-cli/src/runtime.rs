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
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::debug;

use crate::workspace_boundary::WorkspaceBoundaryPolicy;

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
        let cwd = self.cwd.clone();
        let (spec, session) = self.spawn_mcp().await?;
        let mcp = session.handle().clone();

        let (agent_tx, agent_rx, agent_handle) = spawn_agent(&spec, &deps, mcp, Vec::new(), |mut agent_builder| {
            agent_builder = agent_builder.tool_policy(Arc::new(WorkspaceBoundaryPolicy::new(cwd.clone())));
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
        let cwd = self.cwd.clone();
        let (spec, mut session) = self.spawn_mcp().await?;
        let snapshot = session
            .block_until_ready()
            .await
            .ok_or_else(|| CliError::McpError("MCP bootstrap aborted before completion".to_string()))?;
        let mcp = session.handle().clone();

        let (agent_tx, agent_rx, agent_handle) = spawn_agent(&spec, &deps, mcp, Vec::new(), |mut agent_builder| {
            agent_builder = agent_builder.tool_policy(Arc::new(WorkspaceBoundaryPolicy::new(cwd.clone())));
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
    use llm::testing::llm_response;
    use llm::ToolDefinition;
    use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse};
    use serde_json::json;
    use std::error::Error;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "test tool", json!({"type": "object"}))
    }

    /// Drives a fake MCP and the [`WorkspaceBoundaryPolicy`] the runtime wires in
    /// (TASK-26-615). An LLM-scripted `coding__read_file` call whose argument
    /// `../secret` escapes the canonical root must surface as a
    /// [`ToolEvent::Refused`](aether_core::events::ToolEvent::Refused) whose
    /// `reason` names the offending path; a sibling in-root call must dispatch
    /// and produce a result event. The test exercises the same
    /// `AgentBuilder::tool_policy(...)` consult point (`agent.rs:767`) the
    /// runtime populates, so it pins the wiring without reaching for a real
    /// provider-driven `RuntimeBuilder::build` (which has no fake-provider seam).
    #[tokio::test]
    async fn workspace_boundary_refuses_dotdot_escape_and_lets_in_root_through() -> Result<(), Box<dyn Error>> {
        use aether_core::events::{AgentEvent, ToolEvent};

        // Build the canonical root exactly as the production policy will: a
        // freshly canonicalized tempdir so the `/tmp` <-> `/private/tmp`
        // symlink dance on macOS lines up the same way the runtime sees it.
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::write(root.join("notes.txt"), "hello")?;

        // Fake `coding` MCP server: the only tool exposed is `read_file`,
        // and it returns a fixed body whose contents are unique enough that
        // a leaking call would be unmistakable in the trace. The fake
        // server records every call, so we can assert the escaping call
        // never reached it.
        let server = FakeMcpServer::new()
            .with_tool(FakeTool::new("read_file").responds(FakeToolResponse::text("FILE CONTENTS")));
        let server_state = server.state();

        let escape_args = json!({ "filePath": "../secret" }).to_string();
        let in_root_args = json!({ "filePath": "notes.txt" }).to_string();

        let llm_responses = [
            llm_response()
                .tool_call("escape", "coding__read_file", &[&escape_args])
                .build(),
            llm_response()
                .tool_call("in-root", "coding__read_file", &[&in_root_args])
                .build(),
            llm_response().text(&["done"]).build(),
        ];

        let observer = aether_core::testing::FakeAgentObserver::new();
        let events = observer.events();
        let messages = aether_core::testing::test_agent()
            .fake_mcp_server("coding", server)
            .llm_responses(&llm_responses)
            .user_text("read")
            .tool_policy(std::sync::Arc::new(WorkspaceBoundaryPolicy::new(&root)))
            .observer(Box::new(observer))
            .run()
            .await?;

        assert_eq!(
            messages.len(),
            events.lock().unwrap().len(),
            "observer must see every event the agent emits"
        );

        let observer_events = events.lock().unwrap();

        // The ../secret call must surface as a refusal that names the
        // offending path and the canonical root; the simulated server must
        // not have been hit, so nothing outside the root is read.
        let escape_refused = observer_events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Tool(ToolEvent::Refused { request, reason })
                    if request.name == "coding__read_file" && request.id == "escape" =>
                {
                    Some((request.clone(), reason.clone()))
                }
                _ => None,
            })
            .expect("the ../secret call must be refused as ToolEvent::Refused");
        let (_request, escape_reason) = escape_refused;
        assert!(
            escape_reason.contains("../secret"),
            "refusal reason must name the offending path `../secret`; got: {escape_reason}",
        );
        assert!(
            escape_reason.contains(&*root.to_string_lossy()),
            "refusal reason must name the canonical root `{}`; got: {escape_reason}",
            root.display(),
        );
        assert_eq!(
            server_state.calls_for("read_file").len(),
            1,
            "only the in-root call may have reached the server; got {} call(s)",
            server_state.calls_for("read_file").len(),
        );
        assert!(
            !observer_events.iter().any(|event| matches!(
                event,
                AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, .. }) if tool_id == "escape"
            )),
            "the escaped call must never be marked as executing",
        );
        assert!(
            !observer_events.iter().any(|event| matches!(
                event,
                AgentEvent::Tool(ToolEvent::Result { result, .. }) if result.id == "escape"
            )),
            "the escaped call must never produce a result event",
        );

        // The sibling in-root `notes.txt` call must dispatch and yield a
        // result event for its `in-root` call id, with the canned body the
        // fake server returned. This guards against an over-broad policy
        // that would refuse legitimate in-root calls along with the escape.
        assert!(
            observer_events.iter().any(|event| matches!(
                event,
                AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, .. }) if tool_id == "in-root"
            )),
            "the in-root call must have started executing",
        );
        assert!(
            observer_events.iter().any(|event| matches!(
                event,
                AgentEvent::Tool(ToolEvent::Result { result, .. })
                    if result.id == "in-root" && result.result.contains("FILE CONTENTS")
            )),
            "the in-root call must have produced a result event with the canned file body",
        );

        Ok(())
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
