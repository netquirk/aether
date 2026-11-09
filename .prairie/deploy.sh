#!/usr/bin/env bash
set -euo pipefail
dst="${AETHER_BIN:-$HOME/.cargo/bin/aether}"
root="${AETHER_RELEASES:-$HOME/.local/lib/aether/releases}"
prior="$(sha256sum "$dst" | cut -d' ' -f1)"
next="$(sha256sum bin/aether | cut -d' ' -f1)"
mkdir -p "$root/$prior" "$root/$next"
[ -e "$root/$prior/aether" ] || cp --reflink=auto "$dst" "$root/$prior/aether"
install -m 755 bin/aether "$root/$next/aether"
"$root/$next/aether" --version
ln -s "$root/$next/aether" "$dst.pending-$$"
mv -Tf "$dst.pending-$$" "$dst"
[ "$(sha256sum "$dst" | cut -d' ' -f1)" = "$next" ]
"$dst" --version
