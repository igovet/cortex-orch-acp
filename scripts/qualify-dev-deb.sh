#!/usr/bin/env bash
# Real-environment qualification of a trusted, locally built Workspace Dev archive.
# Runs the extracted Dev package as the owner (real HOME, keyring, native and gh
# logins) with private Dev XDG roots, so the persistent release and Dev
# installations, their settings and stores, production ~/.paseo and owner projects
# are never used. Environment gaps are recorded facts, never dead ends.
set -Eeuo pipefail
umask 022
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"
export PATH="$HOME/.cargo/bin:$PATH"
builds="$root/.workspace-build"
local_dev="$builds/local-dev"
package=cortex-workspace-dev
uid=$(id -u)

usage() {
    cat <<'EOF'
Usage (real host, as the owner):
  bash scripts/qualify-dev-deb.sh probe
  bash scripts/qualify-dev-deb.sh smoke [--archive PATH|newest] [--xvfb|--real-display]
  bash scripts/qualify-dev-deb.sh drive --archive PATH|newest --smoke-evidence DIR [--xvfb] [--capture-opener] -- DRIVER [ARG ...]
  bash scripts/qualify-dev-deb.sh install --archive PATH|newest
  bash scripts/qualify-dev-deb.sh cleanup build-XXXXXXXX [build-XXXXXXXX ...]
probe prints broad host diagnostics (display, logins, sudo, Docker, free space, tools, local Dev archives); it never fails on a gap.
Smoke and drive do not run probe; their GitHub login fact is limited to the active account on github.com.
smoke extracts the exact Dev archive, starts its service and desktop with private Dev roots and the owner's real
  logins, and reads back settings and the observed light route. It sends no task and makes no paid call.
drive does the same, then runs a trusted absolute foreground DRIVER (only for an owner-approved paid scenario).
--archive newest reuses the newest local Dev archive. Only smoke can build without --archive.
Smoke and drive require a trusted Dev build manifest; drive requires the evidence directory of a
successful private Xvfb smoke for the exact same archive SHA-256.
Drive always requires private authenticated Xvfb, with no fallback. Smoke uses this mode with --xvfb or
  by default when Xvfb is available; smoke alone may select --real-display.
Both modes require the in-service verifier and native View PR capture before desktop input.
--capture-opener is retained for drive callers; capture is already mandatory. Available View PR addresses
  are recorded in opener.sha256 instead of handed to the owner's browser.
install installs the exact Dev archive with sudo (the owner's password prompt answers it); never the release package.
cleanup deletes explicitly named local Dev build pairs; cortex-xtask clean-builds keeps the newest per channel.
Exit status: 0 every step observed, 1 product or archive failure, 2 usage, 3 completed with unobserved steps (listed).
EOF
}
die() { printf 'Dev qualification: %s\n' "$*" >&2; exit 1; }
owned_dir() {
    [[ -d $1 && ! -L $1 && $(stat -c %u -- "$1") == "$uid" &&
       $(realpath -e -- "$1") == "$1" && $(stat -c %a -- "$1") =~ ^[0-7]*[0145][0145]$ ]]
}
safe_build_roots() {
    owned_dir "$builds" && owned_dir "$local_dev" || {
        echo "Unsafe or missing local Dev build roots." >&2; return 1;
    }
}
owned_file() {
    [[ -f $1 && ! -L $1 && -r $1 && $(stat -c %u -- "$1") == "$uid" &&
       $(stat -c %a -- "$1") =~ ^[0-7]?[4567][0145][0145]$ ]]
}
build_id() { [[ $1 =~ ^build-[a-z0-9_]{8}$ ]]; }
ledger() { printf '%s\n' "$*" >>"$evidence/ledger"; }

# Require exactly one complete, LF-terminated receipt from the service stdout.
candidate_verifier_receipt() {
    local line receipt=
    while IFS= read -r line; do
        [[ $line == qualification_nested_verifier=* ]] || continue
        [[ -z $receipt && $line =~ ^qualification_nested_verifier=native-command:[0-9a-f]{64}$ ]] ||
            return 1
        receipt=$line
    done <"$1"
    [[ -n $receipt && $line != qualification_nested_verifier=* ]] || return 1
    printf '%s\n' "$receipt"
}
# Record a command, its exit and its output digest (never the output itself).
record() {
    local label=$1 output=$2 rc=0
    shift 2
    { printf '+ label=%q' "$label"; printf ' %q' "$@"; printf '\n'; } >>"$evidence/ledger"
    "$@" >"$output" 2>&1 || rc=$?
    ledger "exit=$rc output-sha256=$(sha256sum <"$output" | cut -d' ' -f1) label=$label"
    return "$rc"
}
# Validate framing as well as decoding: a tolerant decoder can accept a PNG
# with missing IEND or trailing bytes, and an empty decode is not a frame.
valid_screenshot() {
    local png=$1
    [[ -f $png && ! -L $png && -s $png ]] &&
        [[ $(od -An -v -tx1 -N8 -- "$png" | tr -d ' \n') == 89504e470d0a1a0a ]] &&
        [[ $(tail -c 12 -- "$png" | od -An -v -tx1 | tr -d ' \n') == 0000000049454e44ae426082 ]] &&
        timeout --kill-after=2 10 ffmpeg -nostdin -v error -xerror -err_detect explode \
            -abort_on empty_output -f png_pipe -i "$png" -frames:v 1 -f null - >/dev/null 2>&1
}
capture_screenshot() {
    local png="$evidence/desktop.png" digest
    if record screenshot "$evidence/capture.out" timeout --kill-after=2 15 env "${view[@]}" \
        ffmpeg -nostdin -v error -f x11grab -i "$display" -frames:v 1 -update 1 "$png" &&
        valid_screenshot "$png" && digest=$(sha256sum -- "$png"); then
        ledger "screenshot $digest"
    else
        rm -f -- "$png"
        return 1
    fi
}
declare -a unobserved=()
gap() { unobserved+=("$*"); ledger "not-observed: $*"; printf 'not observed: %s\n' "$*"; }
new_evidence() {
    [[ ! -L $builds ]] || die 'Build output is a symlink.'
    mkdir -p "$builds"
    owned_dir "$builds" || die 'Unsafe build output.'
    evidence=$(mktemp -d "$builds/dev-evidence.XXXXXXXX")
    printf 'Evidence: %s\n' "$evidence"
}
dev_archive() {
    [[ -f $1 && ! -L $1 && $(stat -c %u -- "$1") == "$uid" &&
       $(dpkg-deb -f "$1" Package) == "$package" ]]
}
have() { command -v "$1" >/dev/null 2>&1; }
# Exit code of a bounded, silent command: a fact, never a credential.
status_of() { local rc=0; timeout --kill-after=2 "$1" "${@:2}" </dev/null >/dev/null 2>&1 || rc=$?; printf '%s' "$rc"; }

cleanup_manifest_archive() {
    owned_file "$1" || return 1
    jq -ers '
        select(length == 1) | .[0] | select(.channel == "dev")
        | .artifacts | select(type == "array")
        | [.[].file | select(test("^cortex-workspace-dev_[a-zA-Z0-9.+:_~-]+[.]deb$"))]
        | select(length == 1) | .[0]
    ' "$1"
}

# Only the literal manifest-listed relative path may contain a Debian archive.
cleanup_archive_tree() {
    local unexpected
    unexpected=$(cd -- "$1" && find . -name '*.deb' ! -path "./$2" -printf x -quit) || return 1
    [[ -z $unexpected ]]
}

cleanup_build() (
    local id=$1 item original copy cid containers archive manifest expected original_expected
    local -a archives=()
    build_id "$id" && safe_build_roots || return 1
    # Serialize with build-dev-deb.sh. Do not wait indefinitely on another build.
    exec 8< "$local_dev"
    flock -xn 8 || { echo "Local Dev build lock busy; retained $id." >&2; return 1; }
    original="$builds/$id"
    copy="$local_dev/$id"
    for item in "$original" "$copy"; do
        if [[ -e $item || -L $item ]]; then
            owned_dir "$item" || return 1
        fi
    done
    [[ ! -e "$builds/local-prod/$id" && ! -L "$builds/local-prod/$id" ]] || return 1
    if [[ ! -e $original && ! -e $copy ]]; then
        ledger "cleanup build=$id already absent"
        return 0
    fi
    # An original can outlive its local copy. Validate it independently before
    # allowing any deletion; absence of a local-prod copy is not channel proof.
    if [[ -e $original ]]; then
        original_expected=$(cleanup_manifest_archive "$original/manifest.json") || return 1
        cleanup_archive_tree "$original" "artifacts/$original_expected" || return 1
    fi
    manifest="$copy/manifest.json"
    if [[ -e $copy ]]; then
        expected=$(cleanup_manifest_archive "$manifest") || return 1
        cleanup_archive_tree "$copy" "$expected" || return 1
        archives+=("$copy/$expected")
        if [[ -e $original ]]; then
            cmp -s "$manifest" "$original/manifest.json" || return 1
        fi
        sha256sum -- "$manifest" >>"$evidence/ledger" || return 1
    else
        sha256sum -- "$original/manifest.json" >>"$evidence/ledger" || return 1
    fi
    if [[ -e $original/container.cid || -L $original/container.cid ]]; then
        [[ -f $original/container.cid && ! -L $original/container.cid ]] || return 1
        cid=$(<"$original/container.cid")
        [[ $cid =~ ^[a-f0-9]{64}$ ]] || return 1
        containers=$(timeout 10 docker ps -aq --no-trunc --filter "id=$cid") || return 1
        [[ -z $containers ]] || { echo "Container still exists for $id." >&2; return 1; }
    fi
    if [[ -e $original/artifacts || -L $original/artifacts ]]; then
        owned_dir "$original/artifacts" || return 1
        archives+=("$original/artifacts/$original_expected")
    fi
    for archive in "${archives[@]}"; do
        if [[ -e $archive || -L $archive ]]; then
            [[ ! -L $(dirname -- "$archive") ]] && dev_archive "$archive" || return 1
            sha256sum -- "$archive" >>"$evidence/ledger" || return 1
        fi
    done
    ledger "cleanup build=$id argv=rm,-rf,--one-file-system,--,$original,$copy"
    rm -rf --one-file-system -- "$original" "$copy" || return 1
    [[ ! -e $original && ! -e $copy ]] || return 1
    ledger "cleanup build=$id exit=0"
)

newest_archive() {
    local f dir newest= stamp best=
    safe_build_roots || return 1
    for f in "$local_dev"/build-*/"$package"_*.deb; do
        [[ -e $f || -L $f ]] || continue
        dir=${f%/*}
        # An unreadable candidate has no trustworthy sort key. Never silently
        # substitute another archive when the caller asked for newest.
        owned_dir "$dir" && owned_file "$f" && owned_file "$dir/manifest.json" || return 1
        stamp=$(jq -ers 'select(length == 1) | .[0] | select(.channel == "dev")
            | .build_stamp | select(type == "string" and length > 0)' "$dir/manifest.json") || return 1
        if [[ -z $best || $stamp > $best ]]; then best=$stamp; newest=$f; fi
    done
    [[ -n $newest ]] && printf '%s\n' "$newest"
}

local_host_facts() {
    local t path free
    printf 'tools:'
    for t in dpkg-deb jq Xvfb xauth mcookie xdotool xdpyinfo xprop ffmpeg ss gh docker sudo systemctl; do
        if have "$t"; then printf ' %s=yes' "$t"; else printf ' %s=no' "$t"; fi
    done
    printf '\n'
    printf 'display: x11=%s wayland=%s session=%s\n' "${DISPLAY:-none}" "${WAYLAND_DISPLAY:-none}" "${XDG_SESSION_TYPE:-unknown}"
    for path in "$root" /tmp; do
        free=$(df -Ph -- "$path" | awk 'NR==2 {print $4}') || free=unknown
        printf 'free %s: %s\n' "$path" "${free:-unknown}"
    done
}

github_login_fact() {
    local rc=not-installed
    if have gh; then rc=$(status_of 5 gh auth status --active --hostname github.com); fi
    printf 'exit gh (active github.com): %s\n' "$rc"
}

probe() {
    local t rc
    local_host_facts
    for t in "gh auth status" "codex login status" "claude auth status" "sudo -n true" "docker info --format {{.ServerVersion}}"; do
        # shellcheck disable=SC2086
        if have "${t%% *}"; then rc=$(status_of 5 $t); else rc=not-installed; fi
        printf 'exit %s: %s\n' "${t%% *}" "$rc"
    done
    printf 'newest local Dev archive: %s\n' "$(newest_archive || echo none)"
    echo 'sudo exit 1 means a password is needed: the owner password prompt supplies it.'
}

archive_arg() {
    case $1 in
        newest) archive=$(newest_archive) || die 'No unambiguous local Dev archive to reuse; check build manifests or select an exact Dev build path.' ;;
        *) [[ ! -L $1 ]] || die 'Expected a non-symlink archive path.'
           archive=$(realpath -e -- "$1") || die 'Archive does not exist.' ;;
    esac
}

snapshot_dev_archive() {
    local build_dir manifest archive_name expected
    # Only accept manifests in the project builder's owned local Dev output.
    safe_build_roots || die 'Expected safe local Dev build roots.'
    build_dir=${archive%/*}
    [[ ${build_dir%/*} == "$local_dev" ]] && build_id "${build_dir##*/}" &&
        owned_dir "$build_dir" || die 'Archive must be in the checkout local Dev build directory.'
    manifest="$build_dir/manifest.json"
    owned_file "$manifest" || die 'Missing or unsafe Dev build manifest.'
    archive_name=${archive##*/}
    [[ $archive_name =~ ^cortex-workspace-dev_[a-zA-Z0-9.+:_~-]+[.]deb$ ]] &&
        owned_file "$archive" || die 'Expected a safe cortex-workspace-dev archive file.'

    # Use private snapshots, not mutable build paths, for validation and extraction.
    cp -- "$manifest" "$scratch/manifest.json" &&
        cp --reflink=auto -- "$archive" "$scratch/$archive_name" || die 'Could not snapshot Dev build files.'
    candidate_deb="$scratch/$archive_name"
    expected=$(jq -ers --arg name "$archive_name" '
        select(length == 1) | .[0] | select(.channel == "dev")
        | [.artifacts[] | select(.file == $name) | .sha256]
        | select(length == 1) | .[0]
        | select(type == "string" and test("^[0-9a-f]{64}$"))
    ' "$scratch/manifest.json") || die 'Archive is not uniquely listed with a valid SHA-256 in its Dev build manifest.'
    actual=$(sha256sum <"$candidate_deb" | cut -d' ' -f1)
    [[ $actual == "$expected" ]] || die 'Archive hash differs from its Dev build manifest.'
    dev_archive "$candidate_deb" || die 'Wrong package: only cortex-workspace-dev is qualified here.'
    ledger "manifest-sha256=$(sha256sum <"$scratch/manifest.json" | cut -d' ' -f1)"
    printf '%s  %s\n' "$actual" "$archive" | tee "$evidence/archive.sha256" >>"$evidence/ledger"
}

case ${1:-} in
    -h|--help) usage; exit 0 ;;
    probe) probe; exit 0 ;;
    cleanup)
        shift; (($#)) || die 'Supply explicit stale Dev build IDs.'
        new_evidence
        for id in "$@"; do cleanup_build "$id" || die "Refused cleanup of $id."; done
        exit 0 ;;
    install)
        shift
        [[ ${1:-} == --archive && $# == 2 ]] || { usage >&2; exit 2; }
        archive_arg "$2"
        sudo_args=(-n)
        if [[ -n ${SUDO_ASKPASS:-} ]]; then sudo_args=(-A); fi
        new_evidence
        scratch=$(mktemp -d /tmp/cwq.XXXXXXXX)
        trap 'rm -rf --one-file-system -- "$scratch"' EXIT
        snapshot_dev_archive
        record install "$evidence/install.out" sudo "${sudo_args[@]}" apt-get install -y --no-install-recommends -- "$candidate_deb" ||
            die "Install failed (see $evidence/install.out); without SUDO_ASKPASS, sudo needs a passwordless rule."
        echo "Installed $archive"
        exit 0 ;;
    smoke|drive) mode=$1; shift ;;
    *) usage >&2; exit 2 ;;
esac

archive= archive_spec= smoke_evidence= display_mode=auto
inherited_display=${DISPLAY:-}
while (($#)); do
    case $1 in
        --archive)
            [[ $# -ge 2 && -n $2 ]] || die 'Expected --archive PATH|newest.'
            archive_spec=$2; shift 2 ;;
        --smoke-evidence)
            [[ $mode == drive && $# -ge 2 && -n $2 ]] || die 'Expected drive --smoke-evidence DIR.'
            smoke_evidence=$2; shift 2 ;;
        --xvfb) display_mode=xvfb; shift ;;
        --real-display)
            [[ $mode == smoke ]] || die 'Drive requires private authenticated Xvfb; --real-display is not allowed.'
            display_mode=real; shift ;;
        --keep-archive) shift ;; # Archives are always retained now; kept for older callers.
        --capture-opener) [[ $mode == drive ]] || die '--capture-opener is a drive option.'; shift ;;
        --) break ;;
        *) usage >&2; exit 2 ;;
    esac
done
if [[ $mode == drive ]]; then
    [[ $# -ge 2 && $1 == -- && $2 == /* && -f $2 && -x $2 ]] ||
        die 'Expected drive [options] -- ABSOLUTE_EXECUTABLE [ARG ...].'
    shift
    display_mode=xvfb
else
    (($# == 0)) || die "Unexpected argument for $mode."
fi
# Reject missing isolation prerequisites before touching an archive or launching roles.
if [[ $display_mode == auto ]] && have Xvfb; then display_mode=xvfb; fi
if [[ $display_mode == xvfb ]]; then
    for name in Xvfb xauth mcookie xdpyinfo xdotool xprop; do
        have "$name" || die "Missing required private display tool: $name."
    done
    xvfb_exe=$(realpath -e -- "$(command -v Xvfb)") || die 'Could not resolve Xvfb executable.'
fi
if [[ $mode == drive ]]; then
    [[ -n $archive_spec && -n $smoke_evidence ]] || die 'Drive requires --archive and --smoke-evidence.'
    [[ ! -L ${smoke_evidence%/} ]] || die 'Expected a non-symlink smoke evidence directory.'
    smoke_evidence=$(realpath -e -- "$smoke_evidence") || die 'Smoke evidence directory does not exist.'
    [[ ${smoke_evidence%/*} == "$builds" && ${smoke_evidence##*/} == dev-evidence.???????? ]] &&
        owned_dir "$builds" && owned_dir "$smoke_evidence" &&
        owned_file "$smoke_evidence/smoke-passed.sha256" ||
        die 'Expected a trusted private Xvfb smoke pass receipt.'
fi
[[ -z $archive_spec ]] || archive_arg "$archive_spec"
for name in dpkg-deb jq sha256sum setsid; do have "$name" || die "Missing required host tool: $name."; done

new_evidence
scratch=$(mktemp -d /tmp/cwq.XXXXXXXX)
prefix="$scratch/p"
declare -a groups=()
teardown() {
    local rc=$? pid i
    trap - EXIT
    set +e
    for pid in "${groups[@]}"; do kill -TERM -- "-$pid" 2>/dev/null; done
    for ((i=0; i<50; i++)); do
        local alive=no
        for pid in "${groups[@]}"; do kill -0 -- "-$pid" 2>/dev/null && alive=yes; done
        [[ $alive == no ]] && break
        sleep .1
    done
    for pid in "${groups[@]}"; do
        if kill -0 -- "-$pid" 2>/dev/null; then
            kill -KILL -- "-$pid" 2>/dev/null
            ledger "teardown: process group $pid needed SIGKILL"
        fi
    done
    for f in service desktop xvfb driver; do
        [[ -f $scratch/$f.err ]] && ledger "$f stderr-sha256=$(sha256sum <"$scratch/$f.err" | cut -d' ' -f1)"
    done
    rm -rf --one-file-system -- "$scratch"
    ledger "teardown scratch removed=$([[ -e $scratch ]] && echo no || echo yes)"
    if ((rc == 0 && ${#unobserved[@]} > 0)); then rc=3; fi
    if [[ $rc == 0 && $mode == smoke && $display_mode == xvfb ]]; then
        # Publish only after all smoke gates and cleanup, never for exit 3.
        if [[ ! -e $scratch ]] &&
            (umask 077; printf '%s\n' "$actual" >"$evidence/.smoke-passed.tmp") &&
            mv -T -- "$evidence/.smoke-passed.tmp" "$evidence/smoke-passed.sha256"; then :
        else rc=1
        fi
    fi
    ledger "qualification exit=$rc"
    if ((${#unobserved[@]})); then printf 'Unobserved steps (not a pass): %s\n' "${unobserved[*]}"; fi
    printf 'Archive retained: %s\nEvidence: %s\n' "${archive:-none}" "$evidence"
    exit "$rc"
}
trap teardown EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -m 0700 "$prefix"
ledger "mode=$mode scratch=$prefix"

if [[ -z $archive ]]; then
    record build "$evidence/build.out" timeout --kill-after=30 3450 scripts/build-dev-deb.sh ||
        die "Dev build failed (see $evidence/build.out)."
    id=$(sed -nE 's/^Dev build directory: (build-[a-z0-9_]{8})$/\1/p' "$evidence/build.out")
    build_id "$id" || die 'No unique Dev build identity in wrapper output.'
    files=("$local_dev/$id"/"$package"_*.deb)
    [[ ${#files[@]} == 1 && -f ${files[0]} ]] || die 'Expected exactly one Dev archive.'
    archive=${files[0]}
fi
snapshot_dev_archive
if [[ $mode == drive ]]; then
    cmp -s "$smoke_evidence/smoke-passed.sha256" <(printf '%s\n' "$actual") ||
        die 'Drive archive differs from the private Xvfb smoke-passed archive.'
    ledger "smoke pass evidence=$smoke_evidence archive=$actual"
fi

{
    local_host_facts
    github_login_fact
} >"$evidence/host-facts.txt" 2>&1
record bundled-engines "$evidence/bundled.out" timeout --kill-after=10 600 \
    scripts/cortex-xtask.sh check-bundled-engines --archive "$candidate_deb" ||
    die 'Exact archive bundled-engine gate failed.'
record extract "$evidence/extract.out" dpkg-deb -x "$candidate_deb" "$prefix" || die 'Extraction failed.'
chmod 0700 "$prefix"
mkdir -m 0700 "$prefix"/{config,state,run,cache,data,projects}
socket="$prefix/run/$package/service.sock"
runtime="$prefix/usr/lib/$package/runtime"

if [[ $mode == smoke ]]; then
    # Private smoke settings only; the adapter must still report the real catalog.
    smoke_model=gpt-6-luna
    smoke_effort=high
    owner="$prefix/config/$package/cortex"
    (
        umask 077
        mkdir -p -- "$owner"
        jq -n --arg model "$smoke_model" '{
            model_policy_version:2, providers:["codex-acp"],
            enabled:{"codex-acp":[$model]},
            models:{"codex-acp":{auto:[$model], tiers:{($model):["light"]}}}
        }' >"$owner/routing.json"
        jq -n --arg model "$smoke_model" --arg effort "$smoke_effort" '{
            defaults:{coordinator:{engine:"codex-acp", model:$model, parameters:{thought_level:$effort}}}
        }' >"$owner/defaults.json"
    )
fi

# A fresh Git fixture in the private qualification roots; no owner configuration or
# hooks are read creating it. Only the candidate service process runs the verifier.
fixture="$prefix/projects/.qualification-verifier"
mkdir -m 0700 "$fixture"
record verifier-fixture "$evidence/verifier-fixture.out" timeout --kill-after=2 10 \
    env -i HOME=/nonexistent PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    /usr/bin/git -C "$fixture" init --quiet --template= || die 'Could not initialize the verifier fixture.'
printf %s confirmed >"$fixture/file"

# Private Dev roots; real HOME, session bus (keyring) and native/gh logins.
candidate=(-u DISPLAY -u WAYLAND_DISPLAY -u XAUTHORITY -u CORTEX_ROUTING -u CORTEX_WORKSPACE_QUALIFICATION_VERIFIER -u CORTEX_WORKSPACE_QUALIFICATION_OPENER_DIR "CORTEX_WORKSPACE_DEV_EXTRACTED_PREFIX=$prefix" "XDG_CONFIG_HOME=$prefix/config"
    "XDG_STATE_HOME=$prefix/state" "XDG_RUNTIME_DIR=$prefix/run" "XDG_CACHE_HOME=$prefix/cache"
    "XDG_DATA_HOME=$prefix/data" "HOME=$HOME"
    "GH_CONFIG_DIR=${GH_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gh}"
    "DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR:-/run/user/$uid}/bus}")
if [[ -n ${CODEX_HOME:-} ]]; then candidate+=("CODEX_HOME=$CODEX_HOME"); fi
if [[ -n ${CLAUDE_CONFIG_DIR:-} ]]; then candidate+=("CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR"); fi
# Start one background role in its own process group; teardown stops the group.
start() {
    local role=$1
    shift
    local -a qualification=()
    case $role in
        service) qualification=(CORTEX_WORKSPACE_QUALIFICATION_VERIFIER=1) ;;
        desktop) qualification=("CORTEX_WORKSPACE_QUALIFICATION_OPENER_DIR=$evidence") ;;
    esac
    { printf '+ start=%q' "$role"; printf ' %q' "${qualification[@]}" "$@"; printf '\n'; } >>"$evidence/ledger"
    setsid env "${candidate[@]}" "${view[@]}" "${qualification[@]}" "$@" >"$scratch/$role.out" 2>"$scratch/$role.err" </dev/null &
    groups+=("$!")
    ledger "started role=$role pgid=$!"
}
view=()
start service "$prefix/usr/bin/cortex-workspace-service-dev" "$prefix/projects" "$runtime"
for ((i=0; i<450; i++)); do
    [[ -S $socket ]] && break
    kill -0 "${groups[0]}" 2>/dev/null || break
    sleep .1
done
verifier_receipt=$(candidate_verifier_receipt "$scratch/service.out") ||
    die 'No complete nested-verifier receipt from this candidate service.'
printf '%s\n' "$verifier_receipt" >"$evidence/nested-verifier.out"
ledger "$verifier_receipt (same service process, archive=$actual)"
if [[ ! -S $socket || $(stat -c '%u:%a' "$socket") != "$uid:600" ]]; then
    grep -Eo '[a-z][a-z0-9_]{2,63}' "$scratch/service.err" 2>/dev/null | grep -Fx dev_extraction_invalid >>"$evidence/ledger" || :
    die 'Private Dev socket did not appear with mode 0600.'
fi
ledger "socket ready path=$socket"

# Use only the display allocated by the owned Xvfb. X11 display spellings can
# differ (e.g. :0, :0.0 or unix:0); conservatively reject a matching number.
verify_private_display() {
    [[ $display == ":$display_no" && $display != "$inherited_display" ]] || return 1
    if [[ $inherited_display =~ :([0-9]+)([.][0-9]+)?$ ]]; then
        # Canonicalize leading zeroes without arithmetic on untrusted text.
        local inherited_no=${BASH_REMATCH[1]}
        while [[ $inherited_no == 0?* ]]; do inherited_no=${inherited_no#0}; done
        [[ $display_no != "$inherited_no" ]] || return 1
    fi
    local socket_path="/tmp/.X11-unix/X$display_no" inode fd
    kill -0 "$xvfb_pid" 2>/dev/null &&
        [[ $(readlink -e -- "/proc/$xvfb_pid/exe") == "$xvfb_exe" && -S $socket_path ]] || return 1
    # -displayfd does not necessarily create a lock file. Bind the pathname
    # to the actual listening socket held by our Xvfb process.
    inode=$(awk -v path="$socket_path" '$8 == path && $4 == "00010000" { print $7 }' "/proc/$xvfb_pid/net/unix") ||
        return 1
    [[ $inode =~ ^[1-9][0-9]*$ ]] || return 1
    for fd in "/proc/$xvfb_pid/fd/"*; do
        [[ $(readlink -- "$fd") == "socket:[$inode]" ]] && return 0
    done
    return 1
}
display= xauthority=
if [[ $display_mode == xvfb ]]; then
    cookie=$(mcookie)
    [[ $cookie =~ ^[0-9a-fA-F]{32}$ ]] || die 'Could not generate a private Xauthority cookie.'
    install -m 0600 /dev/null "$prefix/seed.auth"
    install -m 0600 /dev/null "$prefix/client.auth"
    install -m 0600 /dev/null "$prefix/empty.auth"
    # Xvfb reads the cookie from this seed before it reports its selected number.
    printf 'add :0 MIT-MAGIC-COOKIE-1 %s\n' "$cookie" | xauth -q -f "$prefix/seed.auth"
    ledger 'start=xvfb (-displayfd, -auth, -nolisten tcp)'
    setsid bash -c 'exec "$2" -displayfd 3 -auth "$3" -nolisten tcp -screen 0 1440x1000x24 3>"$1"' \
        bash "$prefix/display" "$xvfb_exe" "$prefix/seed.auth" >"$scratch/xvfb.out" 2>"$scratch/xvfb.err" </dev/null &
    xvfb_pid=$!
    groups+=("$xvfb_pid")
    for ((i=0; i<100; i++)); do
        [[ -s $prefix/display ]] && break
        kill -0 "$xvfb_pid" 2>/dev/null || break
        sleep .1
    done
    display_no=
    read -r display_no <"$prefix/display" || die 'Private Xvfb did not report a display.'
    [[ $display_no =~ ^(0|[1-9][0-9]*)$ ]] || die 'Private Xvfb reported an invalid display number.'
    display=:$display_no
    verify_private_display || die 'Could not verify private Xvfb display ownership.'
    xauthority=$prefix/client.auth
    printf 'add %s MIT-MAGIC-COOKIE-1 %s\n' "$display" "$cookie" | xauth -q -f "$xauthority"
    unset cookie
    # The unauthorized check must reject access, not time out or fail to run.
    no_auth_rc=$(status_of 5 env "DISPLAY=$display" "XAUTHORITY=$prefix/empty.auth" xdpyinfo)
    [[ $no_auth_rc == 1 ]] || die 'Private Xvfb did not reject the client without its cookie.'
    [[ $(status_of 5 env "DISPLAY=$display" "XAUTHORITY=$xauthority" xdpyinfo) == 0 ]] ||
        die 'Private Xvfb did not accept its authorized client.'
    ledger "display=private-xvfb $display pid=$xvfb_pid (Xauthority verified)"
elif [[ -n $inherited_display ]]; then
    display=$inherited_display
    xauthority=${XAUTHORITY:-}
    ledger "display=owner-real $display (smoke only)"
fi

verify_desktop_window() {
    local property
    [[ $window =~ ^[1-9][0-9]*$ && $(readlink -e -- "/proc/$desktop_pid/exe") == "$prefix/usr/bin/$package" ]] ||
        return 1
    if [[ $display_mode == xvfb ]]; then verify_private_display || return 1; fi
    property=$(timeout 5 env "${view[@]}" xprop -id "$window" _NET_WM_PID) || return 1
    [[ $property == "_NET_WM_PID(CARDINAL) = $desktop_pid" ]]
}
window=
if [[ -n $display ]]; then
    view=("DISPLAY=$display" "GDK_BACKEND=x11" "WAYLAND_DISPLAY=")
    if [[ -n $xauthority ]]; then view+=("XAUTHORITY=$xauthority"); fi
    install -m 0600 /dev/null "$evidence/opener.sha256"
    start desktop "$prefix/usr/bin/$package"
    desktop_pid=${groups[-1]}
    # Only the launched desktop writes this file; independently hash the exact fixed
    # argument and compare bytes (one digest, one LF; no partial or extra records).
    for ((i=0; i<100; i++)); do
        [[ -s $evidence/opener.sha256 ]] && break
        kill -0 "$desktop_pid" 2>/dev/null || break
        sleep .1
    done
    probe_digest=$(printf '%s' 'https://qualification.invalid/cortex/view-pr?exact=1&arg=2' | sha256sum)
    cmp -s "$evidence/opener.sha256" <(printf '%s\n' "${probe_digest%% *}") ||
        die 'The candidate desktop did not record exactly one native View PR probe; no input was sent.'
    mv -- "$evidence/opener.sha256" "$evidence/opener-probe.sha256"
    install -m 0600 /dev/null "$evidence/opener.sha256"
    ledger "native View PR probe $(sha256sum -- "$evidence/opener-probe.sha256")"
    if have xdotool && have xprop; then
        for ((i=0; i<150; i++)); do
            if [[ $display_mode == xvfb ]]; then
                verify_private_display || die 'Private Xvfb was lost before window discovery.'
            fi
            window=$(timeout 5 env "${view[@]}" xdotool search --onlyvisible --pid "$desktop_pid" \
                --name 'Cortex Workspace' 2>/dev/null | sed -n '1p') || window=
            if [[ -n $window ]] && verify_desktop_window; then break; fi
            window=
            kill -0 "$desktop_pid" 2>/dev/null || break
            sleep .2
        done
        [[ -n $window ]] || die 'No visible window was verified for the extracted desktop PID.'
        ledger "window=$window display=$display desktop-pid=$desktop_pid"
    else
        gap 'desktop window identity (xdotool or xprop missing)'
    fi
else
    gap 'desktop window (no display available; service-only run)'
fi

if [[ $mode == smoke ]]; then
    readiness_rc=0
    record readiness "$evidence/readiness.out" timeout --kill-after=5 120 env "${candidate[@]}" \
        "$prefix/usr/lib/$package/engines/node/bin/node" "$root/scripts/qualify-dev-readiness.mjs" \
        "$socket" "$smoke_model" "$smoke_effort" || readiness_rc=$?
    if ((readiness_rc != 0)); then
        reason=$(grep -Eo 'settings readiness refused: [a-z0-9_]{1,64}' "$evidence/readiness.out" | head -n 1) || reason=unknown
        case $reason in
            *native_login_required*|*engine_unavailable*) gap "route readback (${reason#settings readiness refused: }; Codex login is a host fact)" ;;
            *) die "Machine settings or route/default readback failed: ${reason#settings readiness refused: }." ;;
        esac
    fi
fi
if [[ -n $window ]]; then
    # A mapped window can precede the first settled WebKit frame.
    sleep 5
    verify_desktop_window || die 'Display or desktop window identity lost before XTEST input.'
    if record window-input "$evidence/input.out" timeout 10 env "${view[@]}" \
        xdotool mousemove --window "$window" 20 20 click 1; then :; else gap 'XTEST input'; fi
    if have ffmpeg; then
        verify_desktop_window || die 'Display or desktop window identity lost before capture.'
        capture_screenshot || die 'Screenshot capture failed or did not produce a complete decodable PNG.'
    else
        gap 'screenshot (ffmpeg missing)'
    fi
fi
if [[ $mode == drive ]]; then
    [[ -n $window && ${#unobserved[@]} == 0 ]] || die 'Drive prerequisites were not all observed; driver was not started.'
    verify_desktop_window || die 'Display or desktop window identity lost before driver dispatch.'
    driver_env=("CWQ_PROJECTS=$prefix/projects" "CWQ_SOCKET=$socket" "CWQ_WINDOW=$window" "CWQ_EVIDENCE=$evidence")
    record driver "$evidence/driver.out" timeout --kill-after=10 2400 \
        env "${candidate[@]}" "${view[@]}" "${driver_env[@]}" "$@" || die "Foreground driver failed (see $evidence/driver.out)."
fi
kill -0 -- "-${groups[0]}" 2>/dev/null || die 'The service exited while in use.'
printf 'Extracted Dev run complete (%s).\n' "$mode"
