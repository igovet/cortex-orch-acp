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
    """Prompt() re-deliveries to the engine (stall-nudge/refusal
    continuations, recovery-capsule and clarification redeliveries) --
    NOT repeated tool calls; see :func:`_loop_profile` for those. Named
    ``repeat_calls`` for backward read-compatibility, but the unit/note
    below make the actual meaning explicit (Stage 1 audit contradiction:
    the old label read as if it counted tool-call loops)."""
    records = observability_usage.list_usage(store, task_id, limit=2000)
    per_session: dict[str, int] = {}
    for entry in records:
        per_session[entry["engine_session_ref"]] = max(per_session.get(entry["engine_session_ref"], 0), entry["call_sequence"])
    repeated = {session: count for session, count in per_session.items() if count > 1}
    truncated = len(records) >= 2000
    return dict(
        unit="count (ACP prompt() re-deliveries beyond the first call, per engine session -- never repeated "
             "tool calls)",
        availability="available", basis="observed",
        sessions_with_repeat_calls=len(repeated), total_repeat_calls=sum(count - 1 for count in repeated.values()),
        truncated=truncated,
        note=(
            "This counts ACP prompt() re-deliveries to the engine (stall-nudge/refusal continuations, "
            "recovery-capsule/clarification redeliveries) -- never repeated gateway/external/native tool "
            "calls (see loop_profile for those). observability.usage.list_usage caps a single read at 2000 "
            "rows; when truncated is true this task's real total can exceed what was counted here."
        ),
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
    # Per-engine breakdown of the same ``native``/``unified``/``unknown``
    # outcome vocabulary: ``native`` means this session's engine kept its
    # own native system-prompt/instructions channel (Cortex text appended
    # alongside it, e.g. Claude's ``systemPrompt.append`` or Codex's
    # ``cortexNativeContext``); ``unified`` means that native channel was
    # replaced outright by Cortex's own base-prompt text (see
    # ``delegation.live_adapters.base_prompt_mode``, the existing
    # classification reused here -- never reinvented).
    counts_by_engine: dict[str, dict[str, int]] = {}
    # Recorded only once a session actually carried a numeric
    # ``prompt_edition`` in its event detail (see ``engine.unified_system_
    # prompt.PROMPT_EDITION``); older events without it are not counted here.
    editions: dict[str, int] = {}
    # Sessions whose bundle named a situational/catalog `read_skill` entry
    # that would currently fail to load (see
    # ``portable.instruction_bundle.catalog_resolution_gaps``). Advisory
    # only: this never blocked that session. Only catalog skill ids are
    # recorded, never raw skill text or message content.
    sessions_with_unresolved_catalog_entries = 0
    unresolved_catalog_entry_ids: set[str] = set()
    for row in rows:
        mode = row["outcome"]
        counts[mode] = counts.get(mode, 0) + 1
        engine_counts = counts_by_engine.setdefault(row.get("route_engine") or "unknown", {})
        engine_counts[mode] = engine_counts.get(mode, 0) + 1
        detail = row.get("detail")
        if not isinstance(detail, dict):
            continue
        edition = detail.get("prompt_edition")
        if isinstance(edition, int):
            key = str(edition)
            editions[key] = editions.get(key, 0) + 1
        ids = detail.get("unresolved_catalog_ids")
        if isinstance(ids, list) and ids:
            sessions_with_unresolved_catalog_entries += 1
            unresolved_catalog_entry_ids.update(str(skill_id) for skill_id in ids)
    if not rows:
        return dict(
            unit="sessions", availability="unavailable_reason", basis="observed", counts=counts,
            counts_by_engine=counts_by_engine, editions=editions,
            sessions_with_unresolved_catalog_entries=sessions_with_unresolved_catalog_entries,
            unresolved_catalog_entry_ids=sorted(unresolved_catalog_entry_ids),
            reason="no recorded base-prompt mode for this task",
        )
    return dict(
        unit="sessions", availability="available", basis="observed", counts=counts,
        counts_by_engine=counts_by_engine, editions=editions,
        sessions_with_unresolved_catalog_entries=sessions_with_unresolved_catalog_entries,
        unresolved_catalog_entry_ids=sorted(unresolved_catalog_entry_ids),
    )


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


def _percentile(samples: list[float], fraction: float) -> float | None:
    if not samples:
        return None
    ordered = sorted(samples)
    index = min(len(ordered) - 1, int(fraction * len(ordered)))
    return ordered[index]


def _merge_intervals(intervals: list[tuple[float, float]]) -> list[list[float]]:
    """Union of closed (start,end) second-epoch intervals -- an overlap is
    merged into one, never summed twice (critical_path's own requirement)."""
    ordered = sorted((start, end) for start, end in intervals if end >= start)
    merged: list[list[float]] = []
    for start, end in ordered:
        if merged and start <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    return merged


def _interval_seconds(merged: list[list[float]]) -> float:
    return sum(end - start for start, end in merged)


_MAX_LOOP_PROFILE_ROWS = 5000


def _tool_class_counts(store, task_id: str) -> tuple[dict, bool]:
    """Per-attempt observed tool-call counts by class, each read from its
    own authoritative table -- never summed across tables that could
    double count the same call."""
    with store.connection() as db:
        gateway = db.execute(
            "SELECT c.attempt_id AS attempt_id, COUNT(*) AS n FROM runtime_events e "
            "JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id "
            "WHERE e.task_id=? AND e.type='TOOL_AUTHORIZED' AND c.attempt_id IS NOT NULL "
            "GROUP BY c.attempt_id", (task_id,)).fetchall()
        reads = db.execute(
            "SELECT attempt_id, COUNT(*) AS n FROM observability_events "
            "WHERE task_id=? AND category='project_read' AND attempt_id IS NOT NULL "
            "GROUP BY attempt_id", (task_id,)).fetchall()
        external = db.execute(
            "SELECT c.attempt_id AS attempt_id, COUNT(*) AS n FROM runtime_events e "
            "JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id "
            "WHERE e.task_id=? AND e.type='EXTERNAL_TOOL_AUTHORIZED' AND c.attempt_id IS NOT NULL "
            "GROUP BY c.attempt_id", (task_id,)).fetchall()
        acp_total = db.execute(
            "SELECT attempt_id, COUNT(DISTINCT tool_call_ref) AS n FROM tool_result_observations "
            "WHERE task_id=? GROUP BY attempt_id", (task_id,)).fetchall()
    per_attempt: dict[str, dict] = {}
    any_rows = bool(gateway or reads or external or acp_total)
    for rows, key in ((gateway, 'cortex_gateway_calls'), (reads, 'project_read_calls'),
                      (external, 'external_mcp_calls'), (acp_total, 'acp_reported_calls')):
        for row in rows:
            per_attempt.setdefault(row['attempt_id'], {})[key] = row['n']
    return per_attempt, any_rows


def _call_durations(store, task_id, external=False):
    """Unique, ordered boundaries bound to actor/server/request identity."""
    types = ('EXTERNAL_TOOL_AUTHORIZED', 'EXTERNAL_TOOL_RESULT') if external else (
        'TOOL_AUTHORIZED', 'TOOL_COMPLETED', 'TOOL_REJECTED')
    with store.connection() as db:
        rows = db.execute(
            "SELECT e.id,e.type,e.actor,e.created_at,c.attempt_id,"
            "json_extract(e.payload_json,'$.server') AS server,"
            "json_extract(e.payload_json,'$.request_id') AS request_id,"
            "json_extract(e.payload_json,'$.tool_call_ref') AS tool_call_ref "
            "FROM runtime_events e LEFT JOIN trusted_channels c "
            "ON c.task_id=e.task_id AND c.channel_id=e.actor "
            "WHERE e.task_id=? AND e.type IN (" + ",".join("?" for _ in types) + ") "
            "ORDER BY e.id LIMIT ?", (task_id, *types, _MAX_LOOP_PROFILE_ROWS + 1)).fetchall()
    truncated = len(rows) > _MAX_LOOP_PROFILE_ROWS
    groups = {}
    for row in rows[:_MAX_LOOP_PROFILE_ROWS]:
        key = (row['actor'], row['server'], row['request_id']) if external else (
            row['actor'], row['tool_call_ref'])
        groups.setdefault(key, []).append(row)
    totals, invalid = {}, set()
    for key, events in groups.items():
        attempt = events[0]['attempt_id']
        starts = [r for r in events if r['type'] == types[0]]
        ends = [r for r in events if r['type'] != types[0]]
        seconds = None
        if all(v is not None for v in key) and len(starts) == len(ends) == 1 and starts[0]['id'] < ends[0]['id']:
            seconds = _timestamp_delta_seconds(starts[0]['created_at'], ends[0]['created_at'])
        if seconds is None:
            invalid.add(attempt)
        else:
            totals[attempt] = totals.get(attempt, 0.0) + seconds
    if truncated:
        invalid.update(totals)
    return {a: v for a, v in totals.items() if a not in invalid}, truncated, invalid


def _acp_spans(store, task_id):
    with store.connection() as db:
        rows = db.execute(
            "SELECT attempt_id,tool_call_ref,kind,created_at,status FROM tool_result_observations "
            "WHERE task_id=? ORDER BY created_at,id LIMIT ?",
            (task_id, _MAX_LOOP_PROFILE_ROWS + 1)).fetchall()
    truncated = len(rows) > _MAX_LOOP_PROFILE_ROWS
    groups = {}
    for row in rows[:_MAX_LOOP_PROFILE_ROWS]:
        groups.setdefault((row['attempt_id'], row['tool_call_ref']), []).append(row)
    spans = {}
    for key, events in groups.items():
        first = _parse_event_timestamp(events[0]['created_at'])
        terminals = [r for r in events if r['status'] in ('completed', 'failed')]
        terminal = _parse_event_timestamp(terminals[0]['created_at']) if len(terminals) == 1 else None
        kinds = {r['kind'] for r in events}
        spans[key] = dict(first=first.timestamp() if first else None,
                          terminal=terminal.timestamp() if terminal else None,
                          kind=next(iter(kinds)) if len(kinds) == 1 else None,
                          started=events[0]['status'] not in ('completed', 'failed'))
    return spans, truncated


def _loop_profile(store, task_id: str) -> dict:
    per_attempt, any_rows = _tool_class_counts(store, task_id)
    if not any_rows:
        return _unavailable("no observed tool-call rows recorded for this task")
    spans, timing_truncated = _acp_spans(store, task_id)
    gateway_times, gateway_truncated, gateway_invalid = _call_durations(store, task_id)
    external_times, external_truncated, external_invalid = _call_durations(store, task_id, external=True)
    critical = _critical_path(store, task_id)
    lifecycle = {a['attempt_id']: a for a in critical.get('per_attempt', [])}
    waits = critical.get('user_wait_intervals', [])
    ordered = {}
    for (attempt, ref), span in spans.items():
        ordered.setdefault(attempt, []).append(span)
    attempts = sorted({a for a in per_attempt if a} | set(ordered))
    output = []
    for attempt in attempts:
        items = sorted(ordered.get(attempt, []), key=lambda s: s['first'] if s['first'] is not None else float('inf'))
        gaps = []
        spans_without_resolved_timestamp = sum(
            1 for s in items if s['first'] is None or s['terminal'] is None or s['terminal'] < s['first'])
        gaps_complete = not timing_truncated and spans_without_resolved_timestamp == 0
        spans_marked_concurrent = 0
        for previous, current in zip(items, items[1:]):
            if previous['terminal'] is not None and current['first'] is not None:
                if current['first'] >= previous['terminal']:
                    gaps.append([previous['terminal'], current['first']])
                else:
                    # Concurrent tools (the model issued more than one tool
                    # call before the first one's terminal event landed)
                    # make this simple sequential-gap proxy ambiguous.
                    gaps_complete = False
                    spans_marked_concurrent += 1
        clean_gaps = _subtract_intervals(gaps, waits)
        gap_sum = _interval_seconds(clean_gaps) if gaps_complete and len(items) >= 2 else None
        life = lifecycle.get(attempt, {})
        duration = life.get('execution_seconds_excluding_user_waits')
        gap_share = gap_sum / duration if gap_sum is not None and duration and gap_sum <= duration else None
        if gap_sum is not None:
            gap_unavailable_reason = None
        elif timing_truncated:
            gap_unavailable_reason = 'acp tool-result observation rows truncated for this task'
        elif len(items) < 2:
            gap_unavailable_reason = 'fewer than two observed ACP tool spans for this attempt'
        elif spans_without_resolved_timestamp:
            gap_unavailable_reason = 'one or more ACP tool spans lack a resolved first/terminal timestamp'
        elif spans_marked_concurrent:
            gap_unavailable_reason = ('concurrent/overlapping ACP tool spans (the model issued more than one '
                                       'tool call before a prior one completed) make the sequential gap proxy '
                                       'ambiguous for this attempt')
        else:
            gap_unavailable_reason = 'inference gap could not be established for this attempt'
        if gap_share is not None:
            gap_share_unavailable_reason = None
        elif gap_sum is None:
            gap_share_unavailable_reason = gap_unavailable_reason
        elif duration is None:
            gap_share_unavailable_reason = 'execution_seconds_excluding_user_waits unavailable for this attempt'
        elif not duration:
            gap_share_unavailable_reason = 'execution_seconds_excluding_user_waits is not positive for this attempt'
        else:
            gap_share_unavailable_reason = 'inference gap sum exceeds the attempt execution duration'
        counts = per_attempt.get(attempt, {})
        # ACP already includes gateway echoes; never add its count to them.
        # The complete distinct count is observed-as-reported, not exact inference steps.
        total = counts.get('acp_reported_calls')
        if total is None or timing_truncated:
            total = None
        lengths = [_interval_seconds(_subtract_intervals([gap], waits)) for gap in gaps]
        output.append(dict(
            attempt_id=attempt, stage=life.get('stage'),
            execution_seconds_excluding_user_waits=duration,
            cortex_gateway_calls=counts.get('cortex_gateway_calls', 0),
            project_read_calls=counts.get('project_read_calls', 0),
            external_mcp_calls=counts.get('external_mcp_calls', 0),
            acp_reported_calls=counts.get('acp_reported_calls', 0),
            tool_calls_for_decision=total,
            cortex_delegation_calls=_unavailable("ACP kind does not identify delegation versus native tools"),
            inference_gap_seconds=dict(availability='available' if gap_sum is not None else 'unavailable_reason',
                                       basis='estimated', sample_count=len(lengths), sum=gap_sum,
                                       p50=_percentile(lengths, .5), p90=_percentile(lengths, .9),
                                       reason=gap_unavailable_reason,
                                       spans_without_resolved_timestamp=spans_without_resolved_timestamp,
                                       spans_marked_concurrent=spans_marked_concurrent),
            inference_gap_share_of_execution=gap_share,
            inference_gap_share_unavailable_reason=gap_share_unavailable_reason,
            gateway_tool_time_seconds=gateway_times.get(attempt),
            external_mcp_tool_time_seconds=external_times.get(attempt),
            gateway_duration_incomplete=attempt in gateway_invalid,
            external_duration_incomplete=attempt in external_invalid))
    return dict(unit="counts per attempt and seconds", availability="available", basis="observed",
                per_attempt=output, timing_rows_truncated=timing_truncated,
                gateway_duration_rows_truncated=gateway_truncated, external_duration_rows_truncated=external_truncated,
                note="ACP totals include gateway/external echoes and are not additive with gateway counts. "
                     "Inference gaps are estimated from complete, nonconcurrent ACP spans; "
                     "execution shares use each execution attempt's own duration, excluding user waits. "
                     "A null inference_gap_share_of_execution always carries a sibling "
                     "inference_gap_share_unavailable_reason (and inference_gap_seconds.reason) stating exactly "
                     "why -- truncation, fewer than two observed spans, an unresolved span timestamp, or "
                     "concurrent/overlapping spans (the model issuing more than one tool call before a prior "
                     "one's terminal event landed) making the sequential gap proxy ambiguous for that attempt.")


_MAX_REINVESTIGATION_ROWS = 5000


def _read_history(store, task_id=None):
    with store.connection() as db:
        rows = db.execute(
            "SELECT rowid,task_id,attempt_id,outcome AS kind,created_at,"
            "json_extract(detail_json,'$.path') AS path,"
            "json_extract(detail_json,'$.sha256') AS sha256 FROM observability_events "
            "WHERE category='project_read' " + ("AND task_id=? " if task_id else "") +
            "ORDER BY created_at,rowid LIMIT ?",
            ((task_id,) if task_id else ()) + (_MAX_REINVESTIGATION_ROWS + 1,)).fetchall()
    return [dict(r) for r in rows[:_MAX_REINVESTIGATION_ROWS]], len(rows) > _MAX_REINVESTIGATION_ROWS


def _cross_task_reads(store):
    """Per-call repeat metadata, using all retained tasks as history."""
    rows, truncated = _read_history(store)
    reads = [r for r in rows if r['kind'] == 'text' and r['path'] and r['sha256']]
    if truncated:
        return _unavailable("project_read history truncated")
    if len({r['task_id'] for r in reads}) < 2:
        return _unavailable("no project_read history across tasks")
    seen, counts = {}, {}
    for row in reads:
        key = (row['path'], row['sha256'])
        tasks = seen.setdefault(key, set())
        bucket = counts.setdefault((row['task_id'], row['attempt_id']), [0, 0])
        bucket[0] += 1
        if tasks - {row['task_id']}:
            bucket[1] += 1
        tasks.add(row['task_id'])
    total = sum(c[0] for c in counts.values())
    repeated = sum(c[1] for c in counts.values())
    return dict(availability='available', basis='observed_partial', sample_count=total,
                repeat_count=repeated, ratio=repeated / total,
                per_attempt=[dict(task_id=t, attempt_id=a, sample_count=c[0], repeat_count=c[1])
                             for (t, a), c in counts.items()],
                note="Only retained per-call gateway text reads; native and pre-instrumentation reads are absent.")


def _reinvestigation(store, task_id: str) -> dict:
    rows, truncated = _read_history(store, task_id)
    reads = [r for r in rows if r['kind'] in ('text', 'image', 'binary') and r['path'] and r['sha256']]
    if not reads:
        return _unavailable("no successful per-call project_read rows recorded for this task")
    with store.connection() as db:
        channels = db.execute("SELECT attempt_id,channel_id FROM trusted_channels "
                              "WHERE task_id=? AND role='worker'", (task_id,)).fetchall()
        audits = db.execute("SELECT attempt_id,engine FROM attempt_route_audit WHERE task_id=?", (task_id,)).fetchall()
    by_channel = {}
    for c in channels:
        by_channel.setdefault(c['attempt_id'], set()).add(c['channel_id'])
    engines = {r['engine'] for r in audits}
    partial = bool(engines - {'claude-agent-acp'}) or any(
        not any(a['attempt_id'] == r['attempt_id'] for a in audits) for r in reads)
    spans, spans_truncated = _acp_spans(store, task_id)
    critical = _critical_path(store, task_id)
    lifecycle = {a['attempt_id']: a for a in critical.get('per_attempt', [])}
    waits = critical.get('user_wait_intervals', [])
    within, seen, buckets, duration_bindings = {}, {}, {}, {}
    unknown_channel_keys = set()
    cross_repeat = 0
    for row in reads:
        attempt = row['attempt_id']
        key = (row['path'], row['sha256'])
        within[(attempt, key)] = within.get((attempt, key), 0) + 1
        bucket = buckets.setdefault(attempt, dict(text_reads=0, repeated_text_reads=0,
                                                  repeated_read_seconds=0.0, identity_complete=True,
                                                  identity_incomplete_reasons=set(),
                                                  duration_complete=not spans_truncated))
        identities = by_channel.get(attempt, set())
        channel = next(iter(identities)) if len(identities) == 1 else None
        previous = seen.setdefault(key, set())
        if key in unknown_channel_keys:
            bucket['identity_complete'] = False
            bucket['identity_incomplete_reasons'].add(
                'a prior read of this exact content (same path+sha256) was observed from a channel that '
                'could not be resolved to exactly one trusted worker channel')
        repeated = bool(channel and previous - {channel})
        if channel:
            previous.add(channel)
        else:
            bucket['identity_complete'] = False
            unknown_channel_keys.add(key)
            bucket['identity_incomplete_reasons'].add(
                "this attempt's own channel for one or more reads could not be resolved to exactly one "
                'trusted worker channel (zero or more than one row in trusted_channels for this attempt_id)')
        if repeated:
            cross_repeat += 1
        if row['kind'] != 'text':
            continue
        bucket['text_reads'] += 1
        if not repeated:
            continue
        bucket['repeated_text_reads'] += 1
        stamp = _parse_event_timestamp(row['created_at'])
        matching = [(ref, s) for (a, ref), s in spans.items() if a == attempt and s['kind'] == 'read' and s['started']
                    and stamp and s['first'] is not None and s['terminal'] is not None
                    and s['first'] <= stamp.timestamp() <= s['terminal']]
        if len(matching) != 1:
            bucket['duration_complete'] = False
        else:
            ref, s = matching[0]
            duration_bindings.setdefault((attempt, ref), []).append(attempt)
            bucket['repeated_read_seconds'] += _interval_seconds(
                _subtract_intervals([[s['first'], s['terminal']]], waits))
    # A span containing several read events (a batched wrapper, for example)
    # cannot attribute its whole duration to each read.
    for (attempt, ref), bindings in duration_bindings.items():
        s = spans[(attempt, ref)]
        matched_reads = [r for r in reads if r['attempt_id'] == attempt and r['kind'] == 'text'
                         and _parse_event_timestamp(r['created_at'])
                         and s['first'] <= _parse_event_timestamp(r['created_at']).timestamp() <= s['terminal']]
        if len(matched_reads) != 1:
            buckets[attempt]['duration_complete'] = False
    output = []
    for attempt, bucket in buckets.items():
        life = lifecycle.get(attempt, {})
        duration = life.get('execution_seconds_excluding_user_waits')
        identity_ok = bucket['identity_complete'] and not truncated
        time_ok = identity_ok and bucket['duration_complete'] and duration is not None and duration > 0
        seconds = bucket['repeated_read_seconds'] if time_ok else None
        if seconds is not None and seconds > duration:
            seconds = None
        if identity_ok:
            identity_reason = None
        elif truncated:
            identity_reason = 'project_read history for this task is truncated'
        elif bucket['identity_incomplete_reasons']:
            identity_reason = '; '.join(sorted(bucket['identity_incomplete_reasons']))
        else:
            identity_reason = 'identity for one or more reads in this attempt could not be established'
        output.append(dict(attempt_id=attempt, stage=life.get('stage'), dependent=life.get('dependent'),
                           text_reads=bucket['text_reads'],
                           repeated_text_reads=bucket['repeated_text_reads'] if identity_ok else None,
                           repeat_ratio=bucket['repeated_text_reads'] / bucket['text_reads']
                           if identity_ok and bucket['text_reads'] else None,
                           repeated_read_seconds=seconds,
                           repeat_time_share_of_execution=seconds / duration if seconds is not None else None,
                           duration_basis='estimated' if seconds is not None else 'unavailable_reason',
                           repeated_text_reads_unavailable_reason=identity_reason))
    incomplete_buckets = [a for a, b in buckets.items() if not b['identity_complete']]
    cross_ratio = (cross_repeat / len(reads)
                   if all(b['identity_complete'] for b in buckets.values()) and not truncated else None)
    if cross_ratio is not None:
        cross_reason = None
    elif truncated:
        cross_reason = 'project_read history for this task is truncated'
    elif incomplete_buckets:
        cross_reason = (f'{len(incomplete_buckets)} of {len(buckets)} attempt(s) read content whose cross-channel '
                         'identity could not be fully resolved (own channel unresolved, or a prior reader of the '
                         'exact same content had an unresolved channel -- see repeated_text_reads_unavailable_reason '
                         'per attempt)')
    else:
        cross_reason = 'cross-attempt identity could not be established for this task'
    return dict(unit="repeat calls / successful gateway reads", availability="available",
                basis="observed_partial" if partial else "observed", sample_count=len(reads),
                within_attempt_repeat_ratio=sum(n - 1 for n in within.values()) / len(reads),
                cross_attempt_within_task_repeat_ratio=cross_ratio,
                cross_attempt_within_task_repeat_ratio_unavailable_reason=cross_reason,
                cross_task_repeat_ratio=_unavailable("task scoped; use multi-task aggregates"),
                engines_observed=sorted(engines) or None, partial=partial, truncated=truncated,
                duration_rows_truncated=spans_truncated, per_attempt=output,
                note="Repeat decisions use dependent execution text reads by a different exact channel. "
                     "ACP time is estimated only for a unique containing read span; missing identity/time is "
                     "unavailable. A read of content already touched by any attempt whose channel could not be "
                     "resolved to exactly one trusted worker channel (including non-worker roles such as the "
                     "coordinator, which this metric does not track) marks every later attempt touching that same "
                     "content as identity-incomplete too, never a fabricated repeat/non-repeat guess.")


_MAX_WRITE_REDUNDANCY_ROWS = 5000


def _write_redundancy(store, task_id: str) -> dict:
    """Gateway populations only; generic ACP edit echoes are not native edits."""
    with store.connection() as db:
        rows = db.execute(
            "SELECT c.attempt_id,json_extract(e.payload_json,'$.operation') AS operation,"
            "json_extract(e.payload_json,'$.result.byte_count') AS byte_count,"
            "json_extract(e.payload_json,'$.result.changed_bytes') AS changed_bytes "
            "FROM runtime_events e LEFT JOIN trusted_channels c ON c.task_id=e.task_id AND c.channel_id=e.actor "
            "WHERE e.task_id=? AND e.type='TOOL_COMPLETED' "
            "AND json_extract(e.payload_json,'$.operation') IN ('project_write','project_edit') "
            "ORDER BY e.id LIMIT ?", (task_id, _MAX_WRITE_REDUNDANCY_ROWS + 1)).fetchall()
        routes = [dict(r) for r in db.execute("SELECT attempt_id,engine FROM attempt_route_audit WHERE task_id=?", (task_id,))]
        engines = {r['engine'] for r in routes}
    truncated = len(rows) > _MAX_WRITE_REDUNDANCY_ROWS
    rows = [dict(r) for r in rows[:_MAX_WRITE_REDUNDANCY_ROWS]]
    if not rows:
        return _unavailable("no gateway project_write/project_edit completion rows")
    native = _unavailable("ACP kind=edit does not identify apply_patch or exclude gateway echoes")
    # Configured Claude native file editing is disabled. Other/unknown engines
    # cannot establish an all-write denominator from the currently retained kinds.
    all_writes_known = engines == {'claude-agent-acp'}
    def population(items, all_writes_known):
        usable = [r for r in items if isinstance(r['byte_count'], (int, float))
                  and isinstance(r['changed_bytes'], (int, float)) and r['byte_count'] >= 0 and r['changed_bytes'] >= 0]
        full = len(usable) == len(items) and not truncated
        written = sum(r['byte_count'] for r in usable) if full and items else None
        changed = sum(r['changed_bytes'] for r in usable) if full and items else None
        counts = {op: sum(r['operation'] == op for r in items) for op in ('project_write', 'project_edit')}
        pw = [r for r in usable if r['operation'] == 'project_write']
        pw_all = [r for r in items if r['operation'] == 'project_write']
        pw_complete = len(pw) == len(pw_all) and bool(pw) and not truncated
        pw_bytes = sum(r['byte_count'] for r in pw) if pw_complete else None
        pw_changed = sum(r['changed_bytes'] for r in pw) if pw_complete else None
        gateway_shares = {op: n / len(items) for op, n in counts.items()} if items and not truncated else {}
        return dict(availability='available' if full and items else 'unavailable_reason',
                    sample_count=len(usable), redundancy_ratio=written / changed if changed else None,
                    total_bytes_written=written, total_bytes_changed=changed,
                    project_write_bytes=pw_bytes, project_write_changed_bytes=pw_changed,
                    project_write_redundancy_ratio=pw_bytes / pw_changed if pw_changed else None,
                    gateway_share_of_writes_by_class=gateway_shares,
                    share_of_writes_by_class=gateway_shares if all_writes_known else {},
                    project_write_share_of_all_writes=gateway_shares.get('project_write') if all_writes_known else None,
                    project_write_calls=counts['project_write'] if not truncated else None,
                    all_write_calls=len(items) if all_writes_known and not truncated else None,
                    codex_apply_patch_calls_estimated=native, native_write_share=native)
    result = population(rows, all_writes_known)
    groups = {}
    for row in rows:
        if row['attempt_id']:
            groups.setdefault(row['attempt_id'], []).append(row)
    result.update(unit="sum bytes / sum changed bytes", basis="derived", truncated=truncated,
                  per_attempt=[dict(attempt_id=a, **population(
                      items, [r['engine'] for r in routes if r['attempt_id'] == a] == ['claude-agent-acp']))
                      for a, items in groups.items()],
                  note="project_write redundancy is a separate byte-weighted population. "
                       "Gateway shares never add ACP echoes. All-write share is unavailable for other/unknown engines.")
    return result


_MAX_CRITICAL_PATH_EVENTS = 10000


def _parse_event_timestamp(row):
    from datetime import datetime
    value = row['created_at'] if isinstance(row, dict) else row
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(str(value).replace('Z', '+00:00'))
    except (TypeError, ValueError):
        return None
    return parsed if parsed.utcoffset() is not None else None


def _subtract_intervals(intervals, excluded):
    """Union intervals after removing the same user-wait union everywhere."""
    pieces = _merge_intervals(intervals)
    for lo, hi in _merge_intervals(excluded):
        remaining = []
        for start, end in pieces:
            if hi <= start or lo >= end:
                remaining.append([start, end])
            else:
                if start < lo:
                    remaining.append([start, lo])
                if hi < end:
                    remaining.append([hi, end])
        pieces = remaining
    return pieces


def _unavailable(reason):
    return dict(availability="unavailable_reason", reason=reason)


def _critical_path(store, task_id: str) -> dict:
    """Exact attempt boundaries and phase unions, excluding recorded user waits.

    Assignment-only milestones can bind only through one unambiguous attempt.
    A terminal without a session can bind to the single ready session of its
    explicit attempt (the current producer omits the terminal session field).
    Missing, duplicate or unordered boundaries are incomplete, never zero.
    """
    with store.connection() as db:
        rows = [dict(r) for r in db.execute(
            "SELECT id,type,created_at,json_object("
            "'state',json_extract(payload_json,'$.state'),"
            "'assignment_ref',json_extract(payload_json,'$.assignment_ref'),"
            "'attempt_id',json_extract(payload_json,'$.attempt_id'),"
            "'engine_session_id',json_extract(payload_json,'$.engine_session_id'),"
            "'identity',json_extract(payload_json,'$.identity'),"
            "'reason',json_extract(payload_json,'$.reason'),"
            "'revision',json_extract(payload_json,'$.revision'),"
            "'node',json_extract(payload_json,'$.node')) AS payload_json FROM runtime_events "
            "WHERE task_id=? ORDER BY id LIMIT ?", (task_id, _MAX_CRITICAL_PATH_EVENTS + 1))]
        nodes = [dict(r) for r in db.execute(
            "SELECT revision,node_id,assignment_ref,stage,dependencies_json FROM runtime_nodes WHERE task_id=?",
            (task_id,))]
        audits = [dict(r) for r in db.execute(
            "SELECT attempt_id,engine,connection_id,model,params_fingerprint FROM attempt_route_audit WHERE task_id=?",
            (task_id,))]
        provisions = [dict(r) for r in db.execute(
            "SELECT attempt_id,idempotency_key FROM delegation_provisioning WHERE task_id=?", (task_id,))]
        task = db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
    truncated = len(rows) > _MAX_CRITICAL_PATH_EVENTS
    rows = rows[:_MAX_CRITICAL_PATH_EVENTS]
    if len(rows) < 2:
        return _unavailable("fewer than 2 runtime_events rows recorded for this task yet")
    for row in rows:
        row['p'] = json.loads(row.pop('payload_json'))
        stamp = _parse_event_timestamp(row)
        row['ts'] = stamp.timestamp() if stamp else None
    ordered = all(r['ts'] is not None for r in rows) and all(
        a['ts'] <= b['ts'] for a, b in zip(rows, rows[1:]))
    if not ordered:
        return _unavailable("task event timestamps are missing or unordered")
    task_start, task_end = rows[0]['ts'], rows[-1]['ts']
    completed_task = bool(task and task['state'] == 'COMPLETED')
    classes = {name: [] for name in ('engine_startup', 'coordinator_turns', 'worker_execution',
                                     'failed_attempts', 'review', 'verification', 'synthesis', 'scheduler_waits')}
    incomplete = {name: False for name in classes}
    seen = {name: False for name in classes}
    waits = []
    boundaries_incomplete = False

    def add(name, a, b):
        seen[name] = True
        if a and b and a['id'] < b['id'] and a['ts'] <= b['ts']:
            classes[name].append([a['ts'], b['ts']])
            return True
        incomplete[name] = True
        return False

    by_ref = {}
    by_attempt = {}
    for row in rows:
        p = row['p']
        if p.get('assignment_ref'):
            by_ref.setdefault(p['assignment_ref'], []).append(row)
        if p.get('attempt_id') and row['type'] in ('ENGINE_READY', 'WORKER_COMPLETED', 'WORKER_FAILED'):
            by_attempt.setdefault(p['attempt_id'], []).append(row)
    # Include known dispatched attempts even if the milestones are absent.
    for record in audits + provisions:
        by_attempt.setdefault(record['attempt_id'], [])
    attempts = []
    for attempt_id, related in by_attempt.items():
        ready = [r for r in related if r['type'] == 'ENGINE_READY']
        terminals = [r for r in related if r['type'] in ('WORKER_COMPLETED', 'WORKER_FAILED')]
        refs = {r['p'].get('assignment_ref') for r in related} - {None}
        provision_refs = {p['idempotency_key'] for p in provisions if p['attempt_id'] == attempt_id}
        if not refs:
            refs = provision_refs
        ref = next(iter(refs)) if len(refs) == 1 else None
        created = [r for r in by_ref.get(ref, []) if r['type'] == 'ASSIGNMENT_CREATED']
        node_matches = [n for n in nodes if n['assignment_ref'] == ref]
        if len(created) == 1:
            cp = created[0]['p']
            node_matches = [n for n in nodes if n['revision'] == cp.get('revision')
                            and n['node_id'] == cp.get('node')] or node_matches
        node = node_matches[0] if len(node_matches) == 1 else None
        stage = node['stage'] if node else None
        deps = json.loads(node['dependencies_json']) if node else None
        route_matches = [a for a in audits if a['attempt_id'] == attempt_id]
        route = route_matches[0] if len(route_matches) == 1 else {}
        session = ready[0]['p'].get('engine_session_id') if len(ready) == 1 else None
        valid = bool(ref and session and len(ready) == len(terminals) == 1)
        if valid:
            terminal_session = terminals[0]['p'].get('engine_session_id')
            valid = terminal_session in (None, session) and terminals[0]['p'].get('assignment_ref') == ref
        interval = None
        failed = bool(len(terminals) == 1 and terminals[0]['type'] == 'WORKER_FAILED')
        target = 'failed_attempts' if failed else ('review' if stage == 'review' else 'worker_execution')
        if valid and ready[0]['id'] < terminals[0]['id']:
            interval = [ready[0]['ts'], terminals[0]['ts']]
            if stage == 'execution' or failed:
                add(target, ready[0], terminals[0])
            elif stage == 'review':
                reviews = [r for r in by_ref.get(ref, []) if r['type'] == 'REVIEW_STARTED'
                           and r['id'] < terminals[0]['id']
                           and r['p'].get('attempt_id') in (None, attempt_id)
                           and r['p'].get('engine_session_id') in (None, session)]
                # Current REVIEW_STARTED is assignment-only; more than one
                # ready attempt under that ref cannot be attributed safely.
                ref_ready = [r for r in by_ref.get(ref, []) if r['type'] == 'ENGINE_READY']
                if len(reviews) == 1 and (reviews[0]['p'].get('attempt_id') or len(ref_ready) == 1):
                    add('review', reviews[0], terminals[0])
                else:
                    seen['review'] = True
                    incomplete['review'] = True
                    valid = False
            else:
                valid = False
        else:
            valid = False
        if not valid:
            seen[target] = True
            incomplete[target] = True
        # ASSIGNMENT_CREATED lacks attempt_id. Bind startup only when unique.
        ref_ready = [r for r in by_ref.get(ref, []) if r['type'] == 'ENGINE_READY']
        if len(created) == 1 and len(ready) == 1 and (
                created[0]['p'].get('attempt_id') == attempt_id or len(ref_ready) == 1):
            add('engine_startup', created[0], ready[0])
        else:
            seen['engine_startup'] = True
            incomplete['engine_startup'] = True
        attempts.append(dict(attempt_id=attempt_id, engine_session_id=session, stage=stage,
                             revision=node['revision'] if node else None,
                             terminal_at=terminals[0]['ts'] if valid else None,
                             dependent=bool(deps) if deps is not None else None,
                             engine=route.get('engine'), connection_id=route.get('connection_id'),
                             model=route.get('model'), params_fingerprint=route.get('params_fingerprint'),
                             availability='available' if valid else 'unavailable_reason',
                             completed=bool(valid and not failed), failed=failed, interval=interval if valid else None))
    # Unbound assignment milestones remain incomplete.
    for ref, related in by_ref.items():
        if not any(r['type'] == 'ENGINE_READY' for r in related):
            seen['engine_startup'] = True
            incomplete['engine_startup'] = True
        if any(r['type'] == 'REVIEW_STARTED' for r in related) and not any(
                a['stage'] == 'review' and a['availability'] == 'available'
                and a['attempt_id'] in {r['p'].get('attempt_id') for r in related} for a in attempts):
            incomplete['review'] = True

    # Coordinator turns pair by the actual engine session, not global order.
    turns = {}
    for row in rows:
        if row['type'] in ('COORDINATOR_TURN_STARTED', 'COORDINATOR_TURN_ENDED'):
            turns.setdefault(row['p'].get('engine_session_id'), []).append(row)
    for identity, events in turns.items():
        pending = None
        for row in events:
            if row['type'] == 'COORDINATOR_TURN_STARTED':
                if pending:
                    incomplete['coordinator_turns'] = True
                pending = row
                seen['coordinator_turns'] = True
            else:
                add('coordinator_turns', pending if identity else None, row)
                pending = None
        if pending:
            incomplete['coordinator_turns'] = True

    verification_passes = []
    for index, row in enumerate(rows):
        if row['type'] != 'TASK_STATE_CHANGED':
            continue
        state = row['p'].get('state')
        following = rows[index + 1:]
        next_state = next((r for r in following if r['type'] == 'TASK_STATE_CHANGED'), None)
        if state == 'waiting_for_user':
            if next_state:
                waits.append([row['ts'], next_state['ts']])
            else:
                boundaries_incomplete = True
            continue
        name, end_types = {
            'VERIFYING': ('verification', ('VERIFICATION_PASSED', 'VERIFICATION_FAILED')),
            'RECONCILING': ('synthesis', ('SYNTHESIS_SUBMITTED',)),
        }.get(state, (None, ()))
        if name:
            next_start = next((r for r in following if r['type'] == 'TASK_STATE_CHANGED'
                               and r['p'].get('state') == state), None)
            ends = [r for r in following if r['type'] in end_types
                    and (next_start is None or r['id'] < next_start['id'])]
            ok = add(name, row, ends[0] if len(ends) == 1 else None)
            if name == 'verification' and ok and ends[0]['type'] == 'VERIFICATION_PASSED':
                verification_passes.append((ends[0]['p'].get('revision'), ends[0]['ts']))
    sched = {}
    for row in rows:
        if row['type'] == 'SCHEDULER_WAIT_REASON':
            sched.setdefault(row['p'].get('identity'), []).append(row)
    for identity, events in sched.items():
        for i, row in enumerate(events):
            if row['p'].get('reason') is not None:
                add('scheduler_waits', row, events[i + 1] if identity and i + 1 < len(events) else None)
    waits = _merge_intervals(waits)
    total_seconds = task_end - task_start
    denominator = total_seconds - _interval_seconds(waits)
    all_complete = not truncated and not boundaries_incomplete
    summary = {}
    for name, intervals in classes.items():
        cleaned = _subtract_intervals(intervals, waits)
        # Only explicit, closed scheduler no-wait data establishes a zero.
        available = all_complete and not incomplete[name] and (seen[name] or (
            name == 'scheduler_waits' and bool(sched)))
        seconds = _interval_seconds(cleaned) if available else None
        summary[name] = dict(availability='available' if available else 'unavailable_reason',
                             seconds=seconds, interval_count=len(cleaned),
                             share_of_wall_time_excluding_user_waits=seconds / denominator
                             if seconds is not None and denominator > 0 else None)
    combined = {}
    for name, members in (
            ('orchestration', ('engine_startup', 'coordinator_turns', 'review', 'verification', 'synthesis', 'scheduler_waits')),
            ('review_verification', ('review', 'verification'))):
        available = all(summary[m]['availability'] == 'available' for m in members)
        seconds = _interval_seconds(_subtract_intervals(
            [interval for m in members for interval in classes[m]], waits)) if available else None
        combined[name] = dict(availability='available' if available else 'unavailable_reason',
                              seconds=seconds, share_of_wall_time_excluding_user_waits=seconds / denominator
                              if seconds is not None and denominator > 0 else None)
    reviews = [a for a in attempts if a['stage'] == 'review' and a['completed']
               and a['availability'] == 'available']
    for attempt in attempts:
        interval = attempt.pop('interval')
        duration = _interval_seconds(_subtract_intervals([interval], waits)) if interval and all_complete else None
        attempt['execution_seconds_excluding_user_waits'] = duration
        attempt['execution_intervals_excluding_user_waits'] = (
            _subtract_intervals([interval], waits) if interval and all_complete else None)
        attempt['phase_chain_complete'] = bool(
            attempt['stage'] == 'execution' and attempt['completed'] and duration is not None
            and completed_task and summary['review']['availability'] == 'available'
            and attempt['revision'] is not None
            and any(r['revision'] == attempt['revision'] and r['terminal_at'] >= attempt['terminal_at']
                    for r in reviews)
            and any(revision == attempt['revision'] and stamp >= attempt['terminal_at']
                    for revision, stamp in verification_passes)
            and all_complete)
    return dict(unit="seconds (interval union) and shares (0..1)", availability="available", basis="observed",
                wall_seconds=total_seconds, user_wait_seconds=_interval_seconds(waits),
                wall_seconds_excluding_user_waits=denominator, classes=summary, combined=combined,
                per_attempt=attempts, task_completed=completed_task, truncated=truncated,
                boundaries_complete=all_complete, user_wait_intervals=waits,
                note="Missing/unresolved phases are unavailable; failed-attempt cost is separate. "
                     "Combined shares are unions, with user waits removed from every numerator.")


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
        loop_profile=_loop_profile(store, task_id),
        reinvestigation=_reinvestigation(store, task_id),
        write_redundancy=_write_redundancy(store, task_id),
        critical_path=_critical_path(store, task_id),
        **route_outcome_metrics(store, task_id),
    )
