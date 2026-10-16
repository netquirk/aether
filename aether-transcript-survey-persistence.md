# TASK-2B-397 — Persistence-side `Transcript` artifact inventory in `crates/aether-sessions/src/transcript.rs`

Read-only evidence pass. **No source file is edited** — only this one new
markdown file is added by the task, and the working tree is otherwise
byte-identical to the base commit. Verified before writing:

```
$ git rev-parse HEAD
0fa5d92d763a7cd58c567e08e315f42a6547e770      # TASK-2B-396 (Part 1 of 4 of TASK-2B-343)
$ git status --porcelain
                                                # clean
```

`crates/aether-sessions/src/transcript.rs` was read end-to-end with
`read_file` (184 lines, all shown in the receipt). `crates/aether-sessions/src/lib.rs`
was read through line 20 (the file is exactly 20 lines long; the receipt
shows the whole file). Every line number below was re-derived with `sed`
against the base commit, not trusted from the task body.

The task's two asks are both answered here: a four-row artifact table
giving path, 1-based line number, and the ONE verbatim defining line
(declaration/signature line, not the doc block) for each of the four
items, and one explicit two-meanings collision sentence below the table.

---

## 1. Artifacts in `crates/aether-sessions/src/transcript.rs`

For every artifact the task body names, the table records its exact file
path, 1-based line number, and the ONE verbatim line that defines it.
The "defining line" is the struct or fn declaration/signature line (the
one with the `pub` keyword and the opening `{`); the doc comment
immediately above each declaration states the surrounding contract and is
omitted from the table on purpose, because the task explicitly asked for
the *one* defining line.

| # | Artifact | Path | Line | One verbatim defining line |
|---|----------|------|------|----------------------------|
| 1 | `TurnEntry` (one turn in a run transcript, with the model that served it) | `crates/aether-sessions/src/transcript.rs` | 20 | `pub struct TurnEntry {` |
| 2 | `turn_entries_from_events` (turn-by-turn view; drops turns with no `TurnEvent::Ended`) | `crates/aether-sessions/src/transcript.rs` | 41 | `pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {` |
| 3 | `context_from_events` (folds the event stream into an `llm::Context`) | `crates/aether-sessions/src/transcript.rs` | 77 | `pub fn context_from_events(events: &[SessionEvent]) -> Context {` |
| 4 | `conversation_messages_from_events` (non-system `ChatMessage`s from that context) | `crates/aether-sessions/src/transcript.rs` | 94 | `pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {` |

The task also names the re-export at `crates/aether-sessions/src/lib.rs`
line 20; recorded for completeness:

| # | Artifact | Path | Line | One verbatim defining line |
|---|----------|------|------|----------------------------|
| 5 | Re-export of the four items from the crate root | `crates/aether-sessions/src/lib.rs` | 20 | `pub use transcript::{TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events};` |

Reproduce the columns (line numbers confirmed against the source at
base commit `0fa5d92d`):

```
$ git ls-files | grep -E 'crates/aether-sessions/src/(transcript|lib)\.rs$'
crates/aether-sessions/src/lib.rs
crates/aether-sessions/src/transcript.rs
$ sed -n '20p;41p;77p;94p' crates/aether-sessions/src/transcript.rs
pub struct TurnEntry {
pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {
pub fn context_from_events(events: &[SessionEvent]) -> Context {
pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
$ sed -n '20p' crates/aether-sessions/src/lib.rs
pub use transcript::{TurnEntry, context_from_events, conversation_messages_from_events, turn_entries_from_events};
```

(The four `sed` lines reproduce the four "One verbatim defining line"
cells for rows 1–4, and the single `sed` reproduces row 5. The two files
are 184 and 20 lines long respectively at this base commit.)

---

## 2. The two-meanings collision (one sentence)

This `transcript` in `crates/aether-sessions` is an **in-memory view
rebuilt on demand from the persisted `SessionEvent` stream** (a
`Vec<SessionEvent>` folded by the three functions above), which is a
**different thing** from the CLI's on-disk JSONL file written by
`JsonlTranscript` in `crates/aether-cli/src/transcript.rs` and read back
by `load_resume_messages` for `aether headless --resume PATH`.

Corroboration that the persistence-side "transcript" really is the
replayed `Vec<SessionEvent>` and nothing else, taken from
`crates/aether-cli/src/acp/session/actor.rs` (a separate file, not the
focus of this part, but the only place outside `aether-sessions` that
holds one as a field):

- Field declaration: `    transcript: Vec<SessionEvent>,` at line 195
  (with a matching public field on the public config struct at line 174
  of the same file).
- Call sites that fold the field through the persistence-side functions:
  `let messages = conversation_messages_from_events(&self.transcript);`
  at lines 362, 373, and 386.

So the same word "transcript" in this crate means *the replayed
`SessionEvent` stream*, not a serialized file; the brief's `##
Glossary` entry, currently `unknown`, is the place to pick one.

---

## 3. Method note

- `read_file` was used to consume `crates/aether-sessions/src/transcript.rs`
  in one call (the receipt shows all 184 lines, with no
  truncation/offset) and `crates/aether-sessions/src/lib.rs` through
  line 20.
- `sed -n '20p;41p;77p;94p' …` and `sed -n '20p' …` were used to
  re-derive the quoted line numbers from the source, in case the task
  body's line numbers were stale; the four declaration lines and the
  re-export line are byte-identical to the table above, confirming the
  task body's line numbers are current at base commit `0fa5d92d`.
- The corroborating `Vec<SessionEvent>` field and the three
  `conversation_messages_from_events(&self.transcript)` call sites in
  `crates/aether-cli/src/acp/session/actor.rs` are the only out-of-file
  evidence used; everything else in this report comes from
  `crates/aether-sessions/src/transcript.rs` and `crates/aether-sessions/src/lib.rs`.
- No `prairie` command was run during this part.
