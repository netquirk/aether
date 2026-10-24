#!/usr/bin/env bash
# Verify the three environment-dependent workspace test targets.
#
# These targets cannot pass in a sandbox that lacks a container engine or a
# usable language server, so this script first reports what the environment
# provides and then runs every target, collecting the results instead of
# stopping at the first failure. It exits non-zero when any target fails.
#
# Usage:
#   scripts/verify-environment-dependent-tests.sh
#
# Environment:
#   AETHER_ENV_TEST_LOG_DIR  directory for the evidence log (default: test-results)
set -uo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

log_dir="${AETHER_ENV_TEST_LOG_DIR:-test-results}"
log_file="$log_dir/environment-dependent-tests.log"
mkdir -p "$log_dir"
exec > >(tee "$log_file") 2>&1

failed_targets=()

report_environment() {
    echo "== environment =="
    date -u +"%Y-%m-%dT%H:%M:%SZ"
    echo "uname: $(uname -srm)"
    echo "rustc: $(rustc --version 2>&1 || echo 'not available')"
    echo "cargo: $(cargo --version 2>&1 || echo 'not available')"
    echo "rust-analyzer: $(rust-analyzer --version 2>&1 || echo 'not available')"
    echo "RUST_SRC_PATH: ${RUST_SRC_PATH:-<unset>}"
    echo "rust standard library sources:"
    if [ -d "${RUST_SRC_PATH:-/nonexistent}" ]; then
        echo "  $RUST_SRC_PATH"
    else
        echo "  (none found: rust-analyzer cannot type-check Cargo projects without them)"
    fi
    echo "container engine:"
    if docker info >/dev/null 2>&1; then
        docker info 2>&1 | sed -n '1,20p'
    else
        docker info 2>&1 | sed -n '1,5p'
    fi
    echo
}

run_target() {
    local title="$1"
    shift
    echo "== $title =="
    echo "\$ $*"
    if "$@"; then
        echo "-- PASS: $title"
    else
        echo "-- FAIL: $title"
        failed_targets+=("$title")
    fi
    echo
}

report_environment

run_target "aether-lspd e2e tests" \
    cargo test -p aether-lspd --test e2e

run_target "aether-mcp-servers LSP diagnostics e2e tests" \
    cargo test -p aether-mcp-servers --test integration lsp_diagnostics_e2e

run_target "aether-evals docker agent tests" \
    cargo test -p aether-evals --test docker_agent_tests

echo "== summary =="
if [ ${#failed_targets[@]} -eq 0 ]; then
    echo "all environment-dependent targets passed"
    exit 0
fi

for target in "${failed_targets[@]}"; do
    echo "FAILED: $target"
done
exit 1
