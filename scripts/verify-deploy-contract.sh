#!/usr/bin/env bash
# Check the deploy contract's scripts: the release store, the state script and
# the rollback.
#
# The contract only holds if a rollback target is a file that exists: the
# release store keeps one copy per sha256, the state script names the running
# binary by that digest, and the rollback restores that digest's copy. This
# script exercises deploy.sh -> deploy-state.sh -> rollback.sh against a store
# and checks that:
#
#   * the default store and install paths are $HOME/.local/lib/aether/releases
#     and $HOME/.cargo/bin/aether,
#   * the binary that is replaced is preserved under its own digest, and the
#     preserved copy matches that digest,
#   * the state script prints the digest of the running binary and nothing
#     else,
#   * the rollback restores the preserved copy and names the path it restored,
#   * a rollback target that is not in the store is refused with the exact
#     path that is missing and the entries the store does hold,
#   * a store entry that does not match its digest is refused,
#   * a deploy over a missing or dangling install path says that there is no
#     rollback target instead of pretending there is one.
#
# Usage: scripts/verify-deploy-contract.sh
#
# Environment:
#   AETHER_DEPLOY_TEST_BINARY  a real aether binary to run the round trip with
#                               (default: the live runner binary, when it is
#                               readable; the run is reported as skipped when
#                               it is not)
set -uo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
deploy_script="$repo_root/.prairie/deploy.sh"
state_script="$repo_root/.prairie/deploy-state.sh"
rollback_script="$repo_root/.prairie/rollback.sh"

failures=0
pass() {
    printf 'ok   %s\n' "$1"
}
fail() {
    printf 'FAIL %s\n' "$1" >&2
    failures=$((failures + 1))
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

digest_of() {
    sha256sum "$1" | cut -d' ' -f1
}

# A stand-in for the aether binary: it answers --version the way the real one
# does, which is all the contract scripts ask of it.
stub() { # stub <path> <version>
    mkdir -p "$(dirname "$1")"
    cat >"$1" <<EOF
#!/usr/bin/env bash
printf 'aether %s\n' '$2'
EOF
    chmod 755 "$1"
}

# 1. The round trip: deploy -> state -> rollback, with the store and the
#    install path left at their defaults so the documented paths are checked.
home="$work/home"
store="$home/.local/lib/aether/releases"
dst="$home/.cargo/bin/aether"
stub "$dst" "0.9.5-stub"
prior="$(digest_of "$dst")"
stub "$work/build/bin/aether" "0.9.21-stub"
next="$(digest_of "$work/build/bin/aether")"

deploy_output="$(cd "$work/build" && HOME="$home" "$deploy_script" bin/aether 2>&1)"
deploy_status=$?
if [ "$deploy_status" -eq 0 ]; then
    pass 'deploy: the deploy succeeds with the default paths'
else
    fail "deploy: the deploy failed with the default paths: $deploy_output"
fi

if [ -f "$store/$prior/aether" ]; then
    pass "deploy: the replaced binary is preserved at $store/$prior/aether"
else
    fail "deploy: the replaced binary is not preserved at $store/$prior/aether"
fi

if [ "$(digest_of "$store/$prior/aether" 2>/dev/null)" = "$prior" ]; then
    pass 'deploy: the preserved copy matches the digest it is stored under'
else
    fail 'deploy: the preserved copy does not match the digest it is stored under'
fi

if [ -f "$store/$next/aether" ] && [ "$(readlink "$dst")" = "$store/$next/aether" ]; then
    pass 'deploy: the install path points at the new artifact in the store'
else
    fail "deploy: the install path does not point at the new artifact in the store"
fi

if printf '%s' "$deploy_output" | grep -Fq "rollback target $store/$prior/aether ($prior)"; then
    pass 'deploy: the deploy names the rollback target it preserved'
else
    fail 'deploy: the deploy does not name the rollback target it preserved'
fi

state_output="$(HOME="$home" "$state_script" 2>&1)"
state_status=$?
if [ "$state_status" -eq 0 ] && [ "$state_output" = "$next" ]; then
    pass 'state: the state script prints the digest of the running binary and nothing else'
else
    fail "state: the state script printed '$state_output' (exit $state_status), expected $next"
fi

rollback_output="$(HOME="$home" PRAIRIE_PRIOR_STATE="$prior" "$rollback_script" 2>&1)"
rollback_status=$?
if [ "$rollback_status" -eq 0 ] && [ "$(readlink "$dst")" = "$store/$prior/aether" ]; then
    pass 'rollback: the rollback restores the preserved copy'
else
    fail "rollback: the rollback did not restore the preserved copy: $rollback_output"
fi

if printf '%s' "$rollback_output" | grep -Fq "restored $dst to $store/$prior/aether ($prior)"; then
    pass 'rollback: the rollback names the path and digest it restored'
else
    fail 'rollback: the rollback does not name the path and digest it restored'
fi

if [ "$("$dst" --version)" = "aether 0.9.5-stub" ]; then
    pass 'rollback: the restored binary is the one that was replaced'
else
    fail 'rollback: the restored binary is not the one that was replaced'
fi

# 2. A rollback target that is not in the store is refused, with the exact
#    path that is missing and the entries the store does hold.
missing="$(printf '0%.0s' $(seq 64))"
rollback_output="$(HOME="$home" PRAIRIE_PRIOR_STATE="$missing" "$rollback_script" 2>&1)"
rollback_status=$?
if [ "$rollback_status" -ne 0 ] && printf '%s' "$rollback_output" | grep -Fq "no rollback target at $store/$missing/aether"; then
    pass 'rollback: a target that is not in the store is refused with its exact path'
else
    fail "rollback: a target that is not in the store is not refused with its exact path: $rollback_output"
fi

if printf '%s' "$rollback_output" | grep -Fq "$store/$prior/aether"; then
    pass 'rollback: the refusal names the entries the store does hold'
else
    fail 'rollback: the refusal does not name the entries the store does hold'
fi

# 3. A store entry that does not match the digest it is stored under is not
#    handed out as a rollback target.
mislabeled="$(printf 'f%.0s' $(seq 64))"
mkdir -p "$store/$mislabeled"
stub "$store/$mislabeled/aether" "0.9.21-stub"
rollback_output="$(HOME="$home" PRAIRIE_PRIOR_STATE="$mislabeled" "$rollback_script" 2>&1)"
rollback_status=$?
if [ "$rollback_status" -ne 0 ] && printf '%s' "$rollback_output" | grep -Fq "not the requested $mislabeled"; then
    pass 'rollback: a store entry that does not match its digest is refused'
else
    fail "rollback: a store entry that does not match its digest is not refused: $rollback_output"
fi

# 4. A deploy over a missing install path says that there is no rollback
#    target instead of pretending there is one.
bare_home="$work/bare-home"
stub "$work/bare-build/bin/aether" "0.9.21-stub"
deploy_output="$(cd "$work/bare-build" && HOME="$bare_home" "$deploy_script" bin/aether 2>&1)"
deploy_status=$?
if [ "$deploy_status" -eq 0 ] && printf '%s' "$deploy_output" | grep -Fq 'rollback target: none preserved'; then
    pass 'deploy: a deploy with no running binary reports that it preserved no rollback target'
else
    fail "deploy: a deploy with no running binary does not report that it preserved no rollback target: $deploy_output"
fi

# 5. A deploy over a dangling install path (the state left by a wiped store)
#    says what it found and still installs.
dangling_home="$work/dangling-home"
mkdir -p "$dangling_home/.cargo/bin"
ln -s "$dangling_home/.local/lib/aether/releases/missing/aether" "$dangling_home/.cargo/bin/aether"
stub "$work/dangling-build/bin/aether" "0.9.21-stub"
deploy_output="$(cd "$work/dangling-build" && HOME="$dangling_home" "$deploy_script" bin/aether 2>&1)"
deploy_status=$?
if [ "$deploy_status" -eq 0 ] && printf '%s' "$deploy_output" | grep -Fq 'dangling symlink'; then
    pass 'deploy: a deploy over a dangling install path names it and still installs'
else
    fail "deploy: a deploy over a dangling install path does not name it: $deploy_output"
fi

# 6. The round trip with a real aether binary, so the store holds the artifact
#    that is actually deployed and the digest that is recorded is the one the
#    host serves.
real_binary="${AETHER_DEPLOY_TEST_BINARY:-/home/prairie/.local/lib/prairie/bin/aether}"
if [ -r "$real_binary" ] && [ -x "$real_binary" ]; then
    real_home="$work/real-home"
    real_store="$real_home/.local/lib/aether/releases"
    mkdir -p "$real_home/.cargo/bin"
    cp --reflink=auto "$real_binary" "$real_home/.cargo/bin/aether"
    real_prior="$(digest_of "$real_home/.cargo/bin/aether")"
    stub "$work/real-build/bin/aether" "0.9.21-stub"
    deploy_output="$(cd "$work/real-build" && HOME="$real_home" "$deploy_script" bin/aether 2>&1)"
    deploy_status=$?
    rollback_output="$(HOME="$real_home" PRAIRIE_PRIOR_STATE="$real_prior" "$rollback_script" 2>&1)"
    rollback_status=$?
    if [ "$deploy_status" -eq 0 ] && [ "$rollback_status" -eq 0 ] \
        && [ -f "$real_store/$real_prior/aether" ] \
        && [ "$(digest_of "$real_store/$real_prior/aether")" = "$real_prior" ]; then
        pass "real binary: $real_binary ($real_prior) is preserved and restored from the store"
    else
        fail "real binary: $real_binary ($real_prior) is not preserved and restored from the store: $deploy_output $rollback_output"
    fi
    printf '     %s  %s\n' "$real_prior" "$real_binary"
    printf '     %s\n' "$("$real_binary" --version 2>&1 | head -1)"
else
    printf 'skip real binary: %s is not readable here\n' "$real_binary"
fi

if [ "$failures" -ne 0 ]; then
    printf '\n%s check(s) failed\n' "$failures" >&2
    exit 1
fi

printf '\nall deploy contract checks passed\n'
