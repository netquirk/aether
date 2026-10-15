## Purpose
Aether is an AI coding agent harness written in Rust that gives the user control over every token in the model's context. Agents start as blank slates with no system prompt and zero tools; the system prompt is composed as an array of markdown files, and tools are loaded exclusively through MCP servers. Aether supports many LLM providers out of the box (Anthropic, OpenAI, OpenRouter, DeepSeek, Fireworks AI, Gemini, Microsoft Foundry, Moonshot, ZAI, Llama.cpp, Ollama) and exposes a `StreamingModelProvider` trait for bring-your-own providers, with an "alloy" mode that combines multiple providers. The harness is exposed through four entry points: an interactive terminal UI (the `aether` CLI runs the wisp TUI when no subcommand is given, and `wisp` is also installable as a separate binary), an ACP v2 server for editor integration (`aether acp`), a headless mode that streams structured JSON logs (`aether headless`), and a Rust library (`aether-agent-core`) for embedding the agent loop in other applications, with a TypeScript SDK (`@aether-agent/sdk`) layered on top of the CLI.

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

## Domain paths

## Open questions
