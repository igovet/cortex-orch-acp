#!/bin/sh
set -eu
jobs="$1"
rm -rf /build/work
mkdir -p /build/home /build/work /build/artifacts /build/cargo
export CARGO_HOME=/build/cargo
export CARGO_TARGET_DIR=/build/target
export CARGO_BUILD_JOBS="$jobs"
export CARGO_PROFILE_RELEASE_DEBUG=0
cp -R /source/. /build/work/
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
xtask=/build/target/release/cortex-xtask
"$xtask" bundle-engines --root /build/work --output /build/work/engines
"$xtask" bundle-voice --root /build/work --output /build/work/voice
npm --prefix packages/workspace-ui ci --no-audit --no-fund
npm --prefix packages/workspace-domain ci --no-audit --no-fund
npm --prefix apps/cortex-workspace-desktop ci --no-audit --no-fund
# Shared Paseo presentation types resolve its pinned SDK/schema dev dependencies.
"$xtask" npm-ci --root /build/work
npm --prefix apps/cortex-workspace-desktop run typecheck
# cargo test --workspace unifies custom-protocol into the prototype member.
npm --prefix prototypes/cortex-workspace/desktop ci --no-audit --no-fund
npm --prefix prototypes/cortex-workspace/desktop run build
cd apps/cortex-workspace-desktop
"$xtask" package-channel overlay "$CORTEX_WORKSPACE_CHANNEL" --root /build/work > /build/channel.json
npm run tauri -- build --no-bundle --config /build/channel.json -- --locked
cd /build/work
cargo build --release --locked -p workspace-service
# Standalone no-paid peer for installed acceptance, never included in the Debian payload.
cargo build --release --locked -p cortex-engine --bin fake-engine
cp /build/target/release/fake-engine /build/artifacts/native-fake-engine
if [ "${2:-}" = test ]; then
    CARGO_PROFILE_DEV_DEBUG=0 cargo test --locked --workspace
fi
# Derive versioned runtime library dependencies from the actual ELF binaries.
mkdir -p debian
printf 'Source: cortex-workspace\nSection: devel\nPriority: optional\nMaintainer: Cortex\n\nPackage: cortex-workspace\nArchitecture: any\nDescription: Cortex Workspace\n' > debian/control
"$xtask" shlibdeps --target /build/target > /build/shlibs.depends
"$xtask" package-channel package "$CORTEX_WORKSPACE_CHANNEL" --root /build/work --target /build/target --output /build/artifacts --depends-file /build/shlibs.depends --engines /build/work/engines --build-stamp "$CORTEX_WORKSPACE_BUILD_STAMP"
cd /build/artifacts
sha256sum ./*
