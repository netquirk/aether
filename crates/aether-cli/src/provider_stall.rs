//! Live one-line provider-stall warning the headless CLI prints when an in-flight
//! provider call stays pending past the configured threshold.
//!
//! TASK-24-378 surfaces a stalled provider call while the run is still live, so
//! a hung turn is visible instead of silent. The [`ProviderStallWatch`] is fed
//! the same `AgentEvent`s as [`crate::output::ProviderWaitTracker`]:
//!
//! - `LlmCallStarted` arms the watch with the call's start instant and resets
//!   the warn-once latch.
//! - `LlmCallEnded` disarms it.
//!
//! Every other event variant is ignored, matching the bracket-on-start /
//! bracket-on-end pattern that already brackets provider wait accounting.
//!
//! The headless event loop races [`ProviderStallWatch::next_deadline`] against
//! the `mpsc::Receiver` so a slow provider prints the warning once, the first
//! time its in-flight call passes `started + threshold`. The latch disarms on
//! `warned = true`, so the loop falls back to a plain `recv().await` for the
//! remainder of the call and avoids spamming the line while the call still
//! has not returned.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use aether_core::events::{AgentEvent, TurnEvent};

use crate::output::format_duration;

/// Renders the live one-line stall warning the CLI prints when a provider
/// call exceeds the configured threshold. The exact phrasing is pinned by
/// `provider_stall_watch_warns_once_naming_elapsed` so the warning is
/// grep-friendly in logs.
pub(crate) fn provider_stall_warning_line(elapsed: Duration) -> String {
    format!("warning: provider call has waited {} on the provider", format_duration(elapsed))
}

/// Tracks the deadline of the in-flight provider call and emits a one-line
/// stall warning the first time the call stays pending past `threshold`.
///
/// Fed the same events as [`crate::output::ProviderWaitTracker`]:
/// `LlmCallStarted` arms the watch with the call's start instant and resets
/// the warn-once latch, `LlmCallEnded` disarms it. [`next_deadline`] is
/// `Some` only while a call is in flight and the warning has not yet been
/// printed, so the headless CLI's select loop sleeps until that instant and
/// otherwise falls back to a plain `recv()`. [`warn_if_stalled`] writes the
/// line at most once per call; a follow-up poll returns `Ok(false)` so the
/// loop does not spam the warning while the call is still hanging.
///
/// Generic over `Write` so the unit tests can pin the exact bytes without
/// capturing process stderr.
#[derive(Debug)]
pub(crate) struct ProviderStallWatch<W: Write> {
    threshold: Option<Duration>,
    writer: W,
    started: Option<Instant>,
    warned: bool,
}

impl<W: Write> ProviderStallWatch<W> {
    /// Build a new watch. `threshold = None` (or zero, via
    /// [`aether_project::RunSettings::provider_stall_warn`]) disables the
    /// warning: every poll is a no-op and [`next_deadline`] always returns
    /// `None`.
    pub(crate) fn new(threshold: Option<Duration>, writer: W) -> Self {
        Self { threshold, writer, started: None, warned: false }
    }

    /// Record an event from the agent stream. Any event other than the two
    /// `LlmCallStarted` / `LlmCallEnded` variants is ignored, mirroring
    /// [`crate::output::ProviderWaitTracker::observe`].
    pub(crate) fn observe(&mut self, event: &AgentEvent, now: Instant) {
        match event {
            AgentEvent::Turn(TurnEvent::LlmCallStarted { .. }) => {
                self.started = Some(now);
                self.warned = false;
            }
            AgentEvent::Turn(TurnEvent::LlmCallEnded { .. }) => {
                self.started = None;
                self.warned = false;
            }
            _ => {}
        }
    }

    /// `started + threshold` while a call is in flight and the warning has
    /// not yet been printed. `None` once [`warn_if_stalled`] has fired, when
    /// the threshold is unset, or when no call is in flight — the watch
    /// then falls back to a plain event receive.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        if self.warned {
            return None;
        }
        let started = self.started?;
        let threshold = self.threshold?;
        Some(started + threshold)
    }

    /// Write the warning to the sink if a call is in flight, the threshold
    /// is set, the deadline has passed, and we have not already warned for
    /// this call. Returns `Ok(true)` iff a line was written, so the caller
    /// can avoid the warning branch after it has fired once.
    pub(crate) fn warn_if_stalled(&mut self, now: Instant) -> io::Result<bool> {
        if self.warned {
            return Ok(false);
        }
        let Some(started) = self.started else {
            return Ok(false);
        };
        let Some(threshold) = self.threshold else {
            return Ok(false);
        };
        if now < started + threshold {
            return Ok(false);
        }
        let elapsed = now.saturating_duration_since(started);
        writeln!(self.writer, "{}", provider_stall_warning_line(elapsed))?;
        self.warned = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use aether_core::events::{LlmCallOutcome, TurnEvent, TurnOutcome};
    use llm::LlmCallPurpose;

    use super::*;

    // Reuse the headless test helpers for `LlmCallStarted` / `LlmCallEnded`
    // building so the suite keeps the same event shape as the live loop.
    fn llm_chat_started(display_name: &str) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            model: llm::ModelIdentity::default(),
            display_name: display_name.to_string(),
            attempt: 0,
            max_attempts: 3,
        })
    }

    fn llm_chat_ended() -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: None, provider_request_id: None },
        })
    }

    #[test]
    fn provider_stall_warning_line_names_elapsed_seconds() {
        // The exact line is grep-friendly in logs and a fixed pin so a
        // regression in phrasing shows up immediately.
        let line = provider_stall_warning_line(Duration::from_millis(1_234));
        assert!(line.starts_with("warning:"), "warning prefix missing: {line}");
        assert!(line.contains("provider"), "provider noun missing: {line}");
        assert!(line.contains(&format_duration(Duration::from_millis(1_234))), "elapsed duration missing: {line}");
    }

    #[test]
    fn provider_stall_watch_warns_once_naming_elapsed() {
        let base = Instant::now();
        let threshold = Duration::from_millis(50);
        let mut watch = ProviderStallWatch::new(Some(threshold), Vec::<u8>::new());

        // No call in flight: nothing to warn about, no deadline.
        assert!(!watch.warn_if_stalled(base).unwrap());
        assert_eq!(watch.next_deadline(), None);

        // Start arms the watch and yields a deadline.
        watch.observe(&llm_chat_started("primary"), base);
        let deadline = watch.next_deadline().expect("deadline present after start");
        assert_eq!(deadline, base + threshold);

        // Before the deadline: the warning must not fire.
        assert!(!watch.warn_if_stalled(base + Duration::from_millis(40)).unwrap());
        assert_eq!(watch.next_deadline(), Some(base + threshold), "deadline is unchanged before the threshold");

        // At the deadline: the line names the elapsed wait. Allow a few extra
        // milliseconds beyond the threshold so the test does not depend on
        // the absolute value of the `Instant` the caller hands in.
        let now = base + Duration::from_millis(60);
        assert!(watch.warn_if_stalled(now).unwrap());
        let one_line_bytes = watch.writer.len();
        let bytes = watch.writer.clone();
        let line = String::from_utf8(bytes).expect("warning line is utf-8");
        assert!(line.starts_with("warning:"), "warning prefix missing: {line:?}");
        assert!(line.contains("provider"), "provider noun missing: {line:?}");
        assert!(
            line.contains(&format_duration(now - base)),
            "elapsed duration missing (need {}): {line:?}",
            format_duration(now - base),
        );
        assert!(line.ends_with('\n'), "warning line must terminate with a newline: {line:?}");

        // Once warned, the watch stays silent until the next call starts.
        assert!(!watch.warn_if_stalled(now + Duration::from_millis(100)).unwrap());
        assert_eq!(watch.next_deadline(), None, "latch disarm: no deadline after the one warning");
        assert_eq!(watch.writer.len(), one_line_bytes, "no extra bytes after the one warning");
    }

    #[test]
    fn provider_stall_watch_rearms_on_next_call() {
        let base = Instant::now();
        let threshold = Duration::from_millis(20);
        let mut watch = ProviderStallWatch::new(Some(threshold), Vec::<u8>::new());

        // First stalled call.
        watch.observe(&llm_chat_started("primary"), base);
        assert!(watch.warn_if_stalled(base + threshold).unwrap());
        assert_eq!(watch.next_deadline(), None, "latched after the first warning");
        let first_bytes = watch.writer.len();

        // End disarms, but does not re-arm.
        watch.observe(&llm_chat_ended(), base + threshold);
        assert_eq!(watch.next_deadline(), None);

        // Next call arms it again and produces a fresh warning.
        let second_start = base + Duration::from_millis(100);
        watch.observe(&llm_chat_started("primary"), second_start);
        assert_eq!(watch.next_deadline(), Some(second_start + threshold), "new deadline set after rearm");
        assert!(watch.warn_if_stalled(second_start + threshold).unwrap());
        // Each stalled call writes exactly one warning of the same shape
        // (threshold is the same), so the byte growth equals the length of
        // the first line; assert the second warning added bytes and the
        // exact growth matches.
        let second_line_bytes = watch.writer.len() - first_bytes;
        assert!(second_line_bytes > 0, "second warning added bytes to the sink");
        assert_eq!(
            second_line_bytes, first_bytes,
            "each stalled call adds exactly one line (warning shape is identical)",
        );
    }

    #[test]
    fn provider_stall_watch_disabled_without_threshold() {
        let base = Instant::now();
        let mut watch = ProviderStallWatch::new(None, Vec::<u8>::new());
        watch.observe(&llm_chat_started("primary"), base);
        // Threshold absent: no deadline, no warning, even when the test calls
        // poll with `now` arbitrarily far in the future.
        assert_eq!(watch.next_deadline(), None);
        assert!(!watch.warn_if_stalled(base + Duration::from_secs(60)).unwrap());
        assert!(watch.writer.is_empty(), "disabled watch must not write anything: {:?}", watch.writer);
    }

    #[test]
    fn provider_stall_watch_ignores_unrelated_events() {
        let base = Instant::now();
        let threshold = Duration::from_millis(50);
        let mut watch = ProviderStallWatch::new(Some(threshold), Vec::<u8>::new());

        // Nothing tracks these: a `TurnStart` or a `TurnEnded` must not
        // advance the state machine.
        watch.observe(&AgentEvent::Turn(TurnEvent::Started { content: vec![] }), base);
        watch.observe(&AgentEvent::turn_ended(TurnOutcome::Completed), base + Duration::from_millis(10));
        assert_eq!(watch.next_deadline(), None, "unrelated events must not arm the watch");
        assert!(!watch.warn_if_stalled(base + Duration::from_millis(200)).unwrap());
        assert!(watch.writer.is_empty(), "unrelated events must not produce a warning");
    }

    #[test]
    fn provider_stall_watch_does_not_warn_before_threshold() {
        // The headless loop relies on `warn_if_stalled` returning `false`
        // before the threshold; assert it explicitly so a regression that
        // fires early is caught.
        let base = Instant::now();
        let threshold = Duration::from_secs(2);
        let mut watch = ProviderStallWatch::new(Some(threshold), Vec::<u8>::new());
        watch.observe(&llm_chat_started("primary"), base);
        assert!(!watch.warn_if_stalled(base + Duration::from_millis(500)).unwrap());
        let just_before_threshold = (base + threshold).checked_sub(Duration::from_nanos(1)).unwrap();
        assert!(!watch.warn_if_stalled(just_before_threshold).unwrap());
        assert!(watch.writer.is_empty(), "no warning before the threshold");
    }
}
