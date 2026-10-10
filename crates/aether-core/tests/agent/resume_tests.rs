use std::error::Error;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, TurnEvent, TurnOutcome};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, fast_retry, test_agent};
use llm::testing::{failed_call, llm_response};
use llm::{ChatMessage, ContentBlock, LlmResponse, ProviderError};

fn completed(messages: &[AgentEvent]) -> bool {
    matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed))
}

fn retries(messages: &[AgentEvent]) -> usize {
    messages.iter().filter(|event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { .. }))).count()
}

fn user_texts(messages: &[ChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

fn assistant_texts(messages: &[ChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::Assistant { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn cut_after_text_resumes_from_the_partial_reply() -> Result<(), Box<dyn Error>> {
    let attempts = vec![
        llm_response()
            .text(&["The fix is in ", "parser.rs: the"])
            .build_interrupted(ProviderError::stream_interrupted("cut")),
        llm_response().text(&[" loop never ends."]).build_results(),
    ];

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages), "turn should complete: {:?}", result.messages);
    assert_eq!(retries(&result.messages), 1);

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 2, "one call cut, one resumed");
    let resumed = captured[1].messages();
    assert_eq!(
        assistant_texts(resumed),
        vec!["The fix is in parser.rs: the".to_string()],
        "the partial reply must be kept, not re-generated"
    );
    let note = user_texts(resumed).pop().expect("resume note");
    assert!(note.contains("cut off"), "{note}");
    assert!(note.contains("Continue exactly where you stopped"), "{note}");
    assert_eq!(
        captured[0].messages().len() + 2,
        resumed.len(),
        "the resumed call extends the first call's context, so its prefix stays cached"
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn completed_tool_call_before_the_cut_runs_once_and_is_kept() -> Result<(), Box<dyn Error>> {
    let arguments = serde_json::json!({}).to_string();
    let server = FakeMcpServer::new().with_tool(FakeTool::new("build").responds(FakeToolResponse::text("build ok")));
    let server_state = server.state();

    let attempts = vec![
        llm_response()
            .text(&["Building."])
            .tool_call("call-1", "tools__build", &[&arguments])
            .build_interrupted(ProviderError::stream_interrupted("cut after tool call")),
        llm_response().text(&["Build passed."]).build_results(),
    ];

    let result = test_agent()
        .fake_mcp_server("tools", server)
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages), "turn should complete: {:?}", result.messages);
    assert_eq!(server_state.calls_for("build").len(), 1, "a completed tool call must not run twice");

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let resumed = captured[1].messages();
    assert!(
        resumed
            .iter()
            .any(|message| matches!(message, ChatMessage::ToolCallResult(Ok(result)) if result.id == "call-1")),
        "the tool result must be in the resumed context: {resumed:?}"
    );
    let note = user_texts(resumed).pop().expect("resume note");
    assert!(note.contains("their results are above"), "{note}");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn tool_call_cut_mid_arguments_is_replayed_and_not_run() -> Result<(), Box<dyn Error>> {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("write").responds(FakeToolResponse::text("written")));
    let server_state = server.state();

    let mut cut = vec![
        Ok(LlmResponse::Start),
        Ok(LlmResponse::tool_request_start("call-1", "tools__write")),
        Ok(LlmResponse::tool_request_arg("call-1", r#"{"path":"src/main.rs","content":"fn ma"#)),
    ];
    cut.push(Err(ProviderError::stream_interrupted("cut mid arguments").into()));
    let complete_args = r#"{"path":"src/main.rs","content":"fn main() {}"}"#;
    let attempts = vec![
        cut,
        llm_response().tool_call("call-2", "tools__write", &[complete_args]).build_results(),
        llm_response().text(&["Done."]).build_results(),
    ];

    let result = test_agent()
        .fake_mcp_server("tools", server)
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages), "turn should complete: {:?}", result.messages);
    assert_eq!(server_state.calls_for("write").len(), 1, "only the re-issued complete call may run");

    let captured = result.captured_contexts.lock().unwrap();
    let note = user_texts(captured[1].messages()).pop().expect("resume note");
    assert!(note.contains("`tools__write` tool call"), "{note}");
    assert!(note.contains("did NOT run"), "{note}");
    assert!(note.contains(r#"{"path":"src/main.rs","content":"fn ma"#), "partial arguments must be replayed: {note}");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cut_before_any_output_retries_the_same_request() -> Result<(), Box<dyn Error>> {
    let attempts = vec![
        llm_response().build_interrupted(ProviderError::stream_interrupted("cut before output")),
        llm_response().text(&["ok"]).build_results(),
    ];

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages));
    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0].messages().len(),
        captured[1].messages().len(),
        "nothing was produced, so the retry re-sends the identical request with no note"
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cut_after_reasoning_only_retries_the_same_request() -> Result<(), Box<dyn Error>> {
    let attempts = vec![
        llm_response().reasoning(&["thinking about it"]).build_interrupted(ProviderError::stream_interrupted("cut")),
        llm_response().text(&["ok"]).build_results(),
    ];

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages));
    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured[0].messages().len(), captured[1].messages().len(), "unsigned partial reasoning is not kept");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn leftovers_of_a_cut_stream_are_ignored() -> Result<(), Box<dyn Error>> {
    let arguments = serde_json::json!({}).to_string();
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("slow").responds(FakeToolResponse::text("ok").delay(std::time::Duration::from_millis(50))),
    );

    // The provider reports the cut, then still closes with Done while the
    // tool from the cut call is running.
    let attempts = vec![
        llm_response()
            .text(&["Running."])
            .tool_call("call-1", "tools__slow", &[&arguments])
            .build_with_error(ProviderError::stream_interrupted("cut")),
        llm_response().text(&["finished"]).build_results(),
    ];

    let result = test_agent()
        .fake_mcp_server("tools", server)
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages), "turn should complete: {:?}", result.messages);
    let completed_calls = result
        .messages
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    outcome: aether_core::events::LlmCallOutcome::Completed { .. },
                    ..
                })
            )
        })
        .count();
    assert_eq!(completed_calls, 1, "the cut call must not also be reported as completed: {:?}", result.messages);

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn resume_disabled_retries_from_scratch() -> Result<(), Box<dyn Error>> {
    let attempts = vec![
        llm_response().text(&["partial"]).build_interrupted(ProviderError::stream_interrupted("cut")),
        llm_response().text(&["whole reply"]).build_results(),
    ];

    let result = test_agent()
        .retry_config(RetryConfig { resume_partial: false, ..fast_retry(3) })
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages));
    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured[0].messages().len(), captured[1].messages().len());
    assert!(assistant_texts(captured[1].messages()).is_empty());

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_clean_reply_restores_the_resume_budget() -> Result<(), Box<dyn Error>> {
    let arguments = serde_json::json!({}).to_string();
    let server = FakeMcpServer::new().with_tool(FakeTool::new("step").responds(FakeToolResponse::text("ok")));

    // Budget of 1: cut, resume, clean tool call, cut again, resume, finish.
    let attempts = vec![
        llm_response().text(&["one"]).build_interrupted(ProviderError::stream_interrupted("cut 1")),
        llm_response().tool_call("call-1", "tools__step", &[&arguments]).build_results(),
        llm_response().text(&["two"]).build_interrupted(ProviderError::stream_interrupted("cut 2")),
        llm_response().text(&["done"]).build_results(),
    ];

    let result = test_agent()
        .fake_mcp_server("tools", server)
        .retry_config(fast_retry(1))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(completed(&result.messages), "turn should complete: {:?}", result.messages);
    assert_eq!(retries(&result.messages), 2);

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn non_retryable_error_after_output_still_fails_the_turn() -> Result<(), Box<dyn Error>> {
    let attempt = llm_response().text(&["partial"]).build_interrupted(ProviderError::api("HTTP 400"));
    let attempts = vec![attempt, failed_call(ProviderError::api("unused"))];

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Failed { .. })));
    assert_eq!(retries(&result.messages), 0);

    Ok(())
}
