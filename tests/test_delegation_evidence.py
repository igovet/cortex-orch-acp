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
    check=dict(name='check',outcome=outcome,tool_call_ref=ref)
    if outcome in {'not_run', 'unverified'}:
        check['blocker']='The fixture cannot establish this check outcome.'
    args=dict(report_ref=report,summary='Observed',checks=[check],request_key='result')
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
        if case!='failed':
            args['checks'][0]['blocker']='The fixture command receipt did not establish a pass.'
    mcp_tools.dispatch(store,worker,'result_submit',args)
    node=next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref']==assignment)
    # Only a real failure fails the node; not_run/unverified are retained disclosures.
    assert node['status']==('failed' if case=='failed' else 'completed')
    assert node['result']['checks'][0]['outcome']==args['checks'][0]['outcome']


@pytest.mark.parametrize('case',['uncited_ran','cited_completed','cited_failed','no_shell'])
def test_a_check_the_engine_ran_with_its_own_shell_counts_as_passed(tmp_path,case):
    from cortex_runtime.delegation.evidence import record_tool_result_observed
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='evidence',event_id='shell'),'Check','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    worker,assignment=provision_fixture(store,coordinator)
    report=publish_fixture_report(store,worker)
    if case!='no_shell':
        # As engines report it: kind only on the first update, then status alone.
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell','execute','pytest -q','pending')
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell',None,None,
                                    'failed' if case=='cited_failed' else 'completed')
    check=dict(name='pytest',outcome='passed',**({'tool_call_ref':'tc_shell'} if case.startswith('cited') else {}))
    args=dict(report_ref=report,summary='Observed',checks=[check],request_key='result')
    if case in {'cited_failed','no_shell'}:
        with pytest.raises(StoreError, match='check_evidence_required'):
            mcp_tools.dispatch(store,worker,'result_submit',args)
        return
    mcp_tools.dispatch(store,worker,'result_submit',args)
    node=next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref']==assignment)
    assert node['status']=='completed'


@pytest.mark.parametrize('outcome,exit_code,accepted', [
    ('passed', 0, True),
    ('passed', 2, False),
    ('failed', 2, True),
    ('failed', 0, False),
])
def test_outside_command_receipt_supports_matching_check_outcome(tmp_path, monkeypatch,
                                                                  outcome, exit_code, accepted):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, 'request', lambda *args, **kwargs: True)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='outside-evidence',event_id=outcome+str(exit_code)),
                        'Check','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    worker,assignment=provision_fixture(store,coordinator)
    report=publish_fixture_report(store,worker)
    receipt=mcp_tools.dispatch(store,worker,'outside_command',
        dict(argv=['python3','-B','-c',f'raise SystemExit({exit_code})'],
             cwd=str(tmp_path.parent),request_key='outside-check'))
    assert receipt['complete'] and receipt['exit_code']==exit_code
    args=dict(report_ref=report,summary='Observed',
              checks=[dict(name='outside check',outcome=outcome,
                           tool_call_ref=receipt['tool_call_ref'])],
              request_key='result')
    if not accepted:
        with pytest.raises(StoreError,match='check_evidence_required'):
            mcp_tools.dispatch(store,worker,'result_submit',args)
        with store.connection() as db:
            assert db.execute('SELECT 1 FROM runtime_requests WHERE actor=? AND request_key=?',
                              (worker.channel_id,'result')).fetchone() is None
        return
    mcp_tools.dispatch(store,worker,'result_submit',args)
    node=next(n for n in TaskMachine(store).context(task.task_id)['nodes']
              if n['assignment_ref']==assignment)
    assert node['status']==('completed' if outcome=='passed' else 'failed')


@pytest.mark.parametrize('tamper', ['foreign','pending','replayed','missing_output','mismatched_ref'])
def test_outside_command_receipt_rejects_unbound_or_unfinished_evidence(tmp_path,monkeypatch,tamper):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *args,**kwargs: True)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='outside-evidence',event_id=tamper),'Check','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    worker,assignment=provision_fixture(store,coordinator)
    report=publish_fixture_report(store,worker)
    owner=worker
    if tamper=='foreign':
        owner,_=provision_fixture(store,coordinator,key='other')
    receipt=mcp_tools.dispatch(store,owner,'outside_command',
        dict(argv=['python3','-B','-c','print("ok")'],cwd=str(tmp_path.parent),
             request_key='outside-check'))
    ref=receipt['tool_call_ref']
    if tamper!='foreign':
        with store.connection() as db:
            row=db.execute("SELECT result_json FROM runtime_requests WHERE actor=? AND request_key='outside-check'",
                           (worker.channel_id,)).fetchone()
            saved=json.loads(row[0])
            if tamper=='pending': saved['complete']=False
            if tamper=='missing_output': saved['receipt'].pop('large_output_id')
            if tamper=='mismatched_ref': saved['receipt']['tool_call_ref']='gateway:wrong'
            if tamper=='replayed':
                db.execute("INSERT INTO runtime_requests(task_id,request_key,actor,operation,digest,result_json) "
                           "SELECT task_id,'duplicate',actor,operation,digest,result_json "
                           "FROM runtime_requests WHERE actor=? AND request_key='outside-check'",
                           (worker.channel_id,))
            else:
                db.execute("UPDATE runtime_requests SET result_json=? "
                           "WHERE actor=? AND request_key='outside-check'",
                           (json.dumps(saved),worker.channel_id))
            db.commit()
    args=dict(report_ref=report,summary='Observed',
              checks=[dict(name='outside check',outcome='passed',tool_call_ref=ref)],
              request_key='result')
    with pytest.raises(StoreError,match='check_evidence_required'):
        mcp_tools.dispatch(store,worker,'result_submit',args)
