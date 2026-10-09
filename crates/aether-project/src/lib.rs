#![doc = include_str!("../README.md")]

pub mod aether_settings;
mod agent_catalog;
mod agent_config;
mod error;
mod mcp_config_source_config;
mod prompt_catalog;
pub mod prompt_file;
#[cfg(feature = "testing")]
pub mod testing;

pub use aether_core::core::{PromptSource, PromptSourceError};
pub use aether_core::mcp::ToolOutputSettings;
pub use aether_settings::{
    AETHER_TOOL_OUTPUT_MAX_BYTES_ENV, AetherSettings, AetherSettingsSource, CredentialsStoreConfig,
    OtlpTelemetrySettings, PRAIRIE_TOOL_OUTPUT_DIR_ENV, SettingsFileSource, TelemetryContentSettings,
    TelemetrySettings, project_settings_exist, project_settings_path, resolve_tool_output_cap, settings_resource_root,
    tool_output_dir_from_env, tool_output_max_bytes_from_env, user_settings_exist, user_settings_path,
};
pub use agent_catalog::AgentCatalog;
pub use agent_config::AgentConfig;
pub use error::SettingsError;
pub use mcp_config_source_config::{McpFileSpec, McpSourceSpec};
pub use prompt_catalog::PromptCatalog;
pub use prompt_file::{PromptFile, PromptFileError, PromptTriggers, SKILL_FILENAME};
