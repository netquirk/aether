# TASK-2B-372 — Eval-SDK transcript surfaces: the Rust and TypeScript `Transcript` mirrors

Part 2 of TASK-2B-344 ("Survey the ACP, eval-SDK, TUI and docs meaning of 'Transcript'").
Read-only evidence pass. No source file is changed by this task; `git diff --stat`
against the base commit `f955f53ef93ae2dc4202f9f83e2d1d0764a67571` is empty
after the commit that adds this report (only this new file is added).

Base commit: `f955f53ef93ae2dc4202f9f83e2d1d0764a67571
              TASK-2B-371: Survey the ACP transcript surfaces`
Read with `git status --porcelain` clean before any change; `git ls-files` then
confirmed every line cited below is unchanged at this commit.

The task names **four** anchor locations (Rust `transcript.rs:23`, `:57`, `:115`
and the `transcript.ts:39–46` block). To satisfy every reading, the six
defining lines inside those anchors are quoted verbatim below (three in Rust,
three in TypeScript).

---

## Path A — `crates/aether-evals/src/agents/transcript.rs` (the Rust mirror)

The eval SDK's Rust-side `Transcript` and the `TranscriptHeader` it optionally
holds. Together they are the SDK's ordered, type-safe in-memory record of a
run — built by streaming an `AgentRunResult` stream into `from_stream`, not
read back from a file.

### A.1 — `pub struct TranscriptHeader {` (line **23**)

Defining line (line **23**), verbatim, inside `pub struct TranscriptHeader`:

```rust
pub struct TranscriptHeader {
```

The struct body, lines **24–25** (verbatim):

```rust
    pub working_dir: PathBuf,
    pub starting_commit: StartingCommit,
```

The doc comment immediately above (lines **19–21**) states the relationship to
`Transcript` verbatim:

```rust
/// Header that captures, at run start, where the run happened and the git state of
/// that directory. A header is captured once at the beginning of the run and stored on
/// the [`Transcript`] alongside the streamed `AgentEvent`s.
```

— i.e. a `TranscriptHeader` is captured once at run start (via
`TranscriptHeader::capture`, line **32**) and stored on the `Transcript`
alongside its `AgentEvent`s. The Rust header is *only* reachable through the
`Transcript` it lives on (there is no `pub` accessor on the type).

### A.2 — `pub struct Transcript {` (line **57**) with `events` (line **58**) and `header` (line **59**)

Defining line (line **57**), verbatim, inside `pub struct Transcript`:

```rust
pub struct Transcript {
```

Field lines **58–59**, verbatim:

```rust
    events: Vec<AgentEvent>,
    header: Option<TranscriptHeader>,
```

The fields are private (no `pub`), and are reached through `events()`
(lines **133–135**, `pub fn events(&self) -> &[AgentEvent] { &self.events }`),
`add(&mut self, event: AgentEvent)` (lines **129–131**, `self.events.push(event);`),
`with_header(…)` (lines **94–97**) and `header()` (lines **105–107**). The
header is optional — `Transcript::new(Vec<AgentEvent>)` (lines **90–92**) sets
`header: None` and `Default::default()` (lines **212–216`) returns that empty
form.

The element type is the `AgentEvent` imported at line **4**:

```rust
use aether_core::events::{AgentEvent, LlmCallOutcome, ToolEvent, TurnEvent, TurnOutcome};
```

— i.e. the SDK re-uses `aether_core::AgentEvent` (the discriminated union of
`Turn`, `Tool`, `Context`, `SessionUsage` events used by the agent core), it
does not redeclare it.

### A.3 — `pub async fn from_stream<T: Stream<Item = AgentRunResult>>(…)` (line **115**)

Defining line (line **115**), verbatim, with its `#[allow(clippy::result_large_err)]`
attribute on line **114** shown for context:

```rust
    #[allow(clippy::result_large_err)]
    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {
```

Body, lines **116–127**, verbatim:

```rust
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
```

— i.e. the Rust `from_stream` consumes a `futures::Stream<Item = AgentRunResult>`
(the `Agent::run` stream returned by the eval crate's agents), pushes each
`Ok(event)` into a fresh `Transcript` via `add`, and on the first
`Err(error)` returns a `TranscriptError` carrying the *partial* `transcript`
(`TranscriptError.transcript: Transcript`, line **84**) so callers can render
a diagnostic. The `#[allow(clippy::result_large_err)]` is intentional: the
error type intentionally carries the full offending transcript (the comment
on lines **109–113** documents why).

---

## Path B — `packages/aether-evals/src/transcript.ts` (the TypeScript mirror)

The eval SDK's TypeScript-side `Transcript` class — the JS/TS counterpart of
the Rust `Transcript`. Same surface (ordered `AgentEvent` list, same
`fromStream` constructor) over a TypeScript `AsyncIterable<AgentEvent>`.

### B.1 — `export class Transcript {` (line **39**)

Defining line (line **39**), verbatim:

```typescript
export class Transcript {
```

### B.2 — `readonly events: AgentEvent[];` (line **40**)

Field line **40**, verbatim:

```typescript
  readonly events: AgentEvent[];
```

The constructor (lines **42–44**) initialises it from an optional input array,
copied defensively:

```typescript
  constructor(events: AgentEvent[] = []) {
    this.events = [...events];
  }
```

`AgentEvent` is the *SDK*'s event type, re-used via import (lines **1–7**):

```typescript
import type {
  AgentEvent,
  ContextUsage,
  LlmCallOutcome,
  TokenUsage,
  TurnOutcome,
} from "@aether-agent/sdk";
```

— the TypeScript `Transcript` re-uses `@aether-agent/sdk`'s `AgentEvent`
discriminated union rather than redeclaring it, mirroring the Rust
`aether_core::events::AgentEvent` re-use at `transcript.rs:4`.

`add` (lines **60–62**) pushes into the same array, verbatim:

```typescript
  add(event: AgentEvent): void {
    this.events.push(event);
  }
```

— i.e. append-only, same as the Rust `add`.

### B.3 — `static async fromStream(` (lines **46–48**)

Defining line (line **46**), verbatim, with the rest of the signature and
opening brace (lines **46–48**):

```typescript
  static async fromStream(
    stream: AsyncIterable<AgentEvent>,
  ): Promise<Transcript> {
```

Body, lines **49–58**, verbatim:

```typescript
    const transcript = new Transcript();
    try {
      for await (const event of stream) {
        transcript.add(event);
      }
    } catch (err) {
      throw new TranscriptError(transcript, err);
    }
    return transcript;
```

— i.e. the TypeScript `fromStream` consumes a TypeScript
`AsyncIterable<AgentEvent>`, appends each event via `add`, and on the first
thrown error throws a `TranscriptError` (lines **29–37**, with
`readonly transcript: Transcript;` on line **30** and the
`cause`-bearing `super("transcript stream failed", { cause });` on line **33**)
carrying the partial transcript so callers can still inspect it. The Rust
`Stream<Item = AgentRunResult>` and TS `AsyncIterable<AgentEvent>` differ in
their error contract (Rust surfaces `Result<AgentEvent, RunError>`; TS throws),
but the partial-transcript-on-failure shape is the same.

---

## Characterisation (one sentence)

Here "transcript" is the **SDK's ordered event list — the Rust and TypeScript
mirrors**: both wrappers hold an ordered `AgentEvent` sequence (`events: Vec<AgentEvent>`
at `transcript.rs:58` / `readonly events: AgentEvent[];` at `transcript.ts:40`)
plus, on the Rust side only, an optional `TranscriptHeader` (`header: Option<TranscriptHeader>`
at `transcript.rs:59`), and both are built from a stream
(`Transcript::from_stream` over `Stream<Item = AgentRunResult>` at
`transcript.rs:115`; `Transcript.fromStream` over `AsyncIterable<AgentEvent>` at
`transcript.ts:46`), neither of which is a file format — the `Transcript` is
an in-memory record of an eval run, not a persisted JSONL file and not the
session-replay buffer that the ACP actor keeps.

---

## Proof that the cited lines exist at the base commit

```
$ grep -n 'pub struct TranscriptHeader {' crates/aether-evals/src/agents/transcript.rs
23:pub struct TranscriptHeader {

$ grep -n 'pub struct Transcript {' crates/aether-evals/src/agents/transcript.rs
57:pub struct Transcript {

$ grep -n 'events: Vec<AgentEvent>,\|header: Option<TranscriptHeader>,' crates/aether-evals/src/agents/transcript.rs
58:    events: Vec<AgentEvent>,
59:    header: Option<TranscriptHeader>,

$ grep -n 'pub async fn from_stream' crates/aether-evals/src/agents/transcript.rs
115:    pub async fn from_stream<T: Stream<Item = AgentRunResult>>(stream: T) -> Result<Self, TranscriptError> {

$ grep -nE 'export class Transcript|readonly events: AgentEvent\[\]|static async fromStream' packages/aether-evals/src/transcript.ts
39:export class Transcript {
40:  readonly events: AgentEvent[];
46:  static async fromStream(
```

`git status --porcelain` after this report is committed prints only
`aether-transcript-survey-eval-sdk.md` (the new file this task adds); no
source file of the project was edited. The Rust file was read straight from
the working tree (no source change), and so were the four/three defining
lines above. `git show --stat HEAD` after the commit lists exactly one added
file.
