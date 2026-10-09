//! Tests for [`ToolPolicy`](aether_core::core::ToolPolicy): a tool call that the
//! policy refuses must surface on the event stream as
//! [`ToolEvent::Refused`](aether_core::events::ToolEvent::Refused), record the
//! refusal reason in the conversation context, and place a synthetic tool result
//! in the assistant turn so the next LLM call has something to react to.

use std::error::Error;
use std::sync::Arc;

use aether_core::core::ToolPolicy;
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent, TurnOutcome};
use aether_core::testing::{FakeAgentObserver, test_agent};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, ToolCallRequest};
use serde_json::json;

/// Always refuses any tool call the LLM requests, with the given fixed reason.
struct RefuseAll {
    reason: String,
}

impl ToolPolicy for RefuseAll {
    fn refuse(&self, _request: &ToolCallRequest) -> Option<String> {
        Some(self.reason.clone())
    }
}

#[tokio::test]
async fn refused_tool_call_emits_refused_event_with_reason() -> Result<(), Box<dyn Error>> {
    let tool_request = json!({ "command": "rm -rf /" });
    let llm_responses = [
        llm_response().tool_call("call_1", "bash", &[&tool_request.to_string()]).build(),
        llm_response().text(&["okay"]).build(),
    ];

    let observer = FakeAgentObserver::new();
    let events = observer.events();

    let policy: Arc<dyn ToolPolicy> = Arc::new(RefuseAll { reason: "bash is not permitted by policy".to_string() });

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_text("delete the world")
        .tool_policy(policy)
        .observer(Box::new(observer))
        .run()
        .await?;

    assert_eq!(messages.len(), events.lock().unwrap().len(), "observer must see every event the agent emits");

    let refused = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            AgentEvent::Tool(ToolEvent::Refused { request, reason }) => Some((request.clone(), reason.clone())),
            _ => None,
        })
        .expect("ToolEvent::Refused should be emitted for a refused tool call");

    let (refused_request, refused_reason) = refused;
    assert_eq!(refused_request.id, "call_1");
    assert_eq!(refused_request.name, "bash");
    assert_eq!(refused_request.arguments, tool_request.to_string());
    assert_eq!(refused_reason, "bash is not permitted by policy");

    // No tool actually executed; the trace must NOT contain an ExecutionStarted event
    // for the refused call, nor any Result/Error.
    let observer_events = events.lock().unwrap();
    assert!(
        !observer_events
            .iter()
            .any(|event| matches!(event, AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, .. }) if tool_id == "call_1")),
        "refused call must not be marked as executing"
    );
    assert!(
        !observer_events
            .iter()
            .any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Result { result, .. }) if result.id == "call_1")),
        "refused call must not produce a tool result event"
    );
    assert!(
        !observer_events
            .iter()
            .any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Error { error, .. }) if error.id == "call_1")),
        "refused call must not produce a tool error event"
    );

    assert!(matches!(
        messages.last(),
        Some(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed, .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn refused_tool_call_records_refusal_in_context_for_next_turn() -> Result<(), Box<dyn Error>> {
    let tool_request = json!({ "command": "whoami" });
    let llm_responses = [
        llm_response().tool_call("call_1", "bash", &[&tool_request.to_string()]).build(),
        llm_response().text(&["got it"]).build(),
    ];

    let result = test_agent()
        .llm_responses(&llm_responses)
        .user_text("who am I?")
        .tool_policy(Arc::new(RefuseAll { reason: "no shell access".to_string() }))
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert!(contexts.len() >= 2, "expected at least two LLM calls (initial and after refusal)");

    let second_context = &contexts[1];
    let refusal_message = second_context
        .messages()
        .iter()
        .find_map(|message| match message {
            ChatMessage::User { content, .. } => {
                let text = ContentBlock::join_text(content);
                text.contains("<tool-refused").then_some(text)
            }
            _ => None,
        })
        .expect("a refusal context message should be appended to the conversation for the next turn");

    assert!(refusal_message.contains("bash"));
    assert!(refusal_message.contains("no shell access"));
    Ok(())
}

#[tokio::test]
async fn tool_policy_that_returns_none_lets_the_call_proceed() -> Result<(), Box<dyn Error>> {
    let tool_request = json!({ "a": 1, "b": 2 });
    let _tool_result = json!({ "sum": 3 });
    let llm_responses = [
        llm_response().tool_call("call_1", "test__add_numbers", &[&tool_request.to_string()]).build(),
        llm_response().text(&["the sum is 3"]).build(),
    ];

    let observer = FakeAgentObserver::new();
    let events = observer.events();

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_text("1+2")
        .tool_policy(Arc::new(AllowOnly))
        .observer(Box::new(observer))
        .run()
        .await?;

    let observer_events = events.lock().unwrap();
    assert!(
        !observer_events.iter().any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Refused { .. }))),
        "a permissive policy must not emit any Refused events"
    );
    assert!(
        observer_events
            .iter()
            .any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Result { result, .. }) if result.id == "call_1")),
        "a permissive policy must let the tool run and produce a result"
    );

    assert!(messages.iter().any(|event| matches!(
        event,
        AgentEvent::Message(MessageEvent::Text { chunk, .. }) if chunk.contains("the sum is 3")
    )));
    Ok(())
}

struct AllowOnly;

impl ToolPolicy for AllowOnly {
    fn refuse(&self, _request: &ToolCallRequest) -> Option<String> {
        None
    }
}
