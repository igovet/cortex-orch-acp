#!/bin/sh
set -eu
umask 022
export PATH="$HOME/.cargo/bin:$PATH"
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"
# The build driver is the Rust cortex-xtask tool; no scripting runtime is involved.
xtask() { cargo run --locked --quiet -p cortex-xtask -- "$@"; }

# The wrapper owns channel selection and a fresh, invocation-specific output.
for arg do
    case "$arg" in
        --release|--release=*|--local-release|--local-release=*|--ci-dev|--ci-dev=*|--resume|--resume=*)
            printf '%s\n' 'Channel and --resume options are reserved by this wrapper.' >&2
            exit 2 ;;
        -h|--help)
            exec cargo run --locked --quiet -p cortex-xtask -- build-deb --local-release "$@" ;;
    esac
done
if [ -L .workspace-build ] || [ -L .workspace-build/local-prod ]; then
    printf '%s\n' 'Build output must not be a symlink.' >&2
    exit 1
fi
mkdir -p .workspace-build/local-prod
build=$(xtask new-build-dir .workspace-build)
xtask build-deb --local-release --resume "$build" "$@"
set -- ".workspace-build/$build/artifacts/"*.deb
if [ ! -f "$1" ]; then
    printf '%s\n' 'Builder produced no Debian archive.' >&2
    exit 1
fi
dest="$root/.workspace-build/local-prod/$build"
mkdir "$dest"
cp ".workspace-build/$build/manifest.json" "$dest/"
for archive do
    cp "$archive" "$dest/"
    printf '%s/%s\n' "$dest" "${archive##*/}"
done
