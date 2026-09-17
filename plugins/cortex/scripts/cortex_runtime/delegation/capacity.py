"""T017 concurrency capacity accounting over ``capacity_reservations`` (see
``delegation.capacity_store``).

One reservation per owned engine process. ``reserve`` is atomic (``BEGIN
IMMEDIATE`` counts ``reserved``+``ambiguous`` rows for the connection before
inserting) so two concurrent reservers for the same connection never both
observe capacity as free -- no over-subscription. Capacity is released only
on a *confirmed* observation (:func:`release_confirmed`); an unconfirmed
termination attempt (timeout, no observed exit) is :func:`mark_ambiguous`
instead and keeps counting against the limit -- it is never silently
dropped (a bounded reaper that eventually force-resolves a long-ambiguous
reservation is T028 scope).

``max_concurrent_sessions`` is the caller's own concern (see
``connections.model.ConnectionRecord`` for the optional, validated,
backward-compatible field this module expects a caller to resolve and
pass in -- this module has no opinion on where the limit is configured).
"""
from __future__ import annotations

from datetime import datetime, timezone

from ..portable.identity import new_id

DEFAULT_MAX_CONCURRENT_SESSIONS = 4


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def reserve(
    store, connection_id: str, task_id: str, attempt_id: str, kind: str, *,
    assignment_ref: str | None = None, limit: int = DEFAULT_MAX_CONCURRENT_SESSIONS,
) -> str | None:
    """Atomically reserve one capacity unit for ``connection_id``.

    Returns the new ``reservation_id``, or ``None`` if the connection is
    already at ``limit`` active (``reserved``+``ambiguous``) reservations --
    the caller must not open an engine process in that case (queue/retry
    on a later capacity release instead of over-subscribing).
    """
    if kind not in ("coordinator", "worker"):
        raise ValueError(f"unknown capacity reservation kind {kind!r}")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            active = db.execute(
                "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id=? AND state IN ('reserved','ambiguous')",
                (connection_id,),
            ).fetchone()["n"]
            if active >= limit:
                db.commit()
                return None
            reservation_id = new_id("cap_")
            now = _now()
            db.execute(
                "INSERT INTO capacity_reservations "
                "(reservation_id, connection_id, task_id, assignment_ref, attempt_id, kind, state, reserved_at, updated_at, release_observation) "
                "VALUES (?,?,?,?,?,?,'reserved',?,?,NULL)",
                (reservation_id, connection_id, task_id, assignment_ref, attempt_id, kind, now, now),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return reservation_id


def release_confirmed(store, reservation_id: str, observation: str = "process_exited") -> None:
    """Release a reservation after a *positively observed* process exit
    (for example ``EngineProcess.wait_exited()`` returning True). Never
    deletes the row -- history stays readable."""
    _transition(store, reservation_id, "released", observation)


def mark_ambiguous(store, reservation_id: str, observation: str = "timeout_alive") -> None:
    """A termination attempt whose exit was never confirmed -- still counts
    against the connection's limit; never auto-released here."""
    _transition(store, reservation_id, "ambiguous", observation)


def _transition(store, reservation_id: str, state: str, observation: str) -> None:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE capacity_reservations SET state=?, updated_at=?, release_observation=? WHERE reservation_id=?",
                (state, _now(), observation, reservation_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def reservation_for_attempt(store, attempt_id: str) -> str | None:
    """(T023) The still-active (``reserved``/``ambiguous``) reservation id
    for ``attempt_id``, or ``None``. Used by a *different* adapter/process
    instance than the one that originally called :func:`reserve` (see
    ``delegation.live_adapters.EngineSessionManagerAdapter.terminate_owned``'s
    cross-process path), which never has that reservation id in its own
    in-memory state."""
    with store.connection() as db:
        row = db.execute(
            "SELECT reservation_id FROM capacity_reservations WHERE attempt_id=? AND state IN ('reserved','ambiguous') "
            "ORDER BY reserved_at DESC LIMIT 1",
            (attempt_id,),
        ).fetchone()
    return row["reservation_id"] if row is not None else None


def all_active_are_kind(store, connection_id: str, kind: str) -> bool:
    """True iff at least one reservation is active for ``connection_id``
    and every active (``reserved``/``ambiguous``) one has the given
    ``kind`` -- used to detect starvation (T017 fresh review, MEDIUM): a
    worker can never succeed if every active slot on its connection is
    permanently held by a coordinator of the same/another task."""
    with store.connection() as db:
        rows = db.execute(
            "SELECT kind FROM capacity_reservations WHERE connection_id=? AND state IN ('reserved','ambiguous')",
            (connection_id,),
        ).fetchall()
    if not rows:
        return False
    return all(row["kind"] == kind for row in rows)


def count_active(store, connection_id: str) -> int:
    with store.connection() as db:
        row = db.execute(
            "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id=? AND state IN ('reserved','ambiguous')",
            (connection_id,),
        ).fetchone()
    return row["n"]
