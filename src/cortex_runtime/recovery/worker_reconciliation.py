"""T023: reconcile delegated-worker fate during coordinator recovery.

Called once, early in :mod:`recovery.coordinator_recovery`'s sequence
(after the new ownership epoch is acquired -- see that module -- so any
``assignment_failed`` event this writes is itself already fenced under the
task's *current* epoch), for every assignment this task has ever delegated
(a non-superseded ``delegation_requested`` event) that is not yet terminal.

For each such assignment, durable state is inspected *before* any process
mutation, in this order:

1. A ``worker_result`` event for it already exists -- it was already
   durably published (possibly concurrently with this very recovery, in
   the narrow window before the epoch bump above -- see
   ``ownership.reassign_worker_channels_tx``'s docstring on why a worker's
   own channel is never fenced out by this bump). Nothing here ever
   re-delegates or overwrites it; the ordinary coordinator continuation
   loop delivers it exactly once via ``delegation.events.claim_unconsumed``,
   unaffected by this reconciliation. Its process is still opportunistically
   terminated if reachable/owned in this process, to free capacity promptly,
   but never marked failed.
2. Otherwise this assignment's own delegated attempt is inspected via
   ``SessionRuntime.lookup_owned`` (see
   ``delegation.provisioning``'s identical "alive"/"absent"/"unknown"
   contract): whatever the answer, there is no channel secret available in
   this fresh process to safely resume/re-authenticate that attempt (see
   ``delegation.provisioning.ProvisioningService._step_engine_session``'s
   own ``channel_secret_unavailable`` failure for the same reason on a
   same-process retry) -- so it is never silently relaunched. "alive"/
   "unknown" both call ``terminate_owned`` best-effort (confirmed exit
   frees capacity; an ambiguous/unconfirmed exit does not, but is not
   escalated into a task-wide block the way an *unresolved coordinator*
   attempt is -- losing one worker's capacity slot is a contained,
   worker-scoped cost, never a second mutating coordinator risk); "absent"
   needs no termination call. Either way the assignment is recorded
   ``assignment_failed`` with reason ``recovery_worker_lost`` -- a durable,
   honest signal the *coordinator* (a model) later decides whether to
   retry as a fresh delegation, never a silent second attempt at the same
   ``request_key``.
"""
from __future__ import annotations

from ..delegation import events as delegation_events


def _open_assignment_refs(store, task_id: str) -> list[str]:
    with store.connection() as db:
        requested = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events "
            "WHERE task_id=? AND event_type='delegation_requested' AND superseded=0",
            (task_id,),
        ).fetchall()
        refs = []
        for row in requested:
            ref = row["assignment_ref"]
            terminal = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type IN ('assignment_failed','assignment_cancelled') LIMIT 1",
                (task_id, ref),
            ).fetchone()
            if terminal is not None:
                continue
            refs.append(ref)
    return refs


def _attempt_id_for(store, assignment_ref: str) -> str | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
        ).fetchone()
    return row["attempt_id"] if row is not None else None


def _worker_result_exists(store, task_id: str, worker_key: str) -> bool:
    with store.connection() as db:
        row = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='worker_result' AND superseded=0 LIMIT 1",
            (task_id, worker_key),
        ).fetchone()
    return row is not None


def reconcile(store, engine, task_id: str) -> list[dict]:
    """Reconcile every still-open delegated assignment for ``task_id``.
    Returns a list of ``{"assignment_ref", "action"}`` records (for
    tests/observability only -- callers never branch on this list)."""
    actions: list[dict] = []
    for assignment_ref in _open_assignment_refs(store, task_id):
        attempt_id = _attempt_id_for(store, assignment_ref)
        worker_key = attempt_id or assignment_ref
        if _worker_result_exists(store, task_id, worker_key):
            if attempt_id is not None:
                _best_effort_terminate(store, engine, attempt_id)
            actions.append(dict(assignment_ref=assignment_ref, action="already_published"))
            continue
        if attempt_id is None:
            # Never even reached provisioning's engine_session step (no
            # attempt id was ever allocated) -- nothing owned to reconcile;
            # the coordinator model will see no assignment_ready either and
            # is free to retry the same request_key from provisioning's own
            # resumable step machinery, never this module's concern.
            continue
        lookup = getattr(engine, "lookup_owned", None)
        state = lookup(attempt_id) if lookup is not None else "absent"
        if state in ("alive", "unknown"):
            terminate = getattr(engine, "terminate_owned", None)
            if terminate is not None:
                terminate(attempt_id)
        _revoke_worker_channel(store, task_id, attempt_id)
        action = _write_recovery_failure_if_still_lost(store, task_id, assignment_ref, worker_key, attempt_id, state)
        actions.append(dict(assignment_ref=assignment_ref, action=action))
    return actions


def _write_recovery_failure_if_still_lost(
    store, task_id: str, assignment_ref: str, worker_key: str, attempt_id: str, prior_state: str,
) -> str:
    """Re-check for a ``worker_result`` *inside the same transaction* that
    would write ``assignment_failed`` -- a result published concurrently
    (in the window between this module's own earlier
    :func:`_worker_result_exists` check/``terminate_owned``/channel-revoke
    calls above and this write) must win: the failure event is then never
    written at all, so the two can never both end up in the durable log for
    the same assignment. See ``acp.orchestrating_runtime._coalesce`` for the
    remaining defense-in-depth suppression covering the narrower residual
    window between this commit and the worker's own already-in-flight (past
    its own channel check) ``publish_result`` call."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            result_row = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='worker_result' AND superseded=0 LIMIT 1",
                (task_id, worker_key),
            ).fetchone()
            if result_row is not None:
                db.commit()
                return "already_published_late"
            delegation_events.append_event_tx(
                db, task_id, "assignment_failed", f"recovery_worker_lost:{assignment_ref}",
                dict(error="recovery_worker_lost", assignment_ref=assignment_ref, attempt_id=attempt_id,
                     prior_state=prior_state),
                origin="runtime", assignment_ref=assignment_ref,
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return f"recovery_worker_lost:{prior_state}"


def _best_effort_terminate(store, engine, attempt_id: str) -> None:
    lookup = getattr(engine, "lookup_owned", None)
    terminate = getattr(engine, "terminate_owned", None)
    if lookup is None or terminate is None:
        return
    if lookup(attempt_id) in ("alive", "unknown"):
        terminate(attempt_id)


def _revoke_worker_channel(store, task_id: str, attempt_id: str) -> None:
    """A worker whose attempt is being written off as lost never keeps a
    usable channel afterward (T023: "reject stale owner/permission
    responses") -- revoke its trusted channel explicitly rather than
    relying only on the eventual next epoch bump to fence it out."""
    with store.connection() as db:
        row = db.execute(
            "SELECT channel_id FROM trusted_channels WHERE task_id=? AND role='worker' "
            "AND attempt_id=? AND revoked_at IS NULL",
            (task_id, attempt_id),
        ).fetchone()
    if row is None:
        return
    from ..trusted_channel import revoke_channel

    revoke_channel(store, row["channel_id"])
