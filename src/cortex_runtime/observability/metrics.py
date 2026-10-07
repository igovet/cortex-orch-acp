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

import json
import re

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
            availability="unavailable_reason",
            reason="no configuration_revisions row recorded for this task yet",
        )
    ceiling = deserialize_ceiling(row["ceiling_snapshot"])
    monetary_cap = None
    monetary_basis = "unavailable_reason:no_enforced_spend_limit_configured"
    if ceiling.spend_limit is not None and ceiling.spend_limit_enforceable:
        monetary_cap = ceiling.spend_limit
        monetary_basis = "enforced"
    return dict(
        availability="available",
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
    rows = [
        row for category in ("validation", "submission_rejection")
        for row in observability_events.list_events(store, task_id, category=category, limit=500)
    ]
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


def _base_prompt(store, task_id: str) -> dict:
    rows = observability_events.list_events(store, task_id, category="base_prompt", limit=500)
    counts: dict[str, int] = {}
    for row in rows:
        mode = row["outcome"]
        counts[mode] = counts.get(mode, 0) + 1
    if not rows:
        return dict(
            unit="sessions", availability="unavailable_reason", basis="observed", counts=counts,
            reason="no recorded base-prompt mode for this task",
        )
    return dict(unit="sessions", availability="available", basis="observed", counts=counts)


def _tokens(store, task_id: str) -> dict:
    summary = observability_usage.usage_summary(store, task_id)
    # F1 fix: a released_after_result call never carries a real reported
    # count either (see observability.usage.usage_summary), so it must not
    # make this metric falsely claim availability on its own.
    never_reported = summary["unknown_basis_calls"] + summary["released_after_result_calls"]
    availability = "available" if summary["total_calls"] > never_reported else "unavailable_reason"
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


_FIX_NODE = re.compile(r"^_fix_(\d+)_(.+)$")
_MAX_ROUTE_METRIC_GROUPS = 100


def _route_node_identity(node_id: str) -> tuple[str, int]:
    match = _FIX_NODE.fullmatch(str(node_id))
    return (match.group(2), int(match.group(1))) if match else (str(node_id), 0)


def _timestamp_delta_seconds(start: str | None, end: str | None) -> float | None:
    from datetime import datetime

    try:
        before = datetime.fromisoformat(str(start).replace("Z", "+00:00"))
        after = datetime.fromisoformat(str(end).replace("Z", "+00:00"))
        if before.utcoffset() is None or after.utcoffset() is None:
            return None
        seconds = (after - before).total_seconds()
    except (TypeError, ValueError, OverflowError):
        return None
    return seconds if seconds >= 0 else None


def _route_evidence(store, task_id: str) -> tuple[list[dict], list[dict]]:
    """Read existing dispatch, result, plan, and route-audit evidence."""
    with store.connection() as db:
        assignments = db.execute(
            "SELECT n.revision, n.node_id, n.role, n.stage, n.assignment_ref, "
            "d.created_at AS dispatched_at, a.engine, a.model "
            "FROM runtime_nodes n "
            "LEFT JOIN task_events d ON d.task_id=n.task_id "
            " AND d.assignment_ref=n.assignment_ref AND d.event_type='delegation_requested' "
            "LEFT JOIN delegation_provisioning p ON p.task_id=n.task_id "
            " AND p.idempotency_key=n.assignment_ref "
            "LEFT JOIN attempt_route_audit a ON a.rowid=("
            " SELECT a2.rowid FROM attempt_route_audit a2 "
            " WHERE a2.task_id=n.task_id AND a2.attempt_id=p.attempt_id "
            " ORDER BY a2.recorded_at DESC, a2.rowid DESC LIMIT 1) "
            "WHERE n.task_id=? AND n.stage='execution'",
            (task_id,),
        ).fetchall()
        events = db.execute(
            "SELECT id, type, payload_json, created_at FROM runtime_events "
            "WHERE task_id=? AND type IN ('PLAN_ACCEPTED','WORKER_COMPLETED','REVIEW_CHANGES_REQUESTED') "
            "ORDER BY id",
            (task_id,),
        ).fetchall()
    return [dict(row) for row in assignments], [dict(row) for row in events]


def _route_metric_key(row: dict, complexities: dict) -> tuple[str, str, str, str]:
    origin, _ = _route_node_identity(row["node_id"])
    complexity = complexities.get((row["revision"], origin), "unknown")
    return (row["engine"], row["model"], row["role"], complexity)


def _route_acceptance_latency(store, task_id: str, assignments: list[dict], events: list[dict]) -> dict:
    by_assignment = {
        row["assignment_ref"]: row for row in assignments if row["assignment_ref"] is not None
    }
    complexities: dict[tuple[int, str], str] = {}
    accepted_events = []
    excluded = dict(missing_node_binding=0, missing_dispatch=0, missing_route_audit=0, invalid_timestamps=0)
    for event_row in events:
        if event_row["type"] == "PLAN_ACCEPTED":
            try:
                payload = json.loads(event_row["payload_json"])
                revision = int(payload["revision"])
                plan = payload["plan"]
                tasks = plan["tasks"]
            except (KeyError, TypeError, ValueError, json.JSONDecodeError):
                continue
            if not isinstance(tasks, list):
                continue
            for planned in tasks:
                if not isinstance(planned, dict) or not isinstance(planned.get("id"), str):
                    continue
                # attempt_routing() uses "normal" when the optional plan hint is absent.
                complexity = planned.get("complexity", "normal")
                complexities[(revision, planned["id"])] = (
                    complexity if isinstance(complexity, str) and complexity else "unknown"
                )
        elif event_row["type"] == "WORKER_COMPLETED":
            try:
                payload = json.loads(event_row["payload_json"])
            except (TypeError, json.JSONDecodeError):
                excluded["missing_node_binding"] += 1
                continue
            if not isinstance(payload, dict):
                excluded["missing_node_binding"] += 1
                continue
            assignment_ref = payload.get("assignment_ref")
            row = by_assignment.get(assignment_ref)
            if row is None or payload.get("node") != row["node_id"]:
                excluded["missing_node_binding"] += 1
                continue
            accepted_events.append((row, event_row["created_at"]))

    grouped: dict[tuple[str, str, str, str], list[float]] = {}
    execution_result_count = 0
    for row, accepted_at in accepted_events:
        execution_result_count += 1
        if row["dispatched_at"] is None:
            excluded["missing_dispatch"] += 1
            continue
        if not row["engine"] or not row["model"]:
            excluded["missing_route_audit"] += 1
            continue
        seconds = _timestamp_delta_seconds(row["dispatched_at"], accepted_at)
        if seconds is None:
            excluded["invalid_timestamps"] += 1
            continue
        key = _route_metric_key(row, complexities)
        grouped.setdefault(key, []).append(seconds)

    summaries = [
        dict(
            engine=key[0], model=key[1], role=key[2], complexity=key[3],
            sample_count=len(samples), min_seconds=min(samples),
            mean_seconds=sum(samples) / len(samples), max_seconds=max(samples),
        )
        for key, samples in grouped.items()
    ]
    summaries.sort(key=lambda item: (
        -item["sample_count"], item["engine"], item["model"], item["role"], item["complexity"],
    ))
    group_count = len(summaries)
    truncated = group_count > _MAX_ROUTE_METRIC_GROUPS
    summaries = summaries[:_MAX_ROUTE_METRIC_GROUPS]
    sample_count = sum(len(samples) for samples in grouped.values())
    result = dict(
        unit="seconds", basis="derived",
        availability="available" if sample_count else "unavailable_reason",
        sample_count=sample_count, accepted_execution_results=execution_result_count,
        group_by=["engine", "model", "role", "complexity"],
        groups=summaries, group_count=group_count, groups_truncated=truncated,
        excluded=excluded,
    )
    if not sample_count:
        result["reason"] = (
            "no execution result has a complete dispatch timestamp, accepted WORKER_COMPLETED event, "
            "route audit, and parseable non-negative duration"
        )
    return result


def _route_fix_cycles(assignments: list[dict], events: list[dict]) -> dict:
    node_map = {(row["revision"], row["node_id"]): row for row in assignments}
    complexity_by_plan: dict[tuple[int, str], str] = {}
    review_events: list[tuple[int, int | None, dict]] = []
    active_revision: int | None = None
    malformed_review_payloads = 0
    for event_row in events:
        if event_row["type"] == "PLAN_ACCEPTED":
            try:
                payload = json.loads(event_row["payload_json"])
                active_revision = int(payload["revision"])
                tasks = payload["plan"]["tasks"]
            except (KeyError, TypeError, ValueError, json.JSONDecodeError):
                active_revision = None
                continue
            if isinstance(tasks, list):
                for planned in tasks:
                    if not isinstance(planned, dict) or not isinstance(planned.get("id"), str):
                        continue
                    complexity = planned.get("complexity", "normal")
                    complexity_by_plan[(active_revision, planned["id"])] = (
                        complexity if isinstance(complexity, str) and complexity else "unknown"
                    )
        elif event_row["type"] == "REVIEW_CHANGES_REQUESTED":
            try:
                payload = json.loads(event_row["payload_json"])
            except (TypeError, json.JSONDecodeError):
                payload = {}
                malformed_review_payloads += 1
            review_events.append((event_row["id"], active_revision, payload if isinstance(payload, dict) else {}))

    groups: dict[tuple[str, str, str, str], dict] = {}
    attributed_review_events: set[int] = set()
    pending_fix_assignments = 0
    unattributed_dispatched_fix_assignments = 0
    unresolved_fix_assignments = 0
    for event_id, revision, payload in review_events:
        fixes = payload.get("fixes")
        if not isinstance(fixes, list):
            unresolved_fix_assignments += 1
            continue
        for node_id in fixes:
            match = _FIX_NODE.fullmatch(str(node_id))
            if not match:
                unresolved_fix_assignments += 1
                continue
            cycle = int(match.group(1))
            node_id = str(node_id)
            row = node_map.get((revision, node_id))
            if row is None:
                unresolved_fix_assignments += 1
                continue
            if row["assignment_ref"] is None or row["dispatched_at"] is None:
                pending_fix_assignments += 1
                continue
            if not row["engine"] or not row["model"]:
                unattributed_dispatched_fix_assignments += 1
                continue
            origin, _ = _route_node_identity(node_id)
            complexity = complexity_by_plan.get((revision, origin), "unknown")
            key = (row["engine"], row["model"], row["role"], complexity)
            group = groups.setdefault(key, {"cycles": set(), "assignments": set()})
            group["cycles"].add((revision, cycle))
            group["assignments"].add((revision, node_id))
            attributed_review_events.add(event_id)

    summaries = [
        dict(
            engine=key[0], model=key[1], role=key[2], complexity=key[3],
            fix_cycle_count=len(data["cycles"]),
            fix_assignment_count=len(data["assignments"]),
        )
        for key, data in groups.items()
    ]
    summaries.sort(key=lambda item: (
        -item["fix_cycle_count"], -item["fix_assignment_count"],
        item["engine"], item["model"], item["role"], item["complexity"],
    ))
    group_count = len(summaries)
    truncated = group_count > _MAX_ROUTE_METRIC_GROUPS
    summaries = summaries[:_MAX_ROUTE_METRIC_GROUPS]
    total_cycles = len(review_events)
    attributed_cycles = len(attributed_review_events)
    return dict(
        unit="review-requested fix cycles", availability="available", basis="derived",
        total_review_fix_cycles=total_cycles, attributed_review_fix_cycles=attributed_cycles,
        unattributed_review_fix_cycles=total_cycles - attributed_cycles,
        group_by=["engine", "model", "role", "complexity"],
        groups=summaries, group_count=group_count, groups_truncated=truncated,
        pending_fix_assignments=pending_fix_assignments,
        unattributed_dispatched_fix_assignments=unattributed_dispatched_fix_assignments,
        unresolved_fix_assignments=unresolved_fix_assignments,
        malformed_review_payloads=malformed_review_payloads,
        note=(
            "Each group counts distinct review cycles with at least one dispatched fix on that route. "
            "A cycle can appear in multiple groups when its fixes use different routes."
        ),
    )


def route_outcome_metrics(store, task_id: str) -> dict:
    """Derive assignment latency and routed fix-cycle counts from durable task evidence."""
    assignments, events = _route_evidence(store, task_id)
    return dict(
        route_acceptance_latency=_route_acceptance_latency(store, task_id, assignments, events),
        route_fix_cycles=_route_fix_cycles(assignments, events),
    )


def diagnostics_summary(store, task_id: str) -> dict:
    """The T029 read API: every declared metric, each with its own
    ``unit``/``availability``, plus the task's own resource caps. Absent
    optional telemetry never gates this call -- every section always
    returns, unavailable ones simply say so (see each helper above)."""
    return dict(
        resource_caps=resource_caps(store, task_id),
        recovery_latency=_recovery_latency(store, task_id),
        repeat_calls=_repeat_calls(store, task_id),
        schema_validation_errors=_schema_validation_errors(store, task_id),
        manual_interventions=_manual_interventions(store, task_id),
        duplicate_publications=_duplicate_publications(store, task_id),
        base_prompt=_base_prompt(store, task_id),
        tokens=_tokens(store, task_id),
        concurrency=_concurrency(store, task_id),
        **route_outcome_metrics(store, task_id),
    )
