"""T016: real cortex.py MCP subprocess over a trusted channel -- wire-level
exposure of the delegation operations (delegate/list_events/clarify/cancel/
decide for a coordinator channel; ask_question/publish_result for a worker
channel), role-filtered in both tools/list and tools/call.

Spawns the real ``src/cortex.py`` entry point as a
subprocess (never a real engine) against a tmp project store, exactly the
process boundary described in docs/acp/orchestration.md: this test's
subprocess plays the role of "an engine's own Cortex MCP child process".
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx

ROOT = Path(__file__).resolve().parents[1]
CORTEX_PY = ROOT / "src/cortex.py"


def _storage_only_home(home: Path) -> Path:
    """Keep this wire test independent of the optional gateway child (see tests/test_stdio.py)."""
    cortex = home / "cortex"
    cortex.mkdir(parents=True, exist_ok=True)
    cortex.chmod(0o700)
    config = cortex / "config.toml"
    config.write_text("schema_version = 1\n[gateway]\nenabled = false\n")
    config.chmod(0o600)
    return home


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return project, store, result.task_id, result.channel_id, result.secret


def _worker_channel(store, task_id, coordinator_channel_id, attempt_id="xa_wire1"):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", epoch, parent_channel_id=coordinator_channel_id,
            assignment_id="a_wire1", attempt_id=attempt_id,
        )
        db.commit()
    return channel_id, secret, attempt_id


def _call(project, channel_id, secret, messages, tmp_path):
    home = _storage_only_home(tmp_path / "home")
    (home / "sessions").mkdir(parents=True, exist_ok=True)
    env = os.environ | dict(
        CODEX_HOME=str(home),
        CORTEX_CHANNEL_ID=channel_id, CORTEX_CHANNEL_SECRET=secret,
        CORTEX_CHANNEL_PROJECT_ROOT=str(project),
    )
    result = subprocess.run(
        [sys.executable, "-B", str(CORTEX_PY)],
        input="\n".join(json.dumps(m) for m in messages) + "\n",
        text=True, capture_output=True, env=env, timeout=30,
    )
    assert result.returncode == 0 and result.stderr == "", result.stderr
    return [json.loads(line) for line in result.stdout.splitlines()]


def _init_and(method, params, id_=2):
    return [
        dict(jsonrpc="2.0", id=1, method="initialize",
             params=dict(protocolVersion="2025-11-25", capabilities={}, clientInfo=dict(name="t", version="1"))),
        dict(jsonrpc="2.0", method="notifications/initialized"),
        dict(jsonrpc="2.0", id=id_, method=method, params=params),
    ]



def _data(replies):
    response=replies[-1]
    assert 'error' not in response,response
    assert not response['result'].get('isError'),response
    return json.loads(response['result']['content'][0]['text'])


def test_roles_advertise_only_the_new_protocol(tmp_path):
    from cortex_runtime.delegation.contracts import schemas_for
    project,store,task,coordinator,secret=_bootstrap(tmp_path)
    worker,wsecret,_=_worker_channel(store,task,coordinator)
    for role,ch,key in [('coordinator',coordinator,secret),('worker',worker,wsecret)]:
        tools=_call(project,ch,key,_init_and('tools/list',{}),tmp_path)[-1]['result']['tools']
        names={t['name'] for t in tools}
        never_usable={'project_write','project_edit','project_mkdir','project_delete','project_skill_link','outside_write','outside_command','command_run'} if role=='coordinator' else set()
        assert set(schemas_for(role))-never_usable<=names
        assert not never_usable & names
        assert not {'delegate','decide','complete_assignment','integrate','set_governance','create_task'} & names
        assert ('plan_submit' in names)==(role=='coordinator')
        assert ('result_submit' in names)==(role=='worker')
        assert all(t['_meta']['anthropic/alwaysLoad'] is True for t in tools)
        read_skill = next(t for t in tools if t['name'] == 'read_skill')
        assert 'documentation-sync: before assessing documentation impact' in read_skill['description']
        assert 'documentation-sync: before assessing documentation impact' in (
            read_skill['inputSchema']['properties']['skill']['description'])
        named = read_skill['inputSchema']['properties']['skill']['anyOf'][0]
        assert 'documentation-sync' in named['enum']


def test_result_submit_wire_advertises_and_rejects_missing_report(tmp_path):
    project, store, task, coordinator, secret = _bootstrap(tmp_path)
    worker, worker_secret, _ = _worker_channel(store, task, coordinator)
    tools = _call(project, worker, worker_secret, _init_and('tools/list', {}), tmp_path)[-1]['result']['tools']
    schema = next(tool['inputSchema'] for tool in tools if tool['name'] == 'result_submit')
    assert 'oneOf' not in schema and 'if' not in schema and 'report' in schema['required']
    assert schema['properties']['report']['type'] == 'string' and 'properties' not in schema['properties']['report']
    args = dict(summary='Observed', checks=[], request_key='missing-report')
    refused = _call(project, worker, worker_secret, _init_and('tools/call',
                    dict(name='result_submit', arguments=args)), tmp_path)[-1]
    assert 'invalid_arguments' in json.dumps(refused) and 'arguments.report' in json.dumps(refused)
    assert 'report_id' in json.dumps(refused)


def test_plan_wire_is_idempotent_and_does_not_start_a_worker(tmp_path):
    from cortex_runtime.orchestration.state import TaskMachine
    project,store,task,ch,secret=_bootstrap(tmp_path)
    machine=TaskMachine(store);machine.create(task,'Build a page.')
    proposed=dict(tasks=[dict(id='A',role='backend-dev',objective='Implement',dependencies=[],scope=['src/**'],model='auto')],verification=[['true']])
    request=dict(name='plan_submit',arguments=dict(plan=proposed,request_key='plan'))
    first=_data(_call(project,ch,secret,_init_and('tools/call',request),tmp_path))
    assert first==_data(_call(project,ch,secret,_init_and('tools/call',request),tmp_path))
    assert machine.context(task)['state']=='PLAN_READY'
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM delegation_provisioning').fetchone()[0]==0
        assert db.execute('SELECT COUNT(*) FROM runtime_nodes').fetchone()[0]==1
    for operation in ['delegate','decide','complete_task','set_task_state','change_model']:
        result=_call(project,ch,secret,_init_and('tools/call',dict(name=operation,arguments={})),tmp_path)[-1]
        assert 'error' in result
    assert machine.context(task)['state']=='PLAN_READY'


def test_a_read_only_role_is_refused_an_output_file_it_could_never_write(tmp_path):
    from cortex_runtime.orchestration.state import TaskMachine
    project,store,task,ch,secret=_bootstrap(tmp_path)
    machine=TaskMachine(store);machine.create(task,'Review the concept.')
    (Path(project)/'docs').mkdir(exist_ok=True)
    node=dict(id='review',role='ux-designer',objective='Audit the concept.',dependencies=[],model='auto')
    def submit(scope,key):
        plan=dict(tasks=[dict(node,scope=scope)],verification=[['true']])
        return _call(project,ch,secret,_init_and('tools/call',dict(name='plan_submit',arguments=dict(plan=plan,request_key=key))),tmp_path)[-1]
    refused=submit(['docs','docs/review-output.md'],'with-output')
    assert 'read_only_scope_not_writable' in json.dumps(refused), refused
    assert machine.context(task)['state']!='PLAN_READY'
    accepted=submit(['docs','src/**'],'read-only')
    assert 'error' not in accepted and not accepted['result'].get('isError'), accepted
    assert machine.context(task)['state']=='PLAN_READY'


def test_worker_gateway_publication_and_result_cross_real_mcp_process(tmp_path):
    from cortex_runtime.trusted_channel import resolve_channel
    from runtime_scenarios import reserve_assignment
    from cortex_runtime.orchestration.state import TaskMachine
    project,store,task,ch,secret=_bootstrap(tmp_path)
    coordinator=resolve_channel(store,ch,secret)
    reserved=reserve_assignment(store,coordinator,dict(title='Implement',brief='Implement',role='backend-dev',acceptance='Report',request_key='one'))
    worker,wsecret,attempt=_worker_channel(store,task,ch)
    with store.connection() as db:
        stamp=store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
          (reserved['assignment_ref'],task,'a_wire1',attempt,'worker-backend-dev','ready','ready','shared_project_root',worker,None,None,None,'qualified',None,coordinator.owner_epoch,stamp,stamp))
        db.commit()
    def call(name,**args):return _data(_call(project,worker,wsecret,_init_and('tools/call',dict(name=name,arguments=args)),tmp_path))
    missing=call('project_read',path='new.txt')
    assert missing == dict(exists=False,path='new.txt',sha256='absent',parent_exists=True)
    assert call('project_read',path='missing-parent/new.txt')['parent_exists'] is False
    written=call('project_write',path='new.txt',content='new',expected_sha256=missing['sha256'])
    assert call('project_write',path='new.txt',content='new',expected_sha256=missing['sha256']) == written
    assert (project/'new.txt').read_text() == 'new'
    read=call('project_read',path='new.txt',line_numbers=False)
    assert read['content']=='new' and read['complete']
    edited=call('project_edit',path='new.txt',edits=[dict(old_string='new',new_string='edited')])
    assert edited['newText']=='edited' and edited['oldText']=='new'
    assert (project/'new.txt').read_text()=='edited'
    # Creation replay cannot overwrite the now-edited file.
    assert call('project_write',path='new.txt',content='new',expected_sha256='absent')==written
    assert (project/'new.txt').read_text()=='edited'
    draft=call('create_draft',template='general',request_key='draft')
    call('draft_write',draft_id=draft['draft_id'],content=draft['required_first_line']+'\n\n# Report\n\nCompleted fixture.\n',request_key='body')
    report=call('write_report',draft_id=draft['draft_id'],title='Report',summary='Observed',author='worker',request_key='publish')
    args=dict(report=report['report_id'],summary='Observed',checks=[],request_key='result')
    result=call('result_submit',**args)
    assert result['recorded'] and result['node_status']=='completed'
    # Accepted result revokes the channel even while its engine lingers.
    # A new authenticated wire call, including a replay, cannot reactivate it;
    # runtime recovery reads the durable receipt instead of republishing.
    for name, arguments in [('result_submit', args), ('project_write', dict(
            path='late.txt', content='late', expected_sha256='absent', request_key='late'))]:
        response = _call(project, worker, wsecret, _init_and(
            'tools/call', dict(name=name, arguments=arguments)), tmp_path)[-1]
        assert response['result']['isError']
        assert json.loads(response['result']['content'][0]['text'])['error'] == 'channel_revoked'
    with store.connection() as db:
        receipt = db.execute("SELECT result_json FROM runtime_requests WHERE task_id=? "
                             "AND actor=? AND request_key='result'", (task, worker)).fetchone()[0]
        assert json.loads(receipt)['complete']
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='worker_result'",
                          (task,)).fetchone()[0] == 1
    assert not (project / 'late.txt').exists()
    context=TaskMachine(store).context(task)
    assert context['state']=='EXECUTING'
    assert context['nodes'][0]['status']=='completed'
    assert context['nodes'][0]['result']['report_ref']==report['report_id']


def test_result_submit_accepts_descriptive_command_without_trusting_fake_receipt(tmp_path):
    from cortex_runtime.trusted_channel import resolve_channel
    from runtime_scenarios import reserve_assignment
    from cortex_runtime.orchestration.state import TaskMachine

    project, store, task, coordinator_id, coordinator_secret = _bootstrap(tmp_path)
    coordinator = resolve_channel(store, coordinator_id, coordinator_secret)
    reserved = reserve_assignment(store, coordinator, dict(
        title='Implement', brief='Implement', role='backend-dev', acceptance='Report', request_key='one'))
    worker, worker_secret, attempt = _worker_channel(store, task, coordinator_id)
    with store.connection() as db:
        stamp = store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (reserved['assignment_ref'], task, 'a_wire1', attempt, 'worker-backend-dev',
                    'ready', 'ready', 'shared_project_root', worker, None, None, None,
                    'qualified', None, coordinator.owner_epoch, stamp, stamp))
        db.commit()
    args = dict(report='# Report\n\nObserved.', report_title='Report',
                summary='Observed', request_key='result-with-command', checks=[dict(
                    name='unit tests', outcome='passed', command=['python3', '-m', 'pytest'],
                    tool_call_ref='gateway:' + 'a' * 32)])
    result = _data(_call(project, worker, worker_secret, _init_and('tools/call',
                   dict(name='result_submit', arguments=args)), tmp_path))
    assert result['recorded']
    assert result['checks'][0]['outcome'] == 'unverified'
    assert result['checks'][0]['claimed_outcome'] == 'passed'
    retained = TaskMachine(store).context(task)['nodes'][0]['result']['checks'][0]
    assert 'command' not in retained
    assert retained['evidence_gap']


def test_plan_objective_wire_description_rejection_and_corrected_submission(tmp_path):
    from copy import deepcopy
    from cortex_runtime.delegation.contracts import schemas_for
    from cortex_runtime.delegation.mcp_tools import tool_definitions
    from cortex_runtime.engine.mcp_bridge import always_load
    from cortex_runtime.orchestration.state import TaskMachine

    project, store, task, channel, secret = _bootstrap(tmp_path)
    machine = TaskMachine(store)
    machine.create(task, 'Build a page.')
    tools = _call(project, channel, secret, _init_and('tools/list', {}), tmp_path)[-1]['result']['tools']
    entry = next(t for t in tools if t['name'] == 'plan_submit')
    generated = next(t for t in tool_definitions('coordinator') if t['name'] == 'plan_submit')
    # Both adapters consume this shared channel MCP surface. Claude's loading
    # annotation changes no schema; Codex can render property comments only.
    assert entry['inputSchema'] == generated['inputSchema'] == schemas_for('coordinator')['plan_submit']
    assert always_load(generated)['inputSchema'] == entry['inputSchema']
    assert entry['_meta']['anthropic/alwaysLoad'] is True
    prop = entry['inputSchema']['properties']['plan']['properties']['tasks']['items']['properties']['objective']
    assert (prop['minLength'], prop['maxLength']) == (1, 8000)
    assert '1 to 8000 Unicode code points' in prop['description']
    assert 'UTF-8 bytes or UTF-16 code units' in prop['description']
    assert 'never silently truncate' in prop['description']
    args = dict(plan=dict(tasks=[dict(id='A', role='backend-dev', objective='😀' * 8001,
                                     dependencies=[], scope=['src/**'], model='auto')],
                          verification=[['true']]), request_key='objective-boundary')
    before = machine.context(task)
    with store.connection() as db:
        requests = db.execute('SELECT COUNT(*) FROM runtime_requests').fetchone()[0]
    rejected = _call(project, channel, secret, _init_and('tools/call',
                     dict(name='plan_submit', arguments=args)), tmp_path)[-1]
    diagnostic = json.dumps(rejected)
    assert 'invalid_arguments' in diagnostic and '8001 Unicode characters' in diagnostic
    assert 'arguments.plan.tasks[0].objective' in diagnostic
    assert 'verify its character count' in diagnostic
    assert machine.context(task) == before
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM runtime_requests').fetchone()[0] == requests
        assert db.execute('SELECT COUNT(*) FROM runtime_nodes').fetchone()[0] == 0
        assert db.execute('SELECT COUNT(*) FROM delegation_provisioning').fetchone()[0] == 0
    corrected = deepcopy(args)
    corrected['plan']['tasks'][0]['objective'] = '😀' * 8000
    request = dict(name='plan_submit', arguments=corrected)
    accepted = _data(_call(project, channel, secret, _init_and('tools/call', request), tmp_path))
    assert accepted == _data(_call(project, channel, secret, _init_and('tools/call', request), tmp_path))
    assert machine.context(task)['state'] == 'PLAN_READY'
    assert machine.context(task)['nodes'][0]['objective'] == '😀' * 8000
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM runtime_nodes').fetchone()[0] == 1
        assert db.execute('SELECT COUNT(*) FROM delegation_provisioning').fetchone()[0] == 0
