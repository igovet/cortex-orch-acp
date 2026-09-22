"""T017: completion gating, turn-outcome handling (refusal/limit/engine
failure), durable waiting/no-progress persistence, and crash-recovery
reconciliation for OrchestratingRuntime -- fakes only, no real engine."""
from __future__ import annotations

import json
import threading
import time
import uuid
from pathlib import Path

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed, OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import completion as delegation_completion
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.project_docs import build_catalog
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
    engine = FakeSessionRuntime(
        store, router, connection_id=connection_id, capacity_limit=capacity_limit,
        capacity_queue_timeout_seconds=kwargs.pop("capacity_queue_timeout_seconds", 30.0),
    )
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 0.1),
        **kwargs,
    )
    runtime._cleanup_isolated_workspace_if_integrated = lambda task_id, attempt_id: None
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
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    runtime._cleanup_isolated_workspace_if_integrated = lambda task_id, attempt_id: None
    _created_runtimes.append(runtime)
    return runtime, engine


def _own_draft(db, store, channel, report_id, path):
    """T019: matching drafts row owned by this channel's own
    principal_thread_id so publish_result's report_ref-authorship binding
    (see delegation.mcp_tools._worker_publish_result) accepts it."""
    info = path.stat()
    db.execute(
        "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
        "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        ("d_" + uuid.uuid4().hex[:12], channel.task_id, channel.principal_thread_id, "report", "general",
         str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
    )


def _publish(store, channel, label, call_index):
    report_id = "r_" + uuid.uuid4().hex[:12]
    filename = report_id + ".md"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        path = task_dir / filename
        path.write_text(f"# {label}\n", encoding="utf-8")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", filename, store._now()))
        _own_draft(db, store, channel, report_id, path)
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}-{call_index}"))
    return report_id


def _publish_current_pipeline(store, channel, suffix=""):
    """Publish the coordinator's current plan before task-level acceptance."""
    draft = store.call(
        "create_draft", {"template": "pipeline", "request_key": f"pipeline-current{suffix}"},
        channel.principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    body = path.read_text(encoding="utf-8")
    values = {
        "{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}": "Objective: build the page. Decision basis: delegated work is reviewable.",
        "{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}": "Requirements: delegated work is complete and reviewable.",
        "{{CURRENT_WORK_GRAPH}}": "All delegated assignments have completed.",
        "{{CURRENT_ASSIGNMENTS_AND_ROUTING}}": "Coordinator reviewed the assigned workers.",
        "{{CURRENT_EVIDENCE_AND_VERIFICATION}}": "Published worker reports provide the current evidence.",
        "{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}": "Decision: accept the completed work.",
    }
    for marker, value in values.items():
        body = body.replace(marker, value)
    for marker in draft["replaceable_markers"]:
        if marker.startswith("<!--"):
            body = body.replace(marker, "Evidence links: the published worker reports are the selected results.")
    path.write_text(body, encoding="utf-8")
    return store.call(
        "write_report", {
            "title": "Current pipeline", "summary": "Coordinator current plan.",
            "author": "coordinator", "draft_id": draft["draft_id"], "request_key": f"pipeline-current-publish{suffix}",
        }, channel.principal_thread_id, trusted=True,
    )


# -- completion gating -----------------------------------------------------









def _latest_decision_id(store, task_id):
    with store.connection() as db:
        return db.execute(
            "SELECT id FROM task_events WHERE task_id=? AND event_type='coordinator_decision' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()["id"]




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
    assert row["state"] == "waiting_for_user"  # asked what next, never a dead end
    assert row["wait_reason"] == "coordinator_turn_failed:refusal"








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
    assert row["state"] == "waiting_for_user"
    assert row["wait_reason"] == "engine_failure:transport_eof"
    assert user_message["consumed_by"] is not None  # uncertain transmission is reconciled on recovery




def test_coordinator_transport_eof_reopens_fresh_session_on_next_turn(tmp_path):
    """A dead coordinator (transport EOF) must not be reused forever: the
    turn blocks, but the next user message reopens a fresh session and runs."""
    store, task_id = _task(tmp_path)
    calls = {"n": 0}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        calls["n"] += 1
        if calls["n"] == 1:
            raise EngineTurnFailed("transport_eof")
        return PromptOutcome(StopReason.END_TURN, "recovered")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    first = runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, threading.Event())
    assert first == StopReason.END_TURN
    dead_session = runtime._coordinator_sessions.get(task_id)
    assert dead_session is None  # evicted, not left cached for reuse

    # The follow-up prompt reopens a fresh session and completes.
    second = runtime.start_prompt(task_id, "s2", "Please continue.", lambda t: True, threading.Event())
    assert second == StopReason.END_TURN
    assert calls["n"] > 1  # the second turn actually reached the (reopened) engine
    assert runtime._coordinator_sessions.get(task_id) not in (None, "es_1")


def test_coordinator_max_tokens_with_no_unfinished_action_blocks_not_success(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.MAX_TOKENS, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, threading.Event())
    assert result == StopReason.MAX_TOKENS
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "waiting_for_user"
    assert row["wait_reason"] == "coordinator_turn_failed:max_tokens"


# -- durable waiting / no-progress / resumability ---------------------------





# -- crash between claim and delivery ---------------------------------------

# -- capacity queuing through OrchestratingRuntime (lead follow-up) --------









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
