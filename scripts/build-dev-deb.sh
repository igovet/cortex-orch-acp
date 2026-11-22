#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"
# The build driver is the Rust cortex-xtask tool; no scripting runtime is involved.
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
xtask() { cargo run --locked --quiet -p cortex-xtask -- "$@"; }

# The wrapper owns channel selection and a fresh, invocation-specific output.
for arg do
    case "$arg" in
        --release|--release=*|--local-release|--local-release=*|--ci-dev|--ci-dev=*|--resume|--resume=*)
            printf '%s\n' 'Channel and --resume options are reserved by this wrapper.' >&2
            exit 2 ;;
        -h|--help)
            exec cargo run --locked --quiet -p cortex-xtask -- build-deb "$@" ;;
    esac
done
if [ -L .workspace-build ] || [ -L .workspace-build/local-dev ]; then
    printf '%s\n' 'Build output must not be a symlink.' >&2
    exit 1
fi
mkdir -p .workspace-build/local-dev
# Share the directory lock with Dev qualification cleanup. A directory FD does
# not create or follow a separately replaceable lock-file path.
exec 9< .workspace-build/local-dev
flock -x 9
# build-deb keeps the newest Dev builds after success; this wrapper already holds
# the local-dev lock, so their local-dev copies are not in use by a qualification.
export CORTEX_BUILD_LOCAL_DEV_LOCK_HELD=1
build=$(xtask new-build-dir .workspace-build)
printf 'Dev build directory: %s\n' "$build" >&2
xtask build-deb --resume "$build" "$@"
set -- ".workspace-build/$build/artifacts/"*.deb
if [ ! -f "$1" ]; then
    printf '%s\n' 'Builder produced no Debian archive.' >&2
    exit 1
fi
dest="$root/.workspace-build/local-dev/$build"
mkdir "$dest"
cp ".workspace-build/$build/manifest.json" "$dest/"
for archive do
    cp "$archive" "$dest/"
    printf '%s/%s\n' "$dest" "${archive##*/}"
done
