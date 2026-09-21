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
from pathlib import Path

import pytest

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime, PromptOutcome, SecondCoordinatorRefused,
    _accepted_status_correction_needed,
)
from cortex_runtime.acp.orchestrating_runtime import _is_engine_internal_tool
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import DirectProjectWorkspaceProvisioner, IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def test_terminal_worker_card_cannot_restart_from_late_activity_tick():
    """A result update must fence a stale worker activity snapshot.

    The activity watcher and worker-result claim run on different threads.
    If the watcher snapshots ``in_progress`` immediately before the result
    is claimed, sending that snapshot after ``completed`` makes Paseo 0.8
    treat the card as live again and keep its shimmer running.
    """
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_activity = {}
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    cards = []

    runtime._emit_worker_card(cards.append, "assignment-1", status="pending", title="Worker")
    runtime._emit_worker_card(cards.append, "assignment-1", status="in_progress")
    runtime._emit_worker_card(cards.append, "assignment-1", status="completed", entry="Result: done")
    runtime._emit_worker_card(cards.append, "assignment-1", status="in_progress", entry="late activity")

    assert [card["status"] for card in cards] == ["pending", "in_progress", "completed"]
    assert cards[-1]["text"] == "Result: done"


def test_terminal_worker_card_keeps_first_terminal_outcome():
    """A late opposite terminal update must not contradict the durable card."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    cards = []

    runtime._emit_worker_card(cards.append, "assignment-1", status="pending", title="Worker")
    runtime._emit_worker_card(cards.append, "assignment-1", status="failed", entry="Worker failed")
    runtime._emit_worker_card(cards.append, "assignment-1", status="completed", entry="Worker completed")

    assert [card["status"] for card in cards] == ["pending", "failed"]
    assert cards[-1]["text"] == "Worker failed"
    # Paseo 0.8 maps failed timeline error from rawOutput and rejects null.
    # The failure transcript is the same bounded, redacted detail shown in
    # the card, so replay remains truthful and reloadable.
    assert cards[-1]["raw_output"] == cards[-1]["raw_input"] == "Worker failed"


def _worker_card_runtime():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_action_evicted_refs = {}
    runtime._worker_card_project_root = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    return runtime


def test_worker_card_mirror_redacts_external_absolute_path_only_for_display():
    runtime = _worker_card_runtime()
    cards = []

    runtime._emit_worker_card(
        cards.append, "assignment-1", status="in_progress",
        _project_root="/tmp/project", entry="Command — cat /etc/passwd",
    )

    assert cards[-1]["text"] == "Command — cat [external path]"
    assert cards[-1]["raw_input"] == cards[-1]["text"]
    assert "/etc/passwd" not in cards[-1]["text"]


def test_worker_card_mirror_keeps_project_command_and_bounds_rows_and_history():
    runtime = _worker_card_runtime()
    cards = []

    runtime._emit_worker_card(
        cards.append, "assignment-1", status="in_progress",
        _project_root="/tmp/project",
        entry="Command — python3 /tmp/project/tests/test_counter.py",
    )
    assert cards[-1]["text"] == "Command — python3 tests/test_counter.py"
    assert len(cards[-1]["text"]) <= 80

    runtime._emit_worker_card(
        cards.append, "assignment-1", status="in_progress",
        _project_root="/tmp/project", entry="Command — pytest tests/test_counter.py",
    )
    assert all(len(line) <= 80 for line in cards[-1]["text"].splitlines())
    assert len(cards[-1]["raw_input"]) <= 1024 * 1024


def test_worker_action_history_is_complete_and_ref_index_remains_usable():
    runtime = _worker_card_runtime()
    for index in range(60):
        runtime._append_worker_entry("assignment-1", f"Action {index}", f"ref-{index}")

    log = runtime._worker_action_log["assignment-1"]
    refs = runtime._worker_action_refs["assignment-1"]
    assert len(log) == 60
    assert log[0] == "Action 0"
    assert refs["ref-0"] == 0
    assert refs["ref-59"] == 59

    # A late update for a known provider ref replaces its logical row.
    runtime._append_worker_entry("assignment-1", "stale Action 9", "ref-9")
    assert len(runtime._worker_action_log["assignment-1"]) == 60
    assert "stale Action 9" in runtime._worker_action_log["assignment-1"]

    # A late status update for a retained logical call replaces its row and
    # does not create a duplicate or lose any earlier transcript.
    runtime._append_worker_entry("assignment-1", "Action 59 complete", "ref-59")
    assert len(runtime._worker_action_log["assignment-1"]) == 60
    assert runtime._worker_action_log["assignment-1"][-1] == "Action 59 complete"


def test_worker_tool_classifier_hides_host_ui_runtime_but_keeps_project_mcp():
    """Host controls are runtime noise; project MCP remains worker evidence."""
    assert _is_engine_internal_tool(
        "mcp.cua_repl.js — cua_repl.js code=await cua.getState()",
        "execute",
    )
    assert _is_engine_internal_tool("mcp__codex_app__open_in_codex", "execute")
    assert _is_engine_internal_tool("computer.use", "execute")
    assert not _is_engine_internal_tool("mcp.github.search_code", "search")
    assert not _is_engine_internal_tool("mcp.codebase_memory.search", "search")


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_acceptance_reconciles_stale_wait_projection_and_reported_assignment(tmp_path):
    """A delayed accept must publish one coherent terminal projection.

    A worker result can arrive after the coordinator has parked the task on
    ``workers_running``.  Acceptance is authoritative only after the normal
    completion gate, but once it is applied the transient wait marker and
    graph status must not survive into a reloaded capsule.
    """
    store, task_id = _task(tmp_path)
    now = store._now()
    assignment_id = "assignment-reported"
    attempt_id = "attempt-reported"
    failed_assignment_id = "assignment-unreported"
    with store.connection() as db:
        db.execute(
            "INSERT INTO task_orchestration(task_id,state,coordinator_attempt_ref,updated_at,wait_reason,no_progress_count) "
            "VALUES (?,?,?,?,?,?)",
            (task_id, "blocked", None, now, "workers_running", 3),
        )
        db.execute(
            "INSERT INTO work_assignments VALUES (?,?,?,?,?,?,?,?)",
            (assignment_id, task_id, "refresh", "{}", None, "planned", "idem-reported", now),
        )
        db.execute(
            "INSERT INTO work_assignment_attempts VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (attempt_id, task_id, assignment_id, 1, None, None, "{}", "reported", None, "attempt-reported", now),
        )
        db.execute(
            "INSERT INTO work_assignments VALUES (?,?,?,?,?,?,?,?)",
            (failed_assignment_id, task_id, "unreported", "{}", None, "planned", "idem-unreported", now),
        )
        db.commit()

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._lock = threading.RLock()
    runtime._set_state(task_id, "accepted")

    with store.connection() as db:
        projection = db.execute(
            "SELECT state,wait_reason,wait_detail_json,no_progress_count FROM task_orchestration WHERE task_id=?",
            (task_id,),
        ).fetchone()
        assignments = {
            row["assignment_id"]: row["status"]
            for row in db.execute(
                "SELECT assignment_id,status FROM work_assignments WHERE task_id=? ORDER BY assignment_id",
                (task_id,),
            )
        }
    assert dict(projection) == {
        "state": "accepted", "wait_reason": None, "wait_detail_json": None, "no_progress_count": 0,
    }
    assert assignments == {
        assignment_id: "reported", failed_assignment_id: "planned",
    }


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


def _publish_current_pipeline(store, channel, label="Current pipeline"):
    """Publish the coordinator's current pipeline edition before accepting.

    The runtime deliberately creates a bootstrap pipeline before the first
    model turn.  Task-level acceptance requires the coordinator to supersede
    that receipt with its current plan, so successful fake-model scenarios
    must exercise the same publication boundary as a real coordinator.
    """
    draft = store.call(
        "create_draft", dict(template="pipeline", request_key=f"test-pipeline-{uuid.uuid4().hex}"),
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + f"# {label}\n\nThe coordinator's current plan is complete.\n".encode())
    store.call(
        "write_report",
        dict(title=label, summary="Current coordinator pipeline edition.", author="coordinator",
             draft_id=draft["draft_id"], request_key=f"test-pipeline-publish-{uuid.uuid4().hex}"),
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )


def _accept_task(store, channel, rationale, request_key):
    _publish_current_pipeline(store, channel)
    return mcp_tools.dispatch(store, channel, "decide", dict(
        assignment_ref="task", decision="accept", rationale=rationale, request_key=request_key,
    ))


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
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    return runtime, engine


def test_composed_prompt_forwards_mixed_image_blocks_without_text_duplication(tmp_path):
    """The orchestration seam keeps the incoming [text,image,text] order.

    The engine receives Cortex's trusted context followed by the original
    user blocks. The ACP image marker must not be embedded in trusted
    context, and the incoming text must occur once per turn.
    """
    class RecordingSessionRuntime(FakeSessionRuntime):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.prompt_blocks_calls = []

        def prompt(self, session_ref, message, cancel_event, *, prompt_blocks=None,
                   on_delta=None, on_tool_call=None, on_display_update=None):
            self.prompt_blocks_calls.append(prompt_blocks)
            return super().prompt(
                session_ref, message, cancel_event, on_delta=on_delta,
                on_tool_call=on_tool_call, on_display_update=on_display_update,
            )

    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    import base64
    import uuid

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    # The trusted context deliberately contains the same A/B phrase as the
    # current turn. A suffix-based cleanup must preserve this older context
    # while removing only the current user occurrence.
    result = bootstrap_task(store, SourceEvent(origin="acp:image", event_id=str(uuid.uuid4())), "text A\ntext B", "owner")
    seen = []

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(_store, _channel, _ref, message, call_index):
            seen.append(message)
            return PromptOutcome(StopReason.END_TURN, "done")
        return behavior

    engine = RecordingSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)),
        FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
    )
    image = dict(type="image", data=base64.b64encode(b"image-bytes").decode(), mimeType="image/png")
    first = [dict(type="text", text="text A"), image, dict(type="text", text="text B")]
    followup = [dict(type="text", text="followup A"), image, dict(type="text", text="followup B")]
    try:
        runtime.start_prompt(result.task_id, "s1", "text A\ntext B", lambda _text, **_kw: True,
                             threading.Event(), prompt_blocks=first)
        runtime.start_prompt(result.task_id, "s1", "followup A\nfollowup B", lambda _text, **_kw: True,
                             threading.Event(), prompt_blocks=followup)
    finally:
        runtime.shutdown()

    assert len(engine.prompt_blocks_calls) == 2
    for blocks, left, right in zip(engine.prompt_blocks_calls, ("text A", "followup A"), ("text B", "followup B")):
        assert blocks is not None
        assert [block["type"] for block in blocks] == ["text", "text", "image", "text"]
        assert blocks[2] == image
        assert "[image:" not in blocks[0]["text"]
        assert blocks[1] == {"type": "text", "text": left}
        assert blocks[3] == {"type": "text", "text": right}
        assert f"User message: {left}\n{right}" in blocks[0]["text"]
        assert blocks[1]["text"] == left
        assert blocks[3]["text"] == right


def test_same_turn_accept_refusal_delivers_pending_events_next_turn(tmp_path):
    """A pending-event refusal ends the turn before a redundant retry."""
    store, task_id = _task(tmp_path)
    correction = (
        "Task-level accept refused: unconsumed meaningful event(s) remain for this task: progress. "
        "Finish this coordinator turn without retrying acceptance; the runtime will deliver those events."
    )

    def router(_store, channel, _session_ref, _launch, _instructions):
        def coordinator(_store, _channel, _ref, message, call_index, *, on_tool_call, cancel_event):
            if call_index == 0:
                _publish_current_pipeline(store, channel)
                delegation_events.append_event(
                    store, task_id, "progress", "assignment-accept-followup:test",
                    dict(kind="assignment_accept_requires_task", assignment_ref="test"),
                    origin="runtime", assignment_ref="test",
                )
                with pytest.raises(StoreError) as refusal:
                    mcp_tools.dispatch(store, channel, "decide", dict(
                        assignment_ref="task", decision="accept", rationale="Done.",
                        request_key="premature",
                    ))
                assert str(refusal.value) == "acceptance_refused"
                ref = "tc_refused"
                on_tool_call(ref, "execute", "mcp.cortex.decide", "in_progress")
                on_tool_call(ref, "execute", None, "failed", None, None, None, dict(
                    result=dict(content=[dict(type="text", text=json.dumps(dict(
                        error="acceptance_refused", correction=correction,
                    )))]),
                ))
                assert cancel_event.is_set()
                return PromptOutcome(StopReason.CANCELLED, "")
            assert call_index == 1
            assert "Assignment test was accepted as a worker result" in message
            assert not cancel_event.is_set()
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="accept", rationale="Pending event reconciled.",
                request_key="after-delivery",
            ))
            return PromptOutcome(StopReason.END_TURN, "Finished.")
        return coordinator

    runtime, engine = _make_runtime(store, router)
    try:
        outcome = runtime.start_prompt(task_id, "session", "Build a page.", lambda _text, **_kw: True,
                                       threading.Event())
        assert outcome == StopReason.END_TURN
        assert len(engine.prompt_calls) == 2
        with store.connection() as db:
            assert db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()[0] == "accepted"
            progress = db.execute(
                "SELECT consumed_by,delivered_at FROM task_events WHERE task_id=? AND dedup_key=?",
                (task_id, "assignment-accept-followup:test"),
            ).fetchone()
            assert progress["consumed_by"] and progress["delivered_at"]
    finally:
        runtime.shutdown()


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
            _accept_task(store, channel, "Both assignments delivered acceptable results.", "acceptall")
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
    # One initial turn plus at most a small, bounded number of continuations.
    assert 2 <= coordinator_calls <= 4


def test_accepted_final_corrects_stale_blocked_status_claim(tmp_path):
    """The accepted host stream must resolve a stale pre-accept projection.

    The coordinator can have read ``workers_running`` immediately before the
    worker result/decision boundary.  Once the durable gate accepts the task,
    append the canonical status correction after committing ``accepted``.
    """
    store, task_id = _task(tmp_path)

    def coordinator(store, channel, session_ref, message, call_index):
        if call_index == 0:
            _accept_task(store, channel, "Complete.", "accept-stale-status")
            return PromptOutcome(
                StopReason.END_TURN,
                "The task is accepted, but the state projection still reports blocked; I cannot honestly claim finalization.",
            )
        raise AssertionError("accepted task must not receive another coordinator turn")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator if channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    try:
        assert runtime.start_prompt(task_id, "s1", "Finish the task.", updates.append, threading.Event()) == StopReason.END_TURN
    finally:
        runtime.shutdown()

    with store.connection() as db:
        row = db.execute(
            "SELECT state,wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    assert row["state"] == "accepted"
    assert row["wait_reason"] is None
    assert any("the task is accepted, but the state projection" in value.lower() for value in updates)
    assert updates[-1] == "Cortex status: the task is accepted. The preceding wait/status text was stale."


def test_accepted_status_correction_ignores_unrelated_blocked_text():
    """A blocked file or user action is not a stale task projection claim."""
    assert _accepted_status_correction_needed(
        "The file was blocked by the user, but the task is accepted."
    ) is False
    assert _accepted_status_correction_needed(
        "The state projection still reports blocked while the task is accepted."
    ) is True


def test_completion_gate_rejects_failed_assignment_without_verified_result(tmp_path):
    """A failed worker route cannot be converted into coordinator success.

    In particular, a coordinator may have produced a plausible final message
    or edited a private session workspace after the worker failed.  Neither is
    a durable worker result, candidate integration, or check evidence, so the
    task-level accept gate must remain closed.
    """
    from cortex_runtime.delegation import completion as delegation_completion

    store, task_id = _task(tmp_path)
    assignment_ref = "failed-assignment"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate:failed-assignment",
        {"assignment_ref": assignment_ref, "title": "Repair counter"},
        origin="coordinator", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "assignment_failed", "assignment_failed:failed-assignment",
        {"error": "engine_launch_command_missing"},
        origin="runtime", assignment_ref=assignment_ref,
    )

    ok, reasons = delegation_completion.gate(store, task_id)

    assert not ok
    assert any("assignment_failed" in reason for reason in reasons)


def test_completion_gate_does_not_treat_failed_then_cancelled_as_success(tmp_path):
    """Cancellation after a launch failure does not erase the failure."""
    from cortex_runtime.delegation import completion as delegation_completion

    store, task_id = _task(tmp_path)
    assignment_ref = "failed-then-cancelled"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate:failed-then-cancelled",
        {"assignment_ref": assignment_ref}, origin="coordinator", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "assignment_failed", "assignment_failed:failed-then-cancelled",
        {"error": "engine_not_connected"}, origin="runtime", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "assignment_cancelled", "assignment_cancelled:failed-then-cancelled",
        {"reason": "The coordinator completed the work directly."},
        origin="runtime", assignment_ref=assignment_ref,
    )

    ok, reasons = delegation_completion.gate(store, task_id)

    assert not ok
    assert any("assignment_failed" in reason for reason in reasons)


def test_failed_assignment_final_prose_is_deferred_until_durable_acceptance(tmp_path):
    """A model's claim remains prose; a failed assignment blocks acceptance."""
    store, task_id = _task(tmp_path)
    assignment_ref = "failed-stream"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate:failed-stream",
        {"assignment_ref": assignment_ref}, origin="coordinator", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "assignment_failed", "assignment_failed:failed-stream",
        {"error": "engine_not_connected"}, origin="runtime", assignment_ref=assignment_ref,
    )

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "I fixed counter.py and pytest passed.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    runtime.start_prompt(task_id, "s1", "Continue.", updates.append, threading.Event())

    streamed = "".join(value for value in updates if isinstance(value, str))
    assert "I fixed counter.py" in streamed
    assert "Acceptance refused" not in streamed
    with store.connection() as db:
        assert db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"] == "idle_awaiting_user"
        assert db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchone()[0] == 1
        assert db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='coordinator_decision'",
            (task_id,),
        ).fetchone()[0] == 0


def test_same_turn_failed_delegate_cannot_stream_final_claim(tmp_path):
    """A failed same-turn delegation cannot make the task accepted."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Repair counter", brief="repair it", role="totally_bogus_role",
                acceptance="the test passes", request_key="same-turn-failure",
            ))
        return PromptOutcome(StopReason.END_TURN, "The repair is complete and verified.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    runtime.start_prompt(task_id, "s1", "Repair it.", updates.append, threading.Event())

    streamed = "".join(value for value in updates if isinstance(value, str))
    assert "The repair is complete" in streamed
    assert "Acceptance refused" not in streamed
    with store.connection() as db:
        assert db.execute(
            "SELECT state FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()[0] != "accepted"
        assert db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchone()[0] >= 1
        assert db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='coordinator_decision'",
            (task_id,),
        ).fetchone()[0] == 0


def test_candidate_is_staged_reviewed_then_integrated(tmp_path, monkeypatch):
    from cortex_runtime.delegation import candidate_checks

    monkeypatch.setattr(candidate_checks, "_execute_bounded", lambda *_args, **_kwargs: {
        "status": "completed", "exit_code": 0, "output_digest": "observed-output",
        "output_bytes": 6, "output_truncated": False,
    })
    (tmp_path / "note.txt").write_text("before\n")
    store, task_id = _task(tmp_path)
    state = {}

    def coordinator(store, channel, session_ref, message, call_index):
        if "delegate_ref" not in state:
            state["delegate_ref"] = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Edit note", brief="Change note.txt", role="backend-dev",
                acceptance="note.txt contains after", request_key="edit-note",
            ))["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "result" in message and "candidate_id" not in state:
            state["candidate_id"] = mcp_tools.dispatch(store, channel, "prepare_candidate", dict(
                assignment_refs=[state["delegate_ref"]], request_key="stage-note",
            ))["candidate_id"]
            return PromptOutcome(StopReason.END_TURN, "Staging.")
        if "Candidate" in message and "ready" in message and "check_ref" not in state:
            with store.connection() as db:
                row = db.execute("SELECT content_digest FROM candidate_staging WHERE candidate_id=?",
                                 (state["candidate_id"],)).fetchone()
            assert row is not None
            state["check_ref"] = mcp_tools.dispatch(store, channel, "request_check", dict(
                candidate_ref=state["candidate_id"], candidate_digest=row["content_digest"],
                argv=["/usr/bin/cat", "note.txt"], request_key="check-note",
            ))["check_ref"]
            return PromptOutcome(StopReason.END_TURN, "Checking candidate.")
        if "Candidate check" in message and "observed" in message and "integrated" not in state:
            check = mcp_tools.dispatch(store, channel, "read_check", dict(
                check_ref=state["check_ref"],
            ))
            assert check["status"] == "completed"
            assert check["result"]["observed_result"]["exit_code"] == 0
            mcp_tools.dispatch(store, channel, "assess_check", dict(
                check_ref=state["check_ref"], assessment="pass",
                rationale="The exact staged note was readable by the check command.",
                request_key="assess-note",
            ))
            with store.connection() as db:
                row = db.execute("SELECT project_base_digest FROM candidate_staging WHERE candidate_id=?",
                                 (state["candidate_id"],)).fetchone()
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=state["delegate_ref"], candidate_ref=state["candidate_id"],
                expected_base_digest=row["project_base_digest"], request_key="apply-note",
            ))
            state["integrated"] = True
            return PromptOutcome(StopReason.END_TURN, "Applying.")
        if "Integration" in message and "applied" in message:
            _accept_task(store, channel, "Reviewed staged note and applied it.", "accept-note")
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker(store, channel, session_ref, message, call_index):
        with store.connection() as db:
            row = db.execute("SELECT root_path FROM workspace_snapshots WHERE attempt_id=?",
                             (channel.attempt_id,)).fetchone()
        Path(row["root_path"], "note.txt").write_text("after\n")
        _publish(store, channel, "Edit note", call_index)
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator if channel.role == "coordinator" else worker

    runtime, _engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Change note.", lambda _: True, threading.Event())
    assert result == StopReason.END_TURN
    assert (tmp_path / "note.txt").read_text() == "after\n"
    with store.connection() as db:
        row = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "accepted"


def test_revise_dispatches_a_new_attempt_with_previous_report(tmp_path):
    store, task_id = _task(tmp_path)
    state = {}

    def coordinator(store, channel, session_ref, message, call_index):
        if "first_ref" not in state:
            state["first_ref"] = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Check", brief="Inspect the project", role="general", acceptance="Report findings",
                request_key="first-check",
            ))["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "first finished" in message and "continuation_ref" not in state:
            state["continuation_ref"] = mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref=state["first_ref"], decision="revise",
                rationale="Add the missing finding", revision_brief="Include the second finding",
                request_key="revise-check",
            ))["continuation_ref"]
            return PromptOutcome(StopReason.END_TURN, "Revision requested.")
        if "second finished" in message:
            _accept_task(store, channel, "Both reports retained.", "accept-check")
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker(store, channel, session_ref, message, call_index):
        with store.connection() as db:
            row = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?",
                             (channel.attempt_id,)).fetchone()
        is_second = row["idempotency_key"] == state.get("continuation_ref")
        if is_second:
            assert "Include the second finding" in message
            assert "Review previous report" in message
        _publish(store, channel, "second" if is_second else "first", call_index)
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator if channel.role == "coordinator" else worker

    runtime, _engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Check.", lambda _: True, threading.Event())
    assert result == StopReason.END_TURN
    assert state["continuation_ref"] != state["first_ref"]
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='worker_result'",
                          (task_id,)).fetchone()[0] == 2


def test_planned_successor_starts_from_exact_predecessor_candidate(tmp_path):
    store, task_id = _task(tmp_path)
    state = {"phase": "start"}

    def coordinator(store, channel, session_ref, message, call_index):
        phase = state["phase"]
        if phase == "start":
            requirement = mcp_tools.dispatch(store, channel, "update_plan", dict(
                action="create_requirement", statement="Create parent and child files",
                criteria=["Both files exist"], request_key="req-files",
            ))
            state["requirement"] = requirement["requirement_id"]
            parent = mcp_tools.dispatch(store, channel, "update_plan", dict(
                action="add_assignment", node_key="parent",
                requirement_bindings=[dict(requirement_ref=state["requirement"], revision="1")],
                request_key="plan-parent",
            ))
            state["parent_plan"] = parent["assignment_id"]
            state["parent_ref"] = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Parent", brief="Write parent.txt", role="backend-dev",
                acceptance="Parent file exists", plan_assignment_ref=state["parent_plan"],
                request_key="delegate-parent",
            ))["assignment_ref"]
            state["phase"] = "parent_running"
            return PromptOutcome(StopReason.END_TURN, "Parent delegated.")
        if phase == "parent_running" and "parent finished" in message:
            state["parent_candidate"] = mcp_tools.dispatch(store, channel, "prepare_candidate", dict(
                assignment_refs=[state["parent_ref"]], request_key="candidate-parent",
            ))["candidate_id"]
            state["phase"] = "parent_staging"
            return PromptOutcome(StopReason.END_TURN, "Staging parent.")
        if phase == "parent_staging" and "Candidate" in message and "ready" in message:
            bound = mcp_tools.dispatch(store, channel, "update_plan", dict(
                action="bind_candidate", assignment_ref=state["parent_plan"],
                candidate_ref=state["parent_candidate"], request_key="bind-parent",
            ))
            child = mcp_tools.dispatch(store, channel, "update_plan", dict(
                action="add_assignment", node_key="child",
                requirement_bindings=[dict(requirement_ref=state["requirement"], revision="1")],
                predecessors=[dict(assignment_ref=state["parent_plan"], candidate_ref=bound["candidate_ref"])],
                request_key="plan-child",
            ))
            state["child_ref"] = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Child", brief="Write child.txt using parent.txt", role="backend-dev",
                acceptance="Child file exists", plan_assignment_ref=child["assignment_id"],
                request_key="delegate-child",
            ))["assignment_ref"]
            state["phase"] = "child_running"
            return PromptOutcome(StopReason.END_TURN, "Child delegated.")
        if phase == "child_running" and "child finished" in message:
            state["child_candidate"] = mcp_tools.dispatch(store, channel, "prepare_candidate", dict(
                assignment_refs=[state["child_ref"]], request_key="candidate-child",
            ))["candidate_id"]
            state["phase"] = "child_staging"
            return PromptOutcome(StopReason.END_TURN, "Staging child.")
        if phase == "child_staging" and "Candidate" in message and "ready" in message:
            with store.connection() as db:
                row = db.execute("SELECT project_base_digest FROM candidate_staging WHERE candidate_id=?",
                                 (state["child_candidate"],)).fetchone()
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=state["child_ref"], candidate_ref=state["child_candidate"],
                expected_base_digest=row["project_base_digest"], request_key="integrate-child",
            ))
            state["phase"] = "integrating"
            return PromptOutcome(StopReason.END_TURN, "Applying candidate.")
        if phase == "integrating" and "Integration" in message and "applied" in message:
            _accept_task(store, channel, "Exact dependent candidate applied.", "accept-dependent")
            state["phase"] = "done"
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker(store, channel, session_ref, message, call_index):
        with store.connection() as db:
            row = db.execute("SELECT root_path FROM workspace_snapshots WHERE attempt_id=?",
                             (channel.attempt_id,)).fetchone()
        root = Path(row["root_path"])
        if "Parent" in message:
            (root / "parent.txt").write_text("parent\n")
            _publish(store, channel, "parent", call_index)
        else:
            assert (root / "parent.txt").read_text() == "parent\n"
            assert "Current structured task context" in message
            (root / "child.txt").write_text("child\n")
            _publish(store, channel, "child", call_index)
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator if channel.role == "coordinator" else worker

    runtime, _engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Create both files.", lambda _: True, threading.Event())
    assert result == StopReason.END_TURN
    assert state["phase"] == "done", [(e.event_type, e.payload) for e in delegation_events.list_since(store, task_id)]
    assert (tmp_path / "parent.txt").read_text() == "parent\n"
    assert (tmp_path / "child.txt").read_text() == "child\n"
    with store.connection() as db:
        attempts = db.execute(
            "SELECT status FROM work_assignment_attempts WHERE task_id=? ORDER BY attempt_no,created_at",
            (task_id,),
        ).fetchall()
    assert [row["status"] for row in attempts] == ["reported", "reported"]


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
            _accept_task(store, channel, "User chose staging; nothing left to do.", "accept-after-user")
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
    # A worker that ends its turn having produced nothing is over, and
    # says so; the turn ends because the assignment closed, never because
    # the coordinator ran out of patience with work still open (a worker
    # may legitimately take hours).
    with store.connection() as db:
        failure = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchone()
    assert failure is not None and "worker_ended_without_result" in failure["payload_json"]


def test_empty_worker_turn_gets_one_same_session_publication_followup(tmp_path):
    """A clean worker END_TURN is recoverable once in its original session."""
    store, task_id = _task(tmp_path)
    worker_calls = []

    def coordinator(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        _accept_task(store, channel, "Worker published the requested report.", "acceptX")
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def worker(store, channel, session_ref, message, call_index):
        worker_calls.append((session_ref, message))
        if len(worker_calls) == 1:
            return PromptOutcome(StopReason.END_TURN, "I should publish the report.")
        _publish(store, channel, "Assignment X", call_index)
        return PromptOutcome(StopReason.END_TURN, "Published the report.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator if channel and channel.role == "coordinator" else worker

    runtime, _engine = _make_runtime(store, router)
    assert runtime.start_prompt(task_id, "s1", "Do X.", lambda _text: True, threading.Event()) == StopReason.END_TURN

    assert len(worker_calls) == 2
    assert worker_calls[0][0] == worker_calls[1][0]
    assert "Runtime completion follow-up" in worker_calls[1][1]
    assert "assignment's deliverables and acceptance criteria" in worker_calls[1][1]
    assert "checks actually run" in worker_calls[1][1]
    assert "remaining gaps or unrun checks" in worker_calls[1][1]
    with store.connection() as db:
        failures = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,),
        ).fetchall()
        recovery = db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND dedup_key=?",
            (task_id, "worker_recovery:" + db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE task_id=?", (task_id,)
            ).fetchone()[0]),
        ).fetchone()[0]
    assert failures == []
    assert recovery == 1
    assert not runtime._worker_turns_in_flight


def test_stale_recovery_marker_with_absent_attempt_is_orphaned(tmp_path):
    """A restart cannot prompt an ACP session whose owned attempt is gone."""
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    assignment_ref = "orphaned-assignment"
    delegation_events.append_event(
        store, task_id, "progress", f"worker_recovery:{assignment_ref}",
        {"kind": "worker_same_session_recovery", "assignment_ref": assignment_ref},
        origin="runtime", assignment_ref=assignment_ref,
    )

    assert runtime._claim_worker_same_session_recovery(task_id, assignment_ref) == "orphaned"


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
        event_count = db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0]
        failure = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchone()
    # The silent worker's assignment closed itself; the empty coordinator
    # turns are what the retry bound above covers.
    assert failure is not None and "worker_ended_without_result" in failure["payload_json"]
    assert event_count > 0  # history is retained, not discarded, after the turn ends
    reader_channel = _resolve_owner_channel(store, task_id)
    events = mcp_tools.dispatch(store, reader_channel, "list_events", {})
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
            _accept_task(store, channel, "Q result accepted.", "acceptQ")
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
        _accept_task(store, channel, "Nothing else pending.", "acc1")
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


# -- T035: worker-assignment ACP tool-call cards --------------------------


def test_delegation_produces_stable_card_updated_through_running_and_completed(tmp_path):
    """One ACP tool-call card per delegated assignment: created once with a
    stable id (derived from the assignment's own durable ref), and every
    later lifecycle event (running, then finished) updates that SAME card
    -- never creates a second one."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Add a cache", brief="add a cache to the repository", role="backend-dev",
                acceptance="cache added", request_key="reqA",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "finished" in message:
            _accept_task(store, channel, "Done.", "acc1")
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "Cache implementation finished.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    cards = []
    result = runtime.start_prompt(
        task_id, "s1", "Add a cache.", lambda t: True, threading.Event(), emit_tool_call=cards.append,
    )
    assert result == StopReason.END_TURN

    cards = [card for card in cards if card["tool_call_id"].startswith("worker:")]
    ids = {c["tool_call_id"] for c in cards}
    assert len(ids) == 1  # exactly one card, one stable id, for the one assignment
    creates = [c for c in cards if c.get("new")]
    assert len(creates) == 1  # the card is created exactly once
    assert creates[0]["status"] == "pending"
    # Not "execute": a worker assignment is not a command being run, and a
    # client rendered it under a "Run Command" heading that described
    # nothing that was happening. The pinned schema has no kind for
    # delegation, so the catch-all is the honest one.
    assert creates[0]["kind"] == "other"
    updates = [c for c in cards if not c.get("new")]
    assert updates[0]["status"] == "in_progress"
    assert updates[-1]["status"] == "completed"
    assert sum(c.get("status") == "completed" for c in updates) == 1
    # T035 requirement 3: the running/finished title is re-issued from the
    # RESOLVED role recorded on the provisioning row, not left at whatever
    # hint the coordinator typed -- asserted here by the update carrying a
    # title at all (the create already carried the hint's).
    #
    # The role is rendered for a reader, so the storage prefix every
    # canonical id carries ("worker-backend-dev") never reaches a card
    # already titled "Worker:" -- see _display_role. Every alias in
    # portable.roles maps stem -> "worker-" + stem, so once that prefix is
    # stripped the resolved id and the hint are the same string: this
    # assertion can no longer distinguish them by text, and does not
    # pretend to. What it does still catch is the prefix leaking back into
    # a title.
    running = updates[0]
    assert running.get("title") is not None
    assert "worker-" not in running["title"]
    assert "Worker: backend-dev" in running["title"]
    assert "Add a cache" in running["title"]
    completed = updates[-1]
    assert completed["status"] == "completed"
    assert "[final response] Cache implementation finished." in completed["text"]
    assert "Report:" not in completed["text"]
    assert "Queued:" not in completed["text"]
    assert "Provisioning:" not in completed["text"]
    with store.connection() as db:
        result_count = db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task_id,),
        ).fetchone()[0]
    assert result_count == 1


def test_cancelled_assignment_renders_as_failed_card_with_reason(tmp_path):
    """T035: the pinned ACP schema's ToolCallStatus has no 'cancelled'
    value -- a cancelled assignment must render as a 'failed' card, with
    the cancellation reason carried in its content so it stays
    distinguishable from an ordinary engine/refusal failure by its text.
    Mirrors ``test_two_workers_steering_cancels_one_other_finishes``
    (steering cancels X via a second /prompt turn; Y finishes normally)."""
    store, task_id = _task(tmp_path)
    phase = {"delegated": False, "cancelled_x": False, "accepted": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["delegated"]:
            phase["delegated"] = True
            mcp_tools.dispatch(store, channel, "delegate", dict(title="X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Y", brief="do Y", role="backend-dev", acceptance="Y done", request_key="reqY"))
            return PromptOutcome(StopReason.END_TURN, "delegated X and Y")
        if "no longer needed" in message and not phase["cancelled_x"]:
            phase["cancelled_x"] = True
            with store.connection() as db:
                row = db.execute(
                    "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested' "
                    "AND payload_json LIKE '%\"title\": \"X\"%'", (task_id,),
                ).fetchone()
            mcp_tools.dispatch(store, channel, "cancel", dict(
                assignment_ref=row["assignment_ref"], reason="no longer needed", request_key="cancelX",
            ))
            return PromptOutcome(StopReason.END_TURN, "cancelled X")
        if phase["cancelled_x"] and not phase["accepted"]:
            from cortex_runtime.delegation import completion as delegation_completion

            _publish_current_pipeline(store, channel)
            ok, _reasons = delegation_completion.gate(store, task_id)
            if ok:
                phase["accepted"] = True
                mcp_tools.dispatch(store, channel, "decide", dict(
                    assignment_ref="task", decision="accept", rationale="Y delivered; X cancelled.", request_key="acc1",
                ))
                return PromptOutcome(StopReason.END_TURN, "accepted")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        if title == "X":
            # Never publishes -- represents a worker cancelled before finishing.
            return PromptOutcome(StopReason.END_TURN, "")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    cards = []
    result1 = runtime.start_prompt(task_id, "s1", "Do X and Y.", lambda t: True, threading.Event(), emit_tool_call=cards.append)
    assert result1 == StopReason.END_TURN
    result2 = runtime.start_prompt(task_id, "s1", "X is no longer needed.", lambda t: True, threading.Event(), emit_tool_call=cards.append)
    assert result2 == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"

    cancelled_updates = [c for c in cards if c.get("status") == "failed" and "Cancelled" in (c.get("text") or "")]
    assert len(cancelled_updates) == 1
    assert "no longer needed" in cancelled_updates[0]["text"]
    # It updated the SAME card the delegation created, never a new one.
    created_ids = {c["tool_call_id"] for c in cards if c.get("new") and c["tool_call_id"].startswith("worker:")}
    assert cancelled_updates[0]["tool_call_id"] in created_ids
    assert len(created_ids) == 2  # one card each for X and Y


def test_stop_terminalizes_pending_worker_card_once(tmp_path):
    """A task-wide stop closes an already-emitted card immediately.

    ``/cortex-stop`` can run after the coordinator turn returned, so no
    continuation is guaranteed to claim the durable cancellation event. The
    terminal update must still reuse the original card id and be idempotent
    for the later event-claim pass.
    """
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    cards = []
    assignment_ref = "assignment-pending-stop"
    runtime._emit_worker_card(
        cards.append, assignment_ref, title="Worker: backend-dev — inspect",
        kind="other", status="pending",
    )
    # This is the state after the coordinator prompt has returned: the
    # per-task emitter remains available for background worker lifecycle
    # updates until stop/terminal cleanup.
    runtime._worker_card_emitters[assignment_ref] = cards.append
    delegation_events.append_event(
        store, task_id, "delegation_requested", "request-pending-stop",
        {"title": "inspect", "role": "backend-dev"}, origin="coordinator",
        assignment_ref=assignment_ref,
    )

    assert runtime.stop_task(task_id, "s1", lambda _text: True) is True

    terminal = [card for card in cards if card.get("status") == "failed"]
    assert len(terminal) == 1
    assert terminal[0]["tool_call_id"] == cards[0]["tool_call_id"]
    assert "Cancelled: urgent stop (" in terminal[0]["text"]
    with store.connection() as db:
        rows = db.execute(
            "SELECT event_type FROM task_events WHERE task_id=? AND assignment_ref=?",
            (task_id, assignment_ref),
        ).fetchall()
    assert [row["event_type"] for row in rows] == ["delegation_requested", "assignment_cancelled"]


def test_stop_reconciles_durable_worker_result_to_completed_card(tmp_path):
    """A stop racing a published result must close the same card as complete."""
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    cards = []
    assignment_ref = "assignment-result-before-stop"
    runtime._emit_worker_card(
        cards.append, assignment_ref, title="Worker: backend-dev — inspect",
        kind="other", status="in_progress", entry="Tool: Publish result — done",
    )
    runtime._worker_card_emitters[assignment_ref] = cards.append
    delegation_events.append_event(
        store, task_id, "delegation_requested", "request-result-before-stop",
        {"title": "inspect", "role": "backend-dev"}, origin="coordinator",
        assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "result-before-stop",
        {"summary": "Published report", "checks": []}, origin="runtime",
        assignment_ref=assignment_ref,
    )

    assert runtime.stop_task(task_id, "s1", lambda _text: True) is True
    terminal = [card for card in cards if card.get("status") == "completed"]
    assert len(terminal) == 1
    assert terminal[0]["tool_call_id"] == cards[0]["tool_call_id"]
    assert "Published report" in terminal[0]["text"]
    with store.connection() as db:
        cancelled = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled'",
            (task_id,),
        ).fetchone()
    assert cancelled is None


def test_stop_result_precedence_survives_prior_cancellation_event(tmp_path):
    """A later observed result wins a prior stop cancellation for the card."""
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    cards = []
    assignment_ref = "assignment-stop-before-result"
    runtime._emit_worker_card(
        cards.append, assignment_ref, title="Worker: backend-dev — inspect",
        kind="other", status="in_progress", entry="Tool: Publish result — done",
    )
    runtime._worker_card_emitters[assignment_ref] = cards.append
    delegation_events.append_event(
        store, task_id, "delegation_requested", "request-stop-before-result",
        {"title": "inspect", "role": "backend-dev"}, origin="coordinator",
        assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "assignment_cancelled", "cancel-before-result",
        {"reason": "urgent stop (/cortex-stop)"}, origin="runtime", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "result-after-stop",
        {"summary": "Published report", "checks": []}, origin="runtime", assignment_ref=assignment_ref,
    )

    assert runtime.stop_task(task_id, "s1", lambda _text: True) is True
    terminal = [card for card in cards if card.get("status") in {"completed", "failed"}]
    assert terminal[-1]["status"] == "completed"
    assert "Published report" in terminal[-1]["text"]


def test_worker_card_emitter_stays_bound_after_session_reconnect():
    """A reconnect cannot receive updates for a card created by the old session."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_card_emitters = {"assignment-1": "old-session-feed"}
    runtime._task_tool_call_emitters = {"task-1": "new-session-feed"}

    assert runtime._worker_card_emitter("task-1", "assignment-1") == "old-session-feed"
    assert runtime._worker_card_emitter("task-1", "assignment-2") == "new-session-feed"


def test_stop_from_reconnected_session_uses_original_card_emitter(tmp_path):
    """A current session emitter cannot terminalize an old session's card."""
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    old_cards, new_cards = [], []
    assignment_ref = "assignment-old-session"
    runtime._emit_worker_card(
        old_cards.append, assignment_ref, title="Worker: backend-dev — inspect",
        kind="other", status="pending",
    )
    runtime._worker_card_emitters[assignment_ref] = old_cards.append

    runtime._cancel_assignment(
        task_id, assignment_ref, "stop from reconnect",
        emit_tool_call=new_cards.append,
    )

    assert [card["status"] for card in old_cards] == ["pending", "failed"]
    assert new_cards == []


def test_refusal_before_execution_still_produces_a_failed_card(tmp_path):
    """T035 requirement 6: an assignment refused before it ever ran (here,
    an unknown role) must still be visible as a card, in 'failed', with the
    actionable reason -- exactly as visible as the plain-text line commit
    cf6e491 introduced, now carried by the card instead of duplicated as
    text (see OrchestratingRuntime._continuation_loop)."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="totally_bogus_role",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept",
            rationale="Delegation was refused; did the work myself.", request_key="acceptX",
        ))
        return PromptOutcome(StopReason.END_TURN, "Did the work myself.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise AssertionError("a refused delegation must never launch a worker")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates, cards = [], []
    with pytest.raises(StoreError, match="acceptance_refused"):
        runtime.start_prompt(
            task_id, "s1", "Do X.", updates.append, threading.Event(), emit_tool_call=cards.append,
        )
    # The crude text line (cf6e491) is NOT duplicated once a card channel
    # is wired -- the card is the one, non-duplicated, client-visible
    # record of the refusal.
    assert not any("was NOT delegated" in u for u in updates)
    created = [c for c in cards if c.get("new") and c["tool_call_id"].startswith("worker:")]
    assert len(created) == 1
    assert "totally_bogus_role" in created[0]["title"]  # raw hint: never resolved
    failed = [c for c in cards if c.get("status") == "failed"]
    assert len(failed) == 1
    assert failed[0]["tool_call_id"] == created[0]["tool_call_id"]
    assert "unknown_role" in failed[0]["text"]
    assert "worker-backend-dev" in failed[0]["text"]  # the actionable remediation detail


def test_emit_tool_call_none_falls_back_to_plain_text_refusal(tmp_path):
    """A caller that never wires emit_tool_call (every pre-T035 test/
    caller) must keep seeing the pre-T035 plain-text refusal line -- the
    card path never silently drops the one thing cf6e491 made visible."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="totally_bogus_role",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept",
            rationale="Delegation was refused; did the work myself.", request_key="acceptX",
        ))
        return PromptOutcome(StopReason.END_TURN, "Did the work myself.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise AssertionError("a refused delegation must never launch a worker")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates = []
    with pytest.raises(StoreError, match="acceptance_refused"):
        runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    assert any("was NOT delegated" in u for u in updates)


# -- T035 FINDING-12 closure: the coordinator-turn workspace backstop -----


def test_coordinator_turn_that_writes_to_the_project_is_blocked(tmp_path):
    """Engine-independent backstop: even though this fake engine never goes
    through engine.permissions.PermissionBridge at all (it calls
    delegation.mcp_tools.dispatch directly, simulating whatever an engine's
    own native tool did), a coordinator turn that wrote into the shared
    project workspace is still detected and the turn is failed -- never
    silently accepted as an ordinary END_TURN."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        (tmp_path / "hack.txt").write_text("the coordinator edited this\n")
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise AssertionError("no delegation happens in this test")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert any("changed the project workspace" in u for u in updates)
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "blocked"


@pytest.mark.parametrize("coordinator_writes_after_publication", [False, True])
def test_direct_worker_publication_during_coordinator_turn_is_not_misattributed(
    tmp_path, coordinator_writes_after_publication,
):
    """A direct worker can finish before the delegating coordinator turn ends."""
    store, task_id = _task(tmp_path)
    project_file = tmp_path / "counter.py"
    project_file.write_text("return 7\n")
    worker_published = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Fix counter", brief="Return six", role="backend-dev",
                acceptance="Counter returns six", request_key="direct-worker",
            ))
            assert worker_published.wait(15), "direct worker did not publish during the turn"
            if coordinator_writes_after_publication:
                project_file.write_text("return 8\n")
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        _accept_task(store, channel, "Observed the worker result.", "direct-accept")
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        project_file.write_text("return 6\n")
        _publish(store, channel, "counter", call_index)
        worker_published.set()
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, DirectProjectWorkspaceProvisioner(store, str(tmp_path)),
        FakeConfigSnapshotSource(), allow_unqualified_delivery=True, wait_timeout_seconds=20.0,
    )
    updates = []
    cancel_event = threading.Event()
    outcomes = []
    failures = []

    def run_prompt():
        try:
            outcomes.append(runtime.start_prompt(
                task_id, "s1", "Fix counter.", updates.append, cancel_event,
            ))
        except BaseException as exc:
            failures.append(exc)

    driver = threading.Thread(target=run_prompt, daemon=True)
    driver.start()
    driver.join(timeout=10)
    if driver.is_alive():
        cancel_event.set()
        runtime._notify()
        driver.join(timeout=2)
        pytest.fail("coordinator waited indefinitely after the direct worker's terminal publication")
    assert not failures, failures
    assert outcomes == [StopReason.END_TURN]
    assert project_file.read_text() == ("return 8\n" if coordinator_writes_after_publication else "return 6\n")
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
        observations = db.execute(
            "SELECT kind FROM attempt_revisions WHERE phase LIKE 'worker_publish:%'",
        ).fetchall()
    assert state == ("blocked" if coordinator_writes_after_publication else "accepted")
    assert len(observations) == 1 and observations[0]["kind"] == "observed"
    assert any("changed the project workspace" in u for u in updates) == coordinator_writes_after_publication


def test_coordinator_turn_that_only_reads_is_not_blocked(tmp_path):
    """The positive control: a coordinator turn that never mutates the
    workspace proceeds exactly as before -- the backstop never false-
    positives on an ordinary turn."""
    store, task_id = _task(tmp_path)
    (tmp_path / "existing.txt").write_text("unchanged\n")

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        (tmp_path / "existing.txt").read_text()  # ordinary inspection, no write
        _accept_task(store, channel, "Nothing to do; accepted directly.", "acceptdirect")
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise AssertionError("no delegation happens in this test")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert not any("changed the project workspace" in u for u in updates)
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
