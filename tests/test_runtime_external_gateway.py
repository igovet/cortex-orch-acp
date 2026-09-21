"""Real subprocess/sandbox checks; no native model or external network."""
import json
import os
from pathlib import Path
import select
import subprocess
import sys

import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.orchestration.state import TaskMachine

SERVER = '''import json,sys
from pathlib import Path
count=0
for line in sys.stdin:
 q=json.loads(line);method=q.get('method')
 if 'id' not in q: continue
 if method=='initialize': r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'test','version':'1'}}
 elif method=='tools/list': r={'tools':[{'name':name,'inputSchema':{'type':'object'}} for name in ['inspect','forbidden']]}
 elif method=='tools/call':
  count+=1
  try: Path('protected').write_text('bad'); blocked=False
  except OSError: blocked=True
  r={'content':[{'type':'text','text':json.dumps({'count':count,'write_blocked':blocked,'private_visible':Path('.cortex/cortex.sqlite3').exists()})}]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':r}),flush=True)
'''


@pytest.fixture
def proxy(tmp_path):
    root=tmp_path/'project';root.mkdir();(root/'protected').write_text('original')
    store=Store(root/'.cortex',project_root=root)
    task=bootstrap_task(store,SourceEvent(origin='external-test',event_id='one'),'Inspect','owner')
    machine=TaskMachine(store);machine.create(task.task_id,'Inspect')
    script=tmp_path/'server.py';script.write_text(SERVER)
    config=dict(name='fixture',command='/usr/bin/python3',args=['-B',str(script)],allowed_tools=['inspect'],read_only_tools=['inspect'],read_paths=[str(script)])
    path=Path(__file__).resolve().parents[1]/'src/cortex_runtime/engine/external_proxy.py'
    def start(overrides=None,home=None):
        env=dict(PATH='/usr/bin:/bin',PYTHONDONTWRITEBYTECODE='1',**({'HOME':str(home)} if home else {}),
            CORTEX_CHANNEL_PROJECT_ROOT=str(root),CORTEX_CHANNEL_ID=task.channel_id,CORTEX_CHANNEL_SECRET=task.secret,
            CORTEX_EXTERNAL_SERVER=json.dumps(dict(config,**(overrides or {}))))
        return subprocess.Popen([sys.executable,'-B',str(path)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,bufsize=0)
    yield store,task,machine,root,start


def call(proc,identifier,method,params=None):
    proc.stdin.write((json.dumps(dict(jsonrpc='2.0',id=identifier,method=method,params=params or {}))+'\n').encode());proc.stdin.flush()
    assert select.select([proc.stdout],[],[],5)[0], 'gateway did not respond'
    line=proc.stdout.readline()
    assert line, proc.stderr.read().decode()
    return json.loads(line)


def close(proc):
    proc.stdin.close()
    try: proc.wait(timeout=3)
    except subprocess.TimeoutExpired: proc.kill();proc.wait(timeout=3)


def test_external_gateway_grants_filter_containment_replay_and_revocation(proxy):
    store,task,machine,root,start=proxy;proc=start()
    try:
        assert 'result' in call(proc,1,'initialize')
        assert [t['name'] for t in call(proc,2,'tools/list')['result']['tools']]==['inspect']
        assert 'error' in call(proc,3,'tools/call',dict(name='forbidden',arguments={}))
        first=call(proc,4,'tools/call',dict(name='inspect',arguments={}))
        payload=json.loads(first['result']['content'][0]['text'])
        assert payload==dict(count=1,write_blocked=True,private_visible=False)
        assert call(proc,4,'tools/call',dict(name='inspect',arguments={}))==first
        assert 'identity_conflict' in call(proc,4,'tools/call',dict(name='inspect',arguments={'changed':True}))['error']['message']
        second=call(proc,5,'tools/call',dict(name='inspect',arguments={}))
        assert json.loads(second['result']['content'][0]['text'])['count']==2
        machine.cancel(task.task_id)
        assert 'error' in call(proc,6,'tools/call',dict(name='inspect',arguments={}))
        assert (root/'protected').read_text()=='original'
    finally: close(proc)


@pytest.mark.parametrize('key',['read_paths','write_paths'])
@pytest.mark.parametrize('grant',['root','parent','private'])
def test_external_path_grant_cannot_overlay_private_masks(proxy,grant,key):
    store,task,machine,root,start=proxy
    paths={'root':root,'parent':root.parent,'private':root/'.cortex'}
    proc=start({key:[str(paths[grant])]}) if key=='write_paths' else start(dict(read_paths=[str(paths[grant])]))
    try:
        proc.wait(timeout=5)
        assert proc.returncode==65
        assert b'refused startup' in proc.stderr.read()
    finally: close(proc)


def test_server_runs_with_owner_home_and_writes_granted_state(proxy, tmp_path):
    """An index keyed by project path must land in the owner's real cache."""
    store,task,machine,root,start=proxy
    home=tmp_path/'home';cache=home/'.cache'/'index';cache.mkdir(parents=True)
    script=tmp_path/'home_server.py'
    script.write_text('''import json,os,sys
from pathlib import Path
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q: continue
 Path(os.environ['HOME'],'.cache','index','db').write_text('indexed')
 result={'content':[{'type':'text','text':json.dumps({'home':os.environ['HOME']})}]}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
''')
    proc=start(dict(args=['-B',str(script)],read_paths=[str(script)],write_paths=[str(cache)]),home=home)
    try:
        response=call(proc,1,'tools/call',dict(name='inspect',arguments={}))
        assert json.loads(response['result']['content'][0]['text'])==dict(home=str(home))
        assert (cache/'db').read_text()=='indexed'
    finally: close(proc)


def test_partial_frame_has_a_real_deadline():
    import time
    from cortex_runtime.engine.external_proxy import Frames
    from cortex_runtime.contracts import StoreError
    reader,writer=os.pipe()
    with os.fdopen(reader,'rb',buffering=0) as stream:
        try:
            os.write(writer,b'{')
            start=time.monotonic()
            with pytest.raises(StoreError,match='external_mcp_timeout'):
                Frames(stream).read(start+.1)
            assert time.monotonic()-start < 1
        finally: os.close(writer)


def test_nonreading_peer_write_has_a_real_deadline():
    import time
    from cortex_runtime.engine.external_proxy import write_frame
    from cortex_runtime.contracts import StoreError
    reader,writer=os.pipe()
    with os.fdopen(writer,'wb',buffering=0) as stream:
        try:
            start=time.monotonic()
            with pytest.raises(StoreError,match='external_mcp_write_timeout'):
                write_frame(stream,b'x'*1000000,start+.1)
            assert time.monotonic()-start < 1
        finally: os.close(reader)


def test_pipes_above_fd_setsize_still_carry_frames():
    """The sandbox pins one descriptor per granted path, so a worker with a
    broad scope hands the proxy child pipes numbered above 1024."""
    import resource,time
    from cortex_runtime.engine.external_proxy import Frames, write_frame
    soft,hard=resource.getrlimit(resource.RLIMIT_NOFILE)
    if hard!=resource.RLIM_INFINITY and hard<1100: pytest.skip('RLIMIT_NOFILE too low')
    resource.setrlimit(resource.RLIMIT_NOFILE,(max(soft,1100),hard))
    reader,writer=os.pipe()
    high_reader,high_writer=os.dup2(reader,1050),os.dup2(writer,1051)
    os.close(reader);os.close(writer)
    try:
        with os.fdopen(high_reader,'rb',buffering=0) as rs, os.fdopen(high_writer,'wb',buffering=0) as ws:
            write_frame(ws,b'{"id":1}\n',time.monotonic()+1)
            assert Frames(rs).read(time.monotonic()+1)==b'{"id":1}\n'
    finally: resource.setrlimit(resource.RLIMIT_NOFILE,(soft,hard))


def test_network_grant_includes_system_resolution_files_and_loopback(proxy, tmp_path):
    import socket
    store,task,machine,root,start=proxy
    listener=socket.socket()
    listener.bind(('127.0.0.1',0));listener.listen()
    script=tmp_path/'network_server.py'
    script.write_text('''import json,sys,socket
from pathlib import Path
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q: continue
 try:
  with socket.create_connection(('127.0.0.1',PORT),timeout=1): pass
  connected=True
 except OSError: connected=False
 result={'content':[{'type':'text','text':json.dumps({'connected':connected,'dns':Path('/etc/resolv.conf').is_file(),'tls':Path('/etc/ssl/certs/ca-certificates.crt').is_file()})}]}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
'''.replace('PORT',str(listener.getsockname()[1])))
    try:
        for granted in (False,True):
            proc=start(dict(args=['-B',str(script)],read_paths=[str(script)],network=granted))
            try:
                response=call(proc,1+int(granted),'tools/call',dict(name='inspect',arguments={}))
                result=json.loads(response['result']['content'][0]['text'])
                assert result['connected'] is granted
                assert result['dns'] is (granted and Path('/etc/resolv.conf').is_file())
                assert result['tls'] is (granted and Path('/etc/ssl/certs/ca-certificates.crt').is_file())
            finally: close(proc)
    finally: listener.close()
