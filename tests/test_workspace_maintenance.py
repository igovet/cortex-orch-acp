"""No-paid release HTTP, real local Git and isolated connection-store checks."""
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import uuid

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))
import machine_tools as m
import maintenance as jobs
import engine_connections as connections


@pytest.fixture
def isolated(tmp_path, monkeypatch):
    for name in ("HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
        p = tmp_path / name
        p.mkdir()
        monkeypatch.setenv(name, str(p))
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "release")
    return tmp_path / "state"


@pytest.fixture
def release(tmp_path):
    arch = subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
    tree = tmp_path / "web"
    tree.mkdir()
    package = tmp_path / "package"
    (package / "DEBIAN").mkdir(parents=True)
    (package / "DEBIAN").chmod(0o755)  # dpkg-deb refuses what a 0077 umask would leave.
    ver = "1.0.7+z20261106000000000000.sha256.0123456789abcdef"
    name = "cortex-workspace_" + ver + "_" + arch + ".deb"
    (package / "DEBIAN/control").write_text(
        "Package: cortex-workspace\nVersion: " + ver + "\nArchitecture: " + arch +
        "\nMaintainer: Fixture <fixture@example.invalid>\nDescription: No-install fixture\n")
    subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(package), str(tree / name)],
                   check=True, stdout=subprocess.DEVNULL)
    digest = jobs.digest(tree / name)
    (tree / (name + ".sha256")).write_text(digest + "  " + name + "\n")

    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass
    server = ThreadingHTTPServer(("127.0.0.1", 0), partial(Quiet, directory=str(tree)))
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    base = "http://127.0.0.1:" + str(server.server_port)
    document = {"tag_name": "v" + ver, "draft": False, "prerelease": False,
                "assets": [{"name": n, "browser_download_url": base + "/" + n}
                           for n in (name, name + ".sha256")]}
    (tree / "latest").write_text(json.dumps(document))
    try:
        yield tree, base, ver, document
    finally:
        server.shutdown()
        thread.join()
        server.server_close()


def test_http_release_download_verifies_exact_debian_identity(isolated, release):
    tree, base, ver, _ = release
    selected = jobs.check(isolated, url=base + "/latest", current="1.0.6", fixture=True)
    assert selected["version"] == ver
    path = jobs.folder(isolated, uuid.uuid4().hex)
    path.mkdir(parents=True)
    jobs.save(path, phase="starting")
    page = jobs.download(path, selected, fixture=True)
    assert page["phase"] == "ready"
    assert jobs.digest(path / "update.deb") == selected["sha256"]
    assert not page["rollback"]


def test_bad_checksum_downgrade_and_network_never_install(isolated, release):
    tree, base, ver, document = release
    selected = jobs.check(isolated, url=base + "/latest", current="1.0.6", fixture=True)
    path = jobs.folder(isolated, uuid.uuid4().hex)
    path.mkdir(parents=True)
    with pytest.raises(m.Refused) as caught:
        jobs.download(path, {**selected, "sha256": "0" * 64}, fixture=True)
    assert caught.value.code == "bad_checksum"
    assert not (path / "update.deb").exists()
    assert jobs.check(isolated, url=base + "/latest", current="2.0.0", fixture=True)["phase"] == "current"
    with pytest.raises(Exception):
        jobs.check(isolated, url=base + "/missing", current="1.0.6", fixture=True)
    with pytest.raises(m.Refused) as caught:
        jobs.check(isolated, url=base + "/latest", current="1.0.6")  # HTTP not a production capability
    assert caught.value.code == "release_url_refused"


def test_consent_install_failure_and_explicit_rollback(isolated, release, monkeypatch):
    tree, base, ver, _ = release
    selected = jobs.check(isolated, url=base + "/latest", current="1.0.6", fixture=True)
    source = uuid.uuid4().hex
    path = jobs.folder(isolated, source)
    path.mkdir(parents=True)
    jobs.save(path, id=source, kind="download", phase="starting")
    jobs.download(path, selected, fixture=True)
    with pytest.raises(m.Refused, match="consent_required"):
        jobs.start(isolated, {"id": uuid.uuid4().hex, "kind": "install", "source": source}, spawn=False)
    called = []
    def failed(path, argv):
        called.append(argv)
        raise m.Refused("operation_failed", "Fixture install refused.")
    monkeypatch.setattr(jobs, "process", failed)
    monkeypatch.setattr(jobs, "version", lambda *args: "1.0.6")
    key = uuid.uuid4().hex
    jobs.start(isolated, {"id":key, "kind":"install", "source":source, "confirmed":True}, spawn=False)
    assert m.read_json(jobs.folder(isolated, key) / "request.json", {})["kind"] == "install"
    jobs.worker(isolated, key)
    assert jobs.read(isolated, key)["phase"] == "failed"
    assert called[0][:5] == ["/usr/bin/pkexec", "/usr/bin/apt-get", "install", "--no-remove", "-y"]
    # A delivery replay observes the original failure and never runs the installer again.
    assert jobs.start(isolated, {"id":key, "kind":"install"}, spawn=False)["phase"] == "failed"
    assert len(called) == 1
    with pytest.raises(m.Refused, match="rollback_unavailable"):
        jobs.start(isolated, {"id":uuid.uuid4().hex, "kind":"rollback", "source":source, "confirmed":True}, spawn=False)
    previous = path / "previous.deb"
    previous.write_bytes(b"previous fixture archive")
    jobs.save(path, previousSha256=jobs.digest(previous), rollback=True)
    def rollback(p, argv):
        called.append(argv)
        monkeypatch.setattr(jobs, "version", lambda *args: "1.0.6")
    monkeypatch.setattr(jobs, "process", rollback)
    monkeypatch.setattr(jobs, "version", lambda *args: ver)
    key = uuid.uuid4().hex
    jobs.start(isolated, {"id":key, "kind":"rollback", "source":source, "confirmed":True}, spawn=False)
    jobs.worker(isolated, key)
    assert jobs.read(isolated, key)["phase"] == "done"
    assert "--allow-downgrades" in called[-1]


def test_clone_real_local_bare_repository_and_cancel(isolated, tmp_path):
    repo, bare, target = (tmp_path / x for x in ("repo", "origin.git", "clone"))
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    (repo / "README.md").write_text("Local fixture\n")
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                    "commit", "-qm", "fixture"], check=True)
    subprocess.run(["git", "clone", "--bare", "-q", str(repo), str(bare)], check=True)
    key = uuid.uuid4().hex
    jobs.start(isolated, {"id":key, "kind":"clone", "url":str(bare), "destination":str(target)}, spawn=False)
    path = jobs.folder(isolated, key)
    jobs.worker(isolated, key)
    assert jobs.read(isolated, key)["phase"] == "done"
    assert (target / "README.md").read_text() == "Local fixture\n"
    with pytest.raises(m.Refused, match="destination_exists"):
        jobs.start(isolated, {"id":uuid.uuid4().hex, "kind":"clone", "url":"https://github.com/fixture/repo", "destination":str(target)}, spawn=False)
    with pytest.raises(m.Refused, match="invalid_repository"):
        jobs.start(isolated, {"id":uuid.uuid4().hex, "kind":"clone", "url":str(repo), "destination":str(tmp_path / "other")}, spawn=False)
    private = Path(os.environ["XDG_CONFIG_HOME"]) / "private.git"
    subprocess.run(["git", "clone", "--bare", "-q", str(repo), str(private)], check=True)
    with pytest.raises(m.Refused, match="invalid_repository"):
        jobs.start(isolated, {"id":uuid.uuid4().hex, "kind":"clone", "url":str(private),
                             "destination":str(tmp_path / "private-clone")}, spawn=False)
    (path / "cancel").touch()
    with pytest.raises(m.Refused, match="cancelled"):
        jobs.process(path, ["/usr/bin/sleep", "5"])


def test_dev_channel_never_checks_or_installs_and_voice_is_truthful(isolated, monkeypatch):
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    assert jobs.action(isolated, {"action":"update.status"})["phase"] == "dev"
    with pytest.raises(m.Refused, match="dev_updates"):
        jobs.action(isolated, {"action":"update.check"})
    assert jobs.action(isolated, {"action":"voice.status"})["phase"] == "not_downloaded"


def test_api_key_private_store_no_echo_and_stale_revision(isolated, monkeypatch):
    from cortex_runtime.configuration import management
    from cortex_runtime.connections import registry
    from types import SimpleNamespace
    monkeypatch.setattr(connections, "installed", lambda engine: (SimpleNamespace(launch_command=("/usr/bin/true", "/usr/bin/true")), None))
    request = {"engine":"codex-acp", "name":"Fixture key", "key":"fixture-only-not-a-real-key", "revision":management.read_connections()["revision"]}
    reply = connections.add(request)
    assert request["key"] not in json.dumps(reply)
    secret = registry.secret_path(reply["connectionId"])
    assert secret.stat().st_mode & 0o777 == 0o600
    assert secret.stat().st_size == len(request["key"])
    assert request["key"] not in registry.registry_path().read_text()
    with pytest.raises(m.Refused, match="revision_conflict"):
        connections.add(request)
    for p in (isolated / "logs").glob("*"):
        assert request["key"] not in p.read_text()


def test_unconfirmed_installer_needs_explicit_acknowledgement_not_a_fake_success(isolated):
    key = uuid.uuid4().hex
    path = jobs.folder(isolated, key)
    path.mkdir(parents=True)
    jobs.save(path, id=key, kind="install", phase="unknown")
    assert jobs.installing(isolated)
    with pytest.raises(m.Refused, match="confirmation_required"):
        jobs.action(isolated, {"action":"acknowledge", "id":key})
    page = jobs.action(isolated, {"action":"acknowledge", "id":key, "confirmed":True})
    assert page["phase"] == "unknown" and page["acknowledged"]
    assert not jobs.installing(isolated)


def test_native_recheck_refuses_missing_login_without_onboarding(isolated, monkeypatch):
    monkeypatch.setattr(connections, "native_command", lambda e: {"program":"/usr/bin/false","args":["login"],"adapter":["/usr/bin/true","/usr/bin/true"]})
    with pytest.raises(m.Refused, match="native_login_required"):
        connections.action({"action":"recheck", "engine":"codex-acp"})


def test_explicit_workflow_controls_build_the_runtime_instructions():
    import owner_workflows
    harvest = owner_workflows.describe({"action":"harvest"})["description"]
    assert "Create complete source-backed documentation" in harvest
    assert "explicitly requested a knowledge harvest" in harvest
    refresh = owner_workflows.describe({"action":"harvest-refresh", "scope":"API docs"})["description"]
    assert "API docs" in refresh and "explicitly requested a documentation refresh" in refresh
    skill = owner_workflows.describe({"action":"skill-management", "scope":"Import a testing skill"})["description"]
    assert "explicitly invoked /skill-management" in skill
    with pytest.raises(m.Refused):
        owner_workflows.describe({"action":"skill-management", "scope":""})
    with pytest.raises(m.Refused):
        owner_workflows.describe({"action":"arbitrary-command"})


def test_clear_exact_finished_tasks_rechecks_age_and_runtime_guards(tmp_path, isolated):
    from test_workspace_api import api_for, create_command, query, runtime_binding
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    m.task_action(api.store.project_root, api.project_id, isolated, {"action":"merged", "taskId":task})
    params = {"action":"clear_old", "taskId":task, "cutoff":m.now_ms()-1000, "confirmed":True}
    with pytest.raises(m.Refused, match="clear_preview_changed"):
        m.task_action(api.store.project_root, api.project_id, isolated, params)
    assert api.handle(query("tasks.get", task))["taskId"] == task
    params["cutoff"] = m.now_ms()
    m.task_action(api.store.project_root, api.project_id, isolated, params)
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 0
    assert m.product_of(isolated, api.project_id)["tasks"][task]["purged"]

    task = api.handle(create_command("second"))["target"]
    runtime_binding(api, task)
    m.task_action(api.store.project_root, api.project_id, isolated, {"action":"merged", "taskId":task})
    with pytest.raises(m.Refused):
        m.task_action(api.store.project_root, api.project_id, isolated,
                      {"action":"clear_old", "taskId":task, "cutoff":m.now_ms(), "confirmed":True})
    assert api.handle(query("tasks.get", task))["taskId"] == task


def test_update_refuses_running_and_unconfirmed_project_activity(tmp_path):
    from test_workspace_api import api_for, create_command, runtime_binding
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task)
    with pytest.raises(m.Refused, match="work_running"):
        jobs.ensure_quiet([{"root":str(api.store.project_root)}])
    api.store.immediate_write(lambda db: db.execute("UPDATE runtime_tasks SET state='COMPLETED'"))
    jobs.ensure_quiet([{"root":str(api.store.project_root)}])


def test_installer_uses_separate_transient_user_unit(isolated, monkeypatch, release):
    _, base, _, _ = release
    selected = jobs.check(isolated, url=base + "/latest", current="1.0.6", fixture=True)
    source = uuid.uuid4().hex
    path = jobs.folder(isolated, source)
    path.mkdir(parents=True)
    jobs.save(path, phase="starting")
    jobs.download(path, selected, fixture=True)
    calls = []
    original = subprocess.run
    def recorder(argv, **kwargs):
        if argv[0] == "/usr/bin/systemd-run":
            calls.append(argv)
            return subprocess.CompletedProcess(argv, 0)
        return original(argv, **kwargs)
    monkeypatch.setattr(subprocess, "run", recorder)
    jobs.start(isolated, {"id":uuid.uuid4().hex, "kind":"install", "source":source, "confirmed":True})
    assert len(calls) == 1
    assert calls[0][:4] == ["/usr/bin/systemd-run", "--user", "--collect", "--quiet"]
    assert any(a.startswith("--unit=cortex-workspace-update-") for a in calls[0])


def test_bundled_native_login_commands_exist_without_an_account(isolated, monkeypatch):
    from types import SimpleNamespace
    from cortex_runtime.engine.install_resolution import _build_codex_install, _build_claude_install
    tree = ROOT / ".workspace-build/engine-bundle/engines"
    if not tree.is_dir():
        pytest.skip("No locally retained bundled engine tree; no download is authorized")
    node = tree / "node/bin/node"
    for engine, builder, name in (
            ("codex-acp", _build_codex_install, "codex-acp"),
            ("claude-agent-acp", _build_claude_install, "claude-agent-acp")):
        package = tree / "node_modules/@agentclientprotocol" / name
        manifest = json.loads((package / "package.json").read_text())
        bin_entry = manifest["bin"][name] if isinstance(manifest["bin"], dict) else manifest["bin"]
        entry = package / bin_entry
        assert node.is_file() and entry.is_file()
        install = builder(node, entry)
        monkeypatch.setattr(connections, "installed", lambda e: (SimpleNamespace(launch_command=(str(node),str(entry))), install))
        command = connections.native_command(engine)
        done = subprocess.run([command["program"], *command["args"], "--help"], capture_output=True, text=True,
                              cwd=isolated.parent, timeout=8, env=dict(os.environ))
        assert done.returncode == 0
        status_args = ["login", "status", "--help"] if engine == "codex-acp" else ["auth", "status", "--help"]
        status_help = subprocess.run([command["program"], *status_args], capture_output=True, timeout=8, cwd=isolated.parent, env=dict(os.environ))
        assert status_help.returncode == 0
        assert ("Manage login" in done.stdout if engine == "codex-acp" else "Sign in to your Anthropic account" in done.stdout)
        assert not (Path(os.environ["HOME"]) / ".codex/auth.json").exists()
        assert not (Path(os.environ["HOME"]) / ".claude/.credentials.json").exists()
