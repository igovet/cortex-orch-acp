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


def test_task_level_accept_refuses_runtime_bootstrap_pipeline(tmp_path):
    """The runtime's opening receipt cannot stand in for the coordinator's
    current plan and acceptance rationale.

    This reproduces the live failure where a task reached ``accepted`` while
    ``pipeline.md`` still said that no coordinator turn had run.  The runtime
    must leave the task undecided and give the model a concrete next step;
    it must not invent or mutate a pipeline on the model's behalf.
    """
    store, task_id = _task(tmp_path)
    from cortex_runtime.trusted_channel import issue_channel_tx

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        db.commit()
    from orchestration_fakes import channel_context_by_id

    channel = channel_context_by_id(store, channel_id)
    pipeline_bootstrap.publish_initial_pipeline(store, task_id, channel, "Build a page.", None)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="done", request_key="acc-bootstrap",
        ))
    assert str(excinfo.value) == "acceptance_refused"
    assert "publish a coordinator-authored current pipeline edition" in str(excinfo.value.correction)
    with store.connection() as db:
        assert db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='coordinator_decision'",
            (task_id,),
        ).fetchone() is None


def test_task_accept_binds_to_unchanged_current_pipeline(tmp_path):
    store, task_id = _task(tmp_path)
    from cortex_runtime.trusted_channel import issue_channel_tx
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        channel_id, _ = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        db.commit()
    from orchestration_fakes import channel_context_by_id
    channel = channel_context_by_id(store, channel_id)
    _publish_current_pipeline(store, channel, "-binding")
    decision = mcp_tools.dispatch(store, channel, "decide", dict(
        assignment_ref="task", decision="accept", rationale="complete", request_key="accept-binding",
    ))
    assert decision["recorded"] is True
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='coordinator_decision'",
            (task_id,),
        ).fetchone()
        payload = json.loads(row["payload_json"])
        binding = payload["pipeline_binding"]
        assert set(binding) == {"report_id", "sequence", "digest"}
        assert delegation_completion.pipeline_binding_matches(db, task_id, binding)


def test_task_accept_is_refused_when_current_pipeline_changes_after_decision(tmp_path):
    store, task_id = _task(tmp_path)
    from cortex_runtime.trusted_channel import issue_channel_tx
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        channel_id, _ = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        db.commit()
    from orchestration_fakes import channel_context_by_id
    channel = channel_context_by_id(store, channel_id)
    _publish_current_pipeline(store, channel, "-binding-old")
    mcp_tools.dispatch(store, channel, "decide", dict(
        assignment_ref="task", decision="accept", rationale="complete", request_key="accept-stale-binding",
    ))
    _publish_current_pipeline(store, channel, "-binding-new")

    runtime, _ = _make_runtime(store, lambda *args: None)
    assert runtime._recheck_accept(task_id, _latest_decision_id(store, task_id)) is None
    with store.connection() as db:
        refusal = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
            (task_id, f"acceptance_refused:{_latest_decision_id(store, task_id)}"),
        ).fetchone()
    assert refusal is not None
    assert "pipeline edition" in json.loads(refusal["payload_json"])["note"]


def _latest_decision_id(store, task_id):
    with store.connection() as db:
        return db.execute(
            "SELECT id FROM task_events WHERE task_id=? AND event_type='coordinator_decision' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()["id"]


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
        # T019: matching drafts row (owner_thread_id == the synthetic
        # worker's own principal_thread_id below) so publish_result's
        # report_ref-authorship binding accepts it -- the file itself is
        # deliberately never written, which is exactly what this test
        # exercises (a claimed success with no stored evidence).
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], task_id, channel.principal_thread_id, "report", "general",
             f"/nonexistent/{report_id}.md", store._now(), 0, 0, report_id, "test-digest"),
        )
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

def test_task_stop_does_not_promote_partial_worker_commentary_to_final_response(tmp_path):
    store, task_id = _task(tmp_path)
    worker_started = threading.Event()
    refs = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            delegated = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment A", brief="do A", role="backend-dev",
                acceptance="A done", request_key="reqA",
            ))
            refs["assignment"] = delegated["assignment_ref"]
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(
        store, channel, session_ref, message, call_index, *, on_delta, cancel_event,
    ):
        on_delta("I am still inspecting the project; this is not a final answer.")
        worker_started.set()
        cancel_event.wait(timeout=30)
        return PromptOutcome(StopReason.CANCELLED, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, _engine = _make_runtime(store, router, wait_timeout_seconds=0.05)
    cards = []
    result = {}
    turn = threading.Thread(target=lambda: result.setdefault(
        "stop_reason",
        runtime.start_prompt(
            task_id, "s1", "Do A.", lambda _text: True, threading.Event(),
            emit_tool_call=cards.append,
        ),
    ))
    turn.start()
    assert worker_started.wait(timeout=30)
    assert runtime.stop_task(task_id, "s1", lambda _text: True, emit_tool_call=cards.append)
    turn.join(timeout=30)
    assert not turn.is_alive()
    assert result["stop_reason"] == StopReason.CANCELLED

    with store.connection() as db:
        false_final = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND json_extract(payload_json, '$.kind')='worker_activity' "
            "AND json_extract(payload_json, '$.text') LIKE '[final response]%' LIMIT 1",
            (task_id,),
        ).fetchone()
        assignment = db.execute(
            "SELECT status FROM work_assignments WHERE task_id=? ORDER BY created_at LIMIT 1",
            (task_id,),
        ).fetchone()
    assert false_final is None
    assert assignment is not None and assignment["status"] == "failed"
    assert refs["assignment"]


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


@pytest.mark.parametrize("with_indexes", [False, True])
def test_worker_engine_prompt_carries_current_project_docs_catalog(tmp_path, with_indexes):
    """Assert the actual delegated prompt, including a first task without docs."""
    if with_indexes:
        project_index = tmp_path / "docs/project/index.md"
        feature_index = tmp_path / "docs/features/index.md"
        project_index.parent.mkdir(parents=True)
        feature_index.parent.mkdir(parents=True)
        project_index.write_text("# Project map\n", encoding="utf-8")
        feature_index.write_text("# Feature map\n", encoding="utf-8")
    store, task_id = _task(tmp_path)
    worker_prompts = []
    worker_prompt_seen = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Repair arithmetic", brief="Fix multiplication", role="backend-dev",
                acceptance="Tests pass", request_key="docs-worker",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_prompts.append(message)
        worker_prompt_seen.set()
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    runtime, _ = _make_runtime(store, router, max_no_progress_continuations=1)
    runtime.start_prompt(task_id, "s1", "Repair arithmetic", lambda t: True, threading.Event())
    if not worker_prompt_seen.wait(10):
        with store.connection() as db:
            events = [(row["event_type"], row["payload_json"]) for row in db.execute(
                "SELECT event_type, payload_json FROM task_events WHERE task_id=?", (task_id,),
            ).fetchall()]
        pytest.fail(f"delegated engine was never prompted: {events}")
    assert 1 <= len(worker_prompts) <= 2
    prompt = worker_prompts[0]
    assert build_catalog(tmp_path)["catalog_version"] in prompt
    assert "BEGIN UNTRUSTED PROJECT DOCUMENTATION DATA" in prompt
    assert "END UNTRUSTED PROJECT DOCUMENTATION DATA" in prompt
    assert "scope=new worker attempt" in prompt
    assert "Fix multiplication" in prompt
    assert "Acceptance: Tests pass" in prompt
    if with_indexes:
        assert "docs/project/index.md | Project map | sha256:" in prompt
        assert "docs/features/index.md | Feature map | sha256:" in prompt
        assert "read the relevant index before broad source searches" in prompt
    else:
        assert "entries: none observed under docs/project or docs/features" in prompt
        assert "An absent index is not a blocker" in prompt


def test_report_review_handoff_injects_exact_predecessor_ref_into_worker_prompt(tmp_path):
    store, task_id = _task(tmp_path)
    report_ref = "r_" + "c" * 12
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, task_id)
        report_path = task_dir / "predecessor.md"
        report_path.write_text("# Predecessor\n", encoding="utf-8")
        db.execute(
            "INSERT INTO reports VALUES (?,?,?,?,?)",
            (report_ref, task_id, "report", report_path.name, store._now()),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "worker_result", "worker-result-for-handoff",
        dict(report_ref=report_ref, summary="The implementation is ready for review."),
        origin="worker-channel", assignment_ref="predecessor-assignment",
    )
    prompts = []
    seen = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Review implementation", brief="Review the predecessor result.", role="qa-engineer",
                acceptance="Publish independent review findings.", handoff_mode="report_review",
                predecessor_report_ref=report_ref, request_key="review-handoff",
            ))
        return PromptOutcome(StopReason.END_TURN, "Delegated review.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        prompts.append(message)
        seen.set()
        return PromptOutcome(StopReason.END_TURN, "Reviewed.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    runtime, _ = _make_runtime(store, router, max_no_progress_continuations=1)
    runtime.start_prompt(task_id, "s1", "Review the implementation.", lambda update: None, threading.Event())
    assert seen.wait(10)
    assert 1 <= len(prompts) <= 2
    assert '"handoff_mode": "report_review"' in prompts[0]
    assert f'"predecessor_report_ref": "{report_ref}"' in prompts[0]


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
    """A task left blocked resumes on the next prompt: the explanation is
    delivered, durably recorded, and the counter starts over.

    The blocked state is set here directly rather than produced by a
    stalled worker, because that route is gone: an open assignment no
    longer exhausts the coordinator's patience at all (a worker may
    legitimately take hours), and a worker that ends without reporting
    closes its own assignment. What this test is about -- resuming from
    blocked -- is unchanged.
    """
    store, task_id = _task(tmp_path)
    seen_messages: list = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen_messages.append(message)
        if "Still there?" in message or "Resuming" in message:
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
    runtime._ensure_coordinator(task_id)
    from cortex_runtime.acp import messages as acp_messages

    explanation = acp_messages.render("blocked_no_progress", "en")
    runtime._set_state(
        task_id, "blocked", wait_reason="blocked",
        wait_detail=dict(explanation=explanation), no_progress_count=2,
    )
    with store.connection() as db:
        row = db.execute("SELECT state, wait_reason, wait_detail_json, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "blocked"
    assert row["no_progress_count"] >= 2
    assert "no new worker result" in json.loads(row["wait_detail_json"])["explanation"]

    updates2 = []
    result2 = runtime.start_prompt(task_id, "s1", "Still there?", updates2.append, threading.Event())
    assert result2 == StopReason.END_TURN
    with store.connection() as db:
        row2 = db.execute("SELECT state, wait_reason, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row2["state"] == "waiting_for_user"
    # The resume explanation actually reached the coordinator.
    assert any("Resuming after a prior wait" in message for message in seen_messages), seen_messages
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
        _publish_current_pipeline(store, channel)
        mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="ok", request_key="accY"))
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def slow_worker(store, channel, session_ref, message, call_index):
        release.wait(timeout=5)
        report_id = "r_" + uuid.uuid4().hex[:12]
        with store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            task_dir = store._task_directory(db, channel.task_id)
            path = task_dir / f"{report_id}.md"
            path.write_text("# Y\n", encoding="utf-8")
            db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", f"{report_id}.md", store._now()))
            _own_draft(db, store, channel, report_id, path)
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

def test_accept_releases_coordinator_capacity_after_final_and_next_task_opens(tmp_path):
    """An accepted task must not retain an idle engine or monopolize capacity."""
    from cortex_runtime.delegation import capacity as delegation_capacity

    store, first_task = _task(tmp_path)
    second_task = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build another page.", "owner-1",
    ).task_id

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        _publish_current_pipeline(store, channel, "-" + channel.task_id)
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="verified", request_key="accept-" + channel.task_id,
        ))
        return PromptOutcome(StopReason.END_TURN, "Accepted task " + channel.task_id)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior

    runtime, engine = _make_capacity_runtime(
        store, router, connection_id="cn_terminal_accept", capacity_limit=1,
    )
    first_updates = []

    def emit_first(text, **kwargs):
        # The terminal text is delivered while the process is still owned;
        # termination follows only after the accepted state is committed.
        first_updates.append(text)
        assert len(engine._owned) == 1
        return True

    assert runtime.start_prompt(first_task, "s1", "First.", emit_first, threading.Event()) == StopReason.END_TURN
    assert "Accepted task " + first_task in "".join(first_updates)
    first_attempt = engine.sessions[engine.prompt_calls[0]]["attempt_id"]
    assert engine.terminate_calls == [first_attempt]
    assert engine.lookup_owned(first_attempt) == "absent"
    assert delegation_capacity.count_active(store, "cn_terminal_accept") == 0
    with store.connection() as db:
        assert db.execute(
            "SELECT state FROM task_orchestration WHERE task_id=?", (first_task,),
        ).fetchone()["state"] == "accepted"

    second_updates = []
    assert runtime.start_prompt(
        second_task, "s2", "Second.", second_updates.append, threading.Event(),
    ) == StopReason.END_TURN
    assert "Accepted task " + second_task in "".join(second_updates)
    assert len(engine.open_calls) == 2
    assert delegation_capacity.count_active(store, "cn_terminal_accept") == 0


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
            _publish_current_pipeline(store, channel)
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
            hold_first.wait()
        else:
            hold_second.wait()
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    # This test controls both releases. Keep the fake worker and queue alive
    # until those events occur, so a loaded suite cannot make their independent
    # 30-second deadlines expire before the assertion observes the release.
    runtime, engine = _make_capacity_runtime(
        store, router, connection_id="cn_ambiguous", capacity_limit=2,
        capacity_queue_timeout_seconds=None, wait_timeout_seconds=0.1,
        max_no_progress_continuations=5,
    )
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
        second_opened = engine.wait_until(
            lambda: len([a for a in engine.open_calls if a and a.startswith("xa_")]) >= 2, timeout=30,
        )
        if not second_opened:
            with store.connection() as db:
                reservation_states = [tuple(row) for row in db.execute(
                    "SELECT attempt_id, kind, state FROM capacity_reservations WHERE connection_id=?",
                    ("cn_ambiguous",),
                )]
                assignment_failures = [row["payload_json"] for row in db.execute(
                    "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
                    (task_id,),
                )]
            futures = [
                (future.done(), future.cancelled(),
                 type(future.exception()).__name__ if future.done() and not future.cancelled()
                 and future.exception() is not None else None)
                for future in runtime._running.get(task_id, ())
            ]
            pytest.fail(
                f"queued worker did not open: reservations={reservation_states!r}, "
                f"last_attempts={engine.reserve_attempts[-5:]!r}, opens={engine.open_calls!r}, "
                f"cancelled={engine._cancelled_tasks!r}/{engine._cancelled_attempts!r}, "
                f"shutdown={engine._shutdown_event.is_set()}, futures={futures!r}, "
                f"orchestration_thread_alive={thread.is_alive()}, failures={assignment_failures!r}"
            )
        assert latest_attempt_id in engine.open_calls
        assert any(attempt_id == latest_attempt_id and confirmed is not None
                   for attempt_id, confirmed in engine.reserve_attempts)
    finally:
        hold_first.set()
        hold_second.set()
        runtime.shutdown()
        thread.join(timeout=30)
    assert not thread.is_alive()
    with store.connection() as db:
        queued = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'capacity_queued:%'", (task_id,),
        ).fetchone()
    assert queued is not None


def test_normal_turn_marks_delivered_at_once_engine_prompt_returns(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        _publish_current_pipeline(store, channel)
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
            _publish_current_pipeline(store, channel)
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
            refs["channel"] = channel
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
    # The coordinator has no continuation cap (the user's own decision:
    # "хоть миллион"), so its first turn no longer returns while a worker
    # it is waiting on is still running -- it waits, which is the point.
    # The queued sibling therefore has to be observed and cancelled from
    # outside, while that turn is still in flight: the cancel is written
    # exactly as the coordinator's own ``cancel`` tool writes it, and the
    # running turn's own event watcher is what has to act on it.
    turn = {}
    thread = threading.Thread(
        target=lambda: turn.__setitem__(
            "result", runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, threading.Event()),
        ),
    )
    thread.start()
    try:
        assert opened.wait(timeout=30)
        assert _poll(lambda: "A" in refs and "B" in refs)
        queued_title = "B" if opened_title["title"] == "A" else "A"
        queued_ref = refs[queued_title]
        queued_row = _poll(lambda: _fetch_one(
            store, "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (queued_ref,),
        ))
        assert queued_row is not None
        queued_attempt_id = queued_row["attempt_id"]
        assert queued_attempt_id not in engine.open_calls  # genuinely never opened

        start = time.monotonic()
        mcp_tools.dispatch(
            store, refs["channel"], "cancel",
            dict(assignment_ref=queued_ref, reason="no longer needed", request_key="cancelQ"),
        )
        assert _poll(lambda: queued_attempt_id in engine._cancelled_attempts)
        assert time.monotonic() - start < 30  # cancel while queued resolves promptly, not a hang
        # cancel_attempt runs before the durable event append. Wait for the
        # later boundary rather than racing it immediately after observing
        # the engine's cancellation flag.
        assert _poll(lambda: _fetch_one(
            store,
            "SELECT id FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?",
            (task_id, queued_ref),
        ))
        with store.connection() as db:
            leaked = db.execute(
                "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id='cn_assign_cancel' AND attempt_id=?",
                (queued_attempt_id,),
            ).fetchone()
        assert leaked["n"] == 0  # never reserved -- nothing to leak
        assert queued_attempt_id not in engine._owned  # executor thread freed, nothing left owned for it
    finally:
        hold_open.set()
        thread.join(timeout=60)
    assert turn.get("result") == StopReason.END_TURN
    with store.connection() as db:
        cancelled_rows = db.execute(
            "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?",
            (task_id, queued_ref),
        ).fetchone()
    assert cancelled_rows["n"] == 1  # exactly one after all worker activity settled

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


# -- a turn fenced by another client's takeover, not a broken engine --------

def test_a_fenced_coordinator_is_told_it_was_taken_over_not_that_the_engine_broke(tmp_path):
    """Live (2026-09-19, one thread open in two clients): the second client's
    takeover fenced the first coordinator, and the first client was told
    "coordinator engine turn failed (transport_eof)" -- true, and useless to
    whoever reads it. The durable record already knows which of the two it
    was: the task's coordinator attempt is no longer this one."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    with store.connection() as db:
        db.execute("INSERT INTO tasks (id, project_root, created_at) "
                   "VALUES ('t1', ?, '2026-01-01T00:00:00+00:00')", (str(tmp_path),))
        db.execute(
            "INSERT INTO task_orchestration (task_id, state, coordinator_attempt_ref, updated_at) "
            "VALUES ('t1', 'running', 'co_new', '2026-01-01T00:00:00+00:00')")
        db.commit()

    assert runtime._superseded_by_another_coordinator("t1", "co_old") is True
    assert runtime._superseded_by_another_coordinator("t1", "co_new") is False
    # Never guessed from an absent attempt id: that is a plain engine failure.
    assert runtime._superseded_by_another_coordinator("t1", None) is False


def test_the_takeover_message_exists_in_every_language_the_others_do(tmp_path):
    from cortex_runtime.acp import messages as acp_messages

    entry = acp_messages._CATALOG["coordinator_replaced_by_another_client"]
    assert set(entry) == set(acp_messages._CATALOG["engine_failure"])
    for language in entry:
        rendered = acp_messages.render("coordinator_replaced_by_another_client", language)
        assert "{" not in rendered  # no unfilled placeholder
        assert rendered.strip()
