#!/bin/bash
# No-paid verification of Workspace Auto. An optional production .deb binds
# the running service and desktop executable to that exact archive.
set -euo pipefail
umask 022
if [[ ${1:-} != --under-deadline ]]; then
    exec timeout --signal=TERM --kill-after=3s 297s bash "$0" --under-deadline "$@"
fi
shift
if (( $# > 1 )); then
    echo "Usage: $0 [production.deb]" >&2
    exit 2
fi
archive=
if (( $# == 1 )); then archive=$(realpath -e -- "$1"); fi
root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"

check_service() (
    [[ $(dpkg-query -W -f='${db:Status-Status}' cortex-workspace) == installed ]]
    systemctl --user is-active --quiet cortex-workspace.service
    local pid
    pid=$(systemctl --user show -p MainPID --value cortex-workspace.service)
    [[ $pid =~ ^[1-9][0-9]*$ ]]
    cmp --silent "/proc/$pid/exe" /usr/bin/cortex-workspace-service
    if [[ -n $archive ]]; then
        [[ $(dpkg-deb -f "$archive" Package) == cortex-workspace ]]
        [[ $(dpkg-deb -f "$archive" Version) == "$(dpkg-query -W -f='${Version}' cortex-workspace)" ]]
        local binaries
        binaries=$(mktemp -d)
        trap 'rm -rf -- "$binaries"' EXIT
        dpkg-deb --fsys-tarfile "$archive" | tar -x -C "$binaries" ./usr/bin/cortex-workspace ./usr/bin/cortex-workspace-service
        cmp --silent "$binaries/usr/bin/cortex-workspace-service" "/proc/$pid/exe"
        cmp --silent "$binaries/usr/bin/cortex-workspace" /usr/bin/cortex-workspace
    fi
)

step() {
    local name=$1
    shift
    echo "Checking $name"
    "$@"
    echo "PASS $name"
}

step installed-service check_service
cargo="$HOME/.cargo/bin/cargo"
step command-policy-and-store "$cargo" test --locked --offline -p workspace-service --lib native_commands
step linked-checkout-and-owner-decisions "$cargo" test --locked --offline -p workspace-service --test native_sessions -- linked-checkout-command
(
    cd packages/workspace-ui
    step permission-response ./node_modules/.bin/vitest run test/real-port.test.ts -t 'checks the exact permission'
)
echo "Auto verification passed in ${SECONDS}s"
