"""T029 read API: resource caps and a diagnostics summary, both built by
joining already-durable state (never a second write path) plus
``observability_events``/``usage_records`` (see :mod:`observability.store`).

Every metric below declares ``unit`` and either ``availability="available"``
or ``availability="unavailable_reason"`` with a plain-text reason -- a
metric this runtime cannot actually compute yet is reported as
unavailable, never silently omitted or defaulted to 0 (INVARIANTS.md:
"Separate observed, engine-reported, derived, estimated, and unknown
facts.").
"""
from __future__ import annotations

from . import events as observability_events
from . import usage as observability_usage


def resource_caps(store, task_id: str) -> dict:
    """The most recently recorded configuration revision's own
    :class:`~configuration.policy.PolicyCeiling`, as actual call/token/
    concurrency limits and -- only when the ceiling itself marks a
    ``spend_limit`` enforceable -- a monetary cap. ``PolicyCeiling``
    already refuses to construct a ``spend_limit`` without
    ``spend_limit_enforceable=True`` (see ``configuration.policy``), so a
    non-``None`` value here is never presented without a demonstrated
    enforcement mechanism behind it.
    """
    from ..configuration.store import deserialize_ceiling

    with store.connection() as db:
        row = db.execute(
            "SELECT ceiling_snapshot, id FROM configuration_revisions WHERE task_id=? ORDER BY created_at DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    if row is None:
        return dict(
            task_id=task_id, availability="unavailable_reason",
            reason="no configuration_revisions row recorded for this task yet",
        )
    ceiling = deserialize_ceiling(row["ceiling_snapshot"])
    monetary_cap = None
    monetary_basis = "unavailable_reason:no_enforced_spend_limit_configured"
    if ceiling.spend_limit is not None and ceiling.spend_limit_enforceable:
        monetary_cap = ceiling.spend_limit
        monetary_basis = "enforced"
    return dict(
        task_id=task_id, config_revision_id=row["id"], availability="available",
        call_token_concurrency_limits=dict(ceiling.limits),
        limits_unit="count (attempts/calls/tokens: per task; concurrency: simultaneous active attempts)",
        monetary_cap=monetary_cap, monetary_cap_unit="opaque integer spend unit" if monetary_cap is not None else None,
        monetary_cap_basis=monetary_basis,
        note=(
            "No exact monetary cap is ever reported unless PolicyCeiling.spend_limit_enforceable is "
            "True (enforced at construction -- configuration.policy.PolicyCeiling). Absent that, only "
            "the call/token/concurrency limits above are conservative caps; treat monetary_cap=None as "
            "'no enforced monetary limit', never as 'unlimited spend is safe'."
        ),
    )


def _recovery_latency(store, task_id: str) -> dict:
    with store.connection() as db:
        rows = db.execute(
            "SELECT a.attempt_id, a.recorded_at, p.recorded_at AS predecessor_recorded_at "
            "FROM attempt_route_audit a JOIN attempt_route_audit p "
            " ON p.attempt_id = a.predecessor_attempt_id "
            "WHERE a.task_id=? AND a.decision='fallback' AND a.predecessor_attempt_id IS NOT NULL",
            (task_id,),
        ).fetchall()
    if not rows:
        return dict(
            unit="seconds", availability="unavailable_reason",
            reason="no recorded fallback attempt (attempt_route_audit.decision='fallback') for this task",
        )
    from datetime import datetime

    samples = []
    for row in rows:
        try:
            before = datetime.fromisoformat(row["predecessor_recorded_at"])
            after = datetime.fromisoformat(row["recorded_at"])
        except ValueError:
            continue
        samples.append((after - before).total_seconds())
    if not samples:
        return dict(unit="seconds", availability="unavailable_reason", reason="fallback timestamps unparsable")
    return dict(
        unit="seconds", availability="available", basis="derived", sample_count=len(samples),
        max_seconds=max(samples), min_seconds=min(samples), mean_seconds=sum(samples) / len(samples),
    )


def _repeat_calls(store, task_id: str) -> dict:
    records = observability_usage.list_usage(store, task_id, limit=2000)
    per_session: dict[str, int] = {}
    for entry in records:
        per_session[entry["engine_session_ref"]] = max(per_session.get(entry["engine_session_ref"], 0), entry["call_sequence"])
    repeated = {session: count for session, count in per_session.items() if count > 1}
    return dict(
        unit="count (calls beyond the first, per engine session)", availability="available", basis="observed",
        sessions_with_repeat_calls=len(repeated), total_repeat_calls=sum(count - 1 for count in repeated.values()),
    )


def _schema_validation_errors(store, task_id: str) -> dict:
    rows = observability_events.list_events(store, task_id, category="validation", limit=500)
    return dict(unit="count", availability="available", basis="observed", total=len(rows))


def _manual_interventions(store, task_id: str) -> dict:
    with store.connection() as db:
        count = db.execute("SELECT COUNT(*) FROM pending_questions WHERE task_id=?", (task_id,)).fetchone()[0]
    return dict(
        unit="count", availability="available", basis="observed", worker_or_coordinator_questions=count,
        permission_dialogs=dict(
            availability="unavailable_reason",
            reason="permission requests/responses are recorded only in the per-engine-instance "
                   "observations JSONL (engine.logs.EngineLogs), not indexed by task_id in SQLite.",
        ),
    )


def _duplicate_publications(store, task_id: str) -> dict:
    rows = observability_events.list_events(store, task_id, category="publication", limit=500)
    duplicates = [row for row in rows if row["outcome"] == "duplicate_attempt"]
    return dict(unit="count", availability="available", basis="observed", total=len(duplicates))


def _tokens(store, task_id: str) -> dict:
    summary = observability_usage.usage_summary(store, task_id)
    availability = "available" if summary["total_calls"] > summary["unknown_basis_calls"] else "unavailable_reason"
    result = dict(unit="tokens (per provider-reported field name)", availability=availability, **summary)
    if availability == "unavailable_reason":
        result["reason"] = "no engine reported usage_update for any recorded call on this task"
    return result


def _concurrency(store, task_id: str) -> dict:
    with store.connection() as db:
        row = db.execute(
            "SELECT COUNT(*) FROM capacity_reservations WHERE task_id=? AND state='reserved'", (task_id,),
        ).fetchone()
    return dict(unit="count (currently reserved/active capacity reservations)", availability="available", basis="observed", active=row[0])


def diagnostics_summary(store, task_id: str) -> dict:
    """The T029 read API: every declared metric, each with its own
    ``unit``/``availability``, plus the task's own resource caps. Absent
    optional telemetry never gates this call -- every section always
    returns, unavailable ones simply say so (see each helper above)."""
    return dict(
        task_id=task_id,
        resource_caps=resource_caps(store, task_id),
        recovery_latency=_recovery_latency(store, task_id),
        repeat_calls=_repeat_calls(store, task_id),
        schema_validation_errors=_schema_validation_errors(store, task_id),
        manual_interventions=_manual_interventions(store, task_id),
        duplicate_publications=_duplicate_publications(store, task_id),
        tokens=_tokens(store, task_id),
        concurrency=_concurrency(store, task_id),
    )
