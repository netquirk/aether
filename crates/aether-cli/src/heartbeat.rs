//! Live heartbeat log line the headless CLI emits at a configured interval
//! while an agent turn is in flight.
//!
//! TASK-26-111 surfaces a hung turn in the run log: during a long model call
//! the CLI logs a one-line heartbeat at the configured interval so a stalled
//! turn is no longer silent. The [`HeartbeatWatch`] is fed the agent stream's
//! turn events:
//!
//! - `TurnEvent::Started` arms the watch with the turn's start instant and
//!   sets the first deadline at `started + interval`.
//! - `TurnEvent::Ended` disarms the watch (clearing both the active turn
//!   and the pending deadline), so a turn that finishes before the first
//!   interval never logs anything.
//!
//! Every other event variant is ignored, matching the bracket-on-start /
//! bracket-on-end pattern that already brackets per-turn bookkeeping.
//!
//! The headless event loop races [`HeartbeatWatch::next_deadline`] against
//! the `mpsc::Receiver` (and the existing provider-stall and run-timeout
//! deadlines) so the earliest deadline wakes the loop. When the heartbeat
//! deadline is the earliest, [`HeartbeatWatch::beat_if_due`] writes one
//! `tracing::info!` line and the loop re-arms the next deadline at
//! `now + interval`. The watch is also disabled (`next_deadline = None`)
//! when the resolved interval is `None` — i.e. when the `run` block has
//! `heartbeatSeconds: 0` — so an operator who explicitly opts out never
//! sees the line.

use std::time::{Duration, Instant};

use aether_core::events::{AgentEvent, TurnEvent};

/// Exact log message the headless CLI emits once per heartbeat interval
/// while an agent turn is in flight (TASK-26-111). Pinned by
/// `heartbeat_message_is_grep_friendly` so a regression in phrasing shows
/// up immediately; the prefix `heartbeat:` keeps it grep-friendly in the
/// run log and distinct from the existing `warning: provider call has
/// waited ...` provider-stall line.
pub(crate) const HEARTBEAT_MESSAGE: &str = "heartbeat: agent turn still in flight";

/// Tracks the heartbeat interval of an in-flight turn and emits one
/// `tracing::info!` line per interval while the turn is still active.
///
/// Fed the same events as the existing per-turn bookkeeping:
/// `TurnEvent::Started` arms the watch with the turn's start instant and
/// sets the first deadline at `started + interval`, `TurnEvent::Ended`
/// disarms it. [`next_deadline`] is `Some` only while a turn is in flight
/// (and the interval is non-zero), so the headless CLI's select loop
/// sleeps until that instant and otherwise falls back to a plain `recv()`.
/// [`beat_if_due`] writes one `info!` line per interval and advances the
/// deadline to `now + interval` so a subsequent poll returns the next
/// deadline. When `interval = None` (the `run.heartbeatSeconds = 0` case),
/// every poll is a no-op and `next_deadline` always returns `None`.
#[derive(Debug)]
pub(crate) struct HeartbeatWatch {
    interval: Option<Duration>,
    /// `Some(started)` while a turn is in flight; `None` otherwise. Tracks
    /// the turn's start so the emitted line can report the elapsed wait.
    active: Option<Instant>,
    /// `Some(started + interval)` while a turn is in flight and the watch
    /// is armed; `None` otherwise. Drives the select-loop's deadline race
    /// and advances by `interval` on every beat so the cadence is "every
    /// `interval` from the start", not "every `interval` from the last
    /// beat" (the latter would drift if a poll ever returned later than
    /// `next_beat`).
    next_beat: Option<Instant>,
}

impl HeartbeatWatch {
    /// Build a new watch. `interval = None` (or resolved from
    /// `run.heartbeatSeconds = 0` via
    /// [`aether_project::RunSettings::heartbeat_interval`]) disables the
    /// heartbeat: every poll is a no-op and [`next_deadline`] always
    /// returns `None`.
    pub(crate) fn new(interval: Option<Duration>) -> Self {
        Self { interval, active: None, next_beat: None }
    }

    /// Record an event from the agent stream. `TurnEvent::Started` arms
    /// the watch with the turn's start instant and sets the first deadline
    /// at `started + interval`; `TurnEvent::Ended` disarms it. Any other
    /// event variant is ignored, mirroring the bracket-on-start /
    /// bracket-on-end pattern the per-turn bookkeeping already follows.
    pub(crate) fn observe(&mut self, event: &AgentEvent, now: Instant) {
        match event {
            AgentEvent::Turn(TurnEvent::Started { .. }) => {
                self.active = Some(now);
                self.next_beat = self.interval.map(|interval| now + interval);
            }
            AgentEvent::Turn(TurnEvent::Ended { .. }) => {
                self.active = None;
                self.next_beat = None;
            }
            _ => {}
        }
    }

    /// `started + interval` while a turn is in flight and the interval is
    /// configured. `None` while no turn is active, when the interval is
    /// unset (`run.heartbeatSeconds = 0`), or once a turn has ended.
    /// Drives the deadline race in `await_with_deadlines` so the headless
    /// loop sleeps until that instant and otherwise falls back to a plain
    /// `recv()`.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.next_beat
    }

    /// Whether a turn is currently in flight. Used by `await_with_deadlines`
    /// to settle priority ties when several deadlines share the same
    /// instant; the heartbeat only races when the watch is active.
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Emit one `tracing::info!` line at [`HEARTBEAT_MESSAGE`] if the
    /// deadline has passed, advance `next_beat` to `now + interval`, and
    /// return `true`. Returns `false` when no turn is in flight, the
    /// interval is unset, or the deadline has not yet passed, so the
    /// headless loop can fall back to the plain `recv()` path.
    ///
    /// The cadence is anchored to `now` (not `next_beat`) so a late poll
    /// does not silently catch the watch up on multiple intervals — the
    /// next beat still fires `interval` after the previous one.
    pub(crate) fn beat_if_due(&mut self, now: Instant) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        let Some(deadline) = self.next_beat else {
            return false;
        };
        if now < deadline {
            return false;
        }
        let elapsed = self.active.map(|started| now.saturating_duration_since(started));
        self.next_beat = Some(now + interval);
        if let Some(elapsed) = elapsed {
            tracing::info!(elapsed_seconds = elapsed.as_secs(), "{HEARTBEAT_MESSAGE}");
        } else {
            tracing::info!("{HEARTBEAT_MESSAGE}");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_started() -> AgentEvent {
        AgentEvent::Turn(TurnEvent::Started { content: vec![] })
    }

    fn turn_ended() -> AgentEvent {
        AgentEvent::Turn(TurnEvent::Ended { outcome: aether_core::events::TurnOutcome::Completed })
    }

    #[test]
    fn heartbeat_message_is_grep_friendly() {
        // The exact line is grep-friendly in the run log (`heartbeat:` prefix)
        // and pinned so a regression in phrasing shows up immediately.
        assert!(HEARTBEAT_MESSAGE.starts_with("heartbeat:"), "heartbeat prefix missing: {HEARTBEAT_MESSAGE}");
        assert!(HEARTBEAT_MESSAGE.contains("turn"), "turn noun missing: {HEARTBEAT_MESSAGE}");
    }

    #[test]
    fn heartbeat_watch_disabled_without_interval() {
        // `None` interval (the `run.heartbeatSeconds = 0` path) must disable
        // the watch: no deadline, no beat, even with a turn in flight.
        let base = Instant::now();
        let mut watch = HeartbeatWatch::new(None);
        assert_eq!(watch.next_deadline(), None);
        watch.observe(&turn_started(), base);
        // Started without an interval must NOT arm a deadline.
        assert_eq!(watch.next_deadline(), None, "interval `None` must not arm the watch even after Started");
        assert!(!watch.beat_if_due(base + Duration::from_secs(60)));
        assert_eq!(watch.next_deadline(), None);
    }

    #[test]
    fn heartbeat_watch_does_not_beat_before_interval() {
        let base = Instant::now();
        let interval = Duration::from_millis(50);
        let mut watch = HeartbeatWatch::new(Some(interval));
        watch.observe(&turn_started(), base);
        let deadline = watch.next_deadline().expect("deadline present after Started");
        assert_eq!(deadline, base + interval);
        assert!(watch.is_active());

        // Halfway through the first interval: no beat yet, deadline unchanged.
        let now = base + Duration::from_millis(25);
        assert!(!watch.beat_if_due(now), "before the interval: no beat");
        assert_eq!(watch.next_deadline(), Some(base + interval), "deadline is unchanged before the first interval");
    }

    #[test]
    fn heartbeat_watch_beats_at_each_interval_while_turn_in_flight() {
        // The headline guarantee (TASK-26-111): exactly one beat per
        // interval while a turn is in flight. Drive `beat_if_due` across
        // three full intervals and assert (i) it fires once at each tick,
        // (ii) `next_deadline` advances by `interval` each time, and (iii)
        // `is_active` stays `true` because the turn never ended.
        let base = Instant::now();
        let interval = Duration::from_millis(40);
        let mut watch = HeartbeatWatch::new(Some(interval));
        watch.observe(&turn_started(), base);

        let mut deadlines = Vec::new();
        for tick in 1..=3 {
            let now = base + interval * tick;
            assert!(watch.beat_if_due(now), "tick {tick}: beat should fire");
            let next = watch.next_deadline().expect("deadline present after beat");
            assert_eq!(next, now + interval, "deadline advances by `interval` on every beat");
            deadlines.push(next);
        }

        assert!(watch.is_active(), "the turn is still in flight across the three beats");
        // Three distinct deadlines, each `interval` after the previous one.
        assert_eq!(deadlines, vec![base + interval * 2, base + interval * 3, base + interval * 4]);

        // A duplicate poll at the same instant must NOT fire again: the
        // cadence is "every `interval`", not "every poll that lands on
        // or after the deadline".
        assert!(!watch.beat_if_due(base + interval * 3));
    }

    #[test]
    fn heartbeat_watch_late_poll_anchors_next_beat_to_now() {
        // If the loop is delayed past several intervals, the watch fires
        // once and re-anchors the next deadline to `now`, not to the
        // original `next_beat`. This pins "no silent catch-up" so a
        // regression that fired on every tick between the previous
        // deadline and `now` would surface as a test failure.
        let base = Instant::now();
        let interval = Duration::from_millis(20);
        let mut watch = HeartbeatWatch::new(Some(interval));
        watch.observe(&turn_started(), base);

        let late = base + Duration::from_millis(100); // 5x interval
        assert!(watch.beat_if_due(late));
        assert_eq!(
            watch.next_deadline(),
            Some(late + interval),
            "after a late poll, next deadline is anchored to `now`, not the previous deadline",
        );
    }

    #[test]
    fn heartbeat_watch_does_not_beat_before_turn_starts() {
        // No `TurnEvent::Started`: no deadline, no beat, even when the
        // interval has "passed" many times over.
        let base = Instant::now();
        let mut watch = HeartbeatWatch::new(Some(Duration::from_millis(20)));
        assert_eq!(watch.next_deadline(), None);
        assert!(!watch.beat_if_due(base + Duration::from_secs(60)));
    }

    #[test]
    fn heartbeat_watch_stops_on_turn_ended() {
        let base = Instant::now();
        let interval = Duration::from_millis(30);
        let mut watch = HeartbeatWatch::new(Some(interval));
        watch.observe(&turn_started(), base);
        assert!(watch.next_deadline().is_some());

        // End the turn while the deadline is still in the future: the
        // watch disarms immediately and a subsequent poll at any instant
        // does not fire.
        let ended_at = base + Duration::from_millis(10);
        watch.observe(&turn_ended(), ended_at);
        assert_eq!(watch.next_deadline(), None, "Ended disarms the watch");
        assert!(!watch.beat_if_due(base + Duration::from_secs(1)), "post-Ended poll does not fire");
        assert!(!watch.is_active(), "the watch is inactive after Ended");
    }

    #[test]
    fn heartbeat_watch_rearms_on_next_turn_started() {
        // Re-arm on the next Started event after an Ended so a multi-turn
        // run continues to log heartbeats. The deadline re-anchors to the
        // new Started instant rather than the previous one, so a fresh
        // turn gets a fresh first beat.
        let base = Instant::now();
        let interval = Duration::from_millis(20);
        let mut watch = HeartbeatWatch::new(Some(interval));

        // First turn.
        watch.observe(&turn_started(), base);
        assert_eq!(watch.next_deadline(), Some(base + interval));
        let first_deadline = watch.next_deadline().expect("first deadline present");

        // End it after the deadline would have fired, then start a new one.
        watch.observe(&turn_ended(), base + Duration::from_millis(40));
        assert_eq!(watch.next_deadline(), None);

        let second_started = base + Duration::from_millis(50);
        watch.observe(&turn_started(), second_started);
        assert_eq!(
            watch.next_deadline(),
            Some(second_started + interval),
            "new turn re-arms the watch with a fresh deadline",
        );
        assert_ne!(
            watch.next_deadline(),
            Some(first_deadline),
            "the second turn's deadline is anchored to its own Started, not the first turn's",
        );

        // The new turn's first beat fires at its own deadline.
        assert!(watch.beat_if_due(second_started + interval));
        assert!(!watch.beat_if_due(second_started + interval), "no duplicate beat at the same instant");
    }

    #[test]
    fn heartbeat_watch_ignores_unrelated_events() {
        let base = Instant::now();
        let interval = Duration::from_millis(50);
        let mut watch = HeartbeatWatch::new(Some(interval));

        // LLM-call and tool events must not advance the watch — the
        // heartbeat is a turn-level metric, like the per-turn timings.
        watch.observe(
            &AgentEvent::Turn(TurnEvent::LlmCallStarted {
                purpose: llm::LlmCallPurpose::Chat,
                model: llm::ModelIdentity::default(),
                display_name: "primary".to_string(),
                attempt: 0,
                max_attempts: 3,
            }),
            base,
        );
        assert_eq!(watch.next_deadline(), None, "LlmCallStarted must not arm the watch");

        watch.observe(&turn_started(), base + Duration::from_millis(10));
        let deadline = watch.next_deadline().expect("deadline after Started");

        watch.observe(
            &AgentEvent::Tool(aether_core::events::ToolEvent::Call {
                request: llm::ToolCallRequest {
                    id: "tc1".to_string(),
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                },
            }),
            base + Duration::from_millis(20),
        );
        assert_eq!(watch.next_deadline(), Some(deadline), "unrelated events must not advance the deadline");
        assert!(!watch.is_active() || watch.next_deadline().is_some());
    }
}
