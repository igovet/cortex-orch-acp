"""T017 durable tool-call evidence over ``tool_result_observations`` (see
``delegation.evidence_store``).

Bridges an engine's own ``TOOL_CALL_OBSERVED`` updates (see
``engine.session_manager``/``portable.engine_contract.EngineEvent``) into a
durable per-attempt record, and resolves a worker's claimed
``publish_result`` ``checks`` (see ``delegation.contracts``) against it:

- ``claimed`` -- the check is in the worker's own claim; no linked
  observation (or a ``tool_call_ref`` that only resolves under a
  *different* attempt -- never trusted across attempts).
- ``observed`` -- the linked ``tool_call_ref``'s latest observation *for
  this exact attempt* has ``status == "completed"``.
- ``contradicted`` -- the linked observation's latest status is
  ``"failed"``.

Nothing here ever marks a claim ``observed``/confirmed without an actual
matching row from this attempt's own observations.
"""
from __future__ import annotations

from datetime import datetime, timezone


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def record_tool_result_observed(
    store, task_id: str, attempt_id: str, tool_call_ref: str | None, kind: str | None,
    title: str | None, status: str | None,
) -> None:
    """Durably record one observed tool-call status transition.

    A no-op when ``tool_call_ref`` or ``status`` is missing (an engine
    update the correlation table could not resolve, or with no status --
    nothing evidential to record).
    """
    if not tool_call_ref or not status:
        return
    bounded_title = (title or "")[:200] or None
    bounded_kind = (kind or "")[:64] or None
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO tool_result_observations (task_id, attempt_id, tool_call_ref, kind, title, status, created_at) "
                "VALUES (?,?,?,?,?,?,?)",
                (task_id, attempt_id, tool_call_ref, bounded_kind, bounded_title, status, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def _latest_status(store, attempt_id: str, tool_call_ref: str) -> str | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT status FROM tool_result_observations WHERE attempt_id=? AND tool_call_ref=? ORDER BY id DESC LIMIT 1",
            (attempt_id, tool_call_ref),
        ).fetchone()
    return row["status"] if row is not None else None


def resolve_check(store, attempt_id: str, tool_call_ref: str | None) -> str:
    """Return ``"claimed"``, ``"observed"``, or ``"contradicted"`` for one
    ``publish_result`` check's ``tool_call_ref`` -- strictly scoped to
    ``attempt_id`` (a ``tool_call_ref`` that only resolves under a
    different attempt is indistinguishable from "no match": never trusted
    cross-attempt)."""
    if not tool_call_ref:
        return "claimed"
    status = _latest_status(store, attempt_id, tool_call_ref)
    if status == "completed":
        return "observed"
    if status == "failed":
        return "contradicted"
    return "claimed"
