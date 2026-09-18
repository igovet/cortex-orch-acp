"""T035 FINDING-13 #2 and the two client-rendering defects found with it,
in the live Zed run recorded as ``evidence/T035/RUN-07``.

Three things are covered, each a defect observed live rather than a
hypothetical:

- **A running worker is not a stall.** Live, the worker became ready at
  18:37:58 and the coordinator declared ``blocked_no_progress`` at
  18:38:12 -- fourteen seconds later, while the worker's own engine turn
  was still in flight. A worker that is genuinely running produces no
  ``delegation_events.MEANINGFUL_EVENT_TYPES`` event of its own until it
  finishes, so it was indistinguishable from nothing happening at all.
- **Consecutive coordinator messages must be separated.** The deltas of
  two consecutive continuation turns arrived as one unbroken run of
  chunks, rendering ``"...before delegating.Pipeline published."``.
- **A worker card names the role a reader reads**, not the internal skill
  id (``worker-backend-dev``), which on a card already titled ``Worker:``
  reads as a stutter.
"""
from __future__ import annotations

import threading
import uuid

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime, PromptOutcome, _display_role,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_display_role_strips_only_the_storage_prefix():
    assert _display_role("worker-backend-dev") == "backend-dev"
    # The alias form is already what a reader should see.
    assert _display_role("backend-dev") == "backend-dev"
    # Never strips anything else, and never strips a second time.
    assert _display_role("worker-worker-x") == "worker-x"
    assert _display_role("coordinator") == "coordinator"
    assert _display_role("") == ""


def test_a_worker_turn_in_flight_never_exhausts_the_no_progress_budget(tmp_path):
    """The live defect: a worker whose engine turn is genuinely running
    must hold the coordinator's patience open, and the budget must still
    apply the moment that turn ends without producing anything."""
    store, task_id = _task(tmp_path)
    worker_may_finish = threading.Event()
    worker_started = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        # Runs for as long as this test holds it, then ends without ever
        # publishing a result or asking a question -- so the ONLY thing
        # keeping the coordinator waiting is the turn being in flight.
        worker_started.set()
        worker_may_finish.wait(timeout=10.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )

    # While the worker runs, the loop must keep resetting the budget. With
    # a 0.02s tick and a budget of 3, a loop that charged ticks would have
    # ended the turn long before this release.
    def release_later():
        worker_started.wait(timeout=10.0)
        for _ in range(60):
            with store.connection() as db:
                row = db.execute(
                    "SELECT no_progress_count, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
            if row is not None and row["wait_reason"] == "workers_running":
                break
            threading.Event().wait(0.02)
        # Long enough that a charging loop would have exhausted a budget of
        # 3 ticks of 0.02s many times over.
        threading.Event().wait(0.5)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    result = runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event())
    releaser.join(timeout=10.0)

    assert worker_started.is_set(), "the worker's own turn never started; this test proved nothing"
    assert worker_may_finish.is_set(), "the turn ended before the worker was released -- the budget was charged while it ran"
    # The budget still applies once the worker's turn is over and nothing
    # meaningful came of it: the turn ends blocked, never hangs.
    assert result == StopReason.END_TURN
    with store.connection() as db:
        final = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert final["state"] == "blocked"


def test_consecutive_coordinator_turns_are_separated_in_the_client_stream(tmp_path):
    """Two continuation turns must not render as one run-on sentence."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "First turn text.")
        return PromptOutcome(StopReason.END_TURN, "Second turn text.")

    def instant_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else instant_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    updates: list[str] = []
    runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())

    streamed = "".join(u for u in updates if isinstance(u, str))
    # The worker here publishes nothing, so the block that follows the
    # coordinator's own turn is the blocked explanation rather than a
    # second turn -- either way it is a separate block and must not be
    # glued onto the previous sentence, which is exactly what was observed
    # live ("First turn text.Blocked: ...").
    assert "First turn text." in streamed
    assert "First turn text.Blocked" not in streamed, (
        "a notice ran together with the preceding turn's text, with no separator"
    )
    assert "First turn text.\n\n" in streamed
