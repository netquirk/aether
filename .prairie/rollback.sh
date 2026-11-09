#!/usr/bin/env bash
set -euo pipefail
dst="${AETHER_BIN:-$HOME/.cargo/bin/aether}"
root="${AETHER_RELEASES:-$HOME/.local/lib/aether/releases}"
prior="${PRAIRIE_PRIOR_STATE:?missing previous binary digest}"
[[ "$prior" =~ ^[a-f0-9]{64}$ ]]
[ -x "$root/$prior/aether" ]
ln -s "$root/$prior/aether" "$dst.pending-$$"
mv -Tf "$dst.pending-$$" "$dst"
"$dst" --version
