#!/usr/bin/env bash
# Install the aether binary the build produced, and keep the one it replaces
# as the rollback target.
#
# The release store keeps one copy per sha256, at
# $AETHER_RELEASES/<digest>/aether. The binary that is running now is
# preserved under its own digest before the install path is switched, so
# rollback.sh can restore it by digest alone ($PRAIRIE_PRIOR_STATE), and the
# preserved copy is checked against that digest: a store entry that does not
# match its key is never handed out as a rollback target.
#
# The install path and the store are reported at the end of the run, so the
# hand-off names the rollback target that exists instead of one that was never
# written.
#
# Environment:
#   AETHER_BIN       the install path the deploy writes (default $HOME/.cargo/bin/aether)
#   AETHER_RELEASES  the release store (default $HOME/.local/lib/aether/releases)
#
# Usage: .prairie/deploy.sh [artifact]   (default artifact: bin/aether)
set -euo pipefail

dst="${AETHER_BIN:-$HOME/.cargo/bin/aether}"
root="${AETHER_RELEASES:-$HOME/.local/lib/aether/releases}"
artifact="${1:-bin/aether}"

digest_of() {
    sha256sum "$1" | cut -d' ' -f1
}

if [ ! -f "$artifact" ]; then
    printf 'deploy: no build artifact at %s\n' "$artifact" >&2
    exit 1
fi

next="$(digest_of "$artifact")"
mkdir -p "$root"

# The prior state: the binary that is running now. A missing or dangling
# install path means there is nothing to preserve, and the deploy says so
# instead of pretending a rollback target exists.
prior=""
if [ -e "$dst" ]; then
    prior="$(digest_of "$dst")"
elif [ -L "$dst" ]; then
    printf 'deploy: %s is a dangling symlink to %s: there is no running binary to preserve\n' \
        "$dst" "$(readlink "$dst")" >&2
else
    printf 'deploy: no aether binary at %s: there is nothing to preserve\n' "$dst" >&2
fi

if [ -n "$prior" ]; then
    mkdir -p "$root/$prior"
    if [ ! -f "$root/$prior/aether" ]; then
        cp --reflink=auto "$dst" "$root/$prior/aether.tmp-$$"
        mv -f "$root/$prior/aether.tmp-$$" "$root/$prior/aether"
    fi
    stored="$(digest_of "$root/$prior/aether")"
    if [ "$stored" != "$prior" ]; then
        printf 'deploy: the preserved rollback target %s holds %s, not %s\n' \
            "$root/$prior/aether" "$stored" "$prior" >&2
        exit 1
    fi
fi

mkdir -p "$root/$next"
install -m 755 "$artifact" "$root/$next/aether"
if [ "$(digest_of "$root/$next/aether")" != "$next" ]; then
    printf 'deploy: the installed artifact %s does not match its digest %s\n' \
        "$root/$next/aether" "$next" >&2
    exit 1
fi
"$root/$next/aether" --version

mkdir -p "$(dirname "$dst")"
rm -f "$dst.pending-$$"
ln -s "$root/$next/aether" "$dst.pending-$$"
mv -Tf "$dst.pending-$$" "$dst"
if [ "$(digest_of "$dst")" != "$next" ]; then
    printf 'deploy: the installed binary %s does not match its digest %s\n' "$dst" "$next" >&2
    exit 1
fi

printf 'deploy: release store %s\n' "$root"
if [ -n "$prior" ]; then
    printf 'deploy: rollback target %s (%s)\n' "$root/$prior/aether" "$prior"
else
    printf 'deploy: rollback target: none preserved (there was no running binary)\n'
fi
printf 'deploy: installed %s (%s)\n' "$dst" "$next"
"$dst" --version
