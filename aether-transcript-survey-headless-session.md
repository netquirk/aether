# TASK-2B-374 — Headless JSONL file (`crates/aether-cli/src/transcript.rs`) and the persisted/reconstructed session event record (`crates/aether-sessions/src/transcript.rs`)

Part 4 of TASK-2B-344 ("Survey the ACP, eval-SDK, TUI and docs meaning of
'Transcript'"). Read-only evidence pass. No source file is changed by this
task; `git diff --stat` against the base commit `b67859b061b8c9320b5953036d563606b5e0725c`
is empty after the commit that adds this report (only this new file is added).

Base commit: `b67859b061b8c9320b5953036d563606b5e0725c
              TASK-2B-373: Survey the TUI renderer module and doc usages`
Read with `git status --porcelain` clean before any change; `git ls-files` then
confirmed every line cited below is unchanged at this commit. The earlier
parts already established the ACP surfaces (TASK-2B-371), the eval-SDK Rust +
TypeScript mirrors (TASK-2B-372), and the TUI/docs citations (TASK-2B-373).

This part pins down the **first** and **second** of the three distinct
artifacts the parent survey names: the **headless JSONL transcript file**
written by `aether headless --transcript-jsonl` (`Path A` below), and the
**persisted/reconstructed session event record** that the ACP actor folds
back into `ChatMessage`s (`Path B` below). The third (the eval SDK's
ordered event list) belongs to TASK-2B-372 and is referenced here only as a
counterpart.

---

## Path A — `crates/aether-cli/src/transcript.rs` (the headless JSONL transcript file)

The artifact is a **JSON Lines file** on disk: one self-describing header
line plus one JSON object per emitted CLI event, written when
`aether headless` is run with `--transcript-jsonl PATH` and read back when
the same run is started with `--resume PATH`. The artifact is the file
written by `JsonlTranscript` and read by `load_resume_messages`; it is a
**file on disk**, distinct from both the ACP actor's in-memory
`Vec<SessionEvent>` replay buffer (Part 1) and the eval SDK's ordered
`Vec<AgentEvent>` (Part 2).

### A.1 — the artifact, as named (line **1**)

Defining line (line **1**), verbatim, on the very first line of the source
file (this is the phrase that literally names the artifact):

```rust
//! JSON Lines transcript writer for the `aether headless` event stream.
```

The doc on lines **3–13** sharpens the contract (headless flag,
`{"type":"header",...}` then one line per CLI-event kinded `AgentEvent`)
and confirms it is the on-disk file (lines **15–17**: "The file is opened
with `File::create`, which truncates any existing file at the path.").

### A.2 — the writer type (line **96**)

`pub struct JsonlTranscript`, line **96**, verbatim:

```rust
pub struct JsonlTranscript {
```

…holding a `BufWriter<File>`, the current 1-based turn number, the file
path, the rotation threshold, the bytes written since the last rotation,
the cached self-describing header line, and a `SecretRedactor` applied to
every serialised line (lines **96–115**).

### A.3 — the constructor that opens the file (lines **137–138**)

Lines **137–138**, verbatim:

```rust
    pub fn create_with_max_bytes(path: &Path, max_bytes: Option<u64>) -> io::Result<Self> {
        let writer = BufWriter::new(File::create(path)?);
```

i.e. the file is opened with `File::create` (truncating any existing file
at the path) and then wrapped in a `BufWriter`. The bare
`pub fn create(path: &Path) -> io::Result<Self>` on line **124** delegates
to this one.

### A.4 — the per-event write (line **203**)

Line **203**, verbatim, with the doc just above it (lines **194–202**) that
states what it does:

```rust
    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {
```

One transcript line per kinded `AgentEvent` (events with no `CliEventKind`
— partial stream fragments and `ToolEvent::CallUpdate` — are skipped,
matching the `--output text` filter, lines **204–206**). The serialised
record carries `turn`, the CLI event kind under `"type"`, and the full
serialised `AgentEvent` under `"event"` (lines **210–212**, with the
`TranscriptRecord` struct shape on lines **58–64**).

### A.5 — the reader used by `--resume` (line **316**)

Line **316**, verbatim, with the doc on lines **297–315** that names the
artifact as a "JSON Lines transcript produced by `JsonlTranscript`":

```rust
pub fn load_resume_messages(path: &Path) -> Result<Vec<ChatMessage>, TranscriptError> {
```

It validates the header line (`{"type":"header", ...}`, lines **323–330**),
walks the per-event lines (`{turn, type, event}`, lines **336–350**), and
folds them back into `Vec<ChatMessage>` via the private `ResumeBuilder`
(lines **371–506**: a state machine that drops trailing partial `User`
prompts and synthesises a fresh empty assistant when a `tool_call` arrives
without a prior `text`).

### A.6 — the run wiring (corroborating, not defining)

The same function names appear at the call site. In `crates/aether-cli/src/headless/run.rs`:

- line **181**: `Some(path) => crate::transcript::load_resume_messages(path)`
  (inside `let resume_messages = match &config.resume { ... }`, the `--resume`
  flag wiring on lines **180–184**).
- line **234**: `let mut writer = JsonlTranscript::create_with_max_bytes(path, config.transcript_max_bytes)`
  (inside `let mut transcript = match &config.transcript_jsonl { Some(path) => ... }`,
  the `--transcript-jsonl` flag wiring on lines **227–239**).
- line **237**: `writer.write_header(crate::version::aether_version(), &started_at)`
  (immediately after the constructor, the self-describing header is the
  first line written).

### A.7 — the executable statement of the artifact's shape (integration test)

`crates/aether-cli/tests/integration/transcript_jsonl.rs` exercises the
writer against a real `agent(provider).spawn()` run and reads the file back
through `parse_transcript`, which proves the on-the-wire shape (`turn`,
`type`, `event` is a JSON object) the writer actually emits.

- line **30**: `fn parse_transcript(path: &std::path::Path) -> Vec<(u64, String, serde_json::Value)> {`
  — the helper that reads back the file as `(turn, kind, event_obj)` tuples
  (lines **30–48**); the assertions inside (`value.is_object()`,
  `value["turn"].as_u64()`, `value["event"].as_object()`, etc.) pin the
  contract.
- line **63**: `    let path = dir.path().join("transcript.jsonl");`
  — the on-disk path is literally `transcript.jsonl`, matching the
  rotation contract documented at line **237–242** (`<stem>.1` for the
  rotated sibling).
- line **65**: `    let mut transcript = JsonlTranscript::create(&path).expect("create writer");`
  — the writer opened against that path; the `records one JSON object per
  kinded event` test on lines **51–109** asserts one line per `AgentEvent`
  filtered through `has_cli_event_kind` (the same predicate the writer
  uses), with every line carrying the expected `turn` and the nested
  event's `category` + `event` tags preserved (lines **99–108**).

The `transcript_jsonl_rotates_when_max_bytes_is_reached` integration test
on lines **111–162** proves the rotation contract end-to-end
(`transcript.1` sibling must exist after the threshold is reached) and the
`transcript_jsonl_lines_round_trip_through_serde_json` test on lines
**192–229** proves the inner `event` value is exactly the public
`AgentEvent` schema (re-serialising after `serde_json::from_value` must
yield an equal `serde_json::Value`).

---

## Path B — `crates/aether-sessions/src/transcript.rs` (the persisted/reconstructed session event record)

The artifact is the **ordered `Vec<SessionEvent>` session record** — what
the session store writes to disk (one JSONL line per event appended
through `SessionStore::append_event`) and reads back
(`SessionStore::load` returns `(SessionMeta, Vec<SessionEvent>)`) — here
reconstructed by `aether-sessions::transcript` into a turn-indexed view or
into `Vec<ChatMessage>`s the ACP actor seeds into a new run. This is the
**second** distinct artifact: a persisted/replayed record of events (not
an on-disk transcript file, not an eval-SDK ordered event list).

### B.1 — `conversation_messages_from_events` (line **94**, the defining line)

Line **94**, verbatim (this is the function the task names by example):

```rust
pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
```

Body, line **95** (verbatim, with the `SessionEvent → Context → ChatMessage`
flow):

```rust
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
```

— i.e. fold the persisted events through `context_from_events`
(line **77**), take its `Context.messages()`, drop the system messages,
clone the rest. The `SessionEvent` element type is the
discriminated enum from `crates/aether-sessions/src/model.rs:38–42`:

```rust
pub enum SessionEvent {
    User(UserEvent),
    Agent(AgentEvent),
    Control(SessionControlEvent),
}
```

### B.2 — the turn-by-turn index (line **41** with doc on line **33**)

Lines **33–41**, verbatim, the next surface of the same artifact:

```rust
/// Builds a turn-by-turn transcript view from a run's persisted events.
///
/// A turn is the run between two terminal [`TurnEvent::Ended`] events
/// (or from the start of the stream to the first `Ended`). Turns that never
/// reach an `Ended` event are dropped, because their outcome is unknown. The
/// recorded model is the first chat-purpose [`TurnEvent::LlmCallStarted`]
/// in the turn; a turn with no chat call (or none that survived
/// persistence) records `None` for its model fields.
pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {
```

The `TurnEntry` row type is declared at line **20** (verbatim):

```rust
pub struct TurnEntry {
```

…with fields `turn_index: usize`, `outcome: TurnOutcome`,
`provider: Option<String>`, `model_id: Option<String>`,
`display_name: Option<String>` (lines **20–31**, all `pub` and
`#[serde(rename_all = "camelCase")]`).

### B.3 — the shared event folder (line **77**)

Line **77**, verbatim, the function both `conversation_messages_from_events`
and `turn_entries_from_events`'s `Context`-building path share:

```rust
pub fn context_from_events(events: &[SessionEvent]) -> Context {
```

It folds `SessionEvent::User(_)` through `apply_user_event` (lines
**82–85**, **120–127**) and `SessionEvent::Agent(_)` through
`apply_agent_event` (line **86** → lines **129–183**, the 17-arm match
that handles `Text`, `Thought`, `Tool::Call`, `Tool::Result`,
`Tool::TaskCreated`, `Tool::Error`, `Tool::Refused`, `Turn::AutoContinue`,
`Turn::Ended`, `Context::Cleared`, `Context::CompactionResult`, and the
`TaskCompleted`/`TaskFailed`/`TaskCancelled` triplet via
`task_context_message`); `SessionEvent::Control(_)` is intentionally
ignored (line **87**). The shared `MessageAccumulator` (lines **98–118**)
threads in-progress `Assistant` text + reasoning + accumulated `ToolCallResult`s
between arms so the resulting `Context` has well-formed `Assistant` turns.

### B.4 — corroborating: the ACP actor imports and uses it as the authoritative event record

This proves the `Vec<SessionEvent>` here is the *persisted event record*,
not a UI string list:

- `crates/aether-cli/src/acp/session/actor.rs` line **9** (verbatim):

  ```rust
  use aether_sessions::transcript::conversation_messages_from_events;
  ```

- `crates/aether-cli/src/acp/session/actor.rs` — three call sites,
  every one of which passes the actor's own `transcript: Vec<SessionEvent>`
  (the same field named in Part 1 at lines **174, 195**) into the
  reconstruction function:

  - line **362**: `let messages = conversation_messages_from_events(&self.transcript);`
    (inside `select_agent` — switching the active agent seeds the new
    runtime from the persisted record)
  - line **373**: `let messages = conversation_messages_from_events(&self.transcript);`
    (inside `sync_active_conversation` — same shape, replacing the active
    runtime's conversation)
  - line **386**: `let messages = conversation_messages_from_events(&self.transcript);`
    (inside `ensure_active_running` — first launch of the active runtime
    against the persisted record)

- The re-export at `crates/aether-sessions/src/lib.rs:20` (verbatim)
  names the artifact at the crate level:

  ```rust
  pub use transcript::{TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events};
  ```

### B.5 — corroborating: this is the persisted event record, written + loaded by `SessionStore`

The `Vec<SessionEvent>` consumed by `transcript.rs` comes from one source:
the session store. Two lines in `crates/aether-sessions/src/store/mod.rs`
make that explicit:

- line **101** (verbatim, the writer):

  ```rust
  pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> Result<(), SessionStoreError> {
  ```

  Each call appends one JSONL line to the per-session `<session_id>.jsonl`
  file via `append_line` (line **185**, which `fsync`s each line per the
  doc on lines **191–193** so a mid-turn crash cannot lose earlier turns),
  guarded by `event.is_persisted()` (line **102**) to drop the in-memory
  variants that should never reach disk.

- line **115** (verbatim, the reader):

  ```rust
  pub fn load(&self, session_id: &str) -> Result<(SessionMeta, Vec<SessionEvent>), SessionStoreError> {
  ```

  Returns `(SessionMeta, Vec<SessionEvent>)` from a bounded scan of the
  same `<session_id>.jsonl` file. The `Vec<SessionEvent>` is the
  authoritative persisted record; `transcript.rs`'s
  `conversation_messages_from_events` / `turn_entries_from_events` /
  `context_from_events` then fold it into other views.

So Path B is structurally a **persisted/reconstructed session event
record**: persist via `SessionStore::append_event` →
`SessionStore::load → Vec<SessionEvent>` → `aether_sessions::transcript`'s
folders → `Vec<ChatMessage>` (imported by the ACP actor) or
`Vec<TurnEntry>` or `Context`. Not an on-disk transcript file; not the
eval SDK's ordered `Vec<AgentEvent>` (that one wraps an `AgentEvent`
stream rather than a `SessionEvent` log).

---

## Characterisation (one sentence)

Here "transcript" denotes, **first**, a **file on disk** — the JSON Lines
artifact that `crates/aether-cli/src/transcript.rs` writes through
`JsonlTranscript::{create,create_with_max_bytes,record,write_header}` and
that `load_resume_messages` reads back into a `Vec<ChatMessage>`,
exercised end-to-end by `crates/aether-cli/tests/integration/transcript_jsonl.rs`'s
four async tests; and, **second**, the **persisted/reconstructed session
event record** — the `Vec<SessionEvent>` that
`crates/aether-sessions/src/store/mod.rs::append_event` persists and
`SessionStore::load` reconstructs, then folded by
`crates/aether-sessions/src/transcript.rs`'s `conversation_messages_from_events`,
`turn_entries_from_events`, and `context_from_events` into the views the
ACP actor seeds into a new run — the first and second of the three
distinct artifacts in the parent survey; the third (the eval SDK's
ordered `Vec<AgentEvent>` list) is the subject of TASK-2B-372.

---

## Proof that the cited lines exist at the base commit

```
$ grep -n 'JSON Lines transcript writer' crates/aether-cli/src/transcript.rs
1://! JSON Lines transcript writer for the `aether headless` event stream.

$ grep -n 'pub struct JsonlTranscript' crates/aether-cli/src/transcript.rs
96:pub struct JsonlTranscript {

$ grep -nE 'pub fn create_with_max_bytes\(|let writer = BufWriter::new' crates/aether-cli/src/transcript.rs
137:    pub fn create_with_max_bytes(path: &Path, max_bytes: Option<u64>) -> io::Result<Self> {
138:        let writer = BufWriter::new(File::create(path)?);

$ grep -n 'pub fn record' crates/aether-cli/src/transcript.rs
203:    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {

$ grep -n 'pub fn load_resume_messages' crates/aether-cli/src/transcript.rs
316:pub fn load_resume_messages(path: &Path) -> Result<Vec<ChatMessage>, TranscriptError> {

$ grep -n 'fn parse_transcript' crates/aether-cli/tests/integration/transcript_jsonl.rs
30:fn parse_transcript(path: &std::path::Path) -> Vec<(u64, String, serde_json::Value)> {

$ grep -nE 'dir\.path\(\)\.join\("transcript\.jsonl"\)|JsonlTranscript::create\(&path\)' crates/aether-cli/tests/integration/transcript_jsonl.rs
63:    let path = dir.path().join("transcript.jsonl");
65:    let mut transcript = JsonlTranscript::create(&path).expect("create writer");

$ grep -nE 'crate::transcript::load_resume_messages|JsonlTranscript::create_with_max_bytes|writer\.write_header\(crate::version::aether_version' crates/aether-cli/src/headless/run.rs
181:        Some(path) => crate::transcript::load_resume_messages(path)
234:            let mut writer = JsonlTranscript::create_with_max_bytes(path, config.transcript_max_bytes)
237:            writer.write_header(crate::version::aether_version(), &started_at).map_err(CliError::IoError)?;

$ grep -n 'pub struct TurnEntry' crates/aether-sessions/src/transcript.rs
20:pub struct TurnEntry {

$ grep -n 'turn-by-turn transcript view' crates/aether-sessions/src/transcript.rs
33:/// Builds a turn-by-turn transcript view from a run's persisted events.

$ grep -n 'pub fn turn_entries_from_events' crates/aether-sessions/src/transcript.rs
41:pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {

$ grep -n 'pub fn context_from_events' crates/aether-sessions/src/transcript.rs
77:pub fn context_from_events(events: &[SessionEvent]) -> Context {

$ grep -n 'pub fn conversation_messages_from_events' crates/aether-sessions/src/transcript.rs
94:pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {

$ grep -n 'pub use transcript' crates/aether-sessions/src/lib.rs
20:pub use transcript::{TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events};

$ grep -n 'use aether_sessions::transcript::conversation_messages_from_events;' crates/aether-cli/src/acp/session/actor.rs
9:use aether_sessions::transcript::conversation_messages_from_events;

$ grep -n 'conversation_messages_from_events(&self.transcript)' crates/aether-cli/src/acp/session/actor.rs
362:        let messages = conversation_messages_from_events(&self.transcript);
373:        let messages = conversation_messages_from_events(&self.transcript);
386:        let messages = conversation_messages_from_events(&self.transcript);

$ grep -nE 'pub fn append_event\(|pub fn load\(&self' crates/aether-sessions/src/store/mod.rs
101:    pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> Result<(), SessionStoreError> {
115:    pub fn load(&self, session_id: &str) -> Result<(SessionMeta, Vec<SessionEvent>), SessionStoreError> {

$ grep -n 'pub enum SessionEvent' crates/aether-sessions/src/model.rs
38:pub enum SessionEvent {
```

`git status --porcelain` after this report is committed prints only
`aether-transcript-survey-headless-session.md` (the new file this task
adds); no source file of the project was edited. `git show --stat HEAD`
after the commit lists exactly one added file.
