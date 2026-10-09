//! Live progress line that names the tool currently executing.
//!
//! While a long headless run is in progress, the CLI shows a one-line status on
//! stderr naming the tool the agent is currently running. The line is replaced
//! on every new `ToolEvent::ExecutionStarted` and cleared when that tool
//! completes (result, error, refusal, or background-task completion). It is
//! gated on [`OutputFormat::Text`] so machine-readable formats (JSON/Pretty)
//! stay clean.
//!
//! The mapping from [`AgentEvent`] to a [`ToolProgressUpdate`] is a pure
//! function, decoupled from the I/O sink so it can be exercised in unit tests
//! without spawning the headless event loop. The writer
//! [`ToolProgressReporter`] is generic over `std::io::Write` so tests assert
//! byte-for-byte output against a `Vec<u8>`.

use std::io::{self, Write};

use aether_core::events::{AgentEvent, ToolEvent};

/// Single line naming the tool that is currently executing. Used by
/// [`ToolProgressReporter`] to draw the live stderr status; the test
/// `tool_progress_line_names_the_tool` pins the exact format.
pub(crate) fn tool_progress_line(tool_name: &str) -> String {
    format!("⏺ {tool_name}")
}

/// Pure description of what the live progress line should do next, derived
/// from the agent event stream by [`tool_progress_update`]. Decoupled from
/// I/O so the mapping can be exercised without a writer.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(crate) enum ToolProgressUpdate {
    Show(String),
    Clear,
}

/// Map a stream event to a progress update. `ExecutionStarted` flips the
/// status line to the new tool name; every completion variant clears it so a
/// later `Show` writes a fresh line rather than appending. All other events
/// leave the current state alone.
pub(crate) fn tool_progress_update(event: &AgentEvent) -> Option<ToolProgressUpdate> {
    match event {
        AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_name, .. }) => {
            Some(ToolProgressUpdate::Show(tool_name.clone()))
        }
        AgentEvent::Tool(
            ToolEvent::Result { .. }
            | ToolEvent::Error { .. }
            | ToolEvent::Refused { .. }
            | ToolEvent::TaskCreated { .. }
            | ToolEvent::TaskCompleted { .. }
            | ToolEvent::TaskFailed { .. }
            | ToolEvent::TaskCancelled { .. },
        ) => Some(ToolProgressUpdate::Clear),
        _ => None,
    }
}

/// Writes the live tool-progress line to a sink, replacing it on every new
/// execution and clearing it when the current tool completes. The active flag
/// tracks whether a line is currently visible so clearing an idle state
/// writes nothing to the sink. Generic over `Write` so tests can use a
/// `Vec<u8>` to assert byte-for-byte output.
pub(crate) struct ToolProgressReporter<W: Write> {
    sink: W,
    active: bool,
}

impl<W: Write> ToolProgressReporter<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self { sink, active: false }
    }

    /// Apply a derived update: show the named tool or clear the line. Errors
    /// are bubbled up so the caller can log them on stderr without breaking
    /// the loop.
    pub(crate) fn apply(&mut self, update: ToolProgressUpdate) -> io::Result<()> {
        match update {
            ToolProgressUpdate::Show(name) => {
                // `\r` returns to column 0 so the new name overwrites the
                // previous one in place; `\x1b[K` erases to end of line so a
                // shorter name cannot leave a tail of the previous text.
                write!(self.sink, "\r{}\x1b[K", tool_progress_line(&name))?;
                self.sink.flush()?;
                self.active = true;
                Ok(())
            }
            ToolProgressUpdate::Clear => self.clear(),
        }
    }

    /// Clear the live progress line, writing nothing when no line is active.
    /// A redundant clear at run end leaves no stray bytes on stderr.
    pub(crate) fn clear(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        // `\r` returns to column 0 and `\x1b[2K` erases the whole line.
        write!(self.sink, "\r\x1b[2K")?;
        self.sink.flush()?;
        self.active = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aether_core::events::{ContextEvent, StreamState};

    use super::*;

    #[test]
    fn tool_progress_line_names_the_tool() {
        assert_eq!(tool_progress_line("bash"), "⏺ bash");
        assert_eq!(tool_progress_line("edit_file"), "⏺ edit_file");
        assert_eq!(tool_progress_line("a"), "⏺ a");
    }

    #[test]
    fn tool_progress_update_shows_on_execution_started_and_clears_on_completion() {
        let started =
            AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id: "tc1".to_string(), tool_name: "bash".to_string() });
        assert_eq!(tool_progress_update(&started), Some(ToolProgressUpdate::Show("bash".to_string())));

        let result = AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        });
        assert_eq!(tool_progress_update(&result), Some(ToolProgressUpdate::Clear));

        let error = AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: None,
                error: "boom".to_string(),
            },
        });
        assert_eq!(tool_progress_update(&error), Some(ToolProgressUpdate::Clear));

        let refused = AgentEvent::Tool(ToolEvent::Refused {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
            reason: "nope".to_string(),
        });
        assert_eq!(tool_progress_update(&refused), Some(ToolProgressUpdate::Clear));

        let unrelated = AgentEvent::Context(ContextEvent::Cleared);
        assert_eq!(tool_progress_update(&unrelated), None);

        let partial_text = AgentEvent::text("id", "x", StreamState::Partial);
        assert_eq!(tool_progress_update(&partial_text), None);
    }

    #[test]
    fn tool_progress_reporter_replaces_and_clears() {
        let mut reporter = ToolProgressReporter::new(Vec::<u8>::new());
        reporter.apply(ToolProgressUpdate::Show("bash".to_string())).unwrap();
        reporter.apply(ToolProgressUpdate::Show("edit_file".to_string())).unwrap();
        reporter.apply(ToolProgressUpdate::Clear).unwrap();

        let bytes = reporter.sink.clone();
        assert!(bytes.starts_with(b"\r"), "progress line should start with carriage return: {bytes:?}");
        let first_name = "⏺ bash".to_string();
        let second_name = "⏺ edit_file".to_string();
        assert!(
            bytes.windows(first_name.len()).any(|window| window == first_name.as_bytes()),
            "first tool name missing from output: {bytes:?}"
        );
        assert!(
            bytes.windows(second_name.len()).any(|window| window == second_name.as_bytes()),
            "second tool name missing from output: {bytes:?}"
        );
        assert!(
            bytes.windows(3).any(|window| window == b"\xe2\x8f\xba"),
            "should contain the progress glyph: {bytes:?}"
        );
        let expected_tail: &[u8] = b"\r\x1b[2K";
        assert!(bytes.ends_with(expected_tail), "should end with clear escape sequence: {bytes:?}");
        // A second clear after the line is gone is a no-op; nothing more is written.
        let before = bytes.clone();
        reporter.clear().unwrap();
        assert_eq!(reporter.sink, before, "second clear must not write extra bytes");
    }

    #[test]
    fn tool_progress_reporter_clear_is_noop_when_idle() {
        let mut reporter = ToolProgressReporter::new(Vec::<u8>::new());
        // Never called `apply`, so nothing is active; clear must not write anything.
        reporter.clear().unwrap();
        assert!(reporter.sink.is_empty(), "idle clear should not emit bytes: {:?}", reporter.sink);
    }
}
