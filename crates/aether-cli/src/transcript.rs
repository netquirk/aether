//! JSON Lines transcript writer for the `aether headless` event stream.
//!
//! When the `--transcript-jsonl` flag is passed, the CLI writes one JSON object
//! per emitted event to a file. The format is a thin wrapper around
//! [`AgentEvent`]: each line carries the 1-based turn number and the CLI event
//! kind (the same enum that drives `--output`/`--events`), with the full
//! serialized event nested under `"event"`. The nesting keeps the transcript's
//! own `type` field from colliding with the inner `AgentEvent` `type` tag.
//!
//! Events that have no `CliEventKind` (streaming `Partial` text/thought
//! fragments and `ToolEvent::CallUpdate`) are skipped, matching the
//! `--output text` filter. The writer is independent of `--events`, so a
//! filtered run still gets a complete transcript.
//!
//! The file is opened with [`File::create`], which truncates any existing
//! file at the path. The run's stdout is unchanged and stays human-readable
//! by default; this writer is purely additive.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use aether_core::events::{AgentEvent, TurnEvent};
use serde::Serialize;

use crate::headless::CliEventKind;
use crate::headless::run::event_kind;

/// One JSON object per line in the transcript file. The `type` field carries
/// the CLI's event kind (using `snake_case`); the `event` field is the full
/// `AgentEvent` payload.
#[derive(Debug, Serialize)]
struct TranscriptRecord<'a> {
    turn: u32,
    #[serde(rename = "type")]
    event_type: CliEventKind,
    event: &'a AgentEvent,
}

/// Streaming writer for the `--transcript-jsonl` transcript file.
///
/// Holds a `BufWriter<File>` and the current 1-based turn number. Turn
/// numbering increments on every `TurnEvent::Started`; events seen before the
/// first `Started` are written with `turn: 0`.
pub struct JsonlTranscript {
    writer: BufWriter<File>,
    turn: u32,
}

impl JsonlTranscript {
    /// Open `path` for writing, truncating any existing file. The wrapped
    /// `BufWriter` is flushed on `Drop`, but call [`Self::flush`] explicitly
    /// before relying on the bytes on disk — `Drop` is best-effort.
    pub fn create(path: &Path) -> io::Result<Self> {
        let writer = BufWriter::new(File::create(path)?);
        Ok(Self { writer, turn: 0 })
    }

    /// Write one transcript line for `event`. Events without a CLI event
    /// kind (partial stream fragments and `ToolEvent::CallUpdate`) are
    /// skipped. A bad path or full disk is reported via the returned error;
    /// a write failure mid-run is surfaced to the caller, who decides
    /// whether the run should fail.
    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {
        let Some(event_type) = event_kind(event) else {
            return Ok(());
        };
        if matches!(event, AgentEvent::Turn(TurnEvent::Started { .. })) {
            self.turn = self.turn.saturating_add(1);
        }
        let record = TranscriptRecord { turn: self.turn, event_type, event };
        serde_json::to_writer(&mut self.writer, &record).map_err(io::Error::other)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    /// Flush the underlying buffer. Called by the CLI before both the normal
    /// end-of-run return and the early `ExitCode::FAILURE` return so the file
    /// always contains every event that was acknowledged.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Whether `event` would be written to the transcript. Mirrors the predicate
/// `JsonlTranscript::record` uses to skip streaming fragments and tool call
/// updates, so callers (including tests) can count transcript lines without
/// reaching into the private `headless::run::event_kind` API.
pub fn has_cli_event_kind(event: &AgentEvent) -> bool {
    event_kind(event).is_some()
}

#[cfg(test)]
mod tests {
    use aether_core::events::{AgentEvent, StreamState, ToolEvent, TurnEvent, TurnOutcome};

    use super::*;

    fn text(message_id: &str, chunk: &str) -> AgentEvent {
        AgentEvent::text(message_id, chunk, StreamState::Complete)
    }

    fn thought(message_id: &str, chunk: &str) -> AgentEvent {
        AgentEvent::thought(message_id, chunk, StreamState::Complete)
    }

    fn tool_call() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
        })
    }

    fn tool_result() -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        })
    }

    #[test]
    fn record_writes_one_json_object_per_event_with_turn_and_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");

        // Build a deterministic sequence spanning two turns. The expected
        // kinds and turn numbers below mirror this list and are the ground
        // truth for the assertions.
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            text("m1", "hello"),
            tool_call(),
            tool_result(),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            thought("m2", "thinking"),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ];
        let expected_kinds =
            ["turn_started", "text", "tool_call", "tool_result", "turn_ended", "turn_started", "thought", "turn_ended"];
        let expected_turns: [u64; 8] = [1, 1, 1, 1, 1, 2, 2, 2];

        let mut transcript = JsonlTranscript::create(&path).expect("create writer");
        for event in &events {
            transcript.record(event).expect("record writes");
        }
        transcript.flush().expect("flush");

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), events.len(), "one transcript line per event; got {lines:?}");
        // Every line is a JSON object with the contract fields; the
        // `event` field is itself a JSON object.
        for (index, line) in lines.iter().enumerate() {
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("line {index} is not JSON: {error}; line={line:?}"));
            assert!(value.is_object(), "line {index} must be a JSON object: {value:?}");
            let turn = value["turn"].as_u64().unwrap_or_else(|| panic!("turn missing on line {index}: {value:?}"));
            let event_type =
                value["type"].as_str().unwrap_or_else(|| panic!("type missing on line {index}: {value:?}"));
            assert!(value["event"].is_object(), "event must be a JSON object on line {index}: {value:?}");
            assert_eq!(turn, expected_turns[index], "turn on line {index}");
            assert_eq!(event_type, expected_kinds[index], "type on line {index}");
        }
    }

    #[test]
    fn record_skips_partial_streaming_fragments_and_call_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");

        let mut transcript = JsonlTranscript::create(&path).expect("create writer");
        transcript.record(&AgentEvent::text("m1", "partial", StreamState::Partial)).expect("partial text");
        transcript.record(&AgentEvent::thought("m1", "partial", StreamState::Partial)).expect("partial thought");
        transcript
            .record(&AgentEvent::Tool(ToolEvent::CallUpdate {
                tool_call_id: "tc1".to_string(),
                chunk: "x".to_string(),
            }))
            .expect("call update");
        transcript.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("turn started");
        transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn ended");
        transcript.flush().expect("flush");

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "only the two kinded events should be written; got {lines:?}");
        assert_eq!(lines[0], lines[0].trim_end_matches('\n'));
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("first line is JSON");
        assert_eq!(first["type"], "turn_started");
        assert_eq!(first["turn"], 1);
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("second line is JSON");
        assert_eq!(second["type"], "turn_ended");
        assert_eq!(second["turn"], 1);
    }

    #[test]
    fn events_seen_before_first_turn_started_carry_turn_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");

        let mut transcript = JsonlTranscript::create(&path).expect("create writer");
        transcript
            .record(&AgentEvent::Model(aether_core::events::ModelEvent::Switched {
                previous: "a".to_string(),
                new: "b".to_string(),
            }))
            .expect("model switched");
        transcript.flush().expect("flush");

        let contents = std::fs::read_to_string(&path).expect("read file");
        let value: serde_json::Value = serde_json::from_str(contents.lines().next().expect("one line")).expect("JSON");
        assert_eq!(value["turn"], 0);
        assert_eq!(value["type"], "model_switched");
    }

    #[test]
    fn create_truncates_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, "stale contents that should be discarded\n").expect("seed file");

        let mut transcript = JsonlTranscript::create(&path).expect("create writer");
        transcript.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("record writes");
        drop(transcript);

        let contents = std::fs::read_to_string(&path).expect("read file");
        assert_eq!(contents.lines().count(), 1);
        assert!(!contents.contains("stale"));
    }
}
