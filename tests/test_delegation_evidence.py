"""A passed check is verified only by an exact complete command receipt of this attempt.

Every insufficient observation keeps the check unverified with a structured
``evidence_gap``; a cited nonzero exit keeps it failed. Nothing is refused
silently and nothing unverified becomes a pass.
"""
import json
import pytest
from cortex_runtime.contracts import StoreError
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.evidence import record_tool_result_observed, trivial_command
from cortex_runtime.orchestration.state import TaskMachine
from runtime_scenarios import provision_fixture,publish_fixture_report

PASSING = ['python3', '-B', '-c', 'print("observed")']


def _fixture(tmp_path, name):
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='evidence',event_id=name),'Check','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    worker,assignment=provision_fixture(store,coordinator)
    return store,task,coordinator,worker,assignment


def _node(store, task, assignment):
    return next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref']==assignment)


def _run(store, channel, argv=PASSING, key='check', **extra):
    return mcp_tools.dispatch(store,channel,'command_run',dict(argv=argv,request_key=key,**extra))


def _tamper_receipt(store, ref, change):
    with store.connection() as db:
        rows=db.execute("SELECT actor,request_key,result_json FROM runtime_requests WHERE operation='command_run'").fetchall()
        row=next(r for r in rows if ref in r['result_json'])
        saved=json.loads(row['result_json']); change(saved)
        db.execute('UPDATE runtime_requests SET result_json=? WHERE actor=? AND request_key=?',
                   (json.dumps(saved),row['actor'],row['request_key']))
        db.commit()


@pytest.mark.parametrize('case,gap', [
    ('missing_ref', 'tool_call_ref_missing'),
    ('uncited_after_engine_pwd', 'tool_call_ref_missing'),
    ('engine_read_ref', 'not_a_command'),
    ('gateway_write_ref', 'not_a_command'),
    ('engine_shell_ref', 'engine_command_unverifiable'),
    ('foreign_attempt', 'foreign_attempt'),
    ('unknown_ref', 'tool_call_ref_unknown'),
    ('pending', 'call_incomplete'),
    ('timeout', 'call_incomplete'),
    ('engine_in_progress', 'call_incomplete'),
    ('truncated', 'result_truncated'),
    ('output_missing', 'result_missing'),
    ('replayed', 'ambiguous_ref'),
    ('argv_unrecorded', 'command_unrecorded'),
    ('trivial_pwd', 'trivial_command'),
    ('trivial_echo_script', 'trivial_command'),
    ('masked_exit_script', 'trivial_command'),
])
def test_insufficient_evidence_keeps_a_passed_claim_unverified(tmp_path, case, gap):
    store,task,coordinator,worker,assignment=_fixture(tmp_path,case)
    report=publish_fixture_report(store,worker)
    ref=None
    if case in {'uncited_after_engine_pwd','engine_shell_ref','engine_in_progress'}:
        # The audit's case: the engine's own shell ran `pwd` and completed.
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell','execute','pwd','pending')
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell',None,None,
                                    'in_progress' if case=='engine_in_progress' else 'completed')
        ref=None if case=='uncited_after_engine_pwd' else 'tc_shell'
    elif case=='engine_read_ref':
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_read','read','Read src/app.py','pending')
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_read',None,None,'completed')
        ref='tc_read'
    elif case=='gateway_write_ref':
        mcp_tools.dispatch(store,worker,'project_write',dict(path='note.txt',content='x',expected_sha256='absent',request_key='w'))
        with store.connection() as db:
            ref=json.loads(db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_AUTHORIZED'",
                                      (task.task_id,)).fetchone()[0])['tool_call_ref']
    elif case=='foreign_attempt':
        other,_=provision_fixture(store,coordinator,key='other')
        ref=_run(store,other)['tool_call_ref']
    elif case=='unknown_ref':
        ref='gateway:'+'0'*32
    elif case=='timeout':
        ref=_run(store,worker,['python3','-B','-c','import time; time.sleep(30)'],timeout_seconds=1)['tool_call_ref']
    elif case=='trivial_pwd':
        ref=_run(store,worker,['pwd'])['tool_call_ref']
    elif case=='trivial_echo_script':
        ref=_run(store,worker,['bash','-c','cd . && echo tests passed'])['tool_call_ref']
    elif case=='masked_exit_script':
        ref=_run(store,worker,['bash','-c','python3 -B -c "raise SystemExit(3)" || true'])['tool_call_ref']
    elif case!='missing_ref':
        ref=_run(store,worker)['tool_call_ref']
        if case=='pending':
            _tamper_receipt(store,ref,lambda saved: saved.update(complete=False))
        elif case=='argv_unrecorded':
            _tamper_receipt(store,ref,lambda saved: saved['receipt'].pop('argv'))
        elif case=='output_missing':
            _tamper_receipt(store,ref,lambda saved: saved['receipt'].pop('large_output_id'))
        elif case=='truncated':
            with store.connection() as db:
                db.execute('UPDATE large_outputs SET truncated=1 WHERE source_ref=?',(ref,)); db.commit()
        elif case=='replayed':
            with store.connection() as db:
                db.execute("INSERT INTO runtime_requests(task_id,request_key,actor,operation,digest,result_json) "
                           "SELECT task_id,'duplicate',actor,operation,digest,result_json FROM runtime_requests "
                           "WHERE actor=? AND request_key='check'",(worker.channel_id,))
                db.commit()
    check=dict(name='unit tests',outcome='passed',**({'tool_call_ref':ref} if ref else {}))
    receipt=mcp_tools.dispatch(store,worker,'result_submit',
                               dict(report_ref=report,summary='Observed',checks=[check],request_key='result'))
    assert receipt['checks']==[dict(name='unit tests',outcome='unverified',claimed_outcome='passed',evidence_gap=gap,
                                    blocker=receipt['checks'][0]['blocker'])]
    assert receipt['checks'][0]['blocker'].startswith('Runtime kept this passed claim unverified: ')
    node=_node(store,task,assignment)
    # An unverified check is a retained disclosure, never a pass and never a failure.
    assert node['status']=='completed'
    stored=node['result']['checks'][0]
    assert (stored['outcome'],stored['claimed_outcome'],stored['evidence_gap'],stored['evidence_strength'])==(
        'unverified','passed',gap,'none')
    with store.connection() as db:
        event=json.loads(db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
                                    (task.task_id,)).fetchone()[0])
    assert (event['checks'][0]['outcome'],event['checks'][0]['evidence_gap'],event['checks'][0]['evidence'])==(
        'unverified',gap,'claimed')


def test_a_passed_claim_citing_a_nonzero_exit_stays_a_failure(tmp_path):
    store,task,_,worker,assignment=_fixture(tmp_path,'nonzero')
    report=publish_fixture_report(store,worker)
    ref=_run(store,worker,['python3','-B','-c','raise SystemExit(2)'])['tool_call_ref']
    receipt=mcp_tools.dispatch(store,worker,'result_submit',dict(report_ref=report,summary='Observed',
        checks=[dict(name='unit tests',outcome='passed',tool_call_ref=ref)],request_key='result'))
    assert receipt['checks']==[dict(name='unit tests',outcome='failed',claimed_outcome='passed',evidence_gap='nonzero_exit')]
    node=_node(store,task,assignment)
    assert node['status']=='failed'
    assert node['result']['checks'][0]['evidence_strength']=='verified'  # the failure itself is verified


def test_an_exact_complete_successful_command_verifies_a_pass(tmp_path):
    store,task,_,worker,assignment=_fixture(tmp_path,'exact')
    report=publish_fixture_report(store,worker)
    run=_run(store,worker,['env','PYTHONDONTWRITEBYTECODE=1','python3','-B','-c','print("2 passed")'])
    assert run['argv'][0]=='env' and run['exit_code']==0 and run['complete']
    args=dict(report_ref=report,summary='Observed',request_key='result',
              checks=[dict(name='unit tests',outcome='passed',tool_call_ref=run['tool_call_ref']),
                      dict(name='lint',outcome='not_run',blocker='The linter is not installed in this checkout.')])
    receipt=mcp_tools.dispatch(store,worker,'result_submit',args)
    assert receipt['checks'][0]==dict(name='unit tests',outcome='passed')
    node=_node(store,task,assignment)
    assert node['status']=='completed'
    assert [(c['outcome'],c['evidence_strength']) for c in node['result']['checks']]==[('passed','verified'),('not_run','none')]
    assert 'evidence_gap' not in node['result']['checks'][0]
    with store.connection() as db:
        event=json.loads(db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
                                    (task.task_id,)).fetchone()[0])
    assert event['checks'][0]['evidence']=='observed'
    # An exact retry returns the same receipt and records nothing twice.
    assert mcp_tools.dispatch(store,worker,'result_submit',args)==receipt


def test_one_command_verifies_one_check_only(tmp_path):
    store,task,_,worker,assignment=_fixture(tmp_path,'duplicate')
    report=publish_fixture_report(store,worker)
    ref=_run(store,worker)['tool_call_ref']
    receipt=mcp_tools.dispatch(store,worker,'result_submit',dict(report_ref=report,summary='Observed',request_key='result',
        checks=[dict(name='unit tests',outcome='passed',tool_call_ref=ref),
                dict(name='integration tests',outcome='passed',tool_call_ref=ref)]))
    assert [(c['outcome'],c.get('evidence_gap')) for c in receipt['checks']]==[
        ('passed',None),('unverified','ref_already_cited')]


@pytest.mark.parametrize('ref_kind', ['none', 'zero_exit', 'engine'])
def test_a_failed_claim_is_never_softened(tmp_path, ref_kind):
    store,task,_,worker,assignment=_fixture(tmp_path,'failed-'+ref_kind)
    report=publish_fixture_report(store,worker)
    check=dict(name='unit tests',outcome='failed')
    if ref_kind=='zero_exit':
        check['tool_call_ref']=_run(store,worker)['tool_call_ref']
    if ref_kind=='engine':
        record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell','execute','pytest','completed')
        check['tool_call_ref']='tc_shell'
    receipt=mcp_tools.dispatch(store,worker,'result_submit',
                               dict(report_ref=report,summary='Observed',checks=[check],request_key='result'))
    assert receipt['checks']==[dict(name='unit tests',outcome='failed')]
    node=_node(store,task,assignment)
    assert node['status']=='failed'
    assert node['result']['checks'][0]['evidence_strength']=='weak'


@pytest.mark.parametrize('outcome,exit_code,stored,status', [
    ('passed', 0, 'passed', 'completed'),
    ('passed', 2, 'failed', 'failed'),
    ('failed', 2, 'failed', 'failed'),
    ('failed', 0, 'failed', 'failed'),
])
def test_outside_command_receipt_supports_matching_check_outcome(tmp_path, monkeypatch, outcome, exit_code, stored, status):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, 'request', lambda *args, **kwargs: True)
    store,task,_,worker,assignment=_fixture(tmp_path,f'outside-{outcome}-{exit_code}')
    report=publish_fixture_report(store,worker)
    receipt=mcp_tools.dispatch(store,worker,'outside_command',
        dict(argv=['python3','-B','-c',f'raise SystemExit({exit_code})'],cwd=str(tmp_path.parent),request_key='outside-check'))
    assert receipt['complete'] and receipt['exit_code']==exit_code and receipt['argv'][0]=='python3'
    mcp_tools.dispatch(store,worker,'result_submit',dict(report_ref=report,summary='Observed',request_key='result',
        checks=[dict(name='outside check',outcome=outcome,tool_call_ref=receipt['tool_call_ref'])]))
    node=_node(store,task,assignment)
    assert (node['status'],node['result']['checks'][0]['outcome'])==(status,stored)
    # A failure claim contradicted by exit 0 still fails; its evidence is weak.
    assert node['result']['checks'][0]['evidence_strength']==('weak' if (outcome,exit_code)==('failed',0) else 'verified')


def test_older_weak_results_are_marked_at_read_time_without_rewriting_them(tmp_path):
    """A result accepted under the previous rule (an engine `pwd` stood in
    for a pass) keeps its stored outcome; reads mark its evidence weak."""
    store,task,_,worker,assignment=_fixture(tmp_path,'historical')
    record_tool_result_observed(store,task.task_id,worker.attempt_id,'tc_shell','execute','pwd','completed')
    legacy=dict(report_ref='r_000000000000',summary='Older result',checks=[
        dict(name='unit tests',outcome='passed'),
        dict(name='lint',outcome='passed',tool_call_ref='tc_shell')])
    with store.connection() as db:
        db.execute('UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND assignment_ref=?',
                   ('completed',json.dumps(legacy),task.task_id,assignment))
        db.commit()
    node=_node(store,task,assignment)
    assert [(c['outcome'],c['evidence_strength'],c['evidence_gap']) for c in node['result']['checks']]==[
        ('passed','weak','tool_call_ref_missing'),('passed','weak','engine_command_unverifiable')]
    with store.connection() as db:
        assert json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                     (assignment,)).fetchone()[0])==legacy


def test_result_recovery_records_the_accepted_checks(tmp_path):
    from unittest.mock import patch
    from cortex_runtime.orchestration.recovery import reconcile_results
    store,task,_,worker,assignment=_fixture(tmp_path,'recovery')
    report=publish_fixture_report(store,worker)
    args=dict(report_ref=report,summary='Observed',request_key='result',
              checks=[dict(name='unit tests',outcome='passed')])
    with patch.object(TaskMachine,'record_result',side_effect=RuntimeError('process interruption')):
        with pytest.raises(RuntimeError):
            mcp_tools.dispatch(store,worker,'result_submit',args)
    reconcile_results(store,task.task_id)
    node=_node(store,task,assignment)
    assert node['status']=='completed'
    assert (node['result']['checks'][0]['outcome'],node['result']['checks'][0]['evidence_gap'])==(
        'unverified','tool_call_ref_missing')


def test_blocker_rules_still_refuse_malformed_checks(tmp_path):
    store,task,_,worker,assignment=_fixture(tmp_path,'blockers')
    report=publish_fixture_report(store,worker)
    with pytest.raises(StoreError,match='invalid_arguments'):
        mcp_tools.dispatch(store,worker,'result_submit',dict(report_ref=report,summary='Observed',request_key='r1',
            checks=[dict(name='unit tests',outcome='unverified')]))
    with pytest.raises(StoreError,match='invalid_arguments'):
        mcp_tools.dispatch(store,worker,'result_submit',dict(report_ref=report,summary='Observed',request_key='r2',
            checks=[dict(name='unit tests',outcome='passed',blocker='none')]))


@pytest.mark.parametrize('argv,trivial', [
    (['pwd'], True), (['ls', '-la'], True), (['echo', 'ok'], True), (['/bin/cat', 'README.md'], True),
    (['grep', '-rq', 'TODO', 'src'], True), (['find', '.', '-name', '*.py'], True), (['true'], True),
    (['git', 'status'], True), (['git', 'diff'], True), (['git', 'diff', '--check'], False),
    (['env', 'FOO=1'], True), (['timeout', '60', 'pwd'], True), (['bash', '-c', 'pwd'], True),
    (['bash', '-c', 'pytest || true'], True), (['bash', '-c', 'pytest; echo done'], True),
    (['bash', '-c', 'pytest -q 2>&1 | tail -5'], True), (['bash', '-c', 'pytest; exit 0'], True),
    (['sh', '-c', 'if pytest; then echo ok; fi'], True), (['bash', '-c', 'echo $(pytest)'], True),
    (['bash', '-c', 'unbalanced "quote'], True), ([], True),
    (['python3', '-B', '-m', 'pytest', '-q', 'tests/test_x.py'], False),
    (['env', 'PYTHONDONTWRITEBYTECODE=1', 'python3', '-B', '-m', 'pytest', '-q'], False),
    (['timeout', '600', 'npm', 'test'], False), (['npm', 'run', 'typecheck'], False),
    (['bash', '-c', 'cd web && npm test'], False), (['bash', '-lc', 'pytest && echo ok'], False),
    (['bash', '-ec', 'pytest; echo ok'], False), (['bash', '-c', 'set -o pipefail; pytest | tee log'], False),
    (['bash', '-c', 'pytest || exit 1'], False), (['bash', '-c', 'pytest; exit $?'], False),
    (['bash', '-c', '(cd x; pytest)'], False), (['bash', 'scripts/check.sh'], False),
])
def test_trivial_command_rule(argv, trivial):
    assert trivial_command(argv) is trivial


def test_result_submit_schema_states_the_evidence_contract():
    from cortex_runtime.orchestration.contracts import RESULT
    check=RESULT['properties']['checks']
    ref=check['items']['properties']['tool_call_ref']['description']
    outcome=check['items']['properties']['outcome']['description']
    assert 'verified passed' in ref and 'same attempt' in ref and 'exit_code=0' in ref and 'untruncated' in ref
    assert 'gateway:' in ref and 'engine shell call' in ref and 'one check only' in ref
    assert 'recorded as unverified with evidence_gap' in outcome and 'always kept as failed' in outcome
    assert 'qualitative review remarks' in check['description'] and 'report body' in check['description']


def test_status_only_tool_result_inherits_earliest_kind_and_title(tmp_path):
    store, task, _, worker, _ = _fixture(tmp_path, 'status-inheritance')
    record_tool_result_observed(store, task.task_id, worker.attempt_id, 'tc_status', 'execute', 'Run tests', 'pending')
    record_tool_result_observed(store, task.task_id, worker.attempt_id, 'tc_status', None, None, 'failed')
    with store.connection() as db:
        row = db.execute(
            "SELECT kind, title, status FROM tool_result_observations "
            "WHERE task_id=? AND attempt_id=? AND tool_call_ref=? ORDER BY rowid DESC LIMIT 1",
            (task.task_id, worker.attempt_id, 'tc_status'),
        ).fetchone()
    assert dict(row) == dict(kind='execute', title='Run tests', status='failed')
