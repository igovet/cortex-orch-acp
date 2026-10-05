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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True)
    parser.add_argument('--task', required=True)
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    try:
        result = read_timeline(args.project, args.task)
    except (StoreError, OSError, ValueError, sqlite3.Error) as error:
        allowed = {'unsafe_storage', 'legacy_storage_migration_required', 'unsupported_storage',
                   'inspection_uncheckpointed_wal', 'timeline_task_missing', 'timeline_event_overflow'}
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
