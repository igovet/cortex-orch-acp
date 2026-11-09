"""Explicit owner maintenance jobs. No model calls; no raw subprocess output is retained."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
import machine_tools as m

RELEASES = "https://api.github.com/repos/igovet/cortex-orch-acp/releases/latest"
TERMINAL = {"done", "failed", "cancelled", "ready", "unknown"}
MAX_DOWNLOAD = 1024 * 1024 * 1024


def fail(code, message):
    raise m.Refused(code, message)


def folder(state, identity):
    if not isinstance(identity, str) or not re.fullmatch(r"[a-f0-9]{32}", identity):
        fail("invalid_job", "Unknown operation.")
    return Path(state) / "maintenance" / identity


def save(path, **patch):
    value = m.read_json(path / "status.json", {})
    value.update(patch, updated=time.time())
    m.write_json(path / "status.json", value)
    return value


def read(state, identity):
    path = folder(state, identity)
    value = m.read_json(path / "status.json", None)
    if not value:
        fail("unknown_job", "This operation is no longer available.")
    if value["phase"] not in TERMINAL and time.time() - value["updated"] > 660:
        return save(path, phase="unknown", message="The operation stopped reporting. Check the folder or installed version before retrying.")
    return value


def version(package="cortex-workspace"):
    p = subprocess.run(["/usr/bin/dpkg-query", "-W", "-f=${Version}", package],
                       capture_output=True, text=True, timeout=5)
    return p.stdout.strip() if p.returncode == 0 else None


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def newer(candidate, current):
    return subprocess.run(["/usr/bin/dpkg", "--compare-versions", candidate, "gt", current],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def fetch(url, limit, *, fixture=False):
    # Only release-owned HTTPS URLs; local HTTP is an internal fixture seam, not a wire option.
    if not fixture and not (url.startswith("https://api.github.com/") or
                            url.startswith("https://github.com/igovet/cortex-orch-acp/releases/download/")):
        fail("release_url_refused", "The release contains an unsupported download URL.")
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "Cortex-Workspace/1.0.6"}), timeout=10) as response:
        data = response.read(limit + 1)
        if len(data) > limit:
            fail("release_too_large", "The release response exceeds its size limit.")
        return data


def check(state, *, url=RELEASES, current=None, fixture=False):
    current = current or version()
    if not current:
        fail("not_installed", "Install the release package before checking for updates.")
    release = json.loads(fetch(url, 1024 * 1024, fixture=fixture))
    tag = release.get("tag_name", "")
    candidate = tag.removeprefix("v")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[+~.-][A-Za-z0-9.+~-]+)?", candidate):
        fail("invalid_release", "The release has no valid package version.")
    if release.get("draft") or release.get("prerelease") or not newer(candidate, current):
        return {"phase": "current", "current": current, "message": "No newer stable release. Downgrades are refused."}
    arch = subprocess.check_output(["/usr/bin/dpkg", "--print-architecture"], text=True).strip()
    name = "cortex-workspace_" + candidate + "_" + arch + ".deb"
    assets = release.get("assets", [])
    def asset(wanted):
        found = [a.get("browser_download_url") for a in assets if a.get("name") == wanted]
        if len(found) != 1 or not isinstance(found[0], str):
            fail("release_incomplete", "The release is missing its exact archive or SHA-256 sidecar.")
        return found[0]
    archive_url, checksum_url = asset(name), asset(name + ".sha256")
    checksum = fetch(checksum_url, 1024, fixture=fixture).decode("ascii").strip()
    match = re.fullmatch(r"([a-f0-9]{64})  " + re.escape(name), checksum)
    if not match:
        fail("invalid_checksum", "The release checksum file is invalid.")
    value = {"phase": "available", "current": current, "version": candidate, "name": name,
             "url": archive_url, "sha256": match[1], "notes": str(release.get("body") or "")[:8000],
             "message": "SHA-256 is published by the release over HTTPS; this workflow does not publish a signature."}
    with m.Locked(state):
        m.write_json(Path(state) / "maintenance/candidate.json", value)
    return value


def ensure_quiet(projects):
    # Read-only inspection of the runtime's own records, across every checkout.
    for project in projects:
        db = m.store_reader(project["root"])
        if db is None:
            continue
        try:
            tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if "runtime_tasks" not in tables:
                fail("activity_unknown", "Project activity could not be verified. No package change started.")
            if db.execute("SELECT 1 FROM runtime_tasks WHERE state NOT IN ('COMPLETED','FAILED','CANCELLED','PAUSED') LIMIT 1").fetchone():
                fail("work_running", "Stop the running Cortex work before installing an update.")
            if "workspace_starts" in tables and db.execute(
                    "SELECT 1 FROM workspace_starts s LEFT JOIN workspace_executions e USING(execution_id) WHERE s.state='unknown' OR e.runtime_task_id IS NULL LIMIT 1").fetchone():
                fail("activity_unknown", "A task start is unconfirmed. Resolve it before updating.")
        except m.Refused:
            raise
        except Exception:
            fail("activity_unknown", "Project activity could not be verified. No package change started.")
        finally:
            db.close()


def installing(state):
    return any(m.read_json(p, {}).get("kind") in ("install", "rollback") and
               m.read_json(p, {}).get("phase") not in ("done", "failed", "cancelled") and not m.read_json(p, {}).get("acknowledged")
               for p in (Path(state) / "maintenance").glob("*/status.json"))


def start(state, params, *, spawn=True, projects=()):
    identity = params.get("id")
    path = folder(state, identity)
    kind = params.get("kind")
    if kind not in ("clone", "download", "install", "rollback"):
        fail("invalid_job", "Unsupported operation.")
    with m.Locked(state):
        if (path / "status.json").exists():
            return read(state, identity)  # An acknowledged identity never repeats work.
        for previous in (Path(state) / "maintenance").glob("*/status.json"):
            v = m.read_json(previous, {})
            if v.get("phase") not in TERMINAL:
                fail("maintenance_busy", "Another operation is still running. Check its status first.")
        if kind == "clone":
            url = params.get("url", "")
            if isinstance(url, str) and url.startswith(("/", "~/")):
                source = Path(url).expanduser().resolve(strict=True)
                protected = [Path(state).resolve(), *[Path(os.environ[k]).resolve() for k in
                             ("XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME") if os.environ.get(k)]]
                if any(part in {".cortex", ".cortex-dev", ".ssh", ".codex", ".claude", ".config"} for part in source.parts) or any(
                        source == p or p in source.parents for p in protected):
                    fail("invalid_repository", "Choose a local bare Git repository outside private settings and runtime data.")
                env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}
                probe = subprocess.run(["/usr/bin/git", "-C", str(source), "rev-parse", "--is-bare-repository"],
                                       env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                       stderr=subprocess.DEVNULL, timeout=5)
                if probe.returncode or probe.stdout.strip() != b"true":
                    fail("invalid_repository", "The local source must be a bare Git repository.")
                url = str(source)
            elif not isinstance(url, str) or not re.fullmatch(r"(?:https://github\.com/|git@github\.com:)[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?", url):
                fail("invalid_repository", "Enter a credential-free GitHub URL or an absolute local bare repository path.")
            destination = params.get("destination", "")
            if not isinstance(destination, str) or not destination.startswith(("/", "~/")) or "\0" in destination:
                fail("invalid_destination", "Choose a full destination folder path.")
            destination = Path(destination).expanduser()
            destination.parent.resolve(strict=True)
            if destination.exists() or destination.is_symlink():
                fail("destination_exists", "Choose a new folder. Existing folders are never overwritten.")
            data = {"url": url, "destination": str(destination)}
        else:
            if os.environ.get("CORTEX_WORKSPACE_CHANNEL") != "release":
                fail("dev_updates", "Dev updates come from local builds, not GitHub releases.")
            if kind == "download":
                data = m.read_json(Path(state) / "maintenance/candidate.json", {})
                if not data or not newer(data["version"], version() or data["version"]):
                    fail("stale_release", "Check for a newer release again.")
            else:
                if params.get("confirmed") is not True:
                    fail("consent_required", "Confirm the system package change first.")
                ensure_quiet(projects)
                source = folder(state, params.get("source"))
                data = read(state, params["source"])
                if data.get("phase") != "ready":
                    fail("not_ready", "Download and verify the release first.")
                data = {**data, "archive": str(source / ("previous.deb" if kind == "rollback" else "update.deb"))}
                if not Path(data["archive"]).is_file():
                    fail("rollback_unavailable", "No previous package archive is retained. No rollback was attempted.")
                if not shutil.which("pkexec") or not Path("/usr/bin/apt-get").is_file():
                    fail("privilege_prompt_unavailable", "Install using your system package manager; pkexec is unavailable.")
        path.mkdir(parents=True, mode=0o700)
        m.write_json(path / "request.json", {**data, "kind": kind, "projects": list(projects)})
        value = save(path, id=identity, kind=kind, phase="starting", message="Starting…")
        if spawn:
            argv = [sys.executable, "-I", "-B", str(Path(__file__).resolve()), str(state), identity]
            try:
                if kind in ("install", "rollback"):
                    # Package postinst restarts Workspace. Its installer must not be killed
                    # with that service's cgroup; a separate transient user unit owns it.
                    launcher = ["/usr/bin/systemd-run", "--user", "--collect", "--quiet",
                                "--service-type=exec", "--unit=cortex-workspace-update-" + identity]
                    for key in ("HOME", "DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"):
                        if key in os.environ:
                            launcher.append("--setenv=" + key + "=" + os.environ[key])
                    result = subprocess.run([*launcher, "--", *argv], stdin=subprocess.DEVNULL,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
                    if result.returncode:
                        return save(path, phase="failed", message="The system user manager could not start the installer. Nothing was installed; use the system package manager.")
                else:
                    subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)
            except (OSError, subprocess.TimeoutExpired):
                return save(path, phase="unknown", message="The operation launch could not be confirmed. Recheck before retrying.")
        return value


def process(path, argv, *, timeout=600):
    env = {k: v for k, v in os.environ.items() if k in
           {"HOME", "PATH", "LANG", "DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"}}
    env.update(GIT_TERMINAL_PROMPT="0", GIT_CONFIG_NOSYSTEM="1")
    # Do not run project Git config, hooks, credential helpers or shell commands.
    if argv[0] == "/usr/bin/git":
        env["GIT_CONFIG_GLOBAL"] = "/dev/null"
        env["GIT_SSH_COMMAND"] = "/usr/bin/ssh -oBatchMode=yes -oStrictHostKeyChecking=yes"
    child = subprocess.Popen(argv, env=env, cwd="/", stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    installer = argv[0] == "/usr/bin/pkexec"
    expires = time.monotonic() + timeout
    try:
        while child.poll() is None:
            if not installer and ((path / "cancel").exists() or time.monotonic() > expires):
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                fail("cancelled", "Cancelled. Any partial clone folder is kept for inspection.")
            save(path)
            time.sleep(0.2)
        if child.returncode:
            fail("operation_failed", "The operation failed. Check network, Git access or the system package manager. No automatic retry or rollback.")
    finally:
        if child.poll() is None and not installer:
            os.killpg(child.pid, signal.SIGTERM)
            child.wait(timeout=3)


def deb_fields(archive):
    return subprocess.check_output(["/usr/bin/dpkg-deb", "-f", str(archive), "Package", "Version", "Architecture"],
                                   text=True, timeout=10).splitlines()


def download(path, data, *, fixture=False):
    # Streaming avoids retaining a package-sized response in memory.
    url = data["url"]
    if not fixture and not url.startswith("https://github.com/igovet/cortex-orch-acp/releases/download/"):
        fail("release_url_refused", "Unsupported archive URL.")
    archive = path / "update.deb"
    sha, size = hashlib.sha256(), 0
    with urllib.request.urlopen(url, timeout=10) as response, archive.open("xb") as out:
        while True:
            if (path / "cancel").exists():
                fail("cancelled", "Download cancelled. Nothing installed.")
            chunk = response.read(256 * 1024)
            if not chunk:
                break
            size += len(chunk)
            if size > MAX_DOWNLOAD:
                fail("release_too_large", "The archive exceeds its size limit.")
            sha.update(chunk)
            out.write(chunk)
            save(path, phase="downloading", message="Downloading release", bytes=size)
    if sha.hexdigest() != data["sha256"]:
        archive.unlink()
        fail("bad_checksum", "Checksum mismatch. Nothing installed. Check again before retrying.")
    fields = deb_fields(archive)
    arch = subprocess.check_output(["/usr/bin/dpkg", "--print-architecture"], text=True).strip()
    if fields != ["Package: cortex-workspace", "Version: " + data["version"], "Architecture: " + arch]:
        fail("wrong_package", "The archive identity does not match the selected release.")
    # Retain a previous archive only when apt actually has this exact installed version.
    old = Path("/var/cache/apt/archives") / ("cortex-workspace_" + data["current"] + "_" + arch + ".deb")
    rollback = False
    if old.is_file() and deb_fields(old) == ["Package: cortex-workspace", "Version: " + data["current"], "Architecture: " + arch]:
        shutil.copyfile(old, path / "previous.deb")
        rollback = True
    return save(path, phase="ready", message="Checksum verified. Ready for your installation consent.",
                version=data["version"], current=data["current"], rollback=rollback, sha256=data["sha256"],
                previousSha256=digest(path / "previous.deb") if rollback else None)


def worker(state, identity):
    path = folder(state, identity)
    data = m.read_json(path / "request.json", {})
    try:
        kind = data["kind"]
        if kind == "clone":
            destination = Path(data["destination"])
            destination.mkdir(mode=0o700)  # Atomic reservation; never reuse an existing folder.
            save(path, phase="cloning", message="Cloning repository…", destination=str(destination))
            process(path, ["/usr/bin/git", "-c", "core.hooksPath=/dev/null", "-c", "init.templateDir=",
                           "clone", "--no-hardlinks", "--progress", "--", data["url"], str(destination)])
            save(path, phase="done", message="Clone complete.", destination=str(destination))
        elif kind == "download":
            download(path, data)
        else:
            archive = Path(data["archive"])
            actual = digest(archive)
            if kind == "rollback" and (version() != data["version"] or not data.get("previousSha256") or actual != data["previousSha256"]):
                fail("rollback_integrity", "The retained previous package changed. Rollback refused.")
            if kind == "install" and (actual != data["sha256"] or version() != data["current"] or not newer(data["version"], data["current"])):
                fail("stale_release", "The installed version or downloaded archive changed. Check again.")
            ensure_quiet(data.get("projects", []))
            save(path, phase="installing", message="Approve the system privilege prompt. Installing…" if kind == "install" else "Approve the system privilege prompt. Rolling back…")
            args = ["/usr/bin/pkexec", "/usr/bin/apt-get", "install", "--no-remove", "-y"]
            if kind == "rollback":
                args.append("--allow-downgrades")
            process(path, [*args, str(archive)])
            expected = data["version"] if kind == "install" else data["current"]
            if version() != expected:
                fail("install_unconfirmed", "The package manager ended but the installed version is unconfirmed. Check it before retrying.")
            save(path, phase="done", message="Installed. Restart Cortex Workspace.", version=expected)
    except m.Refused as error:
        save(path, phase="cancelled" if error.code == "cancelled" else "failed", code=error.code, message=error.message)
    except Exception:
        save(path, phase="unknown" if data.get("kind") in ("install", "rollback") else "failed", message="The operation could not finish. Nothing was retried; check network and package manager state.")


def action(state, params, *, projects=()):
    action = params.get("action")
    if action == "voice.status":
        root = Path(__file__).resolve().parents[2]
        spec = m.read_json(root / "packaging/voice/model.json", None)
        if spec is None:
            fail("voice_unavailable", "The packaged speech model manifest is unavailable.")
        base = Path(state) / "voice" / spec["name"]
        ready = all((base / name).is_file() and not (base / name).is_symlink()
                    and hashlib.sha256((base / name).read_bytes()).hexdigest() == digest
                    for name, digest in spec["files"].items())
        return {"phase": "ready" if ready else "not_downloaded",
                "message": "Verified local model ready." if ready else "Not downloaded or incomplete. First use downloads and verifies the Russian model; retry is safe."}
    if action == "update.status":
        return {"phase": "dev" if os.environ.get("CORTEX_WORKSPACE_CHANNEL") != "release" else "idle",
                "current": version("cortex-workspace-dev" if os.environ.get("CORTEX_WORKSPACE_CHANNEL") != "release" else "cortex-workspace"), "privilegePrompt": Path("/usr/bin/pkexec").is_file(),
                "message": "Dev updates come from local builds." if os.environ.get("CORTEX_WORKSPACE_CHANNEL") != "release" else "Check GitHub for a newer stable release."}
    if action == "update.check":
        if os.environ.get("CORTEX_WORKSPACE_CHANNEL") != "release":
            fail("dev_updates", "Dev updates come from local builds.")
        try:
            return check(state)
        except m.Refused:
            raise
        except (OSError, ValueError):
            fail("update_check_failed", "Could not check GitHub releases. Check the network and retry Check; nothing was installed.")
    if action == "start":
        return start(state, params, projects=projects)
    if action == "read":
        return read(state, params.get("id"))
    if action == "acknowledge":
        path = folder(state, params.get("id"))
        value = read(state, params["id"])
        if params.get("confirmed") is not True or value["phase"] != "unknown" or value["kind"] not in ("install", "rollback"):
            fail("confirmation_required", "Check the system package manager and explicitly confirm before allowing work.")
        return save(path, acknowledged=True, message="Installation remains unconfirmed. The owner checked the package manager and allowed work; this is not an installation success.")
    if action == "cancel":
        path = folder(state, params.get("id"))
        value = read(state, params["id"])
        if value["kind"] in ("install", "rollback"):
            fail("install_in_progress", "Do not interrupt the system package manager. Wait for its result.")
        (path / "cancel").touch(mode=0o600)
        return value
    fail("invalid_action", "Unsupported maintenance action.")


if __name__ == "__main__":
    worker(sys.argv[1], sys.argv[2])
