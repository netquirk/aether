#!/usr/bin/env bash
# shellcheck disable=SC2016
# Check the rules that keep a run from putting the deploy to a person.
#
# Production is reached through the project's deploy contract
# (.prairie/deploy.toml): the contract's `target` names the hosting target and
# the prairie runner drives it, so a run never asks a person for panel,
# registry or node access, never puts the deploy to a person, and stops at the
# contract boundary naming the exact missing key. This check fails when those
# rules go missing from the files an agent reads: AGENTS.md (the agent
# instructions), .prairie/BRIEF.md (the project brief) and .prairie/DEPLOY.md
# (the deploy path).
#
# Usage: scripts/check-deploy-access-rules.sh
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

AGENT_INSTRUCTIONS="AGENTS.md"
PROJECT_BRIEF=".prairie/BRIEF.md"
DEPLOY_PATH=".prairie/DEPLOY.md"
CONTRACT_TEMPLATE=".prairie/deploy.toml.template"

failures=0

pass() {
    printf 'ok   %s\n' "$1"
}

fail() {
    printf 'FAIL %s\n' "$1" >&2
    failures=$((failures + 1))
}

require() { # require <file> <needle> <check name>
    if grep -Fq -- "$2" "$1"; then
        pass "$3"
    else
        fail "$3: $1 does not contain: $2"
    fi
}

forbid() { # forbid <file> <needle> <check name>
    if grep -Fq -- "$2" "$1"; then
        fail "$3: $1 must not contain: $2"
    else
        pass "$3"
    fi
}

# no-escalation: the deploy is never put to a person.
require "$AGENT_INSTRUCTIONS" 'Never ask for panel, registry or node access' 'no-escalation: the access request is forbidden in AGENTS.md'
require "$AGENT_INSTRUCTIONS" 'Never put the deploy to a person' 'no-escalation: the escalation is forbidden in AGENTS.md'
require "$DEPLOY_PATH" 'Does not** put the deploy to a person' 'no-escalation: the escalation is forbidden in DEPLOY.md'
require "$PROJECT_BRIEF" 'panel, registry or node access' 'no-escalation: the access rule is in BRIEF.md'
require "$PROJECT_BRIEF" 'stops at the contract boundary' 'no-escalation: the stop rule is in BRIEF.md'

# stop-rule: a run names the missing key and stops.
require "$AGENT_INSTRUCTIONS" 'Stop at the contract boundary' 'stop-rule: AGENTS.md keeps the stop rule'
require "$AGENT_INSTRUCTIONS" 'name the exact missing piece' 'stop-rule: AGENTS.md keeps naming the missing piece'
require "$DEPLOY_PATH" 'name the exact missing key' 'stop-rule: DEPLOY.md keeps naming the missing key'

# access-mentions-are-prohibitions: an access request is only ever named to forbid it.
for file in "$AGENT_INSTRUCTIONS" "$PROJECT_BRIEF" "$DEPLOY_PATH"; do
    offenders="$(grep 'prairie need-access' "$file" | grep -viE 'never|not\b|wrong|refus' || true)"
    if [ -n "$offenders" ]; then
        fail "access-mentions-are-prohibitions: $file names an access request without forbidding it: $offenders"
    fi
done
pass 'access-mentions-are-prohibitions: every access request is named to forbid it'

# panel-path-pointers: the instructions point at the contract path, not at a recipe or a deploy host.
require "$CONTRACT_TEMPLATE" 'target = "panel"' 'panel-path-pointers: the shipped contract shape is the panel target'
require "$AGENT_INSTRUCTIONS" 'the `panel` target needs no `.prairie/deploy.sh` and no deploy host' 'panel-path-pointers: AGENTS.md says the panel target needs no recipe and no deploy host'
require "$AGENT_INSTRUCTIONS" '`TASK-2B-1041`' 'panel-path-pointers: AGENTS.md points at the contract task'
require "$AGENT_INSTRUCTIONS" 'belong to the `host-script` target' 'panel-path-pointers: AGENTS.md marks the recipe tasks as the host-script path'
require "$DEPLOY_PATH" 'not a recipe and not a deploy host' 'panel-path-pointers: DEPLOY.md names what is missing'
require "$DEPLOY_PATH" 'no `state` and no `rollback`' 'panel-path-pointers: DEPLOY.md records that the panel target refuses state and rollback'

# brief-names-no-hosting-target: the brief names the mechanism, not a target. The owner declined
# decision 34395d24 ("should the brief name a production hosting target, and which one?"): the
# deploy contract names the target, so the brief must not pick one.
forbid "$PROJECT_BRIEF" 'cp.netquirk.com' 'brief-names-no-hosting-target: the brief does not name the panel host'
forbid "$PROJECT_BRIEF" 'registry.netquirk.com' 'brief-names-no-hosting-target: the brief does not name the registry'

if [ "$failures" -ne 0 ]; then
    printf '\n%s check(s) failed\n' "$failures" >&2
    exit 1
fi

printf '\nall deploy access rule checks passed\n'
