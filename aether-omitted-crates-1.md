# TASK-2A-323 — name -> path inventory: the first six README-omitted crates

Inventory of six workspace crates that the README crate table and the brief's
`## Domain paths` omit. Each row is anchored to a `crates/<dir>/Cargo.toml:<line>`
citation that I read directly at base commit `8e0ef7bf`. No `name` or flag below
is inferred from the directory name.

Base commit verified: `8e0ef7bf TASK-27-22: Add Open questions, audit for invented facts, commit and…`
(`git status --porcelain` is empty; no repo file is changed by this task.)

## Flag semantics used in the table

- **`publish`** — value of the `[package]` `publish` key. Cargo's default (key
  absent) is "publishable". A `publish` key with no value is invalid; valid
  values are `true`, `false`, or a list of registries. Five of the six manifests
  below OMIT the `publish` key entirely.
- **`dist`** — value of `[package.metadata.dist] dist = …`. `dist-workspace.toml`
  only ever distributes `aether-agent-cli` (`packages = ["aether-agent-cli"]`),
  so `dist = false` on every workspace member is an explicit opt-out.

## Table — `name -> path` with manifest citations

| # | Directory (`path`)              | Package `name` (manifest line) | `description` (manifest line)                                         | `publish`                            | `dist`                                    | Notes from manifest                |
|---|---------------------------------|--------------------------------|-----------------------------------------------------------------------|--------------------------------------|-------------------------------------------|-------------------------------------|
| 1 | `crates/aether-sessions/`       | `aether-sessions` (`Cargo.toml:2`)              | "Persisted session logs and transcript reconstruction for Aether" (`:5`) | key absent → publishable by default | no `[package.metadata.dist]` section       | `version = "0.3.4"` (`:3`); features `analytics`, `testing` (`:16–18`) |
| 2 | `crates/aether-session-index/`  | `aether-session-index` (`Cargo.toml:2`)         | **none** — `[package]` table at `:1–5` has no `description` line        | `publish = false` (`:5`)            | `dist = false` (`:7–8`)                    | **TRAP 2**: `publish = false` AND no description; lib `aether_session_index` (`:14`), bin `aether-session-index` (`:18`) |
| 3 | `crates/acp-utils/`             | **`aether-acp-utils`** (`Cargo.toml:2`)         | "Agent Client Protocol (ACP) utilities for the Aether AI agent framework" (`:5`) | key absent → publishable by default | `dist = false` (`:12–13`)                 | **TRAP 1**: directory is `acp-utils`, package name is `aether-acp-utils`; lib `acp_utils` (`:16`); referenced as `aether-acp-utils` from `crates/aether-sessions/Cargo.toml:21` |
| 4 | `crates/aether-auth/`           | `aether-auth` (`Cargo.toml:2`)                 | "OAuth credential storage and authorization flows for the Aether AI agent framework" (`:5`) | key absent → publishable by default | `dist = false` (`:10–11`)                 | `version = "0.1.16"` (`:3`); features `keyring`, `mcp` (`:17–24`) |
| 5 | `crates/aether-schemas/`        | `aether-schemas` (`Cargo.toml:2`)              | "Schema generation utilities for Aether SDK codegen" (`:5`)            | key absent → publishable by default | `dist = false` (`:11–12`)                 | `version = "0.2.9"` (`:3`)          |
| 6 | `crates/aether-telemetry/`      | `aether-telemetry` (`Cargo.toml:2`)            | "OpenTelemetry support for Aether GenAI calls" (`:5`)                  | key absent → publishable by default | `dist = false` (`:9–10`)                  | `version = "0.3.4"` (`:3`)          |

## Verification of the two traps called out by the task

### Trap 1 — `crates/acp-utils` publishes as `aether-acp-utils`, not `acp-utils`

`crates/acp-utils/Cargo.toml:2`:

```
name = "aether-acp-utils"
```

Cross-citation — the consuming crate refers to the package, not the directory:

`crates/aether-sessions/Cargo.toml:21`:

```
acp_utils = { package = "aether-acp-utils", path = "../acp-utils", default-features = false, version = "0.5.4" }
```

So in `deps = […]` tables and on crates.io the package is named
`aether-acp-utils`; the directory is just `crates/acp-utils` and the lib
symbol is `acp_utils` (`crates/acp-utils/Cargo.toml:16`).

### Trap 2 — `crates/aether-session-index` is `publish = false` AND has no `description`

`crates/aether-session-index/Cargo.toml:1–5`:

```
[package]
name = "aether-session-index"
version = "0.1.0"
edition = "2024"
publish = false
```

There is no `description = "…"` line in the `[package]` table. The fields jump
straight from `edition` (`:4`) to `publish = false` (`:5`). So the manifest
literally does not state a description — this is not the place to invent one.

This crate is also the only one in the six that carries its own
`dist = false` opt-out under `[package.metadata.dist]` (`:7–8`), which makes
sense because it is a workspace-internal CLI (`src/main.rs` per `:18–19`) for
querying the session index.

## Notes that may matter to the parent decision

- Five of the six crates OMIT the `publish` key, so they are publishable by
  Cargo's default. Recording them as `publish = true` would be wrong; the
  literal manifest field is *absent*. The exception is
  `crates/aether-session-index`, which sets `publish = false` explicitly.
- Every crate carries `dist = false` under `[package.metadata.dist]` except
  `crates/aether-sessions`, which has no `[package.metadata.dist]` section at
  all. (`dist-workspace.toml` only distributes `aether-agent-cli` anyway, so
  this distinction may not matter for a release decision but it is the
  literal manifest state.)
- The README crate table is missing entries whose `Cargo.toml` `name` field
  disagrees with the directory. `crates/acp-utils` is the prime example
  (`name = "aether-acp-utils"`).
- The other six omitted workspaces (`crates/mcp-utils`, `crates/llm-codegen`,
  `crates/utils`, `crates/doctest`, `crates/internal-evals`,
  `packages/aether-evals`) are out of scope for this part; parts 2/3 of
  TASK-2A-301 own them.

## Re-derive commands

```
sed -n '1,9p' crates/aether-session-index/Cargo.toml
sed -n '1,6p' crates/acp-utils/Cargo.toml
grep -nE '^(name|description|publish|dist|version) |^\[package.metadata.dist\]' crates/*/Cargo.toml | sort
git diff --stat 8e0ef7bf   # empty: no repo file changed
```
