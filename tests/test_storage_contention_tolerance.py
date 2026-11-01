"""A contended SQLite write lock is transient, never a coordinator crash.

Live incident: the scheduler's ``events.claim_unconsumed`` hit ``database is
locked`` (another writer held the lock for seconds), the exception unwound the
whole prompt turn, the teardown killed four live workers, and each was then
recorded as ``engine_failure`` + ``fallback_refused:no_untried_authorized_route``.
"""
from __future__ import annotations

from contextlib import contextmanager
import json
import sqlite3
import threading
import time
from pathlib import Path

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events, mcp_tools
from cortex_runtime.orchestration import controller
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store

from test_orchestrating_runtime import PromptOutcome, _assignment_title
from test_orchestrating_runtime_routing_fallback import (
    _bootstrap, _make_runtime, _publish, _setup_two_route_task,
)


class _LockedBeginStore(Store):
    """A real store whose next ``BEGIN IMMEDIATE`` calls fail as contended."""

    locked_begins = 0

    @contextmanager
    def connection(self, **kwargs):
        with super().connection(**kwargs) as db:
            yield _LockedBeginConnection(db, self)


class _LockedBeginConnection:
    def __init__(self, db, store):
        self._db, self._store = db, store

    def execute(self, sql, *args):
        if sql.strip().upper() == "BEGIN IMMEDIATE" and self._store.locked_begins > 0:
            self._store.locked_begins -= 1
            raise sqlite3.OperationalError("database is locked")
        return self._db.execute(sql, *args)

    def __getattr__(self, name):
        return getattr(self._db, name)


def _contended_store(tmp_path):
    _store, task_id, _epoch = _bootstrap(tmp_path)
    return _LockedBeginStore(tmp_path / ".cortex", project_root=tmp_path), task_id


def test_claim_unconsumed_retries_a_locked_begin_and_returns_the_batch(tmp_path):
    store, task_id = _contended_store(tmp_path)
    delegation_events.append_event(
        store, task_id, "progress", "contention:1", dict(kind="note"), origin="runtime")
    store.locked_begins = 1
    batch = delegation_events.claim_unconsumed(store, task_id, "claim-1")
    assert store.locked_begins == 0  # the contended BEGIN really happened
    assert "contention:1" in [event.dedup_key for event in batch]
    assert all(event.consumed_by == "claim-1" for event in batch)


def test_lifecycle_writes_retry_a_locked_begin(tmp_path):
    store, task_id = _contended_store(tmp_path)
    store.locked_begins = 1
    appended = delegation_events.append_event(
        store, task_id, "progress", "contention:2", dict(kind="note"), origin="runtime")
    assert store.locked_begins == 0 and appended.dedup_key == "contention:2"
    delegation_events.claim_unconsumed(store, task_id, "claim-2")
    store.locked_begins = 1
    assert delegation_events.mark_delivered(store, task_id, "claim-2") >= 1
    assert store.locked_begins == 0
    store.locked_begins = 1
    assert delegation_events.release_claim(store, task_id, "claim-2") >= 1
    assert store.locked_begins == 0


def test_return_to_coordinator_retries_a_locked_begin(tmp_path):
    store, task_id = _contended_store(tmp_path)
    machine = TaskMachine(store)
    machine.create(task_id, "Build.")
    store.locked_begins = 1
    assert machine.return_to_coordinator(task_id, "worker_failed") is True
    assert store.locked_begins == 0
    assert machine.get_state(task_id) == "RECOVERING"


def _coordinator(store, channel, session_ref, message, call_index):
    context = mcp_tools.dispatch(store, channel, "task_get_context", {})
    if context["state"] == "PLANNING":
        mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(
            tasks=[dict(id="A", role="backend-dev", objective="A", dependencies=[], scope=["."], model="auto")],
            verification=[["true"]]), request_key="plan"))
    else:
        mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Reviewed and verified."))
    return PromptOutcome(StopReason.END_TURN, "Visible coordinator text")


def _reviewer(store, channel, session_ref, message, call_index):
    _publish(store, channel, _assignment_title(store, channel), call_index)
    return PromptOutcome(StopReason.END_TURN, "")


def _failure_events(store, task_id):
    with store.connection() as db:
        rows = db.execute(
            "SELECT event_type, payload_json FROM task_events WHERE task_id=?", (task_id,)).fetchall()
    return [row for row in rows
            if row["event_type"] == "assignment_failed" or "engine_failure" in row["payload_json"]]


def test_transient_lock_in_a_scheduler_pass_keeps_running_workers(tmp_path):
    store, task_id, connections, _entry_a, _entry_b = _setup_two_route_task(tmp_path)
    started, released = threading.Event(), threading.Event()
    seen = {}

    def worker(store, channel, session_ref, message, call_index):
        started.set()
        assert released.wait(20)
        seen["alive_after_contention"] = engine.lookup_owned(channel.attempt_id)
        (Path(store.project_root) / "a.py").write_text("A = True\n", encoding="utf-8")
        _publish(store, channel, _assignment_title(store, channel), call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return _coordinator
        return _reviewer if launch.get("role_id") == "worker-code-reviewer" else worker

    runtime, engine = _make_runtime(store, router, connections)
    terminate_completed = runtime._terminate_completed_workers
    injected = []

    def contended_pass(task):
        if started.is_set():
            if not injected:
                injected.append(True)
                raise sqlite3.OperationalError("database is locked")
            released.set()
        return terminate_completed(task)

    runtime._terminate_completed_workers = contended_pass
    try:
        result = runtime.start_prompt(task_id, "s1", "Build A.", lambda _text: True, threading.Event())
    finally:
        released.set()
        runtime.shutdown()
    assert injected  # the scheduler pass really hit the lock
    assert result == StopReason.END_TURN
    assert seen["alive_after_contention"] == "alive"
    assert engine.fallback_calls == []
    assert _failure_events(store, task_id) == []
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"


def test_coordinator_crash_teardown_is_not_a_worker_engine_failure(tmp_path):
    store, task_id, connections, _entry_a, _entry_b = _setup_two_route_task(tmp_path)
    started, killed = threading.Event(), threading.Event()
    attempts = {}

    def worker(store, channel, session_ref, message, call_index):
        attempts["worker"] = channel.attempt_id
        started.set()
        assert killed.wait(20)
        # The teardown killed the engine mid-turn, as a real interrupted turn.
        raise EngineTurnFailed("transport_eof")

    def fallback_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return _coordinator
        return fallback_worker if launch.get("route_override") is not None else worker

    runtime, engine = _make_runtime(store, router, connections)
    terminate_owned = engine.terminate_owned

    def terminate(attempt_id):
        terminate_owned(attempt_id)
        if attempt_id == attempts.get("worker"):
            killed.set()

    engine.terminate_owned = terminate
    terminate_completed = runtime._terminate_completed_workers

    def faulty_pass(task):
        if started.is_set():
            raise RuntimeError("injected controller fault")
        return terminate_completed(task)

    runtime._terminate_completed_workers = faulty_pass
    try:
        with pytest.raises(RuntimeError, match="injected controller fault"):
            runtime.start_prompt(task_id, "s1", "Build A.", lambda _text: True, threading.Event())
        assert killed.wait(20)  # the crash teardown did interrupt the live worker
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            with runtime._lock:
                if not runtime._running.get(task_id):
                    break
            time.sleep(0.05)
        with runtime._lock:
            assert not runtime._running.get(task_id)
        assert runtime.transport_lost(task_id)  # left as lost-host work for recovery
    finally:
        killed.set()
        runtime.shutdown()
    assert engine.fallback_calls == []
    assert _failure_events(store, task_id) == []
    with store.connection() as db:
        statuses = [row["status"] for row in db.execute(
            "SELECT status FROM runtime_nodes WHERE task_id=? AND node_id='A'", (task_id,))]
    assert statuses and "failed" not in statuses


def test_in_place_bookkeeping_retry_is_bounded_and_ends_the_turn(monkeypatch):
    """Contention outlasting the allowance propagates, and the spent budget
    stops ``run`` from re-entering (and so re-prompting) the turn."""
    monkeypatch.setattr(controller, "_STORAGE_BUSY_FIRST_DELAY", 0.01)
    monkeypatch.setattr(controller, "_STORAGE_BUSY_MAX_DELAY", 0.01)
    monkeypatch.setattr(controller, "_STORAGE_BUSY_BUDGET_SECONDS", 0.05)
    monkeypatch.setattr(controller, "_transport_lost", lambda _runtime, _task_id: False)
    contention = controller._StorageContention(None, "t", threading.Event())
    calls = []

    def locked():
        calls.append(True)
        raise sqlite3.OperationalError("database is locked")

    with pytest.raises(sqlite3.OperationalError):
        contention.retry(locked)
    assert len(calls) > 1 and contention.exhausted
    assert not contention.backoff(sqlite3.OperationalError("database is locked"), progressed=True)

    fresh = controller._StorageContention(None, "t", threading.Event())
    calls.clear()

    def broken():
        calls.append(True)
        raise sqlite3.OperationalError("no such table: runtime_events")

    with pytest.raises(sqlite3.OperationalError, match="no such table"):
        fresh.retry(broken)
    assert len(calls) == 1  # not contention: never retried


@pytest.mark.parametrize("contention, failures", [
    # Eleven: exhausts Store.immediate_write's eight attempts, then fails three
    # more after the controller's backoff before storage recovers.
    (lambda: sqlite3.OperationalError("database is locked"), 11),
    # Never retried by immediate_write itself: each failure leaves the write.
    (lambda: StoreError("storage_busy"), 3),
], ids=["database_is_locked", "storage_busy"])
def test_contended_turn_end_after_a_completed_coordinator_turn_is_not_a_failure(
        tmp_path, monkeypatch, contention, failures):
    """Recording a normally returned coordinator turn's end is bookkeeping.

    Its contention once took the engine-failure path: the claim was released,
    a failed turn recorded and the task moved to BLOCKED
    (``coordinator_prompt_failed:OperationalError``), whose stop handling
    cancels the plan's live work."""
    store, task_id, connections, _entry_a, _entry_b = _setup_two_route_task(tmp_path)
    monkeypatch.setattr(controller, "_STORAGE_BUSY_FIRST_DELAY", 0.01)
    record_event = controller.event
    remaining = [failures]

    def contended_event(db, task, event_type, payload, *args, **kwargs):
        if event_type == "COORDINATOR_TURN_ENDED" and "stop_reason" in payload and remaining[0] > 0:
            remaining[0] -= 1
            raise contention()
        return record_event(db, task, event_type, payload, *args, **kwargs)

    monkeypatch.setattr(controller, "event", contended_event)
    moves = []
    move = TaskMachine.move

    def recording_move(self, task, target, **kwargs):
        moves.append((target, kwargs.get("blocker")))
        return move(self, task, target, **kwargs)

    monkeypatch.setattr(TaskMachine, "move", recording_move)
    prompted_states = []

    def coordinator(store, channel, session_ref, message, call_index):
        prompted_states.append(mcp_tools.dispatch(store, channel, "task_get_context", {})["state"])
        return _coordinator(store, channel, session_ref, message, call_index)

    def worker(store, channel, session_ref, message, call_index):
        (Path(store.project_root) / "a.py").write_text("A = True\n", encoding="utf-8")
        _publish(store, channel, _assignment_title(store, channel), call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return coordinator
        return _reviewer if launch.get("role_id") == "worker-code-reviewer" else worker

    runtime, engine = _make_runtime(store, router, connections)
    try:
        result = runtime.start_prompt(task_id, "s1", "Build A.", lambda _text: True, threading.Event())
    finally:
        runtime.shutdown()
    assert [(target, blocker) for target, blocker in moves if target in {"BLOCKED", "RECOVERING"}] == []
    assert remaining == [0]  # every injected turn-end failure really happened
    assert result == StopReason.END_TURN
    assert prompted_states.count("PLANNING") == 1  # the completed turn was never re-prompted
    assert engine.fallback_calls == []
    assert _failure_events(store, task_id) == []
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
        statuses = [row["status"] for row in db.execute(
            "SELECT status FROM runtime_nodes WHERE task_id=?", (task_id,))]
        started = db.execute(
            "SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='COORDINATOR_TURN_STARTED'",
            (task_id,)).fetchone()[0]
        ended = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COORDINATOR_TURN_ENDED' "
            "ORDER BY id", (task_id,))]
    assert state == "accepted"
    assert statuses and not {"cancelled", "failed"} & set(statuses)
    # Each turn, the contended one included, ends exactly once and as returned.
    assert started == len(ended) == len(prompted_states)
    assert all(payload.get("stop_reason") == "end_turn" and "failure_kind" not in payload for payload in ended)
