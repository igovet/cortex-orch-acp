#!/usr/bin/env python3
"""Assert one bound qualification task's outcome from its complete metadata.

Exit 0 only when every expectation holds on one complete inspection snapshot;
exit 1 lists each failed expectation; exit 2 is an inspection refusal. Reads
through the bounded task inspector only: never initializes, migrates or
writes, and never reads report or output bodies.
"""
import argparse
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from cortex_runtime.contracts import StoreError
from cortex_runtime.storage.inspection import read_binding_page

_ROUTE = re.compile(r'(worker-[a-z0-9-]+)=([a-z0-9-]+)/([^:\s]+):([a-z]+)')


def _rows(args):
    page = read_binding_page(args.project, args.session, args.task, args.owner_epoch, limit=200)
    rows, snapshot = list(page['rows']), page['snapshot_sha256']
    while page['next_cursor']:
        page = read_binding_page(args.project, args.session, args.task, args.owner_epoch,
                                 cursor=page['next_cursor'], limit=200)
        rows.extend(page['rows'])
    if not page['complete'] or len(rows) != page['total'] or page['snapshot_sha256'] != snapshot:
        raise StoreError('inspection_snapshot_changed')
    return page['binding'], snapshot, rows


def evaluate(rows, args):
    failures = []
    section = lambda name: [r for r in rows if r['section'] == name]
    events = section('runtime_events')
    [runtime] = section('runtime') or [None]
    if runtime is None or runtime['state'] != args.expect_state:
        failures.append(dict(expectation='state', expected=args.expect_state,
                             observed=runtime and runtime['state']))
    if args.expect_state == 'COMPLETED' and runtime is not None:
        nodes = [n for n in section('nodes') if n['revision'] == runtime['revision']]
        if not nodes or any(n['status'] != 'completed' for n in nodes):
            failures.append(dict(expectation='nodes_completed', observed=sorted({n['status'] for n in nodes})))
        verdicts = {e.get('node'): e.get('verdict') for e in events if e['type'] == 'WORKER_COMPLETED'}
        reviews = [n['node_id'] for n in nodes if n['stage'] == 'review']
        if reviews and verdicts.get(reviews[-1]) != 'APPROVE':
            failures.append(dict(expectation='review_approved', observed=verdicts.get(reviews[-1])))
        verification = [e for e in events if e['type'] in ('VERIFICATION_PASSED', 'VERIFICATION_FAILED')]
        if (not verification or verification[-1]['type'] != 'VERIFICATION_PASSED'
                or not verification[-1].get('checks')
                or any(c['exit_code'] != 0 or c['complete'] is not True for c in verification[-1]['checks'])):
            failures.append(dict(expectation='verification_passed',
                                 observed=verification[-1]['type'] if verification else None))
        if not section('synthesis'):
            failures.append(dict(expectation='final_text_recorded', observed=None))
    confirmed = {e['attempt_id']: e for e in section('task_events') if e.get('kind') == 'worker_configuration_confirmed'}
    for role, engine, model, effort in args.expect_route:
        attempts = [a['attempt_id'] for a in section('attempts') if a['role_id'] == role]
        observed = [(confirmed[a]['engine'], confirmed[a]['model'], (confirmed[a].get('parameters') or {}).get('thought_level'))
                    if a in confirmed else None for a in attempts]
        if not attempts or any(o != (engine, model, effort) for o in observed):
            failures.append(dict(expectation='route', role=role, expected=f'{engine}/{model}:{effort}',
                                 observed=[o and f'{o[0]}/{o[1]}:{o[2]}' for o in observed]))
    if args.expect_closed_failure:
        checks = [c for e in events if e['type'] == 'WORKER_COMPLETED' for c in e.get('checks') or ()]
        passed = {c['tool_call_ref'] for c in checks if c['outcome'] == 'passed' and c.get('tool_call_ref')}
        if not any(c['outcome'] == 'failed' and c.get('closed_by') in passed for c in checks):
            failures.append(dict(expectation='failed_check_closed_by_exact_rerun', observed=len(checks)))
    if args.expect_retained_output_bytes is not None:
        # Beyond the inline bound the result is cut inline; the retained body
        # must still be complete (not truncated) and bound to its receipt.
        sizes = [r['retained_output']['byte_count'] for r in section('action_receipts')
                 if r.get('retained_output') and r['retained_output'].get('bound')
                 and not r['retained_output'].get('truncated') and r.get('exit_code') == 0]
        if not any(size >= args.expect_retained_output_bytes for size in sizes):
            failures.append(dict(expectation='complete_retained_output', expected=args.expect_retained_output_bytes,
                                 observed=max(sizes, default=None)))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--project', required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--task', required=True)
    parser.add_argument('--owner-epoch', type=int, required=True)
    parser.add_argument('--expect-state', default='COMPLETED')
    parser.add_argument('--expect-route', action='append', default=[], metavar='ROLE=ENGINE/MODEL:EFFORT',
                        help='every attempt of ROLE has this native worker_configuration_confirmed route')
    parser.add_argument('--expect-closed-failure', action='store_true',
                        help='a failed worker check is closed by a passed exact rerun')
    parser.add_argument('--expect-retained-output-bytes', type=int, metavar='BYTES',
                        help='a zero-exit action retained a bound, complete output of at least BYTES bytes')
    args = parser.parse_args()
    routes = []
    for value in args.expect_route:
        match = _ROUTE.fullmatch(value)
        if match is None:
            parser.error(f'--expect-route must be ROLE=ENGINE/MODEL:EFFORT, got {value!r}')
        routes.append(match.groups())
    args.expect_route = routes
    try:
        binding, snapshot, rows = _rows(args)
    except Exception as error:
        code = error.args[0] if isinstance(error, StoreError) and error.args else None
        if isinstance(error, BlockingIOError):
            code = 'inspection_busy'
        print(json.dumps(dict(passed=False, error=code if isinstance(code, str) and code.startswith(
            ('inspection_', 'unsafe_storage', 'legacy_storage', 'unsupported_storage')) else 'inspection_refused')))
        return 2
    failures = evaluate(rows, args)
    print(json.dumps(dict(passed=not failures, failures=failures, binding=binding,
                          snapshot_sha256=snapshot, total=len(rows)), sort_keys=True))
    return 0 if not failures else 1


if __name__ == '__main__':
    raise SystemExit(main())
