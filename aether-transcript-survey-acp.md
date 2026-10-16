# TASK-2B-371 — ACP transcript surfaces: `SessionPreviewResponse.transcript` vs the actor's `transcript: Vec<SessionEvent>`

Part 1 of TASK-2B-344 ("Survey the ACP, eval-SDK, TUI and docs meaning of 'Transcript'").
Read-only evidence pass. No source file is changed by this task; `git diff --stat`
against the base commit `4f74ca2755584efa5d264e32bc8e41b05d9262d8` is empty after
the commit that adds this report (only this new file is added).

Base commit: `4f74ca2755584efa5d264e32bc8e41b05d9262d8
              TASK-2A-321: Brief: record the decision on Should the brief name the…`
Read with `git status --porcelain` clean before any change; `git ls-files` then
confirmed every line cited below is unchanged at this commit.

---

## Path A — `crates/acp-utils/src/notifications.rs` (the preview projection)

The ACP `SessionPreviewResponse` payload exposed to a (re)attaching editor
client. Its `transcript` field is a bounded `role`/`text` preview projection
of *one* session — not a record of session events.

Defining line (line **108**), verbatim:

```rust
    pub transcript: Vec<SessionPreviewTurn>,
```

inside `pub struct SessionPreviewResponse` (declared line **101**) alongside
`session_id` (102), `cwd` (103), `created_at` (104), `model` (105),
`selected_mode` (106), `tool_call_count` (109), `truncated` (110).

`SessionPreviewTurn`'s role/text shape (lines **113–125**), verbatim:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionPreviewTurn {
    pub role: SessionPreviewRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionPreviewRole {
    User,
    Assistant,
}
```

That is the whole `transcript` shape: a `Vec<(SessionPreviewRole, String)>`
of `User`/`Assistant` turns with a flat text payload.

---

## Path B — `crates/aether-cli/src/acp/session/actor.rs` (the in-memory/replayed event record)

The ACP session actor's in-memory transcript buffer — an ordered
`Vec<SessionEvent>` owned by `SessionActor`, accumulated as the session
runs and replayed to a (re)attaching editor client through
`replay_to_client`.

Field, line **195** (inside `pub(crate) struct SessionActor`, declared
line **183**), verbatim:

```rust
    transcript: Vec<SessionEvent>,
```

(corresponding public init field on `pub(crate) struct SessionActorInit`
at line **174**: `    pub transcript: Vec<SessionEvent>,`; consumed in
`SessionActor::spawn` at line **243**: `            transcript: init.transcript,`.)

Push site, lines **406–408** (inside `impl SessionActor`), verbatim:

```rust
    fn record_event(&mut self, event: SessionEvent) {
        self.transcript.push(event);
    }
```

i.e. line **407**: `        self.transcript.push(event);`. This is the only
mutator of the field inside the actor (every other call site reads).

Replay calls.

Spawn-time replay, lines **256–258**, verbatim:

```rust
        if init.replay {
            replay_to_client(&actor.transcript, &actor.io);
        }
```

i.e. line **257**: `            replay_to_client(&actor.transcript, &actor.io);`
(guarded by `if init.replay {` on line 256, immediately before the
`cmd_tx`/`cmd_rx` channel is created at line 259 and the actor task is
spawned at line 260).

Late-attach replay, lines **447–449** (inside `SessionCommand::Attach { ... }`
at line 439), verbatim:

```rust
                if replay {
                    replay_to_client(&self.transcript, &self.io);
                }
```

i.e. line **448**: `                    replay_to_client(&self.transcript, &self.io);`
(guarded by `if replay {` on line 447; preceded by `self.io.connection =
Some(connection);` on line 445 and followed by the first
`SessionUpdate::StateUpdate(state)` on line 456).

`replay_to_client` itself is defined at
`crates/aether-cli/src/acp/protocol/replay.rs:10`:

```rust
pub(crate) fn replay_to_client(events: &[SessionEvent], io: &SessionIo) {
```

— it walks the `&[SessionEvent]` slice and re-emits the ACP `SessionUpdate`s
(`UserMessage`, projected agent events, compaction updates) that the
editor client would otherwise have missed while it was unattached.

Corroborating reads inside the actor showing the field is the session's
event log (not a UI string list):

- line **362**: `let messages = conversation_messages_from_events(&self.transcript);`
  (inside `select_agent`)
- line **373**: `let messages = conversation_messages_from_events(&self.transcript);`
  (inside `sync_active_conversation`)
- line **386**: `let messages = conversation_messages_from_events(&self.transcript);`
  (inside `ensure_active_running`)
- line **396**: `let usage_seed = last_session_usage(&self.transcript).cloned();`
  (inside `ensure_running_with`)

and `aether_sessions::transcript::conversation_messages_from_events` /
`aether_sessions::transcript::last_session_usage` (imported at line 9)
both consume the `Vec<SessionEvent>` as the authoritative event record.

The element type `SessionEvent` is the discriminated enum
`pub enum SessionEvent { User(UserEvent), Agent(AgentEvent),
Control(SessionControlEvent) }` defined at
`crates/aether-sessions/src/model.rs:38` (verified by the import
`use aether_sessions::SessionEvent;` at the top of `actor.rs`).

---

## Characterisation (one sentence)

Here "transcript" is the in-memory/replayed session event record — the
ordered `Vec<SessionEvent>` owned by the ACP session actor, populated by
`record_event` (`self.transcript.push(event)`) and replayed to a
(re)attaching editor client through `replay_to_client(&actor.transcript, ...)`
/ `replay_to_client(&self.transcript, ...)`, a persisted session event
log (not a display projection), in contrast to the
`SessionPreviewResponse.transcript` field in
`crates/acp-utils/src/notifications.rs`, which is a bounded
`role`/`text` preview projection.

---

## Proof that the cited lines exist at the base commit

```
$ grep -n "pub transcript: Vec<SessionPreviewTurn>," crates/acp-utils/src/notifications.rs
108:    pub transcript: Vec<SessionPreviewTurn>,

$ grep -n "transcript: Vec<SessionEvent>," crates/aether-cli/src/acp/session/actor.rs
174:    pub transcript: Vec<SessionEvent>,
195:    transcript: Vec<SessionEvent>,

$ grep -n "self.transcript.push(event)" crates/aether-cli/src/acp/session/actor.rs
407:        self.transcript.push(event);

$ grep -n "replay_to_client(&" crates/aether-cli/src/acp/session/actor.rs
257:            replay_to_client(&actor.transcript, &actor.io);
448:                    replay_to_client(&self.transcript, &self.io);
```

`git status --porcelain` after this report is committed prints only
`aether-transcript-survey-acp.md` (the new file this task adds); no
source file of the project was edited.