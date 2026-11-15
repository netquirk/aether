# Port table: local patches onto upstream `aether-agent-cli-v0.9.21`

The local line was merged with upstream `aether main` (`ff9b9dac`, which contains the
release tag `aether-agent-cli-v0.9.21`) in commit `b39beeb6`. Every local-only
patch is listed below with the upstream state, the outcome and the evidence.

| local commit | upstream state | outcome | evidence |
|---|---|---|---|
| `e934b222`, `efee510e` (path-confining `ToolPolicy` in every runtime) | upstream has no `ToolPolicy` | **ported and verified** - a refused tool call emits `ToolEvent::Refused` and is recorded as a completed tool call, so the turn continues and the model reads why the call did not run | `crates/aether-core/src/core/tool_policy.rs`, `crates/aether-core/src/core/agent.rs` (`handle_tool_completion`, `record_tool_refusal`), `crates/aether-core/src/context/recorder.rs` (`record_refusal`), `crates/aether-core/src/events/tool_event.rs` (`ToolEvent::Refused`, `refusal_context_message`), `crates/aether-cli/src/workspace_boundary.rs`, `crates/aether-cli/src/runtime.rs`, `crates/aether-core/tests/agent/refusal_tests.rs` (3 tests, green) |
| `5117463d` (per-command bash timeout) | upstream's bash tool has the timeout hook, but nothing sets it | **ported and verified** - the settings carry the ceiling, the runtime hands it to the MCP runtime and the `bash` tool applies it | `crates/aether-project/src/aether_settings.rs` (`RunSettings::shell_command_timeout`), `crates/aether-core/src/mcp/mcp_builder.rs` (`with_shell_command_timeout`, `RuntimeServices`), `crates/mcp-servers/src/setup.rs` (`bash_environment`), `crates/aether-cli/src/headless/{mod,run}.rs`, `crates/aether-cli/src/runtime.rs`; tests `crates/aether-project/tests/run_settings.rs`, `crates/aether-core/src/mcp/mcp_builder.rs` (`the_shell_command_timeout_reaches_the_runtime_services`), `crates/mcp-servers/src/setup.rs` (`the_shell_command_timeout_bounds_the_bash_tool`) - all green |
| `cd6752fc` (credential redaction in log/transcript) | upstream has no redaction | **ported and verified** - `SecretRedactor` scrubs the model catalogue's provider env vars, the `custom` provider key and every configured header value before a message reaches stdout or the run log | `crates/aether-cli/src/redact.rs`, `crates/aether-cli/src/output.rs` (`write_message`, `print_message`), `crates/aether-cli/src/headless/run.rs` (`SecretRedactor::from_env_and_spec`), `crates/aether-cli/src/acp/session/actor.rs` (`SecretRedactor::from_env`); tests `output::tests::write_message_redacts_secrets_before_they_reach_the_log` and the 14 `redact::tests::*` - all green |
| `e54268fe` (drop `RUSTUP_TOOLCHAIN` from the language-server environment) | upstream deleted `crates/aether-lspd` (#633) and replaced it with the external `clankerdiff-lspd` crate, which this repository cannot patch | **dropped** - no in-repo home for the change | `crates/aether-lspd/**` deleted in `b39beeb6` |
| `bc9a40c3` (wait for MCP bootstrap in ACP sessions; atomic runtime deployment) | upstream's runtime builder exposes `.wait_for_mcp()`; upstream ships no deploy recipe | **ported** - the deploy recipe is unchanged (`.prairie/deploy.sh`, `.prairie/deploy-state.sh`, `.prairie/rollback.sh`, `.prairie/deploy.toml`); the bootstrap wait maps onto upstream's `wait_for_mcp()` | `.prairie/`, `crates/aether-cli/src/runtime.rs` |
| `83371aa3` (run.lock handling; headless `--cwd` honouring) | upstream's headless has `--cwd`; upstream has no `run.lock` | **ported and verified** - the headless runner takes an exclusive `.aether/run.lock` before it starts and the `--cwd` flag is resolved through `resolve_run_cwd`; a second run on the same directory fails with `CliError::RunLocked` | `crates/aether-cli/src/run_lock.rs`, `crates/aether-cli/src/working_directory.rs`, `crates/aether-cli/src/headless/run.rs`, `crates/aether-cli/src/error.rs` (`RunLocked`), `crates/aether-cli/tests/integration/run_lock.rs` (2 tests, green), `.gitignore` (`**/.aether/run.lock`) |
| `750c3575` (opencode-go provider, reasoning-off group derived from the provider table) | upstream main registers no OpenCode provider at all | **ported and verified** - `opencode-go` is registered in the provider table and served by the generic provider with the `x-opencode-session` header and the extra header set the provider needs | `crates/llm-codegen/src/lib.rs` (`PROVIDERS`), `crates/llm/src/providers/generic.rs` (`OPENCODE_GO`, `opencode_go_headers`), `crates/llm/src/provider_connection.rs` (`headers`), `crates/llm/src/parser.rs` (`opencode_go_is_a_registered_provider`); tests `parser::tests::opencode_go_is_a_registered_provider`, `providers::generic::tests::opencode_go_sends_a_routing_session_header`, `providers::generic::tests::opencode_go_routing_session_is_fresh_per_provider` - all green |
| `e8d10e9f`, `d80d2f18`, `0ec8a0ee`, `40f5a10f`, `0ab9e154`, `0b9c4739` (private-lint, error-lint, wait-lint, README provider check, SDK version check, rustdoc fixes, CI wiring) | upstream has none of these recipes | **ported** - kept; the private-item lint reads a type whose `impl` block it meets before the module that declares it (`bb4ff3b2`), so it no longer reports a public re-export as a private item | `crates/internal-lints/`, `crates/internal-readme-check/`, `crates/internal-sdk-version-check/`, `justfile`, `.github/workflows/ci.yml` |
| `fec825ea` (the CI-gate verification script of TASK-2K-503) | upstream has no such script; upstream replaced `crates/aether-lspd` with the external `clankerdiff-lspd` (#633), so the `lsp_diagnostics_e2e` and `lsp_operations_e2e` targets the script gated no longer exist | **ported** - kept, with the two language-server gates dropped because their targets are gone upstream (`f5dabbed`); the eight remaining gates ran on the upgraded tree and their verdicts are recorded in `CI-GATES.md` | `scripts/verify-ci-gates.sh`, `CI-GATES.md` |

## Verification of the ported patches

Every port that has a check is listed with the command that proves it and the result that
command gave on the delivered tree (rustc 1.100.0, `RUSTUP_TOOLCHAIN=stable`).

| local commit | outcome | check | result |
|---|---|---|---|
| `e934b222`, `efee510e` (path-confining `ToolPolicy`) | ported and verified | `cargo test --locked -p aether-agent-core --test agent refusal` | 3 passed (`refused_tool_call_emits_refused_event_with_reason`, `refused_tool_call_records_refusal_in_context_for_next_turn`, `tool_policy_that_returns_none_lets_the_call_proceed`) |
| `5117463d` (per-command bash timeout) | ported and verified | `cargo test --locked -p aether-project --test run_settings`, `cargo test --locked -p aether-agent-core --lib shell_command_timeout`, `cargo test --locked -p aether-mcp-servers --lib shell_command_timeout` | 2 + 2 + 2 passed |
| `cd6752fc` (credential redaction) | ported and verified | `cargo test --locked -p aether-agent-cli --lib redact` | 15 passed, including `output::tests::write_message_redacts_secrets_before_they_reach_the_log` |
| `83371aa3` (run.lock, `--cwd`) | ported and verified | `cargo test --locked -p aether-agent-cli --test integration run_lock` | 2 passed (`a_stale_empty_run_lock_does_not_refuse_the_run`, `headless_options_json_run_starts_in_the_cwd_flag_directory`) |
| `750c3575` (opencode-go provider) | ported and verified | `cargo test --locked -p aether-llm opencode` | 3 passed |
| `e8d10e9f`, `d80d2f18`, `0ec8a0ee`, `40f5a10f`, `0ab9e154`, `0b9c4739` (lint recipes) | ported and verified | `cargo run -q -p internal-lints --bin private-lint`, `cargo run -q -p internal-lints --bin internal-lints`, `cargo run -q -p internal-lints --bin wait-lint`, `cargo run -q -p internal-readme-check`, `cargo run -q -p internal-sdk-version-check` | all exit 0 |
| `fec825ea` (CI-gate verification script) | ported, verdict recorded | `bash scripts/verify-ci-gates.sh --check` | exit 0 (the eight verdicts in `CI-GATES.md` agree with the script's gate list) |
| `bc9a40c3` (MCP bootstrap wait, atomic deploy recipe) | ported, not covered by a dedicated test | the deploy recipe is unchanged and `.wait_for_mcp()` is wired in `crates/aether-cli/src/runtime.rs` | **gap** - no test drives an ACP session through the MCP bootstrap wait |
| `e54268fe` (drop `RUSTUP_TOOLCHAIN` from the language-server environment) | not ported | upstream deleted `crates/aether-lspd` (#633) and replaced it with the external `clankerdiff-lspd` crate | dropped - no in-repo home for the change |

## Local-only behaviour that changed on the way up

- A refused tool call is recorded as a completed tool call again (`ContextRecorder::record_refusal`),
  so the turn continues and the model reads why the call did not run. Without it the upgrade ended
  the turn on the refusal, which is not what the local policy did.
- The local ACP extras (`--cwd`, the ACP message log, the health endpoint) and the local headless
  flag surface are gone with upstream's rewrite of those areas; the list above is exhaustive.

## Remaining gaps (TASK-33-6 owns the sweep)

- The full CI-gate matrix (`just ci`, the workspace test sweep, the docker targets) was not re-run
  on the final tree; `CI-GATES.md` records the verdicts of the tree before this session's fixes.
- `bc9a40c3`'s MCP bootstrap wait has no test of its own.

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
- The two language-server gates of `scripts/verify-ci-gates.sh` (`mcp-lsp-diagnostics`
  and `mcp-lsp-hover`): they ran the `lsp_diagnostics_e2e` and `lsp_operations_e2e`
  targets of `crates/mcp-servers`, and upstream replaced the daemon this repository
  held (`crates/aether-lspd`) with the external `clankerdiff-lspd` crate, so both
  targets are gone. The script names the eight gates that still exist.

## Upstream fixes this upgrade now delivers

#654 (common agent tool-call failures), #653 (codex prompt cache by session), #590
(provider stream stalls), #586 (agent panics reported as failed turns), #584
(responses streams finalized at terminal events), #607 (provider connections and
credentials shared across agents), #625 (lspd `didSave` only for agent-written
files), #627 (ACP native subagents), #648 (rmcp 3.8 + agent-client-protocol 5.0),
#633 (the lspd replacement), #634 (the client-core / desktop crate split), #560
(oversized tool output handling), and the provider/MCP refactors #549, #551, #556,
#558, #583, #595, #601, #606.
