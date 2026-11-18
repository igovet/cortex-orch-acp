#!/bin/sh
# Build the Cortex Workspace installables on this machine: the Ubuntu desktop package, the Android APK, or both.
#
# Usage: scripts/build-local.sh [desktop|apk|all] [--release] [--debug-apk]
#   desktop      the .deb, built in the shared container (needs Docker)
#   apk          the Android APK (needs the Android SDK, NDK and JDK 17)
#   all          both, the desktop first (default)
#   --release    the release channel instead of dev, for whichever is built
#   --debug-apk  a debug APK signed with Android's debug key, so no keystore is needed
#
# Each part prints the path of what it produced. A part that fails stops the run with its status.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
what=all release= debug=
for arg do
    case "$arg" in
        desktop|apk|all) what=$arg ;;
        --release) release=1 ;;
        --debug-apk) debug=1 ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) printf '%s\n' "Unknown option: $arg" >&2; exit 2 ;;
    esac
done
if [ "$what" != apk ]; then
    printf '%s\n' '== Desktop package =='
    if [ -n "$release" ]; then "$root/scripts/build-prod-deb.sh"; else "$root/scripts/build-dev-deb.sh"; fi
fi
if [ "$what" != desktop ]; then
    printf '%s\n' '== Android APK =='
    set --
    [ -z "$release" ] || set -- "$@" --release
    [ -z "$debug" ] || set -- "$@" --debug
    "$root/scripts/build-android-apk.sh" "$@"
fi
