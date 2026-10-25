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

A reach is a reach, whatever form it takes: the lint reports the call, the method call, the field
access, the struct literal and the `use` declaration that imports the private item. A test that
writes `use my_crate::private_helper;` reaches the private helper, even when it never calls it.

The rule it enforces is about the module the private item belongs to:

- a unit test may reach the private items of the module it is written in and of the modules it is
  nested in: those are the internals of the module under test, and they are refactored together
  with the test that uses them,
- it may not reach the private items of the crate root, which are the internals of the whole crate,
  nor the private items of another module, which are the internals of that module: a test that
  reaches them keeps depending on them when the module under test is refactored,
- an integration test lives outside the crate, so it may use the public API of the crate only, and
  everything it declares itself.

It prints every finding with its file, line, test function, kind of item, the module that declares
the item and the offending expression, how many test functions it scanned and how many it flagged.
It exits with a non-zero status when it found one, so `just ci` fails on a test that breaks the
rule.

The lint only reads files. It needs no network, no credential, no model provider and no deployed
service.

### The decision on the private items of the module a test is written in

`TASK-2K-259` asked whether the rule should also report the private items of the module a test is
written in, so that every private item a test reaches is reported. The decision is **no**: the
carve-out stands, and the rule is the one written above. The Technical Advisor was asked four times
over the course of the task and could not be reached (the host had no memory for another sandbox),
so the decision rests on the measurements below.

What the change would cost, measured on this repository with the carve-out removed (the tree is
clean today: the lint reports nothing):

| The private item the test reaches | Reaches | Items | Files | Crates |
|------------------------------------|---------|-------|-------|--------|
| the module the test is written in | 1,467 | 273 | 93 | 13 |
| a module the test is nested in | 419 | 103 | 60 | 12 |

Together: 1,886 reaches over 376 distinct items in 123 files of 13 crates.

Every one of the 1,467 reaches of the module a test is written in is a helper declared inside the
same `#[cfg(test)] mod tests` block that uses it (`build_credential` in
`crates/aether-auth/src/credential.rs`, `temp_store` in `crates/aether-auth/src/encrypted_file.rs`);
not one of them is a production item. The remedy the rule prescribes - `pub(crate)` - would mark 273
test helpers crate-visible, in modules that exist only in a test build.

The other 103 items are the production internals of the module under test (`PendingToolCall` in
`crates/llm/src/providers/bedrock/streaming.rs`, the private fields `host` and `warnings` in
`crates/wisp/src/surfaces/modal/url.rs`); not one of them lives in a test module. They would become
crate-visible: the internal API of the crate grows and the module boundaries soften, which works
against the "Modularity" goal of the brief, for no gain in the quality of the tests.

The gain would be small in any case. `pub(crate)` is not the public API, so a test that reaches a
`pub(crate)` helper still depends on that helper by name and still breaks when the helper changes:
the change relabels 376 items without decoupling a single test. The reaches the lint already
reports are the ones that survive a refactor of the module under test. The reaches the carve-out
allows are refactored together with the test that uses them, which is the coupling `AGENTS.md` rule
5 warns about only when it crosses a module boundary.

What would change the answer: a rule that lets a test reach only `pub` items would really enforce
`AGENTS.md` rule 5, because it would push every test into the `tests` directory of its crate.
That is a different rule, a much larger change, and not this one.

The decision is pinned by `the_private_items_of_the_module_a_test_is_written_in_are_not_reported` in
`tests/private_lint.rs`, which runs against the `module-crate` fixture: the fixture holds the four
cases - the module a test is written in, a module it is nested in, a sibling module and the crate
root - and the lint reports the last two only.

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
