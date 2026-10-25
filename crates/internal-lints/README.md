# internal-lints

Repository lints that enforce the conventions of `AGENTS.md`. It is not published and it is not
part of the Aether binaries.

## The error-handling lint

`AGENTS.md` asks for concrete error enums: no `anyhow`, no `color-eyre`, and no
`Box<dyn std::error::Error>` in a public type. `just error-lint` runs the lint over the
repository:

```
just error-lint
```

The lint scans every crate of the Cargo workspace and every TypeScript package of the pnpm
workspace, and reports:

- the `anyhow` crate, wherever it is used or depended on,
- the `color-eyre` crate, wherever it is used or depended on,
- a `Box<dyn std::error::Error>` in a public type, meaning in the signature of an item a caller
  can reach.

It prints every path it read, every finding with its file, line and forbidden item, and the number
of findings per kind. It exits with a non-zero status when it found something, so `just ci` fails
on a crate that breaks the rule.

The lint only reads files. It needs no network, no credential, no model provider and no deployed
service.

## The private-item lint

`AGENTS.md` asks for tests that exercise only the public API: a test that reaches a private item
couples the test to the internals of the crate. `just private-lint` runs the lint over the
repository:

```
just private-lint
```

The lint scans every test function of the workspace and reports the private item of the crate under
test that the test reaches:

- a private function,
- a private method,
- a private field,
- a private type,
- a private constant.

A unit test may reach the private items of the module it is written in: those are the internals of
the module under test. The private items of the crate root are the internals of the whole crate,
and no test reaches them. An integration test lives outside the crate, so it may use the public API
of the crate only, and everything it declares itself.

It prints every finding with its file, line, test function, kind of item and offending expression,
how many test functions it scanned and how many it flagged. It exits with a non-zero status when it
found one, so `just ci` fails on a test that breaks the rule.

The lint only reads files. It needs no network, no credential, no model provider and no deployed
service.

## The fixed-wait lint

`AGENTS.md` asks for tests without timeouts: a fixed wait makes a test flaky on machines of
different speeds. `just wait-lint` runs the lint over the repository:

```
just wait-lint
```

The lint scans every `#[test]` and `#[tokio::test]` function of the workspace and reports the
wall-clock wait in a test body:

- a sleep (`sleep`, `sleep_until`, `sleep_after`),
- an explicitly bounded wait (`timeout`, `timeout_at`, `recv_timeout`, `recv_deadline`),
- a duration bound: a wait-shaped call (a name that starts with `wait` or `poll`) that takes a
  `Duration`.

It prints every finding with its file, line, test function and offending expression, how many test
functions it scanned and how many it flagged. It exits with a non-zero status when it found one, so
`just ci` fails on a test that breaks the rule.

Two kinds of test are left alone, because their wait is not a wall-clock wait:

- a test that runs on a fake clock (`#[tokio::test(start_paused = true)]`): its sleeps run on
  tokio's virtual clock,
- a test that drives a fake provider (its body names a `Fake`).

A test that waits on something it cannot fake (a language-server process, a queue) is named in
`wait-allowlist.txt`, next to this README. Every entry is `<path>::<test function>`, with the
reason the test cannot run on a fake clock.

The lint only reads files. It needs no network, no credential, no model provider and no deployed
service.
