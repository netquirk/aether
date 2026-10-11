use aether_auth::OAuthError;
use aether_project::SettingsError;
use aether_telemetry::TelemetryInitError;
use std::io;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("No prompt provided. Pass a prompt as an argument or pipe via stdin.")]
    NoPrompt,
    #[error("{0}")]
    ConflictingArgs(String),
    #[error("Invalid --options-json: {0}")]
    InvalidOptionsJson(#[source] serde_json::Error),
    #[error(transparent)]
    ConflictingSettingsSources(#[from] crate::settings_args::ConflictingSettingsSources),
    #[error("Failed to load settings: {0}")]
    Settings(#[from] SettingsError),
    #[error("Failed to initialize telemetry: {0}")]
    Telemetry(#[from] TelemetryInitError),
    #[error("Model error: {0}")]
    ModelError(String),
    #[error("MCP error: {0}")]
    McpError(String),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("failed to open log file {path}: {source}")]
    LogFileOpen { path: PathBuf, source: io::Error },
    #[error("failed to read system prompt from {path}: {source}")]
    SystemPromptFile { path: PathBuf, source: io::Error },
    #[error("Agent error: {0}")]
    AgentError(String),
    #[error("Credential store error: {0}")]
    CredentialStore(#[from] OAuthError),
    /// A second run tried to use a working directory another run already holds
    /// (TASK-26-21). The run is refused before any agent build or transcript
    /// write so two `aether` runs in the same working directory cannot clobber
    /// each other's files. The message names the lock file so the operator can
    /// delete it if no run is actually active (a stale file is harmless in the
    /// common case because the OS releases advisory locks on process exit;
    /// the manual path is only needed after a hard kill on a non-Linux
    /// platform that does not release the lock cleanly).
    #[error(
        "cannot start run: another aether run is already using this working directory. \
Its lock file is {path}; delete it if no run is active."
    )]
    RunLocked { path: PathBuf },
}
