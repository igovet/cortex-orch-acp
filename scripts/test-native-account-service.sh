#!/usr/bin/env bash
# Manual service-source account tests, not an installed-service or paid check.
# Build the workspace-service lib test binary with the pinned Cargo:
#   umask 022; timeout --kill-after=5 240 ~/.cargo/bin/cargo test --locked --offline -p workspace-service --lib --no-run
# Pass the reported test executable, the exact public engine binary and its
# independently verified digest (never pass an account directory or config).
set -Eeuo pipefail
umask 022
usage() {
    echo "usage: bash $0 MODE TEST_EXE ENGINE_EXE ENGINE_SHA256 KERNEL_DEB KERNEL_SHA256 KERNEL_RELEASE" >&2
    echo "modes: opencode-{engine,bootstrap,catalogue,denials}, opencode-2.2.0-{engine,bootstrap,catalogue,denials}, codex-{usage,refresh}, claude-usage" >&2
    exit 2
}
[[ $# == 7 && $4 =~ ^[a-f0-9]{64}$ && $6 =~ ^[a-f0-9]{64}$ && $7 =~ ^[a-zA-Z0-9.+_-]+$ ]] || usage
mode=$1
route=$mode
opencode_version=2.2.6
# Explicit older control, never a fallback for the requested pin.
case $mode in
    opencode-2.2.0-*) route=opencode-${mode#opencode-2.2.0-}; opencode_version=2.2.0 ;;
esac
case $route in
    opencode-engine) engine=opencode; test_name=guest_opencode_engine; marker=CORTEX_OPENCODE_ENGINE_BOOTSTRAP_WAL_REOPEN_PASS ;;
    opencode-bootstrap) engine=opencode; test_name=guest_opencode_bootstrap; marker=CORTEX_OPENCODE_BOOTSTRAP_ONLY_PASS ;;
    opencode-catalogue) engine=opencode; test_name=guest_opencode_catalogue; marker=CORTEX_OPENCODE_CATALOGUE_PASS ;;
    opencode-denials) engine=opencode; test_name=guest_descendants; marker=CORTEX_DESCENDANT_TESTS_DONE ;;
    codex-refresh) engine=codex; test_name=codex_refresh::guest_codex_refresh; marker=CORTEX_CODEX_REFRESH_SHARED_LOCK_PASS ;;
    codex-usage) engine=codex; test_name=guest_usage; marker=CORTEX_CODEX_USAGE_OFFLINE_ERROR ;;
    claude-usage) engine=claude; test_name=guest_usage; marker='CORTEX_USAGE_RESULT claude' ;;
    *) usage ;;
esac
test_exe=$(realpath -- "$2")
binary=$(realpath -- "$3")
deb=$(realpath -- "$5")
[[ -f $test_exe && -x $test_exe && -f $binary && -x $binary && -f $deb ]] || usage
verify() {
    local digest
    digest=$(sha256sum <"$1")
    [[ ${digest%% *} == "$2" ]] || { echo "fixture input digest mismatch" >&2; exit 2; }
}
verify "$binary" "$4"
verify "$deb" "$6"
parser=$(command -v apparmor_parser || printf /usr/sbin/apparmor_parser)
[[ -x $parser ]] || { echo "missing AppArmor parser" >&2; exit 2; }
for tool in busybox aa-exec bwrap setpriv cpio qemu-system-x86_64 timeout strip; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
kernel=$scratch/vmlinuz
dpkg-deb --fsys-tarfile "$deb" | tar -xOf - -- "./boot/vmlinuz-$7" >"$kernel"
[[ -s $kernel ]] || { echo "empty guest kernel" >&2; exit 2; }
fs=$scratch/root
mkdir -p "$fs"/{usr/bin,dev,proc,sys,etc/apparmor,tmp,var/tmp,run,cortex-fixture}
ln -s usr/bin "$fs/bin"
: >"$fs/etc/apparmor/parser.conf"
printf 'root:x:0:0:root:/root:/bin/sh\nfixture:x:1000:1000:fixture:/cortex-fixture/home:/bin/sh\n' >"$fs/etc/passwd"
printf 'root:x:0:\nfixture:x:1000:\n' >"$fs/etc/group"
install -m 0755 -- "$binary" "$fs/usr/bin/$engine"
verify "$fs/usr/bin/$engine" "$4"
strip --strip-debug -o "$fs/usr/bin/account-service-test" -- "$test_exe"
install -m 0755 -- /usr/bin/setpriv "$fs/usr/bin/setpriv"
for tool in busybox aa-exec bwrap; do
    install -m 0755 -- "$(command -v "$tool")" "$fs/usr/bin/$tool"
done
install -m 0755 -- "$parser" "$fs/usr/bin/apparmor_parser"
# Only trusted public executables and their loader/lib dependencies enter the guest.
for exe in "$fs"/usr/bin/*; do
    if deps=$(LC_ALL=C ldd "$exe" 2>&1); then
        if grep -q 'not found' <<<"$deps"; then printf '%s\n' "$deps" >&2; exit 2; fi
        while IFS= read -r lib; do
            mkdir -p -- "$fs$(dirname -- "$lib")"
            cp -L -- "$lib" "$fs$lib"
        done < <(awk '$3 ~ /^\// {print $3} $1 ~ /^\// {print $1}' <<<"$deps")
    elif [[ $deps != *"not a dynamic executable"* && $deps != *"statically linked"* ]]; then
        printf '%s\n' "$deps" >&2
        exit 2
    fi
done
for tool in sh cat stat; do ln -s busybox "$fs/usr/bin/$tool"; done
for role in engine catalogue usage; do
    profile=cortex-fixture-$engine-$role
    {
        printf 'profile %s {\n' "$profile"
        cat <<'POLICY'
  /usr/bin/{opencode,codex,claude} rix,
  /usr/bin/{busybox,sh,cat,stat} rix,
  /{,usr/}lib{,64}/** mr,
  /etc/{passwd,group,localtime,ld.so.cache} r,
  /dev/{null,random,urandom,tty} rw,
  /proc/ r,
  /proc/{cpuinfo,meminfo,stat,uptime,version,self/**,thread-self/**,[0-9]*/**} r,
  /sys/devices/system/cpu/** r,
  /proc/sys/vm/{overcommit_memory,mmap_min_addr} r,
  /sys/kernel/mm/transparent_hugepage/enabled r,
  /tmp/ rw,
  /tmp/** rwk,
  /var/tmp/ rw,
  /var/tmp/** rwk,
  /cortex-fixture/home/ r,
  /cortex-fixture/home/checkout/ r,
  /cortex-fixture/home/checkout/** r,
  signal (receive) peer=unconfined,
  signal (send, receive) peer=@{profile_name},
  unix (create, read, write, getattr, getopt, setopt),
POLICY
        case $engine in
            opencode)
                cat <<'POLICY'
  /cortex-fixture/home/.local/share/opencode/ r,
  owner /cortex-fixture/home/.local/share/opencode/opencode.db{,-journal,-wal,-shm} rwk,
  # OpenCode ACP starts a local standalone server. The guest has no NIC.
  network inet stream,
  network inet6 stream,
  network inet dgram,
  network inet6 dgram,
  / r,
  /cortex-fixture/ r,
  owner /tmp/{.bun-1000-*,**/.bun-1000-*}.{so,node} mr,
  owner /cortex-fixture/home/.cache/opencode{/,/**} rwk,
  owner /cortex-fixture/home/.local/state/opencode{/,/**} rwk,
  /cortex-fixture/home/.config/opencode/{,opencode.json} r,
POLICY
                ;;
            codex)
                cat <<'POLICY'
  /usr/bin/bwrap rix,
  /proc/filesystems r,
  /proc/sys/kernel/{overflowuid,overflowgid} r,
  network inet stream,
  owner /cortex-fixture/home/.codex/config.toml r,
  owner /cortex-fixture/home/.codex/auth.json rw,
  owner /cortex-fixture/home/.codex/auth.lock rwk,
POLICY
                ;;
            claude)
                cat <<'POLICY'
  owner /cortex-fixture/home/.claude/.credentials.json r,
POLICY
                ;;
        esac
        printf '}\n'
    } >"$fs/etc/apparmor/$profile"
done
printf '%s\n' "$engine" >"$fs/etc/fixture-engine"
printf '%s\n' "$test_name" >"$fs/etc/fixture-test"
printf '%s\n' "$opencode_version" >"$fs/etc/fixture-opencode-version"
cat >"$fs/init" <<'GUEST'
#!/bin/sh
set -eu
export PATH=/usr/bin:/bin
export LC_ALL=C
export HOME=/cortex-fixture/home
export USER=fixture LOGNAME=fixture
umask 077
trap 'busybox poweroff -f' EXIT
busybox mount -t proc proc /proc
busybox mount -t sysfs sysfs /sys
busybox mount -t securityfs securityfs /sys/kernel/security
busybox mount -t devtmpfs devtmpfs /dev
busybox ip link set lo up
busybox chmod 1777 /tmp /var/tmp
busybox mount -t tmpfs -o mode=755 tmpfs /cortex-fixture
# This sysctl is changed only in the fresh guest, for the engines' own sandboxes
# (Codex runs bubblewrap itself); Cortex's own launches use Landlock.
if [ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]; then
    echo 0 > /proc/sys/kernel/apparmor_restrict_unprivileged_userns
fi
printf 'offline cortex native account fixture\n' >/cortex-fixture/guest
busybox chmod 0444 /cortex-fixture/guest
busybox mkdir -p "$HOME/checkout" "$HOME/.config/opencode" "$HOME/.config/cortex" "$HOME/.cache/opencode" \
    "$HOME/.local/share/opencode" "$HOME/.local/state/opencode" "$HOME/.codex" "$HOME/.claude"
printf 'usable project input\n' >"$HOME/checkout/input"
printf 'synthetic neighbor\n' >"$HOME/.local/share/opencode/neighbor"
# The task route needs a selectable model even when no prompt is sent. Supply
# synthetic metadata and a loopback-only endpoint; never contact a provider.
cat >"$HOME/.config/opencode/opencode.json" <<'OPENCODE'
{
  "autoupdate": false,
  "share": "disabled",
  "plugin": ["-opencode.agent", "-opencode.plan"],
  "model": "fixture/chat",
  "small_model": "fixture/chat",
  "enabled_providers": ["fixture"],
  "provider": {
    "fixture": {
      "name": "Offline Fixture",
      "npm": "@ai-sdk/openai-compatible",
      "options": { "baseURL": "http://127.0.0.1:1/v1" },
      "models": { "chat": { "name": "Fixture Chat", "limit": { "context": 8192, "output": 1024 } } }
    }
  }
}
OPENCODE
cat >"$HOME/.codex/config.toml" <<'CODEX'
cli_auth_credentials_store = "file"
sqlite_home = "/tmp/codex-fixture-state"
log_dir = "/tmp/codex-fixture-state"
CODEX
busybox mkdir /tmp/codex-fixture-state
printf '00000000-0000-4000-8000-000000000001\n' >/tmp/codex-fixture-state/installation_id
busybox chown -R 1000:1000 /tmp/codex-fixture-state
busybox ln -s /tmp/codex-fixture-state "$HOME/.codex/tmp"
busybox ln -s /tmp/codex-fixture-state/installation_id "$HOME/.codex/installation_id"
printf '{}\n' >"$HOME/.claude/.credentials.json"
# Keep OpenCode runtime output in disposable scratch, not in its account DB view.
# These synthetic redirects do not imply that owner paths can be relocated.
for dir in log repos shell; do
    busybox mkdir "/tmp/opencode-fixture-$dir"
    busybox chown 1000:1000 "/tmp/opencode-fixture-$dir"
    busybox ln -s "/tmp/opencode-fixture-$dir" "$HOME/.local/share/opencode/$dir"
done
busybox chown -R 1000:1000 "$HOME"
export CORTEX_FIXTURE_ENGINE=$(cat /etc/fixture-engine)
test_name=$(cat /etc/fixture-test)
for p in /etc/apparmor/cortex-fixture-*; do
    apparmor_parser -K -a "$p"
    busybox grep -Fx "${p##*/} (enforce)" /sys/kernel/security/apparmor/profiles
done
if [ "$CORTEX_FIXTURE_ENGINE" = opencode ]; then
    version=$(/usr/bin/setpriv --reuid=1000 --regid=1000 --clear-groups opencode --version)
    printf 'OpenCode executable version: %s\n' "$version"
    export CORTEX_FIXTURE_OPENCODE_VERSION=$(cat /etc/fixture-opencode-version)
    test "${version#opencode v}" = "$CORTEX_FIXTURE_OPENCODE_VERSION"
elif [ "$CORTEX_FIXTURE_ENGINE" = codex ]; then
    version=$(/usr/bin/setpriv --reuid=1000 --regid=1000 --clear-groups codex --version)
    printf 'Codex executable version: %s\n' "$version"
    test "$version" = 'codex-cli 0.166.0'
else
    version=$(/usr/bin/setpriv --reuid=1000 --regid=1000 --clear-groups claude --version)
    printf 'Claude SDK executable version: %s\n' "$version"
    test "$version" = '2.1.323 (Claude Code)'
fi
run_test() {
    if ! /usr/bin/setpriv --reuid=1000 --regid=1000 --clear-groups \
        account-service-test "native_account_fixture::$test_name" --exact --ignored --nocapture; then
        busybox dmesg | busybox grep 'apparmor="DENIED"' | busybox tail -15
        # Only guest-generated diagnostics, e.g. a missing offline model cache.
        if [ "$CORTEX_FIXTURE_ENGINE" = opencode ]; then
            (busybox grep -hEi 'level=(error|fatal)|ERROR|FATAL' /tmp/opencode-fixture-log/* 2>/dev/null || :) |
                busybox cut -c 1-240 | busybox tail -10
        fi
        exit 1
    fi
}
if [ "$test_name" = guest_descendants ]; then
    for CORTEX_FIXTURE_ROLE in engine catalogue; do
        export CORTEX_FIXTURE_ROLE
        run_test
    done
    echo CORTEX_DESCENDANT_TESTS_DONE
else
    run_test
fi
echo CORTEX_ACCOUNT_SERVICE_GUEST_COMPLETE
GUEST
chmod 0755 "$fs/init"
(cd "$fs"; find . -print0 | LC_ALL=C sort -z | cpio --quiet --null -o -H newc | gzip -1n) >"$scratch/initrd.gz"
printf 'fixture mode: %s, engine sha256: %s, kernel package sha256: %s\n' "$mode" "$4" "$6"
sha256sum -- "$kernel" "$scratch/initrd.gz" "$test_exe" "$fs/usr/bin/account-service-test"
# The exact unstripped SDK binary needs more initramfs allocation headroom.
guest_memory=2048
if [[ $engine == claude ]]; then guest_memory=4096; fi
timeout --kill-after=5 120 env -i PATH=/usr/bin:/bin HOME="$scratch" qemu-system-x86_64 \
    -no-user-config -nodefaults -machine accel=tcg -cpu max -m "$guest_memory" -smp 2 -display none \
    -serial stdio -monitor none -nic none -no-reboot -kernel "$kernel" \
    -initrd "$scratch/initrd.gz" -append 'console=ttyS0 quiet rootfstype=ramfs lsm=apparmor apparmor=1 panic=-1' |
    tr -d '\r' | tee "$scratch/serial"
grep -Fxq 'CORTEX_ACCOUNT_SERVICE_GUEST_COMPLETE' "$scratch/serial"
grep -Fxq "$marker" "$scratch/serial"
