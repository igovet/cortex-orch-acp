#!/bin/sh
# Build (if needed) and run the Rust cortex-paseo-tools tool: Paseo dev wrapper (paseo-dev), home clone, ACP smoke, paid probes, client recipes and plugin setup.
# Usage: scripts/cortex-paseo-tools.sh <command> [args...]
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
command -v cargo >/dev/null 2>&1 || PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
cargo build --locked --offline --quiet --manifest-path "$root/Cargo.toml" -p cortex-paseo-tools
exec "${CARGO_TARGET_DIR:-$root/target}/debug/cortex-paseo-tools" "$@"
