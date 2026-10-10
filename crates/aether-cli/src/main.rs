use aether_cli::acp::server::{ServerArgs, ServerRunError, run_server};
use aether_cli::acp::{AcpArgs, AcpRunError, AcpRunOutcome, run_acp};
use aether_cli::client::{ClientArgs, ClientRunError, run_client};
use aether_cli::error::CliError;
use aether_cli::generate_command::{GenerateArgs, GenerateCommandError, run as run_generate_command};
use aether_cli::headless::{HeadlessArgs, run_headless};
use aether_cli::init::{InitError, InitOutcome, InitRequest, next_steps_message, run_init};
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

    let result: Result<ExitCode, MainError> = if cli.list_profiles {
        list_profiles(&cli.settings_source).map(|()| ExitCode::SUCCESS)
    } else {
        let rt = Runtime::new().expect("Failed to create tokio runtime");
        match cli.command {
            Some(Command::Headless(args)) => rt.block_on(run_headless(args)).map_err(Into::into),

            Some(Command::Generate(args)) => rt.block_on(run_generate_command(args)).map_err(Into::into),

            Some(Command::Acp(args)) => rt
                .block_on(run_acp(args))
                .map(|outcome| match outcome {
                    AcpRunOutcome::CleanDisconnect => ExitCode::SUCCESS,
                })
                .map_err(Into::into),

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

            None => rt.block_on(run_default_command(cli.model)),
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

async fn run_default_command(model: Option<String>) -> Result<ExitCode, MainError> {
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
    run_tui(&default_agent_command(model.as_deref()), settings, None)
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
/// model is bypassed for this run only.
fn default_agent_command(model: Option<&str>) -> String {
    match model {
        Some(model) => format!("aether acp --model {model}"),
        None => "aether acp".to_string(),
    }
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
        assert_eq!(default_agent_command(None), "aether acp");
    }

    #[test]
    fn default_agent_command_with_override_interpolates_model() {
        let command = default_agent_command(Some("anthropic:claude-sonnet-4-5"));
        assert_eq!(command, "aether acp --model anthropic:claude-sonnet-4-5");
        assert!(command.contains("anthropic:claude-sonnet-4-5"));
    }

    #[test]
    fn model_flag_overrides_the_configured_model() {
        let command = default_agent_command(Some("zai:glm-5.1"));
        assert_eq!(command, "aether acp --model zai:glm-5.1");
        assert!(command.starts_with("aether acp --model zai:glm-5.1"));
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
