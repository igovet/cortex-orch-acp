#!/usr/bin/env python3
"""Installed Debian real-input acceptance; Docker only, no host install or screenshots.

Run with --archive candidate.deb --prior-archive previous-owner.deb.
Both display backends and both clean/upgrade installs are mandatory for acceptance.
All output is retained under a fresh --evidence directory. This uses a privileged
disposable systemd container, not a hostile-code sandbox. No host session, home,
bus, cgroup filesystem or credentials are mounted. Runtime containers have no network.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tarfile

ROOT = Path(__file__).resolve().parents[2] if Path(__file__).parent != Path("/harness") else Path("/")
PACKAGE = "cortex-workspace-dev"
UNIT = PACKAGE + ".service"
BASE = "ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c"
RUNTIME = Path("/usr/lib") / PACKAGE / "runtime"


def run(argv, **kw):
    completed = subprocess.run([str(a) for a in argv], timeout=kw.pop("timeout", 120), **kw)
    if completed.returncode:
        raise RuntimeError(f"{Path(argv[0]).name} failed (exit {completed.returncode})")
    return completed


def emit(step, **kw):
    print(json.dumps(dict(step=step, **kw)), flush=True)


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def require_native_health(output):
    # A disappeared window is not evidence of a clean exit. gio-launched children
    # can abort while the input driver and installed service still return success.
    fatal = re.search(r"(?im)^(?:corrupted double-linked list|corrupted size vs[.] prev_size|double free or corruption|free[(][)]:|malloc[(][)]:|Segmentation fault|Aborted(?: [(]core dumped[)])?)", output)
    assert fatal is None, "native process failure diagnostic; archive is not qualified"


def verify_payload(archive, root=Path("/")):
    """Compare installed files with the archive itself; no md5sums metadata is assumed."""
    files = links = 0
    mismatches = []
    process = subprocess.Popen(["dpkg-deb", "--fsys-tarfile", str(archive)],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        with tarfile.open(fileobj=process.stdout, mode="r|") as members:
            for member in members:
                relative = Path(member.name)
                assert not relative.is_absolute() and ".." not in relative.parts, "unsafe archive member"
                target = root / relative
                if member.isfile():
                    files += 1
                    with members.extractfile(member) as stream:
                        expected = hashlib.file_digest(stream, "sha256").hexdigest()
                    if not target.is_file() or target.is_symlink() or sha(target) != expected:
                        mismatches.append(member.name)
                elif member.issym():
                    links += 1
                    if not target.is_symlink() or os.readlink(target) != member.linkname:
                        mismatches.append(member.name)
                elif member.isdir():
                    if not target.is_dir():
                        mismatches.append(member.name)
                elif member.islnk():
                    links += 1
                    if not target.exists() or not os.path.samefile(target, root / member.linkname):
                        mismatches.append(member.name)
                else:
                    raise AssertionError("unsupported archive member")
        process.stdout.close()
        stderr = process.stderr.read()
        assert process.wait(timeout=10) == 0, "archive reader failed: " + stderr.decode(errors="replace")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        process.stdout.close()
        process.stderr.close()
    result = dict(files=files, links=links, mismatches=mismatches)
    assert files > 0 and not mismatches, "installed payload differs: " + json.dumps(result)
    return result


def seed(home):
    state = home / ".local/state" / PACKAGE
    project = "wp_" + "0" * 31 + "1"
    folder = home / "incident-project"
    folder.mkdir(exist_ok=True)
    (state / "product").mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(state, 0o700)
    (state / "projects.json").write_text(json.dumps([dict(projectId=project, canonicalRoot=str(folder),
                                                        displayName="Incident fixture", activation=None)]))
    sprint = dict(id="sp_fixture", projectId=project, description="Fixture sprint", title="Fixture sprint",
                  runMode="automatic", targetBranch=None, planning="created", conversation=[], plan=None,
                  materializedVersion=1, taskIds=["wt_missing_1", "wt_missing_2"], queueState="idle", pauseReason=None)
    (state / "product" / (project + ".json")).write_text(json.dumps(dict(tasks={}, sprints=[sprint])))
    for path in (state / "projects.json", state / "product" / (project + ".json")):
        path.chmod(0o600)


def require_full_worker(input):
    """A worker opens directly into its stream, never an intermediate details pane."""
    assert input.find("frontend-dev activity", ("landmark",)), "worker activity pane missing"
    assert input.find("Find in this activity"), "full worker stream controls missing"
    assert not input.find("Show full history"), "intermediate worker view returned"


def seed_worker(project):
    """One explicitly synthetic worker row; this checks its UI, not delegation."""
    import sqlite3
    with sqlite3.connect(project / ".cortex-dev/cortex.sqlite3", timeout=10) as db:
        (task,) = db.execute("SELECT runtime_task_id FROM workspace_executions ORDER BY rowid DESC LIMIT 1").fetchone()
        now = "2026-11-07T10:00:00+00:00"
        db.execute("INSERT INTO work_assignments (assignment_id,task_id,node_key,requirement_bindings_json,status,requirement_state,idempotency_key,created_at) VALUES (?,?,?,'[]','completed','current',?,?)",
                   ("a-input", task, "input", "input-assignment", now))
        db.execute("INSERT INTO work_assignment_attempts (attempt_id,task_id,assignment_id,attempt_no,requirement_bindings_json,status,idempotency_key,created_at) VALUES (?,?,?,1,'[]','completed',?,?)",
                   ("x-input", task, "a-input", "input-attempt", now))
        db.execute("INSERT INTO task_events (task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES (?,'delegation_requested',?,?,?,'runtime',?)",
                   (task, "a-input", "input-worker", json.dumps(dict(role="worker-frontend-dev", title="Input fixture worker", work_attempt_ref="x-input")), now))


def user_command(*argv):
    return ["runuser", "-u", "acceptance", "--", "env", "HOME=/home/acceptance",
            "XDG_RUNTIME_DIR=/run/user/1001", "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1001/bus", *argv]


def ctl(*argv):
    return run(user_command("systemctl", "--user", *argv), capture_output=True, text=True).stdout.strip()


def require_container(*, desktop_user=False):
    if not Path("/.dockerenv").is_file() or Path("/proc/1/comm").read_text().strip() != "systemd":
        raise RuntimeError("internal mode requires a disposable systemd container")
    if os.getuid() != (1001 if desktop_user else 0):
        raise RuntimeError("wrong disposable container user")


def session(backend, phase, close_cycles=1):
    require_container(desktop_user=True)
    from installed_input import Input, wait
    env = dict(os.environ)
    procs = []
    logs = Path("/evidence") / (backend + "-" + phase)
    logs.mkdir()
    def spawn(argv, name):
        with (logs / (name + ".log")).open("wb") as sink:
            child = subprocess.Popen(argv, stdout=sink, stderr=subprocess.STDOUT, env=env)
        procs.append(child)
        return child
    try:
        if backend == "x11":
            env.update(DISPLAY=":99", GDK_BACKEND="x11")
            spawn(["Xvfb", ":99", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], "display")
            wait(lambda: Path("/tmp/.X11-unix/X99").is_socket(), "X server")
            spawn(["openbox"], "wm")
            time.sleep(1)
        else:
            env.update(WAYLAND_DISPLAY="cw-acceptance", GDK_BACKEND="wayland")
            spawn(["mutter", "--headless", "--wayland", "--no-x11", "--virtual-monitor", "1920x1200",
                   "--wayland-display", "cw-acceptance"], "compositor")
            wait(lambda: Path(env["XDG_RUNTIME_DIR"], "cw-acceptance").is_socket(), "Wayland compositor")
        os.environ.update(env)
        input = Input(backend)
        # gio launch reads the installed desktop entry and its real Exec command.
        run(["gio", "launch", "/usr/share/applications/" + PACKAGE + ".desktop"], env=env)
        wait(lambda: input.find("Sidebar"), "installed desktop window", 60)
        if backend == "x11":
            input.click("Maximize")
        else:
            input.key("super+Up")
        time.sleep(1)
        wait(lambda: input.seen("This machine Connected"), "healthy connected machine")
        wait(lambda: any(n["role"] == "frame" and n["w"] == 1920 and n["h"] == 1200 for n in input.nodes()), "maximized native frame")
        def click(name, expect, roles=None, contains=False):
            input.click(name, roles, contains)
            wait(lambda: input.find("Inbox", ("heading",)) if name == "Inbox" else input.seen(expect), "response to " + name)
            emit("real_input", control=name, response=expect, backend=backend, phase=phase)
        if phase == "incident":
            wait(lambda: input.seen("Incident fixture"), "saved plan fixture")
            click("Inbox", "Inbox", ("link",))
            assert input.find("Inbox", ("heading",)), "Inbox failed to render"
            click("Archive", "Filter archived tasks", ("link",))
            click("Settings", "Back to workspace", ("link",))
            click("Back to workspace", "Inbox", ("link",))
        else:
            click("Add project", "Add project")
            click("Search for folder", "Search for folder", ("list item",), contains=True)
            input.fill("Search for folder", "/home/acceptance/journey")
            # The picker resolves the on-disk directory before selection.
            click("journey", "Activate", ("list item",), contains=True)
            click("Activate", "New task")
            wait(lambda: not input.find("Activate"), "activation finished", 60)
            click("New task", "Describe the task")
            input.fill("Describe the task", "Installed input")
            procs.append(input.paste(" journey"))
            wait(lambda: input.seen("Installed input journey"), "OS clipboard paste")
            # Keep source and destination visible together. Wayland has no
            # client positioning API: real WM tiling keys establish their places.
            if backend == "wayland":
                input.key("super+Right")
                time.sleep(.6)
            target = input.find("Describe the task", ("entry",))
            tx = target["x"] + target["w"] / 2 + (960 if backend == "wayland" else 0)
            ty = target["y"] + target["h"] / 2
            fixture, ready = logs / "drop.txt", logs / "drag-ready"
            fixture.write_text("Installed attachment fixture")
            drag = spawn(["python3", "-B", "/harness/installed_transfer.py", str(fixture), str(ready), backend], "drag")
            if backend == "wayland":
                time.sleep(1)
                input.key("super+Left")
                sx, sy = 480, 600
            else:
                time.sleep(1)
                wid = subprocess.check_output(["xdotool", "search", "--name", "^Installed attachment fixture$"], text=True).splitlines()[-1]
                geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", wid], text=True)
                g = dict(line.split("=", 1) for line in geometry.splitlines())
                sx, sy = int(g["X"]) + int(g["WIDTH"]) / 2, int(g["Y"]) + int(g["HEIGHT"]) / 2
            wait(ready.exists, "visible OS drag source")
            input.move(sx, sy)
            input.button(True)
            time.sleep(.1)
            input.move(sx + 20, sy + 20)
            time.sleep(.2)
            for step in range(1, 21):
                input.move(sx + (tx - sx) * step / 20, sy + (ty - sy) * step / 20)
                time.sleep(.04)
            time.sleep(.5)
            input.button(False)
            wait(lambda: input.seen("drop.txt"), "real OS file drop")
            drag.terminate()
            drag.wait(timeout=5)
            if backend == "wayland":
                input.click_at(tx, ty)
                input.key("super+Up")
                time.sleep(.6)
            emit("os_transfer", backend=backend, clipboard=True, drag_drop=True)
            click("Save as draft", "Installed input journey")
            click("Installed input journey", "Start", ("link",), contains=True)
            click("Start", "Choose direction?")
            assert input.find("Question 1 of 2", ("page tab",)) and input.find("Question 2 of 2", ("page tab",)), "missing question batch tabs"
            click("1 Alpha", "Choose format?")
            input.click("1 Text")
            wait(lambda: not input.find("Question 2 of 2", ("page tab",)), "answered question batch left the screen")
            wait(lambda: Path("/home/acceptance/journey/answered").exists(), "complete question batch delivered")
            seed_worker(Path("/home/acceptance/journey"))
            click("Workers", "Input fixture worker", ("page tab",), contains=True)
            input.click("Coordinator", ("page tab",), contains=True)
            wait(lambda: input.find("Coordinator activity", ("landmark",)) and input.find("Find in this activity"), "coordinator feed selected")
            click("Input fixture worker", "Find in this activity", ("page tab",), contains=True)
            require_full_worker(input)
            input.click("Conversation", ("page tab",))
            # Exact controls, not text: a question option reads "Stop the task".
            def task_control(shown, gone):
                return input.find(shown, ("push button", "button")) and not input.find(gone, ("push button", "button"))
            for name, shown in (("Stop", "Resume"), ("Resume", "Stop")):
                input.click(name)
                wait(lambda: task_control(shown, name), "task control after " + name)
                emit("real_input", control=name, response=shown, backend=backend, phase=phase)
            Path("/home/acceptance/journey/finish").touch()
            wait(lambda: input.seen("Installed package result."), "runtime result", 60)
            click("Review changes", "Changes", ("link",))
            wait(lambda: any(n["name"] == "Changes" and n["role"] == "page tab" and n["selected"] for n in input.nodes()), "review page selected")
            input.key("Escape")
            # Every reachable Settings navigation page is followed through a link.
            click("Settings", "Back to workspace", ("link",))
            settings = ["Updates", "Appearance", "Voice input", "Shortcuts", "All machines", "Overview",
                        "Engine connections", "Orchestration", "Coordinator", "Models", "Routing", "Route preview",
                        "Limits and fallback", "Tools and MCP", "Profiles", "Files and presets",
                        "Worktrees", "Storage and archive", "Diagnostics", "Remote access"]
            for name in settings:
                old = [n["name"] for n in input.nodes() if n["role"] == "heading"]
                input.click(name, ("page tab",), contains=name == "All machines")
                if name == "Coordinator":  # Alias of the default Orchestration page.
                    wait(lambda: any(n["name"] == name and n["selected"] for n in input.nodes()), "selected Coordinator page")
                else:
                    wait(lambda: [n["name"] for n in input.nodes() if n["role"] == "heading"] != old, "Settings content " + name)
                if name != "All machines":
                    input.stable(name)
                emit("settings_page", name=name)
            click("Back to workspace", "Inbox", ("link",))
        input.stable("Sidebar")
        def sidebar_link():
            return any(n["role"] == "link" and re.fullmatch(r"Inbox(?: \\d+)?", n["name"]) for n in input.nodes())
        input.click("Sidebar")
        wait(lambda: not sidebar_link(), "sidebar hidden")
        input.click("Sidebar")
        wait(sidebar_link, "sidebar restored")
        input.closing = True
        input.click("Close window")
        def app_alive():
            for entry in Path("/proc").glob("[0-9]*/exe"):
                try:
                    if entry.parent.stat().st_uid == os.getuid() and (entry.resolve().name == PACKAGE or entry.resolve().name.startswith("WebKit")):
                        return True
                except (FileNotFoundError, PermissionError):
                    pass
            return False
        wait(lambda: not app_alive(), "desktop process exited")
        wait(lambda: not input.nodes(), "window removed")
        # Reopen the actual installed desktop entry without changing its profile or service.
        for cycle in range(2, close_cycles + 1):
            input.closing = False
            run(["gio", "launch", "/usr/share/applications/" + PACKAGE + ".desktop"], env=env)
            wait(lambda: input.find("Sidebar"), "reopened desktop window", 60)
            wait(lambda: input.seen("This machine Connected"), "reopened connected machine")
            if backend == "wayland":
                # Establish global hit-test geometry again: Wayland exposes no window position.
                input.key("super+Up")
                wait(lambda: any(n["role"] == "frame" and n["w"] == 1920 and n["h"] == 1200 for n in input.nodes()), "reopened maximized native frame")
            input.closing = True
            input.click("Close window")
            wait(lambda: not app_alive(), "reopened desktop process exited")
            wait(lambda: not input.nodes(), "reopened window removed")
            emit("repeated_close", backend=backend, phase=phase, cycle=cycle, process_exited=True)
        import socket
        import struct
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(5)
            stream.connect("/run/user/1001/cortex-workspace-dev/service.sock")
            peer_pid, peer_uid, _ = struct.unpack("3i", stream.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert peer_pid > 0 and peer_uid == os.getuid(), "wrong service peer"
            request = json.dumps(dict(jsonrpc="2.0", id=1, method="service.status", params={})).encode()
            stream.sendall(struct.pack(">I", len(request)) + request)
            def read_exact(size):
                data = b""
                while len(data) < size:
                    part = stream.recv(size - len(data))
                    assert part, "service health reply truncated"
                    data += part
                return data
            size, = struct.unpack(">I", read_exact(4))
            assert 0 < size <= 1048576
            reply = json.loads(read_exact(size))
            assert reply.get("id") == 1 and "result" in reply and "error" not in reply, "unhealthy service reply"
        emit("own_close_button", backend=backend, phase=phase, process_exited=True, service_responded=True)
    except Exception:
        emit("failure_native_state", backend=backend, phase=phase, children=[dict(pid=p.pid, exit_code=p.poll()) for p in procs])
        if "input" in locals():
            (logs / "failure-tree.json").write_text(json.dumps(input.nodes(check_errors=False), indent=2))
        raise
    finally:
        for child in reversed(procs):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)


def inside(backend, mode, close_cycles=1):
    require_container()
    from installed_input import wait
    run(["useradd", "--uid", "1001", "--create-home", "--shell", "/bin/sh", "acceptance"])
    wait(lambda: Path("/run/dbus/system_bus_socket").is_socket(), "system bus")
    run(["loginctl", "enable-linger", "acceptance"])
    wait(lambda: Path("/run/user/1001/bus").is_socket(), "lingering user bus")
    def install(path):
        return run(["apt", "install", "--yes", path], capture_output=True, text=True)
    if mode == "upgrade":
        prior = install("/archives/prior.deb")
        print(prior.stdout, prior.stderr, flush=True)
        run(["systemctl", "restart", "user@1001.service"])
        wait(lambda: ctl("is-active", UNIT) == "active", "prior unit")
        before = ctl("show", "-p", "MainPID", "--value", UNIT)
    current = install("/archives/candidate.deb")
    print(current.stdout, current.stderr, flush=True)
    if mode == "upgrade":
        assert "downgrad" not in (current.stdout + current.stderr).lower()
        assert re.search(r"Upgrading:\s+1", current.stdout), "not an ordinary apt upgrade"
        wait(lambda: ctl("show", "-p", "MainPID", "--value", UNIT) != before, "upgrade restarted service")
    else:
        # New lingering user's existing default.target predates package install.
        run(["systemctl", "restart", "user@1001.service"])
    wait(lambda: ctl("is-active", UNIT) == "active", "installed unit")
    ctl("stop", UNIT)
    home = Path("/home/acceptance")
    seed(home)
    run(["chown", "-R", "1001:1001", str(home)])
    ctl("start", UNIT)
    def phase(name):
        try:
            run(user_command("python3", "-B", "/harness/check_installed.py", "--session", backend, name, "--close-cycles", str(close_cycles)), timeout=600)
        except Exception:
            emit("failed_service_state", state=ctl("show", UNIT, "-p", "ActiveState", "-p", "SubState", "-p", "ExecMainStatus", "-p", "MainPID"))
            journal = run(user_command("journalctl", "--user", "--unit", UNIT, "--no-pager"), capture_output=True, text=True)
            Path("/evidence/failure-user-service.log").write_text(journal.stdout)
            raise
        finally:
            project = home / "journey"
            emit("fake_markers", phase=name, answered=(project / "answered").exists(), finish=(project / "finish").exists(),
                 fixture_error=(project / "fixture-error").read_text() if (project / "fixture-error").exists() else None)
        assert ctl("is-active", UNIT) == "active", "service died when window closed"
        emit("service_survives_close", backend=backend, phase=name)
    try:
        phase("incident")
    finally:
        journal = run(["journalctl", "--user-unit=" + UNIT, "--no-pager"], capture_output=True, text=True)
        Path("/evidence/service.log").write_text(journal.stdout)
    ctl("stop", UNIT)
    shutil.rmtree(home / ".local/state" / PACKAGE)
    # Only test files are added. Existing package payload is never patched.
    tests = RUNTIME / "tests"
    tests.mkdir()
    shutil.copyfile("/harness/installed_fixture.py", tests / "test_workspace_journey.py")
    shutil.copyfile("/harness/orchestration_fakes.py", tests / "orchestration_fakes.py")
    override = home / ".config/systemd/user" / (UNIT + ".d")
    override.mkdir(parents=True)
    (override / "fixture.conf").write_text("[Service]\nExecStart=\nExecStart=/usr/bin/cortex-workspace-service-dev /home/acceptance /usr/bin/python3 "
                                         + str(RUNTIME) + " --fixture-launcher " + str(tests / "test_workspace_journey.py") + "\n")
    project = home / "journey"
    project.mkdir()
    (project / "a.txt").write_text("Fixture notes\n")
    run(["git", "-C", str(project), "init", "-q", "-b", "main"])
    run(["git", "-C", str(project), "add", "a.txt"])
    run(["git", "-C", str(project), "-c", "user.name=Acceptance", "-c", "user.email=acceptance@example.invalid",
         "commit", "-q", "-m", "Disposable acceptance fixture"])
    run(["chown", "-R", "1001:1001", str(home)])
    ctl("daemon-reload")
    ctl("start", UNIT)
    try:
        phase("journey")
    finally:
        journal = run(["journalctl", "--user-unit=" + UNIT, "--no-pager"], capture_output=True, text=True)
        Path("/evidence/service.log").write_text(journal.stdout)
    # Compare directly: these archives do not carry a DEBIAN/md5sums file.
    verified = verify_payload(Path("/archives/candidate.deb"))
    Path("/evidence/payload.json").write_text(json.dumps(verified, indent=2) + "\n")
    emit("installed_payload_matches_archive", **verified)
    emit("installed_scenario_passed", backend=backend, mode=mode)


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--prior-archive", type=Path)
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--display", choices=("both", "x11", "wayland"), default="both")
    parser.add_argument("--close-cycles", type=int, choices=range(1, 11), default=1,
                        help="native close/reopen cycles per phase, 1 to 10")
    parser.add_argument("--inside", nargs=2, help=argparse.SUPPRESS)
    parser.add_argument("--session", nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.session:
        session(*args.session, close_cycles=args.close_cycles)
        return
    if args.inside:
        inside(*args.inside, close_cycles=args.close_cycles)
        return
    if not (args.archive and args.prior_archive and args.evidence):
        parser.error("--archive, --prior-archive and --evidence are required")
    archive, prior = (p.resolve(strict=True) for p in (args.archive, args.prior_archive))
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    archive_hash, prior_hash = sha(archive), sha(prior)
    for path in (archive, prior):
        assert subprocess.check_output(["dpkg-deb", "-f", str(path), "Package"], text=True).strip() == PACKAGE
    depends = set()
    for path in (archive, prior):
        fields = subprocess.check_output(["dpkg-deb", "-f", str(path), "Depends"], text=True)
        depends.update(re.match(r"[a-z0-9.+-]+", part.strip()).group() for part in fields.split(","))
    image = "cortex-installed-check:" + archive_hash[:16]
    with tempfile.TemporaryDirectory(prefix="installed-", dir=evidence) as tmp:
        context = Path(tmp)
        for name in ("check_installed.py", "installed_input.py", "installed_fixture.py", "installed_transfer.py"):
            shutil.copyfile(Path(__file__).with_name(name), context / name)
        shutil.copyfile(ROOT / "tests/orchestration_fakes.py", context / "orchestration_fakes.py")
        (context / "Dockerfile").write_text("FROM " + BASE + "\nENV container=docker\nRUN apt-get update && apt-get install -y --no-install-recommends "
            + " ".join(sorted(depends)) + " systemd-sysv dbus-user-session xvfb openbox xdotool x11-utils mutter at-spi2-core python3-gi gir1.2-atspi-2.0 git libglib2.0-bin xclip wl-clipboard gir1.2-gtk-3.0"
            + " && rm -rf /var/lib/apt/lists/*\nRUN echo 'path-include=/usr/share/doc/cortex-workspace-dev/*' >> /etc/dpkg/dpkg.cfg.d/excludes\nCOPY *.py /harness/\nSTOPSIGNAL SIGRTMIN+3\nCMD [\"/sbin/init\"]\n")
        built = subprocess.run(["docker", "build", "-t", image, str(context)], capture_output=True, text=True, timeout=600)
        (evidence / "image-build.log").write_text(built.stdout + built.stderr)
        print(built.stdout + built.stderr, end="", flush=True)
        built.check_returncode()
    image_id = subprocess.check_output(["docker", "image", "inspect", image, "--format", "{{.Id}}"], text=True).strip()
    result = dict(archive_sha256=archive_hash, prior_sha256=prior_hash, image=image_id,
                  harness_sha256={name: sha(Path(__file__).with_name(name)) for name in ("check_installed.py", "installed_input.py", "installed_fixture.py", "installed_transfer.py")},
                  scenarios=[], close_cycles=args.close_cycles, accepted=False)
    try:
        for backend in (("x11", "wayland") if args.display == "both" else (args.display,)):
            for mode in ("clean", "upgrade"):
                folder = evidence / (backend + "-" + mode)
                folder.mkdir()
                os.chmod(folder, 0o777)  # Disposable container user writes only this evidence directory.
                identifier = subprocess.check_output(["docker", "run", "--detach", "--privileged", "--cgroupns=private",
                    "--network=none", "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp",
                    "--mount", f"type=bind,source={archive},target=/archives/candidate.deb,readonly",
                    "--mount", f"type=bind,source={prior},target=/archives/prior.deb,readonly",
                    "--mount", f"type=bind,source={folder},target=/evidence", image], text=True).strip()
                try:
                    with (folder / "output.log").open("wb") as sink:
                        completed = subprocess.run(["docker", "exec", identifier, "python3", "-B", "/harness/check_installed.py",
                                                    "--inside", backend, mode, "--close-cycles", str(args.close_cycles)], stdout=sink, stderr=subprocess.STDOUT, timeout=1200)
                    # Complete fixture-only output; never an owner's log or environment.
                    output = (folder / "output.log").read_text()
                    print(output, end="", flush=True)
                    result["scenarios"].append(dict(backend=backend, mode=mode, exit_code=completed.returncode))
                    assert completed.returncode == 0, backend + " " + mode + " failed"
                    require_native_health(output)
                finally:
                    run(["docker", "rm", "--force", identifier])
        assert sha(archive) == archive_hash and sha(prior) == prior_hash, "archive changed during check"
        result["accepted"] = args.display == "both" and len(result["scenarios"]) == 4
    finally:
        (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    assert result["accepted"], "partial display run is diagnostic, not owner acceptance"
    print("PASS: exact installed archive; X11 and Wayland, clean and plain apt upgrade; all windows closed.")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("FAIL: installed-package acceptance is incomplete; do not hand over this archive.", flush=True)
        raise
