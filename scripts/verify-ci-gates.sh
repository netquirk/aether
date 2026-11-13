#!/usr/bin/env bash
# Run (or check) the CI gates the TASK-2K-259 run could not run.
#
# TASK-2K-259 changed only crates/internal-lints and AGENTS.md, but its sandbox had
# neither `just`, `cargo nextest`, `cargo sqlx` nor a populated node_modules, so five
# CI gates were never executed there. This script runs each of them and writes one
# log per gate under test-results/ci-gates/, then prints a verdict table.
#
# The verdict of every gate is recorded in CI-GATES.md, in a machine-readable block
# that `--check` reads back:
#
#   <!-- ci-gates:begin -->
#   doc-check   green
#   ...
#   <!-- ci-gates:end -->
#
# Usage:
#   scripts/verify-ci-gates.sh            # run every gate
#   scripts/verify-ci-gates.sh --check    # check the recorded verdicts and evidence
#
# Environment:
#   AETHER_CI_GATES_LOG_DIR   directory for the per-gate logs (default test-results/ci-gates)
#   AETHER_CI_GATES_REPORT    file holding the verdicts (default CI-GATES.md)
#   AETHER_CI_GATES_ONLY      space separated list of gate names to run
#   AETHER_CI_GATES_REQUIRE_LOGS=1  make --check fail when the log of a gate is missing
set -uo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The tools this run installed for itself (just, cargo-nextest, cargo-sqlx) live in
# ~/.local/bin; put that directory on the PATH if it is there.
if [ -d "$HOME/.local/bin" ]; then
    PATH="$HOME/.local/bin:$PATH"
    export PATH
fi

log_dir="${AETHER_CI_GATES_LOG_DIR:-test-results/ci-gates}"
report_file="${AETHER_CI_GATES_REPORT:-$repo_root/CI-GATES.md}"
mkdir -p "$log_dir"

# The gate name and the command it runs. The log of a gate is $log_dir/<name>.log.
GATES=(
    "doc-check|just doc-check"
    "doc-test|just doc-test"
    "sqlx-check|just sqlx-check"
    "pnpm-fmt-check|pnpm fmt-check"
    "pnpm-sdk-typecheck|pnpm sdk:typecheck"
    "pnpm-sdk-test|pnpm sdk:test"
)

# The environment-dependent targets of the task: the ones that need a container
# engine or a language server. They are run by the same script, each one as its
# own gate, so every one of them has a log and an exit code of its own.
GATES+=(
    "cli-integration-trace-context|cargo test -p aether-agent-cli --test integration trace_context::headless_rejects_an_invalid_tracestate_when_telemetry_is_enabled"
    "mcp-lsp-diagnostics|cargo test -p aether-mcp-servers --test integration lsp_diagnostics_e2e"
    "mcp-lsp-hover|cargo test -p aether-mcp-servers --test integration lsp_operations_e2e::test_hover_falls_back_from_stale_line_hint"
    "evals-docker-agent|cargo test -p aether-evals --test docker_agent_tests"
)

check_only=0
[ "${1:-}" = "--check" ] && check_only=1

only="${AETHER_CI_GATES_ONLY:-}"

failed_gates=()
skipped_gates=()

# The exit code the gate exited with, read back out of its log.
exit_code_of() {
    local name="$1"
    local log="$log_dir/$name.log"
    [ -s "$log" ] || return 1
    grep -E '^(GATE_|[A-Z_]+)_EXIT: [0-9]+$' "$log" | tail -1 | grep -Eo '[0-9]+$'
}

# The verdict the report records for the gate: green, red-pre-existing or
# red-environment.
verdict_of() {
    local name="$1"
    [ -s "$report_file" ] || return 1
    sed -n '/<!-- ci-gates:begin -->/,/<!-- ci-gates:end -->/p' "$report_file" \
        | awk -v gate="$name" '$1 == gate { print $2 }'
}

# The version of a tool, or the fact that it is not on the PATH. The last line is
# the version itself: corepack prints a notice before pnpm's version.
version_of() {
    local output
    if output="$(timeout 30 "$@" 2>&1)"; then
        echo "$output" | tail -1
    else
        echo "not on PATH"
    fi
}

run_gate() {
    local name="$1"
    local command="$2"
    local log="$log_dir/$name.log"

    echo "== $name =="
    echo "\$ $command"
    if [ "$check_only" -eq 1 ]; then
        check_gate "$name" "$log"
        echo
        return
    fi

    if [ -n "$only" ] && [[ " $only " != *" $name "* ]]; then
        echo "-- skipped (not selected)"
        skipped_gates+=("$name")
        echo
        return
    fi

    # `set -o pipefail` so a failure inside the pipe is not hidden by `tail`.
    set -o pipefail
    eval "$command" >"$log" 2>&1
    local status=$?
    echo "GATE_EXIT: $status" >>"$log"
    if [ "$status" -eq 0 ]; then
        echo "-- PASS: $name"
    else
        echo "-- FAIL: $name (see $log)"
        failed_gates+=("$name")
    fi
    set +o pipefail
    echo
}

# The check of one gate: the report has to record a verdict for it, and the log of
# the gate, when it is there, has to agree with that verdict. The logs are
# gitignored and published as an artifact, so a checkout without them still has a
# verdict; only AETHER_CI_GATES_REQUIRE_LOGS=1 makes a missing log a failure.
check_gate() {
    local name="$1"
    local log="$2"
    local verdict code

    if [ ! -s "$report_file" ]; then
        echo "-- MISSING report: $report_file"
        failed_gates+=("$name")
        return
    fi

    verdict="$(verdict_of "$name")"
    if [ -z "$verdict" ]; then
        echo "-- MISSING verdict for $name in $report_file"
        failed_gates+=("$name")
        return
    fi

    code="$(exit_code_of "$name")" || code=""
    if [ -z "$code" ]; then
        echo "-- verdict: $verdict (no log in this checkout: $log)"
        if [ "${AETHER_CI_GATES_REQUIRE_LOGS:-0}" = "1" ]; then
            echo "-- MISSING log: $log"
            failed_gates+=("$name")
        fi
        return
    fi

    echo "-- verdict: $verdict (recorded exit code: $code)"
    case "$verdict" in
        green)
            if [ "$code" -ne 0 ]; then
                echo "-- INCONSISTENT: recorded green, but the log exits $code"
                failed_gates+=("$name")
            fi
            ;;
        red-*)
            if [ "$code" -eq 0 ]; then
                echo "-- INCONSISTENT: recorded $verdict, but the log exits 0"
                failed_gates+=("$name")
            fi
            ;;
        *)
            echo "-- UNKNOWN verdict: $verdict"
            failed_gates+=("$name")
            ;;
    esac
}

echo "== environment =="
date -u +"%Y-%m-%dT%H:%M:%SZ"
echo "head: $(git rev-parse --short HEAD)"
echo "base: ${PRAIRIE_BASE_COMMIT:-unknown}"
echo "report: $report_file"
echo "rustc: $(version_of rustc --version)"
echo "just: $(version_of just --version)"
echo "cargo-nextest: $(version_of cargo nextest --version)"
echo "cargo-sqlx: $(version_of cargo sqlx --version)"
echo "node: $(version_of node --version)"
echo "pnpm: $(version_of pnpm --version)"
echo "container engine: $(docker info >/dev/null 2>&1 && echo available || echo 'not available')"
echo

for entry in "${GATES[@]}"; do
    run_gate "${entry%%|*}" "${entry#*|}"
done

echo "== summary =="
for entry in "${GATES[@]}"; do
    name="${entry%%|*}"
    if [ "$check_only" -eq 1 ]; then
        echo "recorded: $name ($(verdict_of "$name"))"
    elif [ -n "$only" ] && [[ " $only " != *" $name "* ]]; then
        echo "skipped: $name"
    elif [[ " ${failed_gates[*]} " == *" $name "* ]]; then
        echo "FAILED: $name"
    else
        echo "PASSED: $name"
    fi
done

if [ ${#failed_gates[@]} -gt 0 ]; then
    for name in "${failed_gates[@]}"; do
        echo "FAILED: $name"
    done
    exit 1
fi
exit 0
