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
        never_usable={'project_write','project_mkdir','project_delete','outside_write','outside_command','command_run'} if role=='coordinator' else set()
        assert set(schemas_for(role))-never_usable<=names
        assert not never_usable & names
        assert not {'delegate','decide','complete_assignment','integrate','set_governance','create_task'} & names
        assert ('plan_submit' in names)==(role=='coordinator')
        assert ('result_submit' in names)==(role=='worker')


def test_plan_wire_is_idempotent_and_does_not_start_a_worker(tmp_path):
    from cortex_runtime.orchestration.state import TaskMachine
    project,store,task,ch,secret=_bootstrap(tmp_path)
    machine=TaskMachine(store);machine.create(task,'Build a page.')
    proposed=dict(tasks=[dict(id='A',role='backend-dev',objective='Implement',dependencies=[],scope=['src/**'])],verification=[['true']])
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
    draft=call('create_draft',template='general',request_key='draft')
    call('draft_write',draft_id=draft['draft_id'],content=draft['required_first_line']+'\n\n# Report\n\nCompleted fixture.\n',request_key='body')
    report=call('write_report',draft_id=draft['draft_id'],title='Report',summary='Observed',author='worker',request_key='publish')
    args=dict(report_ref=report['report_id'],summary='Observed',checks=[],request_key='result')
    result=call('result_submit',**args)
    assert result['recorded']
    assert call('result_submit',**args)==result
    context=TaskMachine(store).context(task)
    assert context['state']=='EXECUTING'
    assert context['nodes'][0]['status']=='completed'
    assert context['nodes'][0]['result']['report_ref']==report['report_id']
