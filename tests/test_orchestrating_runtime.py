"""T016: acp.orchestrating_runtime.OrchestratingRuntime -- model-owned
coordination and managed workers, against in-process fakes only.

No real engine subprocess, no paid model call, no network I/O; every Store
is rooted at tmp_path. The "fake model" behaviors call
``delegation.mcp_tools.dispatch`` directly with each session's own
``ChannelContext``, the same MCP tool handlers a real engine's Cortex MCP
subprocess would call.
"""
from __future__ import annotations

import json
import threading
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime, PromptOutcome, SecondCoordinatorRefused,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _assignment_title(store, channel):
    with store.connection() as db:
        row = db.execute(
            "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (channel.attempt_id,),
        ).fetchone()
        if row is None:
            return None
        event = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND assignment_ref=?",
            (channel.task_id, row["idempotency_key"]),
        ).fetchone()
    return json.loads(event["payload_json"])["title"] if event is not None else None


def _publish(store, channel, label, call_index):
    """Insert a ``reports`` row *and* write its file, matching real
    ``write_report`` storage (``<task_dir>/<report_id>.md``) -- T017's
    completion gate (``delegation.completion.gate``) requires the file to
    actually resolve on disk, not only a DB row. T019: also insert a
    matching ``drafts`` row owned by this exact channel's
    ``principal_thread_id`` so ``publish_result``'s report_ref-authorship
    binding (see ``delegation.mcp_tools._worker_publish_result``) accepts
    it, the same way a real worker's own ``create_draft``/``write_report``
    would."""
    report_id = "r_" + uuid.uuid4().hex[:12]
    filename = report_id + ".md"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        path = task_dir / filename
        path.write_text(f"# {label}\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", filename, store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], channel.task_id, channel.principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
        )
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}-{call_index}"))


def _make_runtime(store, router, **kwargs):
    engine = FakeSessionRuntime(store, router)
    # T018 fresh-review fix: the default here is only used by tests that
    # expect a *success* path (a background executor worker actually
    # finishing and notifying _wait_for_progress's condition variable --
    # see orchestrating_runtime._wait_for_progress), never by a test
    # deliberately exercising the short-timeout 'blocked' path (those pass
    # their own short wait_timeout_seconds explicitly). A short wall-clock
    # budget here is a real-thread-scheduling race, not a deterministic
    # bound; under CPU contention (reproduced: 4 CPU hogs + 10x repeats)
    # the previous 2.0s default occasionally elapsed before the worker
    # thread pool got scheduled, flipping an expected 'accepted' outcome to
    # 'blocked'. A generous budget costs nothing on a healthy run (the
    # condition-variable wait still returns as soon as progress is
    # notified; see _wait_for_progress's bounded backoff poll) and only
    # matters when something has gone genuinely, durably wrong.
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    return runtime, engine


def test_one_prompt_delegates_two_assignments_and_accepts_both_results(tmp_path):
    store, task_id = _task(tmp_path)
    seen, decided = set(), {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Assignment A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Assignment B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        for name in ("Assignment A", "Assignment B"):
            if f"{name} finished" in message:
                seen.add(name)
        if len(seen) >= 2 and not decided["done"]:
            decided["done"] = True
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="Both assignments delivered acceptable results.", request_key="acceptall"))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting for remaining results.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Build A and B.", updates.append, cancel_event)

    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    assert any("Accepted" in u for u in updates)
    # No raw worker chunk (never "do A"/"do B" verbatim as a coordinator update).
    assert not any("do A" in u or "do B" in u for u in updates)
    coordinator_session_ref = runtime._coordinator_sessions[task_id]
    coordinator_calls = sum(1 for ref in engine.prompt_calls if ref == coordinator_session_ref)
    # One initial turn plus at most a small, bounded number of continuations
    # -- never one coordinator prompt per DB-polling wait tick.
    assert 2 <= coordinator_calls <= 4


def test_second_coordinator_is_refused_while_one_is_active(tmp_path):
    store, task_id = _task(tmp_path)
    release = threading.Event()
    entered = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        entered.set()
        release.wait(timeout=5)
        return PromptOutcome(StopReason.END_TURN, "done")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    thread = threading.Thread(target=runtime.start_prompt, args=(task_id, "s1", "Go.", lambda t: True, cancel_event))
    thread.start()
    try:
        assert entered.wait(timeout=5)
        with pytest.raises(SecondCoordinatorRefused):
            runtime.start_prompt(task_id, "s2", "Go again.", lambda t: True, threading.Event())
    finally:
        release.set()
        thread.join(timeout=5)


def _insert_configuration_revision(store, task_id, revision_id, state, *, error_code=None):
    """Test-only: seed one ``configuration_revisions`` row directly (never
    through ``ConfigurationService`` -- these tests exercise
    ``OrchestratingRuntime``'s own post-open gate against whatever state a
    revision has settled into, independent of how it got there)."""
    from cortex_runtime.configuration.policy import PolicyCeiling
    from cortex_runtime.configuration.store import serialize_ceiling

    now = "2020-01-01T00:00:00+00:00"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO configuration_revisions "
                "(id, task_id, revision, route_id, route_engine, route_connection_id, parameters, "
                "ceiling_snapshot, state, confirmation_evidence_ref, error_code, created_at, updated_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (revision_id, task_id, 1, "xr_test", "codex-acp", "cn_test", "{}",
                 serialize_ceiling(PolicyCeiling()), state, None, error_code, now, now),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def test_turn_refused_when_configuration_revision_settled_failed(tmp_path):
    """T035 FINDING-02: a turn must not proceed on a configuration
    revision that has already settled 'failed' -- the client must see the
    engine's own refusal reason, never a silent substitution."""
    store, task_id = _task(tmp_path)
    revision_id = "cr_" + task_id[2:]  # matches FakeConfigSnapshotSource's config_revision
    _insert_configuration_revision(
        store, task_id, revision_id, "failed", error_code="claude_acp_option_value_unsupported:model",
    )

    def router(store, channel, session_ref, launch, instructions):
        return lambda store, channel, session_ref, message, call_index: PromptOutcome(StopReason.END_TURN, "READY")

    runtime, engine = _make_runtime(store, router)
    with pytest.raises(PortableError) as excinfo:
        runtime.start_prompt(task_id, "s1", "Go.", lambda text: True, threading.Event())
    assert excinfo.value.code == "claude_acp_option_value_unsupported:model"
    # Never opened a prompt turn on the refused configuration.
    assert engine.prompt_calls == []


def test_turn_proceeds_when_engine_has_no_configuration_surface_at_all(tmp_path):
    """T035 FINDING-02 scope guard: 'engine_does_not_support_configuration_options'
    is the pre-existing, documented, intentional case (see
    tests/test_acp_setup_fresh_session.py::
    test_rejected_configuration_never_becomes_active) -- there was never a
    specific requested value to have silently substituted, so this must
    never be gated, unlike every other failed/rejected error_code."""
    store, task_id = _task(tmp_path)
    revision_id = "cr_" + task_id[2:]
    _insert_configuration_revision(
        store, task_id, revision_id, "failed", error_code="engine_does_not_support_configuration_options",
    )

    def router(store, channel, session_ref, launch, instructions):
        return lambda store, channel, session_ref, message, call_index: PromptOutcome(StopReason.END_TURN, "READY")

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda text: True, threading.Event())
    assert result == StopReason.END_TURN


def test_turn_proceeds_on_a_fresh_unapplied_selected_revision(tmp_path):
    """T035 FINDING-02 regression guard: a fresh task's first coordinator
    open must still resolve its not-yet-applied 'selected' revision --
    the T035 fix that made ``current_or_latest``/the snapshot source
    tolerate 'selected' must keep working; only a settled 'failed'/
    'rejected' revision is ever gated."""
    store, task_id = _task(tmp_path)
    revision_id = "cr_" + task_id[2:]
    _insert_configuration_revision(store, task_id, revision_id, "selected")

    def router(store, channel, session_ref, launch, instructions):
        return lambda store, channel, session_ref, message, call_index: PromptOutcome(StopReason.END_TURN, "READY")

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Go.", lambda text: True, threading.Event())
    assert result == StopReason.END_TURN


def test_ask_user_ends_turn_and_next_prompt_resumes_the_same_task(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="ask_user", rationale="Need to know which environment.",
                question="Which environment: staging or production?", request_key="ask1",
            ))
            return PromptOutcome(StopReason.END_TURN, "")
        if "Use staging" in message:
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="User chose staging; nothing left to do.", request_key="accept-after-user"))
            return PromptOutcome(StopReason.END_TURN, "Accepted with staging.")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    updates1 = []
    result1 = runtime.start_prompt(task_id, "s1", "Deploy it.", updates1.append, threading.Event())
    assert result1 == StopReason.END_TURN
    assert any("staging or production" in u for u in updates1)
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "waiting_for_user"

    updates2 = []
    result2 = runtime.start_prompt(task_id, "s1", "Use staging.", updates2.append, threading.Event())
    assert result2 == StopReason.END_TURN
    assert any("Accepted" in u for u in updates2)
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"


def test_no_progress_budget_ends_turn_blocked_without_infinite_waiting(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Assignment X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX"))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        # Never publishes a result or asks a question -- a stalled worker.
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.2, max_no_progress_continuations=2)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "blocked"
    assert any("Blocked" in u for u in updates)


def test_repeated_empty_coordinator_turns_end_blocked_and_retain_history(tmp_path):
    """PR-06: a coordinator that keeps producing literally empty turns
    (not merely unproductive text) while a worker never progresses must
    still be stopped by the no-progress budget -- never loop forever
    re-prompting the model -- and every event/report already recorded
    stays readable (via list_events) after the turn ends blocked."""
    call_counts = {"coordinator": 0}

    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        call_counts["coordinator"] += 1
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Assignment Y", brief="do Y", role="backend-dev", acceptance="Y done", request_key="reqY"))
            return PromptOutcome(StopReason.END_TURN, "")
        # Every subsequent turn is a genuinely empty response -- no text,
        # no tool call, no progress -- exactly the "repeated empty turns"
        # scenario PR-06 requires exercising.
        return PromptOutcome(StopReason.END_TURN, "")

    def silent_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.2, max_no_progress_continuations=2)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do Y.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    # The budget bounds the retry count: it must not have kept calling the
    # coordinator forever (an infinite model loop).
    assert call_counts["coordinator"] <= 10
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
        event_count = db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0]
    assert state == "blocked"
    assert event_count > 0  # history is retained, not discarded, after the blocked close
    reader_channel = _resolve_owner_channel(store, task_id)
    events = mcp_tools.dispatch(store, reader_channel, "list_events", dict(request_key="list-after-block"))
    assert isinstance(events["events"], list) and len(events["events"]) > 0


def _resolve_owner_channel(store, task_id):
    from cortex_runtime.ownership import current_owner_epoch
    from cortex_runtime.trusted_channel import issue_channel, resolve_channel

    with store.connection() as db:
        epoch = current_owner_epoch(db, task_id)
    channel_id, secret = issue_channel(store, task_id, "coordinator", epoch)
    return resolve_channel(store, channel_id, secret)


def test_worker_question_reaches_coordinator_only_then_clarify_then_publish(tmp_path):
    store, task_id = _task(tmp_path)
    phase = {"clarified": False, "accepted": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Assignment Q", brief="do Q", role="backend-dev", acceptance="Q done", request_key="reqQ"))
            return PromptOutcome(StopReason.END_TURN, "Delegated Q.")
        if "question" in message.lower() and not phase["clarified"]:
            phase["clarified"] = True
            assignment_ref = None
            with store.connection() as db:
                row = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE task_id=?", (task_id,)).fetchone()
                assignment_ref = row["idempotency_key"]
            mcp_tools.dispatch(store, channel, "clarify", dict(assignment_ref=assignment_ref, message="Use the default config.", request_key="clar1"))
            return PromptOutcome(StopReason.END_TURN, "Clarified.")
        if "result" in message.lower() and not phase["accepted"]:
            phase["accepted"] = True
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="Q result accepted.", request_key="acceptQ"))
            return PromptOutcome(StopReason.END_TURN, "Accepted Q.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "ask_question", dict(question="Which config should I use?", request_key="askQ"))
            return PromptOutcome(StopReason.END_TURN, "")
        _publish(store, channel, "Assignment Q", call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do Q.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    # The worker's question text must never be streamed to the user directly.
    assert not any("Which config should I use" in u for u in updates)


def test_unknown_role_delegation_is_visible_and_recorded_not_silently_absorbed(tmp_path):
    """FINDING 11: a refused delegation (unknown role) must never silently
    degrade into "the coordinator quietly did it all itself" -- the client
    must see the refusal, and the durable record must distinguish it from a
    real delegation."""
    store, task_id = _task(tmp_path)
    state = {"seen_failure": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="totally_bogus_role",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        if "was NOT delegated" in message:
            state["seen_failure"] = True
        if state["seen_failure"]:
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="accept",
                rationale="Delegation was refused; did the work myself.", request_key="acceptX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Did the work myself.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise AssertionError("a refused delegation must never launch a worker")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    assert result == StopReason.END_TURN

    # 1) Visible to the client: a session update names the refusal itself,
    # distinct from the coordinator's own free-form text.
    assert any("was NOT delegated" in u for u in updates)

    # 2) Durably recorded, and actionable (not a bare code): the requested
    # role and the valid ids are both carried, and there is no worker_result
    # for this task -- a real delegation and a refused one are
    # programmatically distinguishable, never collapsed together.
    with store.connection() as db:
        failed_rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchall()
        worker_result_rows = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchall()
    assert len(failed_rows) == 1
    payload = json.loads(failed_rows[0]["payload_json"])
    assert payload["error"] == "unknown_role"
    assert payload["role"] == "totally_bogus_role"
    assert "worker-backend-dev" in payload["detail"]
    assert "orchestrator" in payload["detail"]
    assert worker_result_rows == []


def test_claimed_events_survive_a_prompt_exception_and_are_delivered_once(tmp_path):
    """T016 review (MEDIUM): if engine.prompt() raises, the events claimed
    for that turn must not be lost -- they must be released back to
    unconsumed and delivered on a later continuation, exactly once."""
    store, task_id = _task(tmp_path)
    calls = {"n": 0}

    def flaky_coordinator(store, channel, session_ref, message, call_index):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("simulated engine.prompt() failure")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Nothing else pending.", request_key="acc1",
        ))
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def router(store, channel, session_ref, launch, instructions):
        return flaky_coordinator if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    with pytest.raises(RuntimeError):
        runtime.start_prompt(task_id, "s1", "Please check in.", lambda t: True, threading.Event())

    with store.connection() as db:
        row = db.execute(
            "SELECT consumed_by, superseded FROM task_events WHERE task_id=? AND event_type='user_message'", (task_id,),
        ).fetchone()
    assert row["consumed_by"] is None and row["superseded"] == 0  # released, not lost

    updates = []
    result = runtime.start_prompt(task_id, "s1", "Second try.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert any("Accepted" in u for u in updates)

    with store.connection() as db:
        first_message = db.execute(
            "SELECT consumed_by FROM task_events WHERE task_id=? AND event_type='user_message' AND dedup_key=?",
            (task_id, "user_message:s1:0"),
        ).fetchone()
    assert first_message["consumed_by"] is not None  # eventually claimed and delivered exactly once
