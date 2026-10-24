# Options: should aether's brief name a production hosting target?

The open question in `.prairie/BRIEF.md` (line 95):

> Deployment/hosting: the board still carries open work to deploy aether to production and bind its
> production secrets (`TASK-26-143`, `TASK-27-488`) while `## Constraints`/`## Out of scope` name no
> hosting target; should the brief state one?

This note holds the option analysis for the one decision card raised by `TASK-2C-890`. The brief
itself is **not** changed here: the answer is written back into `.prairie/BRIEF.md` by its own task.

## 1. What the brief says today

`BRIEF.md:42` (the only deployment line under `## Constraints`):

> Production is reached through the project's deploy contract (`.prairie/deploy.toml`), which the
> prairie runner drives; a run is never given panel, registry or node access, because the deploy
> token bound to the deploy creates and upgrades the container itself (`.prairie/DEPLOY.md`).

`## Out of scope` (BRIEF.md:46-53) says nothing about hosting or deployment.

So the brief names the **mechanism** (the prairie deploy contract) and even names the panel/registry
in the negative ("a run is never given panel, registry or node access", "creates and upgrades the
container"), but it never names a **target**: no `target = "..."`, no registry, no host, no health
URL. That is the gap the open question points at.

## 2. What the rest of the repository already commits to

| Source | What it states |
| --- | --- |
| `AGENTS.md:12-20` (`## Deployment`) | "Production is reached through the project's **deploy contract**, `.prairie/deploy.toml`, which the prairie runner drives"; rule 1 names `target = "panel"` explicitly: "the contract's `target = \"panel\"` creates and upgrades the container itself through the deploy token the owner binds to the *deploy* (never to an agent run)". `AGENTS.md:20` records today's verdict: "`prairie deploy check` prints `OFF no .prairie/deploy.sh`: aether declares no deploy contract yet". |
| `.prairie/DEPLOY.md:3` | "Aether is deployed by the **prairie runner** through the project's deploy contract, not by hand. The project declares *what* to deploy in `.prairie/deploy.toml`". |
| `.prairie/DEPLOY.md:11-32` | Documents **the `target = "panel"` contract** as the shape: `image = "registry.netquirk.com/<org>/<app>"`, `panel_container`/`panel_node`, `secrets = ["CP_API_KEY"]`, one `[[health]]` block, `state`/`rollback` **rejected** on this target ("the panel API is its own rollback", DEPLOY.md:30). |
| `.prairie/deploy.toml.template` | Is already a **panel** contract: `production = true`, `target = "panel"` (line 19), `image = "registry.netquirk.com/OWNER/aether"` (23), `panel_container`/`panel_node` (27-28), `secrets = ["CP_API_KEY"]` (32), one `[[health]]` URL (36). Only the `OWNER` values are missing; the shape is decided. |
| `prairie deploy --help` | The runner's closed target set is `bunny-mc`, `panel`, `static`, `host-script`, `edge`. `target = "panel"`: "the runner drives a cp.netquirk.com container update through the panel API ... **No `.prairie/deploy.sh` is required or run**". `target = "host-script"`: the generic recipe target, needs `host`, `state` and `rollback` scripts. |
| `dist-workspace.toml`, `.github/workflows/release.yml`, `release-sdk.yml`, `deploy-website.yml`, `README.md:65` | The artefact surfaces aether's users actually consume: cargo-dist installers (shell/npm/homebrew) for `aether-agent-cli`, `@aether-agent/sdk` on npm, and the docs site on GitHub Pages. |

The brief is therefore the **only** place in the repository that does not name the target - and it
is the place agents read first (`AGENTS.md:3` points at it).

## 3. What I verified with the runner (scratch copies under /tmp, nothing written in the workspace)

```
# panel contract, no .prairie/deploy.sh
prairie deploy: OK production (each commit needs approval), target panel, 1 health check,
  panel container 3f2504e0-..., secrets CP_API_KEY (an owner binds each to this project's
  deploys), does not wait for idle                       -> exit 0

# the same panel contract plus state/rollback
prairie deploy: ERROR .prairie/deploy.toml: target = "panel" does not take a state script;
  the runner reads the running tag from the panel         -> exit 1

# host-script contract (deploy.sh + deploy-state.sh + rollback.sh)
prairie deploy: OK production (each commit needs approval), target host-script, 1 health check,
  rollback .prairie/rollback.sh, secrets REGISTRY_TOKEN    -> exit 0
```

In this checkout: `prairie deploy check` prints `prairie deploy: OFF no .prairie/deploy.sh`, so
nothing here reaches production automatically today.

## 4. The board's deploy chain - and the contradiction the answer settles

`TASK-26-143` (run the production deploy; its health gate re-verifies `TASK-26-110` live) waits on
`TASK-27-488` (place the recipe on the deploy host, bind the production secrets), which was split
into four children:

- `TASK-2B-1039` - get the owner's target, health URL, secret bindings and deploy-host access.
- `TASK-2B-1040` - write `.prairie/deploy.sh`, `.prairie/deploy-state.sh`, `.prairie/rollback.sh`.
- `TASK-2B-1041` - write `.prairie/deploy.toml` with `state`, `rollback`, `secrets`, `[[health]]`.
- `TASK-2B-1042` - run the deploy and verify the live criteria.

Children 2 and 3 describe the **host-script** shape (recipe + state + rollback). The **panel**
target the repo documents rejects `state` and `rollback` and needs no recipe at all. So the answer
to the brief's question also decides which contract the board's children write: picking `panel`
makes `TASK-2B-1040` unnecessary and `TASK-2B-1041`'s `state`/`rollback` keys illegal; picking
`host-script` means `AGENTS.md:16` and `.prairie/DEPLOY.md` have to change. Until the answer lands,
both shapes stay on the board and the deploy stays blocked on the owner's values (`image`,
`panel_container`/`panel_node`, the `[[health]]` URL, the `CP_API_KEY` binding) - no agent may
invent them (`AGENTS.md:17`, DEPLOY.md:34).

## 5. The options

### a. Name the panel target (recommended)

*What it rests on*: `AGENTS.md:16`, `.prairie/DEPLOY.md:11-32`, the panel-shaped
`.prairie/deploy.toml.template`, and `prairie deploy --help`'s panel section.

*Effect*: the brief's `## Constraints` (and `## Out of scope`) state that production is a container
the prairie runner upgrades on the cp.netquirk.com panel from `registry.netquirk.com`, with the
deploy's secrets bound to the deploy; the open question closes. The board's contract work is
re-pointed at the panel contract: `.prairie/deploy.toml` with `production = true`,
`target = "panel"`, `image`, `panel_container`/`panel_node`, `secrets = ["CP_API_KEY"]` and one
`[[health]]` URL - and `TASK-2B-1040`'s recipe/state/rollback scripts become unnecessary, because
the runner drives the panel itself.

*Cost / risk*: the brief names a vendor (Netquirk's panel and registry) as aether's production host
in a public repository. The owner's values are still missing - registry path, container id or
name+node, health URL, and the `CP_API_KEY` binding - so `TASK-26-143` stays blocked on the owner
after the decision, and the brief has to be edited again if the owner picks another target.

### b. Name the host-script target

*What it rests on*: the board's own children `TASK-2B-1040`/`-1041`/`-1042`, which specify
`.prairie/deploy.sh` + state + rollback scripts and a `host` key; the runner's `host-script`
target, verified above to check OK with exactly that shape.

*Effect*: the brief says production is reached by the project's own shell recipe on a deploy host -
matching the work already on the board - and `AGENTS.md:16-17` and `.prairie/DEPLOY.md` are edited
to match, because they currently name `target = "panel"` and forbid panel/registry/node access as
the deploy path.

*Cost / risk*: it contradicts the repository's own agent rules, so the answer touches two files
beyond the brief and one of them is a binding rule for agents. It keeps the deploy blocked on the
same owner-only inputs (a deploy host, a secret binding, a health URL), and it adds three shell
scripts to maintain for a CLI whose users install it themselves.

### c. Name the published-artefact surface as production

*What it rests on*: what aether actually is - a CLI, a library and an npm SDK that users install
(`README.md:65`, `dist-workspace.toml`, `.github/workflows/release.yml`, `release-sdk.yml`), plus the
docs site (`deploy-website.yml`, `packages/website`).

*Effect*: the brief says production means published artefacts (crates.io, the npm CLI package, the
`@aether-agent/sdk` package, the docs site) and that the container/panel work is out of scope, so
`TASK-26-143` and `TASK-27-488` are closed or re-scoped onto the publish workflows and the brief's
existing Constraint about `.prairie/deploy.toml` is replaced.

*Cost / risk*: a published package has no `[[health]]` URL, so the prairie deploy contract's health
gate has nothing to check and the live re-verification of `TASK-26-110` ("the CLI refuses to start
when the working directory is not writable", naming the path) has no live system to run against.
It also contradicts the owner's standing approval for unattended production deploys and the panel
work already drafted. It is the honest description of the product, but it gives up the only
production surface the board has actually prepared.

### d. Keep the brief target-free

*What it rests on*: the brief's current wording and `## Out of scope`, which deliberately name no
target; `AGENTS.md:20` already tracks the deploy work on the board instead.

*Effect*: no brief or `AGENTS.md` edit; the brief keeps naming only the deploy-contract mechanism
and the open question stays on the board until `TASK-2B-1039`'s card is answered.

*Cost / risk*: this is the "leave it" path - the question has been open since 2026-10-12 while four
board children wait on the same unanswered choice, and the board keeps carrying a contract shape
(recipe + `state` + `rollback`) that the panel target rejects, so the contradiction is paid for
again on the next deploy task.

## 6. Recommendation

**Recommend (a), the panel target, with (a) as the default.**

- The repository already commits to it: `AGENTS.md:16` names `target = "panel"`, `.prairie/DEPLOY.md`
  documents that contract, and `.prairie/deploy.toml.template` already is that shape with only the
  `OWNER` values missing. Naming it in the brief closes a gap between the brief and the repo
  instead of inventing a new fact.
- The brief's own `## Constraints` line already leans that way ("creates and upgrades the container
  itself", and it cites `.prairie/DEPLOY.md`, the document that names the panel contract), so (a)
  makes the brief consistent with its own citation.
- It is the only target the runner drives end to end without giving an agent panel, registry or node
  access, which is the rule `AGENTS.md:16` and `.prairie/DEPLOY.md:38-41` set for agents.
- It removes work rather than adding it: on the panel target no `.prairie/deploy.sh` is required or
  run, so `TASK-2B-1040` drops out and `TASK-2B-1041` writes a contract the runner accepts.

The default is (a) rather than "wait": if the owner does not answer within 48 hours, the default
still moves the work forward by recording in the brief the target the repository already states,
leaving only the owner's own values (`image`, container/node, health URL, secret binding) open.

## 7. What the card says

Raised with `prairie decide` on `TASK-2C-890` (decision id
`34395d24-09a4-4b03-a585-a70002bf02ee`, raised 2026-10-24T01:50Z): the question "Should aether's
brief name a production hosting target, and which one?", four paths (a panel / b host-script /
c published artefacts / d keep it target-free), each with its effect and its cost, `--recommend a
--why ...`, `--deadline 48h --default a --no-hold`, plus four `--evidence` facts.

One operational note for whoever raises such a card: the runner refuses a decision whose text names
a company secret that is not bound to the project - the first attempt was turned back with
"company secrets ... exist but are not bound to this project; an owner binding one satisfies this
request - do not raise a decision". The card therefore describes the panel API token by its role
("the panel API token in the secrets list") rather than by name; the name itself is unchanged in
`.prairie/deploy.toml.template`, where it belongs.

## 8. What happens next

- The owner's answer (or the (a) default at the 48h deadline, 2026-10-26T01:50Z) is written into
  `.prairie/BRIEF.md` by the follow-up task `TASK-2J-38` ("Brief: record the decision on the
  deployment/hosting target"): one bullet under `## Decisions`, the open question at `BRIEF.md:95`
  removed, and - when the answer names a target - one matching line under `## Constraints`.
  The card's answer is read from the owner's Decisions page (a completed Options task's record does
  not show it); the follow-up also handles the default when the deadline passes unanswered.
- The deploy work itself stays where it is: `TASK-26-143` waits on the owner's values
  (`image`, `panel_container`/`panel_node`, the `[[health]]` URL, the `CP_API_KEY` binding) and on
  the contract task; no agent may invent them or ask for panel/registry/node access.
- Nothing in this note is a deploy recipe: `.prairie/deploy.toml` and `.prairie/deploy.sh` are not
  written here, and no deploy was run.

## Update: the card was declined, and what this task delivered instead

The decision card `34395d24-09a4-4b03-a585-a70002bf02ee` was **declined** by the
operator: *"Prairie capability gap: the deploy contract owns panel, registry and
node access; a run token cannot create a container, so this is not an owner
decision."* The runner refuses to treat it as an owner decision, so this task
raises **no further card** and asks for **no access**: `AGENTS.md` rule 1 says a
run token is refused panel, registry and node access by design.

The board now carries the rest:

- `TASK-2J-41` -- *Prairie capability gap: a run cannot be given panel,
  registry or node access* (in progress; the runner closes this gap).
- `TASK-2J-44` -- *Brief: record the decision on Deployment/hosting* (todo; it
  owns the write-back into `.prairie/BRIEF.md`, which this task leaves alone).
- `TASK-2J-38` (the earlier brief write-back task) is superseded by
  `TASK-2J-44`.

So the open question in the brief -- *should the brief state a hosting target?*
-- is answered by the operator's decline, not by this task, and the brief stays
untouched until `TASK-2J-44` writes it back.

### What this task did deliver (the code part)

The deploy contract's shape already existed (`.prairie/DEPLOY.md`,
`.prairie/deploy.toml.template`, `AGENTS.md`), but two pieces a deploy needs had
no owner: **the image the runner builds and pushes** and **the health endpoint
its `[[health]]` check probes**. Both are now in the repository:

- `Dockerfile` (repo root): the panel image. Runtime-only -- it copies the
  artefact the runner's build step produces (`target/debug/aether`, the path
  `.dockerignore` keeps in the build context) to `/usr/local/bin/aether`, runs
  as a non-root user and serves `aether server --listen 0.0.0.0:8765`
  (`EXPOSE 8765`). Nothing is compiled inside the image.
- `crates/aether-cli/src/acp/health.rs` (new) + `crates/aether-cli/src/acp/server.rs`:
  the ACP server answers `GET /healthz` with `HTTP/1.1 200 OK` and the body
  `ok` on its own listener -- the same port the WebSocket API uses, which is
  the port the contract's health URL points at. The request head is read
  before the WebSocket handshake starts, because tungstenite can only answer a
  failed handshake with an error status, never with `200`; a probe is answered
  there and every other request is replayed to the handshake byte for byte
  (`PrefixedStream`). A probe never keeps the client slot, so it also answers
  while an ACP client is attached.
- `crates/aether-cli/tests/integration/acp_remote/health.rs` (new): the probe
  is answered with `200 ok`, the probe does not take the client slot, and other
  paths are still left to the handshake. `cargo test -p aether-agent-cli
  --all-features --test integration acp_remote` passes (37 tests).

Still true, and unchanged by this task: aether still declares **no deploy
contract** -- there is no `.prairie/deploy.toml`, and `prairie deploy check`
prints `OFF no .prairie/deploy.sh`. The image was not built here: this sandbox
has no container engine, so `docker build` was not run against the Dockerfile.
The registry path, the container and its node and the health URL remain the
owner's to set in the contract (`TASK-2B-1041`).
