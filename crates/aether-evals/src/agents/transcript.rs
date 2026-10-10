use super::{AgentRunResult, RunError};
use crate::EvalRunError;
use crate::git_repo::GitRepo;
use aether_core::events::{AgentEvent, LlmCallOutcome, ToolEvent, TurnEvent, TurnOutcome};
use futures::{Stream, StreamExt};
use llm::{SessionUsageTotals, TokenUsage};
use std::fmt::{self, Debug, Display};
use std::path::PathBuf;
use thiserror::Error;

/// The git commit the run started from, or an explicit signal that the working
/// directory was not a git repository at capture time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartingCommit {
    Commit(String),
    NotARepository,
}

/// Header that captures, at run start, where the run happened and the git state of
/// that directory. A header is captured once at the beginning of the run and stored on
/// the [`Transcript`] alongside the streamed `AgentEvent`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptHeader {
    pub working_dir: PathBuf,
    pub starting_commit: StartingCommit,
}

impl TranscriptHeader {
    /// Capture a header for `working_dir`. The directory's current `HEAD` is recorded
    /// when it lives inside a git repository; otherwise `starting_commit` is set to
    /// [`StartingCommit::NotARepository`].
    pub fn capture(working_dir: impl Into<PathBuf>) -> Self {
        let working_dir = working_dir.into();
        let starting_commit = GitRepo::from_path(&working_dir)
            .head_commit()
            .map_or_else(|_| StartingCommit::NotARepository, StartingCommit::Commit);
        Self { working_dir, starting_commit }
    }
}

impl Display for StartingCommit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartingCommit::Commit(sha) => formatter.write_str(sha),
            StartingCommit::NotARepository => formatter.write_str("not a git repository"),
        }
    }
}

impl Display for TranscriptHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Working directory: {}", self.working_dir.display())?;
        write!(formatter, "Starting commit: {}", self.starting_commit)
    }
}

pub struct Transcript {
    events: Vec<AgentEvent>,
    header: Option<TranscriptHeader>,
}

pub struct ToolCall<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
    /// Exit code returned by the tool, when applicable.
    ///
    /// Populated for shell (bash) calls that produced a `ToolEvent::Result`
    /// whose payload includes an `exitCode` field or whose display metadata
    /// contains a `(exit N)` tail. `None` for non-shell tools, failed calls
    /// (`ToolEvent::Error`), or results that carry no exit code.
    pub exit_code: Option<i32>,
}

/// Token usage attributed to a single completed turn.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsage {
    pub outcome: TurnOutcome,
    pub usage: TokenUsage,
}

#[derive(Error)]
#[error("{error}")]
pub struct TranscriptError {
    transcript: Transcript,
    #[source]
    error: EvalRunError,
}

impl Transcript {
    pub fn new(events: Vec<AgentEvent>) -> Self {
        Self { events, header: None }
    }

    pub fn with_header(mut self, header: TranscriptHeader) -> Self {
        self.header = Some(header);
        self
    }

    /// Attach a header that captures `working_dir` and its current git HEAD commit (or
    /// records that the directory is not a git repository).
    pub fn with_working_dir(self, working_dir: impl Into<PathBuf>) -> Self {
        self.with_header(TranscriptHeader::capture(working_dir))
    }

    pub fn header(&self) -> Option<&TranscriptHeader> {
        self.header.as_ref()
    }

    // `TranscriptError` aggregates several large variant payloads; the size trips
    // `clippy::result_large_err` on a recent compiler but the size is
    // intentional (the error carries the full offending transcript so a
    // caller can render a useful diagnostic). Suppress locally so the
    // TASK-25-421 changes do not have to refactor an unrelated type.
    #[allow(clippy::result_large_err)]
    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {
        let mut transcript = Self::default();
        futures::pin_mut!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    transcript.add(event);
                }
                Err(error) => return Err(TranscriptError::new(transcript, error)),
            }
        }
        Ok(transcript)
    }

    pub fn add(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn all_tool_calls(&self) -> impl Iterator<Item = ToolCall<'_>> + '_ {
        self.events.iter().filter_map(|event| match event {
            AgentEvent::Tool(ToolEvent::Result { result, result_meta }) => Some(ToolCall {
                name: &result.name,
                arguments: &result.arguments,
                exit_code: shell_exit_code(
                    result_meta.as_ref().map(|meta| meta.display.value.as_str()),
                    &result.result,
                ),
            }),
            AgentEvent::Tool(ToolEvent::Error { error, .. }) => Some(ToolCall {
                name: &error.name,
                arguments: error.arguments.as_deref().unwrap_or(""),
                exit_code: None,
            }),
            AgentEvent::Tool(ToolEvent::Refused { request, .. }) => {
                Some(ToolCall { name: &request.name, arguments: &request.arguments, exit_code: None })
            }
            _ => None,
        })
    }

    pub fn tool_calls<'a>(&'a self, name: &'a str) -> impl Iterator<Item = ToolCall<'a>> + 'a {
        self.all_tool_calls().filter(move |call| call.name == name)
    }

    pub fn tool_called(&self, name: &str) -> bool {
        self.tool_calls(name).next().is_some()
    }

    pub fn tool_call_count(&self, name: &str) -> usize {
        self.tool_calls(name).count()
    }

    /// Session-wide token totals and estimated cost from the last usage event,
    /// or zeroed totals if no usage was recorded.
    pub fn usage(&self) -> SessionUsageTotals {
        self.events
            .iter()
            .rev()
            .find_map(|event| match event {
                AgentEvent::SessionUsage(usage) => Some(usage.totals.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Token usage attributed to each completed turn, in order.
    ///
    /// Each `TurnUsage` is produced when a [`TurnEvent::Ended`] event is observed, summing the
    /// `usage` carried by every [`TurnEvent::LlmCallEnded`] that completed during that turn. A
    /// turn with no completed calls — for example, one whose only calls failed or were cancelled
    /// — yields a zeroed `TokenUsage` rather than inheriting the previous turn's tokens.
    pub fn turn_usage(&self) -> Vec<TurnUsage> {
        let mut turn_usages = Vec::new();
        let mut pending = TokenUsage::default();
        for event in &self.events {
            match event {
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    outcome: LlmCallOutcome::Completed { usage: Some(usage), .. },
                    ..
                }) => {
                    pending += *usage;
                }
                AgentEvent::Turn(TurnEvent::Ended { outcome }) => {
                    turn_usages.push(TurnUsage { outcome: outcome.clone(), usage: pending });
                    pending = TokenUsage::default();
                }
                _ => {}
            }
        }
        turn_usages
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl From<Vec<AgentEvent>> for Transcript {
    fn from(events: Vec<AgentEvent>) -> Self {
        Self::new(events)
    }
}

impl ToolCall<'_> {
    pub fn arguments_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.arguments)
    }
}

/// Extract the shell exit code from a `ToolEvent::Result`, if any.
///
/// The bash server records the exit code in two places: a structured
/// `exitCode` field on the serialized result, and a `"(exit N)"` tail appended
/// to the result's display metadata. Both can survive `maybe_spillover`'s head
/// preview because the metadata always travels alongside the result and the
/// preview keeps the prefix that contains `exitCode`.
///
/// The display detail is tried first because it is the most reliable source
/// (a single line of text), then the structured payload as a fallback. Either
/// source independently yields the same code when both are present.
pub(crate) fn shell_exit_code(display_detail: Option<&str>, payload: &str) -> Option<i32> {
    parse_exit_detail(display_detail?).or_else(|| parse_exit_code(payload))
}

/// Pull an exit code from a `<display-value>` trailing `(exit N)` segment.
///
/// Handles the standard bash form (`/path (exit 7)`) and the timed-out form
/// (`/path (exit -1, timed out)`). Returns `None` for any other shape.
fn parse_exit_detail(value: &str) -> Option<i32> {
    let marker = " (exit ";
    let index = value.rfind(marker)?;
    let tail = &value[index + marker.len()..];
    // Optional sign, then ASCII digits. The terminating `)` may be followed by
    // either end-of-string or `, timed out)`, so scan past both.
    let bytes = tail.as_bytes();
    let (sign, start) = match bytes.first() {
        Some(b'-') => (-1, 1),
        _ => (1, 0),
    };
    let end =
        bytes[start..].iter().position(|byte| !byte.is_ascii_digit()).map_or(bytes.len(), |offset| start + offset);
    let digits = &tail[start..end];
    let parsed: i32 = digits.parse().ok()?;
    Some(sign * parsed)
}

/// Pull an exit code from a bash result payload that contains an `exitCode`
/// key, regardless of whether the payload is serialized as YAML or JSON.
///
/// The tool bridge serializes tool results as YAML for token efficiency, but
/// downstream tools that bypass it (tests, mocks, other servers) may emit
/// JSON. Both round-trip cleanly through `serde_yml::from_str::<Value>`.
fn parse_exit_code(payload: &str) -> Option<i32> {
    let value = serde_yml::from_str::<serde_json::Value>(payload).ok()?;
    let exit_code = value.get("exitCode")?.as_i64()?;
    i32::try_from(exit_code).ok()
}

impl TranscriptError {
    fn new(transcript: Transcript, error: RunError) -> Self {
        Self { transcript, error: EvalRunError::from(error) }
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn error(&self) -> &EvalRunError {
        &self.error
    }

    pub fn into_parts(self) -> (Transcript, EvalRunError) {
        (self.transcript, self.error)
    }
}

impl Debug for TranscriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TranscriptError").field("error", &self.error).finish_non_exhaustive()
    }
}

pub(crate) fn is_terminal(event: &AgentEvent) -> bool {
    event.turn_outcome().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agent, FakeAgent, Task};
    use aether_core::events::{LlmCallOutcome, TurnEvent, TurnOutcome};
    use llm::testing::session_usage_event;
    use llm::{LlmCallPurpose, TokenUsage, ToolCallRequest, ToolCallResult};
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    #[tokio::test]
    async fn transcript_from_stream() {
        let agent = FakeAgent::with_tool_call("bash", "success");
        let stream = agent.run(Task::new("do the thing"));
        let transcript = Transcript::from_stream(stream).await.unwrap();

        assert!(transcript.tool_called("bash"));
        assert!(matches!(transcript.events().last(), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))));
    }

    #[test]
    fn header_names_the_starting_commit_in_a_git_directory() {
        let repo = init_git_repo();
        let expected_sha = read_head_commit(repo.path());

        let transcript = Transcript::new(vec![]).with_working_dir(repo.path());

        let header = transcript.header().expect("header should be captured when attached");
        assert_eq!(header.working_dir, repo.path());
        assert_eq!(header.starting_commit, StartingCommit::Commit(expected_sha));

        let rendered = header.to_string();
        assert!(rendered.contains("Working directory:"), "rendered header: {rendered}");
        assert!(rendered.contains(repo.path().to_str().unwrap()), "rendered header: {rendered}");
        assert!(rendered.contains("Starting commit:"), "rendered header: {rendered}");
        assert!(!rendered.contains("not a git repository"), "rendered header: {rendered}");
    }

    #[test]
    fn header_says_not_a_git_repository_outside_one() {
        let non_repo = TempDir::new().unwrap();

        let transcript = Transcript::new(vec![]).with_working_dir(non_repo.path());

        let header = transcript.header().expect("header should always be captured when attached");
        assert_eq!(header.working_dir, non_repo.path());
        assert_eq!(header.starting_commit, StartingCommit::NotARepository);

        let rendered = header.to_string();
        assert!(rendered.contains("Working directory:"));
        assert!(rendered.contains(non_repo.path().to_str().unwrap()));
        assert!(rendered.contains("Starting commit: not a git repository"));
    }

    #[test]
    fn transcript_without_a_header_has_none() {
        let transcript = Transcript::new(vec![]);
        assert!(transcript.header().is_none());
    }

    #[test]
    fn tool_call_count_counts_matching_tool_calls() {
        let transcript = transcript_with_events(vec![tool_call("bash"), tool_call("read"), tool_result("bash")]);

        assert!(transcript.tool_called("bash"));
        assert!(!transcript.tool_called("read"));
        assert!(!transcript.tool_called("write"));
        assert_eq!(transcript.tool_call_count("bash"), 1);
        assert_eq!(transcript.tool_call_count("read"), 0);
    }

    #[test]
    fn tool_call_count_includes_refused_calls() {
        let transcript = transcript_with_events(vec![
            tool_call("bash"),
            refused_tool_call("bash", "no shell access"),
            tool_result("bash"),
        ]);

        assert!(transcript.tool_called("bash"));
        assert_eq!(transcript.tool_call_count("bash"), 2);
    }

    #[test]
    fn tool_call_arguments_json_parses_arguments() {
        let call = ToolCall { name: "bash", arguments: r#"{"command":"pwd"}"#, exit_code: None };

        assert_eq!(call.arguments_json().unwrap(), serde_json::json!({ "command": "pwd" }));
    }

    #[test]
    fn tool_call_arguments_json_returns_error_for_invalid_json() {
        let call = ToolCall { name: "bash", arguments: "not json", exit_code: None };

        assert!(call.arguments_json().is_err());
    }

    #[test]
    fn tool_call_records_a_non_zero_shell_exit_code() {
        let transcript = transcript_with_events(vec![bash_tool_result(
            "output: nope\nexitCode: 7\nkilled: false\n",
            Some(r"echo nope 1>&2; exit 7 (exit 7)"),
        )]);

        let bash_call = transcript.tool_calls("bash").next().expect("bash should be called");
        assert_eq!(bash_call.exit_code, Some(7));
    }

    #[test]
    fn tool_call_exit_code_is_none_when_payload_lacks_it() {
        let transcript = transcript_with_events(vec![bash_tool_result("plain text result", None)]);

        let bash_call = transcript.tool_calls("bash").next().expect("bash should be called");
        assert_eq!(bash_call.exit_code, None);
    }

    #[test]
    fn tool_call_exit_code_is_none_when_payload_records_a_failure() {
        let transcript = transcript_with_events(vec![tool_error("bash")]);

        let bash_call = transcript.tool_calls("bash").next().expect("bash should be called");
        assert_eq!(bash_call.exit_code, None);
    }

    #[test]
    fn usage_returns_zeroed_totals_when_no_usage_was_recorded() {
        let transcript = transcript_with_events(vec![tool_call("bash")]);
        assert_eq!(transcript.usage(), SessionUsageTotals::default());
    }

    #[test]
    fn usage_extracts_the_final_session_totals() {
        let mut last = session_usage_event(2, TokenUsage::new(2000, 500));
        last.totals.tokens = TokenUsage::new(3000, 600);
        last.totals.unpriced_calls = 2;
        let transcript = transcript_with_events(vec![
            AgentEvent::SessionUsage(session_usage_event(1, TokenUsage::new(1000, 100))),
            AgentEvent::SessionUsage(last),
        ]);

        let usage = transcript.usage();
        assert_eq!(usage.tokens.input_tokens.get(), 3000);
        assert_eq!(usage.tokens.output_tokens.get(), 600);
        assert_eq!(usage.tokens.total_tokens().get(), 3600);
        assert_eq!(usage.unpriced_calls, 2);
        assert!(!usage.is_fully_priced());
    }

    #[test]
    fn turn_usage_returns_one_entry_per_terminal_turn() {
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_call_ended(TokenUsage::new(10, 2)),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_call_ended(TokenUsage::new(30, 5)),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ];
        let transcript = transcript_with_events(events);

        let turns = transcript.turn_usage();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].outcome, TurnOutcome::Completed);
        assert_eq!(turns[0].usage, TokenUsage::new(10, 2));
        assert_eq!(turns[1].outcome, TurnOutcome::Completed);
        assert_eq!(turns[1].usage, TokenUsage::new(30, 5));
    }

    #[test]
    fn turn_usage_averages_completed_calls_within_a_turn() {
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_call_ended(TokenUsage::new(10, 2)),
            llm_call_ended(TokenUsage::new(20, 3)),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ];
        let transcript = transcript_with_events(events);

        let turns = transcript.turn_usage();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].usage, TokenUsage::new(30, 5));
    }

    #[test]
    fn turn_usage_yields_zeroed_usage_for_a_cancelled_turn() {
        let events = vec![
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_call_ended(TokenUsage::new(10, 2)),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Chat,
                outcome: LlmCallOutcome::Cancelled,
            }),
            AgentEvent::turn_ended(TurnOutcome::Cancelled),
        ];
        let transcript = transcript_with_events(events);

        let turns = transcript.turn_usage();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].usage, TokenUsage::new(10, 2));
        assert_eq!(turns[0].outcome, TurnOutcome::Completed);
        assert_eq!(turns[1].usage, TokenUsage::default());
        assert_eq!(turns[1].outcome, TurnOutcome::Cancelled);
    }

    #[test]
    fn turn_usage_ignores_session_usage_events() {
        let events = vec![
            AgentEvent::SessionUsage(session_usage_event(1, TokenUsage::new(1000, 100))),
            AgentEvent::Turn(TurnEvent::Started { content: vec![] }),
            llm_call_ended(TokenUsage::new(10, 2)),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ];
        let transcript = transcript_with_events(events);

        let turns = transcript.turn_usage();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].usage, TokenUsage::new(10, 2));
    }

    #[test]
    fn turn_usage_is_empty_when_no_turn_completed() {
        let transcript = transcript_with_events(vec![tool_call("bash"), tool_result("bash")]);
        assert!(transcript.turn_usage().is_empty());
    }

    fn llm_call_ended(usage: TokenUsage) -> AgentEvent {
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Completed { stop_reason: None, usage: Some(usage), provider_request_id: None },
        })
    }

    fn transcript_with_events(events: Vec<AgentEvent>) -> Transcript {
        Transcript::new(events)
    }

    fn tool_call(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Call {
            request: ToolCallRequest { id: name.to_string(), name: name.to_string(), arguments: "{}".to_string() },
        })
    }

    fn tool_result(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: name.to_string(),
                name: name.to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
        })
    }

    fn init_git_repo() -> TempDir {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path();
        run_git(path, &["init", "--initial-branch", "main"]);
        std::fs::write(path.join("README.md"), "header test\n").unwrap();
        run_git(path, &["add", "README.md"]);
        run_git(path, &["-c", "user.email=header@example.com", "-c", "user.name=Header", "commit", "-m", "init"]);
        repo
    }

    fn read_head_commit(repo: &Path) -> String {
        let output =
            Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().expect("git rev-parse HEAD");
        assert!(output.status.success(), "git rev-parse HEAD failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(repo).args(args).output().expect("git invocation");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn bash_tool_result(payload: &str, display_value: Option<&str>) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: "call_bash".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: payload.to_string(),
            },
            result_meta: display_value.map(|value| mcp_utils::display_meta::ToolDisplayMeta::new("Ran", value).into()),
        })
    }

    fn tool_error(name: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: format!("{name}_err"),
                name: name.to_string(),
                arguments: Some("{}".to_string()),
                error: "boom".to_string(),
            },
        })
    }

    fn refused_tool_call(name: &str, reason: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Refused {
            request: ToolCallRequest { id: name.to_string(), name: name.to_string(), arguments: "{}".to_string() },
            reason: reason.to_string(),
        })
    }
}
