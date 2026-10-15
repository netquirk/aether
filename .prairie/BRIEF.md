## Purpose
Aether is an AI coding agent harness written in Rust that gives the user control over every token in the model's context. Agents start as blank slates with no system prompt and zero tools; the system prompt is composed as an array of markdown files, and tools are loaded exclusively through MCP servers. Aether supports many LLM providers out of the box (Anthropic, OpenAI, OpenRouter, DeepSeek, Fireworks AI, Gemini, Microsoft Foundry, Moonshot, ZAI, Llama.cpp, Ollama) and exposes a `StreamingModelProvider` trait for bring-your-own providers, with an "alloy" mode that combines multiple providers. The harness is exposed through four entry points: an interactive terminal UI (the `aether` CLI runs the wisp TUI when no subcommand is given), an ACP v2 server for editor integration (`aether acp`), a headless mode that streams structured JSON logs (`aether headless`), and a Rust library (`aether-agent-core`) for embedding the agent loop in other applications, with a TypeScript SDK (`@aether-agent/sdk`) layered on top of the CLI.

## Stakeholders and users
- Developers running the interactive terminal UI (`aether` launches the wisp TUI): the default command in `crates/aether-cli/src/main.rs` calls `wisp::run_tui`.
- Developers integrating Aether into an IDE or editor that supports the draft Agent Client Protocol v2; the CLI exposes the `acp` subcommand and the workspace pins `agent-client-protocol = "=2.1.0"` with the `unstable_protocol_v2` feature.
- Developers running Aether headlessly (the `headless` subcommand in `crates/aether-cli/src/main.rs`) or operating the remote split (`aether server` hosts a persistent ACP session over WebSocket; `aether client` attaches a TUI to that server) for SSH-tunnelled or otherwise remoted work.
- Rust application developers who embed the agent loop through the `aether-agent-core` crate, re-exporting `AgentSpec` and `AgentSpecExposure` from `crates/aether-core/src/lib.rs`; the README's "Using Aether as a library" section documents a ~25-line embed.
- TypeScript developers consuming the `@aether-agent/sdk` package (workspace `packages/aether-sdk`, currently v0.7.1); the SDK spawns `aether acp` as a subprocess (bundling `@aether-agent/cli` for the host platform) and exposes one `AetherSession` API for sending prompts over that ACP stream.
- Maintainers and contributors: the LICENSE file names Joshua Carver (2026) as the copyright holder; CI is defined under `.github/workflows/`. End users are developers using the harness in their own repositories; end customers of the end users are unknown to the repository.

## Problem statement
A developer using an AI coding agent needs control over every token in the model's context window because other harnesses ship with hardcoded system prompts and tools that bloat context and force the developer to fight the harness for control.

## Quality goals (ranked)

- Ranking: unknown — the repository does not state a priority order among these goals.
- Total control over every token in the model's context window (README.md:10, 35–37).
- Blank-slate, opt-in agents: agents begin with no system prompt and zero tools; the user adds what is needed (README.md:37).
- Modularity: "highly modular by design"; go from a minimal agent to batteries-included with a few lines of config (README.md:37).
- Prompt composability: the system prompt is supplied as an array of markdown files the user composes (README.md:39).
- Tool extensibility with context economy: tools are loaded exclusively through MCP servers, in any language, local or remote (including OAuth), with progressive discovery so many MCPs do not blow the context window (README.md:40).
- Model portability: eleven LLM providers supported out of the box, plus a `StreamingModelProvider` trait for bring-your-own providers and an "alloy" mode that combines providers (README.md:41).
- Simplicity: "Favor simplicity over complexity. Good architecture results in simple systems" (AGENTS.md:20).
- Observable and explainable runs: the run log names the config file and profile loaded (TASK-26-858, TASK-27-16/17/18), the model and provider used (TASK-25-119), the files read but not changed (TASK-26-275, TASK-26-276, TASK-26-277), the token usage, cost and wall-clock time (TASK-25-7, TASK-24-278, TASK-25-148), and the refusal names the offending path, flag, or reason (TASK-26-394, TASK-26-110, TASK-26-472).
- Safety and sandboxing: tool calls reaching outside the working directory are refused, naming the path (TASK-26-394); a path-confining `ToolPolicy` is installed in every runtime the CLI spawns (TASK-26-614, TASK-26-615); credential-looking values are redacted before they are written to the log or transcript (TASK-26-22).
- Resilience: stream cuts without a finish reason are retried and a cut call resumes from its partial reply (TASK-23-191); repeated turns are stopped, not looped (TASK-23-227); a run can be resumed from a saved transcript (TASK-26-20).
- Verifiable tests: fakes over behaviour-verifying mocks, asserting state through the public API only, with no timeouts embedded in tests (AGENTS.md:31–36).

## Constraints

- Written in Rust; `cargo install aether-agent-cli` requires Rust 1.85+ (README.md:65). Workspace is built and checked via `just build` and `just check` (justfile:9–14).
- Tools are obtained exclusively through MCP servers; the harness exposes no built-in tools (README.md:40).
- The system prompt is supplied as an array of markdown files composed by the user (README.md:39).
- Aether is an ACP v2 agent; v1-only editor clients are not compatible (README.md:44). The protocol is pinned in the workspace as `agent-client-protocol = "=2.1.0"` with the `unstable_protocol_v2` feature (Cargo.toml:49).
- The raw `aether server` is unauthenticated; it must sit behind an SSH tunnel, a private network, or an authenticating TLS proxy (README.md:106–118).
- Error handling: no `anyhow` or `color-eyre`; use concrete error enums, not `Box<dyn std::error::Error>` (AGENTS.md:40–42).
- Tests must exercise only the public API and must not embed timeouts (AGENTS.md:34–35).
- The SQLite session-index schema is fixed by `crates/aether-sessions/migrations/001_session_index.sql` and `002_session_usage.sql`; checked queries must match the schema (`just sqlx-check`, justfile:44–52).
- CI gates: `just ci` runs `fmt-check`, `lint` (clippy `-D warnings`), `test-ci`, `doc-check` (rustdoc `-D warnings`), and `sqlx-check`, followed by `pnpm fmt-check`, `pnpm sdk:typecheck`, and `pnpm sdk:test` (justfile:78–82; .github/workflows/ci.yml).
- License: MIT (README.md:196–198; LICENSE).
- All GitHub operations go through the `gh` CLI (AGENTS.md:46).

## Out of scope

- Backwards compatibility and fallbacks: explicitly not a concern when planning or implementing code (AGENTS.md:19).
- ACP v1-only editor clients: not compatible with the v2 protocol Aether speaks (README.md:44).
- Hardcoded system prompts and built-in tools of the kind other harnesses ship — Aether deliberately ships neither (README.md:33–40).
- Built-in authentication for the raw `aether server`; the README mandates a tunnel, a private network, or an authenticating TLS proxy instead (README.md:118).
- `anyhow` and `color-eyre` as error-handling dependencies, and `Box<dyn std::error::Error>` as a public error type (AGENTS.md:40–42).
- Testing private methods, behaviour-verifying mocks, and timeouts embedded in tests (AGENTS.md:33–36).

## Glossary

- **Aether** — the AI coding agent harness written in Rust that gives the user control over every token in context (README.md:10).
- **Agent** — begins as a blank slate with no system prompt and zero tools; the user adds prompts and tools (README.md:37).
- **Prompt (system prompt)** — supplied as an array of markdown files the user composes (README.md:39).
- **Tool** — capability an agent may call; obtained exclusively via MCP servers (README.md:40).
- **MCP** — Model Context Protocol; the only route by which agents get tools, supported in any language, local or remote (including OAuth) (README.md:40).
- **Provider** — an LLM backend; eleven are supported out of the box (Anthropic, OpenAI, OpenRouter, DeepSeek, Fireworks AI, Gemini, Microsoft Foundry, Moonshot, ZAI, Llama.cpp, Ollama) plus the `StreamingModelProvider` trait for bring-your-own providers (README.md:41); each usage event records a `provider` (`migrations/002_session_usage.sql`).
- **Alloy** — combining multiple providers/models to round-robin across providers per turn (README.md:41, README.md:122).
- **Session** — an agent conversation recorded in the session index; one `session_files` row per session, and the `sessions` view selects rows whose `status = 'indexed'` (`migrations/001_session_index.sql`).
- **Event** — a record of something that happened in a session, stored in the `events` table (`migrations/001_session_index.sql`); the library names these `AgentEvent` / `TurnEvent` (README.md:135).
- **Turn** — a unit of model interaction, indexed as `turn_index` on `events` (`migrations/001_session_index.sql`); README refers to "per turn" alloying (README.md:122).
- **Tool call** — an `events` row with `event_type = 'tool_call'`, exposed as the `tool_calls` view (`migrations/001_session_index.sql`).
- **Tool result** — an `events` row with `event_type = 'tool_result'`, exposed as the `tool_results` view (`migrations/001_session_index.sql`).
- **Context usage** — token usage against a context limit; surfaced by the `context_usage` view and recorded by the `usage_ratio` / `context_limit` columns on `events` (`migrations/001_session_index.sql`).
- **Sub-agent** — an agent spawned by another; the index records `agent_id` / `parent_agent_id` (`migrations/002_session_usage.sql`), and Aether ships sub-agents as a first-party MCP (README.md:40).
- **Skill** — a first-party capability delivered through MCP (README.md:40).
- **ACP** — Agent Client Protocol v2; Aether is an ACP v2 agent, so v1-only editor clients are not compatible (README.md:44).
- **TUI** — the interactive terminal UI for Aether (README.md:43).
- **Headless** — running Aether as an agent that streams structured JSON logs (README.md:45).
- **Transcript** — unknown (not defined in README.md or `crates/aether-sessions/migrations/*.sql`).

## Domain paths

- `aether-agent-core` → `crates/aether-core` (README.md:180).
- `llm` → `crates/llm` (README.md:181).
- `wisp` → `crates/wisp` (README.md:182).
- `aether-agent-cli` → `crates/aether-cli` (README.md:183).
- `mcp-servers` → `crates/mcp-servers` (README.md:184).
- `aether-evals` → `crates/aether-evals` (README.md:185).
- `aether-lspd` → `crates/aether-lspd` (README.md:186).
- `aether-project` → `crates/aether-project` (README.md:187).
- `@aether-agent/sdk` → `packages/aether-sdk` (README.md:193).
- `@aether-agent/website` → `packages/website` (README.md:194).
- Additional workspace crates present under `crates/` (e.g. `aether-sessions`, `aether-session-index`) and TypeScript packages under `packages/` (e.g. `packages/aether-evals`) exist on disk but are not listed in the README's crate/package tables; their mapping to README entries is unknown.

## Open questions

- Should workspace crates/packages the README tables omit (`crates/aether-sessions`, `crates/aether-session-index`, `crates/acp-utils`, `crates/aether-auth`, `crates/aether-schemas`, `crates/aether-telemetry`, `crates/mcp-utils`, `crates/llm-codegen`, `crates/utils`, `crates/doctest`, `crates/internal-evals`, `packages/aether-evals`) get `## Domain paths` entries, and what maps to what?
- Define "Transcript" for the `## Glossary` entry that currently reads `unknown`, or confirm it stays undefined.
- Is the raw unauthenticated `aether server` acceptable long-term, or is built-in authentication planned? The README today mandates a tunnel, private network or authenticating TLS proxy.
- Are merged task titles (`TASK-*`) acceptable evidence in this brief, or must every claim cite README.md/a source file? The `## Quality goals` list rests on task titles.
- Deployment/hosting: the board still carries open work to deploy aether to production and bind its production secrets (`TASK-26-143`, `TASK-27-488`) while `## Constraints`/`## Out of scope` name no hosting target; should the brief state one?
- Confirm this draft (assembled by `TASK-27-19`/`-20`/`-21`/`-22` from README.md, AGENTS.md and merged task titles) as the canonical brief for aether, or name what must change.

## Decisions

- Rank the quality goals: the repository lists them but states no priority order; what is the order? — "Control-first (README order)" (a), 2026-10-15, chosen by operator.
- Should the brief name the end customers of Aether's users? The repository does not establish them (`unknown` today). — "Scope them out of the brief" (a), 2026-10-15, chosen by operator.
