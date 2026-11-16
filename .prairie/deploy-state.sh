#!/usr/bin/env bash
# Print the version of the aether binary that is running: the sha256 of the file
# the install path resolves to.
#
# The prairie runner reads this script's stdout as the prior state of a deploy
# and hands it to rollback.sh as $PRAIRIE_PRIOR_STATE, so stdout carries the
# digest and nothing else. Every diagnostic goes to stderr, and a binary that
# cannot be read is a failure: a prior state that cannot be read is a deploy
# that cannot be rolled back.
#
# Environment:
#   AETHER_BIN   the install path the deploy writes (default $HOME/.cargo/bin/aether)
set -euo pipefail

dst="${AETHER_BIN:-$HOME/.cargo/bin/aether}"

if [ -L "$dst" ] && [ ! -e "$dst" ]; then
    printf 'deploy-state: %s is a dangling symlink to %s\n' "$dst" "$(readlink "$dst")" >&2
    printf 'deploy-state: no running binary, so there is no prior state to record\n' >&2
    exit 1
fi

if [ ! -e "$dst" ]; then
    printf 'deploy-state: no aether binary at %s\n' "$dst" >&2
    printf 'deploy-state: no running binary, so there is no prior state to record\n' >&2
    exit 1
fi

if [ ! -x "$dst" ]; then
    printf 'deploy-state: %s is not executable\n' "$dst" >&2
    exit 1
fi

digest="$(sha256sum "$dst" | cut -d' ' -f1)"
printf '%s\n' "$digest"
