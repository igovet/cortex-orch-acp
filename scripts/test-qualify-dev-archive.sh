#!/usr/bin/env bash
# Offline fixtures: no native accounts, desktop, extraction or foreground driver.
set -Eeuo pipefail
umask 022
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture"/{bin,scripts,home,images}
cp "$root/scripts/qualify-dev-deb.sh" "$fixture/scripts/"
launcher="$fixture/scripts/qualify-dev-deb.sh"
build="$fixture/.workspace-build/local-dev/build-test0001"
smoke="$fixture/.workspace-build/dev-evidence.test0001"
mkdir -p "$build" "$smoke"
archive="$build/cortex-workspace-dev_1.0.6_all.deb"
printf 'current archive fixture\n' >"$archive"
digest=$(sha256sum <"$archive" | cut -d' ' -f1)
other=$(printf 'different smoke archive fixture\n' | sha256sum | cut -d' ' -f1)
for tool in awk bash basename cat cmp cp cut df dirname find flock id jq mkdir mktemp realpath rm sha256sum stat tee timeout; do
    ln -s "$(command -v "$tool")" "$fixture/bin/$tool"
done
for tool in Xvfb xauth mcookie xdpyinfo xdotool xprop; do
    ln -s /usr/bin/false "$fixture/bin/$tool"
done
cat >"$fixture/bin/gh" <<'EOF'
#!/bin/bash
printf '%s\0' "$@" >>"$FIXTURE/gh.argv"
printf 'stub auth stdout\n'
printf 'stub auth stderr\n' >&2
exit 17
EOF
cat >"$fixture/bin/host-diagnostic" <<'EOF'
#!/bin/bash
printf '%s\n' "$0" >>"$FIXTURE/diagnostics"
exit 1
EOF
chmod 0700 "$fixture"/bin/{gh,host-diagnostic}
for tool in codex claude sudo docker; do
    ln -s host-diagnostic "$fixture/bin/$tool"
done
cat >"$fixture/bin/dpkg-deb" <<'EOF'
#!/bin/bash
if [[ $1 == -f ]]; then printf 'cortex-workspace-dev\n'; exit 0; fi
: >"$FIXTURE/extraction.started"
exit 99
EOF
cat >"$fixture/scripts/cortex-xtask.sh" <<'EOF'
#!/bin/bash
sha256sum <"$3" | cut -d' ' -f1 >"$FIXTURE/bundled.started"
exit 98
EOF
cat >"$fixture/bin/setsid" <<'EOF'
#!/bin/bash
: >"$FIXTURE/launch.started"
exit 99
EOF
cat >"$fixture/driver" <<'EOF'
#!/bin/bash
: >"$FIXTURE/driver.started"
exit 99
EOF
chmod 0700 "$fixture"/bin/{dpkg-deb,setsid} "$fixture"/scripts/cortex-xtask.sh "$fixture"/driver
write_manifest() {
    printf '{"channel":"dev","build_stamp":"test","artifacts":[{"file":"%s","sha256":"%s"}]}\n' "${archive##*/}" "$1" >"$build/manifest.json"
}
refuses() {
    local expected=$1 rc=0 output
    shift
    output=$(env -i HOME="$fixture/home" PATH="$fixture/bin" FIXTURE="$fixture" \
        /bin/bash "$launcher" "$@" 2>&1) || rc=$?
    if [[ $rc != 1 || $output != *"Dev qualification: $expected"* ||
          -e $fixture/extraction.started || -e $fixture/launch.started || -e $fixture/driver.started ]]; then
        printf 'Expected refusal (%s), got exit %s: %s\n' "$expected" "$rc" "$output" >&2
        return 1
    fi
}

refuses 'Missing or unsafe Dev build manifest.' install --archive "$archive"
write_manifest "$other"
refuses 'Archive hash differs from its Dev build manifest.' install --archive newest
[[ ! -e $fixture/diagnostics ]]
rm "$build/manifest.json"

source_args=(smoke --real-display --archive "$archive")
refuses 'Missing or unsafe Dev build manifest.' "${source_args[@]}"
refuses 'No unambiguous local Dev archive to reuse;' smoke --real-display --archive newest
write_manifest "$digest"
mv "$build/manifest.json" "$fixture/actual.json"
ln -s "$fixture/actual.json" "$build/manifest.json"
refuses 'Missing or unsafe Dev build manifest.' "${source_args[@]}"
rm "$build/manifest.json"
write_manifest "$other"
refuses 'Archive hash differs from its Dev build manifest.' "${source_args[@]}"
write_manifest "$digest"
chmod 0666 "$build/manifest.json"
refuses 'Missing or unsafe Dev build manifest.' "${source_args[@]}"
chmod 0000 "$build/manifest.json"
refuses 'Missing or unsafe Dev build manifest.' "${source_args[@]}"
chmod 0644 "$build/manifest.json"

drive_args=(drive --archive newest --smoke-evidence "$smoke" -- "$fixture/driver")
refuses 'Expected a trusted private Xvfb smoke pass receipt.' "${drive_args[@]}"
printf '%s\n' "$other" >"$smoke/smoke-passed.sha256"
refuses 'Drive archive differs from the private Xvfb smoke-passed archive.' "${drive_args[@]}"
[[ ! -e $fixture/bundled.started ]]
printf '%s\n' "$digest" >"$smoke/smoke-passed.sha256"
# Both real launcher branches reach host-facts capture. The bundled-engine
# stub stops each invocation before extraction, desktop startup or driver dispatch.
[[ ! -e $fixture/gh.argv && ! -e $fixture/diagnostics ]]
printf '%s\0' auth status --active --hostname github.com >"$fixture/expected-gh.argv"
for invocation in smoke drive; do
    case $invocation in
        smoke) args=("${source_args[@]}") ;;
        drive) args=("${drive_args[@]}") ;;
    esac
    rm -f -- "$fixture/gh.argv" "$fixture/bundled.started"
    refuses 'Exact archive bundled-engine gate failed.' "${args[@]}"
    [[ $(<"$fixture/bundled.started") == "$digest" ]]
    cmp -s "$fixture/expected-gh.argv" "$fixture/gh.argv" || {
        printf '%s did not make exactly one scoped GitHub status call\n' "$invocation" >&2
        exit 1
    }
    [[ ! -e $fixture/diagnostics ]]
done
facts=("$fixture"/.workspace-build/dev-evidence.*/host-facts.txt)
[[ ${#facts[@]} == 2 ]]
for fact in "${facts[@]}"; do
    grep -Fxq 'exit gh (active github.com): 17' "$fact"
    if grep -Fq 'stub auth ' "$fact"; then
        printf 'GitHub CLI output leaked into host facts\n' >&2
        exit 1
    fi
done

# The separately requested broad probe must still report gaps and exit zero.
probe_output=$(env -i HOME="$fixture/home" PATH="$fixture/bin" FIXTURE="$fixture" \
    /bin/bash "$launcher" probe)
[[ $probe_output == *'exit gh: 17'* && $probe_output != *'stub auth '* ]]
cmp -s "$fixture/diagnostics" <(printf '%s\n' "$fixture"/bin/{codex,claude,sudo,docker})

# Only the verified private snapshot may be handed to apt. Replace the sudo
# diagnostic link (not its shared target) with a verifier; never run apt.
rm "$fixture/bin/sudo"
cat >"$fixture/bin/sudo" <<'EOF'
#!/bin/bash
set -eu
[[ $# == 7 && $1 == -n && $2 == apt-get && $3 == install && $4 == -y &&
   $5 == --no-install-recommends && $6 == -- && $7 != "$SOURCE" &&
   $(stat -c %a -- "${7%/*}") == 700 ]] || exit 1
cmp -s "$SOURCE" "$7"
printf '%s\n' "$7" >"$FIXTURE/snapshot.path"
EOF
chmod 0700 "$fixture/bin/sudo"
env -i HOME="$fixture/home" PATH="$fixture/bin" FIXTURE="$fixture" SOURCE="$archive" \
    /bin/bash "$launcher" install --archive newest
[[ -s $fixture/snapshot.path && ! -e $(<"$fixture/snapshot.path") ]]

# The public cleanup command must classify originals even without either local copy.
cleanup_root="$fixture/.workspace-build"
cleanup_id=build-clean001
original="$cleanup_root/$cleanup_id"
copy="$cleanup_root/local-dev/$cleanup_id"
dev_deb=${archive##*/}
run_cleanup() {
    env -i HOME="$fixture/home" PATH="$fixture/bin" FIXTURE="$fixture" \
        /bin/bash "$launcher" cleanup "$cleanup_id"
}
cleanup_refuses() {
    local reason=$1 rc=0 output
    output=$(run_cleanup 2>&1) || rc=$?
    if [[ $rc != 1 || $output != *"Refused cleanup of $cleanup_id."* || ! -d $original ]]; then
        printf 'Expected cleanup to retain %s original (exit 1); got exit %s: %s\n' "$reason" "$rc" "$output" >&2
        return 1
    fi
}
mkdir -p "$original/artifacts"
[[ ! -e $copy && ! -e $cleanup_root/local-prod/$cleanup_id ]]
printf '{"channel":"default","artifacts":[{"file":"%s"}]}\n' "$dev_deb" >"$original/manifest.json"
cleanup_refuses 'non-Dev'
rm "$original/manifest.json"
cleanup_refuses 'manifestless'
printf '{"channel":"dev","artifacts":[{"file":"%s"},{"file":"%s"}]}\n' \
    "$dev_deb" "$dev_deb" >"$original/manifest.json"
cleanup_refuses 'ambiguous'
cp "$build/manifest.json" "$original/manifest.json"
chmod 0000 "$original/manifest.json"
cleanup_refuses 'unreadable manifest'
chmod 0644 "$original/manifest.json"
cp "$archive" "$original/artifacts/$dev_deb"
mkdir "$original/artifacts/nested"
cp "$archive" "$original/artifacts/nested/.unexpected.deb"
cleanup_refuses 'unexpected archive'
[[ -f $original/artifacts/$dev_deb && -f $original/artifacts/nested/.unexpected.deb ]]
rm "$original/artifacts/nested/.unexpected.deb"
run_cleanup
[[ ! -e $original && ! -e $copy ]]

mkdir -p "$original/artifacts" "$copy"
cp "$archive" "$original/artifacts/$dev_deb"
cp "$archive" "$copy/$dev_deb"
cp "$build/manifest.json" "$original/manifest.json"
cp "$original/manifest.json" "$copy/manifest.json"
printf '\n' >>"$copy/manifest.json"
cleanup_refuses 'mismatching paired manifest'
[[ -f $copy/$dev_deb ]]
cp "$original/manifest.json" "$copy/manifest.json"
mkdir "$copy/nested"
cp "$archive" "$copy/nested/.unlisted.deb"
cleanup_refuses 'unlisted archive in paired copy'
[[ -f $copy/nested/.unlisted.deb && -f $original/artifacts/$dev_deb ]]
rm "$copy/nested/.unlisted.deb"
run_cleanup
[[ ! -e $original && ! -e $copy ]]

mkdir -p "$copy"
cp "$build/manifest.json" "$copy/manifest.json"
cp "$archive" "$copy/.unlisted.deb"
refuses "Refused cleanup of $cleanup_id." cleanup "$cleanup_id"
[[ ! -e $original && -f $copy/.unlisted.deb ]]
rm "$copy/.unlisted.deb"
run_cleanup
[[ ! -e $copy ]]

# Source only the launcher's helper definitions; use a fake capture producer,
# but let the installed ffmpeg validate PNGs and generate the complete control.
# No X server is contacted. A successful decode must produce a frame.
source <(sed -n '/^ledger() /,/^declare -a unobserved=/p' "$launcher")
real_ffmpeg=$(command -v ffmpeg)
export real_ffmpeg fixture
"$real_ffmpeg" -nostdin -v error -f lavfi -i color=red:s=16x16 -frames:v 1 -threads 1 -update 1 "$fixture/images/full.png"
head -c -12 "$fixture/images/full.png" >"$fixture/images/partial.png"
{ head -c 8 "$fixture/images/full.png"; tail -c 12 "$fixture/images/full.png"; } >"$fixture/images/undecodable.png"
cat >"$fixture/bin/ffmpeg" <<'EOF'
#!/bin/bash
case " $* " in
    *' -f x11grab '*)
        cp -- "$fixture/images/$IMAGE.png" "${!#}"
        exit "$CAPTURE_RC" ;;
    *) exec "$real_ffmpeg" "$@" ;;
esac
EOF
chmod 0700 "$fixture/bin/ffmpeg"
export PATH="$fixture/bin:$PATH" IMAGE CAPTURE_RC
display=:fixture
view=()
for sample in partial:1 full:1 partial:0 undecodable:0 full:0; do
    IMAGE=${sample%:*} CAPTURE_RC=${sample#*:}
    evidence="$fixture/images/$IMAGE-$CAPTURE_RC"
    mkdir "$evidence"
    if capture_screenshot; then
        [[ $sample == full:0 ]]
        cmp -s "$evidence/desktop.png" "$fixture/images/full.png"
        grep -q '^screenshot [0-9a-f]\{64\} ' "$evidence/ledger"
    else
        [[ $sample != full:0 && ! -e $evidence/desktop.png ]]
        if grep -q '^screenshot ' "$evidence/ledger"; then
            printf 'Invalid capture produced a digest (%s)\n' "$sample" >&2
            exit 1
        fi
    fi
done
printf 'Archive, host probe, cleanup and screenshot gate checks passed.\n'
