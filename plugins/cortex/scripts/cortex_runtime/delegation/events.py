"""Durable semantic event log for T016 model-owned coordination.

Wraps the ``task_events`` table (see :mod:`events_store`). Every row is one
meaningful occurrence -- never raw transport/model chatter -- and is
deduplicated by *identity* (a caller-chosen ``dedup_key``: a
``request_key``, a ``report_ref``, or an origin event id), never by
comparing payload text. ``append_event`` is therefore safe to call more
than once for the same logical occurrence: a duplicate ``(task_id,
dedup_key)`` is silently treated as the already-recorded row (its earlier
``id``/``created_at`` are returned) rather than raising or inserting again.

``claim_unconsumed`` atomically marks a batch of not-yet-consumed,
not-superseded rows as consumed by one continuation id (so two concurrent
readers -- for example a crash-recovered second attempt -- never each build
a coordinator continuation prompt from the same events). ``mark_superseded``
flags rows that must no longer influence a continuation (for example a
worker result that arrives after its assignment was cancelled).
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import json
from typing import Any, Mapping, Sequence

EVENT_TYPES = frozenset({
    "delegation_requested", "assignment_ready", "assignment_failed",
    "assignment_cancel_requested", "assignment_cancelled",
    "worker_result", "worker_question", "coordinator_clarification",
    "coordinator_decision", "user_message", "progress",
})

# Event types that require a coordinator continuation (see
# acp.orchestrating_runtime); "progress" and provisioning-transition events
# are informative only and never by themselves wake a waiting coordinator.
MEANINGFUL_EVENT_TYPES = frozenset({
    "worker_result", "worker_question", "assignment_failed",
    "assignment_cancelled", "user_message",
})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


@dataclass(frozen=True)
class TaskEvent:
    id: int
    task_id: str
    event_type: str
    assignment_ref: str | None
    dedup_key: str
    payload: Mapping[str, Any]
    origin: str
    created_at: str
    consumed_by: str | None
    superseded: bool

    @classmethod
    def _from_row(cls, row) -> "TaskEvent":
        return cls(
            id=row["id"], task_id=row["task_id"], event_type=row["event_type"],
            assignment_ref=row["assignment_ref"], dedup_key=row["dedup_key"],
            payload=json.loads(row["payload_json"]), origin=row["origin"],
            created_at=row["created_at"], consumed_by=row["consumed_by"],
            superseded=bool(row["superseded"]),
        )


def append_event_tx(
    db, task_id: str, event_type: str, dedup_key: str, payload: Mapping[str, Any],
    origin: str, *, assignment_ref: str | None = None,
) -> TaskEvent:
    """Same as :func:`append_event`, but operates on an already-open
    transaction (``db``) and performs no transaction control of its own --
    for a caller (see ``acp.replay_store.record_user_message_event``) that
    must commit this insert atomically together with another write in the
    very same transaction.
    """
    if event_type not in EVENT_TYPES:
        raise ValueError(f"unknown event_type {event_type!r}")
    payload_json = json.dumps(dict(payload), sort_keys=True, ensure_ascii=False)
    existing = db.execute(
        "SELECT * FROM task_events WHERE task_id=? AND dedup_key=?", (task_id, dedup_key),
    ).fetchone()
    if existing is not None:
        return TaskEvent._from_row(existing)
    db.execute(
        "INSERT INTO task_events "
        "(task_id, event_type, assignment_ref, dedup_key, payload_json, origin, created_at, consumed_by, superseded) "
        "VALUES (?,?,?,?,?,?,?,NULL,0)",
        (task_id, event_type, assignment_ref, dedup_key, payload_json, origin, _now()),
    )
    row = db.execute(
        "SELECT * FROM task_events WHERE task_id=? AND dedup_key=?", (task_id, dedup_key),
    ).fetchone()
    return TaskEvent._from_row(row)


def append_event(
    store, task_id: str, event_type: str, dedup_key: str, payload: Mapping[str, Any],
    origin: str, *, assignment_ref: str | None = None,
) -> TaskEvent:
    """Append one event, idempotently on ``(task_id, dedup_key)``, in its
    own standalone transaction (see :func:`append_event_tx` for the
    shared-transaction variant).

    Returns the (newly inserted or already-existing) :class:`TaskEvent`.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            event = append_event_tx(
                db, task_id, event_type, dedup_key, payload, origin, assignment_ref=assignment_ref,
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return event


def list_since(store, task_id: str, after: int = 0, limit: int = 200) -> list[TaskEvent]:
    """Cursor-paginated, superseded-inclusive read of every event for ``task_id``.

    ``after`` is the last-seen event id (0 to start from the beginning);
    callers that need to exclude superseded rows filter the result
    themselves (list_events callers see everything for auditability, while
    continuation building explicitly skips superseded rows -- see
    ``acp.orchestrating_runtime``).
    """
    limit = max(1, min(int(limit), 200))
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM task_events WHERE task_id=? AND id>? ORDER BY id LIMIT ?",
            (task_id, int(after), limit),
        ).fetchall()
    return [TaskEvent._from_row(row) for row in rows]


def claim_unconsumed(store, task_id: str, continuation_id: str) -> list[TaskEvent]:
    """Atomically claim every not-yet-consumed, not-superseded event.

    Marks the claimed rows' ``consumed_by`` in the same transaction that
    reads them, so a concurrent second claimer never observes the same
    batch. Superseded rows are never claimed (a one-line note about a
    cancelled/superseded assignment is synthesized by the caller from
    ``assignment_cancelled`` events instead of replaying the superseded
    ones -- see ``acp.orchestrating_runtime``).
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            rows = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 ORDER BY id",
                (task_id,),
            ).fetchall()
            ids = [row["id"] for row in rows]
            if ids:
                db.executemany(
                    "UPDATE task_events SET consumed_by=? WHERE id=?",
                    [(continuation_id, event_id) for event_id in ids],
                )
                rows = db.execute(
                    f"SELECT * FROM task_events WHERE id IN ({','.join('?' for _ in ids)}) ORDER BY id",
                    ids,
                ).fetchall()
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return [TaskEvent._from_row(row) for row in rows]


def release_claim(store, task_id: str, continuation_id: str) -> int:
    """Undo a :func:`claim_unconsumed` claim that was never actually
    delivered to the coordinator (its ``engine.prompt`` call raised before
    returning), returning the affected events to unconsumed so the next
    continuation claims and delivers them exactly once.

    Only rows still marked ``consumed_by=continuation_id`` are released
    (never a row a later claim already reassigned), and never a superseded
    row (superseded is a durable, independent terminal marker). Returns the
    number of rows released.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            cursor = db.execute(
                "UPDATE task_events SET consumed_by=NULL WHERE task_id=? AND consumed_by=? AND superseded=0",
                (task_id, continuation_id),
            )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count


def mark_superseded(store, task_id: str, assignment_ref: str) -> int:
    """Flag every event for ``assignment_ref`` as superseded; returns the count."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            cursor = db.execute(
                "UPDATE task_events SET superseded=1 WHERE task_id=? AND assignment_ref=?",
                (task_id, assignment_ref),
            )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count


def mark_delivered(store, task_id: str, continuation_id: str) -> int:
    """Mark every row still claimed by ``continuation_id`` as delivered,
    once ``engine.prompt`` for that continuation has actually returned.

    T017 crash recovery (see :func:`reconcile_undelivered_claims`): a claim
    (``consumed_by`` set) with ``delivered_at`` still NULL means the process
    crashed between claiming the batch and the ``engine.prompt`` call
    returning -- "delivered" is at-least-once to the model, but only ever
    recorded here, never assumed. Returns the number of rows marked.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            cursor = db.execute(
                "UPDATE task_events SET delivered_at=? WHERE task_id=? AND consumed_by=? AND delivered_at IS NULL",
                (_now(), task_id, continuation_id),
            )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count


def reconcile_undelivered_claims(store, task_id: str | None = None) -> int:
    """Release every claim (``consumed_by`` set) whose delivery was never
    confirmed (``delivered_at`` still NULL) -- a crash between
    :func:`claim_unconsumed` and the matching :func:`mark_delivered`.

    Call once when a runtime instance first handles a task (see
    ``acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator``);
    idempotent and safe to call with no crash pending (a no-op). Returns the
    number of rows released back to unconsumed.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if task_id is None:
                cursor = db.execute(
                    "UPDATE task_events SET consumed_by=NULL WHERE consumed_by IS NOT NULL AND delivered_at IS NULL AND superseded=0",
                )
            else:
                cursor = db.execute(
                    "UPDATE task_events SET consumed_by=NULL WHERE task_id=? AND consumed_by IS NOT NULL AND delivered_at IS NULL AND superseded=0",
                    (task_id,),
                )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count


def has_unconsumed(store, task_id: str) -> bool:
    with store.connection() as db:
        row = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 LIMIT 1",
            (task_id,),
        ).fetchone()
    return row is not None
