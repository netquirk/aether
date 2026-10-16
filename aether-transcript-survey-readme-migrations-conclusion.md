# TASK-2B-375 — README/migrations negatives and the survey conclusion

Part 5 of 5 of TASK-2B-344 ("Survey the ACP, eval-SDK, TUI and docs meaning of
'Transcript'"). Read-only evidence pass — no source file is edited, and only
this one file is added. The four earlier parts of the survey (TASK-2B-371,
TASK-2B-372, TASK-2B-373, TASK-2B-374) already pinned the ACP, eval-SDK,
TUI/docs, and headless+session definitions; this part records the two
explicit negatives (`README.md` and `crates/aether-sessions/migrations/*.sql`
contain no case-insensitive match for `transcript`) and states the survey's
verdict.

**Base commit:** `94c0739d4825f333027aaf4f88f78e4ea9df8786`
(`TASK-2B-374: Establish the headless JSONL and persisted…`).

`git status --porcelain` was clean before this change; no source file is
edited; only `aether-transcript-survey-readme-migrations-conclusion.md` is
added by this task.

---

## Negative A — `README.md` (root)

**Statement:** `README.md` (root) contains no case-insensitive match for
`transcript` — zero matches.

**Command + output (empty, exit 1):**

```
$ git grep -n -i transcript -- README.md
$ echo "exit=$?"
exit=1
```

`git grep` with no matches prints nothing to stdout and exits with status 1;
no line of `README.md` carries the word `transcript` (in any letter case).

---

## Negative B — `crates/aether-sessions/migrations/*.sql`

The glob `crates/aether-sessions/migrations/*.sql` covers exactly two files
(the directory listing is shown below), both of them already cited as
binding schema sources elsewhere in the project brief.

```
$ ls crates/aether-sessions/migrations/*.sql
crates/aether-sessions/migrations/001_session_index.sql
crates/aether-sessions/migrations/002_session_usage.sql
```

**Statement:** `crates/aether-sessions/migrations/001_session_index.sql`
and `002_session_usage.sql` contain no case-insensitive match for
`transcript` — zero matches in the migrations directory.

**Command + output (empty, exit 1):**

```
$ git grep -n -i transcript -- 'crates/aether-sessions/migrations/*.sql'
$ echo "exit=$?"
exit=1
```

Neither of the two migrations (`001_session_index.sql`, the session/events
schema; `002_session_usage.sql`, the usage schema) names the word
`transcript` in any letter case. The Glossary entries that *do* mention the
`events` / `tool_call` / `tool_result` / `context_usage` tables all use
those names, not "transcript".

**Cross-check (system grep, recursive, case-insensitive, both roots):**

```
$ grep -rin transcript README.md crates/aether-sessions/migrations/
$ echo "exit=$?"
exit=1
```

— also empty, also exit 1; confirms the two `git grep` negatives above.

---

## Every earlier location, with path and quote

Consolidated index of the citations from the four earlier survey reports
(TASK-2B-371/372/373/374). Each line below is reproduced with its full path
and a verbatim quote. The four source files are left byte-identical at
`94c0739d`; nothing here paraphrases or rewrites them.

### Part 1 (TASK-2B-371) — `aether-transcript-survey-acp.md`

- `crates/acp-utils/src/notifications.rs:108` →
  `    pub transcript: Vec<SessionPreviewTurn>,`
  (inside `pub struct SessionPreviewResponse`, the bounded
  `role`/`text` preview projection; `SessionPreviewTurn` is the
  `User`/`Assistant` enum shape on lines 113–125 of the same file).

- `crates/aether-cli/src/acp/session/actor.rs:174` →
  `    pub transcript: Vec<SessionEvent>,`
  (inside `pub(crate) struct SessionActorInit`; the public init field).

- `crates/aether-cli/src/acp/session/actor.rs:195` →
  `    transcript: Vec<SessionEvent>,`
  (inside `pub(crate) struct SessionActor`; the in-memory session event log
  replayed to the editor client).

- `crates/aether-cli/src/acp/session/actor.rs:407` →
  `        self.transcript.push(event);`
  (the only mutator of the field, inside `SessionActor::record_event` at
  lines 406–408).

- `crates/aether-cli/src/acp/session/actor.rs:257` →
  `            replay_to_client(&actor.transcript, &actor.io);`
  (spawn-time replay, guarded by `if init.replay {` on line 256).

- `crates/aether-cli/src/acp/session/actor.rs:448` →
  `                    replay_to_client(&self.transcript, &self.io);`
  (late-attach replay inside `SessionCommand::Attach { ... }`, guarded by
  `if replay {` on line 447).

- `crates/aether-cli/src/acp/protocol/replay.rs:10` →
  `pub(crate) fn replay_to_client(events: &[SessionEvent], io: &SessionIo) {`
  (the re-emitter that walks the `&[SessionEvent]` slice and sends the
  missed `SessionUpdate`s over the wire).

### Part 2 (TASK-2B-372) — `aether-transcript-survey-eval-sdk.md`

- `crates/aether-evals/src/agents/transcript.rs:23` →
  `pub struct TranscriptHeader {`
  (body on lines 24–25: `pub working_dir: PathBuf,` and
  `pub starting_commit: StartingCommit,`; the doc on lines 19–21 explicitly
  says "A header is captured once at the beginning of the run and stored
  on the [`Transcript`] alongside the streamed `AgentEvent`s.").

- `crates/aether-evals/src/agents/transcript.rs:57` →
  `pub struct Transcript {`
  (with private `events: Vec<AgentEvent>` on line **58** and
  `header: Option<TranscriptHeader>` on line **59**).

- `crates/aether-evals/src/agents/transcript.rs:58` →
  `    events: Vec<AgentEvent>,`

- `crates/aether-evals/src/agents/transcript.rs:59` →
  `    header: Option<TranscriptHeader>,`

- `crates/aether-evals/src/agents/transcript.rs:115` →
  `    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {`
  (preceded by `#[allow(clippy::result_large_err)]` on line 114; body on
  lines 116–127 drains the stream via `transcript.add(event)` and returns
  `TranscriptError` carrying the partial transcript on first error).

- `packages/aether-evals/src/transcript.ts:39` →
  `export class Transcript {`
  (the TypeScript mirror of the Rust `Transcript`).

- `packages/aether-evals/src/transcript.ts:40` →
  `  readonly events: AgentEvent[];`
  (defensively copied from the constructor argument on lines 42–44).

- `packages/aether-evals/src/transcript.ts:46` →
  `  static async fromStream(`
  (signature on lines 46–48:
  `  static async fromStream(` / `    stream: AsyncIterable<AgentEvent>,` /
  `  ): Promise<Transcript> {`; body on lines 49–58 drains the iterable via
  `transcript.add(event)` and throws a `TranscriptError` carrying the
  partial transcript on the first thrown error).

### Part 3 (TASK-2B-373) — `aether-transcript-survey-tui-docs.md`

- `crates/wisp/src/renderer/transcript.rs` — **explicit negative**: no
  `transcript` literal anywhere in the file. The 130-line `impl Renderer`
  block (lines 15–129) contains five methods
  (`live_lines` at line 18, `lines` at line 26, `live_lines_inner` at line
  43, `item_suffix` at line 73, `cached_item_lines` at line 101) and zero
  identifier/string/comment containing the word. Corroborated by
  `git grep -n -i transcript -- crates/wisp/src/renderer/transcript.rs`
  (empty, exit 1). The only prose that names what the module is lives in
  the parent module, at `crates/wisp/src/renderer/mod.rs:9`:
  `//! - `transcript` turns conversation items into rows, through the `cache` of`
  (with the continuation `//!   sealed items and the `streaming` incremental renderer;`
  on lines 10–11; the child-module declaration `mod transcript;` is on
  line 22). Because the file itself emits no string and no identifier that
  contains the word, it is *not a definition cite* for "Transcript" — only
  a structural mention via the module path.

- `packages/website/src/content/docs/aether/running/headless.mdx:87` →
  `    Human-readable lines. Final text is printed as plain text; tool calls, tool results, errors, context usage, and other selected events are formatted as simple lines. Background MCP tasks get their own lines as they start and finish (`Tool deferred ...`, `Background task completed ...`, `Background task failed ...`, `Background task cancelled ...`). At the end of a run the transcript ends with `Done` (or `Cancelled` / `Error: ...`) followed by a `Files changed: N (X created, Y modified, Z deleted)` line that tallies every `FileDiff` the run produced through the coding tools (`write_file`, `edit_file`, and any future tool that emits a `FileDiff`). A read-only run reports `Files changed: 0 (0 created, 0 modified, 0 deleted)`.
  (key clause: *"At the end of a run the transcript ends with `Done` (or
  `Cancelled` / `Error: ...`) followed by a `Files changed: N (X created,
  Y modified, Z deleted)` line …"* — "the transcript" = the headless
  CLI's text-mode output stream).

- `packages/website/src/content/docs/aether/running/remote.mdx:75` →
  `An unexpected connection loss restores the terminal and exits with an error. There is **no automatic reconnect or prompt retry**. If a prompt's acceptance response was lost, reconnect and inspect the transcript before resubmitting: a retry could execute tools twice.`
  (key clause: *"reconnect and inspect the transcript before resubmitting:
  a retry could execute tools twice."* — "the transcript" = the
  conversation history the (re)attached remote client can inspect, the
  same `Vec<SessionEvent>` replayed by the ACP actor in Part 1).

### Part 4 (TASK-2B-374) — `aether-transcript-survey-headless-session.md`

- `crates/aether-cli/src/transcript.rs:1` →
  `//! JSON Lines transcript writer for the `aether headless` event stream.`
  (the phrase that literally names the on-disk artifact; the doc on lines
  3–13 sharpens the contract to a self-describing header line plus one
  JSON object per `AgentEvent` with a `CliEventKind`).

- `crates/aether-cli/src/transcript.rs:96` →
  `pub struct JsonlTranscript {`
  (the writer type, holding a `BufWriter<File>`, the current 1-based turn
  number, the file path, the rotation threshold, the bytes written since
  the last rotation, the cached self-describing header line, and a
  `SecretRedactor`).

- `crates/aether-cli/src/transcript.rs:137` →
  `    pub fn create_with_max_bytes(path: &Path, max_bytes: Option<u64>) -> io::Result<Self> {`
  (the constructor that opens the file with `File::create`; line 138
  inside: `        let writer = BufWriter::new(File::create(path)?);`).

- `crates/aether-cli/src/transcript.rs:203` →
  `    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {`
  (the per-event write; one transcript line per kinded `AgentEvent`,
  filtered through `has_cli_event_kind`; the serialised record carries
  `turn`, the CLI event kind under `"type"`, and the full serialised
  `AgentEvent` under `"event"`).

- `crates/aether-cli/src/transcript.rs:316` →
  `pub fn load_resume_messages(path: &Path) -> Result<Vec<ChatMessage>, TranscriptError> {`
  (the reader used by `--resume`; validates the `{"type":"header", ...}`
  line, walks the per-event `{turn, type, event}` lines, and folds them
  back into `Vec<ChatMessage>`).

- `crates/aether-sessions/src/transcript.rs:20` →
  `pub struct TurnEntry {`
  (the per-turn row type, with `turn_index: usize`, `outcome: TurnOutcome`,
  `provider: Option<String>`, `model_id: Option<String>`,
  `display_name: Option<String>`, lines 20–31).

- `crates/aether-sessions/src/transcript.rs:33` →
  `/// Builds a turn-by-turn transcript view from a run's persisted events.`
  (the doc comment immediately above `turn_entries_from_events`).

- `crates/aether-sessions/src/transcript.rs:41` →
  `pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {`
  (turn-by-turn index over the persisted `Vec<SessionEvent>`).

- `crates/aether-sessions/src/transcript.rs:77` →
  `pub fn context_from_events(events: &[SessionEvent]) -> Context {`
  (the shared `SessionEvent → Context` folder, used by both
  `turn_entries_from_events` and `conversation_messages_from_events`).

- `crates/aether-sessions/src/transcript.rs:94` →
  `pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {`
  (the function the ACP actor imports — line 9 of `actor.rs` — to seed
  `select_agent` / `sync_active_conversation` / `ensure_active_running`
  on lines 362, 373, 386 from the actor's own `transcript: Vec<SessionEvent>`).

- `crates/aether-sessions/src/lib.rs:20` →
  `pub use transcript::{TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events};`
  (the crate-level re-export that names the artifact from the crate root).

- `crates/aether-sessions/src/store/mod.rs:101` →
  `    pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> Result<(), SessionStoreError> {`
  (the writer: appends one JSONL line per `SessionEvent` to the per-session
  `<session_id>.jsonl` file, guarded by `event.is_persisted()` on line 102).

- `crates/aether-sessions/src/store/mod.rs:115` →
  `    pub fn load(&self, session_id: &str) -> Result<(SessionMeta, Vec<SessionEvent>), SessionStoreError> {`
  (the reader: returns `(SessionMeta, Vec<SessionEvent>)` from a bounded
  scan of the same `<session_id>.jsonl` file).

- `crates/aether-sessions/src/model.rs:38` →
  `pub enum SessionEvent {`
  (the discriminated enum `SessionEvent { User(UserEvent), Agent(AgentEvent),
  Control(SessionControlEvent) }` on lines 38–42 — the element type the
  whole persisted record carries).

---

## Conclusion

Verbatim (the survey's three-artifact statement, as the parent task's
acceptance criterion requires):

> 'Transcript' denotes at least three distinct artifacts - the headless JSONL file, the persisted/replayed session event record, and the eval SDK's ordered event list - and none of these is defined in README.md or the migrations.

The same claim in the parent criterion's phrasing:

> the word denotes at least three distinct artifacts — the headless JSONL file, the persisted/replayed session event record, and the eval SDK's ordered event list — none of which is defined in README.md or the migrations.

Mapping each artifact to its defining path:

- **The headless JSONL file** → `crates/aether-cli/src/transcript.rs`
  (the on-disk artifact written by `JsonlTranscript::{create,
  create_with_max_bytes, record, write_header}` at lines 96 / 137 / 203
  and read back by `load_resume_messages` at line 316; the artifact is
  literally named on line 1: `//! JSON Lines transcript writer for the
  `aether headless` event stream.`). This is the file `aether headless
  --transcript-jsonl PATH` writes and `aether headless --resume PATH`
  re-reads. It is a *file format on disk*, exercised end-to-end by
  `crates/aether-cli/tests/integration/transcript_jsonl.rs`.
- **The persisted/replayed session event record** → the pair
  `crates/aether-sessions/src/transcript.rs` (the folder
  `turn_entries_from_events` at line 41, `context_from_events` at
  line 77, `conversation_messages_from_events` at line 94) +
  `crates/aether-cli/src/acp/session/actor.rs` (the in-memory
  `transcript: Vec<SessionEvent>` at lines 174 / 195, mutated by
  `self.transcript.push(event)` at line 407, replayed through
  `replay_to_client(&actor.transcript, &actor.io)` at line 257 and
  `replay_to_client(&self.transcript, &self.io)` at line 448). The
  underlying `SessionEvent` is the discriminated enum at
  `crates/aether-sessions/src/model.rs:38`; it is persisted by
  `SessionStore::append_event` (`crates/aether-sessions/src/store/mod.rs:101`)
  and re-read by `SessionStore::load`
  (`crates/aether-sessions/src/store/mod.rs:115`).
- **The eval SDK's ordered event list** → the pair
  `crates/aether-evals/src/agents/transcript.rs` (the `Transcript`
  struct at line 57 with `events: Vec<AgentEvent>` at line 58 and
  `header: Option<TranscriptHeader>` at line 59; built by
  `Transcript::from_stream` at line 115) +
  `packages/aether-evals/src/transcript.ts` (the `Transcript` class
  at line 39 with `readonly events: AgentEvent[];` at line 40; built
  by `Transcript.fromStream` at line 46). This is the eval SDK's
  in-memory ordered event list — not a file format, not a session
  event log.

None of those three artifacts is named in `README.md` (Negative A) or in
`crates/aether-sessions/migrations/*.sql` (Negative B); the brief's
`## Glossary` entry for `Transcript` is left reading `unknown` (its
correction is a separate task), and the two schema files instead name
`events`, `tool_call`, `tool_result`, `context_usage`, `provider`, etc.

---

## Proof

The two negative greps and the cross-check, with their empty outputs and
`exit=1`:

```
$ git grep -n -i transcript -- README.md; echo "exit=$?"
exit=1

$ git grep -n -i transcript -- 'crates/aether-sessions/migrations/*.sql'; echo "exit=$?"
exit=1

$ grep -rin transcript README.md crates/aether-sessions/migrations/; echo "exit=$?"
exit=1
```

(`grep -rin` walks both roots recursively; the `migrations/` glob is
implicit since the directory has only `001_session_index.sql` and
`002_session_usage.sql` — verified by `ls crates/aether-sessions/migrations/*.sql`
above.)

`git status --porcelain` after this report is committed prints only
`aether-transcript-survey-readme-migrations-conclusion.md` (the new file
this task adds); no source file of the project was edited. The four
earlier survey files (`aether-transcript-survey-acp.md`,
`aether-transcript-survey-eval-sdk.md`,
`aether-transcript-survey-tui-docs.md`,
`aether-transcript-survey-headless-session.md`) are byte-identical at
`94c0739d` and remain so.
