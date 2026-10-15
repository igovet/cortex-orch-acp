"""A project scan a write transaction relies on runs before the store's
write lock (delegation.revision.observation_snapshot). Scanning under
``BEGIN IMMEDIATE`` held that lock for the whole scan, and another
runtime's writer failed with "database is locked" after the 15 s busy
timeout. Gateway file effects stay excluded by the project-change lock."""
from __future__ import annotations

import sqlite3
import threading
import time
from types import SimpleNamespace

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import revision
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _store(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement feature", "owner")
    return store, task.task_id


def _write_lock_free(store):
    db = sqlite3.connect(store.path, timeout=0, isolation_level=None)
    try:
        db.execute("BEGIN IMMEDIATE")
        db.execute("ROLLBACK")
        return True
    except sqlite3.OperationalError:
        return False
    finally:
        db.close()


def test_the_execution_baseline_scan_runs_without_the_write_lock(tmp_path, monkeypatch):
    store, task_id = _store(tmp_path)
    machine = TaskMachine(store)
    machine.create(task_id, "Implement feature")
    channel = SimpleNamespace(task_id=task_id, channel_id="coordinator", role="coordinator")
    machine.submit_plan(channel, dict(
        tasks=[dict(id="N0", role="backend-dev", objective="Implement it", dependencies=[], scope=["src/**"])],
        verification=[["python3", "-B", "-m", "pytest", "-q"]]), "p")
    real = revision._capture_revision_now
    seen = []

    def capture(root):
        seen.append(_write_lock_free(store))
        return real(root)

    monkeypatch.setattr(revision, "_capture_revision_now", capture)
    machine.move(task_id, "EXECUTING")
    assert seen == [True]
    assert machine.get_state(task_id) == "EXECUTING"


def test_one_scan_serves_every_capture_of_the_block(tmp_path, monkeypatch):
    store, _ = _store(tmp_path)
    calls = []
    monkeypatch.setattr(revision, "_capture_revision_now",
                        lambda root: calls.append(root) or dict(kind="observed", dirty_digest="d", head=None))
    with revision.observation_snapshot(store, tmp_path):
        first = revision.capture_revision(str(tmp_path))
        with revision.observation_snapshot(store, tmp_path):
            second = revision.capture_revision(tmp_path)
    assert first == second and len(calls) == 1
    # Outside the block, a capture observes again.
    revision.capture_revision(tmp_path)
    assert len(calls) == 2


def test_a_gateway_effect_waits_for_an_observation_and_writers_do_not(tmp_path, monkeypatch):
    store, _ = _store(tmp_path)
    scanning, release = threading.Event(), threading.Event()

    def slow(root):
        scanning.set()
        release.wait(5)
        return dict(kind="observed", dirty_digest="d", head=None)

    monkeypatch.setattr(revision, "_capture_revision_now", slow)
    holder = []

    def observe():
        with revision.observation_snapshot(store, tmp_path):
            holder.append("observed")

    observer = threading.Thread(target=observe)
    observer.start()
    assert scanning.wait(5)
    # Any other writer proceeds during the scan.
    assert _write_lock_free(store)
    acquired = []

    def effect():
        with revision.project_mutation_lock(store, exclusive=True):
            acquired.append(time.monotonic())

    writer = threading.Thread(target=effect)
    writer.start()
    time.sleep(0.3)
    assert acquired == []
    released_at = time.monotonic()
    release.set()
    observer.join(5)
    writer.join(5)
    assert holder == ["observed"] and acquired and acquired[0] >= released_at
