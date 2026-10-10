//! TASK-25-421: round-trip the provider's request id from the streaming
//! parser onto the `LlmCallEnded` event the headless loop logs.
//!
//! The id is captured by the streaming parser from the SSE body (`OpenAI`,
//! `OpenRouter`, `Z.ai`, Ollama) or from a response header (Anthropic,
//! Bedrock), threaded through `LlmResponse::Done` → `LlmCallOutcome::Completed`.
//! The headless CLI surfaces it on the run log so each turn can be traced
//! back to the provider-side request; absent means absent (no placeholder).
//!
//! These tests exercise the agent-side path — the `Done` event from the
//! fake LLM is shaped with `done_with_request_id(...)`, and the test asserts
//! the id lands on the `LlmCallEnded` event and is reachable via
//! `LlmCallOutcome::provider_request_id()`. The complement — that the CLI's
//! `info!("llm call ended")` line is emitted with that id — lives in
//! `crates/aether-cli/src/headless/run.rs::tests`.

use aether_core::events::{AgentEvent, TurnEvent};
use aether_core::testing::test_agent;
use llm::{LlmResponse, StopReason};

/// Helper: locate the first `LlmCallEnded` event in the agent's event stream
/// and assert it carries the expected `provider_request_id`. Returning the
/// matched outcome (not the whole event) keeps the assertion site focused on
/// the `provider_request_id()` accessor the headless loop reads.
fn assert_provider_request_id(events: &[AgentEvent], expected: Option<&str>) {
    let outcome = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallEnded { outcome, .. }) => Some(outcome),
            _ => None,
        })
        .expect("the agent must emit at least one LlmCallEnded event");
    assert_eq!(
        outcome.provider_request_id(),
        expected,
        "LlmCallOutcome::provider_request_id() must round-trip the id the LlmResponse::Done carried"
    );
}

#[tokio::test]
async fn llm_call_ended_round_trips_provider_request_id_when_response_carries_one() {
    // Drive the agent with one turn whose terminal `Done` carries
    // `Some("req-1")`. The agent is responsible for copying the id from the
    // streaming parser's `Done` onto `LlmCallOutcome::Completed`; this test
    // pins that copy by reading the id back via the public accessor.
    let responses = vec![vec![
        Ok(LlmResponse::Start),
        Ok(LlmResponse::text("hi")),
        Ok(LlmResponse::done_with_request_id(Some(StopReason::EndTurn), Some("req-1".to_string()))),
    ]];

    let events = test_agent().llm_result_responses(&responses).without_mcp().user_text("hello").run().await.unwrap();

    assert_provider_request_id(&events, Some("req-1"));
}

#[tokio::test]
async fn llm_call_ended_provider_request_id_is_none_when_response_carries_none() {
    // Drive the agent with a `Done` that does NOT carry a request id. The
    // headless CLI's "absent means absent (no placeholder)" rule is enforced
    // here at the agent boundary: `LlmCallOutcome::provider_request_id()`
    // must return `None`, so the CLI never sees a placeholder to log.
    let responses = vec![vec![Ok(LlmResponse::Start), Ok(LlmResponse::text("hi")), Ok(LlmResponse::done())]];

    let events = test_agent().llm_result_responses(&responses).without_mcp().user_text("hello").run().await.unwrap();

    assert_provider_request_id(&events, None);
}
