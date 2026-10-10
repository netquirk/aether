use aether_project::{
    AetherSettings, AetherSettingsSource, AgentCatalog, SettingsError, SettingsFileSource, ToolOutputSettings,
};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Default, clap::Args)]
pub struct SettingsSourceArgs {
    #[arg(long = "settings-json", conflicts_with_all = ["settings_file", "config"])]
    pub settings_json: Option<String>,

    #[arg(
        long = "config",
        value_name = "PATH",
        conflicts_with_all = ["settings_json", "settings_file"]
    )]
    /// Read settings from PATH instead of the default user/project files.
    /// Mutually exclusive with `--settings-json` and `--settings-file`; both
    /// spellings (`--config` and `--settings-file`) load a single settings
    /// document with no implicit defaults merged in.
    pub config: Option<PathBuf>,

    #[arg(long = "settings-file", conflicts_with_all = ["settings_json", "config"])]
    pub settings_file: Option<PathBuf>,
}

/// A JSON options object supplied both an inline `settings` object and a `settingsFile`.
#[derive(Debug, Error)]
#[error("settings and settingsFile cannot both be supplied")]
pub struct ConflictingSettingsSources;

impl SettingsSourceArgs {
    pub fn from_json_options(
        settings: Option<AetherSettings>,
        settings_file: Option<PathBuf>,
    ) -> Result<Self, ConflictingSettingsSources> {
        if settings.is_some() && settings_file.is_some() {
            return Err(ConflictingSettingsSources);
        }
        Ok(Self {
            settings_json: settings.map(|settings| serde_json::to_string(&settings).expect("settings serialize")),
            config: None,
            settings_file,
        })
    }

    pub fn source(&self, root: &Path) -> Option<AetherSettingsSource> {
        if let Some(json) = &self.settings_json {
            Some(AetherSettingsSource::Json(json.clone()))
        } else if let Some(path) = &self.config {
            Some(AetherSettingsSource::File(SettingsFileSource::new(path.clone(), root)))
        } else {
            self.settings_file
                .as_ref()
                .map(|path| AetherSettingsSource::File(SettingsFileSource::new(path.clone(), root)))
        }
    }

    pub fn load_settings(&self, cwd: &Path) -> Result<AetherSettings, SettingsError> {
        match self.source(cwd) {
            Some(source) => AetherSettings::load(cwd, [source]),
            None => AetherSettings::load_default(cwd),
        }
    }

    pub fn load_agent_catalog(&self, cwd: &Path) -> Result<AgentCatalog, SettingsError> {
        let settings = self.load_settings(cwd)?;
        AgentCatalog::from_settings_or_empty(cwd, settings)
    }

    /// Return the top-level `toolOutput` block from the loaded settings, or
    /// `None` if the source is missing the block. Used by [`SessionFactory`]
    /// to thread the cap into the runtime without re-loading the catalog.
    pub fn tool_output_settings(&self) -> Result<Option<ToolOutputSettings>, SettingsError> {
        // The settings source may not point at a file at all (e.g. CLI args
        // with neither `--settings-json` nor `--settings-file`); in that case
        // fall through to the project/user defaults so the runtime still
        // picks up the top-level cap if one is declared.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let settings = self.load_settings(&cwd)?;
        Ok(settings.tool_output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_maps_to_json_source() {
        let args = SettingsSourceArgs {
            settings_json: Some("{\"agents\":[]}".to_string()),
            config: None,
            settings_file: None,
        };

        let Some(AetherSettingsSource::Json(json)) = args.source(Path::new(".")) else {
            panic!("expected JSON settings source");
        };
        assert_eq!(json, "{\"agents\":[]}");
    }

    #[test]
    fn from_json_options_serializes_inline_settings() {
        let args = SettingsSourceArgs::from_json_options(Some(AetherSettings::default()), None).unwrap();

        assert!(args.settings_json.is_some());
        assert!(args.config.is_none());
        assert!(args.settings_file.is_none());
    }

    #[test]
    fn from_json_options_rejects_both_settings_and_file() {
        let error = SettingsSourceArgs::from_json_options(
            Some(AetherSettings::default()),
            Some(PathBuf::from("settings.json")),
        );

        assert!(matches!(error, Err(ConflictingSettingsSources)));
    }

    #[test]
    fn settings_file_maps_to_file_source() {
        let args = SettingsSourceArgs {
            settings_json: None,
            config: None,
            settings_file: Some(PathBuf::from("settings.json")),
        };

        let Some(AetherSettingsSource::File(source)) = args.source(Path::new("/workspace")) else {
            panic!("expected file settings source");
        };
        assert_eq!(source, SettingsFileSource::new("settings.json", "/workspace"));
    }

    #[test]
    fn config_maps_to_required_file_source() {
        let args = SettingsSourceArgs {
            settings_json: None,
            config: Some(PathBuf::from("my-settings.json")),
            settings_file: None,
        };

        let Some(AetherSettingsSource::File(source)) = args.source(Path::new("/workspace")) else {
            panic!("expected file settings source");
        };
        assert_eq!(source, SettingsFileSource::new("my-settings.json", "/workspace"));
    }

    #[test]
    fn no_source_arg_returns_none_and_loads_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = SettingsSourceArgs::default();

        assert!(args.source(dir.path()).is_none());

        // `load_default` reads the user-level (`$AETHER_HOME/settings.json`)
        // and project-level (`.aether/settings.json`) files; we only assert it
        // returns `Ok` here because those files may legitimately exist on the
        // test host. The contract under test is that the *absent* `--config`
        // path calls `load_default` rather than a missing-file error.
        let settings = args.load_settings(dir.path()).expect("load_default with no --config");
        // Sanity-check: we always get a settings value back, and it is the
        // same type produced by the default loader.
        let _ = settings;
    }

    #[test]
    fn missing_config_path_returns_error_naming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist.json");
        let missing_for_assert = missing.clone();

        let args = SettingsSourceArgs { settings_json: None, config: Some(missing), settings_file: None };

        let error = args.load_settings(dir.path()).expect_err("missing --config path must fail");
        let rendered = error.to_string();
        let expected_path = missing_for_assert.to_string_lossy().into_owned();

        assert!(rendered.contains(&expected_path), "error {rendered:?} must name the path {expected_path:?}");
    }
}
