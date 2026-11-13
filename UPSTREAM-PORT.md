# Port table: local patches onto upstream `aether-agent-cli-v0.9.21`

The local line was merged with upstream `aether main` (`ff9b9dac`, which contains the
release tag `aether-agent-cli-v0.9.21`) in commit `b39beeb6`. Every local-only
patch is listed below with the upstream state, the outcome and the evidence.

| local commit | upstream state | outcome | evidence |
|---|---|---|---|
| `e934b222`, `efee510e` (path-confining `ToolPolicy` in every runtime) | upstream has no `ToolPolicy` | **ported** | `crates/aether-core/src/core/tool_policy.rs`, `crates/aether-core/src/core/agent.rs` (`handle_tool_completion`), `crates/aether-core/src/events/tool_event.rs` (`ToolEvent::Refused`, `refusal_context_message`), `crates/aether-cli/src/workspace_boundary.rs`, `crates/aether-cli/src/runtime.rs`, `crates/aether-core/tests/agent/refusal_tests.rs` |
| `5117463d` (per-command bash timeout) | upstream's bash tool has the timeout hook, but nothing sets it | **ported** | `crates/aether-project/src/aether_settings.rs` (`RunSettings::shell_command_timeout`), `crates/aether-core/src/mcp/mcp_builder.rs` (`with_shell_command_timeout`, `RuntimeServices`), `crates/mcp-servers/src/setup.rs` (`bash_environment`), `crates/aether-cli/src/headless/{mod,run}.rs`, `crates/aether-cli/src/runtime.rs` |
| `cd6752fc` (credential redaction in log/transcript) | upstream has no redaction | **not ported yet** - `crates/aether-cli/src/redact.rs` is in the tree, unwired | next step |
| `e54268fe` (drop `RUSTUP_TOOLCHAIN` from the language-server environment) | upstream deleted `crates/aether-lspd` (#633) and replaced it with the external `clankerdiff-lspd` crate, which this repository cannot patch | **dropped** - no in-repo home for the change | `crates/aether-lspd/**` deleted in `b39beeb6` |
| `bc9a40c3` (wait for MCP bootstrap in ACP sessions; atomic runtime deployment) | upstream's runtime builder exposes `.wait_for_mcp()`; upstream ships no deploy recipe | **ported** - the deploy recipe is unchanged (`.prairie/deploy.sh`, `.prairie/deploy-state.sh`, `.prairie/rollback.sh`, `.prairie/deploy.toml`); the bootstrap wait maps onto upstream's `wait_for_mcp()` | `.prairie/`, `crates/aether-cli/src/runtime.rs` |
| `83371aa3` (run.lock handling; headless `--cwd` honouring) | upstream's headless has `--cwd`; upstream has no `run.lock` | **not ported yet** - `crates/aether-cli/src/run_lock.rs` and `crates/aether-cli/src/working_directory.rs` are in the tree, unwired | next step |
| `750c3575` (opencode-go provider, reasoning-off group derived from the provider table) | upstream main registers no OpenCode provider at all | **not ported yet** | next step |
| `e8d10e9f`, `d80d2f18`, `0ec8a0ee`, `40f5a10f`, `0ab9e154`, `0b9c4739` (private-lint, error-lint, wait-lint, README provider check, SDK version check, rustdoc fixes, CI wiring) | upstream has none of these recipes | **ported** - kept | `crates/internal-lints/`, `crates/internal-readme-check/`, `crates/internal-sdk-version-check/`, `justfile`, `.github/workflows/ci.yml` |

## Local-only behaviour dropped with upstream's rewrite of the same area

These local features have no upstream equivalent and were dropped together with the
code upstream rewrote. They are listed here so no drop is silent.

- The local headless CLI flag surface: `--log-level`, `--log-format`, `--log-file`,
  log rotation, the transcript writer, `--resume`, `--max-tokens`, `--max-tool-calls`,
  `--timeout`, the session lock, the heartbeat line, the provider-stall warning, the
  file-change report and the version banner (upstream rewrote the headless runner in
  #601/#606; the local modules were removed with it).
- The `maxTurns` and `fallbackModel` agent settings; `toolOutput` is superseded by
  upstream's #560 oversized-tool-output handling.
- The repetition config (`aether-core/src/core/repetition_config.rs`).
- The `require_tool_call` turn rule.
- The `resume_partial` retry mode (a cut call is retried from scratch, as upstream does).
- The settings-driven `custom` provider (the table above covers `opencode-go` only).
- The local ACP extras: `--cwd`, `--log-level`, the ACP message log, the health
  endpoint and the detached arguments (upstream's ACP 5.0 implementation replaces them).

## Upstream fixes this upgrade now delivers

#654 (common agent tool-call failures), #653 (codex prompt cache by session), #590
(provider stream stalls), #586 (agent panics reported as failed turns), #584
(responses streams finalized at terminal events), #607 (provider connections and
credentials shared across agents), #625 (lspd `didSave` only for agent-written
files), #627 (ACP native subagents), #648 (rmcp 3.8 + agent-client-protocol 5.0),
#633 (the lspd replacement), #634 (the client-core / desktop crate split), #560
(oversized tool output handling), and the provider/MCP refactors #549, #551, #556,
#558, #583, #595, #601, #606.
