"""T017: completion gating, turn-outcome handling (refusal/limit/engine
failure), durable waiting/no-progress persistence, and crash-recovery
reconciliation for OrchestratingRuntime -- fakes only, no real engine."""
from __future__ import annotations

import json
import threading
import time
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed, OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import completion as delegation_completion
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


# Load-test determinism fix: every OrchestratingRuntime created in this file
# owns its own ThreadPoolExecutor (worker dispatch); by default it is never
# explicitly shut down at the end of a test, so a still-alive background
# thread from one test can add scheduling pressure while a *later* test in
# the same process opens its own, unrelated Store -- under heavy load this
# was observed to trip an unrelated store.connection() file-integrity check
# (WAL/-shm sidecar mid-recreation) in a later test, not a capacity-queue
# race itself. Registering every created runtime here and shutting its
# executor down (non-blocking; every capacity test already releases its own
# hold_* events before returning) after each test closes that gap.
_created_runtimes: list = []


@pytest.fixture(autouse=True)
def _shutdown_created_runtimes():
    yield
    for runtime in _created_runtimes:
        try:
            runtime._executor.shutdown(wait=False, cancel_futures=True)
        except Exception:
            pass
    _created_runtimes.clear()


def _make_capacity_runtime(store, router, *, connection_id, capacity_limit, **kwargs):
    engine = FakeSessionRuntime(store, router, connection_id=connection_id, capacity_limit=capacity_limit)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 0.1),
        **kwargs,
    )
    _created_runtimes.append(runtime)
    return runtime, engine


def _make_runtime(store, router, **kwargs):
    engine = FakeSessionRuntime(store, router)
    # T018 fresh-review fix: see the identical rationale in
    # test_orchestrating_runtime.py's _make_runtime -- this default is only
    # used by tests expecting a background-executor *success* path; every
    # test deliberately exercising the short-timeout 'blocked' path passes
    # its own explicit wait_timeout_seconds. A short wall-clock default
    # here is a real-thread-scheduling race under CPU contention, not a
    # deterministic bound.
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    _created_runtimes.append(runtime)
    return runtime, engine


def _publish(store, channel, label, call_index):
    report_id = "r_" + uuid.uuid4().hex[:12]
    filename = report_id + ".md"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        (task_dir / filename).write_text(f"# {label}\n", encoding="utf-8")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", filename, store._now()))
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}-{call_index}"))
    return report_id


# -- completion gating -----------------------------------------------------

def test_task_level_accept_refused_with_a_still_running_assignment(tmp_path):
    store, task_id = _task(tmp_path)
    ok, reasons = delegation_completion.gate(store, task_id)
    assert ok and not reasons  # nothing delegated yet: vacuously satisfied

    # Simulate a coordinator channel + a delegated, still-running assignment.
    from cortex_runtime.trusted_channel import issue_channel_tx

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        db.commit()
    from orchestration_fakes import channel_context_by_id

    channel = channel_context_by_id(store, channel_id)
    mcp_tools.dispatch(store, channel, "delegate", dict(
        title="Assignment A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA",
    ))
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="nothing", request_key="acc1",
        ))
    assert str(excinfo.value) == "acceptance_refused"
    assert "not terminal" in str(excinfo.value.correction)


def test_task_level_accept_refused_when_report_file_is_missing(tmp_path):
    store, task_id = _task(tmp_path)
    from cortex_runtime.trusted_channel import issue_channel_tx

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        db.commit()
    from orchestration_fakes import channel_context_by_id

    channel = channel_context_by_id(store, channel_id)
    delegate_result = mcp_tools.dispatch(store, channel, "delegate", dict(
        title="Assignment A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA",
    ))
    assignment_ref = delegate_result["assignment_ref"]
    attempt_id = "xa_" + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, "
            "workspace_binding_id, channel_id, config_revision, context_snapshot_ref, engine_session_ref, "
            "instruction_delivery, error_code, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, None, attempt_id, "backend-dev", "ready", "ready",
             None, None, None, None, None, None, None, channel.owner_epoch, store._now(), store._now()),
        )
        db.commit()

    # Publish a "report" row whose file was never actually written --
    # a claimed success with no stored evidence.
    report_id = "r_" + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, task_id, "report", f"{report_id}.md", store._now()))
        db.commit()
    from cortex_runtime.trusted_channel import ChannelContext

    worker_channel = ChannelContext(
        channel_id="synthetic-worker", task_id=task_id, principal_thread_id=channel.principal_thread_id,
        parent_principal_thread_id=None, role="worker", owner_epoch=channel.owner_epoch,
        assignment_id=None, attempt_id=attempt_id, workspace_binding_id=None, context_generation_id=None,
    )
    mcp_tools.dispatch(store, worker_channel, "publish_result", dict(report_ref=report_id, summary="done", request_key="pub1"))

    ok, reasons = delegation_completion.gate(store, task_id)
    assert not ok
    assert any("does not resolve to a stored report file" in reason for reason in reasons)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="nothing", request_key="acc1",
        ))
    assert str(excinfo.value) == "acceptance_refused"


# -- refusal / cancellation / engine failures -------------------------------

def test_coordinator_refusal_returns_stop_reason_refusal_and_opens_only_one_session(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.REFUSAL, "I refuse to do this.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Do something.", lambda t: True, threading.Event())
    assert result == StopReason.REFUSAL
    assert len(engine.sessions) == 1  # never re-routed to a second engine/session
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "refused"
    assert row["wait_reason"] == "refused"


def test_worker_refusal_records_assignment_failed_refusal_without_reroute(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.REFUSAL, "I refuse.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router, max_no_progress_continuations=1)
    runtime.start_prompt(task_id, "s1", "Do A.", lambda t: True, threading.Event())

    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,),
        ).fetchall()
    assert len(rows) == 1
    assert json.loads(rows[0]["payload_json"])["kind"] == "refusal"
    # Exactly one worker session ever opened for this assignment (no reroute).
    worker_opens = [s for s in engine.sessions.values() if s["channel"] and s["channel"].role == "worker"]
    assert len(worker_opens) == 1


def test_coordinator_engine_failure_blocks_without_silent_end_turn(tmp_path):
    store, task_id = _task(tmp_path)
    calls = {"n": 0}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        calls["n"] += 1
        raise EngineTurnFailed("transport_eof")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    assert calls["n"] == 1  # never silently retried into a fabricated success
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
        user_message = db.execute(
            "SELECT consumed_by FROM task_events WHERE task_id=? AND event_type='user_message'", (task_id,),
        ).fetchone()
    assert row["state"] == "blocked"
    assert row["wait_reason"] == "engine_failure:transport_eof"
    assert user_message["consumed_by"] is None  # released, not lost


def test_worker_engine_failure_records_assignment_failed_with_kind(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("timeout_alive")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router, max_no_progress_continuations=1)
    runtime.start_prompt(task_id, "s1", "Do A.", lambda t: True, threading.Event())

    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,),
        ).fetchall()
    assert len(rows) == 1
    payload = json.loads(rows[0]["payload_json"])
    assert payload["kind"] == "timeout_alive"


def test_coordinator_max_tokens_with_no_unfinished_action_blocks_not_success(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.MAX_TOKENS, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "blocked"
    assert row["wait_reason"] == "limit:max_tokens"


# -- durable waiting / no-progress / resumability ---------------------------

def test_blocked_state_is_resumable_with_explanation_and_resets_no_progress(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX"))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        if "Resuming" in message:
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="ask_user", rationale="stuck", question="How do I proceed?", request_key="ask1"))
            return PromptOutcome(StopReason.END_TURN, "")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.1, max_no_progress_continuations=2)
    result1 = runtime.start_prompt(task_id, "s1", "Do X.", lambda t: True, threading.Event())
    assert result1 == StopReason.END_TURN
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason, wait_detail_json, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "blocked"
    assert row["wait_reason"] == "blocked"
    assert row["no_progress_count"] >= 2
    assert "no new worker result" in json.loads(row["wait_detail_json"])["explanation"]

    updates2 = []
    result2 = runtime.start_prompt(task_id, "s1", "Still there?", updates2.append, threading.Event())
    assert result2 == StopReason.END_TURN
    with store.connection() as db:
        row2 = db.execute("SELECT state, wait_reason, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row2["state"] == "waiting_for_user"
    # The resume explanation was durably recorded as a progress event.
    with store.connection() as db:
        resumed = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'resume_context:%'", (task_id,),
        ).fetchone()
    assert resumed is not None


def test_idle_wait_emits_no_updates_and_does_not_reprompt_a_running_worker(tmp_path):
    release = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Y", brief="do Y", role="backend-dev", acceptance="Y done", request_key="reqY"))
            return PromptOutcome(StopReason.END_TURN, "Delegated Y.")
        mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="ok", request_key="accY"))
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def slow_worker(store, channel, session_ref, message, call_index):
        release.wait(timeout=5)
        report_id = "r_" + uuid.uuid4().hex[:12]
        with store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            task_dir = store._task_directory(db, channel.task_id)
            (task_dir / f"{report_id}.md").write_text("# Y\n", encoding="utf-8")
            db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", f"{report_id}.md", store._now()))
            db.commit()
        mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary="Y finished", request_key="pubY"))
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else slow_worker

    def store_factory(tmp_path):
        return Store(tmp_path / ".cortex", project_root=tmp_path)

    import tempfile
    from pathlib import Path

    tmp_dir = Path(tempfile.mkdtemp())
    tmp_dir.chmod(0o700)
    store, task_id = _task(tmp_dir)
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.15, max_no_progress_continuations=50)

    updates = []
    thread = threading.Thread(target=lambda: updates.append(runtime.start_prompt(task_id, "s1", "Do Y.", updates.append, threading.Event())))
    thread.start()
    import time

    time.sleep(0.6)  # several wait-loop backoff ticks while the worker is "running"
    coordinator_session_ref = None
    for ref, info in engine.sessions.items():
        if info["channel"] and info["channel"].role == "coordinator":
            coordinator_session_ref = ref
    coordinator_calls_before = sum(1 for ref in engine.prompt_calls if ref == coordinator_session_ref)
    release.set()
    thread.join(timeout=5)
    coordinator_calls_after = sum(1 for ref in engine.prompt_calls if ref == coordinator_session_ref)
    # No coordinator prompt happened purely from idle DB-polling wait ticks;
    # the count only grows once the worker's result actually arrives.
    assert coordinator_calls_after - coordinator_calls_before <= 1
    assert not any("still running" in str(u).lower() for u in updates if isinstance(u, str))


# -- crash between claim and delivery ---------------------------------------

# -- capacity queuing through OrchestratingRuntime (lead follow-up) --------

def test_capacity_full_queues_second_delegation_until_confirmed_release(tmp_path):
    store, task_id = _task(tmp_path)
    seen, accepted = set(), {"done": False}
    hold_first, first_opened = threading.Event(), threading.Event()
    lock = threading.Lock()
    first_title = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        for name in ("A finished", "B finished"):
            if name in message:
                seen.add(name)
        if len(seen) >= 2 and not accepted["done"]:
            accepted["done"] = True
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="both done", request_key="acc1"))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = "A" if "do A" in message else "B"
        # Symmetric: whichever of A/B's open() wins the single free worker
        # slot first (coordinator permanently holds the other of the 2)
        # reaches here first and is held deterministically; never assume it
        # is specifically "A" -- both delegations are dispatched concurrently.
        with lock:
            is_first = not first_opened.is_set()
            if is_first:
                first_title["title"] = title
                first_opened.set()
        if is_first:
            hold_first.wait(timeout=30)
        _publish(store, channel, f"{title} finished", call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    # limit=2: the coordinator's own session permanently holds one slot, so
    # only ONE worker can be open at a time -- the second delegation must queue.
    runtime, engine = _make_capacity_runtime(
        store, router, connection_id="cn_shared", capacity_limit=2, wait_timeout_seconds=0.1,
        max_no_progress_continuations=50,
    )
    results = []
    thread = threading.Thread(target=lambda: results.append(runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, threading.Event())))
    thread.start()
    try:
        assert first_opened.wait(timeout=30)  # the first worker has opened and holds the one free reservation
        # Deterministic sync point (not a fixed sleep): the second worker's
        # own queued-open loop always records this event on its first failed
        # reserve attempt, strictly before it could ever succeed (capacity
        # is still held by the first, deliberately, via hold_first).
        assert engine.wait_until(lambda: _fetch_one(
            store, "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ) is not None, timeout=30)
        worker_opens_before = [a for a in engine.open_calls if a and a.startswith("xa_")]
        assert len(worker_opens_before) == 1  # the second is queued, never opened while the first holds capacity
        hold_first.set()  # let it publish -> coordinator sees it -> terminates it -> releases capacity -> the other proceeds
    finally:
        thread.join(timeout=30)
    assert results and results[0] == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    worker_opens_after = [a for a in engine.open_calls if a and a.startswith("xa_")]
    assert len(worker_opens_after) == 2  # both workers eventually opened
    with store.connection() as db:
        rows = db.execute("SELECT event_type FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,)).fetchall()
    assert rows == []  # capacity-full never spammed a failure


def test_ambiguous_release_keeps_second_delegation_queued(tmp_path):
    from cortex_runtime.delegation import capacity as delegation_capacity

    store, task_id = _task(tmp_path)
    hold_first, hold_second, first_opened = threading.Event(), threading.Event(), threading.Event()
    lock = threading.Lock()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        # Symmetric: whichever of A/B's open() wins the free capacity slot
        # first blocks on hold_first; the other necessarily queues inside
        # open() and only reaches this function once released.
        with lock:
            is_first = not first_opened.is_set()
            first_opened.set()
        if is_first:
            hold_first.wait(timeout=30)
        else:
            hold_second.wait(timeout=30)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_capacity_runtime(store, router, connection_id="cn_ambiguous", capacity_limit=2, wait_timeout_seconds=0.1, max_no_progress_continuations=5)
    thread = threading.Thread(target=runtime.start_prompt, args=(task_id, "s1", "Do A and B.", lambda t: True, threading.Event()))
    thread.start()
    try:
        assert first_opened.wait(timeout=30)
        # Deterministic: the second worker's own reserve loop always records
        # this event on its first failed attempt, strictly before it could
        # ever succeed (the first holds capacity via hold_first).
        assert engine.wait_until(lambda: _fetch_one(
            store, "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ) is not None, timeout=30)
        worker_opens = [a for a in engine.open_calls if a and a.startswith("xa_")]
        assert len(worker_opens) == 1  # only the first worker opened; the other is queued (capacity full)
        reservation_id = engine._reservations[worker_opens[0]]
        # Mark the open reservation ambiguous (unconfirmed termination) --
        # this must still count against the limit, so the second stays queued.
        # (Bypasses terminate_owned() on purpose, so this deliberately does
        # NOT notify the queued waiter -- it must observe this via its own
        # next backoff-bounded re-check, same as production.)
        attempts_before_ambiguous = len(engine.reserve_attempts)
        delegation_capacity.mark_ambiguous(store, reservation_id, "timeout_alive")
        # T018 fresh-review fix: deterministic, event-driven sync point --
        # wait (generously bounded, never a fixed-duration sleep standing
        # in for "nothing happened") for proof the queued worker actually
        # re-attempted reservation *after* the ambiguous mark, then assert
        # purely on durable state, never on elapsed wall time.
        assert engine.wait_until(lambda: len(engine.reserve_attempts) > attempts_before_ambiguous, timeout=30)
        latest_attempt_id, latest_reservation = engine.reserve_attempts[-1]
        assert latest_reservation is None  # refused: the ambiguous hold still counted against the limit
        with store.connection() as db:
            ambiguous_row = db.execute(
                "SELECT state FROM capacity_reservations WHERE reservation_id=?", (reservation_id,),
            ).fetchone()
        assert ambiguous_row["state"] == "ambiguous"
        worker_opens_after_ambiguous = [a for a in engine.open_calls if a and a.startswith("xa_")]
        assert len(worker_opens_after_ambiguous) == 1  # ambiguous held capacity; still no second open
        # Now confirm the release for real -- the second must proceed.
        delegation_capacity.release_confirmed(store, reservation_id, "process_exited")
        hold_second.set()
        assert engine.wait_until(
            lambda: len([a for a in engine.open_calls if a and a.startswith("xa_")]) >= 2, timeout=30,
        )
    finally:
        hold_first.set()
        hold_second.set()
        thread.join(timeout=30)
    with store.connection() as db:
        queued = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ).fetchone()
    assert queued is not None


def test_normal_turn_marks_delivered_at_once_engine_prompt_returns(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="nothing pending", request_key="acc1"))
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        row = db.execute(
            "SELECT consumed_by, delivered_at FROM task_events WHERE task_id=? AND event_type='user_message'", (task_id,),
        ).fetchone()
    assert row["consumed_by"] is not None and row["delivered_at"] is not None


def test_reconcile_releases_only_claims_never_confirmed_delivered(tmp_path):
    store, task_id = _task(tmp_path)
    e1 = delegation_events.append_event(store, task_id, "user_message", "um:0", dict(content="a", session_id="s1"), origin="s1")
    e2 = delegation_events.append_event(store, task_id, "user_message", "um:1", dict(content="b", session_id="s1"), origin="s1")
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont_a")
    assert {e.id for e in claimed} == {e1.id, e2.id}
    delegation_events.mark_delivered(store, task_id, "cont_a")
    # A second batch claimed but never confirmed delivered (simulated crash).
    e3 = delegation_events.append_event(store, task_id, "user_message", "um:2", dict(content="c", session_id="s1"), origin="s1")
    claimed2 = delegation_events.claim_unconsumed(store, task_id, "cont_b")
    assert {e.id for e in claimed2} == {e3.id}

    released = delegation_events.reconcile_undelivered_claims(store, task_id)
    assert released == 1
    with store.connection() as db:
        rows = {row["dedup_key"]: (row["consumed_by"], row["delivered_at"]) for row in db.execute(
            "SELECT dedup_key, consumed_by, delivered_at FROM task_events WHERE task_id=?", (task_id,),
        )}
    assert rows["um:0"][0] is not None and rows["um:0"][1] is not None  # delivered: untouched
    assert rows["um:1"][0] is not None and rows["um:1"][1] is not None
    assert rows["um:2"] == (None, None)  # released back to unconsumed, not lost


def test_ensure_coordinator_reconciles_undelivered_claims_on_first_use(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    calls = []
    original = delegation_events.reconcile_undelivered_claims

    def spy(store_arg, task_id_arg=None):
        calls.append(task_id_arg)
        return original(store_arg, task_id_arg)

    monkeypatch.setattr(delegation_events, "reconcile_undelivered_claims", spy)
    runtime._ensure_coordinator(task_id)
    assert calls == [task_id]
    # A second call for the same task (cached) never re-reconciles.
    runtime._ensure_coordinator(task_id)
    assert calls == [task_id]


# -- fresh-review fix: stale accept must never be honored (HIGH) -----------

def test_decide_accept_then_delegate_same_turn_is_never_honored(tmp_path):
    """Reproduces the reviewed HIGH: a coordinator that calls
    decide(task, accept) and then delegate(...) in the *same* turn must
    never see that turn end accepted -- the new, unopened assignment
    supersedes the stale accept."""
    store, task_id = _task(tmp_path)
    state = {"phase": 0}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        if "A finished" in message and state["phase"] == 0:
            state["phase"] = 1
            # The race: accept the task, then -- in the SAME tool-call
            # sequence -- delegate a brand new, still-unopened assignment.
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="A is done.", request_key="acc1"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Accepted and delegated B.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = "A" if "do A" in message else "B"
        _publish(store, channel, f"{title} finished", call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.2, max_no_progress_continuations=3)
    result = runtime.start_prompt(task_id, "s1", "Do A then B.", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state_row = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
        refused = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'acceptance_refused:%'", (task_id,),
        ).fetchone()
    # Never accepted with B still unopened/unresolved at the moment of the
    # stale decide() -- whatever the loop's eventual bounded outcome is
    # (blocked, since B may never resolve in this scripted run), it must
    # never be "accepted" purely off the stale decision.
    assert state_row["state"] != "accepted"
    assert refused is not None


def test_accept_superseded_by_a_later_worker_question_is_never_honored(tmp_path):
    """Direct, deterministic check of ``_recheck_accept``: a coordinator
    accept decision followed by a *later* (higher id) worker_question event
    must never be honored, even though no gate condition alone would catch
    a mid-air worker_question (a still-running worker never files one until
    it does)."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    runtime._ensure_coordinator(task_id)
    decision_event = delegation_events.append_event(
        store, task_id, "coordinator_decision", "decide:acc1",
        dict(assignment_ref="task", decision="accept", rationale="nothing pending", question=None),
        origin="ch_test",
    )
    delegation_events.append_event(
        store, task_id, "worker_question", "askQ:late", dict(question="What next?"),
        origin="ch_worker", assignment_ref="xa_late",
    )
    terminal = runtime._check_terminal_decision(task_id)
    assert terminal is None
    with store.connection() as db:
        refused = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
            (task_id, f"acceptance_refused:{decision_event.id}"),
        ).fetchone()
    assert refused is not None
    assert "worker_question" in refused["payload_json"]


# -- fresh-review fix: queued-open cancellation / shutdown / starvation ----

def test_cancel_task_while_worker_queued_frees_the_thread_without_a_reservation_leak(tmp_path):
    store, task_id = _task(tmp_path)
    hold_a, a_opened = threading.Event(), threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        a_opened.set()
        hold_a.wait(timeout=30)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_capacity_runtime(store, router, connection_id="cn_cancel", capacity_limit=2, wait_timeout_seconds=0.1)
    task_cancel_event = threading.Event()
    results = []
    thread = threading.Thread(target=lambda: results.append(
        runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, task_cancel_event)
    ))
    thread.start()
    try:
        assert a_opened.wait(timeout=30)  # A holds the one free worker slot; B is queued
        assert engine.wait_until(lambda: _fetch_one(
            store, "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ) is not None, timeout=30)
        task_cancel_event.set()  # cancel the task while B is still queued
        thread.join(timeout=30)
        assert results and results[0] == StopReason.CANCELLED
        assert task_id in engine._cancelled_tasks  # signaled promptly by _run()'s cancel check
        # B's own worker thread needs up to one more backoff tick to notice
        # the cancellation and record it -- generous bounded wait, not a
        # fixed sleep (wait_until re-polls on its own activity notifications).
        failed = engine.wait_until(lambda: _fetch_all(
            store, "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,),
        ), timeout=30)
        assert failed and any('"task"' in row["payload_json"] for row in failed)
        # No reservation leak: B never actually reserved capacity.
        with store.connection() as db:
            b_reservations = db.execute(
                "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id='cn_cancel' AND kind='worker'",
            ).fetchone()["n"]
        assert b_reservations == 1  # only A's -- B never reserved
    finally:
        hold_a.set()
        thread.join(timeout=30)


def test_runtime_shutdown_with_queued_opens_returns_promptly(tmp_path):
    store, task_id = _task(tmp_path)
    hold_a, a_opened = threading.Event(), threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        a_opened.set()
        hold_a.wait(timeout=30)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_capacity_runtime(store, router, connection_id="cn_shutdown", capacity_limit=2, wait_timeout_seconds=0.1)
    thread = threading.Thread(target=lambda: runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, threading.Event()))
    thread.start()
    try:
        assert a_opened.wait(timeout=30)
        assert engine.wait_until(lambda: _fetch_one(
            store, "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ) is not None, timeout=30)
        start = time.monotonic()
        runtime.shutdown()
        elapsed = time.monotonic() - start
        assert elapsed < 30  # shutdown() itself returns promptly (generous bound, not a hang)
        assert engine._shutdown_event.is_set()
    finally:
        hold_a.set()
        thread.join(timeout=30)


def test_starvation_coordinator_holds_only_slot_fails_worker_explicitly(tmp_path):
    """limit=1 shared with a coordinator that permanently holds the one
    slot: a delegated worker can never succeed -- must fail fast and
    explicitly (capacity_unsatisfiable), never queue forever."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_capacity_runtime(
        store, router, connection_id="cn_starve", capacity_limit=1, wait_timeout_seconds=0.1,
        max_no_progress_continuations=3,
    )
    start = time.monotonic()
    result = runtime.start_prompt(task_id, "s1", "Do A.", lambda t: True, threading.Event())
    elapsed = time.monotonic() - start
    assert elapsed < 30  # never hangs waiting on unsatisfiable capacity
    assert result == StopReason.END_TURN
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,),
        ).fetchall()
    assert any("capacity_unsatisfiable" in row["payload_json"] for row in rows)


def _poll(predicate, timeout=30.0, interval=0.05):
    deadline = time.monotonic() + timeout
    result = predicate()
    while not result and time.monotonic() < deadline:
        time.sleep(interval)
        result = predicate()
    return result


def test_assignment_cancel_while_worker_queued_aborts_promptly_no_leak(tmp_path):
    """Lead's own-risk follow-up: coordinator cancel(assignment_ref) for a
    still-*queued* (never-opened) worker must abort its blocked open()
    promptly, free the executor thread, leave no reservation row, record
    exactly one assignment_cancelled -- and never disturb the other,
    unrelated in-flight assignment.

    Symmetric: whichever of A/B's open() actually wins the single free
    worker slot is never assumed in advance (a real race) -- the queued one
    (whichever it turns out to be) is identified and cancelled."""
    store, task_id = _task(tmp_path)
    hold_open, opened, lock, refs = threading.Event(), threading.Event(), threading.Lock(), {}
    opened_title = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result_a = mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            result_b = mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            refs["A"] = result_a["assignment_ref"]
            refs["B"] = result_b["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        if "Cancel queued" in message:
            queued_title = "B" if opened_title.get("title") == "A" else "A"
            mcp_tools.dispatch(store, channel, "cancel", dict(assignment_ref=refs[queued_title], reason="no longer needed", request_key="cancelQ"))
            return PromptOutcome(StopReason.END_TURN, f"Cancelled {queued_title}.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = "A" if "do A" in message else "B"
        with lock:
            is_first = not opened.is_set()
            if is_first:
                opened_title["title"] = title
                opened.set()
        if not is_first:
            raise AssertionError("the queued worker must never actually open/run")
        hold_open.wait(timeout=30)
        _publish(store, channel, f"{title} finished", call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_capacity_runtime(
        store, router, connection_id="cn_assign_cancel", capacity_limit=2, wait_timeout_seconds=0.1,
        max_no_progress_continuations=3,
    )
    try:
        result1 = runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, threading.Event())
        assert result1 == StopReason.END_TURN  # bounded blocked (one held, one queued, nothing meaningful yet)
        assert opened.is_set()
        assert "A" in refs and "B" in refs
        queued_title = "B" if opened_title["title"] == "A" else "A"
        queued_ref = refs[queued_title]
        queued_row = _poll(lambda: _fetch_one(
            store, "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (queued_ref,),
        ))
        assert queued_row is not None
        queued_attempt_id = queued_row["attempt_id"]
        assert queued_attempt_id not in engine.open_calls  # genuinely never opened

        start = time.monotonic()
        result2 = runtime.start_prompt(task_id, "s1", "Cancel queued worker please.", lambda t: True, threading.Event())
        elapsed = time.monotonic() - start
        assert result2 == StopReason.END_TURN
        assert elapsed < 30  # cancel while queued resolves promptly, not a hang

        assert _poll(lambda: queued_attempt_id in engine._cancelled_attempts)
        with store.connection() as db:
            cancelled_rows = db.execute(
                "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?",
                (task_id, queued_ref),
            ).fetchone()
        assert cancelled_rows["n"] == 1  # exactly one
        with store.connection() as db:
            leaked = db.execute(
                "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id='cn_assign_cancel' AND attempt_id=?",
                (queued_attempt_id,),
            ).fetchone()
        assert leaked["n"] == 0  # never reserved -- nothing to leak
        assert queued_attempt_id not in engine._owned  # executor thread freed, nothing left owned for it
    finally:
        hold_open.set()

    # Other task work (the one that actually opened) is unaffected by the
    # queued sibling's cancellation.
    assert _poll(lambda: _fetch_one(
        store, "SELECT 1 FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
    ))


def _fetch_one(store, query, params):
    with store.connection() as db:
        return db.execute(query, params).fetchone()


def _fetch_all(store, query, params):
    with store.connection() as db:
        return db.execute(query, params).fetchall()
