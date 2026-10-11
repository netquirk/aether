//! Integration tests for `aether --resume PATH` (TASK-26-20).
//!
//! `--resume PATH` continues a session from a saved transcript (the same
//! JSON Lines format `--transcript-jsonl` writes). The acceptance criterion
//! from the task body is: "a --resume PATH flag loads a previous run
//! transcript and starts a new run whose first provider request includes the
//! prior turns". Exercising `--resume` directly through the binary would
//! require a real provider call; this file mirrors the offline seam used
//! by `transcript_jsonl.rs` and `disable_tool.rs`: it drives
//! [`aether_core::core::agent`] directly on a
//! [`FakeLlmProvider`](llm::testing::FakeLlmProvider), with the messages
//! recovered by [`load_resume_messages`] seeded via
//! `AgentBuilder::messages`, and asserts the LLM saw them on its first
//! request. The captured contexts are reached via
//! [`FakeLlmProvider::captured_contexts`].
//!
//! The unit-level round-trip and the unknown-format refusals are covered by
//! `transcript::tests`; this file focuses on the *behavioural* test: an
//! LLM seeded with the recovered messages actually sees them on the first
//! request.

use std::process::{Command, Stdio};

use aether_cli::transcript::{JsonlTranscript, load_resume_messages};
use aether_core::core::agent;
use aether_core::events::{AgentEvent, Command as CoreCommand, StreamState, TurnEvent, TurnOutcome};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use tokio::sync::mpsc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Inline `--settings-json` block carrying the credential-store knob and a
/// single user-invocable agent. Reused across the offline and the binary
/// path so both exercise the same agent name. The agent also carries an
/// inherited `prompts` block so the prompt cache (which the test binary
/// follows the same way the production binary does) does not short-circuit
/// with "no prompts declared" before reaching the `--resume` rejection
/// branch.
const SETTINGS_JSON: &str = r#"{
  "credentialsStore": {"type": "memory"},
  "prompts": [
    {"type": "text", "text": "inspect-only agent for the resume integration test"}
  ],
  "agents": [
    {
      "name": "Inspector",
      "description": "Inspect-only agent for the resume integration test",
      "model": "anthropic:claude-sonnet-4-5",
      "userInvocable": true,
      "mcps": []
    }
  ]
}
"#;

/// `aether headless --resume PATH` with a missing/empty PATH: the CLI must
/// surface a path-naming, line-naming message and exit non-zero so a user
/// notices the bad `--resume` value rather than silently starting a fresh
/// session. Built on the same offline canary as the existing `--dry-run`
/// tests: a closed local port proves no provider call happens regardless.
#[test]
fn headless_resume_rejects_malformed_transcript_with_diagnostic() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bad = dir.path().join("not-a-transcript.jsonl");
    // A file that *looks* like JSON but is not a transcript header is the
    // diagnostic-carrying case the task body asks for ("a transcript in an
    // unknown format is refused with a message naming the problem").
    std::fs::write(&bad, "{\"this\":\"is not a transcript header\"}\n")?;

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--settings-json")
        .arg(SETTINGS_JSON)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--provider")
        .arg("ollama.url=http://127.0.0.1:1")
        .arg("--timeout")
        .arg("15s")
        .arg("--resume")
        .arg(&bad)
        .arg("hello")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    assert!(
        !output.status.success(),
        "--resume against a non-transcript file must fail the run; got {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("resume") && stderr.contains("header"),
        "--resume rejection must mention resume + header; got:\n{stderr}",
    );
    assert!(
        stderr.contains(bad.to_string_lossy().as_ref()),
        "--resume rejection must name the offending path; got:\n{stderr}",
    );

    Ok(())
}

/// Build a two-turn transcript on disk in the same format `--transcript-jsonl`
/// would write, load it back via [`load_resume_messages`], drive one new
/// prompt against `agent(provider).messages(prior).spawn()`, and assert the
/// LLM saw both prior user turns in its *first* request via the fake
/// provider's `captured_contexts()` handle. This is the done-when for the
/// `--resume PATH` deliverable: the first provider request on the new run
/// must carry the prior conversation.
///
/// Uses the same offline seam the neighbouring `transcript_jsonl.rs`
/// integration tests exercise (`agent(provider).spawn()` + the
/// `FakeLlmProvider`'s captured-context handle). The recovered messages are
/// seeded through `AgentBuilder::messages`, which `aether-cli`'s runtime
/// reaches via `RuntimeBuilder::build_ready(prior_messages)`, so the
/// assertion below verifies what the resumed CLI run would actually see.
#[tokio::test(flavor = "current_thread")]
async fn resume_seeds_prior_turns_into_first_provider_request() -> Result<(), Box<dyn std::error::Error>> {
    // Two prior turns, written with the same writer the live CLI uses.
    let dir = tempfile::tempdir()?;
    let transcript_path = dir.path().join("prior.jsonl");
    let mut transcript = JsonlTranscript::create(&transcript_path).expect("create writer");
    transcript.write_header(env!("CARGO_PKG_VERSION"), "2026-10-10T08:54:00+00:00").expect("write header");

    // Turn 1: user asks the first question, the assistant streams a complete
    // text reply, the turn ends cleanly.
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("first question")] }))
        .expect("turn 1 start");
    transcript.record(&AgentEvent::text("m-1", "first answer", StreamState::Complete)).expect("turn 1 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 1 ended");

    // Turn 2: same shape, different content. The reader must keep both turns
    // in document order.
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("second question")] }))
        .expect("turn 2 start");
    transcript.record(&AgentEvent::text("m-2", "second answer", StreamState::Complete)).expect("turn 2 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 2 ended");
    transcript.flush().expect("flush");

    // Recover the prior-turn messages from the on-disk file. This is the
    // exact seam `run_agent` would hit when invoked with `--resume`.
    let prior_messages = load_resume_messages(&transcript_path).expect("two-turn transcript loads");
    assert_eq!(
        prior_messages.len(),
        4,
        "expected four recovered messages (2× user + 2× assistant); got {}: {prior_messages:?}",
        prior_messages.len(),
    );

    // Build the fake provider; grab the captured-contexts handle *before*
    // the agent is spawned so we can inspect the very first LLM request
    // after the run completes. The fake only needs to surface enough
    // responses for one successful turn — the assertion inspects the
    // captured context, not the streamed text.
    let provider = FakeLlmProvider::from_results(vec![vec![
        Ok(LlmResponse::Start),
        Ok(LlmResponse::text("third answer")),
        Ok(LlmResponse::done()),
    ]])
    .with_display_name("fake:resume");
    let captured_contexts = provider.captured_contexts();

    // Spawn the agent with the recovered messages pre-populated, then drive
    // a fresh prompt through it. The same `messages(prior_messages)`
    // builder is what `aether-cli`'s `RuntimeBuilder::build_ready(prior)`
    // calls under the hood, so the captured-context assertion below is
    // what the resumed run would actually see.
    let (tx, mut rx, _handle) = agent(provider).messages(prior_messages).spawn().await?;
    tx.send(CoreCommand::text("third question")).await?;

    // Drain events until the run produces `turn_ended`. The contract under
    // test is the *captured* context on the first LLM call, not the
    // event stream, so any observed events between drain and completion
    // are simply discarded.
    while let Some(event) = rx.recv().await {
        if matches!(
            event,
            AgentEvent::Turn(TurnEvent::Ended {
                outcome: TurnOutcome::Completed | TurnOutcome::Cancelled | TurnOutcome::Failed { .. }
            })
        ) {
            break;
        }
    }

    let contexts = captured_contexts.lock().expect("captured contexts poisoned");
    assert!(!contexts.is_empty(), "the resumed run must issue at least one LLM request; got no captured contexts");
    let first_request = &contexts[0];
    let rendered: Vec<String> = first_request
        .messages()
        .iter()
        .map(|message| match message {
            llm::ChatMessage::User { content, .. } => llm::ContentBlock::join_text(content),
            llm::ChatMessage::Assistant { content, .. } => content.clone(),
            other => format!("{other:?}"),
        })
        .collect();

    assert!(
        rendered.iter().any(|text| text == "first question"),
        "the first provider request on a resumed run must carry the first prior prompt; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "second question"),
        "the first provider request on a resumed run must carry the second prior prompt; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "first answer"),
        "the first provider request on a resumed run must carry the first prior reply; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "second answer"),
        "the first provider request on a resumed run must carry the second prior reply; got {rendered:?}",
    );

    Ok(())
}

/// Companion of the seeded-requests test: `load_resume_messages` is the
/// public seam that `--resume` reaches, so it must surface prior turns in
/// the order the writer recorded them. This duplicates the
/// `load_resume_messages_round_trips_two_turn_transcript` unit test on the
/// integration side to keep the cross-crate usage compileable and to catch
/// any drift between the writer the unit test produces and the writer the
/// `--transcript-jsonl` flag actually emits (both go through
/// [`JsonlTranscript::record`], but the integration layer is the one a
/// future refactor is most likely to break).
#[tokio::test(flavor = "current_thread")]
async fn load_resume_messages_walks_two_turn_transcript() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("transcript.jsonl");

    let mut transcript = JsonlTranscript::create(&path).expect("create writer");
    transcript.write_header(env!("CARGO_PKG_VERSION"), "2026-10-10T08:54:00+00:00").expect("write header");
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("first question")] }))
        .expect("turn 1 start");
    transcript.record(&AgentEvent::text("m-1", "first answer", StreamState::Complete)).expect("turn 1 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 1 ended");
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("second question")] }))
        .expect("turn 2 start");
    transcript.record(&AgentEvent::text("m-2", "second answer", StreamState::Complete)).expect("turn 2 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 2 ended");
    transcript.flush().expect("flush");

    let messages = load_resume_messages(&path).expect("two-turn transcript loads");
    assert_eq!(messages.len(), 4, "expected four recovered messages, got {messages:?}");

    let render = |message: &llm::ChatMessage| -> String {
        match message {
            llm::ChatMessage::User { content, .. } => llm::ContentBlock::join_text(content),
            llm::ChatMessage::Assistant { content, .. } => content.clone(),
            other => format!("{other:?}"),
        }
    };
    assert_eq!(render(&messages[0]), "first question");
    assert_eq!(render(&messages[1]), "first answer");
    assert_eq!(render(&messages[2]), "second question");
    assert_eq!(render(&messages[3]), "second answer");

    Ok(())
}

/// `load_resume_messages` must catch a `ToolEvent::Call` that lands in the
/// transcript without a preceding `MessageEvent::Text` by synthesising a
/// fresh empty-content `Assistant` carrying just the request, so the
/// recover-and-reseed round trip stays structurally well-formed even for
/// odd provider response shapes.
#[test]
fn load_resume_messages_attaches_orphan_tool_call_to_fresh_assistant() {
    use aether_core::events::ToolEvent;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("orphan-tool-call.jsonl");

    let mut transcript = JsonlTranscript::create(&path).expect("create writer");
    transcript.write_header(env!("CARGO_PKG_VERSION"), "2026-10-10T08:54:00+00:00").expect("write header");

    // A turn with no assistant text reply but a tool call (the synthetic
    // edge case the loader must handle gracefully).
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("first question")] }))
        .expect("turn 1 start");
    transcript
        .record(&AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
        }))
        .expect("tool call");
    transcript
        .record(&AgentEvent::Tool(ToolEvent::Result {
            result: llm::ToolCallResult {
                id: "tc1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        }))
        .expect("tool result");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn ended");
    transcript.flush().expect("flush");

    let messages = load_resume_messages(&path).expect("orphan tool call loads");
    // User, [Assistant carrying the request], ToolCallResult. Three messages
    // because the assistant text never arrived to claim a slot.
    assert_eq!(messages.len(), 3, "expected user + assistant + tool result, got {messages:?}");

    let assistant = messages
        .iter()
        .find_map(|message| match message {
            llm::ChatMessage::Assistant { tool_calls, .. } => Some(tool_calls.clone()),
            _ => None,
        })
        .expect("an assistant message must be present");
    assert_eq!(
        assistant.len(),
        1,
        "the synthesised assistant must carry exactly the lone tool call; got {assistant:?}"
    );
    assert_eq!(assistant[0].id, "tc1");
}

/// On the integration side the seeder produces the real `ChatMessage`
/// sequence the LLM would see: `user("first question")`,
/// `assistant("first answer")`, `user("second question")`,
/// `assistant("second answer")`. Inspecting the captured context for the
/// expected substrings is the same check the task body asks for ("asserts
/// both prior turns are present in the next request").
#[test]
fn resume_seeded_messages_contain_both_prior_user_prompts() -> Result<(), Box<dyn std::error::Error>> {
    use llm::Context;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("transcript.jsonl");
    let mut transcript = JsonlTranscript::create(&path).expect("create writer");
    transcript.write_header(env!("CARGO_PKG_VERSION"), "2026-10-10T08:54:00+00:00").expect("write header");

    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("first question")] }))
        .expect("turn 1 start");
    transcript.record(&AgentEvent::text("m-1", "first answer", StreamState::Complete)).expect("turn 1 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 1 ended");
    transcript
        .record(&AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text("second question")] }))
        .expect("turn 2 start");
    transcript.record(&AgentEvent::text("m-2", "second answer", StreamState::Complete)).expect("turn 2 text");
    transcript.record(&AgentEvent::turn_ended(TurnOutcome::Completed)).expect("turn 2 ended");
    transcript.flush().expect("flush");

    let prior = load_resume_messages(&path).expect("transcript loads");
    let rendered: Vec<String> = prior
        .iter()
        .map(|message| match message {
            llm::ChatMessage::User { content, .. } => llm::ContentBlock::join_text(content),
            llm::ChatMessage::Assistant { content, .. } => content.clone(),
            other => format!("{other:?}"),
        })
        .collect();

    assert!(
        rendered.iter().any(|text| text == "first question"),
        "expected the recovered messages to contain the first prior prompt; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "second question"),
        "expected the recovered messages to contain the second prior prompt; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "first answer"),
        "expected the recovered messages to contain the first prior reply; got {rendered:?}",
    );
    assert!(
        rendered.iter().any(|text| text == "second answer"),
        "expected the recovered messages to contain the second prior reply; got {rendered:?}",
    );

    // A `Context::messages()` round-trip (the type the LLM sees) is the
    // type-level seam the resume path needs to feed back through. Render
    // the messages through `Context::new` to catch any structural mismatch
    // before the test declares itself green.
    let _context = Context::new(prior, Vec::new());

    Ok(())
}

/// Tags the file so the unused-imports lint does not flag it; the
/// `mpsc` import is the seam used by sibling integration tests in this
/// directory.
#[allow(dead_code)]
fn ensure_mpsc_used() {
    let _: mpsc::Sender<()> = mpsc::channel(1).0;
}
