#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"

# The wrapper owns channel selection and a fresh, invocation-specific output.
for arg do
    case "$arg" in
        --release|--release=*|--local-release|--local-release=*|--ci-dev|--ci-dev=*|--resume|--resume=*)
            printf '%s\n' 'Channel and --resume options are reserved by this wrapper.' >&2
            exit 2 ;;
        -h|--help)
            exec python3 -B scripts/build_workspace_desktop.py "$@" ;;
    esac
done
if [ -L .workspace-build ] || [ -L .workspace-build/local-dev ]; then
    printf '%s\n' 'Build output must not be a symlink.' >&2
    exit 1
fi
mkdir -p .workspace-build/local-dev
build=$(python3 -B -c 'import tempfile; from pathlib import Path; print(Path(tempfile.mkdtemp(prefix="build-", dir=".workspace-build")).name)')
python3 -B scripts/build_workspace_desktop.py --resume "$build" "$@"
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
