import json
import os
from pathlib import Path
import subprocess
import sys
import copy
import uuid
import pytest
from types import SimpleNamespace

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx

ROOT=Path(__file__).resolve().parents[1]


def storage_only_home(home: Path) -> Path:
    """Keep stdio protocol tests independent of the optional gateway child."""
    cortex = home / 'cortex'
    cortex.mkdir(parents=True, exist_ok=True)
    cortex.chmod(0o700)
    config = cortex / 'config.toml'
    config.write_text('schema_version = 1\n[gateway]\nenabled = false\n')
    config.chmod(0o600)
    return home


def bootstrap(tmp_path, text='Build a greeting.', owner='owner-1'):
    """Bootstrap one task/coordinator channel directly (the real, only,
    channeled identity path -- T033 removed the unchanneled Codex ``_meta``
    wire path this file used to bootstrap ``create_task`` through)."""
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), text, owner)
    from cortex_runtime.orchestration.state import TaskMachine
    TaskMachine(store).create(result.task_id,text)
    return store, result


def exchange(tmp_path, messages, channel=None):
    """Run ``messages`` through the real stdio subprocess, channeled if given."""
    messages=copy.deepcopy(messages)
    home=storage_only_home(tmp_path/'host')
    env=dict(os.environ, CODEX_HOME=str(home))
    if channel is not None:
        env.update(CORTEX_CHANNEL_ID=channel.channel_id, CORTEX_CHANNEL_SECRET=channel.secret,
                    CORTEX_CHANNEL_PROJECT_ROOT=str(tmp_path))
    result=subprocess.run([sys.executable,'-B',str(ROOT/'src/cortex.py')],input='\n'.join(json.dumps(m) for m in messages)+'\n',text=True,capture_output=True,env=env,timeout=30)
    assert result.returncode==0 and result.stderr==''
    return [json.loads(line) for line in result.stdout.splitlines()]


def test_wire_catalogue_and_private_errors(tmp_path):
    store, channel = bootstrap(tmp_path)
    replies=exchange(tmp_path,[
        dict(jsonrpc='2.0',id=1,method='initialize',params=dict(protocolVersion='2025-11-25',capabilities={},clientInfo=dict(name='test',version='1'))),
        dict(jsonrpc='2.0',method='notifications/initialized'),
        dict(jsonrpc='2.0',id=2,method='tools/list'),
        dict(jsonrpc='2.0',id=3,method='tools/call',params=dict(name='write_report',arguments={'secret':'PRIVATE_SENTINEL'})),
        dict(jsonrpc='2.0',id=4,method='tools/call',params=dict(name='removed_tool',arguments={})),
    ],channel=channel)
    assert len(replies)==4 and len(replies[1]['result']['tools'])>=7
    assert replies[2]['result']['isError'] and 'PRIVATE_SENTINEL' not in json.dumps(replies)
    assert replies[3]['error']['code']==-32602


def test_process_restart_preserves_task_and_files(tmp_path):
    store, channel = bootstrap(tmp_path, text='Build a greeting.')
    message=dict(jsonrpc='2.0',id=1,method='tools/call',params=dict(name='list_reports',arguments={}))
    first=exchange(tmp_path,[message],channel=channel)[0]['result']
    assert not first['isError']
    second=exchange(tmp_path,[message],channel=channel)[0]['result']
    assert second==first


def test_markdown_code_and_quotes_survive_wire_storage_and_restart(tmp_path):
    markdown = '# Evidence\n`files` and "quotes" and apostrophes: it\'s literal.\n```js\nconst text = `${not_executed}`;\nconst path = "C:\\\\work";\n```\nРусский текст — 🧪\n'
    store, channel = bootstrap(tmp_path, text='Preserve literal report text.')
    created_draft=exchange(tmp_path,[dict(jsonrpc='2.0',id=2,method='tools/call',params=dict(name='create_draft',arguments=dict(template='general',request_key='literal-draft')))],channel=channel)
    draft_data=created_draft[0]['result']['structuredContent'];draft_path=Path(draft_data['draft_path'])
    marker=draft_path.read_bytes().split(b'\n\n',1)[0]+b'\n\n';draft_path.write_bytes(marker+markdown.encode())
    written=exchange(tmp_path,[dict(jsonrpc='2.0',id=3,method='tools/call',params=dict(name='write_report',arguments=dict(title='Literal text',summary='Code and quotes preserved.',author='reviewer',draft_id=draft_data['draft_id'],request_key='literal-report')))],channel=channel)
    assert not written[0]['result']['isError']
    report_id = written[0]['result']['structuredContent']['report_id']
    read = exchange(tmp_path, [dict(jsonrpc='2.0', id=3, method='tools/call', params=dict(name='read_report', arguments=dict(report_id=report_id)))],channel=channel)[0]['result']
    assert read['structuredContent']['markdown'] == marker.decode()+markdown
    assert json.loads(read['content'][0]['text']) == read['structuredContent']


def test_worker_channel_read_report_persists_exact_execution_receipt(tmp_path):
    store, coordinator = bootstrap(tmp_path, text='Receipt binding.')
    with store.connection() as db:
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (coordinator.task_id,)).fetchone()
        db.execute("BEGIN IMMEDIATE")
        worker_id, worker_secret = issue_channel_tx(
            db, coordinator.task_id, 'worker', owner['epoch'],
            parent_channel_id=coordinator.channel_id, attempt_id='xa_receipt123456',
        )
        db.commit()
        report_id = coordinator.report_id
    worker = SimpleNamespace(channel_id=worker_id, secret=worker_secret)
    read = exchange(
        tmp_path,
        [dict(jsonrpc='2.0', id='read-1', method='tools/call',
              params=dict(name='read_report', arguments=dict(report_id=report_id)))],
        channel=worker,
    )[0]['result']
    assert not read['isError']
    replay = exchange(
        tmp_path,
        [dict(jsonrpc='2.0', id='read-1', method='tools/call',
              params=dict(name='read_report', arguments=dict(report_id=report_id)))],
        channel=worker,
    )[0]['result']
    assert not replay['isError']
    with store.connection() as db:
        receipt = db.execute(
            "SELECT task_id,attempt_id,report_id,request_id,outcome "
            "FROM report_read_observations WHERE task_id=?",
            (coordinator.task_id,),
        ).fetchone()
        count = db.execute(
            "SELECT COUNT(*) FROM report_read_observations "
            "WHERE task_id=? AND attempt_id=? AND report_id=? AND request_id=?",
            (coordinator.task_id, 'xa_receipt123456', report_id, 'read-1'),
        ).fetchone()[0]
    assert tuple(receipt) == (coordinator.task_id, 'xa_receipt123456', report_id, 'read-1', 'success')
    assert count == 1


def test_raw_invalid_and_oversize_frames(tmp_path):
    raw='{"jsonrpc":"2.0","id":1,"method":"ping","method":"PRIVATE"}\n'+'x'*2_000_001+'\n'+json.dumps(dict(jsonrpc='2.0',id=3,method='ping'))+'\n'
    home=storage_only_home(tmp_path/'host')
    result=subprocess.run([sys.executable,'-B',str(ROOT/'src/cortex.py')],input=raw,text=True,capture_output=True,env=os.environ|{'CODEX_HOME':str(home)},timeout=30)
    rows=[json.loads(line) for line in result.stdout.splitlines()]
    assert len(rows)==3 and rows[-1]['result']=={}
    assert 'PRIVATE' not in result.stdout and result.stderr==''


def test_actionable_schema_errors_never_echo_private_values(tmp_path):
    from cortex_runtime.server import Server
    store, channel = bootstrap(tmp_path)
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    cases=[
        ({},'required field(s)'),
        ({'private':'PRIVATE_SENTINEL','template':'general'},'Remove unadvertised fields'),
        ({'template':123},'JSON string'),
    ]
    for arguments,expected in cases:
        result=server.dispatch('tools/call',dict(name='create_draft',arguments=arguments))
        body=result['content'][0]['text']
        assert result['isError'] and expected in body and 'PRIVATE_SENTINEL' not in body
        assert json.loads(body)['write_state']=='rejected'


def test_preview_budget_has_headroom_and_exact_length_correction(tmp_path):
    from cortex_runtime.server import Server
    from cortex_runtime.contracts import BY_NAME
    schema=BY_NAME['write_report']['inputSchema']['properties']['summary']
    assert schema['maxLength']==320 and '320' in schema['description']
    store, channel = bootstrap(tmp_path)
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    args=dict(title='Review',summary='x'*321,
              author='reviewer',draft_id='d_000000000000',request_key='preview-check')
    from cortex_runtime.contracts import clip_summary_preview, validate
    clipped=clip_summary_preview('write_report',args)['summary']
    # An overlong preview is clipped visibly instead of costing a retry.
    assert len(clipped)==320 and clipped.endswith('full text in the report]')
    validate('write_report',clip_summary_preview('write_report',args))
    result=server.dispatch('tools/call',dict(name='write_report',arguments=args))
    error=json.loads(result['content'][0]['text'])
    assert error.get('field')!='summary'


def test_draft_routes_are_schema_visible_and_errors_explain_the_exact_repair(tmp_path):
    from jsonschema import Draft202012Validator, ValidationError
    from cortex_runtime.server import Server
    from cortex_runtime.contracts import BY_NAME
    schema=BY_NAME['write_report']['inputSchema'];validator=Draft202012Validator(schema)
    base=dict(title='Evidence',summary='One useful sentence.',author='worker',draft_id='d_000000000000',request_key='route')
    validator.validate(base)
    with pytest.raises(ValidationError):validator.validate({key:value for key,value in base.items() if key!='draft_id'})
    store, channel = bootstrap(tmp_path)
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    for arguments,field,repair in [
        (base|dict(markdown='report body'),'markdown','Remove unadvertised fields'),
        (base|dict(draft_id=''),'draft_id','server-issued identifier'),
    ]:
        result=server.dispatch('tools/call',dict(name='write_report',arguments=arguments))
        body=json.loads(result['content'][0]['text'])
        assert result['isError'] and body['field']==field and repair in body['correction']


def test_passive_context_observation_never_logs_other_metadata(tmp_path,monkeypatch):
    """T033: the observation row no longer derives thread/parent from a Codex
    ``_meta`` turn-metadata object (that legacy path was removed); it now
    only ever records the safe, value-free operation fields."""
    from cortex_runtime.server import observe
    folder=tmp_path/'observations'
    monkeypatch.setenv('CORTEX_OBSERVATION_DIR',str(folder))
    observe('list_reports','success',arguments={'cursor':'PRIVATE_SENTINEL'})
    observe('read_report','success',
        arguments={'report_id':'r_0123456789ab','cursor':'PRIVATE_SENTINEL'},
        result={'report_id':'r_0123456789ab','kind':'pipeline'})
    raw=next(folder.glob('*.jsonl')).read_text()
    rows=[json.loads(line) for line in raw.splitlines()]
    assert rows[0]['page']=='continuation'
    assert rows[1]['document_kind']=='pipeline' and rows[1]['report_id']=='r_0123456789ab'
    assert rows[1]['page']=='continuation'
    assert 'PRIVATE_SENTINEL' not in raw
    assert 'thread_id' not in rows[0] and 'parent_thread_id' not in rows[0]


def test_wire_schemas_match_every_success(tmp_path):
    from jsonschema import Draft202012Validator
    from cortex_runtime.server import Server
    store, channel = bootstrap(tmp_path, text='Точный запрос')
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    catalogue=server.dispatch('tools/list',{})['tools']
    by_name={t['name']:t for t in catalogue}
    for item in catalogue:
        Draft202012Validator.check_schema(item['inputSchema'])
        if 'outputSchema' in item:
            Draft202012Validator.check_schema(item['outputSchema'])
        assert item['title'] and item['description']
        assert all(rule['description'] for rule in item['inputSchema']['properties'].values())
    def call(name,**arguments):
        Draft202012Validator(by_name[name]['inputSchema']).validate(arguments)
        result=server.dispatch('tools/call',dict(name=name,arguments=arguments))
        assert not result['isError'],result
        data=result['structuredContent']
        assert json.loads(result['content'][0]['text'])==data
        Draft202012Validator(by_name[name]['outputSchema']).validate(data)
        return data
    base=dict(title='Report',summary='Evidence',author='worker')
    assert not by_name['create_draft']['annotations']['idempotentHint']
    draft=call('create_draft',template='general')
    path=Path(draft['draft_path']);marker=path.read_text().split('\n\n',1)[0]+'\n\n';path.write_text(marker+'Part one\nPart two')
    final=call('write_report',**base,draft_id=draft['draft_id'],request_key='report-1')
    call('list_reports')
    result=call('read_report',report_id=final['report_id'])
    assert result['markdown'].endswith('Part one\nPart two')
    rejected=server.dispatch('tools/call',dict(name='create_draft',arguments=dict(template='pipeline',request_key='pipeline-draft')))
    assert rejected['isError']
    assert 'runtime_owned_operation' in rejected['content'][0]['text']


def test_protocol_revision_and_error_categories(tmp_path):
    store, channel = bootstrap(tmp_path)
    replies=exchange(tmp_path,[
        dict(jsonrpc='2.0',id=1,method='initialize',params=dict(protocolVersion='2025-11-25',capabilities={},clientInfo=dict(name='test',version='1'))),
        dict(jsonrpc='2.0',id=2,method='initialize',params=dict(protocolVersion='unsupported',capabilities={},clientInfo=dict(name='test',version='1'))),
        dict(jsonrpc='2.0',id=3,method='tools/call',params=dict(name='read_report',arguments=[])),
        dict(jsonrpc='2.0',id=4,method='unknown-method'),
        dict(jsonrpc='2.0',id=5,method='initialize',params={}),
    ],channel=channel)
    assert replies[0]['result']['protocolVersion']=='2025-11-25'
    assert replies[1]['result']['protocolVersion']=='2025-11-25'
    assert replies[2]['error']['code']==-32602 and 'arguments as an object' in replies[2]['error']['message']
    assert replies[3]['error']['code']==-32601
    assert replies[4]['error']['code']==-32602 and 'clientInfo' in replies[4]['error']['message']


def test_list_route_options_is_advertised_and_dispatches_through_the_real_channeled_path(tmp_path, tmp_path_factory, monkeypatch):
    """Regression for `list_route_options` returning `unknown_operation`: the
    advertised coordinator tool must reach `routing.auto_route.route_menu`
    through the real ``tools/list``/``tools/call`` wire path, and a worker
    channel (never advertised the tool) must be refused it too."""
    from cortex_runtime.server import Server
    monkeypatch.setenv('XDG_CONFIG_HOME', str(tmp_path_factory.mktemp('xdg-config')))
    store, coordinator = bootstrap(tmp_path)
    (tmp_path / '.cortex' / 'routing.json').write_text(json.dumps({'model_policy_version': 2}))
    server = Server(channel_credentials=(coordinator.channel_id, coordinator.secret, str(tmp_path)))
    catalogue = server.dispatch('tools/list', {})['tools']
    by_name = {t['name']: t for t in catalogue}
    assert by_name['list_route_options']['inputSchema']['properties'] == {}
    assert by_name['list_route_options']['annotations']['readOnlyHint'] is True
    result = server.dispatch('tools/call', dict(name='list_route_options', arguments={}))
    assert not result['isError'], result
    data = result['structuredContent']
    assert json.loads(result['content'][0]['text']) == data
    assert set(data['tiers']) == {'flagship', 'deep', 'standard', 'light'}
    assert 'roles' in data

    with store.connection() as db:
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (coordinator.task_id,)).fetchone()
        db.execute("BEGIN IMMEDIATE")
        worker_id, worker_secret = issue_channel_tx(
            db, coordinator.task_id, 'worker', owner['epoch'],
            parent_channel_id=coordinator.channel_id, attempt_id='xa_routeopts01',
        )
        db.commit()
    worker_server = Server(channel_credentials=(worker_id, worker_secret, str(tmp_path)))
    worker_catalogue = worker_server.dispatch('tools/list', {})['tools']
    assert 'list_route_options' not in {t['name'] for t in worker_catalogue}
    from cortex_runtime.server import ProtocolError
    with pytest.raises(ProtocolError):
        worker_server.dispatch('tools/call', dict(name='list_route_options', arguments={}))


@pytest.mark.parametrize("policy,code", [
    (None, "model_policy_migration_required"),
    ("{}", "model_policy_migration_required"),
    ('{"model_policy_version": 3}', "model_policy_version_unsupported"),
    ('{"model_policy_version": 2, "enabled": []}', "invalid_routing_overrides"),
    ("{", "invalid_routing_overrides"),
])
def test_route_menu_policy_diagnostic_survives_tools_call(tmp_path, monkeypatch, policy, code):
    from cortex_runtime.server import Server
    from cortex_runtime.routing import auto_route

    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.delenv("CORTEX_ROUTING", raising=False)
    store, coordinator = bootstrap(tmp_path)
    path = tmp_path / ".cortex" / "routing.json"
    if policy is not None:
        path.write_text(policy)
    monkeypatch.setattr(auto_route, "_entries", lambda *_: pytest.fail("invalid policy reached catalogue"))
    server = Server(channel_credentials=(coordinator.channel_id, coordinator.secret, str(tmp_path)))
    result = server.dispatch("tools/call", dict(name="list_route_options", arguments={}))
    assert result["isError"]
    data = json.loads(result["content"][0]["text"])
    assert data["error"] == code
    assert data["correction"]
    assert data["field"] == ("routing" if code == "invalid_routing_overrides" else "model_policy_version")
    assert data["write_state"] == "rejected" and "storage_error" not in result["content"][0]["text"]
    wire = exchange(tmp_path, [dict(jsonrpc="2.0", id="policy-read", method="tools/call",
                                   params=dict(name="list_route_options", arguments={}))],
                    channel=coordinator)[0]["result"]
    assert wire == result  # The real stdio child preserves the same public diagnostic.
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM configuration_revisions").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM attempt_route_audit").fetchone()[0] == 0
    assert path.exists() == (policy is not None)
    if policy is not None:
        assert path.read_text() == policy
