use aether_core::events::{AgentEvent, ContextEvent, LlmCallOutcome, TurnEvent, TurnOutcome};
use aether_core::testing::{TestScenario, test_agent};
use aether_sessions::testing::{
    agent_switched, assistant_text, compaction_result, llm_call_started, partial_text, tool_call, tool_error,
    tool_result, turn_ended, user_message,
};
use aether_sessions::{
    SessionEvent, TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events,
};
use llm::LlmCallPurpose;
use llm::testing::FakeLlmProvider;
use llm::testing::llm_response;

#[test]
fn reconstruction_preserves_stored_message_identity() {
    let context = context_from_events(&[assistant_text("stored-id", "Response"), turn_ended(TurnOutcome::Completed)]);
    let value = serde_json::to_value(&context.messages()[0]).unwrap();
    assert_eq!(value["message_id"], "stored-id");
}

#[test]
fn reconstruction_preserves_messages_within_one_turn() {
    let context = context_from_events(&[
        assistant_text("first", "First response"),
        assistant_text("second", "Second response"),
        turn_ended(TurnOutcome::Completed),
    ]);
    assert_eq!(context.message_count(), 2);
    assert!(
        matches!(&context.messages()[0], llm::ChatMessage::Assistant { content, .. } if content == "First response")
    );
    assert!(
        matches!(&context.messages()[1], llm::ChatMessage::Assistant { content, .. } if content == "Second response")
    );
}

#[test]
fn reconstructs_conversation_and_ignores_control_events() {
    let messages = conversation_messages_from_events(&[
        user_message("Hello"),
        agent_switched(None, Some("coder")),
        assistant_text("message-1", "Hi there!"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], llm::ChatMessage::User { .. }));
    assert!(matches!(messages[1], llm::ChatMessage::Assistant { .. }));
}

#[test]
fn task_completion_during_iteration_preserves_pending_tools() {
    let task = aether_core::events::ToolEvent::TaskCancelled {
        request: llm::ToolCallRequest {
            id: "background-call".into(),
            name: "background".into(),
            arguments: "{}".into(),
        },
        task_id: "task".into(),
    };
    let context = context_from_events(&[
        tool_result("call", "read", "contents"),
        SessionEvent::Agent(AgentEvent::Tool(task.clone())),
        assistant_text("assistant", "done"),
        turn_ended(TurnOutcome::Completed),
    ]);
    assert_eq!(context.message_count(), 3);
    assert_eq!(context.messages()[0].message_id().as_deref(), Some("assistant"));
    assert!(context.messages()[1].is_tool_result());
    assert_eq!(context.messages()[2].message_id(), task.task_context_message().unwrap().message_id());
}

#[test]
fn reconstructs_successful_tool_calls() {
    let context = context_from_events(&[
        user_message("Read Cargo.toml"),
        tool_call("call-1", "read_file", "{}"),
        tool_result("call-1", "read_file", "file contents"),
        assistant_text("message-1", "Here is the file"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 3);
    assert!(
        matches!(&context.messages()[1], llm::ChatMessage::Assistant { content, tool_calls, .. } if content == "Here is the file" && tool_calls.len() == 1)
    );
    assert!(context.messages()[2].is_tool_result());
}

#[test]
fn reconstructs_tools_failures_and_context_boundaries() {
    let events = [
        user_message("Read missing.txt"),
        tool_error("call-1", "read_file", "file not found"),
        assistant_text("tool-response", ""),
        turn_ended(TurnOutcome::Completed),
        SessionEvent::Agent(AgentEvent::Context(ContextEvent::Cleared)),
        user_message("Start fresh"),
    ];
    let context = context_from_events(&events);
    let before_clear = context_from_events(&events[..4]);

    assert_eq!(before_clear.message_count(), 3);
    assert!(
        matches!(before_clear.messages()[2], llm::ChatMessage::ToolCallResult(Err(ref error)) if error.error == "file not found")
    );
    assert_eq!(context.message_count(), 1);
    assert!(matches!(context.messages()[0], llm::ChatMessage::User { .. }));
}

#[test]
fn empty_or_contentless_turns_do_not_add_messages() {
    assert_eq!(context_from_events(&[]).message_count(), 0);
    assert_eq!(context_from_events(&[turn_ended(TurnOutcome::Completed)]).message_count(), 0);
}

#[test]
fn completed_turns_reset_the_accumulator() {
    let context = context_from_events(&[
        assistant_text("message-1", "Turn 1"),
        turn_ended(TurnOutcome::Completed),
        assistant_text("message-2", "Turn 2"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 2);
}

#[test]
fn compaction_replaces_prior_messages_with_a_summary() {
    let context = context_from_events(&[
        user_message("Hello"),
        assistant_text("message-1", "Hi!"),
        turn_ended(TurnOutcome::Completed),
        compaction_result("Earlier we greeted each other.", 2),
        user_message("What did we talk about?"),
    ]);

    assert_eq!(context.message_count(), 2);
    assert!(context.messages()[0].is_summary());
}

#[test]
fn complete_messages_are_reconstructed_but_streaming_chunks_are_not() {
    let context = context_from_events(&[
        partial_text("partial", "partial"),
        assistant_text("complete", "complete"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 1);
    assert!(matches!(&context.messages()[0], llm::ChatMessage::Assistant { content, .. } if content == "complete"));
}

#[test]
fn turn_entries_name_each_turns_model_from_persisted_events() {
    let events = vec![
        // Turn 1: served by codex:gpt-5.5
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Started { content: vec![] })),
        llm_call_started(LlmCallPurpose::Chat, Some("codex"), Some("gpt-5.5"), "codex"),
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: None },
        })),
        assistant_text("m1", "first answer"),
        turn_ended(TurnOutcome::Completed),
        // Turn 2: served by anthropic:claude-opus-4-6
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Started { content: vec![] })),
        llm_call_started(LlmCallPurpose::Chat, Some("anthropic"), Some("claude-opus-4-6"), "Anthropic"),
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: None },
        })),
        assistant_text("m2", "second answer"),
        turn_ended(TurnOutcome::Failed { error: "boom".into() }),
    ];

    let entries = turn_entries_from_events(&events);
    assert_eq!(entries.len(), 2);

    assert_eq!(
        entries[0],
        TurnEntry {
            turn_index: 0,
            outcome: TurnOutcome::Completed,
            provider: Some("codex".into()),
            model_id: Some("gpt-5.5".into()),
            display_name: Some("codex".into()),
        }
    );
    assert_eq!(
        entries[1],
        TurnEntry {
            turn_index: 1,
            outcome: TurnOutcome::Failed { error: "boom".into() },
            provider: Some("anthropic".into()),
            model_id: Some("claude-opus-4-6".into()),
            display_name: Some("Anthropic".into()),
        }
    );
}

#[test]
fn turn_entries_record_turns_without_a_chat_call_with_no_model() {
    // A turn that emitted no LlmCallStarted (e.g. an empty or aborted turn) is
    // still recorded: a transcript entry for a turn whose model is unknown is
    // more useful than a gap, so the reader can see the turn happened.
    let events = vec![
        turn_ended(TurnOutcome::Completed),
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Started { content: vec![] })),
        llm_call_started(LlmCallPurpose::Chat, Some("openai"), Some("gpt-5.5"), "OpenAI"),
        assistant_text("m", "ok"),
        turn_ended(TurnOutcome::Completed),
    ];

    let entries = turn_entries_from_events(&events);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].provider, None);
    assert_eq!(entries[0].model_id, None);
    assert_eq!(entries[0].turn_index, 0);
    assert_eq!(entries[1].provider.as_deref(), Some("openai"));
    assert_eq!(entries[1].model_id.as_deref(), Some("gpt-5.5"));
    assert_eq!(entries[1].turn_index, 1);
}

#[tokio::test]
async fn persisted_transcript_records_each_turns_model_across_a_switch() {
    // First turn uses the openai model; second turn uses the anthropic one.
    let initial: llm::LlmModel = "openai:gpt-5.5".parse().expect("model parses");
    let switched: llm::LlmModel = "anthropic:claude-opus-4-6".parse().expect("model parses");

    let second_provider = FakeLlmProvider::with_single_response(llm_response().text(&["after switch"]).build())
        .with_model(switched.clone())
        .with_display_name("Anthropic");

    let events = test_agent()
        .without_mcp()
        .model(initial.clone())
        .llm_responses(&[llm_response().text(&["first"]).build()])
        .scenario(
            TestScenario::new()
                .user_text("hi")
                .wait_for_turn_end()
                .switch_model(second_provider)
                .user_text("after switch")
                .wait_for_turn_end(),
        )
        .run()
        .await
        .expect("agent run succeeds");

    let mut persisted: Vec<SessionEvent> = vec![user_message("hi")];
    persisted.extend(events.iter().cloned().map(SessionEvent::Agent));
    let store = aether_sessions::testing::TestStore::new().session("mixed", &persisted);

    // Read the events back from disk so the assertions are over what the
    // run transcript would actually contain after a reload.
    let (_, reloaded) = store.store().load("mixed").expect("session loads");
    let entries = turn_entries_from_events(&reloaded);

    assert_eq!(entries.len(), 2, "two turns were run: {entries:?}");
    assert_eq!(entries[0].turn_index, 0);
    assert_eq!(entries[0].provider.as_deref(), Some(initial.provider()));
    assert_eq!(entries[0].model_id.as_deref(), Some(initial.model_id().as_ref()));
    assert_eq!(entries[0].outcome, TurnOutcome::Completed);
    assert_eq!(entries[1].turn_index, 1);
    assert_eq!(entries[1].provider.as_deref(), Some(switched.provider()));
    assert_eq!(entries[1].model_id.as_deref(), Some(switched.model_id().as_ref()));
    assert_eq!(entries[1].outcome, TurnOutcome::Completed);
}
