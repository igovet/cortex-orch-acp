"""No-paid baseline measurements for orchestration hot paths.

Run with:
    PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q -s tests/test_orchestration_overhead.py

The tests deliberately use temporary project-local stores and in-process engine
fakes. Their loose assertions capture the current baseline while leaving room
for later optimization steps to tighten the expected counts and payload sizes.
"""
from __future__ import annotations

from contextlib import contextmanager
import json
import threading
import time
import tracemalloc
import uuid
from types import SimpleNamespace

import pytest

import cortex_runtime.delegation.revision as revision
import cortex_runtime.orchestration.sandbox as sandbox
from cortex_runtime.acp.wire_budget import SESSION_UPDATE_MAX_BYTES
from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime,
    PromptOutcome,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.controller import _verification, run as controller_run
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery import large_output_store
from cortex_runtime.store import Store
from orchestration_fakes import (
    FakeConfigSnapshotSource,
    FakeSessionRuntime,
    FakeWorkspace,
)


_OUTPUT_BYTES = 5 * 1024 * 1024


class _Measurements:
    def __init__(self):
        self.reset()

    def reset(self):
        self.connection_opens = 0
        self.context_builds = 0
        self.state_reads = 0
        self.begin_immediate = 0
        self.assignment_retried_reads = 0

    def trace_sql(self, sql):
        normalized = sql.lstrip().upper()
        if normalized.startswith("SELECT STATE FROM RUNTIME_TASKS"):
            self.state_reads += 1
        if normalized.startswith("BEGIN IMMEDIATE"):
            self.begin_immediate += 1
        if (normalized.startswith("SELECT")
                and "RUNTIME_EVENTS" in normalized
                and "ASSIGNMENT_RETRIED" in normalized):
            self.assignment_retried_reads += 1

    def snapshot(self):
        return {
            "connection_opens": self.connection_opens,
            "context_builds": self.context_builds,
            "state_reads": self.state_reads,
            "begin_immediate": self.begin_immediate,
            "assignment_retried_reads": self.assignment_retried_reads,
        }


def _instrument(monkeypatch):
    measurements = _Measurements()
    original_connection = Store.connection
    original_context = TaskMachine.context

    @contextmanager
    def measured_connection(store):
        measurements.connection_opens += 1
        with original_connection(store) as db:
            db.set_trace_callback(measurements.trace_sql)
            yield db

    def measured_context(machine, task_id):
        measurements.context_builds += 1
        return original_context(machine, task_id)

    monkeypatch.setattr(Store, "connection", measured_connection)
    monkeypatch.setattr(TaskMachine, "context", measured_context)
    return measurements


def _task(tmp_path, goal):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store,
        SourceEvent(origin="test", event_id=str(uuid.uuid4())),
        goal,
        "baseline-owner",
    )
    machine = TaskMachine(store)
    machine.create(task.task_id, goal)
    channel = SimpleNamespace(
        task_id=task.task_id,
        channel_id="baseline-coordinator",
        role="coordinator",
        principal_thread_id="baseline-owner",
        parent_principal_thread_id=None,
    )
    return store, task.task_id, machine, channel


def _legacy_context(machine, task_id):
    """Multi-read reference, including the current default wait/access fields."""
    from cortex_runtime.delegation.evidence import annotate_checks
    from cortex_runtime.orchestration.policy import denied_paths
    from cortex_runtime.orchestration.skill_proposals import task_proposals
    from cortex_runtime.orchestration.state import current_review_waiver

    with machine.store.connection() as db:
        task = db.execute(
            "SELECT * FROM runtime_tasks WHERE task_id=?", (task_id,),
        ).fetchone()
        nodes = db.execute(
            "SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
            (task_id, task["revision"]),
        ).fetchall()
        answers = db.execute(
            "SELECT id,text,answer_text FROM pending_questions "
            "WHERE task_id=? AND status='answered' ORDER BY created_at",
            (task_id,),
        ).fetchall()
        steering = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? "
            "AND type='USER_STEERING' ORDER BY id DESC LIMIT 64",
            (task_id,),
        ).fetchall()
        proposals = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? "
            "AND type='REPLAN_REQUEST_ACCEPTED' ORDER BY id DESC LIMIT 3",
            (task_id,),
        ).fetchall()
        verification = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? "
            "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
        denials = denied_paths(db, task_id)
        waiver = current_review_waiver(db, task_id)

    value = dict(task)
    value["review_not_required"] = waiver
    value["runtime_proposals"] = [json.loads(row[0]) for row in reversed(proposals)]
    value["steering"] = [json.loads(row[0]) for row in reversed(steering)]
    value["policy"] = dict(deny_write=denials, native_tools=True)
    value["verification"] = json.loads(verification[0]) if verification else None
    with machine.store.connection() as db:
        asked = db.execute(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? "
            "AND type='ASK_USER_REQUIRED' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
        answered = db.execute(
            "SELECT MAX(id) FROM runtime_events WHERE task_id=? "
            "AND type IN ('USER_ANSWER_RECEIVED','PLAN_ACCEPTED')",
            (task_id,),
        ).fetchone()[0]
    value["ask_user_required"] = (
        json.loads(asked[1])["blocker"]
        if asked and (answered is None or answered < asked[0])
        else None
    )
    with machine.store.connection() as db:
        value["verification_attempts"] = db.execute(
            "SELECT COUNT(*) FROM runtime_events WHERE task_id=? "
            "AND type='VERIFICATION_REVISED'",
            (task_id,),
        ).fetchone()[0]
    value["user_answers"] = [dict(answer) for answer in answers]
    with machine.store.connection() as db:
        value["skill_proposals"] = task_proposals(db, task_id)
    value["plan"] = json.loads(value.pop("plan_json") or "null")
    value["nodes"] = [
        {
            **dict(node),
            "dependencies": json.loads(node["dependencies_json"]),
            "scope": json.loads(node["scope_json"]),
            "result": json.loads(node["result_json"] or "null"),
            # This fixture has no wait/access events. The new public fields
            # must retain their defaults without altering the old projection.
            "wait": None,
            "access": {
                "declared_scope_revision": 0,
                "effective_declared_scope": json.loads(node["scope_json"]),
                "enforcement": "already_project_writable",
                "last_request": None,
            },
        }
        for node in nodes
    ]
    for node in value["nodes"]:
        for key in ("dependencies_json", "scope_json", "result_json"):
            node.pop(key)
    with machine.store.connection() as db:
        for node in value["nodes"]:
            result = node["result"]
            if isinstance(result, dict) and result.get("checks"):
                attempt = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning "
                    "WHERE task_id=? AND idempotency_key=?",
                    (task_id, node["assignment_ref"]),
                ).fetchone()
                result["checks"] = annotate_checks(
                    db, task_id, attempt["attempt_id"] if attempt else None,
                    result["checks"],
                )
    return value


def test_context_matches_legacy_projection_in_one_read_transaction(
    tmp_path, monkeypatch,
):
    store, task_id, machine, channel = _task(
        tmp_path, "Check the populated context projection.",
    )
    machine.submit_plan(channel, _plan(3), "context-projection-plan")
    with store.connection() as db:
        for index in range(3):
            assignment_ref = f"context-assignment-{index}"
            result = {
                "summary": f"result-{index}",
                "checks": [
                    {
                        "name": f"check-{index}",
                        "outcome": "passed",
                        "tool_call_ref": f"missing-command-{index}",
                    },
                    {"name": f"unrun-{index}", "outcome": "not_run"},
                ],
            }
            db.execute(
                "UPDATE runtime_nodes SET status='completed',assignment_ref=?,result_json=? "
                "WHERE task_id=? AND node_id=?",
                (
                    assignment_ref, json.dumps(result), task_id, f"N{index}",
                ),
            )
    expected = _legacy_context(machine, task_id)

    connections = 0
    statements = []
    original_connection = store.connection

    @contextmanager
    def measured_connection():
        nonlocal connections
        connections += 1
        with original_connection() as db:
            db.set_trace_callback(statements.append)
            yield db

    monkeypatch.setattr(store, "connection", measured_connection)
    actual = machine.context(task_id)

    assert actual == expected
    assert connections == 1
    normalized = [statement.lstrip().upper() for statement in statements]
    assert normalized.count("BEGIN") == 1
    assert not any(statement.startswith(("BEGIN IMMEDIATE", "INSERT", "UPDATE", "DELETE"))
                   for statement in normalized)
    assert sum(
        statement.startswith(
            "SELECT IDEMPOTENCY_KEY,ATTEMPT_ID FROM DELEGATION_PROVISIONING"
        )
        for statement in normalized
    ) == 1


def test_answer_ready_text_before_tool_update_stays_suppressed(
    tmp_path, monkeypatch,
):
    import cortex_runtime.orchestration.controller as controller_module

    store, task_id, _machine, _channel = _task(
        tmp_path, "Suppress streamed answer text after answer submission.",
    )
    measurements = _instrument(monkeypatch)
    answer_text = "This is the structured answer, shown once."
    callback_order = []
    phase = {"value": "setup"}
    get_state_calls = {"setup": 0, "callbacks": 0, "post_turn": 0}
    original_get_state = TaskMachine.get_state

    def tracked_get_state(machine, requested_task_id):
        get_state_calls[phase["value"]] += 1
        return original_get_state(machine, requested_task_id)

    monkeypatch.setattr(TaskMachine, "get_state", tracked_get_state)

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(store, channel, _session_ref, _message, _call_index,
                     *, on_delta=None, on_tool_call=None, cancel_event=None):
            # The MCP mutation commits before its later tool lifecycle notice.
            with store.connection() as db:
                db.execute(
                    "UPDATE runtime_tasks SET state='ANSWER_READY' WHERE task_id=?",
                    (channel.task_id,),
                )
            phase["value"] = "callbacks"
            contexts_before = measurements.context_builds
            state_reads_before = measurements.state_reads
            on_delta(answer_text)
            callback_order.append("delta")
            assert measurements.context_builds == contexts_before
            assert measurements.state_reads == state_reads_before + 1
            state_reads_before = measurements.state_reads
            on_tool_call()
            callback_order.append("tool")
            assert measurements.context_builds == contexts_before
            assert measurements.state_reads == state_reads_before + 1
            phase["value"] = "post_turn"
            cancel_event.set()
            return PromptOutcome(StopReason.CANCELLED, answer_text)
        return behavior

    engine = FakeSessionRuntime(store, router)
    engine._emit_outcome_text_as_delta = False
    runtime = OrchestratingRuntime(
        store,
        engine,
        FakeWorkspace(),
        FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True,
    )
    emitted = []
    monkeypatch.setattr(controller_module, "_stop_turn", lambda *_args: StopReason.CANCELLED)
    try:
        runtime._ensure_coordinator(task_id)
        result = runtime.start_prompt(
            task_id,
            "answer-ready-session",
            "Continue with the structured answer.",
            emitted.append,
            threading.Event(),
        )
        assert result == StopReason.CANCELLED
        assert callback_order == ["delta", "tool"]
        assert answer_text not in emitted
        # Both callbacks and the post-turn fallback use the lightweight method.
        assert get_state_calls["callbacks"] == 2
        assert get_state_calls["post_turn"] == 1
    finally:
        runtime.shutdown()


def _plan(count, *, shared_scope=False):
    scope = ["src/shared/**"] if shared_scope else None
    return {
        "tasks": [
            {
                "id": f"N{index}",
                "role": "backend-dev",
                "objective": f"Baseline worker {index}",
                "dependencies": [],
                "scope": scope or [f"src/part{index}/**"],
                "model": "auto",
            }
            for index in range(count)
        ],
        "verification": [["python3", "-B", "-m", "pytest", "-q"]],
    }


def _emit_metric(name, values):
    print("ORCHESTRATION_OVERHEAD_BASELINE=" + json.dumps(
        {"scenario": name, **values}, sort_keys=True,
    ))


def test_baseline_1000_coordinator_text_deltas(tmp_path, monkeypatch):
    store, task_id, _machine, _channel = _task(
        tmp_path, "Measure 1000 coordinator text deltas.",
    )
    measurements = _instrument(monkeypatch)
    emitted_deltas = 0

    def router(_store, channel, _session_ref, _launch, _instructions):
        def behavior(_store, channel, _session_ref, _message, _call_index,
                     *, on_delta=None, cancel_event=None):
            nonlocal emitted_deltas
            assert channel.role == "coordinator"
            for index in range(1000):
                assert on_delta is not None
                context_builds_before = measurements.context_builds
                state_reads_before = measurements.state_reads
                on_delta(f"delta-{index} ")
                assert measurements.context_builds == context_builds_before
                assert measurements.state_reads == state_reads_before + 1
                emitted_deltas += 1
            cancel_event.set()
            return PromptOutcome(StopReason.CANCELLED, "")
        return behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store,
        engine,
        FakeWorkspace(),
        FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True,
    )
    try:
        runtime._ensure_coordinator(task_id)
        measurements.reset()
        result = runtime.start_prompt(
            task_id,
            "baseline-session",
            "Measure 1000 coordinator text deltas.",
            lambda _text: True,
            threading.Event(),
            emit_tool_call=lambda _update: True,
        )
        assert result == StopReason.CANCELLED
        values = measurements.snapshot()
        values["deltas"] = emitted_deltas
        _emit_metric("coordinator_1000_deltas", values)

        assert emitted_deltas == 1000
        # No streamed text callback rebuilds the full task context.
        assert measurements.context_builds < emitted_deltas
        assert measurements.state_reads >= emitted_deltas
        assert measurements.connection_opens >= measurements.context_builds
    finally:
        runtime.shutdown()


class _IdleLoopRuntime:
    """Controller dependencies for an idle loop with one held fake worker."""

    def __init__(self, store, task_id):
        self._store = store
        self._engine = FakeSessionRuntime(store, lambda *_args: None)
        self._running = {task_id: {self._engine}}
        self._lock = threading.RLock()
        self._cond = threading.Condition(self._lock)
        self._terminate_completed_workers = lambda *_args, **_kwargs: None
        self._process_new_events_safely = lambda *_args, **_kwargs: None
        self._emit_assignment_tool_calls = lambda *_args, **_kwargs: None
        self._set_state = lambda *_args, **_kwargs: None


def _idle_task(tmp_path, monkeypatch, goal):
    """An EXECUTING task: N0 runs, N1 waits on N0's overlapping scope."""
    store, task_id, machine, channel = _task(tmp_path, goal)
    machine.submit_plan(channel, _plan(2, shared_scope=True), "idle-baseline-plan")
    machine.move(task_id, "EXECUTING")
    with store.connection() as db:
        db.execute(
            "UPDATE runtime_nodes SET status='running' "
            "WHERE task_id=? AND node_id='N0'",
            (task_id,),
        )
    # Pipeline publication is a one-time host binding operation; omit it to
    # keep this measurement focused on the steady idle scheduler loop.
    import cortex_runtime.orchestration.pipeline as pipeline
    monkeypatch.setattr(pipeline, "publish", lambda *_args, **_kwargs: None)
    return store, task_id, machine, channel


def _run_idle(runtime, task_id, channel, cancel_event, emitted=None):
    return controller_run(
        runtime,
        task_id,
        "baseline-session",
        channel,
        "baseline-coordinator-session",
        "Measure the idle execution polling loop.",
        (emitted.append if emitted is not None else (lambda _text: True)),
        cancel_event,
        "en",
        emit_tool_call=lambda _update: True,
    )


def _commit_elsewhere(store, sql, params):
    """Commit through a raw connection the controller knows nothing about."""
    import sqlite3

    db = sqlite3.connect(store.path, timeout=15, isolation_level=None)
    try:
        db.execute(sql, params)
    finally:
        db.close()


def test_baseline_idle_execution_loop_with_running_fake_worker(
    tmp_path, monkeypatch,
):
    import cortex_runtime.orchestration.controller as controller_module

    store, task_id, machine, channel = _idle_task(
        tmp_path, monkeypatch, "Measure the idle execution polling loop.",
    )
    # Four times more often than the default, so the window includes several
    # periodic full passes and proves they stay write-free as well.
    monkeypatch.setattr(controller_module, "_RECONCILE_SECONDS", 0.5)
    runtime = _IdleLoopRuntime(store, task_id)
    measurements = _instrument(monkeypatch)
    # Separate the idle window from turn setup and the pause on cancel.
    window = {}
    original_wait = controller_module._wait_for_change
    original_pause = controller_module._pause_turn

    def measured_wait(*args, **kwargs):
        window.setdefault("start", (time.perf_counter(), measurements.snapshot()))
        return original_wait(*args, **kwargs)

    def measured_pause(*args, **kwargs):
        window.setdefault("end", (time.perf_counter(), measurements.snapshot()))
        return original_pause(*args, **kwargs)

    monkeypatch.setattr(controller_module, "_wait_for_change", measured_wait)
    monkeypatch.setattr(controller_module, "_pause_turn", measured_pause)
    claims = []
    original_claim = TaskMachine.claim_ready
    monkeypatch.setattr(
        TaskMachine, "claim_ready",
        lambda machine, *args: claims.append(1) or original_claim(machine, *args),
    )
    cancel_event = threading.Event()
    timer = threading.Timer(2.0, cancel_event.set)
    started = time.perf_counter()
    timer.start()
    try:
        result = _run_idle(runtime, task_id, channel, cancel_event)
    finally:
        timer.cancel()
    elapsed = time.perf_counter() - started
    values = measurements.snapshot()
    values.update(elapsed_seconds=round(elapsed, 3), result=result.value)
    (window_start, before), (window_end, after) = window["start"], window["end"]
    idle = {key: after[key] - before[key] for key in before}
    values.update(
        idle_window_seconds=round(window_end - window_start, 3),
        idle_window={key: value for key, value in idle.items()},
        claim_ready_calls=len(claims),
    )
    _emit_metric("idle_execution_2s", values)

    assert result == StopReason.CANCELLED
    assert 1.75 <= elapsed < 5.0
    # The idle scheduler never takes the write lock: no reservation is
    # possible, so claim_ready stays a plain read, and nothing is claimed.
    assert idle["begin_immediate"] == 0
    assert idle["assignment_retried_reads"] == 0
    # Full passes happen only at the periodic reconciliation; each rebuilds
    # the context at most twice. Idle ticks cost at most two short reads.
    passes = int((window_end - window_start) / controller_module._RECONCILE_SECONDS) + 1
    assert len(claims) >= 3
    assert len(claims) <= passes + 1
    assert idle["context_builds"] <= 2 * passes
    ticks = (window_end - window_start) / controller_module._POLL_SECONDS
    assert idle["connection_opens"] <= 2 * ticks + 8 * passes


def test_idle_loop_sees_change_committed_elsewhere_via_fingerprint(
    tmp_path, monkeypatch,
):
    import cortex_runtime.orchestration.controller as controller_module

    store, task_id, _machine, channel = _idle_task(
        tmp_path, monkeypatch, "Pick up an external state change.",
    )
    # No periodic pass in this window: only the fingerprint can notice.
    monkeypatch.setattr(controller_module, "_RECONCILE_SECONDS", 60.0)
    runtime = _IdleLoopRuntime(store, task_id)
    cancel_event = threading.Event()
    guard = threading.Timer(5.0, cancel_event.set)
    change = threading.Timer(0.3, _commit_elsewhere, (
        store, "UPDATE runtime_tasks SET state='FAILED',blocker='external' WHERE task_id=?", (task_id,),
    ))
    emitted = []
    started = time.perf_counter()
    guard.start()
    change.start()
    try:
        result = _run_idle(runtime, task_id, channel, cancel_event, emitted)
    finally:
        guard.cancel()
        change.cancel()
    elapsed = time.perf_counter() - started

    assert not cancel_event.is_set()
    assert result == StopReason.END_TURN
    assert any("Runtime: FAILED" in text for text in emitted)
    assert elapsed < 1.5


def test_periodic_reconciliation_recovers_change_the_fingerprint_missed(
    tmp_path, monkeypatch,
):
    import cortex_runtime.orchestration.controller as controller_module

    store, task_id, _machine, channel = _idle_task(
        tmp_path, monkeypatch, "Recover a change nobody announced.",
    )
    # A fingerprint that never moves and no notification: only the
    # deadline-driven full pass can observe the committed change.
    monkeypatch.setattr(TaskMachine, "change_fingerprint", lambda *_args: ("frozen",))
    monkeypatch.setattr(controller_module, "_RECONCILE_SECONDS", 0.5)
    runtime = _IdleLoopRuntime(store, task_id)
    cancel_event = threading.Event()
    guard = threading.Timer(5.0, cancel_event.set)
    change = threading.Timer(0.2, _commit_elsewhere, (
        store, "UPDATE runtime_tasks SET state='FAILED',blocker='external' WHERE task_id=?", (task_id,),
    ))
    emitted = []
    started = time.perf_counter()
    guard.start()
    change.start()
    try:
        result = _run_idle(runtime, task_id, channel, cancel_event, emitted)
    finally:
        guard.cancel()
        change.cancel()
    elapsed = time.perf_counter() - started

    assert not cancel_event.is_set()
    assert result == StopReason.END_TURN
    assert any("Runtime: FAILED" in text for text in emitted)
    assert 0.45 <= elapsed < 2.0


def test_worker_notification_wakes_the_scheduler_early(tmp_path, monkeypatch):
    import cortex_runtime.orchestration.controller as controller_module

    store, task_id, _machine, channel = _idle_task(
        tmp_path, monkeypatch, "Wake on a worker notification.",
    )
    monkeypatch.setattr(TaskMachine, "change_fingerprint", lambda *_args: ("frozen",))
    monkeypatch.setattr(controller_module, "_RECONCILE_SECONDS", 60.0)
    runtime = _IdleLoopRuntime(store, task_id)
    claims = []
    original_claim = TaskMachine.claim_ready
    monkeypatch.setattr(
        TaskMachine, "claim_ready",
        lambda machine, *args: claims.append(time.perf_counter()) or original_claim(machine, *args),
    )

    # Two workers; one ends the way _on_worker_done reports it. The changed
    # worker set is seen even if the notify lands outside the wait.
    ending = object()
    runtime._running[task_id].add(ending)

    def notify():
        with runtime._cond:
            runtime._running[task_id].discard(ending)
            runtime._cond.notify_all()

    cancel_event = threading.Event()
    guard = threading.Timer(1.0, cancel_event.set)
    wake = threading.Timer(0.3, notify)
    started = time.perf_counter()
    guard.start()
    wake.start()
    try:
        _run_idle(runtime, task_id, channel, cancel_event)
    finally:
        guard.cancel()
        wake.cancel()

    assert len(claims) == 2
    assert 0.25 <= claims[1] - started < 0.6


def test_claim_ready_orders_by_longest_remaining_chain(tmp_path, monkeypatch):
    store, task_id, machine, channel = _task(
        tmp_path, "Start the node that unblocks the most work first.",
    )
    plan = {
        "tasks": [
            {"id": "A", "role": "backend-dev", "objective": "Leaf A",
             "dependencies": [], "scope": ["src/a/**"], "model": "auto"},
            {"id": "S1", "role": "backend-dev", "objective": "Short, shared scope",
             "dependencies": [], "scope": ["src/shared/**"], "model": "auto"},
            {"id": "B", "role": "backend-dev", "objective": "Head of a chain",
             "dependencies": [], "scope": ["src/b/**"], "model": "auto"},
            {"id": "S2", "role": "backend-dev", "objective": "Long, shared scope",
             "dependencies": [], "scope": ["src/shared/**"], "model": "auto"},
            {"id": "C", "role": "backend-dev", "objective": "Middle",
             "dependencies": ["B", "S2"], "scope": ["src/c/**"], "model": "auto"},
            {"id": "D", "role": "backend-dev", "objective": "Tail",
             "dependencies": ["C"], "scope": ["src/d/**"], "model": "auto"},
        ],
        "verification": [["python3", "-B", "-m", "pytest", "-q"]],
    }
    machine.submit_plan(channel, plan, "launch-order-plan")
    machine.move(task_id, "EXECUTING")
    with store.connection() as db:
        # Two earlier retries of B in this revision, one of an older revision.
        revision = db.execute(
            "SELECT revision FROM runtime_tasks WHERE task_id=?", (task_id,),
        ).fetchone()[0]
        for payload in ({"node": "B", "revision": revision},
                        {"node": "B", "revision": revision},
                        {"node": "A", "revision": revision - 1}):
            db.execute(
                "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                "VALUES(?,'ASSIGNMENT_RETRIED','runtime',?,'now')",
                (task_id, json.dumps(payload)),
            )
    measurements = _instrument(monkeypatch)
    claimed = machine.claim_ready(task_id, channel.channel_id)

    # Chain lengths B=3 and S2=3 (both unblock C then D), A=1 and S1=1; ties
    # keep plan order. S2 claims the shared scope, so S1 waits behind it.
    assert [node["node_id"] for node in claimed] == ["B", "S2", "A"]
    keys = {node["node_id"]: node["request_key"] for node in claimed}
    assert keys == {
        "B": f"runtime:{revision}:B:2",
        "S2": f"runtime:{revision}:S2:0",
        "A": f"runtime:{revision}:A:0",
    }
    import hashlib
    for node in claimed:
        assert node["assignment_ref"] == hashlib.sha256(
            f"{task_id}|{channel.channel_id}|{node['request_key']}".encode()
        ).hexdigest()
    assert measurements.begin_immediate == 1
    assert measurements.assignment_retried_reads == 1

    # Nothing more is reservable: the next pass is read-only.
    measurements.reset()
    assert machine.claim_ready(task_id, channel.channel_id) == []
    assert measurements.begin_immediate == 0
    assert measurements.assignment_retried_reads == 0


def test_change_fingerprint_tracks_scheduler_inputs(tmp_path):
    store, task_id, machine, channel = _task(
        tmp_path, "Fingerprint every scheduler input.",
    )
    machine.submit_plan(channel, _plan(2), "fingerprint-plan")
    machine.move(task_id, "EXECUTING")
    seen = [machine.change_fingerprint(task_id)]
    assert machine.change_fingerprint(task_id) == seen[-1]
    for sql, params in (
        ("UPDATE runtime_nodes SET status='running' WHERE task_id=? AND node_id='N0'", (task_id,)),
        ("UPDATE runtime_nodes SET result_json='{}' WHERE task_id=? AND node_id='N0'", (task_id,)),
        ("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
         "VALUES(?,'USER_STEERING','user','{}','now')", (task_id,)),
        ("UPDATE runtime_tasks SET synthesis='done' WHERE task_id=?", (task_id,)),
        ("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task_id,)),
    ):
        _commit_elsewhere(store, sql, params)
        seen.append(machine.change_fingerprint(task_id))
        assert seen[-1] != seen[-2], sql
    from cortex_runtime.delegation import events

    events.append_event(
        store, task_id, "progress", "fingerprint-progress", {"kind": "test"},
        origin="runtime",
    )
    assert machine.change_fingerprint(task_id) != seen[-1]


@pytest.mark.real_verification_sandbox
def test_verification_5mb_output_persistence_and_bounded_tool_card(
    tmp_path, monkeypatch,
):
    store, task_id, machine, channel = _task(
        tmp_path, "Measure verification output buffering.",
    )
    plan = _plan(1)
    plan["verification"] = [[
        "python3",
        "-c",
        "import sys;sys.stdout.buffer.write(b'x'*5242880)",
    ]]
    machine.submit_plan(channel, plan, "verification-baseline-plan")
    digest = "baseline-project-revision"
    with store.connection() as db:
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id=?",
            (task_id,),
        )
        db.execute(
            "UPDATE runtime_nodes SET stage='review',status='completed',result_json=? "
            "WHERE task_id=? AND node_id='N0'",
            (json.dumps({"project_digest": digest}), task_id),
        )

    measurements = _instrument(monkeypatch)
    monkeypatch.setattr(
        revision,
        "capture_revision",
        lambda _root: {"dirty_digest": digest},
    )
    monkeypatch.setattr(revision, "ignored_directories", lambda _root: [])

    @contextmanager
    def passthrough_gateway(_root, _scratch, argv, *_args, **_kwargs):
        yield list(argv), []

    # The verification seam's own profile (sandbox.readonly_command) and
    # the profiles it builds on are replaced by a passthrough launch.
    monkeypatch.setattr(sandbox, "readonly_command", passthrough_gateway)
    monkeypatch.setattr(sandbox, "gateway_command", passthrough_gateway)
    monkeypatch.setattr(sandbox, "with_umask", lambda argv: argv)
    monkeypatch.setattr(sandbox, "as_owner", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(
        sandbox, "readonly_test_cache", lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        sandbox, "compact_argv", lambda command, fds: (command, fds),
    )
    machine.record_verification = lambda *_args, **_kwargs: None

    persisted_sizes = []
    original_persist = large_output_store.persist_output_stream

    def measured_persist(*args, **kwargs):
        record = original_persist(*args, **kwargs)
        persisted_sizes.append(record["byte_count"])
        return record

    monkeypatch.setattr(large_output_store, "persist_output_stream", measured_persist)
    runtime = SimpleNamespace(
        _store=store,
        _task_workspace_root=lambda _task_id: str(tmp_path),
    )
    cards = []
    tracemalloc.start()
    try:
        _verification(
            runtime,
            task_id,
            machine,
            lambda update: cards.append(dict(update)) or True,
            threading.Event(),
        )
        _current, peak_bytes = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()

    completed = [card for card in cards if card.get("raw_output")]
    assert completed
    raw_output = completed[-1]["raw_output"]
    body_bytes = len(raw_output["output"].encode("utf-8"))
    raw_output_json_bytes = len(json.dumps(raw_output).encode("utf-8"))
    values = measurements.snapshot()
    values.update(
        expected_output_bytes=_OUTPUT_BYTES,
        persisted_output_bytes=persisted_sizes[-1],
        tool_card_body_bytes=body_bytes,
        raw_output_json_bytes=raw_output_json_bytes,
        tracemalloc_peak_bytes=peak_bytes,
    )
    _emit_metric("verification_5mb_output", values)

    assert persisted_sizes == [_OUTPUT_BYTES]
    assert raw_output["large_output_id"]
    assert body_bytes <= SESSION_UPDATE_MAX_BYTES
    assert raw_output["output"].endswith("x" * 128)
    assert raw_output_json_bytes <= SESSION_UPDATE_MAX_BYTES // 2
    # The output stays in the temporary file and is copied in chunks; no
    # Python allocation should need to hold the complete multi-megabyte body.
    assert peak_bytes < _OUTPUT_BYTES
