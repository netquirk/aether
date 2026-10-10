//! Acceptance tests for the 5xx → secondary provider fallback path
//! (TASK-25-465).
//!
//! The agent swaps the configured fallback provider in for the rest of
//! the run when the primary fails with a server (5xx) error, recording
//! the swap as a `ModelEvent::Fallback` in the transcript so the
//! provider that took over is named. With no fallback configured the
//! run fails exactly as it did before this change.

use aether_core::events::{AgentEvent, ModelEvent, TurnEvent, TurnOutcome};
use aether_core::testing::{fast_retry, test_agent};
use llm::ProviderError;
use llm::testing::{failed_call, llm_response};

/// The 5xx path with a configured fallback swaps providers, records a
/// `ModelEvent::Fallback` naming the secondary's display name, and
/// completes the turn on the secondary.
#[tokio::test]
async fn server_error_falls_back_to_secondary_and_records_provider() -> Result<(), Box<dyn std::error::Error>> {
    let primary = vec![failed_call(ProviderError::server("boom").with_http_status(503))];
    let secondary = vec![llm_response().text(&["ok"]).build_results()];

    let result = test_agent()
        .without_mcp()
        .retry_config(fast_retry(0))
        .llm_result_responses(&primary)
        .fallback_llm_result_responses(&secondary)
        .user_text("go")
        .run_with_context()
        .await?;

    // The primary produced one failed call and the secondary produced
    // the successful reply; the test agent's `captured_contexts` only
    // tracks the primary, so we expect exactly one capture from the
    // primary's call.
    let primary_calls = result.captured_contexts.lock().unwrap().len();
    assert_eq!(primary_calls, 1, "primary should have been called once: {:?}", result.messages);

    // The Fallback event names the secondary's display name (the test
    // builder defaults it to "Fallback LLM" so it is distinct from the
    // primary's "Fake LLM" and the "naming the provider used"
    // requirement is provable from the event alone).
    let fallback = result
        .messages
        .iter()
        .find_map(|event| match event {
            AgentEvent::Model(ModelEvent::Fallback { from, to, reason }) => {
                Some((from.clone(), to.clone(), reason.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected ModelEvent::Fallback, got: {:?}", result.messages));
    let (from, to, reason) = fallback;
    assert_eq!(from, "Fake LLM", "from should name the primary provider");
    assert_eq!(to, "Fallback LLM", "to should name the secondary provider");
    assert!(
        reason.contains("503") || reason.contains("Server error"),
        "reason should describe the 5xx trigger, got: {reason:?}"
    );

    // The turn completed on the secondary; no RetryScheduled should
    // appear because the fallback path bypasses the retry budget.
    assert!(
        matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)),
        "turn should complete on the fallback, got {:?}",
        result.messages.last()
    );
    assert!(
        !result.messages.iter().any(|event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { .. }))),
        "fallback path must not emit RetryScheduled, got: {:?}",
        result.messages
    );

    Ok(())
}

/// Without a fallback, a 5xx burns the configured retry budget and
/// then fails the turn exactly as it did before TASK-25-465. The
/// `ModelEvent::Fallback` event is NOT emitted because no secondary
/// provider took over.
#[tokio::test]
async fn no_secondary_still_fails_after_retry_budget() -> Result<(), Box<dyn std::error::Error>> {
    // Six 5xx failures: initial call + three retries consume four; the
    // extra scripts are not consumed (the run ends after the budget is
    // exhausted) and ensure the agent never silently succeeds on a
    // missed retry.
    let primary = vec![
        failed_call(ProviderError::server("boom 1").with_http_status(503)),
        failed_call(ProviderError::server("boom 2").with_http_status(503)),
        failed_call(ProviderError::server("boom 3").with_http_status(503)),
        failed_call(ProviderError::server("boom 4").with_http_status(503)),
        failed_call(ProviderError::server("boom 5").with_http_status(503)),
        failed_call(ProviderError::server("boom 6").with_http_status(503)),
    ];

    let result = test_agent()
        .without_mcp()
        .retry_config(fast_retry(3))
        .llm_result_responses(&primary)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_attempts: Vec<u32> = result
        .messages
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(TurnEvent::RetryScheduled { attempt, .. }) => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(
        retry_attempts,
        vec![1, 2, 3],
        "should see exactly three retries before the budget is exhausted: {:?}",
        result.messages
    );

    assert!(
        matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Failed { .. })),
        "turn should fail after the retry budget is exhausted, got {:?}",
        result.messages.last()
    );

    assert!(
        !result.messages.iter().any(|event| matches!(event, AgentEvent::Model(ModelEvent::Fallback { .. }))),
        "no Fallback event should be emitted when no secondary is configured, got: {:?}",
        result.messages
    );

    Ok(())
}
