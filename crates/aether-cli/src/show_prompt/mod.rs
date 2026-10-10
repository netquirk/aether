mod run;

use crate::mcp_config_args::McpConfigArgs;
use crate::settings_args::SettingsSourceArgs;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct PromptArgs {
    /// Working directory
    #[arg(short = 'C', long = "cwd", default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Additional system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Named agent to inspect (defaults to first user-invocable agent)
    #[arg(short = 'a', long = "agent")]
    pub agent: Option<String>,

    /// Print each model-visible tool name (one per line) and exit
    #[arg(long = "list-tools")]
    pub list_tools: bool,

    /// Withhold a model-visible tool from the prompt inspection (TASK-25-123).
    /// Repeatable to drop more than one tool. Same matching rules as the
    /// headless `--disable-tool` flag.
    #[arg(long = "disable-tool", value_name = "NAME")]
    pub disable_tools: Vec<String>,
}

pub use run::run_prompt;
