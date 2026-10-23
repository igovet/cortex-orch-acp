"""The qualification assertion exits nonzero unless the bound task's complete
metadata shows every expected outcome. Fresh controller-driven fixtures only."""
from argparse import Namespace
from copy import deepcopy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.delegation import events

from test_steering_after_verification import _drive, _misused_then_answered, _try

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'cortex_qualification_assert.py'
_spec = importlib.util.spec_from_file_location('qualification_assert', SCRIPT)
qualification = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(qualification)


def _bind(store, tmp_path, task):
    sid = 'acps_' + 'b' * 12
    sessions.create_session(store, sid, str(tmp_path), {'name': 'fixture'}, {}, [])
    sessions.bind_task(store, sid, task.task_id)
    return [sys.executable, '-B', str(SCRIPT), '--project', str(tmp_path), '--session', sid,
            '--task', task.task_id, '--owner-epoch', str(task.owner_epoch.epoch)]


def _run(argv):
    result = subprocess.run(argv, capture_output=True, text=True)
    return result.returncode, json.loads(result.stdout) if result.stdout.strip() else result.stderr


def _files(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (root / '.cortex').rglob('*') if p.is_file()}


def test_completed_task_passes_and_each_unmet_expectation_exits_nonzero(tmp_path, monkeypatch):
    store, task, context, seen = _drive(tmp_path, monkeypatch, _misused_then_answered)
    assert context['state'] == 'COMPLETED'
    argv = _bind(store, tmp_path, task)
    before = _files(tmp_path)
    code, result = _run(argv)
    assert (code, result['passed'], result['failures']) == (0, True, [])
    assert _files(tmp_path) == before
    assert result['binding']['task_id'] == task.task_id and len(result['snapshot_sha256']) == 64
    # Dispatch alone is not a route: without a native confirmation it fails.
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:high'])
    assert code == 1 and [f['expectation'] for f in result['failures']] == ['route']
    with store.connection() as db:
        attempts = [dict(r) for r in db.execute(
            "SELECT * FROM delegation_provisioning WHERE task_id=? AND role_id='worker-backend-dev'",
            (task.task_id,))]
    for attempt in attempts:
        events.append_event(store, task.task_id, 'progress', f"confirmed-{attempt['attempt_id']}", dict(
            kind='worker_configuration_confirmed', attempt_id=attempt['attempt_id'], engine='claude-agent-acp', model='haiku',
            parameters=dict(thought_level='high'), confirmation_evidence_ref=f"fixture-{attempt['attempt_id']}",
            engine_session_ref=attempt['engine_session_ref'], config_revision_id=attempt['config_revision'],
            connection_id=None), 'fixture', assignment_ref=attempt['idempotency_key'])
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:high'])
    assert (code, result['failures']) == (0, [])
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:low'])
    assert code == 1 and result['failures'][0]['observed'] == ['claude-agent-acp/haiku:high']
    # This fixture ran no failing check and retained no large output.
    code, result = _run(argv + ['--expect-closed-failure', '--expect-retained-output-bytes', '65537'])
    assert code == 1 and [f['expectation'] for f in result['failures']] == [
        'failed_check_closed_by_exact_rerun', 'complete_retained_output_metadata']
    code, result = _run(argv[:-1] + [str(task.owner_epoch.epoch + 1)])
    assert (code, result) == (2, dict(passed=False, error='inspection_binding_mismatch'))
    code, _ = _run(argv + ['--expect-route', 'backend=claude-agent-acp/haiku'])
    assert code == 2


def test_task_stuck_waiting_for_the_user_is_a_failed_assertion(tmp_path, monkeypatch):
    def synthesis_only(store, channel, context, seen):
        _try(seen, store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, synthesis_only)
    assert context['state'] == 'WAITING_USER'
    code, result = _run(_bind(store, tmp_path, task))
    assert code == 1 and result['passed'] is False
    assert result['failures'][0] == dict(expectation='state', expected='COMPLETED', observed='WAITING_USER')
    code, result = _run(_bind_again(store, tmp_path, task) + ['--expect-state', 'WAITING_USER'])
    assert (code, result['failures']) == (0, [])


def _bind_again(store, tmp_path, task):
    sid = 'acps_' + 'b' * 12
    return [sys.executable, '-B', str(SCRIPT), '--project', str(tmp_path), '--session', sid,
            '--task', task.task_id, '--owner-epoch', str(task.owner_epoch.epoch)]


# Controlled metadata counterexamples, not live qualification evidence.
REF = 'f' * 64
BINDING = dict(project_root='/fixture', session_id='acps_' + '1' * 12, task_id='t_' + '2' * 12, owner_epoch=1)


def _evaluate(rows, *, routes=(), output=None, binding=BINDING):
    args = Namespace(expect_state='EXECUTING', expect_route=routes, expect_closed_failure=False,
                     expect_retained_output_bytes=output, task=BINDING['task_id'], owner_epoch=1)
    return qualification.evaluate([dict(section='runtime', state='EXECUTING', revision=1)] + rows,
                                  args, binding=binding)


def _route_rows():
    return [
        dict(section='nodes', role='worker-backend-dev', assignment_ref=REF),
        dict(section='attempts', role_id='worker-backend-dev', idempotency_key=REF,
             attempt_id='xa_' + '1' * 12, engine_session_ref='es_' + '1' * 12, owner_epoch=1, config_revision='cr_fixture'),
        dict(section='task_events', id=1, kind='worker_configuration_confirmed', assignment_ref=REF,
             attempt_id='xa_' + '1' * 12, engine_session_ref='es_' + '1' * 12,
             engine='claude-agent-acp', model='haiku', parameters=dict(thought_level='high'),
             confirmation_evidence_ref='fixture-native-confirmation', config_revision_id='cr_fixture',
             connection_id='cn_fixture'),
        dict(section='configuration', id='cr_fixture'),
        dict(section='dispatch', attempt_id='xa_' + '1' * 12, decision='authorized', role='worker-backend-dev',
             engine='claude-agent-acp', model='haiku', connection_id='cn_fixture', config_revision_id='cr_fixture'),
    ]


ROUTE = [('worker-backend-dev', 'claude-agent-acp', 'haiku', 'high')]


def test_route_requires_native_identity_but_never_infers_unavailable_dispatch():
    rows = _route_rows()
    assert _evaluate(rows, routes=ROUTE) == []
    assert _evaluate(rows[:3], routes=ROUTE) == []  # no dispatch/config rows exposed
    rows[2]['parameters'] = dict(effort='high')
    assert _evaluate(rows, routes=ROUTE) == []
    assert _evaluate(rows, routes=ROUTE, binding=None)[0]['expectation'] == 'route'
    assert _evaluate(rows, routes=ROUTE, binding=dict(BINDING, task_id='another'))


@pytest.mark.parametrize('index, update', [
    (1, dict(owner_epoch=2)), (1, dict(engine_session_ref=None)), (1, dict(idempotency_key=None)),
    (2, dict(engine_session_ref=None)), (2, dict(engine_session_ref='es_wrong')),
    (2, dict(assignment_ref=None)), (2, dict(assignment_ref='e' * 64)),
    (2, dict(attempt_id='xa_wrong')), (2, dict(confirmation_evidence_ref=None)),
    (2, dict(confirmation_evidence_ref='')), (2, dict(confirmation_evidence_ref='  ')),
    (2, dict(config_revision_id='cr_wrong')), (2, dict(id=None)), (2, dict(superseded=True)),
    (2, dict(parameters=dict(thought_level='high', effort='low'))),
    (3, dict(id='cr_wrong')), (4, dict(connection_id='cn_wrong')),
    (4, dict(config_revision_id='cr_wrong')), (4, dict(engine='codex-acp')),
    (4, dict(model='wrong')), (4, dict(role='worker-qa-engineer')), (4, dict(decision='denied')),
])
def test_route_rejects_missing_or_mismatched_identity(index, update):
    rows = _route_rows()
    rows[index].update(update)
    assert _evaluate(rows, routes=ROUTE)[0]['expectation'] == 'route'


@pytest.mark.parametrize('index', [0, 1, 2, 3, 4])
def test_route_rejects_duplicate_relational_rows(index):
    rows = _route_rows()
    rows.append(dict(rows[index]))
    assert _evaluate(rows, routes=ROUTE)


@pytest.mark.parametrize('conflict_first', [False, True])
def test_route_rejects_conflicting_confirmations_in_either_order(conflict_first):
    rows = _route_rows()
    conflict = dict(rows[2], id=2, engine_session_ref='es_wrong', confirmation_evidence_ref='different')
    rows.insert(2 if conflict_first else 3, conflict)
    assert _evaluate(rows, routes=ROUTE)


@pytest.mark.parametrize('update', [
    dict(id=1, confirmation_evidence_ref='different'),
    dict(id=2, confirmation_evidence_ref='fixture-native-confirmation'),
])
def test_route_rejects_replayed_event_or_confirmation_evidence(update):
    rows = _route_rows()
    rows.append(dict(rows[2], attempt_id='xa_other', engine_session_ref='es_other', assignment_ref='e' * 64, **update))
    assert _evaluate(rows, routes=ROUTE)


def _receipt():
    return dict(section='action_receipts', operation='command_run', identity_sha256='a' * 64,
                tool_call_ref='gateway:' + 'b' * 32, large_output_id='lo_' + 'c' * 12,
                complete=True, run_complete=True, exit_code=0, failure=None, error=None,
                retained_output=dict(bound=True, byte_count=65537, truncated=False, exit_code=0,
                                     failure=None, signal=None))


@pytest.mark.parametrize('operation', ['command_run', 'outside_command'])
def test_retained_output_accepts_only_complete_metadata_and_exact_size(operation):
    receipt = _receipt()
    receipt['operation'] = operation
    assert _evaluate([receipt], output=65537) == []
    assert _evaluate([receipt], output=65538)[0]['expectation'] == 'complete_retained_output_metadata'


@pytest.mark.parametrize('update', [
    dict(operation='project_write'), dict(complete=False), dict(complete=None), dict(complete=1),
    dict(run_complete=False), dict(run_complete=None), dict(run_complete=1),
    dict(exit_code=1), dict(exit_code=None), dict(exit_code=False),
    dict(failure='timeout'), dict(error='execution_error'), dict(signal='SIGTERM'),
    dict(tool_call_ref=None), dict(tool_call_ref='unrecognized'), dict(identity_sha256=None),
    dict(large_output_id=None), dict(large_output_id='invalid'),
])
def test_retained_output_rejects_each_bad_execution_signal(update):
    receipt = _receipt()
    receipt.update(update)
    assert _evaluate([receipt], output=65537)[0]['expectation'] == 'complete_retained_output_metadata'


@pytest.mark.parametrize('update', [
    dict(bound=False), dict(bound=1), dict(truncated=True), dict(truncated=None),
    dict(exit_code=1), dict(exit_code=None), dict(exit_code=False),
    dict(failure='timeout'), dict(error='output_error'), dict(signal='SIGTERM'),
    dict(byte_count=None), dict(byte_count=-1), dict(byte_count='65537'), dict(byte_count=True),
])
def test_retained_output_rejects_each_bad_retained_signal(update):
    receipt = _receipt()
    receipt['retained_output'].update(update)
    assert _evaluate([receipt], output=65537)[0]['expectation'] == 'complete_retained_output_metadata'


@pytest.mark.parametrize('shared', ['tool_call_ref', 'identity_sha256', 'large_output_id', 'all'])
def test_retained_output_refuses_duplicate_or_ambiguous_bindings(shared):
    receipt = _receipt()
    other = deepcopy(receipt)
    if shared != 'all':
        other.update(tool_call_ref='gateway:' + 'd' * 32, identity_sha256='e' * 64, large_output_id='lo_' + 'f' * 12)
        other[shared] = receipt[shared]
    assert _evaluate([receipt, other], output=65537)


def test_inconsistent_execution_and_output_metadata_cannot_hide_behind_size():
    receipt = _receipt()
    receipt.update(complete=False, run_complete=False, failure='timeout', exit_code=0)
    receipt['retained_output'].update(exit_code=7, failure='timeout', byte_count=1000000)
    assert _evaluate([receipt], output=65537)


def completion_rows():
    return [
        dict(section='runtime', state='COMPLETED', revision=1),
        dict(section='nodes', revision=1, node_id='work', stage='execution', status='completed', assignment_ref=REF),
        dict(section='runtime_events', id=1, type='EXECUTION_BASELINE', revision=1, kind='observed',
             revision_digest='a' * 64, head='b' * 40),
        dict(section='runtime_events', id=2, type='REVIEW_NOT_REQUIRED', revision=1, reason='no_project_changes',
             reference='execution_baseline', reference_revision=1, revision_digest='a' * 64, head='b' * 40),
        dict(section='runtime_events', id=3, type='VERIFICATION_PASSED', checks=[dict(exit_code=0, complete=True)]),
        dict(section='synthesis', text_sha256='f' * 64)]


def completed(rows, require_review=False):
    args = Namespace(expect_state='COMPLETED', expect_route=[], expect_closed_failure=False,
                     expect_retained_output_bytes=None, require_review=require_review)
    return qualification.evaluate(rows, args, binding=BINDING)


def test_valid_unchanged_project_waiver_is_not_a_missing_review():
    assert completed(completion_rows()) == []
    assert completed(completion_rows(), require_review=True)[0]['expectation'] == 'review_or_valid_waiver'


@pytest.mark.parametrize('change', ['missing', 'mismatch', 'duplicate', 'prose'])
def test_waiver_requires_actual_runtime_baseline(change):
    rows = completion_rows()
    if change == 'missing':
        rows.pop(3)
    elif change == 'mismatch':
        rows[3]['revision_digest'] = 'c' * 64
    elif change == 'duplicate':
        rows.append(deepcopy(rows[3]))
    else:
        rows[3] = dict(section='synthesis', text='REQUEST_CHANGES APPROVE REVIEW_NOT_REQUIRED')
    assert completed(rows)


def test_real_review_verdict_is_not_execution_qa_or_arbitrary_prose():
    rows = completion_rows()
    rows.pop(3)
    rows += [dict(section='nodes', revision=1, node_id='_review_1', stage='review', status='completed', assignment_ref='e' * 64),
             dict(section='attempts', idempotency_key='e' * 64, attempt_id='xa_review', owner_epoch=1),
             dict(section='runtime_events', type='WORKER_COMPLETED', node='_review_1', attempt_id='xa_review', verdict='APPROVE')]
    assert completed(rows, require_review=True) == []
    rows[-1]['verdict'] = None
    rows[-1]['text'] = 'APPROVE'
    assert completed(rows)
    rows[-1]['verdict'] = 'REQUEST_CHANGES'
    assert completed(rows)
    rows[-1]['verdict'] = 'APPROVE'
    rows[-1]['attempt_id'] = 'xa_execution_qa'
    assert completed(rows)


def test_exact_command_expectation_rejects_unattached_or_foreign_attempt_receipts():
    receipt = _receipt()
    receipt.update(channel_owner_epoch=1, channel_attempt_id="xa_current")
    event = dict(section="runtime_events", type="WORKER_COMPLETED", attempt_id="xa_current",
                 checks=[dict(tool_call_ref=receipt["tool_call_ref"], outcome="passed")])
    args = Namespace(expect_state="EXECUTING", expect_route=[], expect_closed_failure=False,
                     expect_retained_output_bytes=None, expect_command_ref=[receipt["tool_call_ref"]])
    rows = [dict(section="runtime", state="EXECUTING", revision=1), receipt, event]
    assert qualification.evaluate(rows, args, binding=BINDING) == []
    event["attempt_id"] = "xa_foreign"
    assert qualification.evaluate(rows, args, binding=BINDING)
    event["attempt_id"] = "xa_current"
    receipt["channel_owner_epoch"] = 2
    assert qualification.evaluate(rows, args, binding=BINDING)
    receipt["channel_owner_epoch"] = 1
    event["checks"] = []
    assert qualification.evaluate(rows, args, binding=BINDING)
