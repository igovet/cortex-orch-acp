"""A check's current outcome is separate from its run history.

An earlier failed run stays in the result as history. It stops failing the
assignment only when a passed check of the same result cites an exact,
strictly verified, later rerun of the same command (operation, argv and, for
outside_command, cwd) that is still the latest run of that command in the
attempt. Anything weaker or ambiguous keeps the failure open; a later failure
of a command makes an earlier pass of it a failure.
"""
import json
import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.evidence import open_failure, record_tool_result_observed
from cortex_runtime.orchestration.state import TaskMachine, event
from runtime_scenarios import provision_fixture, publish_fixture_report

# Exits 0 once the fix (fixed.flag) is in the project, 1 before.
TOGGLE = ['python3', '-B', '-c', 'import pathlib,sys; sys.exit(0 if pathlib.Path("fixed.flag").exists() else 1)']


def _fixture(tmp_path, name):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='history', event_id=name), 'Check', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    return store, task, worker, assignment


def _run(store, channel, key, argv=TOGGLE):
    return mcp_tools.dispatch(store, channel, 'command_run', dict(argv=argv, request_key=key))


def _submit(store, worker, checks, **extra):
    report = publish_fixture_report(store, worker)
    return mcp_tools.dispatch(store, worker, 'result_submit',
                              dict(report=report, summary='Observed', checks=checks, request_key='result', **extra))


def _node(store, task, assignment):
    return next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref'] == assignment)


def _fail_then_fix(tmp_path, store, worker):
    failed = _run(store, worker, 'first')
    (tmp_path / 'fixed.flag').write_text('fixed')
    passed = _run(store, worker, 'rerun')
    assert (failed['exit_code'], passed['exit_code']) == (1, 0)
    return failed['tool_call_ref'], passed['tool_call_ref']


def test_an_exact_later_rerun_closes_the_failure_and_keeps_it_as_history(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, 'closed')
    failed, passed = _fail_then_fix(tmp_path, store, worker)
    receipt = _submit(store, worker, [dict(name='unit tests', outcome='failed', tool_call_ref=failed),
                                      dict(name='unit tests', outcome='passed', tool_call_ref=passed)])
    assert receipt['checks'] == [dict(name='unit tests', outcome='failed', closed_by=passed),
                                 dict(name='unit tests', outcome='passed')]
    node = _node(store, task, assignment)
    assert node['status'] == receipt['node_status'] == 'completed'
    # The failure is retained, visible and still verified as a real failed run.
    assert [(c['outcome'], c.get('closed_by'), c['evidence_strength']) for c in node['result']['checks']] == [
        ('failed', passed, 'verified'), ('passed', None, 'verified')]
    with store.connection() as db:
        published = json.loads(db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
                                          (task.task_id,)).fetchone()[0])
        completed = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='WORKER_COMPLETED'",
                               (task.task_id,)).fetchone()[0]
    assert [(c['outcome'], c.get('closed_by'), c['evidence']) for c in published['checks']] == [
        ('failed', passed, 'observed'), ('passed', None, 'observed')]
    assert completed == 1


@pytest.mark.parametrize('case', ['different_argv', 'no_ref', 'engine_ref', 'earlier_pass',
                                  'later_uncited_failure', 'later_unresolved_run', 'unrecorded_later_run'])
def test_anything_short_of_an_exact_latest_rerun_keeps_the_failure(tmp_path, case):
    store, task, worker, assignment = _fixture(tmp_path, case)
    expected_pass = ('passed', None)
    if case == 'earlier_pass':
        # The latest run failed after an earlier pass of the same command.
        (tmp_path / 'fixed.flag').write_text('fixed')
        passed = _run(store, worker, 'pass-first')['tool_call_ref']
        (tmp_path / 'fixed.flag').unlink()
        failed = _run(store, worker, 'fail-later')['tool_call_ref']
        pass_check = dict(name='unit tests', outcome='passed', tool_call_ref=passed)
        expected_pass = ('failed', 'later_run_failed')
    elif case == 'different_argv':
        failed = _run(store, worker, 'first')['tool_call_ref']
        (tmp_path / 'fixed.flag').write_text('fixed')
        other = _run(store, worker, 'other', TOGGLE + ['tests/test_other.py'])['tool_call_ref']
        pass_check = dict(name='unit tests', outcome='passed', tool_call_ref=other)
    elif case in {'no_ref', 'engine_ref'}:
        failed = _run(store, worker, 'first')['tool_call_ref']
        (tmp_path / 'fixed.flag').write_text('fixed')
        pass_check = dict(name='unit tests', outcome='passed')
        expected_pass = ('unverified', 'tool_call_ref_missing')
        if case == 'engine_ref':
            record_tool_result_observed(store, task.task_id, worker.attempt_id, 'tc_shell', 'execute', 'pytest', 'completed')
            pass_check['tool_call_ref'] = 'tc_shell'
            expected_pass = ('unverified', 'engine_command_unverifiable')
    else:
        failed, passed = _fail_then_fix(tmp_path, store, worker)
        pass_check = dict(name='unit tests', outcome='passed', tool_call_ref=passed)
        if case == 'later_uncited_failure':
            (tmp_path / 'fixed.flag').unlink()
            assert _run(store, worker, 'again')['exit_code'] == 1
            expected_pass = ('failed', 'later_run_failed')
        elif case == 'later_unresolved_run':
            again = _run(store, worker, 'again')['tool_call_ref']
            _tamper(store, again, lambda saved: saved.update(complete=False))
            expected_pass = ('unverified', 'later_run_unresolved')
        else:
            # A later command whose identity the receipt does not record
            # could be the same command: ambiguity keeps the failure open.
            unrecorded = _run(store, worker, 'unrecorded', ['python3', '-B', '-c', 'print(1)'])['tool_call_ref']
            _tamper(store, unrecorded, lambda saved: saved['receipt'].pop('argv'))
    receipt = _submit(store, worker, [dict(name='unit tests', outcome='failed', tool_call_ref=failed), pass_check],
                      assignment_outcome='failed')
    assert receipt['checks'][0] == dict(name='unit tests', outcome='failed')
    assert (receipt['checks'][1]['outcome'], receipt['checks'][1].get('evidence_gap')) == expected_pass
    node = _node(store, task, assignment)
    assert node['status'] == 'failed'
    assert 'closed_by' not in node['result']['checks'][0]


def test_a_later_failure_of_a_cited_pass_fails_it_even_uncited(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, 'hidden')
    (tmp_path / 'fixed.flag').write_text('fixed')
    passed = _run(store, worker, 'pass')['tool_call_ref']
    (tmp_path / 'fixed.flag').unlink()
    assert _run(store, worker, 'fail')['exit_code'] == 1
    receipt = _submit(store, worker, [dict(name='unit tests', outcome='passed', tool_call_ref=passed)],
                      assignment_outcome='failed')
    assert receipt['checks'] == [dict(name='unit tests', outcome='failed', claimed_outcome='passed',
                                      evidence_gap='later_run_failed')]
    assert _node(store, task, assignment)['status'] == 'failed'


def test_a_run_the_gateway_rejected_before_executing_is_not_history(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, 'rejected')
    failed, passed = _fail_then_fix(tmp_path, store, worker)
    with store.connection() as db:
        event(db, task.task_id, 'TOOL_AUTHORIZED', dict(operation='command_run', tool_call_ref='gateway:' + 'f' * 32),
              worker.channel_id)
        event(db, task.task_id, 'TOOL_REJECTED', dict(operation='command_run', tool_call_ref='gateway:' + 'f' * 32,
                                                      error='gateway_io_failure'), worker.channel_id)
        db.commit()
    receipt = _submit(store, worker, [dict(name='unit tests', outcome='failed', tool_call_ref=failed),
                                      dict(name='unit tests', outcome='passed', tool_call_ref=passed)])
    assert receipt['checks'][0] == dict(name='unit tests', outcome='failed', closed_by=passed)
    assert _node(store, task, assignment)['status'] == 'completed'


def test_outside_command_identity_includes_its_cwd(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, 'request', lambda *args, **kwargs: True)
    (tmp_path / 'project').mkdir()
    store, task, worker, assignment = _fixture(tmp_path / 'project', 'outside')
    first, second = tmp_path / 'first', tmp_path / 'second'
    first.mkdir(); second.mkdir(); (second / 'fixed.flag').write_text('fixed')

    def outside(key, cwd):
        return mcp_tools.dispatch(store, worker, 'outside_command', dict(argv=TOGGLE, cwd=str(cwd), request_key=key))
    failed = outside('first', first)
    elsewhere = outside('elsewhere', second)
    assert (failed['exit_code'], elsewhere['exit_code'], elsewhere['cwd']) == (1, 0, str(second))
    receipt = _submit(store, worker, [dict(name='deploy check', outcome='failed', tool_call_ref=failed['tool_call_ref']),
                                      dict(name='deploy check', outcome='passed', tool_call_ref=elsewhere['tool_call_ref'])],
                      assignment_outcome='failed')
    assert receipt['checks'][0] == dict(name='deploy check', outcome='failed')
    assert _node(store, task, assignment)['status'] == 'failed'


def test_a_receipt_without_identity_keeps_its_pass_and_never_closes_a_failure(tmp_path, monkeypatch):
    """An outside_command receipt written before cwd was recorded."""
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, 'request', lambda *args, **kwargs: True)
    (tmp_path / 'project').mkdir(); (tmp_path / 'project' / 'fixed.flag').write_text('fixed')
    store, task, worker, assignment = _fixture(tmp_path / 'project', 'no-identity')
    run = mcp_tools.dispatch(store, worker, 'outside_command',
                             dict(argv=TOGGLE, cwd=str(tmp_path / 'project'), request_key='outside'))
    with store.connection() as db:
        row = db.execute("SELECT actor,request_key,result_json FROM runtime_requests WHERE operation='outside_command'").fetchone()
        saved = json.loads(row['result_json']); saved['receipt'].pop('cwd')
        db.execute('UPDATE runtime_requests SET result_json=? WHERE actor=? AND request_key=?',
                   (json.dumps(saved), row['actor'], row['request_key']))
        db.commit()
    receipt = _submit(store, worker, [dict(name='deploy check', outcome='passed', tool_call_ref=run['tool_call_ref'])])
    assert receipt['checks'] == [dict(name='deploy check', outcome='passed')]
    assert _node(store, task, assignment)['status'] == 'completed'


def test_open_failure_rederives_closure_and_never_trusts_a_stored_closed_by(tmp_path):
    store, task, worker, _ = _fixture(tmp_path, 'backstop')
    failed, passed = _fail_then_fix(tmp_path, store, worker)
    other = _run(store, worker, 'other', ['python3', '-B', '-c', 'print("other")'])['tool_call_ref']
    pass_check = dict(name='unit tests', outcome='passed', tool_call_ref=passed)
    with store.connection() as db:
        def still_failing(closed_by):
            check = dict(name='unit tests', outcome='failed', tool_call_ref=failed)
            if closed_by:
                check['closed_by'] = closed_by
            return open_failure(db, task.task_id, worker.attempt_id, [check, pass_check])
        assert still_failing(passed) is False
        # A retained result without the recorded closure, or with a forged one, still fails.
        assert still_failing(None) is True
        assert still_failing(other) is True


def test_recovery_records_the_closure_published_with_the_result(tmp_path):
    from unittest.mock import patch
    from cortex_runtime.orchestration.recovery import reconcile_results
    store, task, worker, assignment = _fixture(tmp_path, 'recovery')
    failed, passed = _fail_then_fix(tmp_path, store, worker)
    with patch.object(TaskMachine, 'record_result', side_effect=RuntimeError('process interruption')):
        with pytest.raises(RuntimeError):
            _submit(store, worker, [dict(name='unit tests', outcome='failed', tool_call_ref=failed),
                                    dict(name='unit tests', outcome='passed', tool_call_ref=passed)])
    reconcile_results(store, task.task_id)
    node = _node(store, task, assignment)
    assert node['status'] == 'completed'
    assert [(c['outcome'], c.get('closed_by')) for c in node['result']['checks']] == [('failed', passed), ('passed', None)]


def _tamper(store, ref, change):
    with store.connection() as db:
        rows = db.execute("SELECT actor,request_key,result_json FROM runtime_requests WHERE operation='command_run'").fetchall()
        row = next(r for r in rows if ref in r['result_json'])
        saved = json.loads(row['result_json']); change(saved)
        db.execute('UPDATE runtime_requests SET result_json=? WHERE actor=? AND request_key=?',
                   (json.dumps(saved), row['actor'], row['request_key']))
        db.commit()


def test_a_stop_question_lists_only_current_failures(tmp_path):
    from cortex_runtime.orchestration.controller import _stop_question
    result = dict(report_ref='r_000000000000', summary='Observed', checks=[
        dict(name='unit tests', outcome='failed', tool_call_ref='gateway:a', closed_by='gateway:b'),
        dict(name='unit tests', outcome='passed', tool_call_ref='gateway:b'),
        dict(name='lint', outcome='failed', tool_call_ref='gateway:c')])
    context = dict(nodes=[dict(node_id='N0', status='failed', result=result)])
    question = _stop_question(dict(blocker='worker_failed', question_id='q'), context, 'en')['question']
    assert 'lint: failed' in question and 'unit tests: failed' not in question


@pytest.mark.parametrize('intentional',[False,True])
def test_open_failed_check_refuses_before_recording_or_intentionally_fails(tmp_path,intentional):
    from cortex_runtime.contracts import StoreError
    store,task,worker,assignment=_fixture(tmp_path,'open-failure')
    failed=_run(store,worker,'wrong-check')['tool_call_ref']
    other=_run(store,worker,'corrected-check',['python3','-B','-c','print("corrected assertion")'])['tool_call_ref']
    args=dict(report='# Result\n\nObserved.', report_title='Result',summary='Observed',request_key='result',
              checks=[dict(name='unit tests',outcome='failed',tool_call_ref=failed),
                      dict(name='unit tests',outcome='passed',tool_call_ref=other)])
    with store.connection() as db:
        reports_before=db.execute("SELECT COUNT(*) FROM reports").fetchone()[0]
    if not intentional:
        with pytest.raises(StoreError,match='open_failed_check') as caught:
            mcp_tools.dispatch(store,worker,'result_submit',args)
        error=caught.value
        assert error.field == 'arguments.checks[0]'
        assert failed in error.received and 'unit tests' in error.received
        assert json.dumps(TOGGLE,ensure_ascii=False,separators=(',',':')) in error.correction
        assert 'assignment_outcome="failed"' in error.correction
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM runtime_requests WHERE operation='result_submit'").fetchone()[0] == 0
            assert db.execute("SELECT COUNT(*) FROM task_events WHERE event_type='worker_result'").fetchone()[0] == 0
            assert db.execute("SELECT COUNT(*) FROM reports").fetchone()[0] == reports_before
            assert db.execute("SELECT revoked_at FROM trusted_channels WHERE channel_id=?",
                              (worker.channel_id,)).fetchone()[0] is None
        assert _node(store,task,assignment)['status'] == 'running'
        (tmp_path/'fixed.flag').write_text('fixed')
        rerun=_run(store,worker,'exact-rerun')['tool_call_ref']
        args['checks'].append(dict(name='unit tests',outcome='passed',tool_call_ref=rerun))
        receipt=mcp_tools.dispatch(store,worker,'result_submit',args)
        assert receipt['checks'][0]['closed_by'] == rerun
        assert receipt['node_status'] == 'completed'
    else:
        receipt=mcp_tools.dispatch(store,worker,'result_submit',dict(args,assignment_outcome='failed'))
        assert receipt['recorded'] and receipt['node_status'] == 'failed'
        assert _node(store,task,assignment)['status'] == 'failed'
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE type='WORKER_FAILED'").fetchone()[0] == 1


def test_intentional_assignment_failure_without_a_command_failure_is_explicit(tmp_path):
    store,task,worker,assignment=_fixture(tmp_path,'intentional')
    receipt=_submit(store,worker,[],assignment_outcome='failed')
    assert receipt['node_status'] == _node(store,task,assignment)['status'] == 'failed'


# The codex worker of task t_2cf2f985b809 (2026-10-07) ran its profile check
# with an unknown --check option (argparse usage error, exit 2) and then with
# the supported positional form (exit 0). Citing the usage error as a failed
# check left it open for good, so it failed an otherwise verified assignment.
MISTYPED = ['python3', '-B', '-c', 'import argparse; argparse.ArgumentParser().parse_args()', '--check']
CORRECTED = ['python3', '-B', '-c',
             'import argparse; p = argparse.ArgumentParser(); p.add_argument("mode"); p.parse_args()', 'check']


def test_a_mistyped_invocation_is_never_closed_by_a_different_argv_and_need_not_be_cited(tmp_path):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.delegation.evidence import command_history
    store, task, worker, assignment = _fixture(tmp_path, 'mistyped')
    mistyped = _run(store, worker, 'mistyped', MISTYPED)
    corrected = _run(store, worker, 'corrected', CORRECTED)
    assert (mistyped['exit_code'], corrected['exit_code']) == (2, 0)
    cited = [dict(name='profile check', outcome='failed', tool_call_ref=mistyped['tool_call_ref']),
             dict(name='profile check', outcome='passed', tool_call_ref=corrected['tool_call_ref'])]
    # Exact identity binding is unchanged: a different argv closes nothing.
    with pytest.raises(StoreError, match='open_failed_check') as caught:
        _submit(store, worker, cited)
    assert 'a different argv does not close it' in caught.value.correction
    assert 'mistyped' in caught.value.correction and 'remove it from checks' in caught.value.correction
    # The advertised path: a command line that never started the check is not
    # a check of the work. The corrected command stands on its own receipt;
    # the mistyped run stays in the runtime's own history, never closed.
    receipt = _submit(store, worker, cited[1:])
    assert receipt['checks'] == [dict(name='profile check', outcome='passed')]
    assert receipt['node_status'] == _node(store, task, assignment)['status'] == 'completed'
    with store.connection() as db:
        history = command_history(db, task.task_id, worker.attempt_id)
    assert [(run['ref'], run['status']) for run in history] == [
        (mistyped['tool_call_ref'], 'failed'), (corrected['tool_call_ref'], 'passed')]
