"""T028: bounded, idempotent owned-resource reaper (the T017/T023 handoff
``delegation.capacity``'s own docstring names explicitly: "a bounded reaper
that eventually force-resolves a long-ambiguous reservation is T028 scope").

Two independent sweeps, each safe to call repeatedly (idempotent) and cheap
when there is nothing to do (a single bounded, indexed SELECT):

- :func:`sweep_capacity` -- re-probes every ``ambiguous`` capacity
  reservation via T023 process identity (``engine.process_identity``,
  pid + start ticks + boot id) through the caller's own
  ``lookup_owned``/``terminate_owned`` duck-type (the identical contract
  ``recovery.worker_reconciliation.reconcile`` already uses -- this module
  never signals a process directly). Capacity is released only once that
  probe confirms the process is gone; a process this reaper cannot
  positively identify as owned-and-exited is left exactly as ambiguous --
  never falsely released, and never terminated by pid alone without the
  identity match ``lookup_owned``/``terminate_owned`` themselves already
  enforce (see ``delegation.live_adapters``'s ``_terminate_cross_process``).
- :func:`sweep_owned_workspaces` -- removes only a Cortex-owned isolated
  snapshot workspace (``workspace_snapshots``, under this project's own
  ``.cortex/workspaces/<attempt_id>/`` -- never ``project_root``) once its
  owning assignment reached a terminal state and (for a real attempt) its
  integration outcome was durably recorded, or the assignment was
  cancelled with nothing to integrate. Mirrors
  ``acp.orchestrating_runtime.OrchestratingRuntime.
  _cleanup_isolated_workspace_if_integrated``'s exact predicate; exists
  for every row a crash left uncleaned across a process restart, or that
  belongs to a task no connection in *this* process ever continued.

Both are invoked once at runtime start and once after a host disconnect
(see ``acp.production_runtime``/``acp.agent``) -- never on a per-request
hot path. Neither ever touches a process this Cortex process did not
itself launch (or a previous Cortex process record durably attests to
having launched), a user's project files, engine installs, or credentials.
"""
from __future__ import annotations

DEFAULT_SWEEP_LIMIT = 200


def sweep_capacity(store, engine, *, limit: int = DEFAULT_SWEEP_LIMIT) -> dict:
    """Bounded re-probe of up to ``limit`` ``ambiguous`` reservations
    (oldest first). ``engine`` is any object exposing
    ``lookup_owned(attempt_id) -> "alive"|"absent"|"unknown"`` and,
    optionally, ``terminate_owned(attempt_id)`` -- the same duck-type
    ``SessionRuntime`` already implements (see
    ``delegation.live_adapters.EngineSessionManagerAdapter``). Returns
    action counts for observability only; callers never branch on them."""
    from ..delegation import capacity as delegation_capacity
    from .._fault_injection import checkpoint

    with store.connection() as db:
        rows = db.execute(
            "SELECT reservation_id, attempt_id FROM capacity_reservations "
            "WHERE state='ambiguous' ORDER BY updated_at ASC LIMIT ?", (limit,),
        ).fetchall()
    actions = dict(released=0, terminate_attempted=0, unknown_left_ambiguous=0)
    lookup = getattr(engine, "lookup_owned", None)
    terminate = getattr(engine, "terminate_owned", None)
    for row in rows:
        attempt_id = row["attempt_id"]
        state = lookup(attempt_id) if lookup is not None else "unknown"
        checkpoint("t028_reaper_capacity_probed")
        if state == "absent":
            delegation_capacity.release_confirmed(store, row["reservation_id"], "reaper_confirmed_absent")
            checkpoint("t028_reaper_capacity_released")
            actions["released"] += 1
        elif state == "alive" and terminate is not None:
            # terminate_owned itself re-transitions the reservation
            # (release_confirmed on a freshly confirmed exit, or
            # mark_ambiguous again otherwise) -- never assumed here; see
            # its own docstring for the identity-matched kill it performs.
            terminate(attempt_id)
            actions["terminate_attempted"] += 1
        else:
            actions["unknown_left_ambiguous"] += 1
    return actions


def sweep_owned_workspaces(store, *, limit: int = DEFAULT_SWEEP_LIMIT) -> dict:
    """Bounded removal of up to ``limit`` Cortex-owned isolated snapshot
    workspaces (oldest first) whose owning attempt is terminal+integrated
    (or cancelled). Never touches ``project_root``; see module docstring."""
    from ..contracts import StoreError
    from ..delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
    from .._fault_injection import checkpoint

    with store.connection() as db:
        rows = db.execute(
            "SELECT workspace_binding_id, task_id, attempt_id, project_root FROM workspace_snapshots "
            "WHERE cleaned_up_at IS NULL AND mode='isolated_snapshot' ORDER BY created_at ASC LIMIT ?",
            (limit,),
        ).fetchall()
    actions = dict(removed=0, not_terminal=0, error=0)
    for row in rows:
        with store.connection() as db:
            provisioning = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (row["attempt_id"],),
            ).fetchone()
            assignment_ref = provisioning["idempotency_key"] if provisioning else None
            cancelled = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' "
                "AND assignment_ref=?", (row["task_id"], assignment_ref),
            ).fetchone() if assignment_ref else None
            integrated = db.execute(
                "SELECT 1 FROM workspace_integrations WHERE workspace_binding_id=?",
                (row["workspace_binding_id"],),
            ).fetchone()
        if integrated is None and cancelled is None:
            actions["not_terminal"] += 1
            continue
        checkpoint("t028_reaper_workspace_before_cleanup")
        try:
            IsolatedSnapshotWorkspaceProvisioner(store, row["project_root"], row["task_id"]).cleanup_binding(
                row["workspace_binding_id"], reason="reaper_terminal_integrated",
            )
            actions["removed"] += 1
        except StoreError:
            actions["error"] += 1
    return actions


def sweep(store, engine, *, limit: int = DEFAULT_SWEEP_LIMIT) -> dict:
    """Both sweeps together -- the single entry point called at runtime
    start and after a host disconnect."""
    return dict(
        capacity=sweep_capacity(store, engine, limit=limit),
        workspaces=sweep_owned_workspaces(store, limit=limit),
    )
