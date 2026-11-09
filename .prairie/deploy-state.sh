#!/usr/bin/env bash
set -euo pipefail
sha256sum "${AETHER_BIN:-$HOME/.cargo/bin/aether}" | cut -d' ' -f1
