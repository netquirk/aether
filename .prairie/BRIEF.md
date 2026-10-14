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

## Constraints

## Out of scope

## Glossary

## Domain paths

## Open questions
