# internal-sdk-version-check

The CI check that keeps the README's SDK version in step with the version the SDK package
publishes. It is not published and it is not part of the Aether binaries.

## The check

The README tells its reader which version of the `@aether-agent/sdk` package the docs describe.
The check compares that version with the version the package publishes:

```
just sdk-version-check
```

The check reads the `version` of `packages/aether-sdk/package.json` -- the version npm publishes --
and the version each README states. A README states it on the line that names the package and a
version, for example:

```
The `@aether-agent/sdk` package is published at version `1.2.3`.
```

The check reads that line from the repository's `README.md` and from the SDK package's
`packages/aether-sdk/README.md`. A README that states another version is a failure, and the failure
names both versions:

```
sdk-version-check: packages/aether-sdk/README.md: the README states SDK version '1.2.2', but the package publishes version '1.2.3'
```

The check exits with a non-zero status when it found one, so `just ci` fails on a README that
describes a version the package does not publish.

The check only reads files. It needs no network, no credential, no model provider and no deployed
service.

## The suite

`crates/internal-sdk-version-check/tests/sdk_version_check.rs` holds the tests of the check. They
prove that the check passes on the current workspace, that it fails on a workspace whose README
states another version, and that the failure names both versions.
