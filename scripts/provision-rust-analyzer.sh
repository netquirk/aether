#!/usr/bin/env bash
# Run rust-analyzer from a toolchain that can type-check Cargo projects.
#
# Why a wrapper is needed:
#   The aether-lspd daemon spawns the analyzer as a child process and
#   inherits the environment of whoever started the daemon.  In this
#   repository that environment points `CARGO`/`RUSTUP_TOOLCHAIN` at the
#   toolchain pinned by rust-toolchain.toml, which may not be the toolchain
#   that actually ships a `rust-analyzer` binary.  The analyzer then tries to
#   drive a Cargo that is not the one it was built against and never loads the
#   workspace.  This wrapper selects a toolchain that has both `rust-analyzer`
#   and `cargo`, points `CARGO`/`RUSTUP_TOOLCHAIN` at it and puts its `bin`
#   directory first on `PATH`.
#
# What this wrapper deliberately does NOT do:
#   It does not set `RUST_SRC_PATH`.  rust-analyzer type-checks against the
#   precompiled sysroot metadata that every installed toolchain ships; the
#   standard library sources are not required.  Pointing the analyzer at the
#   sources makes it parse the whole standard library, which is orders of
#   magnitude slower and pushes the first diagnostics far past any timeout a
#   caller can wait for.
#
# Usage:
#   scripts/provision-rust-analyzer.sh [rust-analyzer args...]
#
# Environment:
#   AETHER_RUST_ANALYZER_TOOLCHAIN  toolchain to use (default: the toolchain
#                                   the workspace itself compiles with, if it
#                                   ships an analyzer)
#   AETHER_RUST_ANALYZER_LOG        file the analyzer's stderr is appended to
set -uo pipefail

rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
version="$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9][0-9.]*\).*/\1/p')"

# Print the name of the toolchain to use: the requested one, else the
# toolchain this workspace compiles with, else any toolchain with an analyzer.
select_toolchain() {
    if [ -n "${AETHER_RUST_ANALYZER_TOOLCHAIN:-}" ]; then
        printf '%s\n' "$AETHER_RUST_ANALYZER_TOOLCHAIN"
        return
    fi

    local dir name numeric
    for dir in "$rustup_home"/toolchains/*/; do
        [ -x "${dir}bin/rust-analyzer" ] || continue
        name="$(basename "$dir")"
        numeric="${name%%-*}"
        case "$version" in
            "$numeric"*)
                printf '%s\n' "$name"
                return
                ;;
        esac
    done

    for dir in "$rustup_home"/toolchains/*/; do
        [ -x "${dir}bin/rust-analyzer" ] || continue
        basename "$dir"
        return
    done
}

toolchain="$(select_toolchain)"
if [ -z "$toolchain" ]; then
    echo "rust-analyzer: no toolchain under $rustup_home ships a rust-analyzer" >&2
    exit 127
fi

toolchain_dir="$rustup_home/toolchains/$toolchain"
analyzer="$toolchain_dir/bin/rust-analyzer"

export RUSTUP_TOOLCHAIN="$toolchain"
if [ -x "$toolchain_dir/bin/cargo" ]; then
    export CARGO="$toolchain_dir/bin/cargo"
fi
export PATH="$toolchain_dir/bin:$PATH"

# The precompiled sysroot is enough; parsing the standard library sources
# only slows the analyzer down.
unset RUST_SRC_PATH || true

if [ -n "${AETHER_RUST_ANALYZER_LOG:-}" ]; then
    exec "$analyzer" "$@" 2>>"$AETHER_RUST_ANALYZER_LOG"
fi
exec "$analyzer" "$@"
