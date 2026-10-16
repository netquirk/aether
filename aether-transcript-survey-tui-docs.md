# TASK-2B-373 — TUI renderer module + docs usages: a `transcript` module with **no `transcript` literal** in it, and two docs that call the emitted event lines "the transcript"

Part 3 of TASK-2B-344 ("Survey the ACP, eval-SDK, TUI and docs meaning of 'Transcript'").
Read-only evidence pass. No source file is changed by this task; `git diff --stat`
against the base commit `3793275d6e9bf7443aea2ed68547f76843988cb4` is empty after
the commit that adds this report (only this new file is added).

Base commit: `3793275d6e9bf7443aea2ed68547f76843988cb4
              TASK-2B-372: Survey the eval SDK Transcript definitions`
Read with `git status --porcelain` clean before any change; the renderer module
and the two docs are read straight from the working tree at this commit.

The deliverable is exactly one negative (Path A) and two doc citations (Paths B
and C): the wisp `transcript` renderer module contains **no `transcript`
literal at all**, and only the two running docs use the phrase "the
transcript". Together with parts 1 (ACP) and 2 (eval-SDK), this fills in the
"transcript" survey.

---

## Path A — `crates/wisp/src/renderer/transcript.rs` (the TUI renderer module: explicit negative)

The wisp (TUI) renderer module named `transcript`. **The file contains no
`transcript` literal anywhere** — not in a doc comment, not in a function
name, not in a string, not in a `use` path, not in an identifier. It defines
the word only by being *the module that is named "transcript"*: the parent
module's prose at `crates/wisp/src/renderer/mod.rs:9` says it "turns
conversation items into rows, through the `cache` of sealed items and the
`streaming` incremental renderer"; that is the only prose anywhere that names
what this module is. Because the module emits no string literal and no
identifier that contains the word, it is **not a definition cite** for
"Transcript" — it is at most a structural mention via the module path
(`Renderer::live_lines`, `Renderer::item_suffix`, etc.).

### A.0 — the negative, proved

```
$ git grep -n -i transcript -- crates/wisp/src/renderer/transcript.rs
$ # git grep prints no output, exits 1: zero matches.

$ # Confirm the literal *does* appear elsewhere under renderer/ (so the file-
$ # level absence is not just a coincidence of the wider tree):
$ git grep -n -i transcript -- crates/wisp/src/renderer/
crates/wisp/src/renderer/frame.rs:23:    let [transcript_area, plan_area, composer_area, status_area] = layout.split(frame.area());
crates/wisp/src/renderer/frame.rs:28:    draw_transcript(layout, live, transcript_area, buf, app.progress_indicator(), theme, app.spinner_tick());
crates/wisp/src/renderer/frame.rs:51:fn draw_transcript(
crates/wisp/src/renderer/history.rs:52:            let notice = wrap_line(Line::raw("Transcript updated; earlier scrollback is superseded."), width);
crates/wisp/src/renderer/history.rs:66:    /// Moves transcript rows the viewport can no longer show into the
crates/wisp/src/renderer/layout.rs:30:    pub(super) transcript_height: u16,
crates/wisp/src/renderer/layout.rs:65:            transcript_height: remaining.saturating_sub(plan_height),
crates/wisp/src/renderer/mod.rs:9://! - `transcript` turns conversation items into rows, through the `cache` of
crates/wisp/src/renderer/mod.rs:22:mod transcript;
crates/wisp/src/renderer/mod.rs:120:        let capacity = usize::from(layout.transcript_height).saturating_sub(usize::from(layout.progress_height));
```

i.e. the word "transcript" appears in `frame.rs`, `history.rs`, `layout.rs`,
and `mod.rs` — but **not** in the file `transcript.rs` itself.

### A.1 — the file: 130 lines, five `impl Renderer` methods, no wording

The whole file (one `impl Renderer` block, lines 15–129) defines five
methods. They are all generic ratatui-row builders over `ConversationItem`;
none of them produces a quoted sentence, a label string, or any user-visible
wording. Signatures, verbatim:

- `pub(super) fn live_lines(...) → Result<Vec<Line<'static>>, MarkdownStreamError>` (line **18**)
- `pub(super) fn lines(...) → Result<Vec<Line<'static>>, MarkdownStreamError>` (line **26**)
- `fn live_lines_inner(...) → Result<Vec<Line<'static>>, MarkdownStreamError>` (line **43**, private)
- `pub(super) fn item_suffix(...) → Result<Vec<Line<'static>>, MarkdownStreamError>` (line **73**)
- `fn cached_item_lines(...) → Rc<[Line<'static>]>` (line **101**, private)

The five bodies, in sequence:

- `live_lines` (lines 18–22) calls the private `live_lines_inner`, then
  updates `stats.max_live_rows`:
  ```rust
  pub(super) fn live_lines(&mut self, app: &App, width: u16) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
      let lines = self.live_lines_inner(app, width)?;
      self.stats.max_live_rows = self.stats.max_live_rows.max(lines.len());
      Ok(lines)
  }
  ```
- `lines` (lines 26–41) loops a slice of `ConversationItem`s, threading
  `previous_kind` through `item_suffix`:
  ```rust
  pub(super) fn lines(
      &mut self,
      items: &[ConversationItem],
      previous_kind: Option<ContentKind>,
      width: u16,
      padding: usize,
      spinner_tick: usize,
  ) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
      let mut lines = Vec::new();
      let mut previous = previous_kind;
      for item in items {
          lines.extend(self.item_suffix(item, previous, width, padding, spinner_tick, 0)?);
          previous = Some(content_kind(item));
      }
      Ok(lines)
  }
  ```
- `live_lines_inner` (lines 43–71), the only named non-trivial branch: when
  `commit.rows == 0` it just delegates to `lines` over the items *from* the
  commit item forward (lines 50–58); otherwise it renders one `item_suffix`
  for the in-progress item using the partial-commit dimensions, then `lines`
  for the rest (lines 60–69). No string.
- `item_suffix` (lines 73–94) pushes a single blank `Line::default()` when
  the kind changes (line **85**: `lines.push(Line::default());`), then
  delegates either to `streaming_item_lines` (for items that stream into
  native history) or to `cached_item_lines` (line **91**). The only literal
  in the function is `Line::default()` (the empty separator) — never the
  word "transcript".
- `cached_item_lines` (lines 101–129) is the per-frame render cache: it
  builds a `RenderKey { item_id, revision, shape, spinner }` (lines 111–116)
  and pulls the rendered `Rc<[Line<'static>]>` from `render_cache.get_or_insert_with`
  (line **119**), bumping `stats.item_rebuilds` / `stats.ns_item_rebuild` on a
  cache miss and tracking `stats.markdown_bytes_parsed` for assistant text
  (lines 120–128). Again, no wording.

So the whole surface — one `impl Renderer` block, five methods, 130 lines,
zero identifier/string/comment containing the word "transcript" — is
**rendering plumbing, not wording**. It is not a definition cite for
"Transcript".

### A.2 — the only prose naming what the module is (outside the file)

`crates/wisp/src/renderer/mod.rs:9`, verbatim:

```rust
//! - `transcript` turns conversation items into rows, through the `cache` of
```

(with the continuation `//!   sealed items and the `streaming` incremental renderer;`
on lines 10–11, and the child-module declaration `mod transcript;` on
line 22.) Note this prose is *in the parent module*, calling the
child `transcript` by backtick-quoted identifier — it does not put the word
in any string a user sees, and it does not say anything semantic about what
a "transcript" is in the project vocabulary.

---

## Path B — `packages/website/src/content/docs/aether/running/headless.mdx:87` (the user-facing label)

The headless CLI reference page, inside `<TabItem label="text">`. Line **87**
in full, verbatim:

```
    Human-readable lines. Final text is printed as plain text; tool calls, tool results, errors, context usage, and other selected events are formatted as simple lines. Background MCP tasks get their own lines as they start and finish (`Tool deferred ...`, `Background task completed ...`, `Background task failed ...`, `Background task cancelled ...`). At the end of a run the transcript ends with `Done` (or `Cancelled` / `Error: ...`) followed by a `Files changed: N (X created, Y modified, Z deleted)` line that tallies every `FileDiff` the run produced through the coding tools (`write_file`, `edit_file`, and any future tool that emits a `FileDiff`). A read-only run reports `Files changed: 0 (0 created, 0 modified, 0 deleted)`.
```

Key clause (the one the task names): *"At the end of a run the transcript
ends with `Done` (or `Cancelled` / `Error: ...`) followed by a `Files
changed: N (X created, Y modified, Z deleted)` line …"*

— Here "the transcript" denotes the *stream of text-mode lines that the
headless CLI prints to stdout* (`--output text`): the `Done` / `Cancelled` /
`Error: ...` terminator, the `Files changed: N (X created, Y modified, Z
deleted)` summary, the per-event `Tool deferred …` / `Background task …`
lines, the assistant text and tool call/result/error lines. It is a
*user-facing label for the produced output stream*, not a Rust struct, not a
file format, not a persisted session event log.

There is only one such phrase on the whole page (`git grep -n "the
transcript" packages/website/src/content/docs/aether/running/headless.mdx`
→ one hit, line **87**).

---

## Path C — `packages/website/src/content/docs/aether/running/remote.mdx:75` (the user-facing label)

The Remote agent page, inside the "Detach, cancel, and restore" section.
Line **75** in full, verbatim:

```
An unexpected connection loss restores the terminal and exits with an error. There is **no automatic reconnect or prompt retry**. If a prompt's acceptance response was lost, reconnect and inspect the transcript before resubmitting: a retry could execute tools twice.
```

Key clause: *"If a prompt's acceptance response was lost, reconnect and
inspect the transcript before resubmitting: a retry could execute tools
twice."*

— Here "the transcript" denotes **whatever the user / attached client UI can
look at on (re)attach**: the persisted conversation the session actor holds
and replays (the same `transcript: Vec<SessionEvent>` field named in part 1
at `crates/aether-cli/src/acp/session/actor.rs:174,195`, replayed via
`replay_to_client` at lines 257 and 448). The phrase is qualitative —
"inspect the transcript" — and again denotes a *user-facing concept*, not a
Rust struct or a file format. It is not contradictory with Path B: in B the
"transcript" is the headless text stream on disk/discipline; in C it is the
conversation history the remote client sees on the wire. Both are
*labels for output*.

There is only one such phrase on the whole page (`git grep -n "the
transcript" packages/website/src/content/docs/aether/running/remote.mdx` →
one hit, line **75**).

---

## Characterisation (one sentence)

The TUI/headless side of the survey has **one structural cite and two
documentation cites**:

- **Path A (`transcript.rs`).** A module is named `transcript.rs` —
  a 130-line `impl Renderer` with five methods (`live_lines`, `lines`,
  `item_suffix` are public-ish; `live_lines_inner`, `cached_item_lines`
  are private) that turn `ConversationItem`s into ratatui `Vec<Line<'static>>`
  / `Rc<[Line<'static>]>` rows through a per-frame render cache. The file
  contains **no `transcript` literal**, so it is *not* a definition cite for
  the word — only a structural one via the module path and the prose at
  `crates/wisp/src/renderer/mod.rs:9`.
- **Path B (`headless.mdx:87`).** "The transcript" denotes the headless
  CLI's text-output stream — the emitted lines that end in `Done` /
  `Cancelled` / `Error: ...` and a `Files changed:` summary.
- **Path C (`remote.mdx:75`).** "The transcript" denotes the conversation
  history the (re)attached remote client can inspect — qualitatively the
  same `Vec<SessionEvent>` replayed by the ACP session actor in part 1.

So in the docs (B and C) and the TUI module (A), "transcript" is *a
display / output concept — emitted event lines (text mode) and the
on-the-wire session history surfaced to the user — not a Rust struct or a
file format*. None of the three paths is a definition cite for the
`## Glossary` entry; combined with parts 1 (ACP, the persisted session
event log) and 2 (eval-SDK, the Rust/TypeScript `Transcript` class), the
word denotes at least four distinct things across the project.

---

## Proof that the cited lines exist at the base commit

```
$ git grep -n -i transcript -- crates/wisp/src/renderer/transcript.rs
$ # (no output — proves the explicit negative)

$ git grep -n "the transcript" packages/website/src/content/docs/aether/running/
packages/website/src/content/docs/aether/running/headless.mdx:87:    Human-readable lines. Final text is printed as plain text; tool calls, tool results, errors, context usage, and other selected events are formatted as simple lines. Background MCP tasks get their own lines as they start and finish (`Tool deferred ...`, `Background task completed ...`, `Background task failed ...`, `Background task cancelled ...`). At the end of a run the transcript ends with `Done` (or `Cancelled` / `Error: ...`) followed by a `Files changed: N (X created, Y modified, Z deleted)` line that tallies every `FileDiff` the run produced through the coding tools (`write_file`, `edit_file`, and any future tool that emits a `FileDiff`). A read-only run reports `Files changed: 0 (0 created, 0 modified, 0 deleted)`.
packages/website/src/content/docs/aether/running/remote.mdx:75:An unexpected connection loss restores the terminal and exits with an error. There is **no automatic reconnect or prompt retry**. If a prompt's acceptance response was lost, reconnect and inspect the transcript before resubmitting: a retry could execute tools twice.

$ # Five methods in transcript.rs:
$ grep -n 'pub(super) fn live_lines\|pub(super) fn lines\|fn live_lines_inner\|pub(super) fn item_suffix\|fn cached_item_lines' crates/wisp/src/renderer/transcript.rs
18:    pub(super) fn live_lines(&mut self, app: &App, width: u16) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
26:    pub(super) fn lines(
43:    fn live_lines_inner(&mut self, app: &App, width: u16) -> Result<Vec<Line<'static>>, MarkdownStreamError> {
73:    pub(super) fn item_suffix(
101:    fn cached_item_lines(
```

`git status --porcelain` after this report is committed prints only
`aether-transcript-survey-tui-docs.md` (the new file this task adds); no
source file of the project was edited.
