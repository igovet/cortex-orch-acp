#!/bin/sh
set -eu
jobs="$1"
mkdir -p /build/home /build/cache
exec 9>/build/cache/.build.lock
flock -x 9
rm -rf /build/work
mkdir -p /build/work /build/artifacts
export CARGO_HOME=/build/cache/cargo
export CARGO_TARGET_DIR=/build/cache/target
export CARGO_BUILD_JOBS="$jobs"
export CARGO_PROFILE_RELEASE_DEBUG=0
export NPM_CONFIG_CACHE=/build/cache/npm
export NPM_CONFIG_PREFER_OFFLINE=true
# Cargo checks source mtimes. Reuse a timestamp only when the content and mode
# match the last snapshot used with this target directory, including after a retry.
cp -R /source/. /build/work/
node --input-type=module <<'SYNC'
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import path from 'node:path';
const state = '/build/cache/input-times.json';
const previous = fs.existsSync(state) ? new Map(JSON.parse(fs.readFileSync(state, 'utf8'))) : new Map();
const next = new Map();
function record(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
        const file = path.join(directory, entry.name);
        if (entry.isDirectory()) { record(file); continue; }
        if (!entry.isFile()) throw new Error('unexpected build input: ' + file);
        const fingerprint = createHash('sha256').update(fs.readFileSync(file)).digest('hex') + ':' + fs.statSync(file).mode;
        const prior = previous.get(file);
        const time = prior?.fingerprint === fingerprint ? prior.time : Date.now() / 1000;
        fs.utimesSync(file, time, time);
        next.set(file, {fingerprint, time});
    }
}
record('/build/work');
fs.writeFileSync(state + '.tmp', JSON.stringify([...next]));
fs.renameSync(state + '.tmp', state);
SYNC
cd /build/work
{
    . /etc/os-release
    printf 'Ubuntu %s\n' "$VERSION_ID"
    rustc --version
    cargo --version
    node --version
    npm --version
    pkg-config --modversion webkit2gtk-4.1 javascriptcoregtk-4.1 gtk+-3.0 openssl
    dpkg-query -W -f='${Package} ${Version}\n' libwebkit2gtk-4.1-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
} | tee /build/versions.txt
# The Rust build/packaging tool replaces every former Python build helper.
cargo build --release --locked -p cortex-xtask
xtask=/build/cache/target/release/cortex-xtask
"$xtask" bundle-engines --root /build/work --output /build/work/engines
"$xtask" bundle-voice --root /build/work --output /build/work/voice
npm --prefix packages/workspace-ui ci --no-audit --no-fund
npm --prefix packages/workspace-domain ci --no-audit --no-fund
npm --prefix apps/cortex-workspace-desktop ci --no-audit --no-fund
# Shared Paseo presentation types resolve its pinned SDK/schema dev dependencies.
"$xtask" npm-ci --root /build/work
npm --prefix apps/cortex-workspace-desktop run typecheck
# Workspace tests unify custom-protocol into the prototype member.
if [ "${2:-}" = test ]; then
    npm --prefix prototypes/cortex-workspace/desktop ci --no-audit --no-fund
    npm --prefix prototypes/cortex-workspace/desktop run build
fi
cd apps/cortex-workspace-desktop
"$xtask" package-channel overlay "$CORTEX_WORKSPACE_CHANNEL" --root /build/work > /build/channel.json
npm run tauri -- build --no-bundle --config /build/channel.json -- --locked
cd /build/work
cargo build --release --locked -p workspace-service
# Standalone no-paid peer for installed acceptance, never included in the Debian payload.
cargo build --release --locked -p cortex-engine --bin fake-engine
cp /build/cache/target/release/fake-engine /build/artifacts/native-fake-engine
if [ "${2:-}" = test ]; then
    CARGO_PROFILE_DEV_DEBUG=0 cargo test --locked --workspace
fi
# Derive versioned runtime library dependencies from the actual ELF binaries.
mkdir -p debian
printf 'Source: cortex-workspace\nSection: devel\nPriority: optional\nMaintainer: Cortex\n\nPackage: cortex-workspace\nArchitecture: any\nDescription: Cortex Workspace\n' > debian/control
"$xtask" shlibdeps --target /build/cache/target > /build/shlibs.depends
"$xtask" package-channel package "$CORTEX_WORKSPACE_CHANNEL" --root /build/work --target /build/cache/target --output /build/artifacts --depends-file /build/shlibs.depends --engines /build/work/engines --build-stamp "$CORTEX_WORKSPACE_BUILD_STAMP"
cd /build/artifacts
sha256sum ./*
