# The CI gates the TASK-2K-259 run could not run

`TASK-2K-259` ("Decide whether the private-item lint reports the private items of the module a
test is written in") changed only `crates/internal-lints/` and `AGENTS.md`. Its own gates were
green, but its sandbox had no `just`, no `cargo nextest`, no `cargo sqlx` and no populated
`node_modules`, so five CI gates were never executed there. This file records the verdict of each
of them, run afterwards on a machine that has the missing tooling.

`scripts/verify-ci-gates.sh` runs the gates and writes one log per gate under
`test-results/ci-gates/`. `scripts/verify-ci-gates.sh --check` reads the verdict of every gate back
out of this file and, when the log of a gate is in the checkout, checks that the log agrees with
the verdict.

## The environment the gates ran on

| | |
|---|---|
| commit under test | `83371aa3` (the task base; `TASK-2K-259` is `4392425a` inside it) |
| `rustc` | 1.98.1 |
| `just` | 1.59.0 (installed into `~/.local/bin`) |
| `cargo-nextest` | 0.9.150 (installed into `~/.local/bin`) |
| `cargo-sqlx` | sqlx-cli 0.9.0 (installed into `~/.local/bin`; the version the workspace's `sqlx`
  dependency is pinned to) |
| `node` / `pnpm` | v24.21.0 / 10.28.0, `pnpm install --frozen-lockfile` run |
| container engine | **not available** - the sandbox's engine service did not come up, so the
  Docker-dependent target could not run (see below) |
| language server | `rust-analyzer 1.98.1`, started through the committed
  `scripts/provision-rust-analyzer.sh` wrapper |

## The verdicts

| # | Gate | Command | Verdict |
|---|------|---------|---------|
| 1 | `just doc-check` | `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --all-features --workspace --examples` | **green** (exit 0, no rustdoc warning) |
| 2 | `just doc-test` | `cargo test --doc --workspace --exclude internal-evals` | **green** (exit 0, 20 crates ran their doc tests, 21 doc tests in all, 0 failed) |
| 3 | `just sqlx-check` | `cargo sqlx migrate run` + `cargo sqlx prepare --check -- --features analytics` | **green** (exit 0) |
| 4a | `pnpm fmt-check` | `prettier --check "packages/**/*.{ts,mts,cts,mjs,cjs,js,jsx,json,astro,css}"` | **green** (exit 0) |
| 4b | `pnpm sdk:typecheck` | `pnpm --filter @aether-agent/sdk typecheck` | **green** (exit 0) |
| 4c | `pnpm sdk:test` | `pnpm --filter @aether-agent/sdk test` | **green** (exit 0, 11 test files, 83 tests) |
| 5a | `crates/aether-agent-cli --test integration` | `cargo test -p aether-agent-cli --test integration trace_context::headless_rejects_an_invalid_tracestate_when_telemetry_is_enabled` | **green** (1 passed, 0 failed) |
| 5b | `crates/aether-mcp-servers --test integration` (LSP) | `cargo test -p aether-mcp-servers --test integration lsp_diagnostics_e2e` and `lsp_operations_e2e::test_hover_falls_back_from_stale_line_hint` | **red - pre-existing**, see below |
| 5c | `crates/aether-evals --test docker_agent_tests` | `cargo test -p aether-evals --test docker_agent_tests` | **red - pre-existing** (the cause is the missing container engine; the target fails the same way at the base commit `e8d10e9f`), see below |

The per-gate logs are `test-results/ci-gates/*.log` (gitignored; published with
`prairie artifact`).

This record is for the tree under test (the task base `83371aa3` plus the commits of this report).
The project source has moved on since; see "The current source of the project" below.

## The verdicts, machine-readable

This is the block `scripts/verify-ci-gates.sh --check` reads. Every gate the task named has a
verdict: `green`, or `red-pre-existing` - the gate fails, and it fails the same way at the base
commit `e8d10e9f`, which the evidence in the sections below shows.

<!-- ci-gates:begin -->
doc-check                    green
doc-test                     green
sqlx-check                   green
pnpm-fmt-check               green
pnpm-sdk-typecheck           green
pnpm-sdk-test                green
cli-integration-trace-context green
mcp-lsp-diagnostics          red-pre-existing
mcp-lsp-hover                red-pre-existing
evals-docker-agent           red-pre-existing
<!-- ci-gates:end -->

## Gates 1 to 4: green

Every one of them exercises code `TASK-2K-259` did not touch, and all of them are green on the
tree that carries it:

- `just doc-check` builds the documentation of the whole workspace, private items included, with
  `-D warnings`: no warning, exit 0.
- `just doc-test` runs the doc tests of the whole workspace: 20 crates ran their doc tests, 21
  doc tests in all, 0 failed. The gate was run three times - the second and third time after this
  report was written, to confirm the result is not a one-off - and all three runs ended with exit 0
  (`test-results/ci-gates/doc-test.log`, `test-results/ci-gates/doc-test-rerun.log` and
  `test-results/ci-gates/doc-test-final.log`). The
  `aether-auth` crate, which the sandbox of `TASK-2K-259` could not document because `rmcp` was
  not built there, is included and passes.
- `just sqlx-check` migrates a fresh SQLite database and checks the prepared queries against the
  session-index schema with the `analytics` feature: the prepared metadata in
  `crates/aether-sessions/.sqlx` still matches.
- `pnpm fmt-check`, `pnpm sdk:typecheck` and `pnpm sdk:test` are green: the change touches no
  TypeScript. `pnpm sdk:test` needs the `aether` binary the SDK spawns; the gate was run with
  `AETHER_BIN` pointing at the build of `-p aether-agent-cli` (the same binary the `just sdk-test`
  recipe builds first), which is what makes the 83 tests pass.

## Gate 5: the environment-dependent targets

These are the three targets the `TASK-2K-259` sandbox saw fail because it had no Docker daemon and
no language server. No crate in the workspace depends on `internal-lints`, so the change under
test cannot have caused them; the question this task answers is whether they are green on a host
that has the two.

### 5c - the Docker-dependent target: red, and it is red at the base commit too

`cargo test -p aether-evals --test docker_agent_tests` needs a running container engine. This
sandbox was granted one (`.prairie/sandbox.toml` has `docker = true`), but the engine service did
not come up:

```
prairie-docker: the engine could not be started: this run's container engine did not come up
(unit prairie-engine-….service): Finished with result: success | Main processes terminated with:
code=exited, status=0/SUCCESS
```

`docker info` fails with that message and there is no `/var/run/docker.sock`. The two cases the
task names fail before they touch the code under test:

```
---- container_exec_shell_returns_non_zero_exit_codes stdout ----
Error: Container(Testcontainers(Client(Init(SocketNotFoundError("/run/prairie-engine/containers.sock"))))))

---- docker_agent_direct_agent_event_eval stdout ----
Error: Container(Testcontainers(Client(Init(SocketNotFoundError("/run/prairie-engine/containers.sock"))))))
```

`test result: FAILED. 0 passed; 2 failed` - both failures are the missing socket, not an
assertion. This is the environment, not the tree: the target cannot run in a sandbox whose engine
did not start, and `crates/aether-evals` is unchanged since the base commit `e8d10e9f`.

The same target fails the same way at the base commit. The test binary was run in the worktree of
the base commit `e8d10e9f` (whose `crates/aether-evals` is byte-identical to the tree under test),
and it gives the same two failures and the same error:

```
---- docker_agent_direct_agent_event_eval stdout ----
Error: Container(Testcontainers(Client(Init(SocketNotFoundError("/run/prairie-engine/containers.sock")))))

---- container_exec_shell_returns_non_zero_exit_codes stdout ----
Error: Container(Testcontainers(Client(Init(SocketNotFoundError("/run/prairie-engine/containers.sock")))))

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
```

So the target is red at the base commit too: it is a pre-existing failure of the base commit, and
the only thing missing to close it is a host whose container engine is up.

### 5a - the aether-agent-cli integration case: green

`trace_context::headless_rejects_an_invalid_tracestate_when_telemetry_is_enabled` was one of the
cases that failed in the `TASK-2K-259` sandbox. It passes here:

```
test trace_context::headless_rejects_an_invalid_tracestate_when_telemetry_is_enabled ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 184 filtered out
```

The sandbox failure was therefore environmental and not a regression.

### 5b - the aether-mcp-servers LSP e2e cases: red, and pre-existing

`crates/mcp-servers` and `crates/aether-lspd` are **byte-identical** between the base commit
`e8d10e9f` and the commit under test:

```
$ git log --oneline e8d10e9f..HEAD -- crates/mcp-servers crates/aether-lspd
(no output)
```

and no crate of the workspace depends on `internal-lints`, so the change under test cannot have
caused these failures. They are the failures this task was asked to record.

The same test binary, run in the worktree of the base commit `e8d10e9f`, fails the same way there:
`lsp_diagnostics_e2e` gives `test result: FAILED. 5 passed; 4 failed` and the hover case gives
`test result: FAILED. 0 passed; 1 failed` - the logs are
`test-results/ci-gates/mcp-lsp-diagnostics-base-commit.log` and
`test-results/ci-gates/mcp-lsp-hover-base-commit.log`. (The number of failing cases moves between
runs - 7 of 9 with the wrapper alone, 4 of 9 with the sources of the standard library - because
the analyzer has to type-check the temporary project inside the test's 30 s to 60 s window. The
failures are the same in kind, and they are there at the base commit.)

The daemon spawns the analyzer itself, so the analyzer has to come from a toolchain that ships both
`rust-analyzer` and `cargo`; the committed `scripts/provision-rust-analyzer.sh` wrapper does that
and is pointed at with `AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER`. With that wrapper alone the
analyzer never type-checks the crate graph, so the diagnostics store stays empty:

```
---- lsp_diagnostics_e2e::test_diagnostics_all_files_after_edit stdout ----
poll_lsp_tool(lsp_check_errors) timed out after 60s. Last result:
{"scope":"workspace","workspaceRoot":"/tmp/s/.tmpixi97P","diagnostics":[],
 "summary":{"errors":0,"warnings":0,"infos":0,"hints":0,"total":0}, …}
```

`test result: FAILED. 2 passed; 7 failed` for `lsp_diagnostics_e2e`, and
`lsp_operations_e2e::test_hover_falls_back_from_stale_line_hint` fails the same way.

The toolchain the analyzer needs is not installable here: the active toolchain (`1.98`) has no
`rust-src` and the toolchain directory is read-only (`rustup component add rust-src` ->
"component download failed for rust-src: error opening file for download: cleaning up cached
downloads: Read-only file system"). The `stable` toolchain in the same rustup home does carry the
sources, so the run was repeated with

```
RUST_SRC_PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library"
```

which is the second attempt `TASK-2J-392` recorded. It loads the crate graph, but four of the
nine cases still time out (`test result: FAILED. 5 passed; 4 failed`) and the hover case still
fails. The remaining failures are the ones `TASK-2K-333` and its follow-up `TASK-2K-656` are
already on the board for.

The logs are `test-results/ci-gates/mcp-lsp-diagnostics.log`,
`test-results/ci-gates/mcp-lsp-diagnostics-rust-src.log`,
`test-results/ci-gates/mcp-lsp-hover.log`, `test-results/ci-gates/mcp-lsp-hover-rust-src.log` and
the two `-base-commit.log` logs of the base-commit runs.

## The reason the change cannot have broken any of them

`TASK-2K-259` touched `crates/internal-lints/{README.md, src/bin/private-lint.rs,
src/private_scan.rs, tests/private_lint.rs, tests/fixtures/module-crate/}` and `AGENTS.md`. No
crate of the workspace depends on `internal-lints` (it is a `workspace = true` member that
nothing lists as a dependency), so the change is invisible to every other crate:

```
$ grep -rn "internal-lints" --include=Cargo.toml . | grep -v "^./crates/internal-lints/Cargo.toml"
(no output)
```

`crates/aether-auth`, the crate whose doc test the `TASK-2K-259` sandbox could not build, is
byte-identical between the base commit `e8d10e9f` and the commit under test:

```
$ git diff --quiet e8d10e9f..HEAD -- crates/aether-auth && echo IDENTICAL
IDENTICAL
```

## The current source of the project

The record above is for the tree under test: the task base `83371aa3` plus the commits of this
report. The project source has since moved on to `50d4fa28`, which contains the `TASK-2K-333` fix
(`e54268fe`): the daemon now removes the inherited `RUSTUP_TOOLCHAIN` before it starts a language
server, which is the cause of the hover failure recorded in 5b. The hover target is therefore
expected to be green on the current source; this report does not claim that, because the hover run
of this record was made on the tree under test, where the fix is not in yet.

`crates/mcp-servers`, `crates/aether-evals` and `crates/aether-auth` are byte-identical between
`e8d10e9f` and `50d4fa28`. The only change to a crate these gates touch is
`crates/aether-lspd/src/process_transport.rs` of that fix, and it does not touch the diagnostics
module: the analyzer of this sandbox still never type-checks the temporary project, so the
diagnostics steps stay red exactly as documented in 5b. That module is the scope of the open
`TASK-2K-656`.

## Reproducing

```
# the tooling the sandbox was missing
cargo install sqlx-cli --version 0.9.0 --locked --no-default-features --features sqlite,sqlx-toml --root ~/.local
# just 1.59.0 and cargo-nextest 0.9.150 are prebuilt binaries; see scripts/verify-ci-gates.sh
pnpm install --frozen-lockfile

export PATH="$HOME/.local/bin:$PATH"
scripts/verify-ci-gates.sh          # runs the gates, writes test-results/ci-gates/*.log
scripts/verify-ci-gates.sh --check  # checks the recorded verdicts and the evidence
```

The language-server-dependent targets need the analyzer wrapper:

```
export AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER="$PWD/scripts/provision-rust-analyzer.sh"
```

## What is still open

One piece of the task could not be closed here: the Docker-dependent target of gate 5c needs a
container engine that this sandbox never got (the engine service did not come up). It has to be
run on a host whose engine is up. The run that does it must end with

```
cargo test -p aether-evals --test docker_agent_tests    # exits 0, "test result: ok. 2 passed; 0 failed"
```

`crates/aether-evals` is byte-identical between the base commit `e8d10e9f` and the commit under
test, and the target fails the same way at the base commit (see 5c), so the target is expected to be
green on a host with an engine; the two failures recorded here are the missing socket and nothing
else. The other environment-dependent targets that the task named are settled: the
aether-agent-cli one is green (5a), and the aether-mcp-servers LSP ones are red for a reason that
predates the change (5b) and are already on the board as `TASK-2K-333` and `TASK-2K-656`.

The board refused to let this run file that follow-up itself (`prairie create` and
`prairie split --rest` both answer: "this run's task is itself 3 generations of agent-created tasks
deep; agent-created work may not keep spawning more. Report what you need in your final message and
a person will file it."), so a person has to file it. The prompt of that follow-up is:

> Run `cargo test -p aether-evals --test docker_agent_tests` on a host whose container engine is up
> and record the result. The target needs a running container engine; a sandbox whose engine service
> did not come up fails it with
> `Container(Testcontainers(Client(Init(SocketNotFoundError("/run/prairie-engine/containers.sock"))))))`,
> which is what this run recorded (see 5c). The run must end with
> `test result: ok. 2 passed; 0 failed`. `crates/aether-evals` is unchanged since `e8d10e9f`, so
> nothing in the change under test can make it fail.
