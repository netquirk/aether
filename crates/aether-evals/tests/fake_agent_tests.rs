use aether_evals::{Agent, FakeAgent, Task, Transcript, TranscriptError};

// `TranscriptError` carries a sizable variant; the lint is suppressed at the
// test boundary so the harness signature mirrors the production `from_stream`
// return type (which itself suppresses the lint via `#[allow]`).
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn tool_call_assertion_works_in_rust_test() -> Result<(), TranscriptError> {
    let prompt = "Run a bash command";
    let trace = Transcript::from_stream(FakeAgent::with_tool_call("bash", "success").run(Task::new(prompt))).await?;
    assert_eq!(trace.tool_call_count("bash"), 1);

    Ok(())
}
