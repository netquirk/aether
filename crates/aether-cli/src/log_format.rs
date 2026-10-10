//! Shared `--log-format` enum for the `aether` CLI.
//!
//! Two output formats for the run's tracing log: the default human-readable
//! `text` (unchanged) and `json` (one JSON object per line, each carrying at
//! least `level`, `message`, and `timestamp`). The flag composes with
//! `--log-file PATH` (TASK-25-48): JSON output is written to the same path the
//! text format would have been, so a single run can be redirected to a file
//! in either format.

/// Output format for the run's tracing log.
///
/// The `clap` derive renders the variants in their default `lower` form
/// (`text`, `json`) so no `rename_all` attribute is needed. The `Default`
/// impl is derived with `Text` as the default variant so the absent-flag
/// path matches the pre-existing `setup_tracing` output exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    /// Human-readable text (the default; matches the pre-existing
    /// `setup_tracing` output). Each line is a `tracing` event rendered with
    /// the standard `fmt` layer; ANSI colour codes are emitted only when
    /// stderr is the destination and `NO_COLOR` is not set.
    #[default]
    Text,
    /// One JSON object per line. Each line carries top-level `timestamp`,
    /// `level`, `message`, and `target` fields; `flatten_event(true)` puts
    /// the message at the top level rather than under a `fields` key, and
    /// ANSI colour codes are always disabled so the on-disk / on-stderr log
    /// is a stable format.
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_is_text() {
        // The absent-flag path is plain text; this is the regression
        // anchor for the "default output does not change" requirement.
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }

    #[test]
    fn default_format_does_not_equal_json() {
        // Defensive: a future rename must not accidentally swap the
        // default to JSON and silently change every caller's output.
        assert_ne!(LogFormat::default(), LogFormat::Json);
    }
}
