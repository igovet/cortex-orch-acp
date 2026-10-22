"""G4 (arguments_sha256 on EXTERNAL_TOOL_AUTHORIZED) and G2-external (denied
tool / transport error observability) checks for the external MCP gateway.

Real subprocess/sandbox; no native model or external network. Mirrors the
``proxy`` fixture in test_runtime_external_gateway.py.
"""
import hashlib
import io
import json
from pathlib import Path
import select
import subprocess
import sys
import types

import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.observability.events import list_events
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.orchestration.state import TaskMachine, encoded

SERVER = '''import json,sys
for line in sys.stdin:
 q=json.loads(line);method=q.get('method')
 if 'id' not in q: continue
 if method=='initialize': r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'test','version':'1'}}
 elif method=='tools/list': r={'tools':[{'name':'inspect','inputSchema':{'type':'object'}}]}
 elif method=='tools/call': r={'content':[{'type':'text','text':'ok'}]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':r}),flush=True)
'''


@pytest.fixture
def proxy(tmp_path):
    root = tmp_path / 'project'; root.mkdir()
    store = Store(root / '.cortex', project_root=root)
    task = bootstrap_task(store, SourceEvent(origin='external-signals-test', event_id='one'), 'Inspect', 'owner')
    machine = TaskMachine(store); machine.create(task.task_id, 'Inspect')
    script = tmp_path / 'server.py'; script.write_text(SERVER)
    config = dict(name='fixture', command='/usr/bin/python3', args=['-B', str(script)],
                  allowed_tools=['inspect'], read_only_tools=['inspect'], read_paths=[str(script)])
    path = Path(__file__).resolve().parents[1] / 'src/cortex_runtime/engine/external_proxy.py'

    def start():
        env = dict(PATH='/usr/bin:/bin', PYTHONDONTWRITEBYTECODE='1',
                   CORTEX_CHANNEL_PROJECT_ROOT=str(root), CORTEX_CHANNEL_ID=task.channel_id,
                   CORTEX_CHANNEL_SECRET=task.secret, CORTEX_EXTERNAL_SERVER=json.dumps(config))
        return subprocess.Popen([sys.executable, '-B', str(path)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, bufsize=0)
    yield store, task, start


def call(proc, identifier, method, params=None):
    proc.stdin.write((json.dumps(dict(jsonrpc='2.0', id=identifier, method=method, params=params or {})) + '\n').encode())
    proc.stdin.flush()
    assert select.select([proc.stdout], [], [], 5)[0], 'gateway did not respond'
    line = proc.stdout.readline()
    assert line, proc.stderr.read().decode()
    return json.loads(line)


def close(proc):
    proc.stdin.close()
    try: proc.wait(timeout=3)
    except subprocess.TimeoutExpired: proc.kill(); proc.wait(timeout=3)


def _runtime_events(store, task_id, kind):
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id",
            (task_id, kind),
        ).fetchall()
    return [json.loads(row[0]) for row in rows]


@pytest.mark.requires_bwrap
def test_same_arguments_different_meta_same_arguments_sha256(proxy):
    store, task, start = proxy; proc = start()
    try:
        assert 'result' in call(proc, 1, 'initialize')
        first = call(proc, 2, 'tools/call', dict(name='inspect', arguments={'path': 'a'}, _meta=dict(tag='one')))
        assert 'result' in first
        second = call(proc, 3, 'tools/call', dict(name='inspect', arguments={'path': 'a'}, _meta=dict(tag='two')))
        assert 'result' in second
        authorized = _runtime_events(store, task.task_id, 'EXTERNAL_TOOL_AUTHORIZED')
        assert len(authorized) == 2
        # Same tool+arguments, different _meta: arguments_sha256 is stable.
        assert authorized[0]['arguments_sha256'] == authorized[1]['arguments_sha256']
        # Each call still has its own identity: different request ids, and
        # the unchanged runtime_requests digest (which still includes _meta)
        # differs between them -- G4 leaves that digest untouched.
        assert authorized[0]['request_id'] != authorized[1]['request_id']
        with store.connection() as db:
            digests = [row[0] for row in db.execute(
                "SELECT digest FROM runtime_requests WHERE task_id=? AND operation='external_call' ORDER BY rowid",
                (task.task_id,),
            )]
        assert len(digests) == 2 and digests[0] != digests[1]
    finally: close(proc)


@pytest.mark.requires_bwrap
def test_denied_tool_records_one_refusal_row_without_arguments(proxy):
    store, task, start = proxy; proc = start()
    try:
        assert 'result' in call(proc, 1, 'initialize')
        response = call(proc, 2, 'tools/call', dict(name='forbidden', arguments={'secret': 'value'}))
        assert 'error' in response
        events = list_events(store, task.task_id, category='external_refusal')
        assert len(events) == 1
        assert events[0]['outcome'] == 'external_mcp_tool_denied'
        assert events[0]['basis'] == 'observed'
        assert events[0]['detail'] == dict(server='fixture', tool='forbidden', error='external_mcp_tool_denied')
        assert 'arguments' not in events[0]['detail']
        assert 'secret' not in json.dumps(events[0])
    finally: close(proc)


@pytest.mark.requires_bwrap
def test_denied_method_records_one_transport_error_row(proxy):
    store, task, start = proxy; proc = start()
    try:
        assert 'result' in call(proc, 1, 'initialize')
        # Startup discovery listings are answered empty, never logged as denials.
        assert call(proc, 3, 'resources/list')['result'] == {'resources': []}
        assert call(proc, 4, 'prompts/list')['result'] == {'prompts': []}
        assert list_events(store, task.task_id, category='external_error') == []
        response = call(proc, 2, 'resources/read')
        assert 'error' in response
        events = list_events(store, task.task_id, category='external_error')
        assert len(events) == 1
        assert events[0]['outcome'] == 'external_mcp_method_denied'
        assert events[0]['detail'] == dict(server='fixture', tool=None, error='external_mcp_method_denied')
        assert 'arguments' not in events[0]['detail'] and 'result' not in events[0]['detail']
    finally: close(proc)


# Unjailed (no bwrap) checks of the same G4 / G2-external behavior. The real
# external_proxy.main() runs in this process against the real store with a
# fake MCP child. Only the final OS launch is replaced, through the existing
# unjailed_gateway_command opt-in; containment is not asserted here, and the
# bwrap tests above remain the containment evidence.

SERVER_WRONG_ID = '''import json,sys
for line in sys.stdin:
 q=json.loads(line);method=q.get('method')
 if 'id' not in q: continue
 if method=='initialize': print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':{'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'test','version':'1'}}}),flush=True)
 else: print(json.dumps({'jsonrpc':'2.0','id':'not-the-request','result':{'content':[{'type':'text','text':'ok'}]}}),flush=True)
'''


def _sha(value):
    """The same canonical encoding the gateway hashes with."""
    return hashlib.sha256(encoded(value).encode()).hexdigest()


def _request(identifier, method, params=None):
    return dict(jsonrpc='2.0', id=identifier, method=method, params=params or {})


def _inproc_setup(tmp_path, server_source=SERVER):
    root = tmp_path / 'project'; root.mkdir()
    store = Store(root / '.cortex', project_root=root)
    task = bootstrap_task(store, SourceEvent(origin='external-signals-inproc', event_id='one'), 'Inspect', 'owner')
    TaskMachine(store).create(task.task_id, 'Inspect')
    script = tmp_path / 'server.py'; script.write_text(server_source)
    config = dict(name='fixture', command='/usr/bin/python3', args=['-B', str(script)],
                  allowed_tools=['inspect'], read_only_tools=['inspect'], read_paths=[str(script)])
    return store, task, root, config


@pytest.fixture
def inproc(tmp_path, monkeypatch, capsys, unjailed_gateway_command):
    from cortex_runtime.engine import external_proxy
    home = tmp_path / 'home'; home.mkdir()
    monkeypatch.setenv('HOME', str(home))

    def drive(root, task, config, requests):
        monkeypatch.setenv('CORTEX_CHANNEL_PROJECT_ROOT', str(root))
        monkeypatch.setenv('CORTEX_CHANNEL_ID', task.channel_id)
        monkeypatch.setenv('CORTEX_CHANNEL_SECRET', task.secret)
        monkeypatch.setenv('CORTEX_EXTERNAL_SERVER', json.dumps(config))
        lines = b''.join(json.dumps(request).encode() + b'\n' for request in requests)
        monkeypatch.setattr(sys, 'stdin', types.SimpleNamespace(buffer=io.BytesIO(lines)))
        external_proxy.main()
        return [json.loads(line) for line in capsys.readouterr().out.splitlines() if line.strip()]
    return drive


def test_unjailed_arguments_sha256_ignores_meta_and_follows_arguments(inproc, tmp_path):
    store, task, root, config = _inproc_setup(tmp_path)
    one = dict(name='inspect', arguments={'path': 'a'}, _meta=dict(tag='one'))
    two = dict(name='inspect', arguments={'path': 'a'}, _meta=dict(tag='two'))
    changed = dict(name='inspect', arguments={'path': 'b'}, _meta=dict(tag='one'))
    replies = inproc(root, task, config, [
        _request(1, 'initialize'),
        _request(2, 'tools/call', one),
        _request(3, 'tools/call', two),
        _request(4, 'tools/call', changed),
    ])
    assert len(replies) == 4 and all('result' in reply for reply in replies)
    authorized = _runtime_events(store, task.task_id, 'EXTERNAL_TOOL_AUTHORIZED')
    assert [event['request_id'] for event in authorized] == [2, 3, 4]
    # Identical {tool, arguments} with different _meta: one arguments identity.
    assert authorized[0]['arguments_sha256'] == authorized[1]['arguments_sha256']
    assert authorized[0]['arguments_sha256'] == _sha(dict(name='inspect', arguments={'path': 'a'}))
    # Changed arguments: a different identity.
    assert authorized[2]['arguments_sha256'] == _sha(dict(name='inspect', arguments={'path': 'b'}))
    assert authorized[0]['arguments_sha256'] != authorized[2]['arguments_sha256']
    # The receipt digest is unchanged: it still covers the whole params, _meta included.
    with store.connection() as db:
        digests = [row[0] for row in db.execute(
            "SELECT digest FROM runtime_requests WHERE task_id=? AND operation='external_call' ORDER BY rowid",
            (task.task_id,),
        )]
    assert digests == [_sha(one), _sha(two), _sha(changed)]
    assert digests[0] != digests[1]


def test_unjailed_denied_tool_appends_one_refusal_row_without_arguments(inproc, tmp_path):
    store, task, root, config = _inproc_setup(tmp_path)
    replies = inproc(root, task, config, [
        _request(1, 'initialize'),
        _request(2, 'tools/call', dict(name='forbidden', arguments={'secret': 'value'})),
    ])
    assert 'result' in replies[0]
    assert 'external_mcp_tool_denied' in replies[1]['error']['message']
    events = list_events(store, task.task_id, category='external_refusal')
    assert len(events) == 1
    assert events[0]['outcome'] == 'external_mcp_tool_denied'
    assert events[0]['basis'] == 'observed'
    assert events[0]['detail'] == dict(server='fixture', tool='forbidden', error='external_mcp_tool_denied')
    assert 'secret' not in json.dumps(events[0])
    assert 'value' not in json.dumps(events[0]['detail'])
    # Denied before any authorization row or external_error row is written.
    assert _runtime_events(store, task.task_id, 'EXTERNAL_TOOL_AUTHORIZED') == []
    assert list_events(store, task.task_id, category='external_error') == []


def test_unjailed_transport_failure_appends_one_error_row_without_arguments(inproc, tmp_path):
    store, task, root, config = _inproc_setup(tmp_path, SERVER_WRONG_ID)
    replies = inproc(root, task, config, [
        _request(1, 'initialize'),
        _request(2, 'tools/call', dict(name='inspect', arguments={'secret': 'value'})),
    ])
    assert 'result' in replies[0]
    assert 'external_mcp_response_identity_mismatch' in replies[1]['error']['message']
    events = list_events(store, task.task_id, category='external_error')
    assert len(events) == 1
    assert events[0]['outcome'] == 'external_mcp_response_identity_mismatch'
    assert events[0]['basis'] == 'observed'
    assert events[0]['detail'] == dict(server='fixture', tool='inspect', error='external_mcp_response_identity_mismatch')
    assert 'secret' not in json.dumps(events[0])
    assert 'value' not in json.dumps(events[0]['detail'])
    assert list_events(store, task.task_id, category='external_refusal') == []
