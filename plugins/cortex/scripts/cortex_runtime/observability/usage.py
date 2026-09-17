"""Usage/cost attribution: one immutable ``usage_records`` row per completed
engine ``prompt()`` call (see :mod:`observability.store`'s module
docstring). Called from
``delegation.live_adapters.LiveEngineAdapters.prompt`` for *every* call --
a coordinator continuation, a worker assignment, a retry/fallback hop, or a
recovery-capsule/clarification delivery -- never only the final successful
one, so retry/recovery work is never charged as free.

Route/connection/model/config-revision are read at attribution time by
joining ``attempt_route_audit`` (T027) on ``attempt_id`` -- see
:func:`attribute_usage` -- instead of duplicating those columns onto every
usage row.
"""
from __future__ import annotations

import json
from typing import Any, Mapping

from ..portable.engine_contract import UsageBasis
from ..portable.identity import new_id
from .store import BASIS_VALUES


def record_call_usage(
    store, *, task_id: str, attempt_id: str | None, engine_session_ref: str,
    usage_basis: "UsageBasis | str | None", usage_values: Mapping[str, int] | None,
    stop_reason: str | None, failed: bool,
) -> str:
    """Persist one call's usage. ``usage_basis`` is ``None`` (never
    reported at all during this call, e.g. an adapter that emits no
    ``usage_update`` events) or ``UsageBasis.UNKNOWN`` -- both are stored
    as ``basis='unknown'`` with empty ``tokens_json``, distinct from an
    engine that reported and the figure happened to be ``0``."""
    basis = usage_basis.value if isinstance(usage_basis, UsageBasis) else usage_basis
    if basis is None:
        basis = UsageBasis.UNKNOWN.value
    if basis not in BASIS_VALUES:
        raise ValueError(f"unknown usage basis {basis!r}")
    tokens = dict(usage_values or {}) if basis != UsageBasis.UNKNOWN.value else {}
    record_id = new_id("use_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            sequence = db.execute(
                "SELECT COALESCE(MAX(call_sequence), 0) + 1 FROM usage_records WHERE engine_session_ref=?",
                (engine_session_ref,),
            ).fetchone()[0]
            db.execute(
                "INSERT INTO usage_records "
                "(id, task_id, attempt_id, engine_session_ref, call_sequence, basis, tokens_json, "
                " stop_reason, failed, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
                (
                    record_id, task_id, attempt_id, engine_session_ref, sequence, basis,
                    json.dumps(tokens, sort_keys=True), stop_reason, 1 if failed else 0, store._now(),
                ),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return record_id


def _route_for_attempt(db, attempt_id: str | None) -> dict:
    if attempt_id is None:
        return dict(engine=None, connection_id=None, model=None, config_revision_id=None, route_basis="unknown")
    row = db.execute(
        "SELECT engine, connection_id, model, config_revision_id FROM attempt_route_audit "
        "WHERE attempt_id=? ORDER BY recorded_at DESC LIMIT 1",
        (attempt_id,),
    ).fetchone()
    if row is None:
        return dict(engine=None, connection_id=None, model=None, config_revision_id=None, route_basis="unknown")
    return dict(
        engine=row["engine"], connection_id=row["connection_id"], model=row["model"],
        config_revision_id=row["config_revision_id"], route_basis="derived",
    )


def list_usage(store, task_id: str, *, attempt_id: str | None = None, limit: int = 500) -> list[dict]:
    """Every usage record for ``task_id`` (optionally narrowed to one
    ``attempt_id``), each enriched with its attempt's own route/connection
    (``route_basis='derived'`` -- joined from ``attempt_route_audit``, not
    stored a second time; ``'unknown'`` when this call has no attempt_id or
    no route audit row was ever recorded for it, e.g. a coordinator's own
    continuation turn before T027-style per-attempt route auditing applies
    to it)."""
    limit = max(1, min(int(limit), 2000))
    query = "SELECT * FROM usage_records WHERE task_id=?"
    params: list[Any] = [task_id]
    if attempt_id is not None:
        query += " AND attempt_id=?"
        params.append(attempt_id)
    query += " ORDER BY created_at LIMIT ?"
    params.append(limit)
    with store.connection() as db:
        rows = db.execute(query, params).fetchall()
        results = []
        for row in rows:
            entry = dict(
                id=row["id"], task_id=row["task_id"], attempt_id=row["attempt_id"],
                engine_session_ref=row["engine_session_ref"], call_sequence=row["call_sequence"],
                basis=row["basis"], tokens=json.loads(row["tokens_json"]), stop_reason=row["stop_reason"],
                failed=bool(row["failed"]), created_at=row["created_at"],
            )
            entry.update(_route_for_attempt(db, row["attempt_id"]))
            results.append(entry)
    return results


def usage_summary(store, task_id: str) -> dict:
    """Aggregate call counts and (only where reported) summed token
    figures, grouped by whether the engine actually reported usage for
    that call. Unknown-basis calls are counted separately and are never
    folded into ``tokens_reported`` as zero."""
    records = list_usage(store, task_id, limit=2000)
    by_connection: dict[str, dict[str, Any]] = {}
    total_calls = 0
    unknown_calls = 0
    failed_calls = 0
    for entry in records:
        total_calls += 1
        if entry["failed"]:
            failed_calls += 1
        connection_key = entry["connection_id"] or "unattributed"
        bucket = by_connection.setdefault(
            connection_key, dict(calls=0, failed_calls=0, unknown_basis_calls=0, tokens_reported={}),
        )
        bucket["calls"] += 1
        if entry["failed"]:
            bucket["failed_calls"] += 1
        if entry["basis"] == "unknown":
            unknown_calls += 1
            bucket["unknown_basis_calls"] += 1
        else:
            for token_key, token_value in entry["tokens"].items():
                bucket["tokens_reported"][token_key] = bucket["tokens_reported"].get(token_key, 0) + token_value
    return dict(
        task_id=task_id, total_calls=total_calls, unknown_basis_calls=unknown_calls,
        failed_calls=failed_calls, by_connection=by_connection,
        note=(
            "unknown_basis_calls are calls this engine reported no usage_update for; they are "
            "never summed into tokens_reported as zero -- see docs/acp/observability.md."
        ),
    )
