//! Shared `--log-level` enum for the `aether` CLI.
//!
//! Four named verbosity values (`error`, `warn`, `info`, `debug`) that map
//! to `tracing_subscriber` `EnvFilter` directives. The `agent=off` suffix
//! silences the `agent` crate so its loud model-internal events do not drown
//! out our own logs, matching the behaviour the previous
//! `--verbose`/`setup_tracing` path established.

/// Verbosity for the run's tracing output.
///
/// The `clap` derive renders the variants in their default `lower` form
/// (`error`, `warn`, `info`, `debug`) so no `rename_all` attribute is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum LogLevel {
    /// `error` and above
    Error,
    /// `warn` and above (the default for a non-verbose run)
    Warn,
    /// `info` and above
    Info,
    /// `debug` and above
    Debug,
}

impl LogLevel {
    /// The `EnvFilter` directive that reproduces this level. The `agent=off`
    /// suffix matches the existing `setup_tracing` filter so the `agent`
    /// crate's internal events stay silenced regardless of the run's level.
    pub fn directive(self) -> &'static str {
        match self {
            LogLevel::Error => "error,agent=off",
            LogLevel::Warn => "warn,agent=off",
            LogLevel::Info => "info,agent=off",
            LogLevel::Debug => "debug,agent=off",
        }
    }
}

/// Resolve the effective `LogLevel` from the explicit flag and the legacy
/// `--verbose` boolean. `--log-level` always wins; without it, `--verbose`
/// keeps its existing behaviour (`Debug`), and the no-flag default stays
/// `Warn` to match the pre-existing `setup_tracing(false)` branch.
pub fn resolve(level: Option<LogLevel>, verbose: bool) -> LogLevel {
    level.unwrap_or(if verbose { LogLevel::Debug } else { LogLevel::Warn })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_matches_each_level() {
        assert_eq!(LogLevel::Error.directive(), "error,agent=off");
        assert_eq!(LogLevel::Warn.directive(), "warn,agent=off");
        assert_eq!(LogLevel::Info.directive(), "info,agent=off");
        assert_eq!(LogLevel::Debug.directive(), "debug,agent=off");
    }

    #[test]
    fn resolve_prefers_explicit_level_over_verbose() {
        // `--log-level` wins when both are set: callers asking for a quieter
        // run should not get `debug` because `--verbose` is also on the line.
        assert_eq!(resolve(Some(LogLevel::Error), true), LogLevel::Error);
        assert_eq!(resolve(Some(LogLevel::Info), true), LogLevel::Info);
        assert_eq!(resolve(Some(LogLevel::Warn), true), LogLevel::Warn);
        assert_eq!(resolve(Some(LogLevel::Debug), true), LogLevel::Debug);
    }

    #[test]
    fn resolve_falls_back_to_verbose_when_level_absent() {
        assert_eq!(resolve(None, true), LogLevel::Debug);
        assert_eq!(resolve(None, false), LogLevel::Warn);
    }
}
