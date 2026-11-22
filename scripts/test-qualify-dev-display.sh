#!/usr/bin/env bash
# No archive extraction, application startup, X input, or paid work.
set -Eeuo pipefail
umask 022
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir "$fixture/bin"
for tool in dirname id; do ln -s "$(command -v "$tool")" "$fixture/bin/$tool"; done
# Presence-only shims: neither Xvfb nor the driver may be executed.
ln -s /usr/bin/false "$fixture/bin/Xvfb"
driver="$fixture/driver"
printf '#!/bin/bash\n: >"%s"\n' "$fixture/driver.started" >"$driver"
chmod 0700 "$driver"

refuses() {
    local expected=$1 rc=0 output
    shift
    output=$(env -i HOME="$fixture" PATH="$fixture/bin" DISPLAY=:91.0 \
        /usr/bin/bash "$root/scripts/qualify-dev-deb.sh" "$@" 2>&1) || rc=$?
    if [[ $rc != 1 || $output != "Dev qualification: $expected" || -e $fixture/driver.started ]]; then
        printf 'Expected early refusal (%s), got exit %s: %s\n' "$expected" "$rc" "$output" >&2
        return 1
    fi
}

# Nonexistent archive is a stop sentinel even on the old launcher.
archive=(--archive "$fixture/no-archive.deb")
refuses 'Drive requires private authenticated Xvfb; --real-display is not allowed.' \
    drive --real-display "${archive[@]}" -- "$driver"
refuses 'Missing required private display tool: xauth.' \
    drive "${archive[@]}" -- "$driver"
refuses 'Missing required private display tool: xauth.' \
    smoke --xvfb "${archive[@]}"
rm -- "$fixture/bin/Xvfb"
refuses 'Missing required private display tool: Xvfb.' \
    drive "${archive[@]}" -- "$driver"
printf 'Private display refusal checks passed (4 scenarios).\n'
