"""Passed checks require exact completed gateway receipts from this attempt."""
import json
import pytest
from cortex_runtime.contracts import StoreError
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from runtime_scenarios import provision_fixture,publish_fixture_report


@pytest.mark.parametrize('case',['passed','failed','not_run','unverified','missing','foreign','native','incomplete'])
def test_result_uses_actual_gateway_evidence(tmp_path,case):
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='evidence',event_id='one'),'Check','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    worker,assignment=provision_fixture(store,coordinator)
    report=publish_fixture_report(store,worker)
    owner=worker
    if case=='foreign':owner,_=provision_fixture(store,coordinator,key='other')
    receipt=mcp_tools.dispatch(store,owner,'command_run',dict(argv=['python3','-B','-c','raise SystemExit(2)' if case=='failed' else 'print("observed")'],request_key='check'))
    ref=receipt['tool_call_ref']
    if case=='native':ref='native-tool-1'
    if case=='missing':ref='gateway:absent'
    if case=='incomplete':
        with store.connection() as db:
            db.execute("UPDATE runtime_requests SET result_json=? WHERE operation='command_run'",(json.dumps(dict(complete=False)),));db.commit()
    outcome=case if case in {'not_run','unverified'} else 'passed'
    args=dict(report_ref=report,summary='Observed',checks=[dict(name='check',outcome=outcome,tool_call_ref=ref)],request_key='result')
    if case in {'failed','missing','foreign','native','incomplete'}:
        with pytest.raises(StoreError, match='check_evidence_required'):
            mcp_tools.dispatch(store,worker,'result_submit',args)
        with store.connection() as db:
            assert db.execute('SELECT 1 FROM runtime_requests WHERE actor=? AND request_key=?',
                              (worker.channel_id, 'result')).fetchone() is None
        node=next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref']==assignment)
        assert node['status']=='running' and node['result'] is None
        # An honest failure/unresolved check is retained and blocks completion.
        args['checks'][0]['outcome']='failed' if case=='failed' else 'unverified'
    mcp_tools.dispatch(store,worker,'result_submit',args)
    node=next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref']==assignment)
    assert node['status']==('completed' if case=='passed' else 'failed')
