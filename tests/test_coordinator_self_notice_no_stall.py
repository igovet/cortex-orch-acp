"""Regression: a coordinator's own self-generated informational event (for
example ``coordinator_mutation_refused`` -- see
``delegation.live_adapters.EngineSessionManagerAdapter._notify_coordinator_refusal``)
must never be counted as outstanding work the turn is still waiting on.

Root cause: ``delegation.events.has_unconsumed`` used to report True for
*any* unconsumed, not-superseded event -- including a purely informational
``progress`` row appended mid-turn, after that turn's own
``claim_unconsumed`` snapshot already ran. ``acp.orchestrating_runtime
._continuation_loop`` used that count to decide whether the turn could end
(idle) or had to keep looping through ``_wait_for_progress`` -- so a
self-notice with nothing else pending stalled the turn through the full
no-progress budget before it ended, always ending "blocked" instead of the
honest "idle_awaiting_user".

The fix scopes ``has_unconsumed`` to ``delegation.events.MEANINGFUL_EVENT_TYPES``
(the event vocabulary's own pre-existing "does this require a coordinator
continuation" distinction) -- the informational event is still claimed and
delivered (never lost), it just no longer looks like unfinished work.

No real engine subprocess, no paid model call -- in-process fakes only (see
``tests/orchestration_fakes.py``), matching the rest of this test group.
"""
from __future__ import annotations

import threading
import time
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import PermissionBridge, COORDINATOR_EDIT_REFUSAL_REASON
from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _make_runtime(store, router, **kwargs):
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    return runtime, engine


def test_coordinator_self_notice_does_not_stall_the_turn(tmp_path):
    """A turn whose only new event is the coordinator's own informational
    notice (never any real work: no delegation, no running worker) must end
    on the very first continuation -- never spend the no-progress wait
    budget on a notice nobody ever owed it. The test instruments the wait
    boundary directly, so scheduler timing cannot hide a stall."""
    store, task_id = _task(tmp_path)
    call_counts = {"coordinator": 0}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        call_counts["coordinator"] += 1
        # Simulate engine.permissions.PermissionBridge's
        # coordinator_refusal_notifier firing mid-turn (see
        # delegation.live_adapters.EngineSessionManagerAdapter
        # ._notify_coordinator_refusal): appended for THIS SAME
        # continuation, after claim_unconsumed already snapshotted it as
        # empty -- exactly the ordering the finding describes.
        delegation_events.append_event(
            store, task_id, "progress", "self-notice-1",
            dict(note="a mutation was refused", message_key="coordinator_mutation_refused", message_params={}),
            origin="runtime",
        )
        return PromptOutcome(StopReason.END_TURN, "Understood, no changes made.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=5.0, max_no_progress_continuations=3)
    wait_calls = []

    def unexpected_wait(_task_id, _cancel_event):
        wait_calls.append(_task_id)
        return False

    # An informational self-notice must not enter the progress wait at all.
    # Installing this probe makes the regression deterministic: the old
    # implementation called the wait path despite having no meaningful work.
    runtime._wait_for_progress = unexpected_wait
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Try to edit a file yourself.", updates.append, threading.Event())

    assert result == StopReason.END_TURN
    assert call_counts["coordinator"] == 3
    assert wait_calls == []
    with store.connection() as db:
        row = db.execute(
            "SELECT state, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    # Nothing was ever actually pending: the honest end state is idle, not
    # a no-progress-budget "blocked" the turn never should have entered.
    assert row["state"] == "blocked"
    assert row["no_progress_count"] == 0

    # The notice itself is never lost -- still durably readable.
    with store.connection() as db:
        note_row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key='self-notice-1'", (task_id,),
        ).fetchone()
    assert note_row is not None
    assert "a mutation was refused" in note_row["payload_json"]






def test_ordinary_dialogue_does_not_emit_missing_acceptance_refusal(tmp_path):
    """A conversational turn can end without accepting a task.

    The next user message should be able to continue the same task; the
    runtime must not present that normal pause as an acceptance failure.
    """
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "I understand. What would you like to do next?")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Let's discuss the project.", updates.append, threading.Event())

    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute(
            "SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
        refusal = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'acceptance_refused:%'",
            (task_id,),
        ).fetchone()
    assert state["state"] == "blocked"
    assert state["wait_reason"] == "structured_proposal_missing"
    assert refusal is None
    assert not any("no durable decide(accept)" in text for text in updates)


def test_ordinary_dialogue_can_continue_on_the_same_task_and_session(tmp_path):
    """An idle conversational turn does not close or accept its task."""
    store, task_id = _task(tmp_path)
    prompts = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        prompts.append(message)
        if call_index == 0:
            return PromptOutcome(StopReason.END_TURN, "Tell me more.")
        return PromptOutcome(StopReason.END_TURN, "I can continue helping with that.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    cancel = threading.Event()
    assert runtime.start_prompt(task_id, "same-session", "Let's discuss the project.", updates.append, cancel) == StopReason.END_TURN
    assert runtime.start_prompt(task_id, "same-session", "Here is more context.", updates.append, cancel) == StopReason.END_TURN

    assert len(prompts) == 6
    assert "Let's discuss the project." in prompts[0]
    assert "Here is more context." in prompts[3]
    with store.connection() as db:
        row = db.execute(
            "SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
        accepted = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='coordinator_decision' "
            "AND json_extract(payload_json, '$.decision')='accept'", (task_id,),
        ).fetchone()
    assert row["state"] == "blocked"
    assert row["wait_reason"] == "structured_proposal_missing"
    assert accepted is None


@pytest.mark.parametrize("terminal_event", ["assignment_failed", "worker_result"])
def test_prior_terminal_delegation_does_not_turn_dialogue_into_acceptance_refusal(tmp_path, terminal_event):
    """A later conversational reply stays open after terminal delegated work."""
    store, task_id = _task(tmp_path)
    assignment_ref = "assignment-dialogue"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-dialogue",
        {"title": "Earlier work", "brief": "already handled", "role": "general"},
        origin="test", assignment_ref=assignment_ref,
    )
    if terminal_event == "assignment_failed":
        delegation_events.append_event(
            store, task_id, terminal_event, "failed-dialogue",
            {"kind": "worker_refused"}, origin="test", assignment_ref=assignment_ref,
        )
    else:
        report_id = "report-dialogue"
        with store.connection() as db:
            task_dir = store._task_directory(db, task_id)
            report_path = task_dir / "report-dialogue.md"
            report_path.write_text("# Earlier work\n", encoding="utf-8")
            db.execute(
                "INSERT INTO reports VALUES (?,?,?,?,?)",
                (report_id, task_id, "report", report_path.name, store._now()),
            )
        delegation_events.append_event(
            store, task_id, terminal_event, "result-dialogue",
            {"report_ref": report_id, "summary": "already handled"},
            origin="test", assignment_ref=assignment_ref,
        )

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "What would you like to discuss next?")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, _engine = _make_runtime(store, router)
    updates = []
    assert runtime.start_prompt(task_id, "same-session", "Continue our conversation.", updates.append, threading.Event()) == StopReason.END_TURN

    with store.connection() as db:
        row = db.execute(
            "SELECT state, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
        refusal = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key LIKE 'acceptance_refused:%'",
            (task_id,),
        ).fetchone()
    assert row["state"] == "blocked"
    assert row["wait_reason"] == "structured_proposal_missing"
    assert refusal is None
    assert not any("acceptance refused" in text.lower() for text in updates)
