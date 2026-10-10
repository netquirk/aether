//! Live wall-clock timeout for the headless CLI's run. The watch holds the
//! `Instant` at which the caller-chosen limit is reached, and emits one line
//! the first time that deadline passes so a hung run is no longer silent.
//!
//! TASK-25-18 surfaces a caller-capped wall-clock timeout (`aether headless
//! --timeout 30s`): the run ends once the limit is reached, the watch writes
//! a single line naming the limit to the supplied `Write` sink, and the
//! headless event loop returns the distinct [`TIMEOUT_EXIT_CODE`] exit code
//! instead of waiting forever for an event that may never arrive.
//!
//! The deadline takes precedence over the provider-stall warning: when both
//! the stall deadline and the run deadline are due at the same time, the
//! select loop fires the timeout branch and returns. [`expire_if_due`] latches
//! on first write so a follow-up poll returns `Ok(false)` even if the headless
//! loop's `select!` wakes again on the same tick; the line is printed exactly
//! once per run.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::output::format_duration;

/// Distinct exit code the headless CLI returns when a run is capped by
/// `--timeout`. Mirrors `timeout(1)`'s convention (124) so callers that
/// already key off `timeout(1)`'s exit code in their tooling recognise it
/// without extra wiring. Distinct from [`std::process::ExitCode::SUCCESS`]
/// (`0`) and [`std::process::ExitCode::FAILURE`] (`1`).
pub(crate) const TIMEOUT_EXIT_CODE: u8 = 124;

/// Render the one-line timeout message the headless CLI prints when a run
/// exceeds the caller-supplied limit. Pinned by
/// `run_timeout_line_names_limit` so a regression in phrasing shows up
/// immediately.
pub(crate) fn run_timeout_line(limit: Duration) -> String {
    format!("run timed out after {}", format_duration(limit))
}

/// Tracks the wall-clock deadline for a `--timeout`-bounded run and emits a
/// one-line message the first time the deadline passes.
///
/// Constructed once at the top of `stream_output`, fed no events: the deadline
/// is fixed when the run starts (`started + limit`). [`deadline`] is
/// `Some(deadline)` while the run is still inside its budget and `None` once
/// [`expire_if_due`] has fired (so the headless loop falls back to a plain
/// `recv().await` and exits through the timeout branch exactly once).
///
/// Generic over `Write` so the unit tests can pin the exact bytes without
/// capturing process stderr.
#[derive(Debug)]
pub(crate) struct RunTimeoutWatch<W: Write> {
    limit: Option<Duration>,
    deadline: Option<Instant>,
    writer: W,
    expired: bool,
}

impl<W: Write> RunTimeoutWatch<W> {
    /// Build a new watch. `limit = None` (or zero, see below) disables the
    /// timeout: every poll is a no-op, [`deadline`] always returns `None`,
    /// and the headless loop falls back to the plain `recv()` path.
    ///
    /// `limit = Some(zero)` is treated as disabled rather than "expire
    /// immediately" so `parse_timeout`'s reject of values below one second
    /// (TASK-25-168; `0s` is the only zero-shaped input that still reaches
    /// this branch) matches a binary build that constructs a `RunConfig`
    /// directly without going through the parser.
    pub(crate) fn new(limit: Option<Duration>, started: Instant, writer: W) -> Self {
        let deadline = limit.filter(|limit| !limit.is_zero()).and_then(|limit| started.checked_add(limit));
        Self { limit: limit.filter(|limit| !limit.is_zero()), deadline, writer, expired: false }
    }

    /// The instant the timeout fires (i.e. `started + limit`). `None` once
    /// the timeout has already fired, when the limit was disabled, or when
    /// the watch has not been armed for any reason. The select loop in
    /// `stream_output` sleeps until this instant and races it against the
    /// provider-stall deadline so the earlier of the two wakes the loop.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        if self.expired {
            return None;
        }
        self.deadline
    }

    /// Write the timeout line to the sink if a limit was set, the deadline
    /// has passed, and we have not already fired for this run. Returns
    /// `Ok(true)` iff a line was written, so the caller can exit the loop
    /// once the timeout has fired instead of polling again.
    pub(crate) fn expire_if_due(&mut self, now: Instant) -> io::Result<bool> {
        if self.expired {
            return Ok(false);
        }
        let Some(limit) = self.limit else {
            return Ok(false);
        };
        let Some(deadline) = self.deadline else {
            return Ok(false);
        };
        if now < deadline {
            return Ok(false);
        }
        writeln!(self.writer, "{}", run_timeout_line(limit))?;
        self.expired = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_timeout_line_names_limit() {
        // The exact line is grep-friendly in logs and a fixed pin so a
        // regression in phrasing shows up immediately. Note: the line
        // itself does not include the trailing newline -- the watch adds it
        // via `writeln!` so the bytes reach the sink as a single record.
        let line = run_timeout_line(Duration::from_millis(1_234));
        assert!(line.starts_with("run timed out"), "run-noun prefix missing: {line:?}");
        assert!(line.contains(&format_duration(Duration::from_millis(1_234))), "limit duration missing: {line:?}");
        assert!(!line.ends_with('\n'), "the line is rendered verbatim by `writeln!`: no trailing newline here");
    }

    #[test]
    fn run_timeout_watch_disabled_without_limit() {
        let base = Instant::now();
        let mut watch = RunTimeoutWatch::new(None, base, Vec::<u8>::new());
        assert_eq!(watch.deadline(), None, "limit `None` must disable the watch");
        assert!(!watch.expire_if_due(base + Duration::from_secs(60)).unwrap());
        assert!(watch.writer.is_empty(), "disabled watch must not write anything: {:?}", watch.writer);
    }

    #[test]
    fn run_timeout_watch_treats_zero_limit_as_disabled() {
        // `parse_timeout` rejects everything below one second (TASK-25-168),
        // including `0s`, but the constructor still has to behave when
        // called with `Some(Duration::ZERO)` (e.g. from a test that builds
        // `RunTimeoutWatch` directly). The watch must treat zero as
        // disabled rather than firing immediately; otherwise the headless
        // loop would time out any `--timeout 0` arg that slipped past the
        // parser.
        let base = Instant::now();
        let mut watch = RunTimeoutWatch::new(Some(Duration::ZERO), base, Vec::<u8>::new());
        assert_eq!(watch.deadline(), None, "zero limit must disable the watch");
        assert!(!watch.expire_if_due(base + Duration::from_secs(60)).unwrap());
        assert!(watch.writer.is_empty(), "zero-limit watch must not write anything");
    }

    #[test]
    fn run_timeout_watch_arms_with_started_plus_limit() {
        let base = Instant::now();
        let limit = Duration::from_secs(30);
        let watch = RunTimeoutWatch::new(Some(limit), base, Vec::<u8>::new());
        assert_eq!(watch.deadline(), Some(base + limit), "deadline is started + limit");
    }

    #[test]
    fn run_timeout_watch_does_not_fire_before_deadline() {
        let base = Instant::now();
        let limit = Duration::from_secs(30);
        let mut watch = RunTimeoutWatch::new(Some(limit), base, Vec::<u8>::new());

        // Before the deadline: no write, deadline is still armed.
        assert!(!watch.expire_if_due(base + Duration::from_millis(500)).unwrap());
        assert_eq!(watch.deadline(), Some(base + limit));
        assert!(watch.writer.is_empty(), "pre-deadline polling must not produce bytes");
    }

    #[test]
    fn run_timeout_watch_fires_once_at_deadline_and_disarms() {
        let base = Instant::now();
        let limit = Duration::from_secs(30);
        let mut watch = RunTimeoutWatch::new(Some(limit), base, Vec::<u8>::new());

        // At or past the deadline the watch writes the line exactly once.
        let now = base + limit;
        assert!(watch.expire_if_due(now).unwrap());
        let bytes = watch.writer.clone();
        let line = String::from_utf8(bytes).expect("timeout line is utf-8");
        assert!(line.contains(&format_duration(limit)), "limit duration missing: {line:?}");
        assert!(line.ends_with('\n'), "timeout line must terminate with a newline: {line:?}");

        // Once fired, the watch stays silent: the next poll returns Ok(false)
        // and the deadline arm releases (so the select loop falls back to a
        // plain `recv()` and the run's normal exit path takes over from the
        // caller's branch).
        let one_line_bytes = watch.writer.len();
        assert!(!watch.expire_if_due(now + Duration::from_secs(5)).unwrap());
        assert!(!watch.expire_if_due(now + Duration::from_secs(60)).unwrap());
        assert_eq!(watch.deadline(), None, "latched after firing: no deadline after the one expiration");
        assert_eq!(watch.writer.len(), one_line_bytes, "no extra bytes after the one line");
    }
}
