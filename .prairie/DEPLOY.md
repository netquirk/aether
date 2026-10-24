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
- `panel_container` is the container id, or the container's name together with the node it runs on (`panel_node`): a name without its node is refused.
- The health URL has to be an absolute http(s) URL, and a production deploy needs at least one `[[health]]` check.
- `state` and `rollback` are rejected on this target: the panel API is its own rollback. The runner reads the running image tag from the panel before the build, so a failed health check has a known good tag to step back to.

The sequence the runner performs: build the image in the project's sandbox, push the new tag, ask the panel to upgrade the container, poll the panel task, wait for every `[[health]]` check, keep checking for `health_window`, and on failure ask the panel to upgrade back to the prior tag and re-check.

`.prairie/deploy.toml.template` is this shape as a fill-in file. Its `OWNER` placeholders are deliberately not valid values, so `prairie deploy check` refuses the file until every one of them is filled in and a copied template can never be deployed as it stands. Those values -- the registry path, the container and the node it runs on, the health URL -- are the owner's to set; an agent must not invent them.

## What an agent does, and does not do

- **Does not** request panel, registry or node access (`prairie need-access`): a run token is refused that access by design. The deploy token bound to the deploy is what talks to the panel and the registry.
- **Does not** bypass the contract: a deploy runs through the project's deploy path (`prairie deploy`, which the contract drives), never as hand-rolled `docker push`, `docker run` or SSH steps.
- **Does not** pick a secret name out of `prairie secrets`, and never asks for a secret that is bound to the deploy.
- **Does** run `prairie deploy check` and report its verdict, and name the exact missing key when the contract is incomplete -- `image`, `panel_container`/`panel_node`, the `[[health]]` URL, a secret binding -- so the owner or the tracked task can supply it.

## Status today

`prairie deploy check` prints `OFF no .prairie/deploy.sh`: aether declares no deploy contract, so nothing here reaches production automatically. A `target = "panel"` contract needs no `.prairie/deploy.sh` -- the runner drives the panel itself -- while the recipe scripts belong to the `host-script` target. The rest is tracked on the board:

- `TASK-2B-1040` -- commit the executable recipe and its state/rollback helpers.
- `TASK-2B-1041` -- declare `.prairie/deploy.toml` so `prairie deploy check` returns OK.
- `TASK-26-143` -- run the production deploy and verify the live criteria; the deploy's health gate re-verifies `TASK-26-110` against the live system.
