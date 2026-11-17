#!/bin/sh
# Update the repository-owned Cortex ACP provider in the already-running Paseo
# daemon.  This script intentionally does not start/stop Paseo and does not
# touch accounts, chats, or project-local .cortex stores.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
command -v cargo >/dev/null 2>&1 || PATH="$cargo_bin:$PATH"

die() {
    printf '%s\n' "update-paseo-plugin: $*" >&2
    exit 1
}

command -v cargo >/dev/null 2>&1 || die "cargo was not found"
command -v node >/dev/null 2>&1 || die "node was not found"

if [ "${1-}" = "--help" ] || [ "${1-}" = "-h" ]; then
    printf '%s\n' \
        "Usage: scripts/update-paseo-plugin.sh" \
        "" \
        "Refresh the repository-owned Cortex Paseo plugin in a running Paseo daemon." \
        "The script builds the Rust entries, stamps the package, installs/reloads the plugin, and" \
        "verifies it is running. The launch record names this checkout's built" \
        "target/debug/cortex-acp (cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools)." \
        "Set CORTEX_PASEO_CLI when paseo is not on PATH."
    exit 0
fi
[ -z "${1-}" ] || die "unknown argument: $1 (use --help)"

paseo_cli=${CORTEX_PASEO_CLI-}
if [ -z "$paseo_cli" ]; then
    if command -v paseo >/dev/null 2>&1; then
        paseo_cli=$(command -v paseo)
    elif [ -x /opt/Paseo/resources/bin/paseo ]; then
        paseo_cli=/opt/Paseo/resources/bin/paseo
    else
        die "Paseo CLI was not found; set CORTEX_PASEO_CLI"
    fi
fi
case "$paseo_cli" in
    */*) [ -x "$paseo_cli" ] || die "Paseo CLI is not executable: $paseo_cli" ;;
    *) command -v "$paseo_cli" >/dev/null 2>&1 || die "Paseo CLI was not found: $paseo_cli" ;;
esac

printf '%s\n' "Checking that Paseo is already running..."
daemon_status=$("$paseo_cli" daemon status --json 2>&1) || {
    printf '%s\n' "$daemon_status" >&2
    die "Paseo daemon is not reachable; start Paseo and run this script again"
}
printf '%s\n' "$daemon_status" | node -e '
let text = "";
process.stdin.on("data", (chunk) => { text += chunk; });
process.stdin.on("end", () => {
  let value;
  try { value = JSON.parse(text); } catch { console.error("Paseo daemon status was not valid JSON"); process.exit(1); }
  if (value === null || typeof value !== "object" || Array.isArray(value) || !["running", "reachable"].includes(value.localDaemon)) {
    console.error("Paseo daemon is not running"); process.exit(1);
  }
});
' || die "Paseo daemon is not running"

printf '%s\n' "Building the Cortex Rust entries..."
(cd "$repo_root" && cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools -p cortex-xtask) || die "the Rust build failed"
tools="$repo_root/target/debug/cortex-paseo-tools"
xtask="$repo_root/target/debug/cortex-xtask"

printf '%s\n' "Stamping the Cortex package..."
"$xtask" package stamp --root "$repo_root"

printf '%s\n' "Installing and reloading the Cortex Paseo plugin..."
CORTEX_PASEO_CLI="$paseo_cli" "$tools" install-paseo-plugin --source-root "$repo_root"

printf '%s\n' "Verifying the Cortex Paseo plugin..."
plugin_status=$("$paseo_cli" plugin ls cortex-acp --json 2>&1) || {
    printf '%s\n' "$plugin_status" >&2
    die "could not query the Cortex Paseo plugin"
}
printf '%s\n' "$plugin_status" | node -e '
let text = "";
process.stdin.on("data", (chunk) => { text += chunk; });
process.stdin.on("end", () => {
  let value;
  try { value = JSON.parse(text); } catch { console.error("Paseo plugin status was not valid JSON"); process.exit(1); }
  const rows = Array.isArray(value) ? value : [value];
  if (!rows.some((row) => row !== null && typeof row === "object" && row.id === "cortex-acp" && row.status === "running")) {
    console.error("Cortex Paseo plugin did not reach running status"); process.exit(1);
  }
});
' || die "Cortex Paseo plugin is not running"

printf '%s\n' "Cortex Paseo plugin is running. Restart Paseo once if its UI does not refresh immediately."
