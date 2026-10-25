#!/usr/bin/env bash
# Verify the three environment-dependent workspace test targets.
#
# These targets cannot pass in a sandbox that lacks a container engine or a
# usable language server, so this script first provisions what the environment
# can provide, reports what it found, and then runs every target, collecting
# the results instead of stopping at the first failure. It exits non-zero when
# any target fails.
#
# Provisioning the language server:
#   The aether-lspd daemon spawns the analyzer itself and inherits the
#   environment of the process that started the daemon.
#   `scripts/provision-rust-analyzer.sh` therefore runs the analyzer from a
#   toolchain that ships both `rust-analyzer` and `cargo`, with
#   CARGO/RUSTUP_TOOLCHAIN pointing at that toolchain and without
#   RUST_SRC_PATH: the analyzer type-checks against the precompiled sysroot
#   metadata, so the standard library sources are not needed, and asking for
#   them only makes the analyzer parse the whole standard library.
#   The daemon spawns the analyzer through
#   AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER, so the wrapper is installed here.
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

failed_targets=()

provision_language_server() {
    local wrapper="$repo_root/scripts/provision-rust-analyzer.sh"
    if [ ! -x "$wrapper" ]; then
        echo "language server: wrapper $wrapper is missing or not executable"
        return
    fi
    export AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER="$wrapper"
    echo "language server: AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER=$wrapper"
    echo "language server: analyzer version $("$wrapper" --version 2>&1 | sed -n '1p')"
}

report_environment() {
    echo "== environment =="
    date -u +"%Y-%m-%dT%H:%M:%SZ"
    echo "uname: $(uname -srm)"
    echo "rustc: $(rustc --version 2>&1 || echo 'not available')"
    echo "cargo: $(cargo --version 2>&1 || echo 'not available')"
    echo "rust-analyzer: $(rust-analyzer --version 2>&1 || echo 'not available')"
    echo "analyzer command: ${AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER:-rust-analyzer}"
    echo "RUST_SRC_PATH: ${RUST_SRC_PATH:-<unset>}"
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

main() {
    echo "== provisioning =="
    provision_language_server
    echo

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
        return 0
    fi

    for target in "${failed_targets[@]}"; do
        echo "FAILED: $target"
    done
    return 1
}

# The log is written with `tee` rather than a process substitution because
# `/dev/fd` is not available in every sandbox this script runs in.
main 2>&1 | tee "$log_file"
exit "${PIPESTATUS[0]}"
