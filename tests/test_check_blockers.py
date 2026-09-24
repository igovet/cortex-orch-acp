"""Verification gap blockers survive publication and recovery."""
import json
from unittest.mock import patch

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.recovery import reconcile_results
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report


def setup_worker(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='blocker', event_id='one'), 'Verify', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    report = publish_fixture_report(store, worker)
    return store, task, worker, assignment, report


@pytest.mark.parametrize('outcome, blocker, error', [
    ('not_run', None, 'check_blocker_required'),
    ('unverified', '   ', 'check_blocker_required'),
    ('passed', 'Unavailable host', 'check_blocker_unexpected'),
])
def test_invalid_blocker_rejected_before_request_or_publication(tmp_path, outcome, blocker, error):
    store, task, worker, assignment, report = setup_worker(tmp_path)
    check = dict(name='Live GUI', outcome=outcome)
    if blocker is not None:
        check['blocker'] = blocker
    args = dict(report_ref=report, summary='Observed', checks=[check], request_key='result')
    with pytest.raises(StoreError, match=error):
        mcp_tools.dispatch(store, worker, 'result_submit', args)
    with store.connection() as db:
        assert db.execute(
            'SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
            (task.task_id, worker.channel_id, 'result'),
        ).fetchone() is None
        assert db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
            (task.task_id, worker.attempt_id),
        ).fetchone() is None


def test_blocker_is_retained_in_result_and_publication(tmp_path):
    store, task, worker, assignment, report = setup_worker(tmp_path)
    blocker = 'The disposable GUI host failed to start.'
    args = dict(report_ref=report, summary='Observed',
                checks=[dict(name='Live GUI', outcome='not_run', blocker=blocker)],
                request_key='result')
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', args)
    assert receipt['recorded']
    node = next(n for n in TaskMachine(store).context(task.task_id)['nodes']
                if n['assignment_ref'] == assignment)
    assert node['status'] == 'completed'
    assert node['result']['checks'][0]['blocker'] == blocker
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
            (task.task_id, worker.attempt_id),
        ).fetchone()
    assert json.loads(row[0])['checks'][0]['blocker'] == blocker
    assert mcp_tools.dispatch(store, worker, 'result_submit', args) == receipt


def test_recovery_accepts_publication_with_blocker(tmp_path):
    store, task, worker, assignment, report = setup_worker(tmp_path)
    args = dict(report_ref=report, summary='Observed',
                checks=[dict(name='Live GUI', outcome='unverified',
                             blocker='The GUI closed before the result was observed.')],
                request_key='result')
    with patch.object(TaskMachine, 'record_result', side_effect=RuntimeError('interrupted')):
        with pytest.raises(RuntimeError, match='interrupted'):
            mcp_tools.dispatch(store, worker, 'result_submit', args)
    reconcile_results(store, task.task_id)
    node = next(n for n in TaskMachine(store).context(task.task_id)['nodes']
                if n['assignment_ref'] == assignment)
    assert node['status'] == 'completed'
    assert node['result']['checks'][0]['blocker'] == args['checks'][0]['blocker']


def test_synthesis_rejects_whitespace_blocker_before_publication(tmp_path):
    store,task,worker,assignment,_=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?",
                   (task.task_id,))
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?",
                   (json.dumps({'checks':[{'name':'Live GUI','outcome':'not_run',
                                           'blocker':'GUI unavailable'}]}),assignment))
        db.commit()
    with pytest.raises(StoreError,match='synthesis_blocker_required') as error:
        mcp_tools.dispatch(store,coordinator,'synthesis_submit',
            {'text':'Done','not_verified':[{'item':'Live GUI','blocker':'   '}]})
    assert error.value.field=='arguments.not_verified[0].blocker'
    with store.connection() as db:
        assert db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?',
                          (task.task_id,)).fetchone()[0] is None
    assert mcp_tools.dispatch(store,coordinator,'synthesis_submit',
        {'text':'Done','not_verified':[{'item':'Live GUI','blocker':'GUI failed to start.'}]})['recorded']
