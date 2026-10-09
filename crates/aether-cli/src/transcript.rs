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
//!
//! ## Rotation
//!
//! When the writer is opened with [`JsonlTranscript::create_with_max_bytes`]
//! and `max_bytes` is `Some(N)` with `N > 0`, the transcript is rotated
//! between whole lines once it reaches the configured size: the live file is
//! renamed to `<stem>.1` (overwriting any previous rotated sibling) and a
//! fresh transcript is started. Rotation is triggered *after* the write that
//! crosses the threshold so a line is never split across files. The `turn`
//! counter is preserved across rotation so a rotated run still counts as one
//! run.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

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
/// Holds a `BufWriter<File>`, the current 1-based turn number, the bytes
/// written since the last rotation, the configured rotation threshold (if
/// any), and the file path needed to rename it on rotation. Turn numbering
/// increments on every `TurnEvent::Started`; events seen before the first
/// `Started` are written with `turn: 0`.
pub struct JsonlTranscript {
    writer: BufWriter<File>,
    turn: u32,
    path: PathBuf,
    max_bytes: Option<u64>,
    bytes_written: u64,
}

impl JsonlTranscript {
    /// Open `path` for writing, truncating any existing file. The wrapped
    /// `BufWriter` is flushed on `Drop`, but call [`Self::flush`] explicitly
    /// before relying on the bytes on disk — `Drop` is best-effort.
    ///
    /// Equivalent to `create_with_max_bytes(path, None)`: the transcript
    /// grows without bound.
    pub fn create(path: &Path) -> io::Result<Self> {
        Self::create_with_max_bytes(path, None)
    }

    /// Open `path` for writing, truncating any existing file, with an
    /// optional rotation threshold.
    ///
    /// When `max_bytes` is `Some(N)` and `N > 0`, the writer renames the
    /// current transcript to `<stem>.1` and starts a fresh empty transcript
    /// as soon as a complete line would push the file at or past `N` bytes.
    /// The previous rotated sibling is overwritten on each rotation so
    /// exactly one previous file is kept. `None` and `Some(0)` both disable
    /// rotation (matching the `max_bytes = 0` convention used elsewhere).
    pub fn create_with_max_bytes(path: &Path, max_bytes: Option<u64>) -> io::Result<Self> {
        let writer = BufWriter::new(File::create(path)?);
        Ok(Self { writer, turn: 0, path: path.to_path_buf(), max_bytes, bytes_written: 0 })
    }

    /// Write one transcript line for `event`. Events without a CLI event
    /// kind (partial stream fragments and `ToolEvent::CallUpdate`) are
    /// skipped. A bad path or full disk is reported via the returned error;
    /// a write failure mid-run is surfaced to the caller, who decides
    /// whether the run should fail.
    ///
    /// When a rotation threshold is configured and the just-written line
    /// crosses it, the live file is renamed to `<stem>.1` and a fresh
    /// transcript is opened before this method returns.
    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {
        let Some(event_type) = event_kind(event) else {
            return Ok(());
        };
        if matches!(event, AgentEvent::Turn(TurnEvent::Started { .. })) {
            self.turn = self.turn.saturating_add(1);
        }
        let record = TranscriptRecord { turn: self.turn, event_type, event };
        let mut bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes)?;
        self.bytes_written = self.bytes_written.saturating_add(bytes.len() as u64);
        if let Some(max) = self.max_bytes
            && max > 0
            && self.bytes_written >= max
        {
            self.rotate()?;
        }
        Ok(())
    }

    /// Flush the underlying buffer. Called by the CLI before both the normal
    /// end-of-run return and the early `ExitCode::FAILURE` return so the file
    /// always contains every event that was acknowledged.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    /// Path the current transcript is renamed to on rotation. The extension
    /// is dropped so `transcript.jsonl` rotates to `transcript.1`, matching
    /// the literal name on the work card.
    fn rotated_path(&self) -> PathBuf {
        self.path.with_extension("1")
    }

    /// Rotate the live transcript: flush, rename it to `<stem>.1`, and open
    /// a fresh empty file at the original path. The previous rotated sibling
    /// is deleted first so the rename never collides (Windows-friendly).
    /// The byte counter and turn counter are reset only for the byte count;
    /// turns persist across rotation.
    fn rotate(&mut self) -> io::Result<()> {
        self.writer.flush()?;
        let rotated = self.rotated_path();
        if rotated.exists() {
            std::fs::remove_file(&rotated)?;
        }
        std::fs::rename(&self.path, &rotated)?;
        self.writer = BufWriter::new(File::create(&self.path)?);
        self.bytes_written = 0;
        Ok(())
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

    #[test]
    fn rotate_renames_previous_file_and_starts_a_fresh_transcript_when_threshold_reached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        let rotated = path.with_extension("1");

        // `Some(1)` ensures every record crosses the threshold, so the writer
        // rotates after each record. After every rotation the live file is
        // freshly truncated (zero bytes on disk), so reading it without an
        // intervening `flush()` shows an empty file. The acceptance criterion
        // is: "the previous file is kept and the new one starts empty".
        let mut transcript = JsonlTranscript::create_with_max_bytes(&path, Some(1)).expect("create writer");
        let first = AgentEvent::Turn(TurnEvent::Started { content: vec![] });
        let second = AgentEvent::turn_ended(TurnOutcome::Completed);

        transcript.record(&first).expect("first record");
        // The first write crossed the threshold, so rotation has already
        // happened: the rotated sibling holds the first record and the live
        // file is empty.
        assert!(rotated.exists(), "rotated file should exist after first record");
        let live_after_first = std::fs::read_to_string(&path).expect("read live");
        assert!(live_after_first.is_empty(), "live file must be empty between rotations; got {live_after_first:?}");
        let first_rotated = std::fs::read_to_string(&rotated).expect("read rotated");
        let first_line = first_rotated.lines().next().expect("rotated has one line");
        let first_value: serde_json::Value =
            serde_json::from_str(first_line).unwrap_or_else(|error| panic!("rotated line is not JSON: {error}"));
        assert_eq!(first_value["turn"], 1);
        assert_eq!(first_value["type"], "turn_started");

        transcript.record(&second).expect("second record");
        transcript.flush().expect("flush");

        // After the second record, the rotated sibling is overwritten with
        // the second record; the live file remains empty (every record
        // triggered rotation, and rotation truncates the live file).
        let live = std::fs::read_to_string(&path).expect("read live");
        assert!(
            live.is_empty(),
            "live file must stay empty when every record crosses the threshold (rotation truncates it); got {live:?}"
        );
        let rotated_now = std::fs::read_to_string(&rotated).expect("read rotated");
        let rotated_value: serde_json::Value =
            serde_json::from_str(rotated_now.lines().next().expect("rotated has one line"))
                .unwrap_or_else(|error| panic!("rotated line is not JSON: {error}"));
        assert_eq!(rotated_value["type"], "turn_ended", "rotated sibling holds the most recent prior record");
    }

    #[test]
    fn rotate_accumulates_until_threshold_then_renames_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        let rotated = path.with_extension("1");

        // First record two kinded events with no rotation so we can read one
        // line's exact byte length off the live file.
        let mut probe = JsonlTranscript::create(&path).expect("create writer");
        probe.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("probe record");
        probe.flush().expect("probe flush");
        let one_record_len = std::fs::metadata(&path).expect("metadata").len();
        drop(probe);

        // Open with a threshold that lets *two* records fit but not three:
        // we expect to see two records accumulate, then a rotation triggered
        // by the third record, then another two records accumulate, then a
        // second rotation triggered by the fifth record. The final state is
        // therefore: the most recent rotation cycle is in the rotated sibling
        // and the live file is freshly empty (rotation truncates it).
        let threshold = one_record_len * 2;
        let mut transcript = JsonlTranscript::create_with_max_bytes(&path, Some(threshold)).expect("create writer");
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
        ];
        for event in &events {
            transcript.record(event).expect("record writes");
        }
        transcript.flush().expect("flush");

        // The fifth record did *not* trigger rotation (the previous rotation
        // reset the byte counter, so the fifth only brought us back to
        // `one_record_len`). It sits in the BufWriter until `flush()`
        // persists it to the live file. So the live file holds exactly the
        // fifth record and nothing else.
        let live_size = std::fs::metadata(&path).expect("live metadata").len();
        assert_eq!(
            live_size, one_record_len,
            "live file must hold exactly the post-rotation record; got {live_size}, expected {one_record_len}"
        );

        let live_contents = std::fs::read_to_string(&path).expect("read live");
        let live_value: serde_json::Value = serde_json::from_str(live_contents.lines().next().expect("one line"))
            .unwrap_or_else(|error| panic!("live line is not JSON: {error}"));
        assert_eq!(live_value["turn"], 3, "live file is the third turn (after two rotations)");

        // The rotated sibling holds the two records that triggered rotation.
        let rotated_contents = std::fs::read_to_string(&rotated).expect("read rotated");
        let rotated_lines: Vec<&str> = rotated_contents.lines().collect();
        assert_eq!(
            rotated_lines.len(),
            2,
            "rotated sibling holds the two records that triggered the most recent rotation; got {rotated_lines:?}"
        );
        for (index, line) in rotated_lines.iter().enumerate() {
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("rotated line {index} is not JSON: {error}; line={line:?}"));
            assert!(value["type"].is_string(), "rotated line {index} has a `type`: {value:?}");
        }
        // Specifically, the rotated sibling holds records 3 and 4 (the
        // fourth `turn_ended` triggered the most recent rotation):
        let last_rotated: serde_json::Value = serde_json::from_str(rotated_lines[1])
            .unwrap_or_else(|error| panic!("rotated last line is not JSON: {error}"));
        assert_eq!(
            last_rotated["type"], "turn_ended",
            "rotated sibling must end with the record immediately before rotation; got {last_rotated:?}"
        );
    }

    #[test]
    fn does_not_rotate_without_max_bytes_threshold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        let rotated = path.with_extension("1");

        let mut transcript = JsonlTranscript::create_with_max_bytes(&path, None).expect("create writer");
        for _ in 0..3 {
            transcript.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("record writes");
        }
        transcript.flush().expect("flush");

        assert!(path.exists(), "live transcript should exist");
        assert!(!rotated.exists(), "no rotated sibling should exist when threshold is None");

        let live = std::fs::read_to_string(&path).expect("read live");
        assert_eq!(live.lines().count(), 3, "all three records live in the unrotated transcript");
    }

    #[test]
    fn max_bytes_zero_disables_rotation() {
        // `Some(0)` follows the same convention used by tool-output capping:
        // zero means "no cap". The transcript must not rotate.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        let rotated = path.with_extension("1");

        let mut transcript = JsonlTranscript::create_with_max_bytes(&path, Some(0)).expect("create writer");
        transcript.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("record");
        transcript.flush().expect("flush");

        assert!(!rotated.exists(), "Some(0) must not rotate; got unexpected rotated sibling");
        let live = std::fs::read_to_string(&path).expect("read live");
        assert_eq!(live.lines().count(), 1);
    }

    #[test]
    fn rotation_replaces_previous_sibling_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        let rotated = path.with_extension("1");

        // Seed the rotated sibling so the second rotation must overwrite it.
        std::fs::write(&rotated, "stale rotated contents\n").expect("seed rotated");

        let mut transcript = JsonlTranscript::create_with_max_bytes(&path, Some(1)).expect("create writer");
        transcript.record(&AgentEvent::Turn(TurnEvent::Started { content: vec![] })).expect("record");
        transcript.flush().expect("flush");

        let rotated_contents = std::fs::read_to_string(&rotated).expect("read rotated");
        assert!(!rotated_contents.contains("stale"), "stale contents must be replaced; got {rotated_contents:?}");
        let rotated_value: serde_json::Value =
            serde_json::from_str(rotated_contents.lines().next().expect("rotated has one line"))
                .unwrap_or_else(|error| panic!("rotated line is not JSON: {error}"));
        assert_eq!(rotated_value["type"], "turn_started");
    }
}
