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

import json
from pathlib import Path

from ..delegation import events as delegation_events


def _mark_graph_attempt_failed_tx(db, task_id: str, assignment_ref: str, reason: str) -> None:
    """Close the graph attempt together with the recovery failure event.

    Recovery deliberately never retries an uncertain worker.  The event log
    already records that decision, but leaving the graph attempt in ``running``
    makes the durable work graph claim that the worker is still active after a
    restart.  Resolve the graph row in the same transaction as the event so a
    crash cannot commit one terminal signal without the other.  Older stores
    may not have the additive graph tables yet; in that case the event remains
    the authoritative recovery record.
    """
    try:
        row = db.execute(
            "SELECT payload_json FROM task_events "
            "WHERE task_id=? AND assignment_ref=? AND event_type='delegation_requested' "
            "ORDER BY id DESC LIMIT 1",
            (task_id, assignment_ref),
        ).fetchone()
        if row is None:
            return
        import json

        work_attempt_ref = json.loads(row["payload_json"]).get("work_attempt_ref")
        if not isinstance(work_attempt_ref, str) or not work_attempt_ref:
            return
        db.execute(
            "UPDATE work_assignment_attempts SET status='failed', failure_reason=? "
            "WHERE attempt_id=? AND task_id=? AND status IN ('planned','running')",
            (reason, work_attempt_ref, task_id),
        )
    except Exception as error:  # noqa: BLE001 - graph is additive to recovery
        # A pre-graph store, or malformed historical payload, must not prevent
        # the recovery failure event from being committed.  Do not catch the
        # surrounding transaction's errors; this helper only protects the
        # optional graph reconciliation.
        if getattr(error, "sqlite_errorname", None) not in {"SQLITE_ERROR", "SQLITE_MISUSE"}:
            return


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
        if state == "absent":
            # The engine can prove this attempt's process is gone, so its
            # capacity slot is provably free. Only "alive"/"unknown" went
            # through terminate_owned, which releases on a *confirmed*
            # exit; "absent" called nothing and released nothing, so every
            # crash or restart permanently consumed a slot. Four of them
            # left one connection unable to open anything at all, with
            # every new attempt queued behind reservations whose processes
            # had not existed for half an hour.
            _release_confirmed_capacity(store, attempt_id, "recovery_worker_absent")
        _revoke_worker_channel(store, task_id, attempt_id)
        action = _write_recovery_failure_if_still_lost(store, task_id, assignment_ref, worker_key, attempt_id, state)
        actions.append(dict(assignment_ref=assignment_ref, action=action))
    return actions


def _release_confirmed_capacity(store, attempt_id: str, observation: str) -> None:
    """Release this attempt's capacity reservation, if it holds one.

    Best-effort by design: a reservation that is already released, or was
    never taken (capacity tracking is optional -- see
    ``delegation.live_adapters.EngineSessionManagerAdapter``), is not a
    reason to fail a recovery that has otherwise succeeded.
    """
    from ..delegation import capacity

    try:
        reservation_id = capacity.reservation_for_attempt(store, attempt_id)
        if reservation_id is not None:
            capacity.release_confirmed(store, reservation_id, observation)
    except Exception:  # noqa: BLE001 - never fail recovery over bookkeeping
        pass


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
            from ..orchestration.worker_waits import cancel_tx
            cancel_tx(db, task_id, 'recovery_worker_lost', assignment_ref=assignment_ref, status='orphaned')
            _mark_graph_attempt_failed_tx(db, task_id, assignment_ref, "recovery_worker_lost")
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


# -- host transport loss (plugin update / daemon restart) -------------------
#
# A Paseo plugin update restarts the plugin process, closing every open
# Cortex ACP connection at once (the host then shows "Provider connection
# closed" on each chat, including chats whose task already completed).  That
# is a transport event, not a task outcome: the runtime records one durable
# ``progress`` event of kind ``host_disconnected`` naming exactly what was in
# flight, and leaves task, node and assignment state for :func:`reconcile`
# after restart.
#
# A worker gateway action whose receipt still says ``complete: false`` has an
# unknown outcome.  It is listed in that record and in the coordinator's next
# planning request, and never re-executed automatically
# (``orchestration.recovery.retry_lost`` refuses a lost node that owns one).
PLUGIN = Path(__file__).resolve().parents[3]
HOST_DISCONNECTED = "host_disconnected"
# Gateway operations whose effect may already exist when their receipt is open.
MUTATING_OPERATIONS = ("command_run", "outside_command", "outside_write", "project_write",
                       "project_mkdir", "project_delete", "project_skill_link", "draft_write", "external_call")
MAX_LISTED = 50


def runtime_version() -> str:
    try:
        return json.loads((PLUGIN / "package-identity.json").read_text())["version"]
    except (OSError, ValueError, KeyError):
        return "unknown"


def unknown_outcome_actions(db, task_id: str, *, revoked_only: bool = False, live_only: bool = False,
                            limit: int = MAX_LISTED + 1) -> list[dict]:
    """Worker gateway actions of this task whose outcome was never retained.

    ``revoked_only`` keeps only actions whose worker channel can no longer
    complete them (lost, cancelled or replaced), so a live worker's call that
    is merely still running is not reported as unknown.  ``live_only`` keeps
    only actions whose worker channel is still open: the calls in flight at
    this moment, not ones an earlier stop or loss already fenced.  An external
    MCP call to a tool the server declares read-only has no effect to
    reconcile and is never listed.
    """
    rows = db.execute(
        "SELECT r.operation, r.result_json, c.attempt_id, c.revoked_at FROM runtime_requests r "
        "JOIN trusted_channels c ON c.channel_id=r.actor "
        f"WHERE r.task_id=? AND c.role='worker' AND r.operation IN ({','.join('?' for _ in MUTATING_OPERATIONS)}) "
        + ("AND c.revoked_at IS NOT NULL " if revoked_only else "")
        + ("AND c.revoked_at IS NULL " if live_only else "")
        + "ORDER BY r.rowid LIMIT ?",
        (task_id, *MUTATING_OPERATIONS, limit),
    ).fetchall()
    actions = []
    for row in rows:
        receipt = json.loads(row["result_json"])
        if receipt.get("complete") or receipt.get("read_only"):
            continue
        actions.append(dict(operation=row["operation"], tool_call_ref=receipt.get("tool_call_ref"),
                            attempt_id=row["attempt_id"]))
    return actions


def _interrupted_workers(db, task_id: str) -> list[dict]:
    return [dict(node_id=row["node_id"], assignment_ref=row["assignment_ref"], attempt_id=row["attempt_id"])
            for row in db.execute(
                "SELECT n.node_id, n.assignment_ref, p.attempt_id FROM runtime_nodes n "
                "JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision "
                "LEFT JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref "
                "WHERE n.task_id=? AND n.status='running' ORDER BY n.node_id LIMIT ?",
                (task_id, MAX_LISTED + 1),
            )]


def _host_disconnect_snapshot(store, task_id: str, session_id: str, **observed) -> dict:
    """Exact identities of what the lost connection left in flight."""
    record = dict(kind=HOST_DISCONNECTED, note="host disconnected", session_id=session_id, task_id=task_id,
                  runtime_version=runtime_version(), **observed)
    try:
        with store.connection() as db:
            task = db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
            orchestration = db.execute("SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?",
                                       (task_id,)).fetchone()
            workers = _interrupted_workers(db, task_id)
            # Only calls in flight now; earlier stops already fenced theirs.
            actions = unknown_outcome_actions(db, task_id, live_only=True)
    except Exception as error:  # noqa: BLE001 - a diagnostic must never block teardown
        record.update(history=f"unavailable:{type(error).__name__}")
        return record
    truncated = len(workers) > MAX_LISTED or len(actions) > MAX_LISTED
    record.update(
        task_state=task["state"] if task else None,
        coordinator_attempt_id=orchestration["coordinator_attempt_ref"] if orchestration else None,
        interrupted_workers=workers[:MAX_LISTED],
        unknown_outcome_actions=actions[:MAX_LISTED],
        history="truncated" if truncated else "complete",
    )
    return record


RECOVERY_WORKER_LOST = "recovery_worker_lost"


def interrupted_attempts(db, task_id: str) -> set[str]:
    """Worker attempts a lost host interrupted, read-only from any process.

    An attempt qualifies when a ``host_disconnected`` record names it, when
    recovery already wrote it off as ``recovery_worker_lost``, or when it
    has no terminal event and the Cortex process that launched it is
    provably gone (a replacement no signal reached, see
    ``recovery.registry.owner_liveness``). A live or unidentifiable owner
    never qualifies, so genuinely running work keeps its live status.
    """
    from . import registry

    attempts: set[str] = set()
    for row in db.execute(
            "SELECT event_type, assignment_ref, payload_json FROM task_events WHERE task_id=? AND "
            "((event_type='progress' AND payload_json LIKE ?) OR "
            "(event_type='assignment_failed' AND payload_json LIKE ?))",
            (task_id, f'%"{HOST_DISCONNECTED}"%', f'%"{RECOVERY_WORKER_LOST}"%')):
        try:
            payload = json.loads(row["payload_json"])
        except (TypeError, ValueError):
            continue
        if not isinstance(payload, dict):
            continue
        if row["event_type"] == "progress" and payload.get("kind") == HOST_DISCONNECTED:
            attempts.update(w.get("attempt_id") for w in payload.get("interrupted_workers") or ()
                            if isinstance(w, dict) and w.get("attempt_id"))
        elif row["event_type"] == "assignment_failed" and payload.get("error") == RECOVERY_WORKER_LOST:
            attempt_id = payload.get("attempt_id")
            if not attempt_id and row["assignment_ref"]:
                # The stale-coordinator-channel write-off names only the
                # assignment.
                owner = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? "
                                   "AND idempotency_key=?", (task_id, row["assignment_ref"])).fetchone()
                attempt_id = owner["attempt_id"] if owner is not None else None
            if attempt_id:
                attempts.add(attempt_id)
    has_registry = db.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='attempt_process_registry'").fetchone()
    if has_registry is None:
        return attempts
    for row in db.execute(
            "SELECT p.attempt_id FROM delegation_provisioning p WHERE p.task_id=? AND p.attempt_id IS NOT NULL "
            "AND NOT EXISTS (SELECT 1 FROM task_events e WHERE e.task_id=p.task_id "
            "AND e.assignment_ref IN (p.attempt_id, p.idempotency_key) AND e.superseded=0 "
            "AND e.event_type IN ('worker_result','assignment_failed','assignment_cancelled'))",
            (task_id,)).fetchall():
        if row["attempt_id"] not in attempts and registry.owner_liveness(db, row["attempt_id"]) == "absent":
            attempts.add(row["attempt_id"])
    return attempts


def record_host_disconnect(store, task_id: str, session_id: str, dedup_key: str, **observed) -> dict:
    payload = _host_disconnect_snapshot(store, task_id, session_id, **observed)
    delegation_events.append_event(store, task_id, "progress", dedup_key, payload, origin="runtime")
    return payload


def host_disconnect_notice(store, task_id: str, session_id: str) -> dict | None:
    """Read-only: the persisted outcome to show after the host restored a
    chat whose connection closed, until the user's next message.

    The host itself marks the chat with its connection error; this is the
    saved state beside it.  ``None`` when this session saw no disconnect
    since its last user message, or when that disconnect interrupted nothing
    (no running worker, no action in flight): a finished, stopped or idle
    chat reopens without a notice.  Counts come from the disconnect record,
    so earlier stops are never re-reported here.
    """
    with store.connection() as db:
        latest = None
        for row in db.execute(
                "SELECT id, payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND payload_json LIKE ? ORDER BY id DESC", (task_id, f'%"{HOST_DISCONNECTED}"%')):
            payload = json.loads(row["payload_json"])
            if payload.get("kind") == HOST_DISCONNECTED and payload.get("session_id") == session_id:
                latest = (row["id"], payload)
                break
        if latest is None:
            return None
        if db.execute("SELECT 1 FROM task_events WHERE task_id=? AND event_type='user_message' AND id>? LIMIT 1",
                      (task_id, latest[0])).fetchone():
            return None
        payload = latest[1]
        running = len(payload.get("interrupted_workers") or ())
        unknown = len(payload.get("unknown_outcome_actions") or ())
        if not running and not unknown:
            return None
        task = db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
    return dict(event_id=latest[0], session_id=session_id, task_id=task_id,
                state=task["state"] if task else "unknown", interrupted_workers=running,
                unknown_outcome_actions=unknown)


def unknown_outcome_request(db, task_id: str) -> str:
    """Coordinator instruction for actions whose outcome only the project shows."""
    actions = unknown_outcome_actions(db, task_id, revoked_only=True)
    if not actions:
        return ""
    listed = "; ".join(f"{a['operation']} {a['tool_call_ref']} (attempt {a['attempt_id']})" for a in actions[:MAX_LISTED])
    more = " (list truncated)" if len(actions) > MAX_LISTED else ""
    return ("These worker actions were dispatched but their outcome was never retained (the worker or its host "
            f"connection was lost): {listed}{more}. The runtime did not and will not re-run them. Reconcile them "
            "explicitly: inspect the project state for their effect before planning anything that would repeat "
            "them, or ask the user. ")
