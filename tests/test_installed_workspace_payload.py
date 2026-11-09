"""The installed gate checks archive bytes and links even without Debian md5sums."""
import importlib.util
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]


def qualifier():
    spec = importlib.util.spec_from_file_location("installed_check", ROOT / "packaging/container/check_installed.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("text", [
    "corrupted double-linked list\n",
    "malloc(): unaligned tcache chunk detected\n",
    "corrupted size vs. prev_size\n",
    "double free or corruption (fasttop)\n",
    "free(): invalid pointer\n",
    "Segmentation fault (core dumped)\n",
])
def test_native_failure_blocks_an_otherwise_passing_installed_run(text):
    with pytest.raises(AssertionError, match="native process"):
        qualifier().require_native_health(text)


def test_expected_headless_warnings_are_not_native_crashes():
    qualifier().require_native_health("libEGL warning: failed to open a render device\n")


@pytest.mark.parametrize("missing", [None, "frontend-dev activity", "Find in this activity", "old-view"])
def test_installed_worker_requires_direct_full_stream(missing):
    class Input:
        def find(self, name, roles=None):
            if name == "Show full history":
                return missing == "old-view"
            return name != missing
    if missing is None:
        qualifier().require_full_worker(Input())
    else:
        with pytest.raises(AssertionError):
            qualifier().require_full_worker(Input())


def test_installed_worker_fixture_binds_to_the_selected_graph_attempt(tmp_path):
    import sqlite3

    project = tmp_path / "project"
    (project / ".cortex-dev").mkdir(parents=True)
    db = sqlite3.connect(project / ".cortex-dev/cortex.sqlite3")
    db.row_factory = sqlite3.Row
    db.executescript("""
        CREATE TABLE workspace_executions(task_id, execution_id, runtime_task_id);
        INSERT INTO workspace_executions VALUES('task', 'execution', 'runtime');
        CREATE TABLE work_assignments(assignment_id,task_id,node_key,requirement_bindings_json,status,requirement_state,idempotency_key,created_at);
        CREATE TABLE work_assignment_attempts(attempt_id,task_id,assignment_id,attempt_no,requirement_bindings_json,status,idempotency_key,created_at);
        CREATE TABLE task_events(id INTEGER PRIMARY KEY,task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at);
        CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id,type,actor,payload_json,created_at);
        CREATE TABLE delegation_provisioning(task_id,idempotency_key,attempt_id,channel_id,created_at);
    """)
    db.commit()
    qualifier().seed_worker(project)
    spec = importlib.util.spec_from_file_location("installed_worker_projection", ROOT / "crates/workspace-service/machine_tools.py")
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    worker, = helper._workers_of_db(db, "project", ["task"])
    assert (worker["id"], worker["role"], worker["goal"]) == ("a-input", "frontend-dev", "Input fixture worker")
    assert worker["attempt"] == dict(n=1, of=1)
    db.close()


@pytest.fixture
def payload(tmp_path):
    stage = tmp_path / "stage"
    control = stage / "DEBIAN"
    control.mkdir(parents=True)
    (control / "control").write_text("Package: acceptance-fixture\nVersion: 1.0.6\nArchitecture: all\nMaintainer: Cortex\nDescription: disposable fixture\n")
    data = stage / "usr/lib/acceptance-fixture"
    data.mkdir(parents=True)
    (data / "file").write_text("archive bytes")
    (data / "link").symlink_to("file")
    archive = tmp_path / "fixture.deb"
    subprocess.run(["dpkg-deb", "--build", str(stage), str(archive)], check=True, capture_output=True)
    installed = tmp_path / "installed"
    subprocess.run(["dpkg-deb", "--extract", str(archive), str(installed)], check=True)
    return archive, installed


@pytest.mark.parametrize("mode", ["inside", "session"])
def test_internal_modes_refuse_non_container_before_native_imports(monkeypatch, mode):
    module = qualifier()
    monkeypatch.setattr(module.Path, "is_file", lambda self: False)
    with pytest.raises(RuntimeError, match="requires a disposable systemd container"):
        getattr(module, mode)("x11", "clean")


def test_exact_archive_payload_without_md5sums(payload):
    archive, installed = payload
    assert qualifier().verify_payload(archive, installed) == dict(files=1, links=1, mismatches=[])


@pytest.mark.parametrize("fault", ["changed_bytes", "missing_file", "wrong_link"])
def test_rejects_installed_payload_drift(payload, fault):
    archive, installed = payload
    data = installed / "usr/lib/acceptance-fixture"
    if fault == "changed_bytes":
        (data / "file").write_text("tampered")
    elif fault == "missing_file":
        (data / "file").unlink()
    else:
        (data / "link").unlink()
        (data / "link").symlink_to("other")
    with pytest.raises(AssertionError, match="installed payload differs"):
        qualifier().verify_payload(archive, installed)
