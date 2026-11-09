#!/usr/bin/env python3
"""No-paid install/upgrade/remove qualification; only disposable Docker state is changed."""
import argparse
import configparser
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time

BASE = "ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c"


def outcome(step, status="passed", **details):
    print(json.dumps(dict(step=step, status=status, **details)), flush=True)


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, timeout=600, **kwargs)


def rpc(path, method, params):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.settimeout(30)
        peer.connect(str(path))
        data = json.dumps(dict(jsonrpc="2.0", id="smoke", method=method, params=params)).encode()
        peer.sendall(struct.pack(">I", len(data)) + data)
        def exact(n):
            result = b""
            while len(result) < n:
                part = peer.recv(n - len(result))
                if not part:
                    raise RuntimeError("service_closed")
                result += part
            return result
        size = struct.unpack(">I", exact(4))[0]
        assert 0 < size <= 1048576
        reply = json.loads(exact(size))
        assert reply["id"] == "smoke" and "error" not in reply, "rpc_failed"
        return reply["result"]


def wait_socket(path, process=None):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise RuntimeError("process_exited")
        try:
            return rpc(path, "service.status", {})
        except (OSError, RuntimeError):
            time.sleep(0.1)
    raise RuntimeError("socket_timeout")


def session(package):
    home = Path.home()
    runtime = Path("/usr/lib") / package / "runtime"
    sys.path.insert(0, str(runtime / "src"))
    # Exercise actual first-run discovery/registration with synthetic owned
    # adapters and native login markers, never a real account or model call.
    from unittest.mock import patch
    from cortex_runtime.connections import automatic, discovery, engine_update, registry
    config = home / ".config" / package if package.endswith("-dev") else home / ".config"
    state = home / ".local/state" / package
    for directory in (config, state, state / "state", state / "data", state / "cache"):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        directory.chmod(0o700)
    env = dict(os.environ, XDG_CONFIG_HOME=str(config), XDG_STATE_HOME=str(state / "state"),
               XDG_DATA_HOME=str(state / "data"), XDG_CACHE_HOME=str(state / "cache"),
               PATH="/usr/local/bin:/usr/bin:/bin")
    assert not registry.read_registry(env).connections
    for engine, subdir, marker in (("codex-acp", ".codex", "auth.json"),
                                    ("claude-agent-acp", ".claude", ".credentials.json")):
        native = home / subdir
        native.mkdir(mode=0o700)
        (native / marker).write_text("{}")
        (native / marker).chmod(0o600)
        assert discovery.discover_bundled(engine, environ=env) is not None
    # Only model refresh is doubled. Offline bundled discovery,
    # login-marker checks, defaults and registry transactions are production.
    def refreshed(record, **_):
        automatic.record_status(record.engine, "ready", env)
        return "ready"
    with patch.object(automatic, "refresh_connection", side_effect=refreshed):
        automatic.run(environ=env)
    records = registry.read_registry(env).connections
    assert len(records) == 2
    assert all(not r.credential_ref.owned and r.state.value == "configured" for r in records.values())
    assert all(r.launch_command[0] == str(runtime.parent / "engines/node/bin/node") for r in records.values())
    outcome("empty_namespace_native_discovery", model_refresh="stubbed; no live engine qualification")
    # Stale manual setup must be ignored by the actual installed unit.
    (home / ".config" / package).mkdir(parents=True, exist_ok=True)
    (home / ".config" / package / "service.env").write_text("CORTEX_WORKSPACE_CHECKOUT=/does-not-exist\n")
    unit = configparser.ConfigParser(interpolation=None)
    unit.read("/usr/lib/systemd/user/" + package + ".service")
    assert "EnvironmentFile" not in unit["Service"]
    socket_path = Path(os.environ["XDG_RUNTIME_DIR"]) / package / "service.sock"
    manager = subprocess.run(["/usr/bin/systemctl", "--user", "show-environment"],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10).returncode == 0
    process = app = display = None
    try:
        if manager:
            # Start from a stopped unit, then let the real desktop native bridge start it.
            run(["/usr/bin/systemctl", "--user", "stop", package + ".service"])
            display = subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(1)
            app = subprocess.Popen(["/usr/bin/" + package], env=dict(os.environ, DISPLAY=":99", WEBKIT_DISABLE_COMPOSITING_MODE="1"),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            wait_socket(socket_path, app)
            outcome("app_driven_unit_start")
        else:
            outcome("systemd_user_manager_and_app_start", "unverified", reason="container cannot host a systemd user manager")
            import shlex
            argv = shlex.split(unit["Service"]["ExecStart"].replace("%h", str(home)))
            process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            wait_socket(socket_path, process)
            outcome("unit_execstart_direct")
        project = home / "project"
        project.mkdir()
        registered = rpc(socket_path, "projects.register", dict(root=str(project), displayName="Clean install"))
        project_id = registered["projectId"]
        rpc(socket_path, "projects.activate", dict(projectId=project_id))
        assert rpc(socket_path, "projects.status", dict(projectId=project_id))["ready"] is True
        assert (project / (".cortex-dev" if package.endswith("-dev") else ".cortex") / "cortex.sqlite3").is_file()
        outcome("bundled_runtime_connected", paid_calls=0, fake_adapters="registered; no model prompt")
        probe = subprocess.run(["/usr/bin/bwrap", "--unshare-user", "--ro-bind", "/", "/", "--", "/usr/bin/true"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        outcome("unprivileged_sandbox", "passed" if probe.returncode == 0 else "unverified",
                exit_code=probe.returncode, reason=None if probe.returncode == 0 else "namespace denied by container/host policy; packaged engine execution not qualified")
        rpc(socket_path, "service.stop", {})
    finally:
        for child in (app, process, display):
            if child is not None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)


def container(package):
    archive = next(Path("/archives").glob(package + "_*.deb"))
    # Reproduce the former two-binary shape, without a runtime or wants link.
    stage = Path("/tmp/legacy")
    run(["dpkg-deb", "-R", str(archive), str(stage)], stdout=subprocess.DEVNULL)
    shutil.rmtree(stage / "usr/lib" / package)
    (stage / "usr/lib/systemd/user/default.target.wants" / (package + ".service")).unlink()
    for script in ("postinst", "prerm"):
        (stage / "DEBIAN" / script).unlink()
    control = stage / "DEBIAN/control"
    body = control.read_text()
    import re
    control.write_text(re.sub(r"(?m)^Version:.*$", "Version: 1.0.5", body))
    legacy = Path("/tmp/legacy.deb")
    run(["dpkg-deb", "--build", str(stage), str(legacy)], stdout=subprocess.DEVNULL)
    run(["apt-get", "update"], stdout=subprocess.DEVNULL)
    run(["apt-get", "install", "-y", "--no-install-recommends", str(archive), "xvfb"], stdout=subprocess.DEVNULL)
    outcome("clean_install_with_dependencies")
    # Publish a readable fixture inside the disposable image before dropping uid.
    harness = Path("/tmp/check_install_session.py")
    shutil.copyfile(__file__, harness)
    harness.chmod(0o644)
    assert not shutil.which("gcc") and not shutil.which("cargo"), "clean image must not contain build tools"
    run(["useradd", "--create-home", "--shell", "/bin/sh", "smoke"])
    home = Path("/home/smoke")
    runtime = Path("/run/user") / str(subprocess.check_output(["id", "-u", "smoke"], text=True).strip())
    runtime.mkdir(parents=True, mode=0o700)
    run(["chown", "smoke:smoke", str(runtime)])
    environment = ["env", "HOME=" + str(home), "XDG_RUNTIME_DIR=" + str(runtime), "PATH=/usr/local/bin:/usr/bin:/bin"]
    manager = subprocess.Popen(["runuser", "-u", "smoke", "--", *environment, "/lib/systemd/systemd", "--user"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2)
    try:
        run(["runuser", "-u", "smoke", "--", *environment, "/usr/bin/python3", "-B", str(harness), "--session", package])
        store = home / "project" / (".cortex-dev" if package.endswith("-dev") else ".cortex") / "cortex.sqlite3"
        before = store.read_bytes()
        run(["apt-get", "remove", "-y", package], stdout=subprocess.DEVNULL)
        assert store.read_bytes() == before
        assert not (Path("/usr/lib") / package / "runtime").exists()
        outcome("removal_preserves_user_store")
        run(["apt-get", "install", "-y", "--no-install-recommends", str(legacy)], stdout=subprocess.DEVNULL)
        outcome("legacy_shape_install")
        run(["apt-get", "install", "-y", "--no-install-recommends", str(archive)], stdout=subprocess.DEVNULL)
        assert (Path("/usr/lib") / package / "runtime/src/cortex_acp.py").is_file()
        assert store.read_bytes() == before
        outcome("upgrade_to_bundled_shape")
        run(["apt-get", "remove", "-y", package], stdout=subprocess.DEVNULL)
        if manager.poll() is not None:
            outcome("active_manager_upgrade_restart_and_removal_stop", "unverified",
                    reason="no running user manager; maintainer scripts exercised without an active manager")
    finally:
        if manager.poll() is None:
            manager.terminate()
            manager.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", choices=("docker", "podman"), default="docker")
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--channel", choices=("dev", "release"), default="dev")
    parser.add_argument("--container")
    parser.add_argument("--session")
    args = parser.parse_args()
    if args.session:
        session(args.session)
    elif args.container:
        container(args.container)
    else:
        package = "cortex-workspace-dev" if args.channel == "dev" else "cortex-workspace"
        assert args.artifacts and next(args.artifacts.glob(package + "_*.deb"), None)
        # Only archives and this reviewed harness enter the container. No checkout,
        # user home, credentials, host bus, cgroups or Docker socket are mounted.
        run([args.engine, "run", "--rm", "--mount", "type=bind,source=" + str(args.artifacts.resolve()) + ",target=/archives,readonly",
             "--mount", "type=bind,source=" + str(Path(__file__).resolve().parent) + ",target=/smoke,readonly",
             "--env", "DEBIAN_FRONTEND=noninteractive", BASE, "/bin/sh", "-c", "apt-get update >/dev/null && apt-get install -y --no-install-recommends python3 >/dev/null && exec python3 -B /smoke/check_install.py --container " + package])


if __name__ == "__main__":
    main()
