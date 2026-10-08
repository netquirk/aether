//! Integration tests for the repetition detector.
//!
//! The detector observes every completed iteration inside a turn: the same
//! assistant text (and the same text-only tool call, never executed) drives
//! the loop. After the configured threshold the turn must end with
//! `TurnOutcome::Failed` naming the repetition rather than continuing to
//! re-issue the model call.

use std::error::Error;

use aether_core::events::{AgentEvent, TurnEvent, TurnOutcome};
use aether_core::testing::test_agent;
use llm::testing::llm_response;

/// Build the scripted "loop" response — an identical assistant message plus
/// the same text-only tool call, each one in its own response frame so the
/// `FakeLlmProvider` issues one LLM call per `Vec` element.
fn loop_iteration() -> Vec<Result<llm::LlmResponse, llm::LlmError>> {
    llm_response().text(&["same output"]).tool_call("call-1", "looping-tool", &["{}"]).build_results()
}

/// A plain text response with no tool call. Used to terminate a sequence of
/// tool-calling iterations cleanly.
fn plain_iteration(text: &str) -> Vec<Result<llm::LlmResponse, llm::LlmError>> {
    llm_response().text(&[text]).build_results()
}

/// Pulls the `error` string out of the last `TurnEvent::Ended` whose outcome is
/// `Failed`, returning whether any was found.
fn failed_error(messages: &[AgentEvent]) -> Option<String> {
    let mut found = None;
    for event in messages {
        if let AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error } }) = event {
            found = Some(error.clone());
        }
    }
    found
}

#[tokio::test]
async fn repeated_tool_call_and_text_ends_turn_after_threshold() -> Result<(), Box<dyn Error>> {
    const K: usize = 3;
    // Build (K + 2) scripted iterations; the agent must only consume K of
    // them before stopping, so an unpatched run that loops unbounded would
    // exceed `K` LLM calls and fail the count assertion.
    let mut iterations = Vec::with_capacity(K + 2);
    for _ in 0..(K + 2) {
        iterations.push(loop_iteration());
    }

    let result = test_agent()
        .without_mcp()
        .repetition_limit(u32::try_from(K).expect("threshold fits in u32"))
        .llm_result_responses(&iterations)
        .user_text("go")
        .run_with_context()
        .await?;

    let error = failed_error(&result.messages).unwrap_or_else(|| {
        panic!("expected a Failed TurnEvent::Ended naming the repetition, got {:?}", result.messages)
    });
    assert!(error.to_lowercase().contains("repetition"), "error message should mention the repetition, got {error:?}");

    let captured = result.captured_contexts.lock().expect("captured contexts poisoned");
    assert_eq!(
        captured.len(),
        K,
        "agent should have stopped after K={K} identical iterations; got {} calls",
        captured.len(),
    );

    Ok(())
}

#[tokio::test]
async fn repetition_under_threshold_is_not_a_false_positive() -> Result<(), Box<dyn Error>> {
    // One fewer identical iteration than the threshold, then a differing
    // iteration, then a plain terminal text — the detector must NOT end the
    // turn as failed.
    let threshold: u32 = 3;
    let mut iterations = Vec::new();
    for _ in 0..(threshold - 1) {
        iterations.push(loop_iteration());
    }
    iterations.push(
        llm_response().text(&["different output"]).tool_call("call-2", "looping-tool", &["{\"x\":1}"]).build_results(),
    );
    iterations.push(plain_iteration("done"));

    let result = test_agent()
        .without_mcp()
        .repetition_limit(threshold)
        .llm_result_responses(&iterations)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(
        failed_error(&result.messages).is_none(),
        "iteration under the threshold must not end the turn as failed: {:?}",
        result.messages,
    );

    let captured = result.captured_contexts.lock().expect("captured contexts poisoned");
    // threshold - 1 (loop) + 1 (differing) + 1 (terminal) = threshold + 1
    assert_eq!(
        captured.len(),
        (threshold + 1) as usize,
        "expected exactly threshold + 1 LLM calls before the turn cleanly completes; got {}",
        captured.len(),
    );

    Ok(())
}

#[tokio::test]
async fn differing_outputs_each_iteration_do_not_trigger_detector() -> Result<(), Box<dyn Error>> {
    let iterations = vec![
        llm_response().text(&["one"]).tool_call("call-1", "tools__a", &["{\"n\":1}"]).build_results(),
        llm_response().text(&["two"]).tool_call("call-2", "tools__b", &["{\"n\":2}"]).build_results(),
        llm_response().text(&["three"]).tool_call("call-3", "tools__c", &["{\"n\":3}"]).build_results(),
        plain_iteration("done"),
    ];

    let result = test_agent()
        .without_mcp()
        .repetition_limit(2)
        .llm_result_responses(&iterations)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(
        failed_error(&result.messages).is_none(),
        "distinct signatures must not trip the detector: {:?}",
        result.messages,
    );

    Ok(())
}
