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
