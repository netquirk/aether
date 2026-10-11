use aether_cli::acp::server::{ServerArgs, ServerRunError, run_server};
use aether_cli::acp::{AcpArgs, AcpRunError, AcpRunOutcome, run_acp};
use aether_cli::client::{ClientArgs, ClientRunError, run_client};
use aether_cli::error::CliError;
use aether_cli::generate_command::{GenerateArgs, GenerateCommandError, run as run_generate_command};
use aether_cli::headless::{HeadlessArgs, run_headless};
use aether_cli::init::{InitError, InitOutcome, InitRequest, next_steps_message, run_init};
use aether_cli::log_level::LogLevel;
use aether_cli::mcp_command::{McpArgs, McpCommandError, run as run_mcp_command};
use aether_cli::settings::SettingsCommand;
use aether_cli::settings_args::SettingsSourceArgs;
use aether_cli::show_prompt::{PromptArgs, run_prompt};
use aether_project::{AgentCatalog, project_settings_path, user_settings_path};
use clap::{Parser, Subcommand};
use llm::LlmModel;
use rustls::crypto::aws_lc_rs;
use std::env::current_dir;
use std::process::ExitCode;
use tokio::runtime::Runtime;
use wisp::run_tui;
use wisp::settings::{StatusLineSegmentConfig, StatusLineSettings, load_or_create_settings};

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("{0}")]
    Cli(#[from] CliError),
    #[error("{0}")]
    Generate(#[from] GenerateCommandError),
    #[error("{0}")]
    Acp(#[from] AcpRunError),
    #[error(transparent)]
    Server(#[from] ServerRunError),
    #[error(transparent)]
    Client(#[from] ClientRunError),
    #[error("{0}")]
    Init(#[from] InitError),
    #[error("{0}")]
    Mcp(#[from] McpCommandError),
    #[error("{0}")]
    Lspd(#[from] aether_lspd::LspdRunError),
    #[error("{0}")]
    Tui(#[from] wisp::error::AppError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Settings(String),
}

#[derive(Parser)]
#[command(name = "aether")]
#[command(about = "Aether AI coding agent")]
#[command(version)]
struct Cli {
    /// Run inside a Docker sandbox using the given image
    #[arg(long, global = true)]
    sandbox_image: Option<String>,

    /// Model for this run, as `provider:model` (e.g. `anthropic:claude-sonnet-4-5`).
    /// Overrides the configured model for this run; settings are never written.
    ///
    /// Only the bare `aether` command (the TUI) accepts this flag. When a
    /// subcommand is supplied, the model override lives on the subcommand's
    /// own `--model` flag instead.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// List the profile (agent) names defined in the loaded config and exit.
    ///
    /// Reads the same settings the subcommands do (defaults, or the file
    /// named by `--config`/`--settings-file`/the inline `--settings-json`).
    /// Prints each agent name on its own line and exits 0, including when
    /// the config defines zero agents.
    #[arg(long = "list-profiles")]
    list_profiles: bool,

    /// Validate the loaded config and exit without starting a run.
    ///
    /// Reads the same settings the subcommands do (defaults, or the file
    /// named by `--config`/`--settings-file`/the inline `--settings-json`)
    /// and exercises the same JSON / schema validation the run path sees.
    /// Exits 0 when the config loads, and prints the error and exits
    /// non-zero when it does not. Does not build a session, runtime, or
    /// provider client.
    #[arg(long = "check-config")]
    check_config: bool,

    /// How much the run logs: one of `error`, `warn`, `info`, `debug`, or
    /// `trace`. Forwarded to every subcommand that produces tracing output,
    /// so the flag may be placed before the subcommand (`aether --log-level
    /// debug headless …`) or after it (`aether headless --log-level debug
    /// …`); the first placement wins when both are set. Without it, the
    /// legacy `--verbose` flag (still accepted on the headless subcommand)
    /// controls the level, defaulting to `warn` when neither is set.
    /// `trace` additionally logs every ACP message the `acp` subcommand
    /// sends and receives (TASK-25-467).
    #[arg(long = "log-level", value_name = "LEVEL")]
    log_level: Option<LogLevel>,

    #[command(flatten)]
    settings_source: SettingsSourceArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a single prompt headlessly
    Headless(HeadlessArgs),
    /// Call a model with a single prompt and print its response
    Generate(GenerateArgs),
    /// Start the stdio ACP server
    Acp(AcpArgs),
    /// Host a persistent remote ACP session over WebSocket
    Server(ServerArgs),
    /// Attach the TUI to a remote Aether server without starting a local agent
    Client(ClientArgs),
    /// Print the fully assembled system prompt (for debugging)
    ShowPrompt(PromptArgs),
    /// Discover and call deferred MCP tools
    Mcp(McpArgs),
    /// Manage Aether settings
    #[command(subcommand)]
    Settings(SettingsCommand),
    /// Start the LSP daemon (used internally)
    #[command(hide = true)]
    Lspd(aether_lspd::LspdArgs),
}

fn main() -> ExitCode {
    aws_lc_rs::default_provider().install_default().expect("failed to install the Rustls AWS-LC crypto provider");

    let cli = Cli::parse();

    if let Some(image) = cli.sandbox_image {
        return aether_cli::sandbox::exec_in_container(&image);
    }

    if let (Some(model), Some(_)) = (cli.model.as_deref(), cli.command.as_ref()) {
        let error = CliError::ConflictingArgs(format!(
            "--model '{model}' applies to the bare `aether` command only; pass it after the subcommand instead (for example, `aether headless --model {model}`)."
        ));
        eprintln!("Error: {error}");
        return ExitCode::FAILURE;
    }

    // `--config`/`--settings-file` written before the subcommand (or with no
    // subcommand at all) land on the top-level `Cli`, which the run paths do
    // not read for any command other than `--check-config` / `--list-profiles`.
    // Without this check a mistyped or unreadable path would be silently
    // ignored. Reusing `load_settings` here means the same path-naming error
    // is reported whether the flag was placed on the subcommand or on the
    // top-level `Cli`. A no-op when neither flag was supplied, so the check
    // does not apply when a subcommand's own `--config` is the one being
    // validated.
    if let Err(error) = cli.settings_source.verify_explicit_source() {
        eprintln!("Error: {error}");
        return ExitCode::FAILURE;
    }

    // Copy the top-level flag out before `cli.command` is moved into the
    // `match` below. The subcommand path (`aether headless --log-level …`) is
    // the documented placement, but accepting the flag before the subcommand
    // (`aether --log-level … headless`) keeps it discoverable and matches the
    // `Clap::Args` derives on the subcommand structs. The merge below only
    // overwrites when the subcommand did not see a value itself, so the
    // subcommand-level flag wins when both are set.
    let top_log_level = cli.log_level;

    let result: Result<ExitCode, MainError> = if cli.check_config {
        check_config(&cli.settings_source).map(|()| ExitCode::SUCCESS)
    } else if cli.list_profiles {
        list_profiles(&cli.settings_source).map(|()| ExitCode::SUCCESS)
    } else {
        let rt = Runtime::new().expect("Failed to create tokio runtime");
        match cli.command {
            Some(Command::Headless(mut args)) => {
                if args.log_level.is_none() {
                    args.log_level = top_log_level;
                }
                rt.block_on(run_headless(args)).map_err(Into::into)
            }

            Some(Command::Generate(args)) => rt.block_on(run_generate_command(args)).map_err(Into::into),

            Some(Command::Acp(mut args)) => {
                if args.log_level.is_none() {
                    args.log_level = top_log_level;
                }
                rt.block_on(run_acp(args))
                    .map(|outcome| match outcome {
                        AcpRunOutcome::CleanDisconnect => ExitCode::SUCCESS,
                    })
                    .map_err(Into::into)
            }

            Some(Command::Server(args)) => {
                rt.block_on(run_server(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into)
            }

            Some(Command::Client(args)) => {
                rt.block_on(run_client(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into)
            }

            Some(Command::ShowPrompt(args)) => {
                rt.block_on(run_prompt(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into)
            }

            Some(Command::Mcp(args)) => {
                rt.block_on(run_mcp_command(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into)
            }

            Some(Command::Settings(SettingsCommand::Init(args))) => rt.block_on(run_init_command(args.into())),

            Some(Command::Lspd(args)) => aether_lspd::run_lspd(args).map(|()| ExitCode::SUCCESS).map_err(Into::into),

            None => rt.block_on(run_default_command(cli.model, top_log_level)),
        }
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            match e {
                MainError::Mcp(error) => ExitCode::from(error.exit_code()),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run_init_command(request: InitRequest) -> Result<ExitCode, MainError> {
    let outcome = run_init(request).await?;
    if let Some(msg) = next_steps_message(&outcome) {
        println!("{msg}");
    }

    Ok(match outcome {
        InitOutcome::Applied { .. } | InitOutcome::AlreadyInitialized { .. } | InitOutcome::Cancelled => {
            ExitCode::SUCCESS
        }
    })
}

async fn run_default_command(model: Option<String>, log_level: Option<LogLevel>) -> Result<ExitCode, MainError> {
    if let Some(model) = model.as_deref() {
        validate_model_override(model)?;
    }

    let cwd = current_dir()?;
    let existing_settings = {
        let mut paths = Vec::new();
        if let Some(path) = user_settings_path().filter(|path| path.is_file()) {
            paths.push(path);
        }

        let project_path = project_settings_path(&cwd);
        if project_path.is_file() {
            paths.push(project_path);
        }

        paths
    };

    if AgentCatalog::load_default(&cwd)
        .map_err(|error| MainError::Settings(invalid_settings_message(&existing_settings, error)))?
        .is_none()
    {
        let outcome = run_init(InitRequest::user_onboarding()).await?;
        if let Some(msg) = next_steps_message(&outcome) {
            println!("{msg}");
        }

        match outcome {
            InitOutcome::Cancelled | InitOutcome::Applied { missing_env_var: Some(_), .. } => {
                return Ok(ExitCode::SUCCESS);
            }
            InitOutcome::Applied { missing_env_var: None, .. } | InitOutcome::AlreadyInitialized { .. } => {}
        }
    }

    let settings = load_or_create_settings().with_default_status_line(default_status_line());
    run_tui(&default_agent_command(model.as_deref(), log_level), settings, None)
        .await
        .map(|()| ExitCode::SUCCESS)
        .map_err(Into::into)
}

fn default_status_line() -> StatusLineSettings {
    StatusLineSettings {
        separator: Some(" · ".to_string()),
        left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef]),
        right: Some(vec![
            StatusLineSegmentConfig::Mode,
            StatusLineSegmentConfig::Model { max_width: None },
            StatusLineSegmentConfig::Reasoning,
            StatusLineSegmentConfig::Context,
            StatusLineSegmentConfig::ServerHealth,
        ]),
    }
}

/// Load the config and report whether it is valid. The load performs the
/// same JSON / schema validation the run path sees; on failure `main`
/// prints the error to stderr and returns `ExitCode::FAILURE` via the
/// `MainError::Settings` variant. Does not start a run, build a tokio
/// runtime, or contact any provider.
fn check_config(source: &SettingsSourceArgs) -> Result<(), MainError> {
    let cwd = current_dir()?;
    source.load_settings(&cwd).map_err(|e| MainError::Settings(e.to_string()))?;
    println!("Configuration is valid");
    Ok(())
}

/// Print every profile (agent) name from the loaded config, one per line, and
/// return success even when the config defines zero agents. Reads the raw
/// `AetherSettings` rather than going through `AgentCatalog`: the catalog
/// requires a user-invocable agent and would refuse an empty one with
/// `NoUserInvocableAgents`, which the task's "no profiles" case rejects.
fn list_profiles(source: &SettingsSourceArgs) -> Result<(), MainError> {
    let cwd = current_dir()?;
    let settings = source.load_settings(&cwd).map_err(|e| MainError::Settings(e.to_string()))?;
    for agent in &settings.agents {
        println!("{}", agent.name);
    }
    Ok(())
}
fn invalid_settings_message(paths: &[std::path::PathBuf], error: impl std::fmt::Display) -> String {
    format!(
        "Found settings at {}, but they are invalid: {error}\nRun `aether settings init --user --force` to replace user settings, `aether settings init --project --force` to replace project settings, or edit the settings JSON manually.",
        format_settings_paths(paths)
    )
}

fn format_settings_paths(paths: &[std::path::PathBuf]) -> String {
    if paths.is_empty() {
        "the default locations".to_string()
    } else {
        paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
    }
}

/// Build the agent-subprocess command string passed to the wisp TUI.
/// Without an override, the TUI spawns `aether acp` (which reads the
/// configured model from settings). With `--model`, the same `aether acp`
/// command is launched and handed the override via argv so the configured
/// model is bypassed for this run only. `--log-level` is forwarded the same
/// way so the spawned child sees the same tracing level the TUI was given.
fn default_agent_command(model: Option<&str>, log_level: Option<LogLevel>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = model {
        parts.push(format!("--model {model}"));
    }
    if let Some(level) = log_level {
        // `LogLevel`'s clap render yields the lowercase variant names; we
        // write them out explicitly so a future rename of the enum is caught
        // by the `default_agent_command_*` unit tests below.
        let rendered = match level {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        };
        parts.push(format!("--log-level {rendered}"));
    }
    if parts.is_empty() { "aether acp".to_string() } else { format!("aether acp {}", parts.join(" ")) }
}

/// Reject any `--model` value that is not parseable as an `LlmModel`.
/// The validated model string is then interpolated into the agent
/// subprocess argv, so this also constrains what can land on the child's
/// command line.
fn validate_model_override(model: &str) -> Result<(), MainError> {
    if model.parse::<LlmModel>().is_err() {
        return Err(MainError::Cli(CliError::ModelError(format!(
            "--model '{model}' is not a recognised provider:model id"
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_command_without_override_uses_configured_agent() {
        assert_eq!(default_agent_command(None, None), "aether acp");
    }

    #[test]
    fn default_agent_command_with_override_interpolates_model() {
        let command = default_agent_command(Some("anthropic:claude-sonnet-4-5"), None);
        assert_eq!(command, "aether acp --model anthropic:claude-sonnet-4-5");
        assert!(command.contains("anthropic:claude-sonnet-4-5"));
    }

    #[test]
    fn model_flag_overrides_the_configured_model() {
        let command = default_agent_command(Some("zai:glm-5.1"), None);
        assert_eq!(command, "aether acp --model zai:glm-5.1");
        assert!(command.starts_with("aether acp --model zai:glm-5.1"));
    }

    #[test]
    fn default_agent_command_forwards_log_level_only() {
        assert_eq!(default_agent_command(None, Some(LogLevel::Debug)), "aether acp --log-level debug");
    }

    #[test]
    fn default_agent_command_forwards_model_and_log_level() {
        // The two flags travel together when both are set, in the same
        // --model / --log-level order the function builds them in.
        assert_eq!(
            default_agent_command(Some("anthropic:claude-sonnet-4-5"), Some(LogLevel::Error)),
            "aether acp --model anthropic:claude-sonnet-4-5 --log-level error"
        );
    }

    #[test]
    fn validate_model_override_accepts_a_known_catalog_model() {
        validate_model_override("anthropic:claude-sonnet-4-5").expect("known catalog model should validate");
    }

    #[test]
    fn validate_model_override_accepts_dynamic_providers() {
        validate_model_override("ollama:llama3.2").expect("ollama strings should validate");
    }

    #[test]
    fn validate_model_override_rejects_unknown_models() {
        let error = validate_model_override("mystery:not-a-model").unwrap_err();
        match error {
            MainError::Cli(CliError::ModelError(message)) => {
                assert!(message.contains("mystery:not-a-model"));
            }
            other => panic!("expected ModelError, got {other:?}"),
        }
    }
}
