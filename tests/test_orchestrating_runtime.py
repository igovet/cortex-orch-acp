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

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome, SecondCoordinatorRefused
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
            "INSERT INTO work_assignments(assignment_id,task_id,node_key,"
            "requirement_bindings_json,candidate_ref,status,requirement_state,"
            "idempotency_key,created_at) VALUES (?,?,?,?,?,?,?,?,?)",
            (assignment_id, task_id, "refresh", "{}", None, "planned", "structured", "idem-reported", now),
        )
        db.execute(
            "INSERT INTO work_assignment_attempts VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (attempt_id, task_id, assignment_id, 1, None, None, "{}", "reported", None, "attempt-reported", now),
        )
        db.execute(
            "INSERT INTO work_assignments(assignment_id,task_id,node_key,"
            "requirement_bindings_json,candidate_ref,status,requirement_state,"
            "idempotency_key,created_at) VALUES (?,?,?,?,?,?,?,?,?)",
            (failed_assignment_id, task_id, "unreported", "{}", None, "planned", "structured", "idem-unreported", now),
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


def test_coordinator_emits_complete_blocks_in_tool_order(tmp_path):
    """Coordinator deltas are held as blocks, with tools between blocks."""
    store, task_id = _task(tmp_path)
    timeline = []

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def coordinator(_store, _channel, _ref, _message, _call_index, *, on_delta, on_tool_call):
            on_delta("before")
            on_tool_call(
                "tool-1", "execute", "Shell — pytest -q", "completed",
                [], "pytest -q", {"command": "pytest -q"}, None,
            )
            on_delta("after")
            return PromptOutcome(StopReason.END_TURN, "")

        return coordinator

    runtime, _engine = _make_runtime(store, router)
    try:
        runtime.start_prompt(
            task_id, "s1", "Inspect the task.",
            lambda text, **_kwargs: timeline.append(("message", text)),
            threading.Event(),
            emit_tool_call=lambda update: timeline.append(("tool", update)),
        )
    finally:
        runtime.shutdown()

    messages = [text for kind, text in timeline if kind == "message"]
    assert messages[:2] == ["before", "after"]
    assert "[structured_proposal_missing]" in messages[-1]
    message_positions = [index for index, (kind, _value) in enumerate(timeline) if kind == "message"]
    tool_positions = [index for index, (kind, _value) in enumerate(timeline) if kind == "tool"]
    assert tool_positions
    assert message_positions[0] < tool_positions[0] < message_positions[1]


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

    assert len(engine.prompt_blocks_calls) == 6
    for blocks, left, right in zip([b for b in engine.prompt_blocks_calls if b is not None], ("text A", "followup A"), ("text B", "followup B")):
        assert blocks is not None
        assert [block["type"] for block in blocks] == ["text", "text", "image", "text"]
        assert blocks[2] == image
        assert "[image:" not in blocks[0]["text"]
        assert blocks[1] == {"type": "text", "text": left}
        assert blocks[3] == {"type": "text", "text": right}
        assert "Current user input:" not in blocks[0]["text"]
        assert "Runtime schedules" in blocks[0]["text"]
        assert blocks[1]["text"] == left
        assert blocks[3]["text"] == right
























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




def _resolve_owner_channel(store, task_id):
    from cortex_runtime.ownership import current_owner_epoch
    from cortex_runtime.trusted_channel import issue_channel, resolve_channel

    with store.connection() as db:
        epoch = current_owner_epoch(db, task_id)
    channel_id, secret = issue_channel(store, task_id, "coordinator", epoch)
    return resolve_channel(store, channel_id, secret)






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
        mcp_tools.dispatch(store,channel,"user_question_request",dict(question="Next?",request_key="next"))
        return PromptOutcome(StopReason.END_TURN, "Accepted.")

    def router(store, channel, session_ref, launch, instructions):
        return flaky_coordinator if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    with __import__("contextlib").nullcontext():
        runtime.start_prompt(task_id, "s1", "Please check in.", lambda t: True, threading.Event())

    with store.connection() as db:
        row = db.execute(
            "SELECT consumed_by, superseded FROM task_events WHERE task_id=? AND event_type='user_message'", (task_id,),
        ).fetchone()
    assert row["consumed_by"] is not None and row["superseded"] == 0  # uncertain claim retained

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


def test_clamp_attempt_context_size_never_regresses_within_an_attempt():
    """claude-agent-acp reports a placeholder context size (e.g. 200000)
    on every mid-turn usage_update and corrects it to the model's real
    size (e.g. 1000000) only on the turn's final usage_update, same
    attempt, same ``used`` count. The clamp must keep the largest size
    seen for that attempt and never let a later, smaller report undo it,
    while a different attempt starts its own tracking from scratch."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._attempt_context_size_max = {}
    runtime._model_context_size_max = {}
    runtime._context_sizes_loaded = True  # never read or write the real state file
    runtime._save_context_sizes = lambda: None
    runtime._attempt_models = {"xa_1": "claude-agent-acp/claude-sonnet-5", "xa_2": "codex-acp/gpt-5.6-luna",
                               "xa_3": "claude-agent-acp/claude-sonnet-5"}

    assert runtime._clamp_attempt_context_size("xa_1", 200000) == 200000
    assert runtime._clamp_attempt_context_size("xa_1", 200000) == 200000
    assert runtime._clamp_attempt_context_size("xa_1", 1000000) == 1000000
    # A late/stale smaller report after the real size is known must not
    # regress the panel back down.
    assert runtime._clamp_attempt_context_size("xa_1", 200000) == 1000000

    # A different model tracks its own max independently.
    assert runtime._clamp_attempt_context_size("xa_2", 258400) == 258400
    # A later worker on the same model starts at the size already learned.
    assert runtime._clamp_attempt_context_size("xa_3", 200000) == 1000000

    # Invalid/absent sizes are left alone by the caller (None means "do
    # not touch the reported size").
    assert runtime._clamp_attempt_context_size("xa_1", None) is None
    assert runtime._clamp_attempt_context_size("xa_1", 0) is None
    assert runtime._clamp_attempt_context_size("xa_1", -5) is None
    assert runtime._clamp_attempt_context_size("xa_1", True) is None
    assert runtime._clamp_attempt_context_size(None, 200000) is None






# -- T035 FINDING-12 closure: the coordinator-turn workspace backstop -----
