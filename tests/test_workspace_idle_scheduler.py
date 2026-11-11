"""The service's queues do nothing while nothing has moved."""
import contextlib
import json
import os
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))
sys.path.insert(0, str(ROOT / "src"))

import machine_tools as machine  # noqa: E402
import scheduler  # noqa: E402

PROJECT = "wp_idle"


class Store:
    def __init__(self, root):
        self.path = root / "cortex.sqlite3"
        self.project_root = str(root)
        with self.connection() as db:
            db.execute("CREATE TABLE workspace_projects(project_id TEXT, version INTEGER)")
            db.execute("CREATE TABLE workspace_outbox_state(project_id TEXT, high_watermark INTEGER)")
            db.execute("INSERT INTO workspace_projects VALUES (?,1)", (PROJECT,))
            db.execute("INSERT INTO workspace_outbox_state VALUES (?,1)", (PROJECT,))

    @contextlib.contextmanager
    def connection(self):
        db = sqlite3.connect(self.path)
        try:
            yield db
            db.commit()
        finally:
            db.close()


def task(task_id, phase="result"):
    return dict(taskId=task_id, phase=phase, executionId="ex_" + task_id, version="1")


class Api:
    project_id = PROJECT

    def __init__(self, root, tasks):
        self.store, self.tasks, self.calls = Store(root), tasks, []

    def handle(self, request):
        operation = request["operation"]
        self.calls.append(operation)
        if operation == "project.snapshot":
            return {"tasks": {"items": self.tasks, "nextCursor": None}}
        if operation == "executions.get":
            return {"resultId": "wr_" + request["target"]}
        assert operation == "results.get"
        identity = request["target"][len("wr_ex_"):]
        return dict(executionId="ex_" + identity, taskId=identity, version="1", checks=[], warnings=[])


def product(state):
    path = machine.product_path(state, PROJECT)
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def fixture(tmp_path, tasks, records):
    root, state = tmp_path / "project", tmp_path / "state"
    root.mkdir()
    state.mkdir()
    product(state).write_text(json.dumps({"tasks": records, "sprints": []}), encoding="utf-8")
    return Api(root, tasks), state


def test_nothing_moved_means_no_pass_and_no_database_read(tmp_path):
    api, state = fixture(tmp_path, [], {})
    gate = scheduler.Gate(api, state)
    assert gate.due()
    gate.done()
    calls = len(api.calls)
    connect = api.store.connection
    api.store.connection = lambda: (_ for _ in ()).throw(AssertionError("connected while nothing moved"))
    assert not gate.due()
    assert len(api.calls) == calls
    api.store.connection = connect


def test_any_real_change_reopens_the_gate(tmp_path):
    api, state = fixture(tmp_path, [], {})
    gate = scheduler.Gate(api, state)
    gate.due()
    gate.done()
    # A write elsewhere in the database moves the file but not this project's journal.
    with api.store.connection() as db:
        db.execute("CREATE TABLE elsewhere(x)")
    assert not gate.due()
    with api.store.connection() as db:
        db.execute("UPDATE workspace_outbox_state SET high_watermark=2")
    assert gate.due()
    gate.done()
    os.utime(product(state), ns=(1, 1))
    assert gate.due()
    gate.done()
    assert gate.due(busy=True)


def test_a_change_during_a_pass_is_not_lost(tmp_path):
    api, state = fixture(tmp_path, [], {})
    gate = scheduler.Gate(api, state)
    assert gate.due()
    # The world moves after the pass read it, before the pass is marked done.
    with api.store.connection() as db:
        db.execute("UPDATE workspace_outbox_state SET high_watermark=5")
    gate.done()
    assert gate.due()


def test_gate_still_opens_periodically_for_what_nothing_announces(tmp_path):
    api, state = fixture(tmp_path, [], {})
    clock = [0.0]
    gate = scheduler.Gate(api, state, clock=lambda: clock[0])
    gate.due()
    gate.done()
    clock[0] = scheduler.FULL_PASS_EVERY - 1
    assert not gate.due()
    clock[0] = scheduler.FULL_PASS_EVERY
    assert gate.due()


def test_a_closed_task_costs_nothing_and_an_unchanged_product_is_not_written(tmp_path):
    api, state = fixture(tmp_path, [task("done"), task("purged"), task("open", phase="running")],
                         {"done": {"status": "done"}, "purged": {"purged": True}})
    before = product(state).stat().st_mtime_ns
    os.utime(product(state), ns=(before - 10_000_000, before - 10_000_000))
    stamp = product(state).stat().st_mtime_ns
    scheduler.tick(api, state)
    assert api.calls == ["project.snapshot"]
    assert product(state).stat().st_mtime_ns == stamp


def test_an_unchanged_result_is_read_once(tmp_path):
    api, state = fixture(tmp_path, [task("ready")], {"ready": {"status": "ready_to_merge"}})
    scheduler.tick(api, state)
    assert api.calls.count("results.get") == 1
    api.calls.clear()
    scheduler.tick(api, state)
    assert api.calls == ["project.snapshot"]
    api.tasks[0] = dict(task("ready"), version="2")
    scheduler.tick(api, state)
    assert api.calls.count("results.get") == 1


def test_a_done_task_still_loses_its_automatic_ready_flag(tmp_path):
    api, state = fixture(tmp_path, [task("done")], {"done": {"status": "done", "automaticReady": True}})
    scheduler.tick(api, state)
    assert json.loads(product(state).read_text())["tasks"]["done"]["automaticReady"] is False
