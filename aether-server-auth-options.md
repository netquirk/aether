# Options: should the raw `aether server` stay unauthenticated?

The open question in `.prairie/BRIEF.md`:

> Is the raw unauthenticated `aether server` acceptable long-term, or is built-in authentication planned? The README today mandates a tunnel, private network or authenticating TLS proxy.

This note holds the option analysis for the one decision card this task raised. The brief itself
is **not** changed here: the answer is written back into `.prairie/BRIEF.md` by its own task
(`TASK-2D-613`, commit `d9ec6451`), which is already in this tree.

## 1. What the brief says today

- `## Constraints` (`BRIEF.md:37`): "The raw `aether server` is unauthenticated; it must sit behind an
  SSH tunnel, a private network, or an authenticating TLS proxy (README.md:106–118)."
- `## Out of scope` (`BRIEF.md:51`): "Built-in authentication for the raw `aether server`; the README
  mandates a tunnel, a private network, or an authenticating TLS proxy instead (README.md:118)."
- `## Decisions` (`BRIEF.md:100`) already carries the answer: "Keep unauthenticated; codify the
  tunnel/proxy mandate" (a), 2026-10-18, chosen by default (no answer by deadline). The question is no
  longer in `## Open questions`.

The brief has therefore taken a position: the raw server stays unauthenticated and the network
boundary is the operator's (a tunnel, a private network or an authenticating TLS proxy). What was
still open is whether the code should make that boundary harder to cross by accident.

## 2. What the code does today

| Source | What it states |
| --- | --- |
| `crates/aether-cli/src/acp/server.rs` | `--listen` defaults to `127.0.0.1:8765`. The listener is a raw WebSocket with no authentication of its own: the handshake callback only checks that no client is attached yet, and the first client that completes the handshake owns the agent session (a second one is refused with `409 client already attached`). |
| `crates/aether-cli/src/client.rs` | The client defaults to `ws://127.0.0.1:8765`, documents `wss://` "for an authenticating TLS proxy" and already carries a repeatable `--header` flag "for a gateway/proxy" — the proxy is the client's own supported route. |
| `crates/aether-auth` | OAuth credential storage, browser authorization and SigV4 for model providers and MCP servers. It knows nothing about a client authenticating to the ACP server, so a server-side credential check would be a new mechanism rather than an existing one. |
| `crates/aether-cli/src/acp/testing.rs` | The test server binds `127.0.0.1:0`, so every test today stays on loopback. |
| `README.md:107-119` | The documented remote split is an SSH tunnel to the loopback listener, and the closing sentence is the mandate: "The raw server is unauthenticated—use a tunnel, private network, or authenticating TLS proxy." |

## 3. The options

### (a) Keep unauthenticated; codify the tunnel/proxy mandate — **chosen (default applied)**

*Effect*: `aether server` keeps its loopback default and its unauthenticated protocol; a bind on any
address other hosts can reach is refused unless the operator passes the new opt-in acknowledgement
flag `--allow-unauthenticated-bind`, and then the server says loudly on stderr that anyone who can
reach the address owns the session. The README/brief tunnel-and-proxy wording stays the long-term
policy.

*Cost / risk*: the CLI cannot tell a private network from a public one, so the acknowledgement flag
is the only thing standing between a wide bind and an exposed agent session. An operator who passes
the flag is responsible for the tunnel, the private network or the authenticating proxy.

*Rests on*: the brief's constraint and out-of-scope lines, the client's existing proxy support, and
the fact that nothing on the board needs the raw server to authenticate itself (the production deploy
chain, `TASK-26-143` / `TASK-27-488`, goes through the container deploy contract, not this server).

### (b) Add a shared secret to the raw server

*Effect*: `aether server` takes a token (flag or environment variable) and rejects clients without
it; `aether client` sends it.

*Cost / risk*: new credential plumbing on both ends, key distribution and rotation, a new failure
mode for every existing remote user, and a direct contradiction of the brief's out-of-scope line —
which would have to change first. `crates/aether-auth` stores provider and MCP credentials; it is
not a server credential store, so this is new machinery rather than a configuration change.

### (c) Build the authenticating proxy into the CLI

*Effect*: the server terminates TLS itself (or starts a proxy) and authenticates clients by
certificate or token, so "authenticating TLS proxy" becomes an Aether feature instead of an
operator's.

*Cost / risk*: certificate or token lifecycle management, a much larger security surface to review,
client changes on both the TUI and the SDK path, and the same brief contradiction as (b). `rustls`
is already a dependency, but the WebSocket listener and the client would both have to grow a TLS
and identity story.

### (d) Retire the raw WebSocket server

*Effect*: `aether server` goes away; the remote split is served by the stdio ACP server plus an
external gateway, and the documented `aether server` + `aether client` workflow is withdrawn.

*Cost / risk*: it breaks the documented remote workflow (`README.md:107-119`) and the
`acp_remote` integration tests for users who rely on it, in exchange for a guarantee the brief
already gives through the tunnel mandate.

## 4. The decision card

One card was raised for this task, with the four options above as its paths and the mandate as the
recommended one:

```
prairie decide "<the question>" --context "<what the brief says, what each option rests on>" \
  --path a="Keep unauthenticated; codify the tunnel/proxy mandate" ... \
  --recommend a --why "<because>" --deadline 48h --default a --no-hold
```

Nobody answered the card before its deadline, so its default applied: path **(a)**, "Keep
unauthenticated; codify the tunnel/proxy mandate" (decision `66e37cac-9bfa-4669-b562-a15293ff297a`,
2026-10-18). The default is the path that moves the work forward — it keeps the server usable
today and turns the mandate into something the CLI states at the moment it matters — and it is
recorded in the brief at `BRIEF.md:100` by `TASK-2D-613`.

## 5. What the chosen path means in this tree

- `crates/aether-cli/src/acp/server.rs`: the loopback default is kept (`127.0.0.1:8765`). The new
  `--allow-unauthenticated-bind` flag is the operator's acknowledgement; without it a non-loopback
  bind is refused with an error that names the address, the reason and the three sanctioned
  mitigations, and with it the server prints a loud warning on stderr before it starts.
- `crates/aether-cli/tests/integration/server_bind.rs`: the policy is pinned through the public API
  and the real CLI binary — loopback needs no acknowledgement, a wide bind is refused without it,
  and the warning names the address and the mandate.
- `README.md:119`: the remote-usage section keeps the mandate sentence and adds the acknowledgement
  flag next to it.
- `.prairie/BRIEF.md`: **unchanged**. The decision was written back there by its own task, which is
  already merged (`d9ec6451`).

## 6. What would reopen the question

- A product decision that the remote split must work over the public internet without an operator
  in the middle (that is (b) or (c), and the brief's out-of-scope line changes with it).
- The raw server becoming the transport for a hosted, multi-tenant service.
- A request for the raw server to carry per-client identities rather than the single attached
  client it has today.

## 7. Verification

See the task's hand-off summary for the commands and their results (`cargo fmt --all --check`,
`cargo check -p aether-agent-cli --all-targets`, the `server_bind` integration tests, clippy with
`-D warnings`, and the workspace's `private-lint`, `error-lint` and `wait-lint`).
