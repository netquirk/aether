pub(crate) mod agent;
#[cfg(any(test, feature = "testing"))]
pub(crate) mod fake_prompt_mcp;
pub(crate) mod message_log;
pub(crate) mod protocol;
pub mod server;
pub(crate) mod session;
pub(crate) mod state;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use protocol::map_mcp_prompt_to_available_command;

use crate::acp::message_log::log_acp_message;
use crate::acp::server::DetachedArgs;
use crate::acp::state::{AcpState, AcpStateConfig};
use crate::credentials::oauth_credential_store_from_config;
use crate::log_level::LogLevel;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::resolve::InitialSessionSelection;
use crate::settings_args::{ConflictingSettingsSources, SettingsSourceArgs};
use crate::telemetry::build_telemetry_runtime;
use crate::workspace::{WorkspaceManager, warn_if_not_a_repository};
use aether_project::AetherSettings;
use aether_sessions::SessionStore;
use aether_telemetry::{AgentTraceContext, TelemetryInitError};
use agent_client_protocol as acp;
use agent_client_protocol::{LineDirection, Stdio};
use llm::catalog::{ReasoningEffortError, validate_reasoning_effort};
use llm::{ProviderConnectionOverride, ProviderConnectionOverrides, ReasoningEffort};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::{
    fs::create_dir_all,
    path::{Path, PathBuf},
};
use thiserror::Error;
use tracing::{info, warn};
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

use aether_auth::OAuthError;
use aether_project::SettingsError;

#[derive(clap::Args, Debug)]
pub struct AcpArgs {
    /// JSON object with ACP launch options. Intended for SDKs and other programmatic clients.
    #[clap(
        long = "options-json",
        hide = true,
        conflicts_with_all = ["log_dir", "agent", "model", "reasoning_effort", "providers", "settings_json", "settings_file"]
    )]
    pub options_json: Option<String>,

    /// Path to log file directory (default: /tmp/aether-acp-logs)
    #[clap(long)]
    pub log_dir: Option<PathBuf>,

    /// Initial agent (mode) to select for new sessions. Mutually exclusive with `--model` and `--reasoning-effort`.
    #[clap(long, conflicts_with_all = ["model", "reasoning_effort"])]
    pub agent: Option<String>,

    /// Initial model id (e.g. `anthropic:claude-sonnet-4-5`) for new sessions.
    /// Mutually exclusive with `--agent`.
    #[clap(long, conflicts_with = "agent")]
    pub model: Option<String>,

    /// Initial reasoning effort for an explicit model session. Requires `--model` and is mutually exclusive with `--agent`.
    #[clap(
        long,
        value_name = "default|disabled|minimal|low|medium|high|xhigh|max",
        requires = "model",
        conflicts_with = "agent"
    )]
    pub reasoning_effort: Option<ReasoningEffort>,

    #[command(flatten)]
    pub provider_connection: ProviderConnectionArgs,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,

    /// Directory the session is rooted at. Defaults to the current working directory
    /// when the flag is absent. The directory is canonicalized before use so file
    /// tools can resolve paths under it.
    #[arg(short = 'C', long = "cwd", value_name = "DIR", default_value = ".")]
    pub cwd: PathBuf,

    /// How much the run logs: one of `error`, `warn`, `info`, `debug`, or
    /// `trace` (TASK-25-41). When set, replaces the default `warn` filter
    /// applied to the per-day log file. `trace` additionally installs a
    /// debug callback on the stdio ACP transport so every message the
    /// server sends and receives is recorded in the log file as a single
    /// line naming its direction and method (TASK-25-467). The flag is also
    /// accepted before the subcommand (`aether --log-level debug acp …`);
    /// the subcommand-level flag wins when both are set.
    #[arg(long = "log-level", value_name = "LEVEL")]
    pub log_level: Option<LogLevel>,
}

#[derive(Clone, Debug, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, ProviderConnectionOverride>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<AetherSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<AgentTraceContext>,
}

/// Outcome of running the ACP server successfully.
#[derive(Debug)]
pub enum AcpRunOutcome {
    /// The client disconnected cleanly (e.g. EOF on stdin).
    CleanDisconnect,
}

/// Errors that terminate the ACP server run.
#[derive(Debug, Error)]
pub enum AcpRunError {
    #[error("ACP protocol error: {0}")]
    Protocol(#[from] acp::Error),

    #[error("Invalid --options-json: {0}")]
    OptionsJson(#[from] AcpOptionsJsonError),

    #[error("Failed to load settings: {0}")]
    Settings(#[from] SettingsError),

    #[error("Failed to initialize OAuth credential store: {0}")]
    CredentialStore(#[from] OAuthError),

    #[error("Failed to initialize session store: {0}")]
    SessionStore(#[source] aether_sessions::SessionStoreError),

    #[error("Failed to initialize workspace manager: {0}")]
    WorkspaceManager(#[source] io::Error),

    #[error("Invalid session directory {path}: {source}")]
    Workspace { path: PathBuf, source: io::Error },

    #[error("Failed to initialize telemetry: {0}")]
    Telemetry(#[source] TelemetryInitError),
}

#[derive(Debug, Error)]
pub enum AcpOptionsJsonError {
    #[error("{0}")]
    Parse(#[from] serde_json::Error),
    #[error(transparent)]
    ConflictingSettingsSources(#[from] ConflictingSettingsSources),
    #[error("agent and model cannot both be supplied")]
    ConflictingAgentSelection,
    #[error("reasoningEffort requires model")]
    ReasoningEffortWithoutModel,
    #[error("invalid reasoningEffort: {0}")]
    UnsupportedReasoningEffort(#[from] ReasoningEffortError),
}

pub async fn run_acp(args: AcpArgs) -> Result<AcpRunOutcome, AcpRunError> {
    eprintln!("{}", crate::version::startup_line());
    info!("Starting Aether ACP server");

    let cwd = resolve_session_root(&args)?;
    warn_if_not_a_repository(&cwd);
    // `--log-level trace` (TASK-25-467) attaches a one-line debug callback
    // to the stdio ACP transport so every JSON-RPC message is logged. We
    // compute the gate here — before `args` is moved into
    // `create_acp_state` — and reuse it below to pick the right transport.
    let trace_messages = args.log_level == Some(LogLevel::Trace);
    let state = Arc::new(create_acp_state(args, &cwd, DetachedArgs::default())?);
    let transport =
        if trace_messages { Stdio::new().with_debug(log_acp_message as fn(&str, LineDirection)) } else { Stdio::new() };
    let connect_result = state.serve(transport, state.stop_token()).await;
    state.shutdown_all().await;

    match connect_result {
        Ok(()) => Ok(AcpRunOutcome::CleanDisconnect),
        Err(err) => Err(AcpRunError::Protocol(err)),
    }
}

/// Resolve and canonicalize the directory the session is rooted at. When
/// `--options-json` carries an explicit `cwd` it wins; otherwise the
/// `-C/--cwd` flag is used (default `.`). The resolved path is canonicalized
/// so file tools downstream can resolve paths under it as absolute paths.
fn resolve_session_root(args: &AcpArgs) -> Result<PathBuf, AcpRunError> {
    let requested = match args.options_json.as_deref() {
        Some(json) => serde_json::from_str::<AcpOptions>(json)
            .map_err(AcpOptionsJsonError::from)?
            .cwd
            .unwrap_or_else(|| args.cwd.clone()),
        None => args.cwd.clone(),
    };
    match std::fs::canonicalize(&requested) {
        Ok(canonical) if canonical.is_dir() => Ok(canonical),
        Ok(_) => {
            let source = io::Error::new(io::ErrorKind::NotADirectory, "workspace must be a directory");
            Err(AcpRunError::Workspace { path: requested, source })
        }
        Err(source) => Err(AcpRunError::Workspace { path: requested, source }),
    }
}

#[derive(Debug)]
struct AcpRunConfig {
    log_dir: PathBuf,
    agent: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    trace_context: Option<AgentTraceContext>,
    provider_connections: ProviderConnectionOverrides,
    settings_source: SettingsSourceArgs,
    log_level: Option<LogLevel>,
}

impl AcpRunConfig {
    fn from_args(args: AcpArgs) -> Result<Self, AcpOptionsJsonError> {
        if let Some(json) = args.options_json {
            return Self::from_options(serde_json::from_str(&json)?);
        }

        let config = Self {
            log_dir: args.log_dir.unwrap_or_else(default_log_dir),
            agent: args.agent,
            model: args.model,
            reasoning_effort: args.reasoning_effort,
            trace_context: None,
            provider_connections: args.provider_connection.into_overrides(),
            settings_source: args.settings_source,
            log_level: args.log_level,
        };
        config.validate_reasoning_effort()?;
        Ok(config)
    }

    fn from_options(options: AcpOptions) -> Result<Self, AcpOptionsJsonError> {
        if options.agent.is_some() && options.model.is_some() {
            return Err(AcpOptionsJsonError::ConflictingAgentSelection);
        }
        if options.reasoning_effort.is_some() && options.model.is_none() {
            return Err(AcpOptionsJsonError::ReasoningEffortWithoutModel);
        }

        let settings_source = SettingsSourceArgs::from_json_options(options.settings, options.settings_file)?;

        let config = Self {
            log_dir: options.log_dir.unwrap_or_else(default_log_dir),
            agent: options.agent,
            model: options.model,
            reasoning_effort: options.reasoning_effort,
            trace_context: options.trace_context,
            provider_connections: ProviderConnectionOverrides::new(options.providers.unwrap_or_default()),
            settings_source,
            // `--options-json` callers have no flag-level override; they
            // inherit the default `warn` / RUST_LOG filter.
            log_level: None,
        };
        config.validate_reasoning_effort()?;
        Ok(config)
    }

    fn validate_reasoning_effort(&self) -> Result<(), AcpOptionsJsonError> {
        if let Some(model) = self.model.as_deref() {
            validate_reasoning_effort(model, self.reasoning_effort)?;
        }
        Ok(())
    }
}

fn create_acp_state(args: AcpArgs, cwd: &Path, detached: DetachedArgs) -> Result<AcpState, AcpRunError> {
    let config = AcpRunConfig::from_args(args)?;
    setup_logging(&config.log_dir, config.log_level);

    let initial_selection = if let Some(agent) = config.agent.clone() {
        InitialSessionSelection::agent(agent)
    } else if let Some(model) = config.model.clone() {
        InitialSessionSelection::model(model, config.reasoning_effort)
    } else {
        InitialSessionSelection::default()
    };

    let settings = config.settings_source.load_settings(cwd)?;
    // Capture before the settings value is consumed by downstream callers.
    let shell_environment = settings.shell_environment.clone();
    let telemetry = match build_telemetry_runtime(settings.telemetry.as_ref(), config.trace_context) {
        Ok(telemetry) => telemetry,
        Err(error @ TelemetryInitError::InvalidTraceContext(_)) => return Err(AcpRunError::Telemetry(error)),
        Err(error) => {
            warn!("Telemetry disabled: {error}");
            None
        }
    };

    Ok(AcpState::new(AcpStateConfig {
        session_store: Arc::new(SessionStore::new().map_err(AcpRunError::SessionStore)?),
        workspace_manager: Arc::new(WorkspaceManager::new().map_err(AcpRunError::WorkspaceManager)?),
        oauth_credential_store: oauth_credential_store_from_config(settings.credentials_store)?,
        initial_selection,
        settings_source: config.settings_source,
        provider_connections: config.provider_connections,
        telemetry,
        runtime_factory: None,
        cwd: cwd.to_path_buf(),
        detached,
        tool_output_settings: settings.tool_output,
        shell_environment,
    }))
}

fn setup_logging(log_dir: &Path, log_level: Option<LogLevel>) {
    create_dir_all(log_dir).ok();
    // The explicit `--log-level` wins; without it, fall back to the env
    // filter (so RUST_LOG-based overrides still work) and then to `warn`.
    let filter = match log_level {
        Some(level) => EnvFilter::new(level.directive()),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(daily(log_dir, "aether-acp.log"))
        .with_ansi(false) // No ANSI colors in log files
        .with_env_filter(filter)
        .pretty()
        .try_init();
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("/tmp/aether-acp-logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: AcpArgs,
    }

    #[test]
    fn agent_conflicts_with_model() {
        let err = TestCli::try_parse_from(["test", "--agent", "planner", "--model", "anthropic:claude-sonnet-4-5"])
            .expect_err("agent and model should conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn agent_conflicts_with_reasoning_effort() {
        let err = TestCli::try_parse_from(["test", "--agent", "planner", "--reasoning-effort", "high"])
            .expect_err("agent and reasoning effort should conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn reasoning_effort_requires_model() {
        let err = TestCli::try_parse_from(["test", "--reasoning-effort", "high"])
            .expect_err("reasoning effort should require model");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn reasoning_effort_with_model_is_allowed() {
        let cli =
            TestCli::try_parse_from(["test", "--model", "anthropic:claude-sonnet-4-5", "--reasoning-effort", "high"])
                .expect("reasoning effort can configure an explicit model session");
        assert_eq!(cli.args.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn options_json_conflicts_with_individual_flags() {
        let err = TestCli::try_parse_from([
            "test",
            "--options-json",
            r#"{"model":"anthropic:claude-sonnet-4-5"}"#,
            "--model",
            "anthropic:claude-sonnet-4-5",
        ])
        .expect_err("options JSON should conflict with individual ACP flags");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn resolves_options_json() {
        let cli = TestCli::try_parse_from([
            "test",
            "--options-json",
            r#"{"logDir":"/tmp/custom-aether-logs","settings":{"agents":[]},"model":"anthropic:claude-sonnet-4-5","reasoningEffort":"high","providers":{"bedrock":{"url":"http://127.0.0.1:8787","auth":"none"}}}"#,
        ])
        .unwrap();

        let config = AcpRunConfig::from_args(cli.args).unwrap();

        assert_eq!(config.log_dir, PathBuf::from("/tmp/custom-aether-logs"));
        assert_eq!(config.model.as_deref(), Some("anthropic:claude-sonnet-4-5"));
        assert_eq!(config.reasoning_effort, Some(ReasoningEffort::High));
        assert!(config.trace_context.is_none());
        assert!(config.settings_source.settings_json.as_deref().unwrap().contains(r#""agents":[]"#));
        let bedrock = config.provider_connections.config_for("bedrock");
        assert_eq!(bedrock.base_url.as_deref(), Some("http://127.0.0.1:8787"));
        assert_eq!(bedrock.auth_mode, llm::ProviderAuthMode::None);
    }

    #[test]
    fn options_json_rejects_unsupported_reasoning_effort() {
        let error = AcpRunConfig::from_options(AcpOptions {
            model: Some("anthropic:claude-opus-4-6".to_string()),
            reasoning_effort: Some(ReasoningEffort::Xhigh),
            ..AcpOptions::default()
        })
        .unwrap_err();

        assert!(matches!(error, AcpOptionsJsonError::UnsupportedReasoningEffort(_)));
    }

    #[test]
    fn options_json_validates_selection_rules() {
        let err = AcpRunConfig::from_options(AcpOptions {
            reasoning_effort: Some(ReasoningEffort::High),
            ..AcpOptions::default()
        })
        .unwrap_err();

        assert!(matches!(err, AcpOptionsJsonError::ReasoningEffortWithoutModel));
    }
}
