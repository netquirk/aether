# Agent Instructions

For project context (purpose, stakeholders, problem, quality goals, constraints, out-of-scope, glossary, domain paths, open questions), see `.prairie/BRIEF.md`.

## Key Commands

- **Compile** -- use LSP tools, or `just check` if you must
- **Tests** -- `just test`
- **Lint** -- `just lint`
- **Format** -- `just fmt`

## Deployment

Production is reached through the project's **deploy contract**, `.prairie/deploy.toml`, which the prairie runner drives: `prairie deploy check` validates the contract, and the runner checks out one commit, performs the deploy, gates it on the contract's health checks and rolls back when they fail. The contract is the only place a deployment's specifics live; `.prairie/DEPLOY.md` records its shape and the rules below.

1. **Never ask for panel, registry or node access.** `prairie need-access` for the container panel (cp.netquirk.com), the registry (registry.netquirk.com) or a deploy host is always wrong: a run token is refused that access by design, and the contract's `target = "panel"` creates and upgrades the container itself through the deploy token the owner binds to the *deploy* (never to an agent run). The deploy's secrets are bound to the deploy for the same reason.
2. **Stop at the contract boundary.** When the contract or its recipe is missing, or a value in it is unknown, stop there and name the exact missing piece -- `image`, `panel_container`/`panel_node`, the `[[health]]` URL, a secret binding -- for the owner or the tracked deploy task to supply. Never invent a production identifier, never pick a secret name out of `prairie secrets`, and never fall back to manual steps (registry push, `docker`, SSH) that bypass the contract.
3. **Read the contract before planning a deploy.** `prairie deploy check` prints the verdict (`OK production, target ...`, `OFF`, or `ERROR`) and `prairie secrets` lists what this run may read. Report that output instead of asking for access.

Today `prairie deploy check` prints `OFF no .prairie/deploy.sh`: aether declares no deploy contract yet, so nothing here reaches production automatically. The rest is tracked on the board: `TASK-2B-1040` (the recipe), `TASK-2B-1041` (the contract), `TASK-26-143` (the deploy itself, whose health gate re-verifies `TASK-26-110` against the live system).

## Workflows 

1. When making code changes, rely on your LSP tools for compile/error checking (as its much faster) and run only targeted tests. Then, run other `just` verification checks only at the end. 
2. When nagivating or searching for code, prefer your LSP and AST Grep tools as they're faster.

## Coding Style

1. Backwards compatibility and fallbacks are not a concern when planning or implementing code.
2. Favor simplicity over complexity. Good architecture results in simple systems.
3. Non-doc (double slash) comments should be used extremely sparringly and only to explain the "why?" a certain piece of code exits. Comments should never be used for file separators.

### File organization

- Put `pub` sturcts, traits, functions, and types at the top of the file.
- Put private functions below `pub` constructs, near the bottom of the file.
- The reader's eye should flow from the most important, high-level things (top) to the less important nitty-gritty details (bottom). Example `eval_runner.rs` should have `pub struct EvalRunner` near the top

### Testing

1. Write `Fake` objects that mimic the _real_ behavior of the thing being faked using an in-memory implementation instead of producing side-effects. For example, a `FakeFilesystem` should work just like a real file system, but `write_file()` might write to a `HashMap` instead of the file-system.
2. When fixing a bug, always write test(s) first. Confirm the test(s) fail _before_ attempting to fix the bug.
3. Prefer integration tests that assert against state using fakes (e.g. in memory file system that asserts file contents) over mocks that test behavior (e.g. how many times did we call write_file?)
4. Don't put timeouts into tests, this always leads to flaky tests on CPUs with different speeds.
5. All tests must test _only_ the public API. Never write tests that test a private method as that couples the tests to implementation details and makes the code harder to refactor.
6. Tests should be easy to write and easy to read, meaning they aren't verbose. Use the test builder pattern (see: https://jmmv.dev/2020/12/builder-pattern-for-tests.html). Prefer using and extending existing builders vs creating new bespoke ones.

### Error handling

1. Never add `anyhow` or `color-eyere` as dependencies. Use standard Rust `enum`'s instead, e.g. `enum ApiError { ... }`
2. Prefer using specific enum types over `Box<dyn std::error::Error>` as the later makes it impossible for the caller to pattern match on specific errors.
3. Leverage `map`, `flat_map`, `and_then` etc to flatten nested `match` statements like `Ok(Ok(foo)) => {...}`.

## Workflows

1. Use `gh` (Github CLI) for all Github operations.
