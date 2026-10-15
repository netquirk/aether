# TASK-2A-324 — name -> path inventory: the remaining six README-omitted members

Inventory of six workspace members the README's crate / package tables omit:
five Rust crates under `crates/` and one TypeScript package under
`packages/`. Each row is anchored to a `<manifest>:<line>` citation that I
read directly at base commit `30aa13b8`. No `name`, description or flag below
is inferred from the directory name.

This is **part 2 of 3** of TASK-2A-301 and, on its own, **completes the
full 12-member omitted list** quoted in the brief's Open questions: part 1
(`aether-omitted-crates-1.md`) covered the first six; this file covers the
remaining six; the closing section below names all twelve together.

Base commit verified: `30aa13b8 TASK-2A-323: Inventory the first six README-omitted crates under crates/`
(`git status --porcelain` is empty; no repo file is changed by this task.)

## Flag semantics used in the table

- **`publish`** — value of the `[package]` `publish` key. Cargo's default (key
  absent) is "publishable". A `publish` key with no value is invalid; valid
  values are `true`, `false`, or a list of registries. Four of the five
  Rust manifests below OMIT the `publish` key entirely. (`crates/doctest`
  and `crates/internal-evals` set it to `false`.)
- **`dist`** — value of `[package.metadata.dist] dist = …`. `dist-workspace.toml`
  only ever distributes `aether-agent-cli` (`packages = ["aether-agent-cli"]`),
  so `dist = false` on every workspace member is an explicit opt-out.
- **npm package** — there is no Cargo `publish` / `dist`. The analogues are
  `"private": true` (absent here) and the `"files"` array (`["dist",
  "README.md"]`). Recording "key absent" for the npm row is the literal
  `package.json` state, not an inferred publishability claim.

## Table — `name -> path` with manifest citations

| # | Directory (`path`)              | Package `name` (manifest line)              | `description` (manifest line)                                                                          | `publish`                              | `dist`                          | Notes from manifest                                                                                                                                                                                  |
|---|---------------------------------|---------------------------------------------|--------------------------------------------------------------------------------------------------------|----------------------------------------|---------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1 | `crates/mcp-utils/`             | `aether-mcp-utils` (`Cargo.toml:2`)         | "MCP client and server utilities for the Aether AI agent framework" (`:5`)                              | key absent → publishable by default   | `dist = false` (`:12–13`)       | `version = "0.7.4"` (`:3`); lib `mcp_utils` (`:16`); features `default = ["client"]` (`:27`), `testing` (`:28`), `client` (`:29–37`)                                                                  |
| 2 | `crates/llm-codegen/`           | `aether-llm-codegen` (`Cargo.toml:2`)       | "Code generation utilities for the Aether LLM provider layer" (`:5`)                                    | key absent → publishable by default   | `dist = false` (`:12–13`)       | `version = "0.2.25"` (`:3`); lib `llm_codegen` (`:16`)                                                                                                                                               |
| 3 | `crates/utils/`                 | `aether-utils` (`Cargo.toml:2`)             | "Shared utilities for the Aether AI agent framework" (`:5`)                                             | key absent → publishable by default   | `dist = false` (`:12–13`)       | `version = "0.2.16"` (`:3`); lib `utils` (`:16`); cross-cited as `package = "aether-utils"` from `crates/mcp-utils/Cargo.toml:47` and `crates/llm-codegen/Cargo.toml:34`                            |
| 4 | `crates/doctest/`               | `aether-doctest` (`Cargo.toml:2`)           | "Test helpers for validating fenced examples embedded in Aether's `src/docs/*.md` files" (`:5`)         | `publish = false` (`:8`)              | `dist = false` (`:10–11`)       | `version = "0.1.0"` (`:3`); lib `aether_doctest` (`:14`); no `readme` / `keywords` / `categories` keys                                                                                               |
| 5 | `crates/internal-evals/`        | **`internal-evals`** (`Cargo.toml:2`)       | **none** — `[package]` table at `:1–5` jumps from `edition` (`:4`) straight to `publish = false` (`:5`) | `publish = false` (`:5`)              | `dist = false` (`:7–8`)         | **TRAP A**: no `aether-` prefix (directory *and* package name are both bare `internal-evals`); `version = "0.1.0"` (`:3`); no `[lib]` section (default `src/lib.rs`)                                |
| 6 | `packages/aether-evals/`        | `@aether-agent/evals` (`package.json:2`)    | "Evaluation harness for the Aether agent SDK" (`package.json:4`)                                         | npm: no `"private": true`; `"files": ["dist", "README.md"]` (`package.json:17–20`) | n/a (not a Cargo crate)         | `version = "0.4.1"` (`package.json:3`); ESM (`"type": "module"`, `package.json:10`); the only omitted **TypeScript** member, so citations are JSON line numbers                                              |

## Verification of the three traps called out by the task

### Trap A — `crates/internal-evals` is `internal-evals`, not `aether-internal-evals`

`crates/internal-evals/Cargo.toml:1–5`:

```
[package]
name = "internal-evals"
version = "0.1.0"
edition = "2024"
publish = false
```

Every other Rust member in this batch carries the `aether-` prefix. The
directory is `crates/internal-evals/` and the manifest `name` field is the
bare `internal-evals` — do not "correct" it to match the directory-vs-name
pattern of `crates/acp-utils` (where directory `acp-utils` ⇄ package
`aether-acp-utils`, see `aether-omitted-crates-1.md`).

Cross-citation — a consumer in the same workspace refers to it by the
literal name:

`crates/internal-evals/Cargo.toml:14` (the `aether-cli` line is a separate
package, included here only to show that the convention in this file is
`package = "<published-name>"` whenever a path-only dep has a different
published name):

```
aether-cli = { package = "aether-agent-cli", path = "../aether-cli" }
```

The corresponding `internal-evals` line is a plain path dep at `:15`
(`aether-evals = { path = "../aether-evals" }`) — there is no `package =`
override because the directory name already matches the published name.

### Trap B — `crates/internal-evals` has no `description`

`crates/internal-evals/Cargo.toml:1–5` (same snippet as Trap A) shows the
fields jumping from `edition` (`:4`) to `publish = false` (`:5`). There is
no `description = "…"` line in `[package]`. The manifest literally does
not state a description; this is not the place to invent one.

`crates/aether-session-index` in part 1 is the analogous case (also
`publish = false` AND no description — see `aether-omitted-crates-1.md`).

### Trap C — two different "evals" members

There are **two** members whose directory leaf is `aether-evals` and they
publish under different names:

- **Rust** — `crates/aether-evals/` is the crate the README's crate table
  *does* list (README.md:185):

  ```
  | [`aether-evals`](crates/aether-evals) | Automated testing (evals) for LLM agents ([docs](https://docs.rs/aether-evals)) |
  ```

  Part 1 of this inventory treats it as already-listed; it is **not** in
  the omitted set.
- **TypeScript** — `packages/aether-evals/` is the *separate* npm package
  that the README's TypeScript packages table omits (README.md:193–194 list
  only `@aether-agent/sdk` and `@aether-agent/website`):

  ```
  | [`@aether-agent/sdk`](packages/aether-sdk)     | TypeScript SDK for building agents programmatically |
  | [`@aether-agent/website`](packages/website)   | Documentation website built with Astro              |
  ```

  Cross-citation — the root `package.json` wires the workspace to the
  published name `@aether-agent/evals` (not the directory leaf
  `aether-evals`):

  `package.json:10–12`:

  ```
  "evals:build": "pnpm --filter @aether-agent/evals build",
  "evals:typecheck": "pnpm --filter @aether-agent/evals typecheck",
  "evals:test": "pnpm --filter @aether-agent/evals test",
  ```

  And `packages/aether-evals/package.json:2` confirms the published name:
  `"name": "@aether-agent/evals"`.

## Completing the full 12-member omitted list

Combined with `aether-omitted-crates-1.md`, every member the brief's Open
questions names now has a `name -> path` row cited to its manifest. The
twelve in directory-leaf order, with the source of each `name` citation:

| #  | Directory (`path`)             | Published `name`    | Cited in                  |
|----|--------------------------------|---------------------|---------------------------|
| 1  | `crates/aether-sessions/`      | `aether-sessions`   | part 1                    |
| 2  | `crates/aether-session-index/` | `aether-session-index` | part 1                  |
| 3  | `crates/acp-utils/`            | `aether-acp-utils`  | part 1 (Trap 1)           |
| 4  | `crates/aether-auth/`          | `aether-auth`       | part 1                    |
| 5  | `crates/aether-schemas/`       | `aether-schemas`    | part 1                    |
| 6  | `crates/aether-telemetry/`     | `aether-telemetry`  | part 1                    |
| 7  | `crates/mcp-utils/`            | `aether-mcp-utils`  | this file, row 1          |
| 8  | `crates/llm-codegen/`          | `aether-llm-codegen`| this file, row 2          |
| 9  | `crates/utils/`                | `aether-utils`      | this file, row 3          |
| 10 | `crates/doctest/`              | `aether-doctest`    | this file, row 4          |
| 11 | `crates/internal-evals/`       | `internal-evals`    | this file, row 5 (Trap A) |
| 12 | `packages/aether-evals/`       | `@aether-agent/evals` | this file, row 6 (Trap C) |

This is the 12-member list the brief's Open question asks about; both
sibling inventory files are now in place to feed the parent decision
(TASK-2A-301) without further file lookups.

## Notes that may matter to the parent decision

- Four of the five Rust crates in this batch OMIT the `publish` key, so
  they are publishable by Cargo's default. Recording them as `publish = true`
  would be wrong; the literal manifest field is *absent*. The exceptions
  are `crates/doctest` and `crates/internal-evals`, which set
  `publish = false` explicitly.
- Every Rust crate in this batch carries `dist = false` under
  `[package.metadata.dist]`. `dist-workspace.toml` only distributes
  `aether-agent-cli` anyway, so this distinction may not matter for a
  release decision but it is the literal manifest state.
- The npm package `packages/aether-evals` has **no** `"private": true`
  field and an explicit `"files"` whitelist of `["dist", "README.md"]`
  (`package.json:17–20`). It is therefore packagable by npm's default;
  the README package table omits it despite that.
- The `internal-evals` name is the only one in the full 12-member set
  without the `aether-` prefix. Anyone mapping "directory -> README row"
  has to special-case it.
- The brief's "Domain paths" question is now fully answered *in evidence*:
  every omitted member's published name, description, publish flag, dist
  flag and lib symbol are cited above; the decision step (which row, if
  any, to add to the README) is a separate part of TASK-2A-301 and is
  intentionally not done here.

## Re-derive commands

```
sed -n '1,30p' crates/mcp-utils/Cargo.toml
sed -n '1,20p' crates/llm-codegen/Cargo.toml
sed -n '1,20p' crates/utils/Cargo.toml
sed -n '1,16p' crates/doctest/Cargo.toml
sed -n '1,12p' crates/internal-evals/Cargo.toml
sed -n '1,20p' packages/aether-evals/package.json
grep -n 'package = "aether-utils"' crates/mcp-utils/Cargo.toml crates/llm-codegen/Cargo.toml
grep -nE 'evals:' package.json
git diff --stat 30aa13b8   # only the new aether-omitted-crates-2.md appears
```
