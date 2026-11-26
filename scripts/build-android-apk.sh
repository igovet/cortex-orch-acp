#!/bin/sh
# Build the Cortex Workspace Android app (the shared interface in its phone shell) into one APK.
#
# Usage: scripts/build-android-apk.sh [--release] [--debug] [--all-abis] [--init-signing]
#   (no option)     dev channel, signed with the release key
#   --release       release channel (its own application id, installs beside dev); built outside
#                   GitHub Actions it is an owner-local build and its file name ends in .local
#   --debug         debug build signed with Android's debug key; needs no keystore
#   --all-abis      arm64, armv7, x86 and x86_64 instead of arm64 only
#   --init-signing  create the signing keystore once, print how to give it to CI, and stop
#
# Signing. An update installs over an existing app only when both carry the same key, so the same
# keystore must sign local and CI builds. It is read from:
#   CORTEX_ANDROID_KEYSTORE           path (default ~/.config/cortex-workspace/android/release.jks)
#   CORTEX_ANDROID_KEYSTORE_PASSWORD  password (default: the file <keystore>.password, mode 0600)
#   CORTEX_ANDROID_KEY_ALIAS          alias (default cortex-workspace)
# The keystore and its password never belong in the repository.
#
# Needs: cargo with the pinned toolchain and its Android target(s), npm, JDK 17, the Android SDK
# (ANDROID_HOME, default ~/Android/Sdk) with an NDK and build-tools. A missing one exits 2.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
app="$root/apps/cortex-workspace-android"
fail() { printf '%s\n' "$*" >&2; exit 2; }

channel=dev debug= abis=aarch64 init=
for arg do
    case "$arg" in
        --release) channel=release ;;
        --debug) debug=1 ;;
        --all-abis) abis="aarch64 armv7 i686 x86_64" ;;
        --init-signing) init=1 ;;
        -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) fail "Unknown option: $arg" ;;
    esac
done

keystore=${CORTEX_ANDROID_KEYSTORE:-$HOME/.config/cortex-workspace/android/release.jks}
alias=${CORTEX_ANDROID_KEY_ALIAS:-cortex-workspace}
command -v keytool >/dev/null 2>&1 || fail 'keytool not found: install JDK 17.'

if [ -n "$init" ]; then
    [ ! -e "$keystore" ] || fail "A keystore already exists at $keystore. It is never overwritten: losing it means no update can install over existing apps."
    umask 077
    mkdir -p "$(dirname -- "$keystore")"
    password=$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')
    printf '%s\n' "$password" > "$keystore.password"
    keytool -genkeypair -keystore "$keystore" -storetype PKCS12 -storepass "$password" -keypass "$password" \
        -alias "$alias" -keyalg RSA -keysize 4096 -validity 10950 -dname 'CN=Cortex Workspace' >/dev/null
    printf '%s\n' "Keystore: $keystore" "Password file: $keystore.password" '' \
        'Back both up somewhere safe. For GitHub Actions add these repository secrets:' \
        "  ANDROID_KEYSTORE_BASE64    output of: base64 -w0 $keystore" \
        "  ANDROID_KEYSTORE_PASSWORD  content of $keystore.password" \
        "  ANDROID_KEY_ALIAS          $alias"
    exit 0
fi

command -v cargo >/dev/null 2>&1 || PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
command -v cargo >/dev/null 2>&1 || fail 'cargo not found.'
command -v npm >/dev/null 2>&1 || fail 'npm not found.'
command -v java >/dev/null 2>&1 || fail 'java not found: install JDK 17.'
ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/Sdk}
[ -d "$ANDROID_HOME" ] || fail "Android SDK not found at $ANDROID_HOME (set ANDROID_HOME)."
if [ -z "${NDK_HOME:-}" ]; then
    NDK_HOME=$(ls -d "$ANDROID_HOME"/ndk/*/ 2>/dev/null | sort -V | tail -n 1)
    NDK_HOME=${NDK_HOME%/}
fi
[ -n "$NDK_HOME" ] && [ -d "$NDK_HOME" ] || fail "No Android NDK under $ANDROID_HOME/ndk (set NDK_HOME)."
tools=$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -n 1)
[ -n "$tools" ] && [ -x "${tools}apksigner" ] && [ -x "${tools}zipalign" ] || fail "No Android build-tools with apksigner and zipalign under $ANDROID_HOME/build-tools."
if [ -z "${JAVA_HOME:-}" ]; then
    JAVA_HOME=$(dirname -- "$(dirname -- "$(readlink -f "$(command -v java)")")")
fi
installed=$(cd "$root" && rustup target list --installed 2>/dev/null || true)
for abi in $abis; do
    case "$abi" in
        aarch64) triple=aarch64-linux-android ;;
        armv7) triple=armv7-linux-androideabi ;;
        i686) triple=i686-linux-android ;;
        x86_64) triple=x86_64-linux-android ;;
    esac
    printf '%s\n' "$installed" | grep -qx "$triple" || fail "Rust target $triple is not installed: run 'rustup target add $triple' in $root."
done
if [ -z "$debug" ]; then
    [ -f "$keystore" ] || fail "No signing keystore at $keystore. Create it once with: scripts/build-android-apk.sh --init-signing (or pass --debug)."
    if [ -z "${CORTEX_ANDROID_KEYSTORE_PASSWORD:-}" ]; then
        [ -f "$keystore.password" ] || fail "No keystore password: set CORTEX_ANDROID_KEYSTORE_PASSWORD or provide $keystore.password."
        CORTEX_ANDROID_KEYSTORE_PASSWORD=$(cat "$keystore.password")
    fi
fi

out="$root/.workspace-build/android"
[ ! -L "$root/.workspace-build" ] && [ ! -L "$out" ] || fail 'Build output must not be a symlink.'
mkdir -p "$out"
export ANDROID_HOME NDK_HOME JAVA_HOME PATH
export CORTEX_WORKSPACE_CHANNEL="$channel"
# The release channel is guarded, as for the desktop package: official only from a clean `main`
# in GitHub Actions, otherwise an explicit local owner build, which is marked `local`.
local=
if [ "$channel" = release ]; then
    if [ "${GITHUB_ACTIONS:-}" = true ]; then
        [ "${GITHUB_REF:-}" = refs/heads/main ] || fail 'The release channel is built in GitHub Actions only from main.'
        [ -z "$(git -C "$root" status --porcelain)" ] || fail 'The release channel needs a clean checkout.'
        export CORTEX_WORKSPACE_RELEASE_AUTHORIZED=github-main-clean
    else
        export CORTEX_WORKSPACE_RELEASE_AUTHORIZED=local-owner-test
        local=1
    fi
fi
# A private Gradle home keeps this build independent of the state of ~/.gradle.
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$root/.workspace-build/gradle-home}"
umask 022
cd "$root"

for package in packages/workspace-ui packages/workspace-domain apps/cortex-workspace-android; do
    [ -d "$package/node_modules" ] || npm --prefix "$package" ci --no-audit --no-fund
done
npm --prefix apps/cortex-workspace-android run typecheck

# The channel decides the application id and name, exactly as for the desktop package.
overlay="$out/channel-$channel.json"
node -e '
const channel = require(process.argv[1])[process.argv[2]];
const version = require(process.argv[3]).version.split("+")[0];
process.stdout.write(JSON.stringify({ productName: channel.title, identifier: channel.identifier, version }));
' "$root/crates/workspace-protocol/channels.json" "$channel" "$root/package-identity.json" > "$overlay"
title=$(node -e 'process.stdout.write(require(process.argv[1]).productName)' "$overlay")
version=$(node -e 'process.stdout.write(require(process.argv[1]).version)' "$overlay")
package=$(node -e 'process.stdout.write(require(process.argv[1])[process.argv[2]].package)' "$root/crates/workspace-protocol/channels.json" "$channel")

cd "$app"
# The Android project is generated, never kept: it follows the channel's application id.
rm -rf src-tauri/gen/android
npm run tauri -- android init --ci --skip-targets-install --config "$overlay"
manifest=src-tauri/gen/android/app/src/main/AndroidManifest.xml
# Pairing keys live in the app's private data; they must never leave the phone in a backup.
sed -i 's/<application/<application android:allowBackup="false" android:fullBackupContent="false"/' "$manifest"
grep -q 'android:allowBackup="false"' "$manifest" || fail 'Could not switch app backup off in the generated manifest.'
# The activity that keeps the page above the on-screen keyboard replaces the generated one.
identifier=$(node -e 'process.stdout.write(require(process.argv[1]).identifier)' "$overlay")
activity=$(find src-tauri/gen/android/app/src/main/java -name MainActivity.kt | head -n 1)
[ -n "$activity" ] || fail 'The generated Android project has no MainActivity.kt.'
sed "s/__PACKAGE__/$identifier/" src-tauri/android/MainActivity.kt > "$activity"
grep -q "^package $identifier$" "$activity" || fail 'Could not install the app activity.'
# Dictation uses the phone's own speech recognition through a small native plugin of the app.
speech=src-tauri/gen/android/app/src/main/java/dev/cortex/workspace/speech
mkdir -p "$speech"
cp src-tauri/android/SpeechPlugin.kt "$speech/SpeechPlugin.kt"
# The phone's own notifications: a foreground service (Android 14's special-use type, with the
# reason Android asks for), the notification permission, the plugin and its status bar icon.
notify=src-tauri/gen/android/app/src/main/java/dev/cortex/workspace/notify
mkdir -p "$notify" src-tauri/gen/android/app/src/main/res/drawable
cp src-tauri/android/NotifyService.kt "$notify/NotifyService.kt"
cp src-tauri/android/ic_stat_cortex.xml src-tauri/gen/android/app/src/main/res/drawable/ic_stat_cortex.xml
sed -i 's#<uses-permission android:name="android.permission.INTERNET" />#&\
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />\
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_SPECIAL_USE" />\
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />#' "$manifest"
sed -i 's#</application>#    <service android:name="dev.cortex.workspace.notify.NotifyService" android:exported="false" android:foregroundServiceType="specialUse">\
            <property android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE" android:value="Keeps the end-to-end encrypted connection to the owner'"'"'s own paired computers open while the owner has turned phone notifications on, so the app itself can tell the owner when a task asks a question, finishes or fails. No push service is used." />\
        </service>\
    </application>#' "$manifest"
grep -q 'android.permission.POST_NOTIFICATIONS' "$manifest" && grep -q 'android:foregroundServiceType="specialUse"' "$manifest" \
    || fail 'Could not add the notification service to the generated manifest.'
# The launcher icon is the product's own, drawn from src-tauri/android/icon.json (the foreground
# has the margin Android's icon masks need).
icons="$out/icons-$channel"
rm -rf "$icons"
npm run tauri -- icon src-tauri/android/icon.json -o "$icons" >/dev/null
[ -f "$icons/android/mipmap-anydpi-v26/ic_launcher.xml" ] || fail 'The launcher icons were not generated.'
cp -R "$icons/android/." src-tauri/gen/android/app/src/main/res/
rm -rf "$icons"

set -- android build --apk --ci --config "$overlay"
for abi in $abis; do set -- "$@" --target "$abi"; done
[ -z "$debug" ] || set -- "$@" --debug
npm run tauri -- "$@"

flavour=release; [ -z "$debug" ] || flavour=debug
built=$(find src-tauri/gen/android/app/build/outputs/apk -name '*.apk' -path "*/$flavour/*" | head -n 1)
[ -n "$built" ] && [ -f "$built" ] || fail 'The build produced no APK.'
arch=arm64-v8a; [ "$abis" = aarch64 ] || arch=universal
name="${package}_${version}_${arch}"; [ -z "$local" ] || name="$name.local"; [ -z "$debug" ] || name="$name-debug"
final="$out/$name.apk"
rm -f "$final" "$final.idsig"
if [ -n "$debug" ]; then
    cp "$built" "$final"
else
    "${tools}zipalign" -f -p 4 "$built" "$final.aligned"
    CORTEX_ANDROID_KEYSTORE_PASSWORD="$CORTEX_ANDROID_KEYSTORE_PASSWORD" "${tools}apksigner" sign \
        --ks "$keystore" --ks-key-alias "$alias" --ks-pass env:CORTEX_ANDROID_KEYSTORE_PASSWORD \
        --key-pass env:CORTEX_ANDROID_KEYSTORE_PASSWORD --out "$final" "$final.aligned"
    rm -f "$final.aligned" "$final.idsig"
    "${tools}apksigner" verify "$final" >/dev/null || fail 'The signed APK did not verify.'
fi
(cd "$out" && sha256sum "$name.apk" > "$name.apk.sha256")
printf '%s\n' "$title $version ($flavour)" "$final"
