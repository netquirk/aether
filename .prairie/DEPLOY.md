# How aether reaches production

Aether is deployed by the **prairie runner** through the project's deploy contract, not by hand. The project declares *what* to deploy in `.prairie/deploy.toml`; the runner ships one commit, performs the deploy, gates it on the contract's health checks and rolls back when they fail. The contract is the only place a deployment's specifics live -- the runner itself names no product, no registry and no host.

## The contract is the deploy path

- `prairie deploy check` prints the verdict for the contract in the working tree and runs nothing: `OK production, target ...` when the contract is complete, `OFF` when there is none, `ERROR` naming the offending key when the runner would refuse it.
- The runner checks the commit out into a clean directory (no uncommitted or ignored files) and works from that checkout, so the contract -- and everything it names -- has to be committed.
- A production contract needs a person's approval per commit, and the deploy's secrets are bound to this project's deploys, never to an agent run.

## The `target = "panel"` contract

```toml
production = true
target = "panel"
image = "registry.netquirk.com/<org>/<app>"   # the registry the panel pulls from
secrets = ["CP_API_KEY"]                      # bound to this project's deploys
panel_container = "<container id>"            # or the container's name ...
panel_node = "<node>"                         # ... together with the node it runs on

[[health]]
url = "https://<host>/healthz"
timeout = "60s"
```

- `image` is the registry path the panel pulls from; the runner builds and pushes the new tag itself. It has to start with `registry.netquirk.com/` -- the panel only pulls Netquirk's registry.
- `secrets` names the secrets the **deploy** consumes. For the panel target the runner passes `CP_API_KEY` to the panel as a bearer token, never as an environment variable.
- `panel_container` is the container id, or the container's name together with `panel_node`: a name without its node is refused.
- The health URL has to be an absolute http(s) URL, and a production deploy needs at least one `[[health]]` check.
- `state` and `rollback` are rejected on this target: the panel API is its own rollback. The runner reads the running image tag from the panel before the build, so a failed health check has a known good tag to step back to.

The sequence the runner performs: build the image in the project's sandbox, push the new tag, ask the panel to upgrade the container, poll the panel task, wait for every `[[health]]` check, keep checking for `health_window`, and on failure ask the panel to upgrade back to the prior tag and re-check.

`.prairie/deploy.toml.template` is this shape as a fill-in file. Its `OWNER` placeholders are deliberately not valid values, so `prairie deploy check` refuses the file until every one of them is filled in and a copied template can never be deployed as it stands. Those values -- the registry path, the container and the node it runs on, the health URL -- are the owner's to set; an agent must not invent them.

## The `target = "host-script"` contract: the release store and the rollback target

This project ships the `host-script` contract: `.prairie/deploy.sh` installs the binary the build produced, `.prairie/deploy-state.sh` names the binary that is running, and `.prairie/rollback.sh` puts the previous one back when the health check fails. The runner drives all three on the host; a run does not execute them there.

| what | where it comes from | default |
|---|---|---|
| the install path the deploy writes | `$AETHER_BIN` | `$HOME/.cargo/bin/aether` |
| the release store | `$AETHER_RELEASES` | `$HOME/.local/lib/aether/releases` |
| the binary to restore | `$PRAIRIE_PRIOR_STATE`, the sha256 the state script printed | required |

The store keeps **one copy per sha256**, at `<store>/<digest>/aether`, and the install path is a symlink into it. The deploy preserves the binary it replaces under its own digest before it switches the install path, so a rollback target is found by digest alone: the runner reads that digest from the state script and hands it to the rollback script as `$PRAIRIE_PRIOR_STATE`.

The scripts do not take a rollback target on trust:

- `deploy.sh` checks that the copy it preserved hashes to the digest it is stored under, and ends by naming the store, the rollback target it preserved and the artifact it installed. When there is no running binary to preserve (a missing or dangling install path) it says so instead of pretending a rollback target exists.
- `rollback.sh` checks that the stored copy hashes to `$PRAIRIE_PRIOR_STATE` before it installs it. A target that is not in the store is refused with the exact path that is missing and the entries the store does hold; an entry that does not match its digest is refused as well.
- `deploy-state.sh` prints the digest of the running binary and nothing else (the runner reads its stdout as the prior state); when the install path is missing or a dangling symlink it says so on stderr and fails, because a prior state that cannot be read is a deploy that cannot be rolled back.

`scripts/verify-deploy-contract.sh` exercises the three scripts against a store -- including a round trip with the real deployed binary -- and `scripts/check-deploy-access-rules.sh` keeps the rules in this file intact.

### What the host has, and what a rollback can resolve to

Checked on the runner host on 2026-11-16, from a run's sandbox:

| path | what it is |
|---|---|
| `/home/prairie/.local/lib/prairie/bin/aether` | the runner binary that is live: `aether 0.9.21`, sha256 `aa39f08f148bf807703d9ea328b4b3c14cbdd7cfdabbd8a6c5dac3e605803ec3` |
| `/home/prairie/.local/lib/aether/releases/aa39f08f148bf807703d9ea328b4b3c14cbdd7cfdabbd8a6c5dac3e605803ec3/aether` | the same file on the host: the sandbox mounts the runner binary from there, so the host's release store is `/home/prairie/.local/lib/aether/releases` -- the contract's default store, with `HOME=/home/prairie` |
| `/home/prairie/.local/lib/aether/releases` | not readable from a run's sandbox: only the resolved runner binary is mounted, so a run cannot list the store |
| `/home/prairie/.cargo/bin/aether` | the install path the contract writes: a symlink to `/home/prairie/.local/lib/aether/releases/2b1dd335fa38c0d6e9f04cda700df93b490902e92915059207f76fb49e2ea6e9/aether` (created 2026-11-09), dangling inside a sandbox because the store is not mounted |
| `$HOME/.local/lib/aether/releases` | the default store of a *run's* home (`/home/agent/.local/lib/aether/releases`): it does not exist |

Two digests have been named as the 0.9.5 rollback target, and neither is a file that exists:

- `d2e5e187d20dd8c3c70beb4e47d62a5aac892508e4856476ed097627cf0ee413` -- the operator's hand-off. No file with that digest is reachable, and it is not the digest this project recorded for the 0.9.5 binary.
- `b7b7055f46895dc86a59d3a3813650ef57c7a9433d90139370edae88e2663a86` -- the digest recorded before the upgrade (`TASK-33-6`, commit `a382446c`). That commit also records that the release root "had been wiped" and that the 0.9.5 was copied back into the store *of the run that did it* -- a sandbox store, which does not outlive the run.

So a rollback to the 0.9.5 is not something the contract can resolve today: there is no 0.9.5 binary in any store or cache that is reachable from this host, and every aether binary that is reachable reports `aether 0.9.21`. **A rollback would have to rebuild the previous commit, not swap a binary.**

For the contract to hold at the next install:

1. the deploy must preserve the binary it replaces into the store before it switches the install path -- the recipe does this, and now verifies the preserved copy's digest;
2. the store must not be wiped between the deploy and the rollback: the wipe of 2026-11-13 is what left the install path dangling;
3. the hand-off must name the rollback target the deploy printed (`deploy: rollback target <path> (<digest>)`), not a digest that was never written to the store.

## What an agent does, and does not do

- **Does not** request panel, registry or node access (`prairie need-access`): a run token is refused that access by design. The deploy token bound to the deploy is what talks to the panel and the registry.
- **Does not** put the deploy to a person: it does not file a decision card, a task, a comment or an access request asking which target production is, which host or node it runs on, which registry it pulls from, or for panel, registry or node access. The contract's `target` names the hosting target and the runner drives it; the owner's values and the deploy's secrets are the owner's and the deploy's to supply.
- **Does not** bypass the contract: a deploy runs through the project's deploy path (`prairie deploy`, which the contract drives), never as hand-rolled `docker push`, `docker run` or SSH steps.
- **Does not** pick a secret name out of `prairie secrets`, and never asks for a secret that is bound to the deploy.
- **Does** run `prairie deploy check` and report its verdict, and name the exact missing key when the contract is incomplete -- `image`, `panel_container`/`panel_node`, the `[[health]]` URL, a secret binding -- in the run's own report, so the owner or the tracked task can supply it.

## Status today

`prairie deploy check` prints `OK production, target host-script, 1 health check, rollback .prairie/rollback.sh`: the project ships the `host-script` contract, so the runner installs the binary the build produced, gates it on the contract's `[[health]]` check and rolls back with `.prairie/rollback.sh`. What the contract does not decide -- the hosting target, the secrets and the deploy host -- belongs to the owner, and a run does not ask for it: not a recipe and not a deploy host. The panel shape in `.prairie/deploy.toml.template` stays the fill-in file for a `target = "panel"` contract, which takes no `state` and no `rollback`: the runner refuses both on that target.
