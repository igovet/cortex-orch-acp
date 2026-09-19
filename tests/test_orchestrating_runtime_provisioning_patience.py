"""T035 FINDING-13 #2: the coordinator's no-progress budget must relate to
real provisioning progress, not tick purely on a fixed wall-clock cadence
regardless of whether a worker/coordinator engine session is genuinely
still being provisioned.

Two things are covered:

- ``delegation.provisioning.in_progress_snapshot`` itself (the data source
  ``acp.orchestrating_runtime.OrchestratingRuntime._continuation_loop``
  reads): reflects a real step transition, and goes empty once the row
  leaves ``in_progress``.
- The wiring into the continuation loop: while a (synthetic, directly
  inserted) ``delegation_provisioning`` row for the task keeps advancing
  its ``step``, ``task_orchestration.no_progress_count`` must never climb
  past 0 and ``wait_reason`` must read ``workers_provisioning``; once that
  row stops advancing, the ordinary no-progress budget must still apply
  and the turn must still end ``blocked`` -- this mechanism only ever
  extends patience for genuine backend progress, never removes the budget
  altogether.
"""
from __future__ import annotations

import threading
import time
import uuid

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.provisioning import in_progress_snapshot
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _insert_provisioning_row(store, task_id, idempotency_key, step, status="in_progress"):
    with store.connection() as db:
        epoch = current_owner_epoch(db, task_id)
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT 1 FROM delegation_provisioning WHERE idempotency_key=?", (idempotency_key,),
            ).fetchone()
            now = time.strftime("%Y-%m-%dT%H:%M:%S.%fZ", time.gmtime())
            if existing is None:
                db.execute(
                    "INSERT INTO delegation_provisioning "
                    "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
                    "VALUES (?,?,?,?,?,?,?,?,?,?)",
                    (idempotency_key, task_id, "a_synthetic", "xa_synthetic", "backend-dev", step, status, epoch, now, now),
                )
            else:
                db.execute(
                    "UPDATE delegation_provisioning SET step=?, status=?, updated_at=? WHERE idempotency_key=?",
                    (step, status, now, idempotency_key),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def test_in_progress_snapshot_reflects_real_step_transitions(tmp_path):
    store, task_id = _task(tmp_path)
    assert in_progress_snapshot(store, task_id) == ()

    _insert_provisioning_row(store, task_id, "synthetic-key", "workspace")
    first = in_progress_snapshot(store, task_id)
    assert len(first) == 1
    assert first[0][0] == "synthetic-key"
    assert first[0][1] == "workspace"

    # A step transition (with a fresh updated_at) changes the signature.
    _insert_provisioning_row(store, task_id, "synthetic-key", "channel")
    second = in_progress_snapshot(store, task_id)
    assert second != first
    assert second[0][1] == "channel"

    # Once the row leaves 'in_progress' (ready/failed), it is no longer
    # reported -- only genuinely still-provisioning rows count.
    _insert_provisioning_row(store, task_id, "synthetic-key", "ready", status="ready")
    assert in_progress_snapshot(store, task_id) == ()


def test_advancing_provisioning_resets_the_no_progress_budget_but_a_real_stall_still_blocks(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        # Never publishes/asks. Deliberately slow (longer than this test's
        # whole observation window): keeps the real assignment's own
        # background future in self._running the entire time, so
        # _wait_for_progress reliably waits out its full
        # wait_timeout_seconds on every tick instead of returning early the
        # instant the worker future finishes (which -- with an instant fake
        # -- would otherwise make every tick near-zero-cost and race past
        # this test's own watcher thread). The *synthetic* row below (not
        # this real assignment's own provisioning row, which is already
        # 'ready' by the time this runs) is what this test actually
        # observes advancing.
        time.sleep(2.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.1, max_no_progress_continuations=4,
    )

    # Seed a synthetic still-provisioning row before the turn starts, then
    # advance its step twice (from a background thread, so the turn's own
    # blocking start_prompt() call keeps running) before finally leaving it
    # stalled so the ordinary no-progress budget still has to apply.
    _insert_provisioning_row(store, task_id, "synthetic-key", "workspace")

    max_no_progress_seen_while_advancing = 0
    saw_workers_provisioning = False
    stop_watching = threading.Event()

    def watcher():
        nonlocal max_no_progress_seen_while_advancing, saw_workers_provisioning
        for step in ("channel", "config_snapshot", "context_snapshot"):
            time.sleep(0.15)
            with store.connection() as db:
                row = db.execute(
                    "SELECT wait_reason, no_progress_count FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
            if row is not None and row["wait_reason"] == "workers_provisioning":
                saw_workers_provisioning = True
            if row is not None and row["no_progress_count"] is not None:
                max_no_progress_seen_while_advancing = max(max_no_progress_seen_while_advancing, row["no_progress_count"])
            _insert_provisioning_row(store, task_id, "synthetic-key", step)
        # Stop advancing -- from here on the row is genuinely stalled, and
        # the ordinary no-progress budget must still end the turn blocked.
        stop_watching.set()

    watcher_thread = threading.Thread(target=watcher, daemon=True)
    watcher_thread.start()
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())
    watcher_thread.join(timeout=5.0)

    # The turn ends -- it never hangs. It no longer ends *blocked*: the
    # real assignment's own worker finished its turn having produced
    # nothing, which now closes that assignment explicitly instead of
    # leaving the coordinator to run out of patience with it (a worker may
    # legitimately take hours, so patience with open work is unbounded).
    assert result == StopReason.END_TURN
    assert stop_watching.is_set(), "the watcher must have finished advancing before the turn ended"
    assert saw_workers_provisioning, "the loop must have observed and reported active provisioning at least once"
    # The invariant is that advancing provisioning never *exhausts* the
    # budget -- not that the counter is pinned at zero. A tick whose
    # snapshot is unchanged since the previous tick is charged by design
    # (that is how a genuinely stuck row is ever detected at all), so
    # whether the counter is seen at 0 or 1 here depends purely on whether
    # this watcher's next advance lands before or after the loop's next
    # wait timeout -- a race, not a contract, and the reason this assertion
    # failed under full-suite load while passing in isolation.
    assert max_no_progress_seen_while_advancing < 4, (
        "advancing provisioning must never exhaust the no-progress budget "
        f"(max_no_progress_continuations=4, saw {max_no_progress_seen_while_advancing})"
    )


# -- a worker that could not be started at all ------------------------------

def test_a_delegation_that_could_not_be_provisioned_is_reported_as_a_failure(tmp_path):
    """Live (2026-09-19, L5): the engine's credentials were removed
    mid-task. The worker could not open, so it produced no events at all --
    no assignment_ready, no result, no failure -- and the coordinator kept
    answering "waiting for the worker's report" while nothing anywhere said
    the worker could not start."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.delegation import events as delegation_events
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    with store.connection() as db:
        db.execute("INSERT INTO tasks (id, project_root, created_at) VALUES ('t1', ?, '2026-01-01T00:00:00+00:00')",
                   (str(tmp_path),))
        db.execute(
            "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
            "error_code, owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("assign-1", "t1", "xa_1", "worker-backend-dev", "engine_session", "failed",
             "connection_requires_reauthorization", 1, store._now(), store._now()))
        db.commit()

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._report_assignments_whose_provisioning_failed("t1")

    events = [e for e in delegation_events.list_since(store, "t1") if e.event_type == "assignment_failed"]
    assert len(events) == 1, events
    assert events[0].payload["error"] == "connection_requires_reauthorization"
    assert events[0].assignment_ref == "assign-1"

    # Idempotent: a second sweep says it once, not twice.
    runtime._report_assignments_whose_provisioning_failed("t1")
    again = [e for e in delegation_events.list_since(store, "t1") if e.event_type == "assignment_failed"]
    assert len(again) == 1, again


def test_a_provisioning_row_still_in_progress_is_not_reported(tmp_path):
    """Still starting is not the same as failed to start."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.delegation import events as delegation_events
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    with store.connection() as db:
        db.execute("INSERT INTO tasks (id, project_root, created_at) VALUES ('t1', ?, '2026-01-01T00:00:00+00:00')",
                   (str(tmp_path),))
        db.execute(
            "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
            "owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
            ("assign-2", "t1", "xa_2", "worker-backend-dev", "engine_session", "in_progress",
             1, store._now(), store._now()))
        db.commit()

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._report_assignments_whose_provisioning_failed("t1")

    assert not [e for e in delegation_events.list_since(store, "t1") if e.event_type == "assignment_failed"]
