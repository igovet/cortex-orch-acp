import json
import os
from pathlib import Path
import subprocess
import sys
import copy
import uuid
import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

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
    return store, result


def exchange(tmp_path, messages, channel=None):
    """Run ``messages`` through the real stdio subprocess, channeled if given."""
    messages=copy.deepcopy(messages)
    home=storage_only_home(tmp_path/'host')
    env=dict(os.environ, CODEX_HOME=str(home))
    if channel is not None:
        env.update(CORTEX_CHANNEL_ID=channel.channel_id, CORTEX_CHANNEL_SECRET=channel.secret,
                    CORTEX_CHANNEL_PROJECT_ROOT=str(tmp_path))
    result=subprocess.run([sys.executable,'-B',str(ROOT/'plugins/cortex/scripts/cortex.py')],input='\n'.join(json.dumps(m) for m in messages)+'\n',text=True,capture_output=True,env=env,timeout=30)
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


def test_raw_invalid_and_oversize_frames(tmp_path):
    raw='{"jsonrpc":"2.0","id":1,"method":"ping","method":"PRIVATE"}\n'+'x'*2_000_001+'\n'+json.dumps(dict(jsonrpc='2.0',id=3,method='ping'))+'\n'
    home=storage_only_home(tmp_path/'host')
    result=subprocess.run([sys.executable,'-B',str(ROOT/'plugins/cortex/scripts/cortex.py')],input=raw,text=True,capture_output=True,env=os.environ|{'CODEX_HOME':str(home)},timeout=30)
    rows=[json.loads(line) for line in result.stdout.splitlines()]
    assert len(rows)==3 and rows[-1]['result']=={}
    assert 'PRIVATE' not in result.stdout and result.stderr==''


def test_actionable_schema_errors_never_echo_private_values(tmp_path):
    from cortex_runtime.server import Server
    store, channel = bootstrap(tmp_path)
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    cases=[
        ({},'required field(s)'),
        ({'task_id':'PRIVATE_SENTINEL','mode':'light','rationale':'PRIVATE_SENTINEL','request_key':'k'},'Remove unadvertised fields'),
        ({'mode':'wrong','rationale':'PRIVATE_SENTINEL','request_key':'k'},'minimal, light, full'),
        ({'mode':'light','rationale':123,'request_key':'k'},'JSON string'),
    ]
    for arguments,expected in cases:
        result=server.dispatch('tools/call',dict(name='set_governance',arguments=arguments))
        body=result['content'][0]['text']
        assert result['isError'] and expected in body and 'PRIVATE_SENTINEL' not in body
        assert json.loads(body)['write_state']=='rejected'


def test_preview_budget_has_headroom_and_exact_length_correction(tmp_path):
    from cortex_runtime.server import Server
    from cortex_runtime.contracts import BY_NAME
    schema=BY_NAME['write_report']['inputSchema']['properties']['summary']
    assert schema['maxLength']==320 and '100' in schema['description']
    store, channel = bootstrap(tmp_path)
    server=Server(channel_credentials=(channel.channel_id, channel.secret, str(tmp_path)))
    args=dict(title='Review',summary='x'*321,
              author='reviewer',draft_id='d_000000000000',request_key='preview-check')
    result=server.dispatch('tools/call',dict(name='write_report',arguments=args))
    error=json.loads(result['content'][0]['text'])
    assert result['isError'] and error['field']=='summary'
    assert '321' in error['received'] and '160' in error['correction']
    assert 'x'*321 not in result['content'][0]['text']


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
    call('set_governance',mode='full',rationale='Risk review.',request_key='gov')
    base=dict(title='Report',summary='Evidence',author='worker')
    assert not by_name['create_draft']['annotations']['idempotentHint']
    draft=call('create_draft',template='general')
    path=Path(draft['draft_path']);marker=path.read_text().split('\n\n',1)[0]+'\n\n';path.write_text(marker+'Part one\nPart two')
    final=call('write_report',**base,draft_id=draft['draft_id'],request_key='report-1')
    call('list_reports')
    result=call('read_report',report_id=final['report_id'])
    assert result['markdown'].endswith('Part one\nPart two')
    pipeline=call('create_draft',template='pipeline',request_key='pipeline-draft')
    path=Path(pipeline['draft_path']);marker=path.read_text().split('\n\n',1)[0]+'\n\n';path.write_text(marker+'Current work')
    call('write_report',**base,draft_id=pipeline['draft_id'],request_key='pipeline')


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
