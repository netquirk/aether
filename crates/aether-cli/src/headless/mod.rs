pub mod error;
pub mod run;

use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_project::ToolOutputSettings;
use aether_project::{AetherSettings, AgentCatalog, RunSettings, TelemetrySettings};
use aether_telemetry::AgentTraceContext;
use error::CliError;
use llm::{ProviderConnectionOverride, ProviderConnectionOverrides};
use mcp_utils::client::McpConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::credentials::oauth_credential_store_from_config;
use crate::mcp_config_args::McpConfigArgs;
use crate::output::OutputFormat;
use crate::prompt::prompt_or_stdin;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::resolve::{AgentSelectionError, InitialSessionSelection, resolve_agent_from_settings};
use crate::settings_args::SettingsSourceArgs;
use aether_auth::OAuthCredentialStorage;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, Deserialize, Serialize, JsonSchema)]
#[clap(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CliEventKind {
    Text,
    Thought,
    ToolCall,
    ToolResult,
    ToolError,
    AutoContinue,
    ModelSwitched,
    ToolProgress,
    ContextCompactionStarted,
    ContextCompactionEnded,
    ContextCompactionResult,
    ContextUsage,
    SessionUsage,
    ContextCleared,
    TurnStarted,
    TurnEnded,
    LlmRetryScheduled,
    LlmCallStarted,
    LlmCallEnded,
    ToolExecutionStarted,
    ToolDefinitionsUpdated,
    ToolRefused,
}

pub struct RunConfig {
    pub prompt: String,
    pub cwd: PathBuf,
    pub mcp_config_sources: Vec<McpConfigSource>,
    pub spec: AgentSpec,
    pub agent_catalog: AgentCatalog,
    pub system_prompt: Option<String>,
    pub output: OutputFormat,
    pub verbose: bool,
    /// When true, the headless loop does not write the live per-tool progress
    /// line on stderr. Warnings and errors (including the provider-stall
    /// warning, `eprintln!` failure messages, and `tracing` warnings/errors)
    /// are unaffected. Independent of `--events` so a filtered run still
    /// reflects the tool currently executing unless this flag is set.
    pub quiet: bool,
    pub events: Vec<CliEventKind>,
    pub oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    pub telemetry: Option<TelemetrySettings>,
    pub trace_context: Option<AgentTraceContext>,
    /// When set, the run appends a JSON Lines transcript of every output
    /// event to this path (one object per line, each carrying `turn` and
    /// `type`). The file is truncated on open and independent of `--events`,
    /// so a filtered run still gets a complete transcript. The run's stdout
    /// stays human-readable.
    pub transcript_jsonl: Option<PathBuf>,
    /// When set together with `transcript_jsonl`, the transcript is rotated
    /// to `<stem>.1` once a complete line would push the file at or past
    /// this many bytes; the previous sibling is overwritten on each
    /// rotation so exactly one previous file is kept. `None` and `Some(0)`
    /// both disable rotation. No effect when `transcript_jsonl` is `None`.
    pub transcript_max_bytes: Option<u64>,
    /// Top-level `toolOutput` block from the loaded settings. Threaded into
    /// the runtime via [`crate::runtime::RuntimeBuilder::settings_tool_output`]
    /// so the MCP runtime can resolve the cap together with the per-agent
    /// override and the `AETHER_TOOL_OUTPUT_MAX_BYTES` /
    /// `PRAIRIE_TOOL_OUTPUT_DIR` env vars.
    pub settings_tool_output: Option<ToolOutputSettings>,
    /// Extra environment variables given to every shell command a run
    /// starts. Threaded into the runtime via
    /// [`crate::runtime::RuntimeBuilder::shell_environment`] so the
    /// built-in `coding` MCP server's `bash` tool sees them merged over the
    /// process environment.
    pub shell_environment: BTreeMap<String, String>,
    /// Threshold after which the headless CLI prints a one-line warning
    /// naming the elapsed wait when a provider call is still in flight
    /// (TASK-24-378). `None` disables the warning so the existing run shape
    /// is preserved when the setting is absent. Resolved from the top-level
    /// `run.providerStallWarnSeconds` block before the agent starts so the
    /// headless loop can race the deadline without re-reading settings.
    pub provider_stall_warn: Option<Duration>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeadlessOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
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
    pub mcp_config: Option<McpConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbose: Option<bool>,
    /// Mirror of the `--quiet` flag for `--options-json` callers. `true`
    /// suppresses the live per-tool progress line on stderr; warnings and
    /// errors remain unchanged. `None` falls back to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<CliEventKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<AgentTraceContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_jsonl: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_max_bytes: Option<u64>,
}

pub async fn run_headless(args: HeadlessArgs) -> Result<ExitCode, CliError> {
    // Settings loading can emit `tracing::warn!` for unrecognised keys, so
    // initialise the tracing subscriber before the load (the subscriber is
    // re-installed at the proper verbosity inside `run::run`).
    run::setup_tracing(args.verbose);
    if args.dry_run {
        // Short-circuit before any prompt resolution, session construction,
        // MCP setup, telemetry runtime, or provider call: --dry-run only
        // prints the resolved model/profile/endpoint and exits 0.
        print_dry_run(&args)?;
        return Ok(ExitCode::SUCCESS);
    }
    run::run(RunConfig::from_args(args)?).await
}

#[derive(clap::Args)]
pub struct HeadlessArgs {
    #[arg(long = "options-json", value_name = "JSON", hide = true)]
    pub options_json: Option<String>,

    /// Resolve the run configuration, print the resolved model/profile/endpoint,
    /// and exit 0 without starting a session or calling a provider. Ignores
    /// any prompt argument and never reads stdin.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Prompt to send (reads stdin if omitted and stdin is not a TTY)
    pub prompt: Vec<String>,

    /// Named agent from settings.json (defaults to first user-invocable agent)
    #[arg(short = 'a', long = "agent")]
    pub agent: Option<String>,

    /// Model for ad-hoc runs (e.g. "anthropic:claude-sonnet-4-5"). Mutually exclusive with --agent.
    #[arg(short, long)]
    pub model: Option<String>,

    /// Working directory
    #[arg(short = 'C', long = "cwd", default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,

    #[command(flatten)]
    pub provider_connection: ProviderConnectionArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Additional system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Read the additional system prompt from a file
    #[arg(long = "system-prompt-file", value_name = "PATH", conflicts_with = "system_prompt")]
    pub system_prompt_file: Option<PathBuf>,

    /// Output format
    #[arg(long, default_value = "text")]
    pub output: OutputFormat,

    /// Verbose diagnostic logging to stderr.
    #[arg(short, long)]
    pub verbose: bool,

    /// Suppress the live per-tool progress line on stderr. Warnings and
    /// errors (including the provider-stall warning and `tracing` diagnostics)
    /// are still printed. Has no effect on `--events` filtering or on
    /// `--transcript-jsonl`.
    #[arg(long = "quiet")]
    pub quiet: bool,

    /// Comma-separated list of events to emit (e.g. `tool_call,tool_result,turn_ended`).
    /// Omit to emit every output event. When set, turn outcomes are only shown if `turn_ended` is listed.
    #[arg(long = "events", value_enum, value_delimiter = ',')]
    pub events: Vec<CliEventKind>,

    /// Append a JSON Lines transcript of every output event to PATH (one
    /// object per line, each carrying `turn` and `type`). Written to PATH; the
    /// run's stdout stays human-readable. The file is truncated on open.
    #[arg(long = "transcript-jsonl", value_name = "PATH")]
    pub transcript_jsonl: Option<PathBuf>,

    /// Rotate the transcript to `<stem>.1` when it reaches this many bytes;
    /// the previous sibling is overwritten on each rotation. `0` or
    /// omitted disables rotation. Only meaningful together with
    /// `--transcript-jsonl`.
    #[arg(long = "transcript-max-bytes", value_name = "BYTES")]
    pub transcript_max_bytes: Option<u64>,
}

impl RunConfig {
    fn from_args(args: HeadlessArgs) -> Result<Self, CliError> {
        if let Some(json) = args.options_json {
            return Self::from_options(serde_json::from_str(&json).map_err(CliError::InvalidOptionsJson)?);
        }

        let prompt = resolve_prompt(&args)?;
        let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
        let settings = args.settings_source.load_settings(&cwd)?;
        let provider_connections = args.provider_connection.clone().into_overrides();
        let oauth_credential_store = oauth_credential_store_from_config(settings.credentials_store.clone())?;
        let telemetry = settings.telemetry.clone();
        let settings_tool_output = settings.tool_output.clone();
        // Capture before `settings` is moved into `resolve_agent_from_settings`.
        let shell_environment = settings.shell_environment.clone();
        let provider_stall_warn = settings.run.as_ref().and_then(RunSettings::provider_stall_warn);
        let selection = initial_selection(args.agent, args.model)?;
        let resolved = resolve_agent_from_settings(&cwd, settings, provider_connections, &selection)
            .map_err(map_selection_error)?;
        let mcp_config_sources = args.mcp_config.sources(&cwd);

        Ok(Self {
            prompt,
            cwd,
            mcp_config_sources,
            spec: resolved.spec,
            agent_catalog: resolved.catalog,
            system_prompt: resolve_system_prompt(args.system_prompt, args.system_prompt_file)?,
            output: args.output,
            verbose: args.verbose,
            quiet: args.quiet,
            events: args.events,
            oauth_credential_store,
            telemetry,
            trace_context: None,
            settings_tool_output,
            shell_environment,
            provider_stall_warn,
            transcript_jsonl: args.transcript_jsonl,
            transcript_max_bytes: args.transcript_max_bytes,
        })
    }

    fn from_options(options: HeadlessOptions) -> Result<Self, CliError> {
        let prompt = options.prompt.ok_or(CliError::NoPrompt)?;
        let cwd = options.cwd.unwrap_or_else(|| PathBuf::from(".")).canonicalize().map_err(CliError::IoError)?;
        let settings_source = SettingsSourceArgs::from_json_options(options.settings, options.settings_file)?;
        let settings = settings_source.load_settings(&cwd)?;
        let provider_connections = ProviderConnectionOverrides::new(options.providers.unwrap_or_default());
        let oauth_credential_store = oauth_credential_store_from_config(settings.credentials_store.clone())?;
        let telemetry = settings.telemetry.clone();
        let settings_tool_output = settings.tool_output.clone();
        // Capture before `settings` is moved into `resolve_agent_from_settings`.
        let shell_environment = settings.shell_environment.clone();
        let provider_stall_warn = settings.run.as_ref().and_then(RunSettings::provider_stall_warn);
        let selection = initial_selection(options.agent, options.model)?;
        let resolved = resolve_agent_from_settings(&cwd, settings, provider_connections, &selection)
            .map_err(map_selection_error)?;
        let mcp_config_sources = options
            .mcp_config
            .map(|config| serde_json::to_string(&config).expect("mcp config serialize"))
            .map(McpConfigSource::Json)
            .into_iter()
            .collect();

        Ok(Self {
            prompt,
            cwd,
            mcp_config_sources,
            spec: resolved.spec,
            agent_catalog: resolved.catalog,
            system_prompt: resolve_system_prompt(options.system_prompt, options.system_prompt_file)?,
            output: options.output.unwrap_or(OutputFormat::Text),
            verbose: options.verbose.unwrap_or(false),
            quiet: options.quiet.unwrap_or(false),
            events: options.events.unwrap_or_default(),
            oauth_credential_store,
            telemetry,
            trace_context: options.trace_context,
            settings_tool_output,
            shell_environment,
            provider_stall_warn,
            transcript_jsonl: options.transcript_jsonl,
            transcript_max_bytes: options.transcript_max_bytes,
        })
    }
}

fn resolve_prompt(args: &HeadlessArgs) -> Result<String, CliError> {
    let explicit = (!args.prompt.is_empty()).then(|| args.prompt.join(" "));
    prompt_or_stdin(explicit).map_err(CliError::IoError)?.ok_or(CliError::NoPrompt)
}

/// Resolve the system prompt from either an inline string or a file path.
/// Returns `Ok(None)` when neither is provided. A missing or unreadable file
/// becomes `CliError::SystemPromptFile`, whose display message names the path.
fn resolve_system_prompt(text: Option<String>, file: Option<PathBuf>) -> Result<Option<String>, CliError> {
    match (text, file) {
        (Some(_), Some(_)) => {
            Err(CliError::ConflictingArgs("Cannot specify both --system-prompt and --system-prompt-file".to_string()))
        }
        (Some(text), None) => Ok(Some(text)),
        (None, Some(path)) => {
            std::fs::read_to_string(&path).map(Some).map_err(|source| CliError::SystemPromptFile { path, source })
        }
        (None, None) => Ok(None),
    }
}

fn map_selection_error(error: AgentSelectionError) -> CliError {
    match error {
        AgentSelectionError::Settings(error) => CliError::Settings(error),
        AgentSelectionError::Agent(error) => CliError::AgentError(error.to_string()),
        AgentSelectionError::Model(error) => CliError::ModelError(error),
    }
}

/// Translate a `--agent` / `--model` pair into the corresponding
/// `InitialSessionSelection`, returning the same conflict error the inline
/// match produced. Used by both `RunConfig::from_args`,
/// `RunConfig::from_options`, and `print_dry_run` to keep their selection
/// rules identical.
fn initial_selection(agent: Option<String>, model: Option<String>) -> Result<InitialSessionSelection, CliError> {
    Ok(match (agent, model) {
        (Some(agent), None) => InitialSessionSelection::Agent(agent),
        (None, Some(model)) => InitialSessionSelection::Model { model, reasoning_effort: None },
        (None, None) => InitialSessionSelection::Default,
        (Some(_), Some(_)) => {
            return Err(CliError::ConflictingArgs("Cannot specify both --agent and --model".to_string()));
        }
    })
}

/// Resolve the same configuration `RunConfig::from_args` resolves, but stop
/// short of any prompt, session, telemetry, MCP, or provider work. The only
/// side effect is `println` of the resolved model/profile/endpoint summary.
///
/// `--options-json` is accepted (so `--dry-run` works for harness callers
/// that always pass it); the prompt it normally requires is intentionally
/// skipped.
fn print_dry_run(args: &HeadlessArgs) -> Result<(), CliError> {
    let (cwd, settings, provider_connections, selection) = if let Some(json) = args.options_json.as_deref() {
        let options: HeadlessOptions = serde_json::from_str(json).map_err(CliError::InvalidOptionsJson)?;
        let cwd = options.cwd.unwrap_or_else(|| PathBuf::from(".")).canonicalize().map_err(CliError::IoError)?;
        let settings_source = SettingsSourceArgs::from_json_options(options.settings, options.settings_file)?;
        let settings = settings_source.load_settings(&cwd)?;
        let provider_connections = ProviderConnectionOverrides::new(options.providers.unwrap_or_default());
        let selection = initial_selection(options.agent, options.model)?;
        (cwd, settings, provider_connections, selection)
    } else {
        let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
        let settings = args.settings_source.load_settings(&cwd)?;
        let provider_connections = args.provider_connection.clone().into_overrides();
        let selection = initial_selection(args.agent.clone(), args.model.clone())?;
        (cwd, settings, provider_connections, selection)
    };

    let resolved = resolve_agent_from_settings(&cwd, settings, provider_connections.clone(), &selection)
        .map_err(map_selection_error)?;

    println!("{}", resolved_summary(&resolved.spec, &provider_connections));
    Ok(())
}

/// Format the three-line model/profile/endpoint summary printed by
/// `aether headless --dry-run`.
///
/// `spec.provider_connections` already contains the merged CLI + settings
/// overrides after resolution, but we still consult the caller-supplied
/// `overrides` as a fallback in case future catalogs expose a different
/// merge order. When neither carries a `base_url` for the resolved provider
/// we print `(provider default)`.
fn resolved_summary(spec: &AgentSpec, overrides: &ProviderConnectionOverrides) -> String {
    let provider = spec.model.split(',').next().and_then(|first| first.split(':').next()).unwrap_or("");
    let endpoint = spec
        .provider_connections
        .get(provider)
        .and_then(|c| c.base_url.clone())
        .or_else(|| overrides.get(provider).and_then(|c| c.base_url.clone()))
        .unwrap_or_else(|| "(provider default)".to_string());
    format!(
        "model: {model}\nprofile: {profile}\nendpoint: {endpoint}",
        model = spec.model,
        profile = spec.name,
        endpoint = endpoint
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_selection_rejects_both_agent_and_model() {
        let error = initial_selection(Some("build".to_string()), Some("ollama:llama3.2".to_string()))
            .expect_err("both --agent and --model must be rejected");
        match error {
            CliError::ConflictingArgs(message) => assert!(message.contains("--agent")),
            other => panic!("expected ConflictingArgs, got {other:?}"),
        }
    }

    #[test]
    fn system_prompt_file_contents_become_the_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("system.txt");
        std::fs::write(&path, "you are a helpful agent\n").expect("write file");

        let resolved = resolve_system_prompt(None, Some(path.clone())).expect("file resolves");
        assert_eq!(resolved.as_deref(), Some("you are a helpful agent\n"));
    }

    #[test]
    fn system_prompt_inline_string_is_returned_verbatim() {
        let resolved = resolve_system_prompt(Some("inline".to_string()), None).expect("inline resolves");
        assert_eq!(resolved.as_deref(), Some("inline"));
    }

    #[test]
    fn missing_system_prompt_file_names_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("no-such-prompt.txt");
        let path_for_assert = path.clone();

        let error = resolve_system_prompt(None, Some(path)).expect_err("missing file must fail");

        match error {
            CliError::SystemPromptFile { path, source: _ } => {
                assert_eq!(path, path_for_assert);
            }
            other => panic!("expected SystemPromptFile, got {other:?}"),
        }
    }

    #[test]
    fn missing_system_prompt_file_message_includes_path_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nope.txt");

        let error = resolve_system_prompt(None, Some(path.clone())).expect_err("missing file must fail");
        let rendered = error.to_string();
        let expected = path.to_string_lossy().into_owned();

        assert!(rendered.contains(&expected), "message {rendered:?} must contain the path {expected:?}");
    }

    #[test]
    fn both_system_prompt_sources_conflict() {
        let error = resolve_system_prompt(Some("inline".to_string()), Some(PathBuf::from("prompt.txt")))
            .expect_err("both sources must be rejected");

        match error {
            CliError::ConflictingArgs(message) => {
                assert!(message.contains("--system-prompt"));
                assert!(message.contains("--system-prompt-file"));
            }
            other => panic!("expected ConflictingArgs, got {other:?}"),
        }
    }

    #[test]
    fn no_system_prompt_source_returns_none() {
        let resolved = resolve_system_prompt(None, None).expect("no sources resolves");
        assert!(resolved.is_none());
    }

    #[test]
    fn initial_selection_accepts_each_alone_and_neither() {
        let agent = initial_selection(Some("build".to_string()), None).expect("agent-only selection resolves");
        assert!(matches!(agent, InitialSessionSelection::Agent(name) if name == "build"));

        let model =
            initial_selection(None, Some("ollama:llama3.2".to_string())).expect("model-only selection resolves");
        match model {
            InitialSessionSelection::Model { model, reasoning_effort } => {
                assert_eq!(model, "ollama:llama3.2");
                assert!(reasoning_effort.is_none());
            }
            other => panic!("expected Model selection, got {other:?}"),
        }

        assert!(matches!(initial_selection(None, None), Ok(InitialSessionSelection::Default)));
    }

    /// Tiny harness so the clap `HeadlessArgs` parser can be exercised in
    /// isolation; the production binary adds subcommand plumbing that is not
    /// relevant to argument parsing.
    #[derive(clap::Parser)]
    struct QuietHarness {
        #[command(flatten)]
        args: HeadlessArgs,
    }

    #[test]
    fn quiet_flag_is_accepted() {
        use clap::Parser as _;
        assert!(QuietHarness::try_parse_from(["aether", "--quiet"]).unwrap().args.quiet);
        assert!(!QuietHarness::try_parse_from(["aether"]).unwrap().args.quiet);
    }

    #[test]
    fn quiet_flag_defaults_to_false_and_can_be_combined() {
        use clap::Parser as _;
        let parsed = QuietHarness::try_parse_from(["aether", "hello"]).unwrap().args;
        assert!(!parsed.quiet, "quiet must default to false when the flag is absent");
        assert_eq!(parsed.prompt, vec!["hello".to_string()]);

        // `--quiet` is a long flag, so positional prompt words that look like
        // options still parse. We just verify the flag survives alongside the
        // positional prompt.
        let parsed = QuietHarness::try_parse_from(["aether", "--quiet", "hello"]).unwrap().args;
        assert!(parsed.quiet, "--quiet must set the flag alongside a positional prompt");
        assert_eq!(parsed.prompt, vec!["hello".to_string()]);
    }

    #[test]
    fn resolved_summary_prints_model_profile_and_endpoint() {
        let model: llm::LlmModel = "ollama:llama3.2".parse().expect("model parses");
        let mut spec = AgentSpec::bare(&model, None, Vec::new());
        spec.name = "build".to_string();
        spec.description = "Test build agent".to_string();
        let ollama =
            ProviderConnectionOverride { base_url: Some("http://127.0.0.1:11434".to_string()), ..Default::default() };
        let overrides = ProviderConnectionOverrides::new(BTreeMap::from([("ollama".to_string(), ollama)]));

        let rendered = resolved_summary(&spec, &overrides);

        assert!(rendered.contains("model: ollama:llama3.2"), "missing model line in:\n{rendered}");
        assert!(rendered.contains("profile: build"), "missing profile line in:\n{rendered}");
        assert!(rendered.contains("endpoint: http://127.0.0.1:11434"), "missing endpoint line in:\n{rendered}");
        assert!(rendered.starts_with("model: "), "summary should start with the model line:\n{rendered}");
    }

    #[test]
    fn resolved_summary_falls_back_to_overrides_map() {
        let model: llm::LlmModel = "ollama:llama3.2".parse().expect("model parses");
        let spec = AgentSpec::bare(&model, None, Vec::new());
        // Empty spec-level overrides, populated caller-supplied map.
        let ollama = ProviderConnectionOverride {
            base_url: Some("http://example.test:11434".to_string()),
            ..Default::default()
        };
        let overrides = ProviderConnectionOverrides::new(BTreeMap::from([("ollama".to_string(), ollama)]));

        let rendered = resolved_summary(&spec, &overrides);
        assert!(
            rendered.contains("endpoint: http://example.test:11434"),
            "should fall back to override map:\n{rendered}"
        );
    }

    #[test]
    fn resolved_summary_uses_provider_default_when_no_url_configured() {
        let model: llm::LlmModel = "anthropic:claude-sonnet-4-5".parse().expect("model parses");
        let spec = AgentSpec::bare(&model, None, Vec::new());
        let overrides = ProviderConnectionOverrides::default();

        let rendered = resolved_summary(&spec, &overrides);
        assert!(rendered.contains("endpoint: (provider default)"), "should report the provider default:\n{rendered}");
    }

    #[test]
    fn resolved_summary_picks_first_provider_in_alloy_model() {
        // Alloy specs are stored as comma-separated `provider:model` strings;
        // they never go through `LlmModel::parse`, so construct the spec
        // straight from the bare helper and then overwrite `.model` to the
        // alloy form for this unit test.
        let single: llm::LlmModel = "anthropic:claude-sonnet-4-5".parse().expect("single model parses");
        let mut spec = AgentSpec::bare(&single, None, Vec::new());
        spec.name = "router".to_string();
        spec.model = "anthropic:claude-sonnet-4-5,openai:gpt-4".to_string();
        // Provide an override for *both* providers; only the first (anthropic)
        // should drive the printed endpoint, mirroring how alloy specs route.
        let mut overrides = BTreeMap::new();
        overrides.insert("anthropic".to_string(), ProviderConnectionOverride::url("https://anthropic.example.test"));
        overrides.insert("openai".to_string(), ProviderConnectionOverride::url("https://openai.example.test"));
        let overrides = ProviderConnectionOverrides::new(overrides);

        let rendered = resolved_summary(&spec, &overrides);
        assert!(
            rendered.contains("endpoint: https://anthropic.example.test"),
            "alloy spec should pick first provider's endpoint:\n{rendered}"
        );
    }
}
