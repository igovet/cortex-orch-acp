"""T022 recovery-delivery ledger over ``capsule_deliveries``.

State machine (see ``generations_store.CONTEXT_GENERATIONS_SCHEMA``'s
``state`` CHECK): ``built`` -> ``transmitted`` -> ``delivery_confirmed`` ->
``continuation_observed``, with ``blocked``/``superseded`` reachable from
any non-terminal state. Never inferred from the model's own turn text --
"a model saying it remembers is not a receipt" (INVARIANTS.md) -- every
transition here takes an explicit ``evidence_source`` naming the *technical*
channel that justifies it:

- ``built`` -- the capsule/role/tool-schema blocks were constructed for
  this exact ``(recipient, context_generation_id, capsule_revision)``.
- ``transmitted`` -- the ACP ``session/prompt`` call carrying those blocks
  was actually issued (the wire write succeeded enough to start the call).
- ``delivery_confirmed`` -- the *same* ``session/prompt`` call's JSON-RPC
  response for that exact request arrived (a ``stopReason``, whatever it
  is) -- proof the engine's transport received and processed the request
  carrying the delivery, never proof the model "understood" or "remembers"
  it.
- ``continuation_observed`` -- a *subsequent* engine turn was initiated on
  the same ``engine_session_ref`` after this delivery was confirmed (see
  :func:`mark_continuation_observed_if_pending`, called from
  ``acp.orchestrating_runtime`` immediately before the *next*
  ``session/prompt`` call on that session) -- evidence the session is still
  alive and being continued, not a claim the delivered content was used.
- ``blocked`` -- construction/transmission could not complete for a
  concrete, named reason (``capsule_oversize``, an engine limit reached
  immediately after transmission, ...); :data:`MAX_DELIVERY_RETRIES` bounds
  how many times :func:`record_built` will reopen a blocked row before
  refusing to retry it again (never an unbounded retry loop).
- ``superseded`` -- the owning ``engine_session_ref`` was released/replaced
  while this delivery was not yet ``continuation_observed`` (see
  :func:`supersede_session_deliveries`) -- it can never become
  ``delivery_confirmed``/``continuation_observed`` afterwards.
"""
from __future__ import annotations

from ..portable.identity import new_id

MAX_DELIVERY_RETRIES = 3

_TERMINAL_STATES = ("continuation_observed", "superseded")


def record_built(
    store, *, recipient: str, context_generation_id: str, capsule_revision: str, task_id: str,
    engine_session_ref: str, evidence_source: str, assignment_ref: str | None = None,
) -> dict:
    """Idempotently start (or resume) tracking a delivery for this exact
    ``(recipient, context_generation_id, capsule_revision)`` key.

    A pre-existing ``blocked`` row is reopened to ``built`` (retry_count
    incremented) as long as ``retry_count < MAX_DELIVERY_RETRIES``; once
    the bound is reached the row is returned unchanged (still ``blocked``)
    -- the caller must never resend for it again. Any other pre-existing
    state (including a fresh ``built``) is returned unchanged: this call is
    always safe to repeat.
    """
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT * FROM capsule_deliveries WHERE recipient=? AND context_generation_id=? AND capsule_revision=?",
                (recipient, context_generation_id, capsule_revision),
            ).fetchone()
            if existing is None:
                delivery_id = new_id("cd_")
                db.execute(
                    "INSERT INTO capsule_deliveries "
                    "(id, recipient, context_generation_id, capsule_revision, task_id, assignment_ref, "
                    " engine_session_ref, state, evidence_source, uncertainty, reason, retry_count, "
                    " created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,0,?,?)",
                    (delivery_id, recipient, context_generation_id, capsule_revision, task_id, assignment_ref,
                     engine_session_ref, "built", evidence_source, None, None, now, now),
                )
                db.commit()
                return _row(db, delivery_id)
            if existing["state"] == "blocked" and existing["retry_count"] < MAX_DELIVERY_RETRIES:
                db.execute(
                    "UPDATE capsule_deliveries SET state='built', uncertainty=NULL, reason=NULL, "
                    "retry_count=retry_count+1, evidence_source=?, updated_at=? WHERE id=?",
                    (evidence_source, now, existing["id"]),
                )
                db.commit()
                return _row(db, existing["id"])
            db.commit()
            return dict(existing)
        except BaseException:
            db.rollback()
            raise


def _row(db, delivery_id: str) -> dict:
    return dict(db.execute("SELECT * FROM capsule_deliveries WHERE id=?", (delivery_id,)).fetchone())


def _transition(store, delivery_id: str, *, expected_from: tuple[str, ...], to: str, evidence_source: str, **extra) -> dict | None:
    now = store._now()
    columns = ", ".join(f"{key}=?" for key in extra) + (", " if extra else "")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT * FROM capsule_deliveries WHERE id=?", (delivery_id,)).fetchone()
            if row is None or row["state"] not in expected_from:
                db.commit()
                return None
            db.execute(
                f"UPDATE capsule_deliveries SET state=?, {columns}evidence_source=?, updated_at=? WHERE id=?",
                (to, *extra.values(), evidence_source, now, delivery_id),
            )
            db.commit()
            return _row(db, delivery_id)
        except BaseException:
            db.rollback()
            raise


def mark_transmitted(store, delivery_id: str, evidence_source: str) -> dict | None:
    return _transition(store, delivery_id, expected_from=("built",), to="transmitted", evidence_source=evidence_source)


def mark_delivery_confirmed(store, delivery_id: str, evidence_source: str) -> dict | None:
    return _transition(
        store, delivery_id, expected_from=("transmitted",), to="delivery_confirmed", evidence_source=evidence_source,
    )


def mark_receipt_uncertain(store, delivery_id: str, uncertainty: str) -> dict | None:
    """The turn that should have confirmed delivery instead timed out/failed
    transport-side: the row *stays* ``transmitted`` (never promoted to
    ``delivery_confirmed`` on a guess) with ``uncertainty`` recorded."""
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT * FROM capsule_deliveries WHERE id=?", (delivery_id,)).fetchone()
            if row is None or row["state"] != "transmitted":
                db.commit()
                return None
            db.execute(
                "UPDATE capsule_deliveries SET uncertainty=?, updated_at=? WHERE id=?",
                (uncertainty, now, delivery_id),
            )
            db.commit()
            return _row(db, delivery_id)
        except BaseException:
            db.rollback()
            raise


def mark_continuation_observed(store, delivery_id: str, evidence_source: str) -> dict | None:
    return _transition(
        store, delivery_id, expected_from=("delivery_confirmed",), to="continuation_observed",
        evidence_source=evidence_source,
    )


def mark_continuation_observed_if_pending(store, engine_session_ref: str, evidence_source: str) -> list[dict]:
    """Called immediately before a *new* ``session/prompt`` call on
    ``engine_session_ref``: every not-yet-``continuation_observed``
    ``delivery_confirmed`` row for this session is a delivery whose
    "subsequent engine turn" evidence has just arrived (the very fact this
    next turn is starting). Returns the (possibly empty) list of rows
    advanced."""
    with store.connection() as db:
        ids = [
            row["id"] for row in db.execute(
                "SELECT id FROM capsule_deliveries WHERE engine_session_ref=? AND state='delivery_confirmed'",
                (engine_session_ref,),
            ).fetchall()
        ]
    return [row for row in (mark_continuation_observed(store, delivery_id, evidence_source) for delivery_id in ids) if row is not None]


def mark_blocked(store, delivery_id: str, reason: str, evidence_source: str) -> dict | None:
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT * FROM capsule_deliveries WHERE id=?", (delivery_id,)).fetchone()
            if row is None or row["state"] in _TERMINAL_STATES:
                db.commit()
                return None
            db.execute(
                "UPDATE capsule_deliveries SET state='blocked', reason=?, evidence_source=?, updated_at=? WHERE id=?",
                (reason, evidence_source, now, delivery_id),
            )
            db.commit()
            return _row(db, delivery_id)
        except BaseException:
            db.rollback()
            raise


def supersede_session_deliveries(store, engine_session_ref: str, reason: str) -> list[dict]:
    """A session was released/replaced: every one of its deliveries not
    already ``continuation_observed`` (a delivery this session actually
    continued past) becomes ``superseded`` -- it can never later be
    reported ``delivery_confirmed``/``continuation_observed`` from a stale
    in-flight call that resolves after this point."""
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            ids = [
                row["id"] for row in db.execute(
                    "SELECT id FROM capsule_deliveries WHERE engine_session_ref=? AND state NOT IN ('continuation_observed','superseded')",
                    (engine_session_ref,),
                ).fetchall()
            ]
            for delivery_id in ids:
                db.execute(
                    "UPDATE capsule_deliveries SET state='superseded', reason=?, evidence_source=?, updated_at=? WHERE id=?",
                    (reason, "session_released_or_replaced", now, delivery_id),
                )
            db.commit()
            return [_row(db, delivery_id) for delivery_id in ids]
        except BaseException:
            db.rollback()
            raise


def get(store, delivery_id: str) -> dict | None:
    with store.connection() as db:
        row = db.execute("SELECT * FROM capsule_deliveries WHERE id=?", (delivery_id,)).fetchone()
    return dict(row) if row is not None else None


def for_generation(store, recipient: str, context_generation_id: str) -> list[dict]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM capsule_deliveries WHERE recipient=? AND context_generation_id=? ORDER BY id",
            (recipient, context_generation_id),
        ).fetchall()
    return [dict(row) for row in rows]
