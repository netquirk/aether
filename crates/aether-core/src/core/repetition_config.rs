//! Configuration for the repetition detector.
//!
//! The detector observes every completed iteration inside a turn: it builds a
//! signature from the assistant message content plus any tool-call
//! `(name, arguments)` pairs, and ends the turn as a `TurnOutcome::Failed`
//! when the same signature repeats `max_repeats` times in a row.
//!
//! The detector is independent of [`crate::core::RetryConfig`], which only
//! governs transient LLM provider failures. Setting `max_repeats` to `0`
//! disables the detector entirely (the default is `3`).

/// Repetition policy for a turn.
///
/// When the same assistant signature (content + tool calls) is observed
/// `max_repeats` times in a row within one turn, the turn is ended with
/// [`TurnOutcome::Failed`](crate::events::TurnOutcome::Failed) and the
/// repetition is named in the error message. A new user/queued input or any
/// differing signature resets the counter, so legitimate repeated actions are
/// not interrupted.
///
/// This is independent of [`crate::core::RetryConfig`], which only governs
/// transient LLM provider failures.
///
/// `0` disables detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepetitionConfig {
    /// Maximum consecutive identical signatures before the turn is ended.
    /// `0` disables the detector.
    pub max_repeats: u32,
}

impl Default for RepetitionConfig {
    fn default() -> Self {
        // Conservative threshold: a repeating model usually loops within a
        // couple of attempts, but legitimate retries (e.g. polling tools,
        // requesting the same read-only data) can produce two identical
        // outputs in a row, so we stop at three.
        Self { max_repeats: 3 }
    }
}

impl RepetitionConfig {
    /// Disable repetition detection.
    pub const fn disabled() -> Self {
        Self { max_repeats: 0 }
    }

    /// Returns `true` when detection is enabled (`max_repeats > 0`).
    pub const fn is_enabled(self) -> bool {
        self.max_repeats > 0
    }
}

/// Tracks consecutive identical iteration signatures within one turn.
///
/// `RepetitionTracker` is reset at the start of a turn (and after any user or
/// queued input is committed). Each call to [`observe`](Self::observe) returns
/// `true` once the new signature has matched the previous one at least
/// `max_repeats - 1` times — i.e. when this is the `max_repeats`-th
/// identical observation in a row.
#[derive(Debug, Clone)]
pub(crate) struct RepetitionTracker {
    config: RepetitionConfig,
    last_signature: Option<String>,
    count: u32,
}

impl RepetitionTracker {
    pub(crate) fn new(config: RepetitionConfig) -> Self {
        Self { config, last_signature: None, count: 0 }
    }

    /// Forget any previously observed signature.
    pub(crate) fn reset(&mut self) {
        self.last_signature = None;
        self.count = 0;
    }

    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    /// Record an iteration signature and return whether the repetition
    /// threshold (`max_repeats`) has been reached.
    ///
    /// A signature of `None` (or detection disabled via `max_repeats == 0`) is
    /// recorded as "no repetition" — the next `Some(...)` starts a fresh
    /// streak rather than inheriting the previous one.
    pub(crate) fn observe(&mut self, signature: Option<&str>) -> bool {
        if !self.config.is_enabled() {
            return false;
        }
        let Some(signature) = signature else {
            // No content to fingerprint — nothing to detect; keep the
            // counter primed for the next real signature.
            self.last_signature = None;
            self.count = 0;
            return false;
        };
        if self.last_signature.as_deref() == Some(signature) {
            self.count = self.count.saturating_add(1);
        } else {
            self.last_signature = Some(signature.to_string());
            self.count = 1;
        }
        self.count >= self.config.max_repeats
    }
}

/// Build a fingerprint for one completed iteration.
///
/// The tool-call `id` is intentionally excluded — providers assign fresh IDs
/// per LLM call, so including it would defeat the detector.
pub(crate) fn iteration_signature(
    message_content: &str,
    reasoning_summary_text: &str,
    tool_calls: &[llm::ToolCallRequest],
) -> Option<String> {
    let has_text = !message_content.is_empty() || !reasoning_summary_text.is_empty();
    let has_tools = !tool_calls.is_empty();
    if !has_text && !has_tools {
        return None;
    }
    let mut out = String::with_capacity(message_content.len() + reasoning_summary_text.len() + tool_calls.len() * 32);
    out.push_str("text:");
    out.push_str(message_content);
    out.push('\n');
    out.push_str("reason:");
    out.push_str(reasoning_summary_text);
    out.push('\n');
    out.push_str("tools:");
    for (idx, call) in tool_calls.iter().enumerate() {
        if idx > 0 {
            out.push('|');
        }
        out.push_str(&call.name);
        out.push('{');
        out.push_str(&call.arguments);
        out.push('}');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ToolCallRequest;

    #[test]
    fn default_is_three() {
        assert_eq!(RepetitionConfig::default().max_repeats, 3);
        assert!(RepetitionConfig::default().is_enabled());
    }

    #[test]
    fn disabled_is_zero_and_off() {
        let config = RepetitionConfig::disabled();
        assert_eq!(config.max_repeats, 0);
        assert!(!config.is_enabled());
    }

    #[test]
    fn tracker_resets_on_new_signature() {
        let mut tracker = RepetitionTracker::new(RepetitionConfig { max_repeats: 2 });
        assert!(!tracker.observe(Some("a")), "first occurrence never triggers");
        assert!(tracker.observe(Some("a")), "second occurrence in a row matches max_repeats=2");
        assert!(!tracker.observe(Some("b")), "differing signature resets the counter to 1");
        assert!(tracker.observe(Some("b")), "second occurrence of b reaches max_repeats again");
        assert!(!tracker.observe(Some("c")), "third distinct signature starts a fresh streak");
    }

    #[test]
    fn tracker_treats_none_as_a_reset() {
        let mut tracker = RepetitionTracker::new(RepetitionConfig { max_repeats: 2 });
        tracker.observe(Some("same"));
        tracker.observe(Some("same"));
        assert!(!tracker.observe(None), "a None fingerprint never reaches the threshold");
        assert!(!tracker.observe(Some("same")));
    }

    #[test]
    fn tracker_is_no_op_when_disabled() {
        let mut tracker = RepetitionTracker::new(RepetitionConfig::disabled());
        for _ in 0..10 {
            assert!(!tracker.observe(Some("anything")));
        }
    }

    #[test]
    fn iteration_signature_ignores_tool_call_id() {
        let first = iteration_signature(
            "hi",
            "",
            &[ToolCallRequest { id: "id-1".into(), name: "tool".into(), arguments: "{\"x\":1}".into() }],
        );
        let second = iteration_signature(
            "hi",
            "",
            &[ToolCallRequest { id: "id-2".into(), name: "tool".into(), arguments: "{\"x\":1}".into() }],
        );
        assert_eq!(first, second, "tool-call ids must not affect the signature");
    }

    #[test]
    fn iteration_signature_distinguishes_tool_arguments() {
        let a = iteration_signature(
            "",
            "",
            &[ToolCallRequest { id: "x".into(), name: "tool".into(), arguments: "{\"x\":1}".into() }],
        );
        let b = iteration_signature(
            "",
            "",
            &[ToolCallRequest { id: "x".into(), name: "tool".into(), arguments: "{\"x\":2}".into() }],
        );
        assert_ne!(a, b);
    }
}
