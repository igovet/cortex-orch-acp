"""T017 concurrency capacity accounting over ``capacity_reservations`` (see
``delegation.capacity_store``).

One reservation per owned engine process. ``reserve`` is atomic (``BEGIN
IMMEDIATE`` counts ``reserved``+``ambiguous`` rows for the connection before
inserting) so two concurrent reservers for the same connection never both
observe capacity as free -- no over-subscription. Coordinator and worker
reservations are counted separately, each against the connection's own
limit: a coordinator holds its reservation for the whole task lifetime, so
counting it against worker capacity silently took a worker slot away from
every task on that connection. Capacity is released only
on a *confirmed* observation (:func:`release_confirmed`); an unconfirmed
termination attempt (timeout, no observed exit) is :func:`mark_ambiguous`
instead and keeps counting against the limit -- it is never silently
dropped (a bounded reaper that eventually force-resolves a long-ambiguous
reservation is T028 scope).

The limit comes from the resolved connection's own optional, validated,
backward-compatible ``max_concurrent_sessions`` (see
``connections.model.ConnectionRecord``); callers resolve it with
:func:`limit_for_connection` and pass it to :func:`reserve`.
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
    """Atomically reserve one capacity unit of ``kind`` for ``connection_id``.

    Returns the new ``reservation_id``, or ``None`` if the connection
    already has ``limit`` active (``reserved``+``ambiguous``) reservations
    of the same ``kind`` -- the caller must not open an engine process in
    that case (queue/retry on a later capacity release instead of
    over-subscribing). Reservations of the other kind never count.
    """
    if kind not in ("coordinator", "worker"):
        raise ValueError(f"unknown capacity reservation kind {kind!r}")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            active = db.execute(
                "SELECT COUNT(*) AS n FROM capacity_reservations "
                "WHERE connection_id=? AND kind=? AND state IN ('reserved','ambiguous')",
                (connection_id, kind),
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


def count_active(store, connection_id: str, kind: str | None = None) -> int:
    """Active (``reserved``+``ambiguous``) reservations for the connection;
    all kinds unless ``kind`` names one."""
    query = "SELECT COUNT(*) AS n FROM capacity_reservations WHERE connection_id=? AND state IN ('reserved','ambiguous')"
    params: tuple = (connection_id,)
    if kind is not None:
        query += " AND kind=?"
        params += (kind,)
    with store.connection() as db:
        row = db.execute(query, params).fetchone()
    return row["n"]


def limit_for_connection(record, override: int | None = None) -> int:
    """The per-connection session limit to enforce for ``record``.

    The connection's own validated ``max_concurrent_sessions`` (1..64,
    see ``connections.model.ConnectionRecord``) applies; a record without
    a usable value falls back to :data:`DEFAULT_MAX_CONCURRENT_SESSIONS`.
    ``override`` (an adapter's explicit ``capacity_limit``) can only lower
    that limit, never raise it past what the connection record allows.
    """
    configured = getattr(record, "max_concurrent_sessions", None)
    if isinstance(configured, bool) or not isinstance(configured, int) or configured < 1:
        configured = DEFAULT_MAX_CONCURRENT_SESSIONS
    if override:
        return max(1, min(configured, override))
    return configured
