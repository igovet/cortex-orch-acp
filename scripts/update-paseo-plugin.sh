#!/bin/sh
# Update the repository-owned Cortex ACP provider in the already-running Paseo
# daemon.  This script intentionally does not start/stop Paseo and does not
# touch accounts, chats, or project-local .cortex stores.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
setup="$repo_root/scripts/cortex_acp_setup.py"
package="$repo_root/scripts/cortex_package.py"

die() {
    printf '%s\n' "update-paseo-plugin: $*" >&2
    exit 1
}

command -v python3 >/dev/null 2>&1 || die "python3 was not found"
[ -f "$setup" ] || die "repository setup script was not found: $setup"
[ -f "$package" ] || die "repository package script was not found: $package"

if [ "${1-}" = "--help" ] || [ "${1-}" = "-h" ]; then
    printf '%s\n' \
        "Usage: scripts/update-paseo-plugin.sh" \
        "" \
        "Refresh the repository-owned Cortex Paseo plugin in a running Paseo daemon." \
        "The script stamps the package, installs/reloads the plugin, and verifies it is running." \
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
printf '%s\n' "$daemon_status" | python3 -c '
import json, sys
try:
    value = json.load(sys.stdin)
except Exception:
    raise SystemExit("Paseo daemon status was not valid JSON")
if not isinstance(value, dict) or value.get("localDaemon") not in {"running", "reachable"}:
    raise SystemExit("Paseo daemon is not running")
' || die "Paseo daemon is not running"

printf '%s\n' "Stamping the Cortex package..."
python3 -B "$package" stamp

printf '%s\n' "Installing and reloading the Cortex Paseo plugin..."
CORTEX_PASEO_CLI="$paseo_cli" python3 -B "$setup" install-paseo-plugin

printf '%s\n' "Verifying the Cortex Paseo plugin..."
plugin_status=$("$paseo_cli" plugin ls cortex-acp --json 2>&1) || {
    printf '%s\n' "$plugin_status" >&2
    die "could not query the Cortex Paseo plugin"
}
printf '%s\n' "$plugin_status" | python3 -c '
import json, sys
try:
    value = json.load(sys.stdin)
except Exception:
    raise SystemExit("Paseo plugin status was not valid JSON")
rows = value if isinstance(value, list) else [value]
if not any(isinstance(row, dict) and row.get("id") == "cortex-acp" and row.get("status") == "running" for row in rows):
    raise SystemExit("Cortex Paseo plugin did not reach running status")
' || die "Cortex Paseo plugin is not running"

printf '%s\n' "Cortex Paseo plugin is running. Restart Paseo once if its UI does not refresh immediately."
