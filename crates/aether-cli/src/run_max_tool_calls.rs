//! Per-run cap on the number of tool calls the headless CLI's run makes
//! (TASK-25-260). The watch holds the caller-chosen limit and, fed the
//! already-counted total from [`crate::headless::run::RunSummary`], emits a
//! single line the first time the cap is reached so an over-talkative tool
//! chain is no longer silent.
//!
//! `aether headless --max-tool-calls N` ends a run once the model has
//! triggered N tool calls: the watch writes a single line naming
//! `--max-tool-calls` to the supplied `Write` sink, and the headless event
//! loop returns the distinct [`MAX_TOOL_CALLS_EXIT_CODE`] exit code
//! (`125`) instead of letting the cap run out. The cap lives on the event
//! loop only; MCP setup and the pre-loop agent build are not counted, which
//! matches the sibling `--timeout` flag's documented scope.
//!
//! The cap takes precedence over a regular `turn_ended` outcome: when both
//! the cap and a turn outcome are due at the same time, the headless loop
//! fires the cap branch and returns. [`stop_if_reached`] latches on first
//! write so a follow-up poll returns `Ok(false)` even if the headless loop
//! wakes again on the same tick; the line is printed exactly once per run.

use std::io::{self, Write};

/// Distinct exit code the headless CLI returns when a run is capped by
/// `--max-tool-calls`. Distinct from
/// [`crate::run_timeout::TIMEOUT_EXIT_CODE`] (124),
/// [`std::process::ExitCode::SUCCESS`] (0), and
/// [`std::process::ExitCode::FAILURE`] (1).
pub(crate) const MAX_TOOL_CALLS_EXIT_CODE: u8 = 125;

/// Render the one-line "cap reached" message the headless CLI prints when
/// the run reaches the caller-supplied `--max-tool-calls` cap. Pinned by
/// [`tests::run_max_tool_calls_line_names_flag`] so a regression in phrasing
/// shows up immediately. The line includes the flag verbatim
/// (`--max-tool-calls`) so callers can grep for it without ambiguity.
pub(crate) fn run_max_tool_calls_line(limit: u32) -> String {
    format!("run stopped after reaching --max-tool-calls {limit}")
}

/// Tracks the per-run tool-call cap and emits a one-line message the first
/// time the cap is reached.
///
/// Constructed once at the top of `stream_output`, fed the already-counted
/// total the loop's [`RunSummary`](crate::headless::run::RunSummary) tracks
/// after each event. When the watched count reaches the limit the watch
/// writes the line, latches, and returns `Ok(true)` so the headless loop
/// can exit through the cap branch exactly once.
///
/// Generic over `Write` so the unit tests can pin the exact bytes without
/// capturing process stderr.
#[derive(Debug)]
pub(crate) struct MaxToolCallsWatch<W: Write> {
    limit: Option<u32>,
    writer: W,
    stopped: bool,
}

impl<W: Write> MaxToolCallsWatch<W> {
    /// Build a new watch. `limit = None` (or zero, see below) disables the
    /// cap: every poll is a no-op and the headless loop falls back to the
    /// plain `turn_ended` exit path.
    ///
    /// `limit = Some(0)` is treated as disabled rather than "stop before
    /// any tool call" so `parse_max_tool_calls`'s reject of `0` matches a
    /// binary build that constructs a `RunConfig` directly without going
    /// through the parser.
    pub(crate) fn new(limit: Option<u32>, writer: W) -> Self {
        Self { limit: limit.filter(|limit| *limit != 0), writer, stopped: false }
    }

    /// Write the cap line to the sink if a limit was set, the cap has been
    /// reached, and we have not already fired for this run. Returns
    /// `Ok(true)` iff a line was written, so the caller can exit the loop
    /// once the cap has fired instead of polling again.
    ///
    /// `calls` is the count `RunSummary::record` reported for the most
    /// recent event, so the watch does not duplicate the loop's counting.
    pub(crate) fn stop_if_reached(&mut self, calls: u32) -> io::Result<bool> {
        if self.stopped {
            return Ok(false);
        }
        let Some(limit) = self.limit else {
            return Ok(false);
        };
        if calls < limit {
            return Ok(false);
        }
        writeln!(self.writer, "{}", run_max_tool_calls_line(limit))?;
        self.stopped = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_max_tool_calls_line_names_flag() {
        // The exact line is grep-friendly in logs and a fixed pin so a
        // regression in phrasing shows up immediately. The line itself does
        // not include the trailing newline -- the watch adds it via
        // `writeln!` so the bytes reach the sink as a single record.
        let line = run_max_tool_calls_line(7);
        assert!(line.contains("--max-tool-calls"), "line must name the flag verbatim: {line:?}");
        assert!(line.contains('7'), "line must include the supplied limit: {line:?}");
        assert!(!line.ends_with('\n'), "the line is rendered verbatim by `writeln!`: no trailing newline here");
    }

    #[test]
    fn max_tool_calls_watch_disabled_without_limit() {
        let mut watch = MaxToolCallsWatch::new(None, Vec::<u8>::new());
        assert!(!watch.stop_if_reached(0).unwrap());
        assert!(!watch.stop_if_reached(1_000).unwrap());
        assert!(watch.writer.is_empty(), "disabled watch must not write anything: {:?}", watch.writer);
    }

    #[test]
    fn max_tool_calls_watch_treats_zero_limit_as_disabled() {
        // `parse_max_tool_calls` rejects `0`, but the constructor still has
        // to behave when called with `Some(0)` (e.g. from a test that builds
        // `MaxToolCallsWatch` directly). The watch must treat zero as
        // disabled rather than firing immediately; otherwise the headless
        // loop would cap any `--max-tool-calls 0` arg that slipped past the
        // parser.
        let mut watch = MaxToolCallsWatch::new(Some(0), Vec::<u8>::new());
        assert!(!watch.stop_if_reached(0).unwrap());
        assert!(!watch.stop_if_reached(100).unwrap());
        assert!(watch.writer.is_empty(), "zero-limit watch must not write anything");
    }

    #[test]
    fn max_tool_calls_watch_does_not_fire_below_the_limit() {
        let mut watch = MaxToolCallsWatch::new(Some(3), Vec::<u8>::new());
        assert!(!watch.stop_if_reached(0).unwrap());
        assert!(!watch.stop_if_reached(1).unwrap());
        assert!(!watch.stop_if_reached(2).unwrap());
        assert!(watch.writer.is_empty(), "pre-cap polling must not produce bytes");
    }

    #[test]
    fn max_tool_calls_watch_fires_once_at_the_limit_and_disarms() {
        let mut watch = MaxToolCallsWatch::new(Some(3), Vec::<u8>::new());

        // At or past the limit the watch writes the line exactly once.
        assert!(watch.stop_if_reached(3).unwrap());
        let bytes = watch.writer.clone();
        let line = String::from_utf8(bytes).expect("cap line is utf-8");
        assert!(line.contains("--max-tool-calls"), "limit must name the flag: {line:?}");
        assert!(line.contains('3'), "limit must include the supplied count: {line:?}");
        assert!(line.ends_with('\n'), "cap line must terminate with a newline: {line:?}");

        // Once fired, the watch stays silent: the next poll returns Ok(false)
        // and no extra bytes are written.
        let one_line_bytes = watch.writer.len();
        assert!(!watch.stop_if_reached(4).unwrap());
        assert!(!watch.stop_if_reached(1_000).unwrap());
        assert_eq!(watch.writer.len(), one_line_bytes, "no extra bytes after the one line");
    }

    #[test]
    fn max_tool_calls_watch_fires_when_count_exceeds_the_limit() {
        // A run that overshoots the limit (e.g. the model queued two calls
        // and both land before the headless loop sees the first cap) still
        // gets a single line; the watch latches on the first firing.
        let mut watch = MaxToolCallsWatch::new(Some(2), Vec::<u8>::new());
        assert!(watch.stop_if_reached(5).unwrap());
        assert!(!watch.stop_if_reached(6).unwrap());
        let line = String::from_utf8(watch.writer.clone()).expect("cap line is utf-8");
        assert_eq!(line.lines().count(), 1, "overshoot must still produce exactly one line: {line:?}");
    }
}
