#!/usr/bin/env python3
"""Read metadata-only orchestration phase timings from a checkpointed project."""
import argparse
from datetime import datetime
import json
from pathlib import Path
import sys
import sqlite3

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from cortex_runtime.contracts import StoreError
from cortex_runtime.storage.inspection import _snapshot

MAX_EVENTS = 10000


def phase(name, start, end, **identity):
    result = dict(name=name, **identity, start_event_id=start['id'] if start else None,
                  end_event_id=end['id'] if end else None,
                  start=start['created_at'] if start else None,
                  end=end['created_at'] if end else None, seconds=None)
    missing = []
    if start is None:
        missing.append('start')
    if end is None:
        missing.append('end')
    result['missing'] = missing
    result['status'] = 'missing' if missing else 'observed'
    if not missing:
        try:
            a, b = (datetime.fromisoformat(r['created_at'].replace('Z', '+00:00')) for r in (start, end))
            if a.tzinfo is None or b.tzinfo is None or b < a or end['id'] < start['id']:
                raise ValueError('unordered')
            result['seconds'] = (b - a).total_seconds()
        except (ValueError, TypeError):
            result['status'] = 'invalid_timestamp_or_order'
    return result


def breakdown(events, stages=None):
    """Use exact identities; never substitute nearby timestamps for missing events."""
    stages = stages or {}
    phases = []
    plans = [r for r in events if r['type'] == 'PLAN_ACCEPTED']
    for plan in plans:
        rev = plan['revision']
        turns = [r for r in events if r['type'] == 'COORDINATOR_TURN_STARTED'
                 and r['state'] == 'PLANNING' and r['id'] < plan['id']]
        # One plan is accepted in a particular turn, not the sum of earlier retries.
        previous_plan = max((r['id'] for r in plans if r['id'] < plan['id']), default=0)
        turns = [r for r in turns if r['id'] > previous_plan]
        phases.append(phase('coordinator_to_plan', turns[-1] if turns else None, plan, revision=rev))
        baseline = [r for r in events if r['type'] == 'EXECUTION_BASELINE' and r['revision'] == rev]
        phases.append(phase('plan_to_baseline', plan, baseline[0] if len(baseline) == 1 else None, revision=rev))

    if not plans:
        phases.extend([phase('coordinator_to_plan', None, None), phase('plan_to_baseline', None, None)])
    assignments = [r for r in events if r['type'] == 'ASSIGNMENT_CREATED']
    for created in assignments:
        ref = created['assignment_ref']
        related = [r for r in events if r['assignment_ref'] == ref]
        attempts = sorted({r['attempt_id'] for r in related if r['type'] in {'ENGINE_READY', 'FIRST_MODEL_OUTPUT'}
                           and r['attempt_id'] is not None}) or [None]
        for attempt in attempts:
            def one(kind):
                rows = [r for r in related if r['type'] == kind
                        and (kind not in {'ENGINE_READY', 'FIRST_MODEL_OUTPUT', 'TOOL_AUTHORIZED', 'TOOL_REJECTED'}
                             or r['attempt_id'] == attempt)
                        and (kind not in {'WORKER_COMPLETED', 'WORKER_RELEASED'}
                             or attempt is None or r['attempt_id'] == attempt)]
                return rows[0] if len(rows) == 1 else None
            ready, output = one('ENGINE_READY'), one('FIRST_MODEL_OUTPUT')
            tools = [r for r in related if r['type'] in {'TOOL_AUTHORIZED', 'TOOL_REJECTED'}
                     and (attempt is None or r['attempt_id'] == attempt)]
            tool = tools[0] if tools else None
            completed, released = one('WORKER_COMPLETED'), one('WORKER_RELEASED')
            identity = dict(assignment_ref=ref, attempt_id=attempt, node=created['node'],
                            stage=stages.get(ref), revision=created['revision'])
            for name, a, b in [('assignment_to_ready', created, ready), ('ready_to_output', ready, output),
                               ('output_to_tool', output, tool), ('tool_to_completed', tool, completed),
                               ('completed_to_released', completed, released)]:
                phases.append(phase(name, a, b, **identity))
        if stages.get(ref) == 'review' or any(r['type'] == 'REVIEW_STARTED' for r in related):
            starts = [r for r in related if r['type'] == 'REVIEW_STARTED']
            finishes = [r for r in related if r['type'] == 'WORKER_COMPLETED']
            if not starts:
                phases.append(phase('review', None, finishes[0] if len(finishes) == 1 else None,
                                    assignment_ref=ref))
            for i, start in enumerate(starts):
                upper = starts[i + 1]['id'] if i + 1 < len(starts) else float('inf')
                end = next((r for r in finishes if start['id'] < r['id'] < upper), None)
                phases.append(phase('review', start, end, assignment_ref=ref))

    for i, start in enumerate(events):
        if start['type'] != 'TASK_STATE_CHANGED' or start['state'] not in {'VERIFYING', 'RECONCILING'}:
            continue
        target = {'VERIFICATION_PASSED', 'VERIFICATION_FAILED'} if start['state'] == 'VERIFYING' else {'SYNTHESIS_SUBMITTED'}
        # Never match a later cycle after another state transition.
        later = []
        for row in events[i + 1:]:
            if row['type'] == 'TASK_STATE_CHANGED':
                break
            later.append(row)
        end = next((r for r in later if r['type'] in target), None)
        phases.append(phase('verification' if start['state'] == 'VERIFYING' else 'synthesis', start, end))
    for name in ('verification', 'synthesis'):
        if not any(p['name'] == name for p in phases):
            phases.append(phase(name, None, None))
    return phases


def read_timeline(project, task):
    with _snapshot(str(Path(project).absolute())) as (_, db, _store):
        if not db.execute('SELECT 1 FROM runtime_tasks WHERE task_id=?', (task,)).fetchone():
            raise StoreError('timeline_task_missing')
        # Project only allowlisted metadata in SQL: no prompts, results or tool bodies.
        rows = db.execute("""
            SELECT e.id,e.type,e.created_at,
                   json_extract(e.payload_json,'$.revision') AS revision,
                   json_extract(e.payload_json,'$.state') AS state,
                   json_extract(e.payload_json,'$.node') AS node,
                   COALESCE(json_extract(e.payload_json,'$.attempt_id'),c.attempt_id) AS attempt_id,
                   COALESCE(json_extract(e.payload_json,'$.assignment_ref'),p.idempotency_key) AS assignment_ref
            FROM runtime_events e
            LEFT JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id
            LEFT JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.assignment_id=c.assignment_id
            WHERE e.task_id=? ORDER BY e.id LIMIT ?
        """, (task, MAX_EVENTS + 1)).fetchall()
        if len(rows) > MAX_EVENTS:
            raise StoreError('timeline_event_overflow')
        events = [dict(r) for r in rows]
        stages = {r[0]: r[1] for r in db.execute(
            'SELECT assignment_ref,stage FROM runtime_nodes WHERE task_id=?', (task,))}
        waits = [dict(r) for r in db.execute("""
            SELECT id,created_at,json_extract(payload_json,'$.reason') AS reason,
                   json_extract(payload_json,'$.node') AS node,
                   json_extract(payload_json,'$.attempt_id') AS attempt_id,
                   json_extract(payload_json,'$.revision') AS revision
            FROM runtime_events WHERE task_id=? AND type='SCHEDULER_WAIT_REASON' ORDER BY id
        """, (task,))]
    return dict(task_id=task, event_count=len(events), complete=True,
                phases=breakdown(events, stages), waits=waits,
                note='Observed wall-clock intervals; overlapping workers are not summed into a critical path.')


MAX_AGGREGATE_TASKS = 500

# Owner-approved Stage 2 decision-gate thresholds (Stage 1 audit section 4),
# fixed before data collection -- never adjusted here based on the data.
_WORKER_EXECUTION_SHARE_MIN = 0.5
_MEDIAN_TOOL_CALLS_MIN = 15
_INFERENCE_GAP_SHARE_MIN = 0.4
_WRITE_REDUNDANCY_MIN = 5.0
_PROJECT_WRITE_SHARE_MIN = 0.3
_REINVESTIGATION_READ_RATIO_MIN = 0.25
_CROSS_TASK_RATIO_MIN = 0.3
_ORCHESTRATION_SHARE_MIN = 0.35
_REVIEW_VERIFICATION_SHARE_MIN = 0.25
_VALIDITY_MIN_TASKS = 10
_VALIDITY_MIN_ATTEMPTS = 30
_VALIDITY_MIN_CHAIN_RATIO = 0.8
_VALIDITY_MAX_UNKNOWN_TOKEN_RATIO = 0.2


def _task_ids(db, limit):
    rows = db.execute('SELECT id FROM tasks ORDER BY id LIMIT ?', (limit + 1,)).fetchall()
    return [row[0] for row in rows]


def _attempts_count(db, task_id):
    return db.execute('SELECT COUNT(*) FROM delegation_provisioning WHERE task_id=?', (task_id,)).fetchone()[0]


def _phase_chain_complete(summary):
    critical = summary.get('critical_path') or {}
    if critical.get('availability') != 'available' or critical.get('truncated') or not critical.get('task_completed'):
        return False
    attempts = [a for a in critical.get('per_attempt', []) if a.get('stage') == 'execution']
    return bool(attempts) and all(a.get('phase_chain_complete') for a in attempts)


def _mean(values):
    return sum(values) / len(values) if values and all(v is not None for v in values) else None


def _median(values):
    if not values or any(v is None for v in values):
        return None
    values = sorted(values)
    mid = len(values) // 2
    return values[mid] if len(values) % 2 else (values[mid - 1] + values[mid]) / 2


def _withheld(reason):
    return dict(availability='unavailable_reason', reason=reason, holds=None)


def _runtime_synthesis_outcome(db, task_id):
    """Coordinator-hidden runtime-composed-synthesis outcome for one task.

    Metadata only (event types and a reason code), never report content.
    'unrecorded' means the task completed through the coordinator without a
    retained SYNTHESIS_RUNTIME_DECLINED event (for example, a build before
    that event existed) -- never guessed as a specific reason.
    """
    reconciled = db.execute(
        "SELECT 1 FROM runtime_events WHERE task_id=? AND type='TASK_STATE_CHANGED' "
        "AND json_extract(payload_json,'$.state')='RECONCILING'", (task_id,)).fetchone() is not None
    if not reconciled:
        return dict(reconciled=False, composed=None, fallback_reason=None)
    composed = db.execute(
        "SELECT 1 FROM runtime_events WHERE task_id=? AND type='SYNTHESIS_RUNTIME_COMPOSED'",
        (task_id,)).fetchone() is not None
    if composed:
        return dict(reconciled=True, composed=True, fallback_reason=None)
    completed = db.execute(
        "SELECT 1 FROM runtime_events WHERE task_id=? AND type='TASK_STATE_CHANGED' "
        "AND json_extract(payload_json,'$.state')='COMPLETED'", (task_id,)).fetchone() is not None
    if not completed:
        return dict(reconciled=True, composed=None, fallback_reason=None)
    declined = db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SYNTHESIS_RUNTIME_DECLINED' "
        "ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
    reason = json.loads(declined[0])['reason'] if declined else 'unrecorded'
    return dict(reconciled=True, composed=False, fallback_reason=reason)


def _cohort_decision(items, cross_task):
    """Evaluate only exact completed chains, inside one route/effort cohort."""
    completed_tasks = {e['task_id'] for e, a in items if e['summary']['critical_path'].get('task_completed')}
    eligible = [(e, a) for e, a in items if (
        a.get('phase_chain_complete') and a.get('completed') and not a.get('failed')
        and e['summary']['critical_path'].get('availability') == 'available'
        and e['summary']['critical_path'].get('task_completed')
        and not e['summary']['critical_path'].get('truncated'))]
    ratio = len(eligible) / len(items) if items else None
    enough = len(completed_tasks) >= _VALIDITY_MIN_TASKS or len(items) >= _VALIDITY_MIN_ATTEMPTS
    valid = bool(enough and ratio is not None and ratio >= _VALIDITY_MIN_CHAIN_RATIO)
    # Runtime-composed-synthesis share: per distinct task in this cohort, not
    # gated by the worker-loop validity threshold above (it is a coordinator
    # hand-off outcome, not a worker-loop sample).
    synthesis_by_task = {}
    for e, _a in items:
        synthesis_by_task.setdefault(e['task_id'], e.get('runtime_synthesis'))
    reconciled_outcomes = [s for s in synthesis_by_task.values() if s and s['reconciled']]
    composed_count = sum(1 for s in reconciled_outcomes if s['composed'])
    fallback_reasons = {}
    pending_count = 0
    for s in reconciled_outcomes:
        if not s['composed']:
            if s['fallback_reason'] is None:
                pending_count += 1
            else:
                fallback_reasons[s['fallback_reason']] = fallback_reasons.get(s['fallback_reason'], 0) + 1
    runtime_synthesis = dict(
        reconciled_tasks=len(reconciled_outcomes), composed_tasks=composed_count,
        pending_tasks=pending_count,
        # Keep the all-reconciled denominator, but withhold its unfinished share.
        share=(composed_count / len(reconciled_outcomes)) if reconciled_outcomes and not pending_count else None,
        fallback_reasons=fallback_reasons)
    calls = [e.get('usage_by_attempt', {}).get(a['attempt_id']) for e, a in items]
    calls_known = bool(calls) and all(c is not None for c in calls)
    call_count = sum(c['calls'] for c in calls) if calls_known else None
    unknown_count = sum(c['unknown'] + c['released'] for c in calls) if calls_known else None
    unknown_ratio = unknown_count / call_count if call_count else None
    token_usable = unknown_ratio is not None and unknown_ratio <= _VALIDITY_MAX_UNKNOWN_TOKEN_RATIO

    worker_shares, orchestration_shares, review_shares = [], [], []
    for task_id in sorted({e['task_id'] for e, a in eligible}):
        task_items = [(e, a) for e, a in eligible if e['task_id'] == task_id]
        critical = task_items[0][0]['summary']['critical_path']
        denominator = critical.get('wall_seconds_excluding_user_waits')
        intervals = [a.get('execution_intervals_excluding_user_waits') for e, a in task_items]
        if denominator and all(i is not None for i in intervals):
            from cortex_runtime.observability.metrics import _merge_intervals, _interval_seconds
            worker_shares.append(_interval_seconds(_merge_intervals(
                [piece for group in intervals for piece in group])) / denominator)
        else:
            worker_shares.append(None)
        combined = critical.get('combined', {})
        orchestration_shares.append(combined.get('orchestration', {}).get('share_of_wall_time_excluding_user_waits'))
        review_shares.append(combined.get('review_verification', {}).get('share_of_wall_time_excluding_user_waits'))
    tools, gaps, repeated_counts, text_counts, repeat_seconds, execution_seconds = [], [], [], [], [], []
    written, changed, project_write_counts, all_write_counts = [], [], [], []
    cross_counts = []
    cross_lookup = {(r['task_id'], r['attempt_id']): r for r in cross_task.get('per_attempt', [])}
    for entry, attempt in eligible:
        summary = entry['summary']
        attempt_id = attempt['attempt_id']
        loop = summary.get('loop_profile', {})
        row = next((r for r in loop.get('per_attempt', []) if r['attempt_id'] == attempt_id), {})
        loop_complete = not any(loop.get(k) for k in (
            'timing_rows_truncated', 'gateway_duration_rows_truncated', 'external_duration_rows_truncated'))
        tools.append(row.get('tool_calls_for_decision') if loop_complete else None)
        gaps.append(row.get('inference_gap_share_of_execution') if loop_complete else None)
        writes = summary.get('write_redundancy', {})
        write = next((r for r in writes.get('per_attempt', []) if r['attempt_id'] == attempt_id), {})
        written.append(write.get('project_write_bytes') if not writes.get('truncated') else None)
        changed.append(write.get('project_write_changed_bytes') if not writes.get('truncated') else None)
        project_write_counts.append(write.get('project_write_calls') if not writes.get('truncated') else None)
        all_write_counts.append(write.get('all_write_calls') if not writes.get('truncated') else None)
        reinvest = summary.get('reinvestigation', {})
        read = next((r for r in reinvest.get('per_attempt', []) if r['attempt_id'] == attempt_id), {})
        execution_seconds.append(attempt.get('execution_seconds_excluding_user_waits'))
        if attempt.get('dependent') is None:
            # Unknown dependency status is never treated as 0 or skipped;
            # it stays an explicit unknown input to the mean.
            repeated_counts.append(None)
            text_counts.append(None)
            repeat_seconds.append(None)
        elif attempt.get('dependent') is True:
            # The prospectively approved gate counts dependent reads only.
            # Independent observations remain in the per-attempt diagnostics.
            full = not reinvest.get('truncated') and not reinvest.get('duration_rows_truncated')
            repeated_counts.append(read.get('repeated_text_reads') if full else None)
            text_counts.append(read.get('text_reads') if full else None)
            repeat_seconds.append(read.get('repeated_read_seconds') if full else None)
        cross_counts.append(cross_lookup.get((entry['task_id'], attempt_id)))
    cross_ratio = None
    if cross_task.get('availability') == 'available' and cross_counts and all(r is not None for r in cross_counts):
        cross_total = sum(r['sample_count'] for r in cross_counts)
        cross_ratio = sum(r['repeat_count'] for r in cross_counts) / cross_total if cross_total else None
    pw_ratio = None
    if written and all(v is not None for v in written + changed) and sum(changed) > 0:
        pw_ratio = sum(written) / sum(changed)
    repeat_ratio = sum(repeated_counts) / sum(text_counts) if (
        repeated_counts and all(v is not None for v in repeated_counts + text_counts) and sum(text_counts) > 0) else None
    repeat_time = sum(repeat_seconds) / sum(execution_seconds) if (
        repeat_seconds and all(v is not None for v in repeat_seconds + execution_seconds)
        and sum(execution_seconds) > 0) else None
    inputs = dict(mean_worker_execution_share=_mean(worker_shares),
                  median_tool_calls_per_attempt=_median(tools),
                  mean_inference_gap_share_of_worker_execution=_mean(gaps),
                  mean_write_redundancy_ratio=pw_ratio,
                  mean_project_write_share_of_writes=(sum(project_write_counts) / sum(all_write_counts) if (
                      project_write_counts and all(v is not None for v in project_write_counts + all_write_counts)
                      and sum(all_write_counts) > 0) else None),
                  mean_cross_attempt_within_task_repeat_ratio=repeat_ratio,
                  mean_repeat_read_share_of_worker_execution=repeat_time,
                  mean_orchestration_share_of_critical_path=_mean(orchestration_shares),
                  mean_review_plus_verification_share=_mean(review_shares),
                  cross_task_repeat_ratio=cross_ratio,
                  reinvestigation_partial=items[0][1].get('engine') != 'claude-agent-acp' if items else True)
    def branch(required, thresholds):
        if not valid:
            return _withheld('validity gate not met for this engine/route/effort cohort')
        if any(inputs[k] is None for k in required):
            return _withheld('approved input unavailable: ' + ', '.join(k for k in required if inputs[k] is None))
        return dict(availability='available', holds=all(inputs[k] >= threshold for k, threshold in zip(required, thresholds)))
    worker = branch(('mean_worker_execution_share', 'median_tool_calls_per_attempt',
                     'mean_inference_gap_share_of_worker_execution'),
                    (_WORKER_EXECUTION_SHARE_MIN, _MEDIAN_TOOL_CALLS_MIN, _INFERENCE_GAP_SHARE_MIN))
    edit_known = inputs['mean_write_redundancy_ratio'] is not None and inputs['mean_project_write_share_of_writes'] is not None
    edit_holds = (pw_ratio >= _WRITE_REDUNDANCY_MIN and inputs['mean_project_write_share_of_writes'] >= _PROJECT_WRITE_SHARE_MIN
                  ) if edit_known else None
    worker.update(code_apply_recommended=bool(worker['holds'] and edit_holds)
                  if worker['holds'] is not None and edit_known else None,
                  promote_project_edit=bool(worker['holds'] and not edit_holds)
                  if worker['holds'] is not None and edit_known else None,
                  edit_decision_availability='available' if worker['holds'] is not None and edit_known else 'unavailable_reason')
    reinvest = branch(('mean_cross_attempt_within_task_repeat_ratio', 'mean_repeat_read_share_of_worker_execution'),
                      (_REINVESTIGATION_READ_RATIO_MIN, .15))
    reinvest.update(partial=inputs['reinvestigation_partial'],
                    cross_task_reuse_considered=cross_ratio >= _CROSS_TASK_RATIO_MIN if valid and cross_ratio is not None else None)
    orchestration = branch(('mean_orchestration_share_of_critical_path', 'mean_review_plus_verification_share'), (0, 0))
    if orchestration['holds'] is not None:
        orchestration['holds'] = (inputs['mean_orchestration_share_of_critical_path'] >= _ORCHESTRATION_SHARE_MIN
                                  or inputs['mean_review_plus_verification_share'] >= _REVIEW_VERIFICATION_SHARE_MIN)
    return dict(validity=dict(total_tasks=len({e['task_id'] for e, a in items}), completed_tasks=len(completed_tasks),
                              total_attempts=len(items), eligible_attempts=len(eligible), enough_samples=enough,
                              phase_chain_complete_ratio=ratio, chain_valid=ratio is not None and ratio >= _VALIDITY_MIN_CHAIN_RATIO,
                              token_calls=call_count, unknown_or_released_calls=unknown_count,
                              unknown_token_ratio=unknown_ratio, token_criteria_usable=token_usable, valid=valid),
                inputs=inputs, branches=dict(worker_loops=worker, reinvestigation=reinvest, orchestration_chain=orchestration),
                runtime_synthesis=runtime_synthesis)


def _aggregate_decision(per_task, cross_task=None):
    """No pooled verdict: cohort identity includes the full parameters fingerprint.

    The fingerprint covers effort as well as other dispatch parameters; missing
    route identity cannot stand in for a comparable route.
    """
    cross_task = cross_task or dict(availability='unavailable_reason', reason='no cross-task read history')
    groups, unattributed = {}, 0
    for entry in per_task:
        critical = entry['summary'].get('critical_path', {})
        for attempt in critical.get('per_attempt', []):
            if attempt.get('stage') == 'review':
                continue
            key = tuple(attempt.get(k) for k in ('engine', 'connection_id', 'model', 'params_fingerprint'))
            if attempt.get('stage') != 'execution' or not all(key):
                unattributed += 1
                continue
            groups.setdefault(key, []).append((entry, attempt))
    cohorts = []
    for key, items in sorted(groups.items()):
        cohorts.append(dict(engine=key[0], connection_id=key[1], model=key[2], params_fingerprint=key[3],
                            **_cohort_decision(items, cross_task)))
    # Keep the old top-level shape useful for single-cohort consumers, without
    # turning several incomparable populations into a single sample/verdict.
    if len(cohorts) == 1 and not unattributed:
        top = {k: cohorts[0][k] for k in ('validity', 'inputs', 'branches', 'runtime_synthesis')}
        top['validity'] = dict(top['validity'], total_tasks=len(per_task))
    else:
        top = dict(validity=dict(total_tasks=len(per_task), total_attempts=sum(e['attempts'] for e in per_task),
                                 enough_samples=False, valid=False, token_criteria_usable=False),
                   inputs=dict(cross_task_repeat_ratio={k: v for k, v in cross_task.items() if k != 'per_attempt'}),
                   branches=dict(worker_loops=None, reinvestigation=None, orchestration_chain=None,
                                 note='withheld: use per-cohort verdicts; no pooled engine/route/effort decision'),
                   runtime_synthesis=dict(note='withheld: use per-cohort runtime_synthesis; several cohorts present'))
    top['branches'].setdefault('note', 'Unavailable inputs withhold the affected branch, never false.')
    return dict(unit='metadata counts, ratios and durations only', **top, cohorts=cohorts,
                unattributed_attempts=unattributed,
                cross_task_repeat_ratio={k: v for k, v in cross_task.items() if k != 'per_attempt'},
                per_task=[dict(task_id=e['task_id'], attempts=e['attempts'],
                               phase_chain_complete=_phase_chain_complete(e['summary']), tokens_basis=e['tokens_basis'],
                               runtime_synthesis=e.get('runtime_synthesis'))
                          for e in per_task],
                note="Validity and decisions are per engine/connection/model/parameters fingerprint. "
                     "Only exact completed non-truncated execution chains contribute branch inputs.")


def _token_coverage(db, task_ids):
    """Complete metadata-only coverage by engine/role, not a pooled cost estimate."""
    buckets = {}
    for task_id in task_ids:
        for row in db.execute(
                "SELECT u.basis,u.tokens_json,u.failed,u.stop_reason,"
                "(SELECT engine FROM attempt_route_audit a WHERE a.attempt_id=u.attempt_id "
                "ORDER BY recorded_at DESC LIMIT 1) AS engine,"
                "CASE WHEN substr(u.attempt_id,1,3)='co_' THEN 'coordinator' ELSE "
                "COALESCE((SELECT n.stage FROM delegation_provisioning p JOIN runtime_nodes n "
                "ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
                "WHERE p.task_id=u.task_id AND p.attempt_id=u.attempt_id LIMIT 1),'unattributed') END AS role "
                "FROM usage_records u WHERE u.task_id=?", (task_id,)):
            role = 'worker' if row['role'] == 'execution' else row['role']
            key = (row['engine'] or 'unattributed', role)
            bucket = buckets.setdefault(key, dict(engine=key[0], role=role, calls=0,
                reported_calls=0, unknown_calls=0, released_after_result_calls=0, tokens_reported={}))
            bucket['calls'] += 1
            if row['basis'] == 'unknown':
                field = ('released_after_result_calls' if not row['failed']
                         and row['stop_reason'] == 'released_after_result' else 'unknown_calls')
                bucket[field] += 1
            else:
                bucket['reported_calls'] += 1
                for field, value in json.loads(row['tokens_json']).items():
                    bucket['tokens_reported'][field] = bucket['tokens_reported'].get(field, 0) + value
    for bucket in buckets.values():
        bucket['unknown_token_ratio'] = (
            bucket['unknown_calls'] + bucket['released_after_result_calls']) / bucket['calls']
    return [buckets[key] for key in sorted(buckets)]


def read_aggregates(project, task_ids=None, max_tasks=MAX_AGGREGATE_TASKS):
    """Read-only, aggregates-only multi-task mode: the same checkpointed-
    store guard as :func:`read_timeline` (``_snapshot``), then one
    ``observability.metrics.diagnostics_summary`` read per task -- the
    exact function ``read_diagnostics`` exposes live, so this script's own
    numbers can never drift from what a worker/coordinator already sees.
    Never reads a report body, a project path's content or a raw log."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
    from cortex_runtime.observability import metrics as observability_metrics

    with _snapshot(str(Path(project).absolute())) as (_root, db, store):
        ids = list(dict.fromkeys(task_ids)) if task_ids else _task_ids(db, max_tasks)
        if len(ids) > max_tasks:
            raise StoreError('timeline_aggregate_overflow')
        per_task = []
        for task_id in ids:
            if not db.execute('SELECT 1 FROM tasks WHERE id=?', (task_id,)).fetchone():
                raise StoreError('timeline_task_missing')
            summary = observability_metrics.diagnostics_summary(store, task_id)
            per_task.append(dict(
                task_id=task_id, attempts=_attempts_count(db, task_id),
                phase_chain_complete=_phase_chain_complete(summary),
                tokens_basis=(summary.get('tokens') or {}).get('availability'),
                summary=summary,
                usage_by_attempt={},
                runtime_synthesis=_runtime_synthesis_outcome(db, task_id),
            ))
            usage = per_task[-1]['usage_by_attempt']
            for row in db.execute(
                    "SELECT attempt_id,COUNT(*) AS calls,"
                    "SUM(CASE WHEN basis='unknown' AND NOT (failed=0 AND COALESCE(stop_reason,'')='released_after_result') "
                    "THEN 1 ELSE 0 END) AS unknown,"
                    "SUM(CASE WHEN basis='unknown' AND failed=0 AND stop_reason='released_after_result' "
                    "THEN 1 ELSE 0 END) AS released FROM usage_records WHERE task_id=? GROUP BY attempt_id",
                    (task_id,)):
                usage[row['attempt_id']] = dict(calls=row['calls'], unknown=row['unknown'], released=row['released'])
        cross_task = observability_metrics._cross_task_reads(store)
        token_coverage = _token_coverage(db, ids)
    result = _aggregate_decision(per_task, cross_task)
    result['token_coverage'] = token_coverage
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True)
    parser.add_argument('--task', action='append', default=[],
                        help='Repeatable. Exactly one required unless --aggregate is set (then optional: '
                             'omit to aggregate every task in the store, up to the aggregate cap).')
    parser.add_argument('--aggregate', action='store_true',
                        help='Read-only aggregates-only multi-task mode: counts/ratios/durations/availability '
                             'only, plus the owner-approved decision-gate verdict. Never a path, report body '
                             'or log.')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    allowed = {'unsafe_storage', 'legacy_storage_migration_required', 'unsupported_storage',
               'inspection_uncheckpointed_wal', 'timeline_task_missing', 'timeline_event_overflow',
               'timeline_aggregate_overflow'}
    if args.aggregate:
        try:
            result = read_aggregates(args.project, args.task or None)
        except (StoreError, OSError, ValueError, sqlite3.Error) as error:
            code = error.args[0] if isinstance(error, StoreError) and error.args else None
            if isinstance(error, BlockingIOError):
                code = 'inspection_busy'
            print(json.dumps(dict(available=False, error=code if code in allowed or code == 'inspection_busy'
                                  else 'timeline_read_refused')))
            return 1
        if args.json:
            print(json.dumps(result, sort_keys=True))
        else:
            print('Aggregate over ' + str(result['validity']['total_tasks']) + ' task(s); valid='
                  + str(result['validity']['valid']))
            print(json.dumps(result['branches'], sort_keys=True))
        return 0
    if len(args.task) != 1:
        parser.error('--task is required exactly once unless --aggregate is set')
    try:
        result = read_timeline(args.project, args.task[0])
    except (StoreError, OSError, ValueError, sqlite3.Error) as error:
        code = error.args[0] if isinstance(error, StoreError) and error.args else None
        if isinstance(error, BlockingIOError):
            code = 'inspection_busy'
        print(json.dumps(dict(available=False, error=code if code in allowed or code == 'inspection_busy'
                              else 'timeline_read_refused')))
        return 1
    if args.json:
        print(json.dumps(result, sort_keys=True))
    else:
        print('Task ' + result['task_id'] + '\n' + result['note'])
        for row in result['phases']:
            label = row['name'] + ' ' + (row.get('assignment_ref') or '')
            duration = f"{row['seconds']:.6f}s" if row['seconds'] is not None else row['status']
            print(f"{label}: {duration}" + (f" ({','.join(row['missing'])})" if row['missing'] else ''))
        for row in result['waits']:
            print(f"wait event {row['id']}: {row['node'] or row['attempt_id']} {row['reason']}")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
