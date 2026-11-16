#!/usr/bin/env bash
# Restore the aether binary that the deploy replaced.
#
# The release store keeps one copy per sha256, at
# $AETHER_RELEASES/<digest>/aether, so the rollback target is found by digest
# alone: the runner hands the prior state to this script as
# $PRAIRIE_PRIOR_STATE (the digest the state script printed before the
# deploy). The stored copy is checked against that digest before it is
# installed, and a target that is not in the store is reported with the exact
# path that is missing and the entries the store does hold.
#
# Environment:
#   AETHER_BIN           the install path the deploy writes (default $HOME/.cargo/bin/aether)
#   AETHER_RELEASES      the release store (default $HOME/.local/lib/aether/releases)
#   PRAIRIE_PRIOR_STATE  the sha256 of the binary to restore (required)
set -euo pipefail

dst="${AETHER_BIN:-$HOME/.cargo/bin/aether}"
root="${AETHER_RELEASES:-$HOME/.local/lib/aether/releases}"
prior="${PRAIRIE_PRIOR_STATE:?missing previous binary digest}"

if [[ ! "$prior" =~ ^[a-f0-9]{64}$ ]]; then
    printf 'rollback: PRAIRIE_PRIOR_STATE is not a sha256 digest: %s\n' "$prior" >&2
    exit 1
fi

target="$root/$prior/aether"

if [ ! -f "$target" ]; then
    printf 'rollback: no rollback target at %s\n' "$target" >&2
    if [ -d "$root" ]; then
        printf 'rollback: the release store %s holds:\n' "$root" >&2
        found=0
        for entry in "$root"/*/aether; do
            [ -f "$entry" ] || continue
            found=1
            printf 'rollback:   %s  %s\n' "$entry" "$(sha256sum "$entry" | cut -d' ' -f1)" >&2
        done
        [ "$found" -eq 1 ] || printf 'rollback:   (no binary in the store)\n' >&2
    else
        printf 'rollback: the release store %s does not exist\n' "$root" >&2
    fi
    exit 1
fi

stored="$(sha256sum "$target" | cut -d' ' -f1)"
if [ "$stored" != "$prior" ]; then
    printf 'rollback: %s holds %s, not the requested %s\n' "$target" "$stored" "$prior" >&2
    printf 'rollback: the store is digest-keyed, so that entry is not the rollback target\n' >&2
    exit 1
fi

if [ ! -x "$target" ]; then
    printf 'rollback: %s is not executable\n' "$target" >&2
    exit 1
fi

mkdir -p "$(dirname "$dst")"
rm -f "$dst.pending-$$"
ln -s "$target" "$dst.pending-$$"
mv -Tf "$dst.pending-$$" "$dst"

printf 'rollback: restored %s to %s (%s)\n' "$dst" "$target" "$prior"
"$dst" --version
