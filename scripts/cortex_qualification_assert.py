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


def _nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def _route_observation(attempt, rows, binding):
    """Join identities inside the inspector's already task-bound snapshot."""
    section = lambda name: [r for r in rows if r['section'] == name]
    ref, aid, session = (attempt.get(k) for k in ('idempotency_key', 'attempt_id', 'engine_session_ref'))
    if (not all(_nonempty(v) for v in (ref, aid, session))
            or attempt.get('owner_epoch') != binding['owner_epoch']):
        return None
    if any(len([a for a in section('attempts') if a.get(key) == value]) != 1
           for key, value in (('idempotency_key', ref), ('attempt_id', aid), ('engine_session_ref', session))):
        return None
    nodes = [n for n in section('nodes') if n.get('assignment_ref') == ref]
    if len(nodes) != 1 or nodes[0].get('role') != attempt['role_id']:
        return None
    confirmations = [e for e in section('task_events') if e.get('kind') == 'worker_configuration_confirmed']
    matches = [e for e in confirmations if e.get('attempt_id') == aid
               or e.get('engine_session_ref') == session or e.get('assignment_ref') == ref]
    if len(matches) != 1:
        return None
    [confirmed] = matches
    evidence = confirmed.get('confirmation_evidence_ref')
    if (confirmed.get('attempt_id') != aid or confirmed.get('engine_session_ref') != session
            or confirmed.get('assignment_ref') != ref or not _nonempty(evidence)
            or confirmed.get('superseded') is True
            or type(confirmed.get('id')) is not int or confirmed['id'] < 1
            or sum(e.get('id') == confirmed['id'] for e in section('task_events')) != 1
            or sum(e.get('confirmation_evidence_ref') == evidence for e in confirmations) != 1):
        return None
    # Compare only identities the projection actually exposes. Coordinator
    # configuration parameters are NOT this worker's native parameters.
    config = attempt.get('config_revision')
    if config is not None and confirmed.get('config_revision_id') != config:
        return None
    configurations = section('configuration')
    native_config = confirmed.get('config_revision_id')
    if native_config is not None and configurations:
        if len([c for c in configurations if c.get('id') == native_config]) != 1:
            return None
    dispatch = [d for d in section('dispatch') if d.get('attempt_id') == aid]
    if dispatch:
        if len(dispatch) != 1 or dispatch[0].get('decision') not in ('authorized', 'fallback'):
            return None
        for key, native_key in (('role', None), ('engine', 'engine'), ('model', 'model'),
                                ('connection_id', 'connection_id'), ('config_revision_id', 'config_revision_id')):
            value = dispatch[0].get(key)
            if value is not None and value != (attempt['role_id'] if native_key is None else confirmed.get(native_key)):
                return None
    parameters = confirmed.get('parameters') or {}
    if parameters.get('thought_level') and parameters.get('effort') and parameters['thought_level'] != parameters['effort']:
        return None
    return (confirmed.get('engine'), confirmed.get('model'), parameters.get('thought_level') or parameters.get('effort'))


def _retained_sizes(receipts):
    """Complete execution/output METADATA only; never verify body bytes."""
    sizes = []
    for receipt in receipts:
        retained = receipt.get('retained_output') or {}
        ref, identity, output = (receipt.get(k) for k in ('tool_call_ref', 'identity_sha256', 'large_output_id'))
        if (not isinstance(ref, str) or not re.fullmatch(r'gateway:[0-9a-f]{32}', ref)
                or not isinstance(identity, str) or not re.fullmatch(r'[0-9a-f]{64}', identity)
                or not isinstance(output, str) or not re.fullmatch(r'lo_[0-9a-f]{12}', output)):
            continue
        if any(sum(r.get(key) == value for r in receipts) != 1 for key, value in
               (('tool_call_ref', ref), ('identity_sha256', identity), ('large_output_id', output))):
            continue
        size = retained.get('byte_count')
        if (receipt.get('operation') in ('command_run', 'outside_command')
                and receipt.get('complete') is True and receipt.get('run_complete') is True
                and type(receipt.get('exit_code')) is int and receipt['exit_code'] == 0
                and retained.get('bound') is True and retained.get('truncated') is False
                and type(retained.get('exit_code')) is int and retained['exit_code'] == 0
                and all(receipt.get(k) is None and retained.get(k) is None for k in ('failure', 'error', 'signal'))
                and type(size) is int and size >= 0):
            sizes.append(size)
    return sizes


def evaluate(rows, args, *, binding=None):
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
        reviews = [n['node_id'] for n in nodes if n['stage'] == 'review']
        if reviews:
            review = next(n for n in nodes if n['node_id'] == reviews[-1])
            attempts = [a for a in section('attempts') if a.get('idempotency_key') == review.get('assignment_ref')]
            results = [e for e in events if e['type'] == 'WORKER_COMPLETED' and e.get('node') == reviews[-1]
                       and len(attempts) == 1 and e.get('attempt_id') == attempts[0].get('attempt_id')]
            if (len(results) != 1 or results[0].get('verdict') != 'APPROVE' or binding is None
                    or attempts[0].get('owner_epoch') != binding['owner_epoch']):
                failures.append(dict(expectation='review_approved', observed=results[0].get('verdict') if len(results) == 1 else None))
        else:
            waivers = [e for e in events if e['type'] == 'REVIEW_NOT_REQUIRED'
                       and e.get('revision') == runtime['revision']]
            valid = len(waivers) == 1
            if valid:
                waiver = waivers[0]
                digest, head = waiver.get('revision_digest'), waiver.get('head')
                valid = bool(digest) and bool(head)
                if waiver.get('reason') == 'no_project_changes' and waiver.get('reference') == 'execution_baseline':
                    baselines = [e for e in events if e['type'] == 'EXECUTION_BASELINE'
                                 and e.get('revision') == waiver.get('reference_revision')
                                 and e.get('id', 0) < waiver.get('id', 0)]
                    valid = valid and len(baselines) == 1 and baselines[0].get('kind') == 'observed'
                    valid = valid and baselines[0].get('revision_digest') == digest and baselines[0].get('head') == head
                elif waiver.get('reason') == 'no_changes_since_approved_review' and waiver.get('reference') == 'approved_review':
                    valid = valid and any(e['type'] == 'WORKER_COMPLETED' and e.get('verdict') == 'APPROVE'
                                          and e.get('project_digest') == digest for e in events)
                else:
                    valid = False
            if not valid or getattr(args, 'require_review', False):
                failures.append(dict(expectation='review_or_valid_waiver', observed='waiver' if valid else None))
        verification = [e for e in events if e['type'] in ('VERIFICATION_PASSED', 'VERIFICATION_FAILED')]
        if (not verification or verification[-1]['type'] != 'VERIFICATION_PASSED'
                or not verification[-1].get('checks')
                or any(c['exit_code'] != 0 or c['complete'] is not True for c in verification[-1]['checks'])):
            failures.append(dict(expectation='verification_passed',
                                 observed=verification[-1]['type'] if verification else None))
        if not section('synthesis'):
            failures.append(dict(expectation='final_text_recorded', observed=None))
    for role, engine, model, effort in args.expect_route:
        attempts = [a for a in section('attempts') if a['role_id'] == role]
        bound = (binding is not None and binding.get('task_id') == args.task
                 and binding.get('owner_epoch') == args.owner_epoch)
        observed = [_route_observation(a, rows, binding) if bound else None for a in attempts]
        if not attempts or any(o != (engine, model, effort) for o in observed):
            failures.append(dict(expectation='route', role=role, expected=f'{engine}/{model}:{effort}',
                                 observed=[o and f'{o[0]}/{o[1]}:{o[2]}' for o in observed]))
    if args.expect_closed_failure:
        checks = [c for e in events if e['type'] == 'WORKER_COMPLETED' for c in e.get('checks') or ()]
        passed = {c['tool_call_ref'] for c in checks if c['outcome'] == 'passed' and c.get('tool_call_ref')}
        if not any(c['outcome'] == 'failed' and c.get('closed_by') in passed for c in checks):
            failures.append(dict(expectation='failed_check_closed_by_exact_rerun', observed=len(checks)))
    if args.expect_retained_output_bytes is not None:
        sizes = _retained_sizes(section('action_receipts'))
        if not any(size >= args.expect_retained_output_bytes for size in sizes):
            failures.append(dict(expectation='complete_retained_output_metadata', expected=args.expect_retained_output_bytes,
                                 observed=max(sizes, default=None)))
    manifest = getattr(args, "read_api_expectations", None)
    if manifest is not None:
        from cortex_runtime.read_api_evidence import evaluate_manifest
        failures.extend(dict(expectation=code) for code in evaluate_manifest(rows, binding, manifest))
    for ref in getattr(args, "expect_command_ref", []):
        receipts = [r for r in section("action_receipts") if r.get("tool_call_ref") == ref]
        checks = [(e, c) for e in events if e["type"] == "WORKER_COMPLETED" for c in e.get("checks") or []
                  if c.get("tool_call_ref") == ref and c.get("outcome") == "passed"
                  and len(receipts) == 1 and e.get("attempt_id") == receipts[0].get("channel_attempt_id")]
        if (len(receipts) != 1 or not _retained_sizes(receipts) or not checks or binding is None
                or receipts[0].get("channel_owner_epoch") != binding["owner_epoch"]):
            failures.append(dict(expectation="current_task_command_receipt"))
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
    parser.add_argument("--require-review", action="store_true", help="require an actual runtime review node and APPROVE, not a waiver")
    parser.add_argument('--expect-closed-failure', action='store_true',
                        help='a failed worker check is closed by a passed exact rerun')
    parser.add_argument('--expect-retained-output-bytes', type=int, metavar='BYTES',
                        help='complete zero-exit command/output metadata records at least BYTES bytes (body not verified)')
    parser.add_argument("--read-api-expectations", type=Path,
                        help="independently established public expected fingerprints, not observed output")
    parser.add_argument("--expect-command-ref", action="append", default=[],
                        help="exact current-task runtime command receipt also attached to a passed worker check")
    args = parser.parse_args()
    if args.read_api_expectations is not None:
        try:
            if args.read_api_expectations.stat().st_size > 1048576:
                raise ValueError("expectation_limit")
            args.read_api_expectations = json.loads(args.read_api_expectations.read_text())
        except (OSError, ValueError):
            print(json.dumps(dict(passed=False, error="invalid_read_api_expectations")))
            return 2
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
    failures = evaluate(rows, args, binding=binding)
    print(json.dumps(dict(passed=not failures, failures=failures, binding=binding,
                          snapshot_sha256=snapshot, total=len(rows)), sort_keys=True))
    return 0 if not failures else 1


if __name__ == '__main__':
    raise SystemExit(main())
