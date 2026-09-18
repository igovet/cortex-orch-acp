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

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
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
    budget on a notice nobody ever owed it. Asserts elapsed time AND the
    durable no_progress_count, not just the final state, so the stall
    itself (not merely a wrong label) is what this test catches."""
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

    # Short budget so a stall (were the bug still present) shows up as a
    # real, measurable elapsed-time cost within a fast test run -- never as
    # a fixed sleep standing in for "nothing happened".
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=5.0, max_no_progress_continuations=3)
    updates = []
    started = time.monotonic()
    result = runtime.start_prompt(task_id, "s1", "Try to edit a file yourself.", updates.append, threading.Event())
    elapsed = time.monotonic() - started

    assert result == StopReason.END_TURN
    # The coordinator is prompted exactly once -- the bug never re-prompts
    # the model, it just spins _wait_for_progress uselessly, so a call-count
    # assertion alone would not catch it; elapsed time is what does.
    assert call_counts["coordinator"] == 1
    # Fixed behavior ends the turn the instant the coordinator's own prompt
    # returns (observed ~0.04s). Buggy behavior burns 3 extra
    # _wait_for_progress rounds (each at least one backoff cycle) before
    # giving up (observed ~0.22s) -- 0.15s cleanly separates the two.
    assert elapsed < 0.15, f"turn took {elapsed:.3f}s -- the self-notice stalled it"
    with store.connection() as db:
        row = db.execute(
            "SELECT state, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    # Nothing was ever actually pending: the honest end state is idle, not
    # a no-progress-budget "blocked" the turn never should have entered.
    assert row["state"] == "idle_awaiting_user"
    assert row["no_progress_count"] == 0

    # The notice itself is never lost -- still durably readable.
    with store.connection() as db:
        note_row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key='self-notice-1'", (task_id,),
        ).fetchone()
    assert note_row is not None
    assert "a mutation was refused" in note_row["payload_json"]


def test_genuine_worker_wait_still_waits_and_no_progress_budget_still_ends_turn(tmp_path):
    """A turn that really is waiting on a worker result (the worker never
    publishes or asks anything) must still wait out the bounded no-progress
    budget and end 'blocked' -- the fix must not make genuine waiting a
    no-op. Reuses the same short-budget pattern as
    ``test_orchestrating_runtime.test_no_progress_budget_ends_turn_blocked_without_infinite_waiting``,
    with an explicit elapsed-time floor so the budget is shown to actually
    have been spent, not skipped."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(
                store, channel, "delegate",
                dict(title="Assignment X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX"),
            )
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        # Never publishes a result or asks a question -- a genuinely
        # stalled worker; this must still be waited on.
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    wait_timeout = 0.2
    max_no_progress = 2
    runtime, engine = _make_runtime(
        store, router, wait_timeout_seconds=wait_timeout, max_no_progress_continuations=max_no_progress,
    )
    updates = []
    started = time.monotonic()
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    elapsed = time.monotonic() - started

    assert result == StopReason.END_TURN
    with store.connection() as db:
        row = db.execute(
            "SELECT state, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    assert row["state"] == "blocked"
    assert row["no_progress_count"] >= max_no_progress
    assert any("Blocked" in u for u in updates)
