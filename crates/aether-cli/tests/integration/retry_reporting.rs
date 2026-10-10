use aether_cli::output::summarize_retries;
use aether_core::core::{RetryConfig, agent};
use aether_core::events::{AgentEvent, Command, TurnEvent, TurnOutcome};
use llm::testing::FakeLlmProvider;
use llm::{LlmError, LlmResponse, ProviderError};
use std::time::Duration;
use tokio::sync::mpsc;

fn fast_retry_config() -> RetryConfig {
    RetryConfig {
        max_attempts: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        // Mid-stream resume is irrelevant for the retry-counting tests below;
        // the fake provider always returns a complete (single-shot) response,
        // so disable it to keep the assertions focused on attempt counts.
        resume_partial: false,
    }
}

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

#[tokio::test(flavor = "current_thread")]
async fn summarize_retries_counts_one_retry_then_success() {
    let provider = FakeLlmProvider::from_results(vec![
        // Initial call: provider returns a transient error.
        vec![Err(LlmError::from(ProviderError::server("boom")))],
        // First retry succeeds.
        vec![Ok(LlmResponse::Start), Ok(LlmResponse::text("reply")), Ok(LlmResponse::done())],
    ])
    .with_display_name("fake:retry-then-ok");

    let (tx, rx, _handle) = agent(provider).retry(fast_retry_config()).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "hi").await;

    let summary = summarize_retries(&events);
    assert_eq!(summary.retries, 1, "expected exactly one retry when error-then-succeed");
    assert!(!summary.failed, "the run should have completed");
    assert_eq!(summary.provider.as_deref(), Some("fake:retry-then-ok"));
}

#[tokio::test(flavor = "current_thread")]
async fn summarize_retries_reports_exhaustion() {
    // Three errors with max_attempts=2 -> initial + 2 retries -> turn fails.
    let provider = FakeLlmProvider::from_results(vec![
        vec![Err(LlmError::from(ProviderError::server("overloaded 1")))],
        vec![Err(LlmError::from(ProviderError::server("overloaded 2")))],
        vec![Err(LlmError::from(ProviderError::server("overloaded 3")))],
    ])
    .with_display_name("fake:exhausted");

    let (tx, rx, _handle) = agent(provider).retry(fast_retry_config()).spawn().await.unwrap();
    let events = drive_to_completion(rx, tx, "hi").await;

    let summary = summarize_retries(&events);
    assert_eq!(summary.retries, 2, "expected max_attempts retries before failure");
    assert!(summary.failed, "the run should be marked failed");
    assert_eq!(summary.provider.as_deref(), Some("fake:exhausted"));

    // The terminal failure event should carry the last provider error message.
    let failure_message = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { error } }) => Some(error.clone()),
            _ => None,
        })
        .expect("the failed terminal event must exist");
    assert!(failure_message.contains("overloaded"), "failure should propagate the provider error: {failure_message}");
}
