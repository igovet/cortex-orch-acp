#!/usr/bin/env python3
"""No-paid real systemd lifecycle in one privileged, disposable container.

No host buses, cgroups, home, credentials or daemon sockets are mounted.
The privilege is for the container's PID 1 only; this is not hostile-code isolation.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

BASE = "ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c"
PACKAGE = "cortex-workspace-dev"
UNIT = PACKAGE + ".service"


def outcome(step, **details):
    print(json.dumps(dict(step=step, status="passed", **details)), flush=True)


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, timeout=120, **kwargs)


def wait_for(predicate, description):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.2)
    raise RuntimeError(description)


def rpc(method, params):
    """One service request as the disposable desktop user; prints the whole reply."""
    import socket
    import struct
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.settimeout(60)
        peer.connect(str(Path(os.environ["XDG_RUNTIME_DIR"]) / PACKAGE / "service.sock"))
        data = json.dumps(dict(jsonrpc="2.0", id="systemd", method=method, params=params)).encode()
        peer.sendall(struct.pack(">I", len(data)) + data)
        def exact(n):
            result = b""
            while len(result) < n:
                part = peer.recv(n - len(result))
                assert part, "service_closed"
                result += part
            return result
        size = struct.unpack(">I", exact(4))[0]
        assert 0 < size <= 1048576
        print(exact(size).decode(), flush=True)


def inside():
    assert Path("/.dockerenv").exists() and Path("/proc/1/comm").read_text().strip() == "systemd"
    assert os.getuid() == 0
    archive = next(Path("/archives").glob(PACKAGE + "_*.deb"))
    run(["useradd", "--create-home", "--shell", "/bin/sh", "smoke"])
    uid = subprocess.check_output(["id", "-u", "smoke"], text=True).strip()
    home = Path("/home/smoke")
    runtime = Path("/run/user") / uid
    user = ["runuser", "-u", "smoke", "--", "env", "HOME=" + str(home),
            "XDG_RUNTIME_DIR=" + str(runtime), "DBUS_SESSION_BUS_ADDRESS=unix:path=" + str(runtime / "bus")]
    def ctl(*args):
        return subprocess.run([*user, "systemctl", "--user", *args], capture_output=True, text=True, timeout=10)
    def active():
        return ctl("is-active", "--quiet", UNIT).returncode == 0
    def pid():
        result = ctl("show", "--property=MainPID", "--value", UNIT)
        assert result.returncode == 0 and result.stdout.strip().isdigit()
        return int(result.stdout.strip())
    def install(path, *, upgrade=False, reinstall=False):
        result = subprocess.run(["apt", "install", "--yes", *(["--reinstall"] if reinstall else []), str(path)], capture_output=True, text=True, timeout=120)
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", flush=True)
        assert result.returncode == 0, "apt_install_failed"
        if upgrade:
            assert "downgrad" not in (result.stdout + result.stderr).lower(), "unexpected_downgrade"
            assert re.search(r"Upgrading:\s+1", result.stdout), "apt_did_not_report_upgrade"
            outcome("plain_apt_install_is_upgrade", downgrade_warning=False)
    def remove():
        run(["dpkg", "--remove", PACKAGE])
    # A readable copy for the disposable user; the mounted harness keeps its host mode.
    client = Path("/tmp/check_systemd_client.py")
    shutil.copyfile(__file__, client)
    client.chmod(0o644)
    def request(method, params):
        result = subprocess.run([*user, "python3", "-B", str(client), "--rpc", method, json.dumps(params)],
                                capture_output=True, text=True, timeout=90)
        assert result.returncode == 0, "rpc_failed: " + method + ": " + result.stderr[-2000:]
        reply = json.loads(result.stdout)
        assert reply.get("id") == "systemd" and "error" not in reply, method + " refused: " + json.dumps(reply.get("error"))
        return reply["result"]
    def project_comes_back(project_id, after):
        # Only reads. Nothing here asks the service to start the project again.
        deadline, status = time.monotonic() + 90, None
        while time.monotonic() < deadline:
            try:
                status = request("projects.status", dict(projectId=project_id))
            except (AssertionError, ValueError, subprocess.TimeoutExpired):
                status = None
            if status and status.get("ready") is True:
                outcome("activated_project_ready_" + after, state=status.get("state"))
                return
            time.sleep(.5)
        raise RuntimeError("activated_project_not_ready_" + after + ": " + json.dumps(status))
    run(["loginctl", "enable-linger", "smoke"])
    wait_for(lambda: (runtime / "bus").is_socket(), "user_bus_missing")
    wait_for(lambda: ctl("is-active", "--quiet", "default.target").returncode == 0, "user_default_target_missing")
    outcome("real_systemd_pid1_and_lingering_user")
    install(archive)
    wants = Path("/usr/lib/systemd/user/default.target.wants") / UNIT
    assert wants.is_symlink() and wants.resolve().is_file()
    outcome("clean_install_with_default_wants")
    run(["systemctl", "restart", "user@" + uid + ".service"])
    wait_for(active, "login_default_target_did_not_start_unit")
    assert pid() > 0
    socket_path = runtime / PACKAGE / "service.sock"
    wait_for(socket_path.is_socket, "login_service_socket_missing")
    outcome("login_time_default_target_start")

    assert ctl("stop", UNIT).returncode == 0
    display = subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    app = None
    try:
        wait_for(lambda: Path("/tmp/.X11-unix/X99").is_socket(), "display_not_ready")
        app = subprocess.Popen([*user, "env", "DISPLAY=:99", "WEBKIT_DISABLE_COMPOSITING_MODE=1",
                                "/usr/bin/" + PACKAGE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        wait_for(active, "desktop_did_not_start_unit")
        wait_for(socket_path.is_socket, "app_service_socket_missing")
        assert app.poll() is None and pid() > 0
        outcome("app_driven_start_in_running_session")
    finally:
        for child in (app, display):
            if child is not None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)

    before = pid()
    install(archive, reinstall=True)
    wait_for(lambda: active() and pid() != before, "upgrade_did_not_restart_running_unit")
    outcome("active_manager_upgrade_restart")
    sentinel = home / "preserved-data"
    sentinel.write_text("preserve")
    remove()
    wait_for(lambda: not active() and not socket_path.exists(), "removal_did_not_stop_unit")
    assert sentinel.read_text() == "preserve"
    outcome("active_manager_removal_stop_preserves_data")

    # Reproduce the owner's earlier two-binary archive shape, with no runtime,
    # maintainer scripts or default.target.wants link. This is not his original archive.
    stage = Path("/tmp/legacy")
    run(["dpkg-deb", "-R", str(archive), str(stage)])
    shutil.rmtree(stage / "usr/lib" / PACKAGE)
    (stage / "usr/lib/systemd/user/default.target.wants" / UNIT).unlink()
    for script in ("postinst", "prerm"):
        (stage / "DEBIAN" / script).unlink()
    control = stage / "DEBIAN/control"
    control.write_text(re.sub(r"(?m)^Version:.*$", "Version: 1.0.6+sha256.f3eee9da173725e1", control.read_text()))
    legacy = Path("/tmp/legacy.deb")
    run(["dpkg-deb", "--build", str(stage), str(legacy)])
    prior = Path("/prior/owner.deb")
    install(prior if prior.is_file() else legacy)
    prior_pid = None
    if prior.is_file():
        assert ctl("daemon-reload").returncode == 0
        assert ctl("start", UNIT).returncode == 0
        wait_for(active, "prior_archive_unit_inactive")
        wait_for(socket_path.is_socket, "prior_archive_socket_missing")
        prior_pid = pid()
        # The owner's condition: a project activated by the earlier build, then a plain upgrade.
        upgraded = home / "upgraded-project"
        upgraded.mkdir()
        run(["chown", "smoke:smoke", str(upgraded)])
        project_id = request("projects.register", dict(root=str(upgraded), displayName="Upgraded"))["projectId"]
        request("projects.activate", dict(projectId=project_id))
        outcome("prior_archive_project_activated")
    install(archive, upgrade=True)
    assert ctl("start", UNIT).returncode == 0
    wait_for(active, "legacy_upgrade_unit_inactive")
    if prior_pid is not None:
        wait_for(lambda: pid() != prior_pid, "exact_archive_upgrade_did_not_restart")
    wait_for(socket_path.is_socket, "legacy_upgrade_socket_missing")
    if prior_pid is not None:
        project_comes_back(project_id, "after_upgrade")
        # The unit's crash-loop guard allows five starts a minute; this harness alone made more.
        # A restart or login is a later, separate owner event, so the counter starts afresh.
        assert ctl("reset-failed", UNIT).returncode == 0
        before = pid()
        restarted = ctl("restart", UNIT)
        assert restarted.returncode == 0, "unit_restart_failed: " + restarted.stderr[-2000:]
        wait_for(lambda: active() and pid() != before, "service_restart_did_not_restart")
        project_comes_back(project_id, "after_service_restart")
        assert ctl("reset-failed", UNIT).returncode == 0
        before = pid()
        run(["systemctl", "restart", "user@" + uid + ".service"])
        wait_for(lambda: (runtime / "bus").is_socket() and active() and pid() != before, "login_did_not_start_unit")
        project_comes_back(project_id, "after_login")
    assert (Path("/usr/lib") / PACKAGE / "runtime/src/cortex_acp.py").is_file()
    assert sentinel.read_text() == "preserve"
    outcome("owner_archive_upgrade" if prior.is_file() else "owner_version_legacy_shape_upgrade",
            exact_original_archive=prior.is_file(), running_prior_unit_restarted=prior_pid is not None,
            prior_sha256=hashlib.sha256(prior.read_bytes()).hexdigest() if prior.is_file() else None)
    remove()
    wait_for(lambda: not active(), "final_removal_unit_active")
    outcome("systemd_lifecycle_complete", paid_calls=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--prior-archive", type=Path, help="optional exact earlier dev archive to upgrade")
    parser.add_argument("--inside", action="store_true")
    parser.add_argument("--rpc", nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.rpc:
        rpc(args.rpc[0], json.loads(args.rpc[1]))
        return
    if args.inside:
        inside()
        return
    assert args.artifacts
    artifacts = args.artifacts.resolve(strict=True)
    archive = next(artifacts.glob(PACKAGE + "_*.deb"))
    depends = subprocess.check_output(["dpkg-deb", "-f", str(archive), "Depends"], text=True)
    names = [re.match(r"[a-z0-9.+-]+", part.strip()).group() for part in depends.split(",")]
    prior = args.prior_archive.resolve(strict=True) if args.prior_archive else None
    if prior:
        assert subprocess.check_output(["dpkg-deb", "-f", str(prior), "Package"], text=True).strip() == PACKAGE
        prior_depends = subprocess.check_output(["dpkg-deb", "-f", str(prior), "Depends"], text=True)
        names += [re.match(r"[a-z0-9.+-]+", part.strip()).group() for part in prior_depends.split(",")]
    image = "cortex-systemd-check:" + hashlib.sha256(archive.read_bytes()).hexdigest()[:16]
    with tempfile.TemporaryDirectory(prefix="systemd-", dir=artifacts.parent) as directory:
        context = Path(directory)
        (context / "Dockerfile").write_text("FROM " + BASE + "\nENV container=docker\nRUN apt-get update && apt-get install -y --no-install-recommends systemd-sysv dbus-user-session xvfb "
                                             + " ".join(names) + " && rm -rf /var/lib/apt/lists/*\nSTOPSIGNAL SIGRTMIN+3\nCMD [\"/sbin/init\"]\n")
        run(["docker", "build", "-q", "-t", image, str(context)])
    identifier = subprocess.check_output([
        "docker", "run", "--detach", "--privileged", "--cgroupns=private", "--network=none",
        "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp",
        "--mount", "type=bind,source=" + str(artifacts) + ",target=/archives,readonly",
        "--mount", "type=bind,source=" + str(Path(__file__).resolve().parent) + ",target=/harness,readonly",
        *(["--mount", "type=bind,source=" + str(prior) + ",target=/prior/owner.deb,readonly"] if prior else []), image
    ], text=True).strip()
    try:
        wait_for(lambda: subprocess.run(["docker", "exec", identifier, "systemctl", "is-system-running"],
                                       capture_output=True, text=True, timeout=10).stdout.strip() in {"running", "degraded"}, "systemd_not_ready")
        subprocess.run(["docker", "exec", identifier, "python3", "-B", "/harness/check_systemd.py", "--inside"], check=True, timeout=900)
    finally:
        run(["docker", "rm", "--force", identifier])
    outcome("owned_systemd_container_removed")


if __name__ == "__main__":
    main()
