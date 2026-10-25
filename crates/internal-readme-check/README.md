# internal-readme-check

The CI check that keeps the README's provider list in step with the providers the harness
registers. It is not published and it is not part of the Aether binaries.

## The check

The README promises a set of LLM providers "supported out of the box". The check reads that list
and compares it with the providers the harness registers:

```
just readme-check
```

The check reads the `**Your Model**` bullet of the root `README.md` and takes the names it lists.
A name matches a provider when it is the provider's display name or the name a user writes in a
`provider:model` string, compared without regard to case and punctuation: `Llama.cpp` is the
`LlamaCpp` provider and `Microsoft Foundry` is the `azure-foundry` provider.

The comparison is one-way. A provider the harness registers but the README does not name is fine
-- the README names the providers that work out of the box. A provider the README names that the
harness does not register is a failure, and the failure names it:

```
readme-check: the README names provider 'Nonexistent Provider', which the harness does not register
```

The check exits with a non-zero status when it found one, so `just ci` fails on a README that
advertises a provider that does not exist.

The check only reads files. It needs no network, no credential, no model provider and no deployed
service.

## The suite

`crates/internal-readme-check/tests/readme_check.rs` holds the tests of the check. They prove that
the check passes on the current README, that it fails on a README that names a provider the
harness does not register, and that the failure names that provider.
