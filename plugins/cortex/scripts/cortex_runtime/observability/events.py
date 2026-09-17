"""Append/read for ``observability_events`` (see :mod:`observability.store`).

Every append site chooses ``basis`` explicitly out of the same closed set
``recovery.compaction_guard``/``recovery.token_budget`` already use
(``observed``/``engine_reported``/``derived``/``estimated``/``unknown``) --
never defaulted, since a silent default is exactly how "unknown" quietly
becomes "0"/"observed" (the failure mode INVARIANTS.md forbids).
"""
from __future__ import annotations

import json
from typing import Any, Mapping

from ..portable.identity import new_id
from .store import BASIS_VALUES


def _now(store) -> str:
    return store._now()


def append_event_tx(
    db, task_id: str, category: str, outcome: str, basis: str, *,
    assignment_ref: str | None = None, attempt_id: str | None = None,
    engine_session_ref: str | None = None, context_generation_id: str | None = None,
    config_revision_id: str | None = None, connection_id: str | None = None,
    route_engine: str | None = None, source_event_ref: str | None = None,
    detail: Mapping[str, Any] | None = None, created_at: str,
) -> str:
    """Insert one ``observability_events`` row on an already-open
    transaction (``db``); performs no transaction control of its own -- for
    a caller that must commit this atomically with another write (mirrors
    ``delegation.events.append_event_tx``)."""
    if basis not in BASIS_VALUES:
        raise ValueError(f"unknown basis {basis!r}")
    event_id = new_id("obs_")
    db.execute(
        "INSERT INTO observability_events "
        "(id, task_id, category, outcome, basis, assignment_ref, attempt_id, engine_session_ref, "
        " context_generation_id, config_revision_id, connection_id, route_engine, source_event_ref, "
        " detail_json, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        (
            event_id, task_id, category, outcome, basis, assignment_ref, attempt_id, engine_session_ref,
            context_generation_id, config_revision_id, connection_id, route_engine, source_event_ref,
            json.dumps(dict(detail or {}), sort_keys=True, ensure_ascii=False), created_at,
        ),
    )
    return event_id


def append_event(store, task_id: str, category: str, outcome: str, basis: str, **kwargs) -> str:
    """Same as :func:`append_event_tx`, in its own standalone transaction."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            event_id = append_event_tx(db, task_id, category, outcome, basis, created_at=_now(store), **kwargs)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return event_id


def record_duplicate_publication_tx(db, task_id: str, operation: str, request_key: str, *, created_at: str) -> str:
    """A ``store._call`` request whose ``(scope, operation, delivery_key)``
    already has a confirmed ``deliveries`` row (see ``store.py``'s
    ``previous`` branch) -- an idempotent replay, not a new mutation, but a
    genuine *duplicate publication attempt* for the T029 metric. Recorded
    in the same transaction as the read that detected it."""
    return append_event_tx(
        db, task_id, "publication", "duplicate_attempt", "observed",
        detail=dict(operation=operation, request_key=request_key), created_at=created_at,
    )


def list_events(store, task_id: str, *, category: str | None = None, after: int = 0, limit: int = 200) -> list[dict]:
    limit = max(1, min(int(limit), 500))
    query = "SELECT rowid AS cursor, * FROM observability_events WHERE task_id=? AND rowid>?"
    params: list[Any] = [task_id, int(after)]
    if category is not None:
        query += " AND category=?"
        params.append(category)
    query += " ORDER BY rowid LIMIT ?"
    params.append(limit)
    with store.connection() as db:
        rows = db.execute(query, params).fetchall()
    return [
        dict(
            cursor=row["cursor"], id=row["id"], task_id=row["task_id"], category=row["category"],
            outcome=row["outcome"], basis=row["basis"], assignment_ref=row["assignment_ref"],
            attempt_id=row["attempt_id"], engine_session_ref=row["engine_session_ref"],
            context_generation_id=row["context_generation_id"], config_revision_id=row["config_revision_id"],
            connection_id=row["connection_id"], route_engine=row["route_engine"],
            source_event_ref=row["source_event_ref"], detail=json.loads(row["detail_json"]),
            created_at=row["created_at"],
        )
        for row in rows
    ]
