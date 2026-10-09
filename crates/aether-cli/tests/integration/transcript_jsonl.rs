use aether_cli::transcript::{JsonlTranscript, has_cli_event_kind};
use aether_core::core::agent;
use aether_core::events::{AgentEvent, Command, TurnEvent, TurnOutcome};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use tokio::sync::mpsc;

async fn drive_to_completion(
    mut rx: mpsc::Receiver<AgentEvent>,
    command_tx: mpsc::Sender<Command>,
    prompt: &str,
) -> Vec<AgentEvent> {
    command_tx.send(Command::text(prompt)).await.expect("send prompt");
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let terminal = matches!(
            event,
            AgentEvent::Turn(TurnEvent::Ended {
                outcome: TurnOutcome::Completed | TurnOutcome::Cancelled | TurnOutcome::Failed { .. }
            })
        );
        events.push(event);
        if terminal {
            break;
        }
    }
    events
}

fn parse_transcript(path: &std::path::Path) -> Vec<(u64, String, serde_json::Value)> {
    let raw = std::fs::read_to_string(path).expect("read transcript");
    raw.lines()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).expect("line parses as JSON object");
            assert!(value.is_object(), "transcript line must be a JSON object: {value:?}");
            let turn =
                value["turn"].as_u64().unwrap_or_else(|| panic!("transcript line missing numeric `turn`: {value:?}"));
            let event_type = value["type"]
                .as_str()
                .unwrap_or_else(|| panic!("transcript line missing string `type`: {value:?}"))
                .to_string();
            let event = value["event"]
                .as_object()
                .unwrap_or_else(|| panic!("transcript `event` must be a JSON object: {value:?}"));
            (turn, event_type, serde_json::Value::Object(event.clone()))
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_jsonl_records_one_object_per_kinded_event() {
    // One successful turn: the transcript must contain a `turn_started`,
    // one `text`, and a `turn_ended`, each carrying the right `turn` and a
    // nested event payload.
    let provider =
        FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("hello world"), LlmResponse::done()]])
            .with_display_name("fake:transcript-ok");

    let (tx, rx, _handle) = agent(provider).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "hi").await;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("transcript.jsonl");

    let mut transcript = JsonlTranscript::create(&path).expect("create writer");
    for event in &events {
        transcript.record(event).expect("record writes");
    }
    transcript.flush().expect("flush");

    let parsed = parse_transcript(&path);

    // The fake provider only emits `text` and the `done`; the agent wraps
    // that with `turn_started` and `turn_ended` (plus an internal
    // `llm_call_started`/`llm_call_ended` and a `tool_definitions_updated`).
    // Asserting on kinded events from `events` keeps the count honest and
    // matches what `--output text` would have printed.
    let kinded_in_order: Vec<&AgentEvent> = events.iter().filter(|event| has_cli_event_kind(event)).collect();
    assert_eq!(parsed.len(), kinded_in_order.len(), "one transcript line per kinded event");

    // The run completed in a single turn, so every line must carry `turn: 1`.
    for (index, (turn, _, _)) in parsed.iter().enumerate() {
        assert_eq!(*turn, 1, "single-turn run must report turn 1 on every line; line {index} has turn {turn}");
    }

    // The first kinded event is `turn_started` and the last is `turn_ended`,
    // and at least one `text` event sits between them.
    assert_eq!(parsed.first().expect("at least one line").1, "turn_started");
    assert_eq!(parsed.last().expect("at least one line").1, "turn_ended");
    assert!(
        parsed.iter().any(|(_, kind, _)| kind == "text"),
        "transcript should contain a `text` event for the assistant reply; kinds: {:?}",
        parsed.iter().map(|(_, k, _)| k).collect::<Vec<_>>()
    );

    // Every nested `event` is the full serialized AgentEvent and contains
    // the expected discriminator tags, proving we kept the existing schema
    // instead of inventing a new one.
    for (index, (_, _, event)) in parsed.iter().enumerate() {
        assert!(
            event.get("category").is_some(),
            "nested event must keep the AgentEvent `category` tag; line {index} value: {event:?}"
        );
        assert!(
            event.get("event").is_some(),
            "nested event must keep the AgentEvent `event` object; line {index} value: {event:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_jsonl_rotates_when_max_bytes_is_reached() {
    // Drives the same single-turn fake-provider run as the tests above, but
    // opens the writer with `create_with_max_bytes(..., Some(1))` so every
    // record crosses the rotation threshold. The acceptance criterion from
    // TASK-24-279 is: "a run produces transcript.1 and then a fresh
    // transcript" — we assert both halves:
    //   1. the rotated sibling exists and holds the *previous* transcript,
    //   2. the live transcript is freshly empty (every rotation truncates
    //      the live file), so it starts empty between writes.
    let provider = FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("hi"), LlmResponse::done()]])
        .with_display_name("fake:transcript-rotate");

    let (tx, rx, _handle) = agent(provider).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "hi").await;
    let kinded_count = events.iter().filter(|event| has_cli_event_kind(event)).count();
    assert!(kinded_count >= 2, "a single-turn run must yield at least two kinded events; got {kinded_count}");

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("transcript.jsonl");
    let rotated = path.with_extension("1");

    // `Some(1)` ensures every record crosses the threshold, so the rotated
    // sibling holds the most-recent single-record snapshot and the live file
    // is empty (it just got truncated by the most recent rotation).
    let mut transcript = JsonlTranscript::create_with_max_bytes(&path, Some(1)).expect("create writer");
    for event in &events {
        transcript.record(event).expect("record writes");
    }
    transcript.flush().expect("flush");

    assert!(rotated.exists(), "transcript.1 sibling must exist after rotation; path={rotated:?}");
    let rotated_lines = parse_transcript(&rotated);
    assert_eq!(
        rotated_lines.len(),
        1,
        "rotated sibling holds exactly one record (the most recent before rotation); got {} records: {:?}",
        rotated_lines.len(),
        rotated_lines.iter().map(|(_, kind, _)| kind.clone()).collect::<Vec<_>>()
    );
    assert_eq!(rotated_lines[0].1, "turn_ended", "the rotated sibling holds the last record of the run");

    // The live file is freshly empty: every record triggered rotation, and
    // rotation truncates the live file. `flush()` after the last record
    // only flushes the now-empty BufWriter of the freshly truncated live
    // file.
    let live_contents = std::fs::read_to_string(&path).expect("read live");
    assert!(
        live_contents.is_empty(),
        "live transcript must be empty after every record triggers rotation; got {live_contents:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_jsonl_does_not_rotate_without_max_bytes() {
    // Regression guard: when no rotation threshold is configured the
    // existing semantics (one growing transcript, no sibling) hold. This
    // runs the same fixture as the rotation test above but with `None` so
    // the sibling file must not appear.
    let provider = FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("hi"), LlmResponse::done()]])
        .with_display_name("fake:transcript-no-rotate");

    let (tx, rx, _handle) = agent(provider).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "hi").await;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("transcript.jsonl");
    let rotated = path.with_extension("1");

    let mut transcript = JsonlTranscript::create_with_max_bytes(&path, None).expect("create writer");
    for event in &events {
        transcript.record(event).expect("record writes");
    }
    transcript.flush().expect("flush");

    assert!(!rotated.exists(), "no rotated sibling must exist when threshold is None");
    let parsed = parse_transcript(&path);
    let expected = events.iter().filter(|event| has_cli_event_kind(event)).count();
    assert_eq!(parsed.len(), expected, "all kinded events live in the unrotated transcript");
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_jsonl_lines_round_trip_through_serde_json() {
    // Each line must be exactly one JSON object: parse with the public
    // AgentEvent schema to catch any tag that drifts from the wire format.
    let provider = FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("ping"), LlmResponse::done()]])
        .with_display_name("fake:transcript-roundtrip");

    let (tx, rx, _handle) = agent(provider).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "ping").await;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("transcript.jsonl");

    let mut transcript = JsonlTranscript::create(&path).expect("create writer");
    for event in &events {
        transcript.record(event).expect("record writes");
    }
    transcript.flush().expect("flush");

    let raw = std::fs::read_to_string(&path).expect("read transcript");
    let lines: Vec<&str> = raw.lines().collect();
    assert!(!lines.is_empty(), "transcript must have at least one line");

    for (index, line) in lines.iter().enumerate() {
        let outer: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("line {index} is not valid JSON: {error}; line={line:?}"));
        let inner = &outer["event"];
        // The nested event must round-trip through the AgentEvent schema;
        // this catches accidental renames of the on-the-wire payload.
        let agent_event: AgentEvent = serde_json::from_value(inner.clone()).unwrap_or_else(|error| {
            panic!("nested event on line {index} does not round-trip: {error}; value={inner:?}")
        });
        // And re-serialising it must yield the same value: a structural
        // equality check rather than string equality so the test does not
        // fail on JSON formatting noise.
        let again = serde_json::to_value(&agent_event).expect("re-serialize");
        assert_eq!(again, *inner, "round-trip changed the event payload on line {index}");
    }
}
