"""T017 concurrency capacity accounting over ``capacity_reservations`` (see
``delegation.capacity_store``).

One reservation per owned engine process. ``reserve`` is atomic (``BEGIN
IMMEDIATE`` counts the active -- ``reserved``+``ambiguous`` -- rows before
inserting) so two concurrent reservers never both observe capacity as free
-- no over-subscription.

Worker limits, in order of precedence:

1. **Per chat session** (owner rule, 2026-09-28): at most
   :data:`MAX_WORKERS_PER_SESSION` workers of any engine, role or connection
   run at once within one chat session -- one ACP session and the one task
   it drives (``task_id``). Every connection counts towards the same four
   slots, and each session has its own: another session's workers never take
   one.
2. **Per connection, within the session**: the connection's own validated
   ``max_concurrent_sessions`` (see ``connections.model.ConnectionRecord``,
   resolved with :func:`limit_for_connection`) can only *lower* how many of
   the session's workers run on that connection. It is never a cross-session
   limit: every worker is its own engine process, so no engine process is
   shared that would need one.

Coordinator reservations are not workers: they never count towards either
worker limit, and keep their own per-connection count (a coordinator holds its
reservation for the whole task lifetime, so counting it against worker capacity
silently took a worker slot away).

Capacity is released only on a *confirmed* observation
(:func:`release_confirmed`); an unconfirmed termination attempt (timeout, no
observed exit) is :func:`mark_ambiguous` instead and keeps counting. Both
transitions are conditional on the row still being active, so a slot is freed
exactly once however many terminal paths observe the same exit, and a released
row is never resurrected. A reservation whose holder is observed gone
(``EngineSessionManagerAdapter.reclaim_stale_capacity``, ``engine.reaper``) is
reclaimed through the same conditional release, never while its process is
still observed alive.
"""
from __future__ import annotations

from datetime import datetime, timezone

from ..portable.identity import new_id

DEFAULT_MAX_CONCURRENT_SESSIONS = 4
# Workers running at once within one chat session (the task its ACP session
# drives), across every engine, role and connection.
MAX_WORKERS_PER_SESSION = 4
# A reservation is taken before its engine process is launched and recorded,
# so a young reservation without a process record is an open in flight. An
# open is bounded (the 600 s workspace-lock wait plus bounded wire
# round-trips); one this old without a record belongs to an open that died.
UNRECORDED_RESERVATION_EXPIRY_SECONDS = 3600.0

_ACTIVE_STATES = ("reserved", "ambiguous")


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def reserve(
    store, connection_id: str, task_id: str, attempt_id: str, kind: str, *,
    assignment_ref: str | None = None, limit: int = DEFAULT_MAX_CONCURRENT_SESSIONS,
    session_limit: int = MAX_WORKERS_PER_SESSION,
) -> str | None:
    """Atomically reserve one capacity unit of ``kind``.

    Returns the new ``reservation_id``, or ``None`` when capacity is full --
    the caller must not open an engine process then (queue and retry on a
    later release instead of over-subscribing). A worker is refused when its
    session (``task_id``) already has ``session_limit`` active worker
    reservations on any connection, or ``limit`` of them on
    ``connection_id``. A coordinator is refused when ``connection_id``
    already has ``limit`` active coordinator reservations. Reservations of
    the other kind never count.
    """
    if kind not in ("coordinator", "worker"):
        raise ValueError(f"unknown capacity reservation kind {kind!r}")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if kind == "worker":
                in_session = db.execute(
                    "SELECT COUNT(*) AS n FROM capacity_reservations "
                    "WHERE task_id=? AND kind='worker' AND state IN ('reserved','ambiguous')",
                    (task_id,),
                ).fetchone()["n"]
                on_connection = db.execute(
                    "SELECT COUNT(*) AS n FROM capacity_reservations "
                    "WHERE task_id=? AND connection_id=? AND kind='worker' AND state IN ('reserved','ambiguous')",
                    (task_id, connection_id),
                ).fetchone()["n"]
                full = in_session >= session_limit or on_connection >= limit
            else:
                active = db.execute(
                    "SELECT COUNT(*) AS n FROM capacity_reservations "
                    "WHERE connection_id=? AND kind=? AND state IN ('reserved','ambiguous')",
                    (connection_id, kind),
                ).fetchone()["n"]
                full = active >= limit
            if full:
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


def release_confirmed(store, reservation_id: str, observation: str = "process_exited") -> bool:
    """Release a reservation after a *positively observed* process exit
    (for example ``EngineProcess.wait_exited()`` returning True). Never
    deletes the row -- history stays readable. Returns ``True`` only for the
    call that actually freed the slot; an already released reservation is
    left unchanged (the slot is released exactly once)."""
    return _transition(store, reservation_id, "released", observation)


def mark_ambiguous(store, reservation_id: str, observation: str = "timeout_alive") -> bool:
    """A termination attempt whose exit was never confirmed -- still counts
    against the limits; never auto-released here. A reservation already
    released stays released."""
    return _transition(store, reservation_id, "ambiguous", observation)


def _transition(store, reservation_id: str, state: str, observation: str) -> bool:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            changed = db.execute(
                "UPDATE capacity_reservations SET state=?, updated_at=?, release_observation=? "
                "WHERE reservation_id=? AND state IN ('reserved','ambiguous')",
                (state, _now(), observation, reservation_id),
            ).rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return changed == 1


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


def active_holders(store, connection_id: str, kind: str, task_id: str | None = None) -> list[tuple[str, str]]:
    """``(attempt_id, state)`` of every active (``reserved``/``ambiguous``)
    reservation a queued open of ``kind`` waits behind: for a worker of
    session ``task_id``, every worker of that session on any connection;
    otherwise every reservation of ``kind`` on the connection."""
    if kind == "worker" and task_id is not None:
        query = (
            "SELECT attempt_id, state FROM capacity_reservations "
            "WHERE task_id=? AND kind='worker' AND state IN ('reserved','ambiguous')"
        )
        params: tuple = (task_id,)
    else:
        query = (
            "SELECT attempt_id, state FROM capacity_reservations "
            "WHERE connection_id=? AND kind=? AND state IN ('reserved','ambiguous')"
        )
        params = (connection_id, kind)
    with store.connection() as db:
        rows = db.execute(query, params).fetchall()
    return [(row["attempt_id"], row["state"]) for row in rows]


def active_reservations(store, task_id: str, kind: str = "worker") -> list[dict]:
    """Every active reservation of ``kind`` held by session ``task_id``,
    oldest first -- the candidates a stale-slot reclaim inspects."""
    with store.connection() as db:
        rows = db.execute(
            "SELECT reservation_id, connection_id, attempt_id, state, reserved_at FROM capacity_reservations "
            "WHERE task_id=? AND kind=? AND state IN ('reserved','ambiguous') ORDER BY reserved_at ASC",
            (task_id, kind),
        ).fetchall()
    return [dict(row) for row in rows]


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


def count_session_workers(store, task_id: str) -> int:
    """Active worker reservations of session ``task_id`` on every connection."""
    with store.connection() as db:
        row = db.execute(
            "SELECT COUNT(*) AS n FROM capacity_reservations "
            "WHERE task_id=? AND kind='worker' AND state IN ('reserved','ambiguous')",
            (task_id,),
        ).fetchone()
    return row["n"]


def limit_for_connection(record, override: int | None = None) -> int:
    """The per-connection limit to enforce for ``record`` (for workers,
    within one session -- see the module docstring).

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
