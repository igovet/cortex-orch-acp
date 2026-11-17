#!/bin/sh
# Build (if needed) and run the Rust cortex-xtask tool: generators, package stamp, pins check, bounded no-paid suite (test-fast) and the build driver.
# Usage: scripts/cortex-xtask.sh <command> [args...]
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
command -v cargo >/dev/null 2>&1 || PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
cargo build --locked --offline --quiet --manifest-path "$root/Cargo.toml" -p cortex-xtask
exec "${CARGO_TARGET_DIR:-$root/target}/debug/cortex-xtask" "$@"
