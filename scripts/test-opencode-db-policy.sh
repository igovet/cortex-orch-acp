#!/usr/bin/env bash
# Linux/x86_64 diagnostic, not an installed Cortex qualification. Use a trusted
# AppArmor-enabled guest bzImage and local SQLite development headers. No sudo,
# kernel policy writes on the host, networking, or host filesystems in the guest.
# Usage: bash scripts/test-opencode-db-policy.sh KERNEL SQLITE_INCLUDE_DIR
#        bash scripts/test-opencode-db-policy.sh --native-control SQLITE_INCLUDE_DIR
#        bash scripts/test-opencode-db-policy.sh --kernel-deb SQLITE_INCLUDE_DIR DEB SHA256 KERNEL_RELEASE
# Prefix a guest invocation with --codex to run the Codex file-backend fixture
# instead, passing "-" in place of SQLITE_INCLUDE_DIR.
# SHA256 must be obtained independently from trusted package metadata.
set -Eeuo pipefail
umask 022
usage() {
    echo "usage: $0 [--codex] {KERNEL|--native-control} SQLITE_INCLUDE_DIR" >&2
    echo "       $0 [--codex] --kernel-deb SQLITE_INCLUDE_DIR DEB SHA256 KERNEL_RELEASE" >&2
    exit 2
}
fixture=db
if [[ ${1-} == --codex ]]; then fixture=codex; shift; fi
[[ $# == 2 || $# == 5 ]] || usage
mode=$1
if [[ $mode == --kernel-deb ]]; then [[ $# == 5 ]] || usage
else [[ $# == 2 ]] || usage
fi
if [[ $fixture == codex ]]; then
    [[ $2 == - && $mode != --native-control ]] || usage
else
    headers=$(realpath -- "$2")
    [[ -r $headers/sqlite3.h ]] || { echo "SQLite header is not readable" >&2; exit 2; }
fi
src=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

build_peer() {
    cc -std=gnu11 -O2 -Wall -Wextra -Werror -I "$headers" \
        "$src/opencode-db-policy-fixture.c" -o "$1" -Wl,-l:libsqlite3.so.0
}

if [[ $mode == --native-control ]]; then
    build_peer "$scratch/db-peer"
    sha256sum -- "$src/test-opencode-db-policy.sh" "$src/opencode-db-policy-fixture.c"
    (cd -- "$scratch"; timeout --kill-after=5 30 ./db-peer native-control)
    exit
fi

if [[ $mode == --kernel-deb ]]; then
    deb=$(realpath -- "$3")
    [[ -f $deb && -r $deb && $4 =~ ^[a-f0-9]{64}$ && $5 =~ ^[a-zA-Z0-9.+_-]+$ ]] || usage
    digest=$(sha256sum <"$deb")
    [[ ${digest%% *} == "$4" ]] || { echo "guest kernel package digest mismatch" >&2; exit 2; }
    kernel=$scratch/vmlinuz
    dpkg-deb --fsys-tarfile "$deb" | tar -xOf - -- "./boot/vmlinuz-$5" >"$kernel"
    [[ -s $kernel ]] || { echo "empty guest kernel" >&2; exit 2; }
    printf 'guest kernel package sha256: %s\n' "$4"
else
    kernel=$(realpath -- "$mode")
fi
[[ -f $kernel && -r $kernel ]] || { echo "trusted guest kernel is not readable" >&2; exit 2; }
for tool in cc ldd cpio gzip qemu-system-x86_64 timeout sha256sum busybox aa-exec; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
parser=$(command -v apparmor_parser || printf '%s\n' /usr/sbin/apparmor_parser)
[[ -x $parser ]] || { echo "AppArmor parser is not available" >&2; exit 2; }

fs=$scratch/root
mkdir -p "$fs"/{bin,dev,proc,sys,etc/apparmor,tmp}
: >"$fs/etc/apparmor/parser.conf"
if [[ $fixture == codex ]]; then
    mkdir "$fs/codex-account"
    fixture_source=codex-auth-policy-fixture.c
    cc -std=gnu11 -O2 -Wall -Wextra -Werror "$src/$fixture_source" -o "$fs/bin/auth-peer"
    marker=CORTEX_CODEX_FILE_POLICY_GUEST_PASS
else
    mkdir "$fs/acct"
    fixture_source=opencode-db-policy-fixture.c
    build_peer "$fs/bin/db-peer"
    marker=CORTEX_DB_POLICY_GUEST_PASS
fi
cp -- "$(command -v busybox)" "$fs/bin/busybox"
cp -- "$(command -v aa-exec)" "$fs/bin/aa-exec"
cp -- "$parser" "$fs/bin/apparmor_parser"

# Copy only the dynamic loader and libraries of these known local tools/fixture.
# AppArmor policy grants code loading only for the files copied here.
for bin in "$fs"/bin/*; do
    if deps=$(LC_ALL=C ldd "$bin" 2>&1); then
        if grep -q 'not found' <<<"$deps"; then printf '%s\n' "$deps" >&2; exit 2; fi
        while IFS= read -r lib; do
            mkdir -p -- "$fs$(dirname -- "$lib")"
            cp -L -- "$lib" "$fs$lib"
            printf '%s\n' "$lib" >>"$scratch/libs"
        done < <(awk '$3 ~ /^\// {print $3} $1 ~ /^\// {print $1}' <<<"$deps")
    elif [[ $deps != *"not a dynamic executable"* && $deps != *"statically linked"* ]]; then
        printf '%s\n' "$deps" >&2
        exit 2
    fi
done
ln -s busybox "$fs/bin/sh"
loader_rules() {
    while IFS= read -r lib; do
        printf '  "%s" mr,\n' "$lib"
    done < <(LC_ALL=C sort -u "$scratch/libs")
}
if [[ $fixture == codex ]]; then
    {
        cat <<'POLICY'
profile cortex-codex-file {
  /bin/auth-peer rix,
  /proc/[0-9]*/attr/apparmor/current r,
  owner /codex-account/auth.json rw,
  owner /codex-account/auth.lock rwk,
POLICY
        loader_rules
        printf '}\n'
    } >"$fs/etc/codex-file"
else
for variant in visible hidden; do
    {
        printf 'profile cortex-db-%s {\n' "$variant"
        cat <<'POLICY'
  /bin/db-peer rix,
  /proc/[0-9]*/attr/apparmor/current r,
  owner /acct/opencode.db rwk,
  owner /acct/opencode.db-wal rwk,
  owner /acct/opencode.db-shm rwk,
POLICY
        if [[ $variant == visible ]]; then
            printf '  /acct/ r,\n'
            # Guest-only fault injection; the harness is the unconfined sender.
            printf '  signal (receive) set=(kill) peer=unconfined,\n'
        fi
        loader_rules
        printf '}\n'
    } >"$fs/etc/db-$variant"
done
fi
cat >"$fs/init" <<'GUEST'
#!/bin/sh
set -eu
export PATH=/bin
export LC_ALL=C
umask 077
trap '/bin/busybox poweroff -f' EXIT
/bin/busybox mount -t proc proc /proc
/bin/busybox mount -t sysfs sysfs /sys
/bin/busybox mount -t securityfs securityfs /sys/kernel/security
/bin/busybox cat /sys/module/apparmor/parameters/enabled
/bin/busybox uname -sr
if [ -f /etc/codex-file ]; then
    /bin/busybox mount -t tmpfs -o mode=700 tmpfs /codex-account
    /bin/busybox chown 1000:1000 /codex-account
    /bin/busybox stat -f -c 'account fs=%T' /codex-account
    /bin/apparmor_parser -K -a /etc/codex-file
    /bin/busybox grep '^cortex-codex-file ' /sys/kernel/security/apparmor/profiles
    if /bin/auth-peer; then
        echo 'CORTEX_CODEX_FILE_POLICY_GUEST_PASS'
        exit 0
    fi
    /bin/busybox dmesg | /bin/busybox grep 'profile="cortex-codex-file"' | /bin/busybox tail -20
    exit 1
fi
/bin/busybox mount -t tmpfs -o mode=700 tmpfs /acct
/bin/busybox chown 1000:1000 /acct
/bin/busybox stat -f -c 'database fs=%T' /acct
/bin/apparmor_parser -K -a /etc/db-hidden /etc/db-visible
/bin/busybox grep '^cortex-db-' /sys/kernel/security/apparmor/profiles
if /bin/db-peer; then
    echo 'CORTEX_DB_POLICY_GUEST_PASS'
else
    /bin/busybox dmesg | /bin/busybox grep 'profile="cortex-db-' | /bin/busybox tail -20
    exit 1
fi
GUEST
chmod 0755 "$fs/init"
(cd "$fs"; find . -print0 | LC_ALL=C sort -z | cpio --quiet --null -o -H newc | gzip -n) >"$scratch/initrd.gz"
sha256sum -- "$kernel" "$scratch/initrd.gz" "$src/test-opencode-db-policy.sh" "$src/$fixture_source"
rc=0
timeout --kill-after=5 90 qemu-system-x86_64 \
    -accel tcg -machine q35 -m 384 -smp 2 -no-reboot -nic none \
    -no-user-config -nodefaults -display none -monitor none -serial stdio \
    -kernel "$kernel" -initrd "$scratch/initrd.gz" \
    -append 'console=ttyS0 quiet printk.devkmsg=off apparmor=1 lsm=capability,apparmor' \
    >"$scratch/serial" 2>&1 || rc=$?
cat -- "$scratch/serial"
[[ $rc == 0 ]] || exit "$rc"
[[ $(tr -d '\r' <"$scratch/serial" | grep -Fxc "$marker") == 1 ]]
