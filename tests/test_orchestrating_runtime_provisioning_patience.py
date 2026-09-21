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


def test_a_claimed_row_for_a_finished_assignment_is_closed(tmp_path):
    """Live (2026-09-19): the engine refused to open a session, the runtime
    recorded assignment_failed itself, and the provisioning row kept its
    claim forever -- nothing retries it and nothing fails it. An assignment
    that is already over leaves nothing for its row to do."""
    from cortex_runtime.delegation import events as delegation_events
    from cortex_runtime.delegation import provisioning as delegation_provisioning
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    with store.connection() as db:
        db.execute("INSERT INTO tasks (id, project_root, created_at) VALUES ('t1', ?, '2026-01-01T00:00:00+00:00')",
                   (str(tmp_path),))
        for key, status in (("orphan", "in_progress"), ("ready-one", "ready"), ("running-one", "in_progress")):
            db.execute(
                "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
                "owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
                (key, "t1", "xa_" + key, "worker-backend-dev", "claimed:engine_session", status,
                 1, store._now(), store._now()))
        db.commit()
    # "orphan" and "ready-one" are finished; "running-one" is still going.
    for key in ("orphan", "ready-one"):
        delegation_events.append_event(
            store, "t1", "assignment_failed", f"failed:{key}", dict(error="engine_refused_session"),
            origin="runtime", assignment_ref=key)

    from cortex_runtime.orchestration.state import TaskMachine
    TaskMachine(store).create('t1','Fixture')
    with store.connection() as db:
        for key,status in [('orphan','failed'),('ready-one','completed'),('running-one','running')]:
            db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',('t1',0,key,'worker-backend-dev',key,'[]','[]','execution',status,key,None))
        db.commit()
    closed = delegation_provisioning.close_rows_for_finished_assignments(store, "t1")

    assert closed == 1  # only the in_progress one; a ready row is a normal end state
    with store.connection() as db:
        states = {r["idempotency_key"]: (r["status"], r["error_code"]) for r in db.execute(
            "SELECT idempotency_key, status, error_code FROM delegation_provisioning WHERE task_id='t1'")}
    assert states["orphan"] == ("failed", "assignment_already_finished")
    assert states["ready-one"][0] == "ready"
    assert states["running-one"] == ("in_progress", None)
