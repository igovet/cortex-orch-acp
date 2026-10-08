"""Installed production wrapper inside the engine jail, with a loopback fake provider; never a paid call.

This validates actual native tool dispatch and its jail boundary, not model
reasoning or ACP client UI. Opt in with CORTEX_PINNED_CODEX_BINARY; no
credentials or production HOME are used.
"""
import pytest
import http.server,json,os,re,subprocess,tempfile,threading,pathlib,time
from cortex_runtime.engine.process import _prepare_codex_runtime_wrapper
from cortex_runtime.engine.external_proxy import Frames
from cortex_runtime.orchestration.sandbox import engine_jail


def _patch(path):
 return f'*** Begin Patch\n*** Add File: {path}\n+written\n*** End Patch'


class Handler(http.server.BaseHTTPRequestHandler):
 calls=0
 inputs=[]
 def log_message(self,*args): pass
 def do_POST(self):
  data=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))));Handler.inputs.append(data.get('input'))
  Handler.calls+=1
  if Handler.calls==1: items=[{'type':'tool_search_call','call_id':'search-one','arguments':{'query':'gateway_read'},'execution':'client'}]
  elif Handler.calls==2: items=[{'type':'function_call','name':'gateway_read','namespace':'mcp__cortex','arguments':'{}','call_id':'allowed-one'},
                               {'type':'custom_tool_call','name':'apply_patch','input':_patch('src/in-scope.txt'),'call_id':'in-scope'},
                               {'type':'custom_tool_call','name':'apply_patch','input':_patch('outside.txt'),'call_id':'outside'}]
  else: items=[]
  body=''.join('event: response.output_item.done\ndata: '+json.dumps({'type':'response.output_item.done','output_index':i,'item':item})+'\n\n' for i,item in enumerate(items))
  body+='event: response.completed\ndata: '+json.dumps({'type':'response.completed','response':{'id':'offline-'+str(Handler.calls),'object':'response','status':'completed','output':items,'usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}})+'\n\n'
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(body.encode())


@pytest.mark.requires_bwrap
def test_installed_wrapper_in_jail_runs_native_patch_only_inside_scope(tmp_path):
    binary=os.environ.get('CORTEX_PINNED_CODEX_BINARY')
    if not binary: pytest.skip('set CORTEX_PINNED_CODEX_BINARY for installed no-paid tool dispatch check')
    Handler.calls=0;Handler.inputs=[]
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
    with tempfile.TemporaryDirectory(prefix='cortex-wrapper-proof-') as d:
     root=pathlib.Path(d);home=root/'home';home.mkdir();project=root/'project';(project/'src').mkdir(parents=True)
     scratch=root/'scratch';scratch.mkdir()
     # HOME is mounted read-only in the jail, so the MCP fixture stays visible.
     mcp=home/'gateway.py'
     mcp.write_text("import json,sys\nfor line in sys.stdin:\n q=json.loads(line);m=q.get('method');i=q.get('id')\n if i is None: continue\n r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}} if m=='initialize' else {'tools':[{'name':'gateway_read','description':'Gateway fixture','inputSchema':{'type':'object','properties':{},'additionalProperties':False}}]} if m=='tools/list' else {'content':[{'type':'text','text':'gateway observation'}]}\n print(json.dumps({'jsonrpc':'2.0','id':i,'result':r}),flush=True)\n")
     codex_home=home/'.codex';codex_home.mkdir()
     environ=dict(HOME=str(home),CODEX_HOME=str(codex_home),XDG_CONFIG_HOME=str(root/'config'),XDG_STATE_HOME=str(home/'state'),PATH='/usr/bin:/bin',PYTHONDONTWRITEBYTECODE='1')
     (codex_home/'config.toml').write_text(f'model_provider="offline"\n[model_providers.offline]\nname="offline"\nbase_url="http://127.0.0.1:{server.server_port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\n')
     wrapper=_prepare_codex_runtime_wrapper(binary,'cn_0123456789ab',environ)
     with engine_jail(project,scratch,[wrapper,'app-server'],['src/**'],env=environ,state_dirs={codex_home:'codex-acp'}) as (command,fds):
      proc=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env={'PATH':'/usr/bin:/bin'},pass_fds=fds,bufsize=0)
     frames=Frames(proc.stdout)
     def send(i,m,p):
      proc.stdin.write((json.dumps(dict(jsonrpc='2.0',id=i,method=m,params=p))+'\n').encode());proc.stdin.flush()
     def response(i):
      while True:
       m=json.loads(frames.read(time.monotonic()+20))
       if m.get('id')==i:return m
     try:
      send(1,'initialize',dict(clientInfo=dict(name='cortex-offline',version='1'),capabilities=dict(experimentalApi=True)));assert 'result' in response(1)
      send(2,'thread/start',dict(model='gpt-5.6-luna',modelProvider='offline',cwd=str(project),sandbox='danger-full-access',approvalPolicy='never',ephemeral=True,config={'mcp_servers.cortex':dict(enabled=True,default_tools_approval_mode='approve',command='/usr/bin/python3',args=['-B',str(mcp)])}))
      r=response(2);assert 'result' in r,r;tid=r['result']['thread']['id']
      send(3,'turn/start',dict(threadId=tid,input=[dict(type='text',text='Offline fixture.')]))
      while True:
       m=json.loads(frames.read(time.monotonic()+30))
       assert not ('id' in m and 'method' in m), m.get('method')
       if m.get('method')=='turn/completed':
        assert m['params']['turn']['status']=='completed'
        break
      assert Handler.calls==3
      # The engine's own patch tool works in the granted scope and nowhere else.
      assert (project/'src'/'in-scope.txt').read_text()=='written\n'
      assert not (project/'outside.txt').exists()
      outputs={item['call_id']:item['output'] for turn in Handler.inputs for item in turn if 'call_id' in item and 'output' in item}
      assert 'gateway observation' in json.dumps(outputs['allowed-one'])
      assert 'Failed to write file' in outputs['outside']
     finally:
      proc.terminate();proc.wait(5);server.shutdown()


class RecordingHandler(http.server.BaseHTTPRequestHandler):
 """Captures each raw Responses-API request body the real App Server sends
 to the model provider, then ends the turn immediately with empty output.
 No reasoning or tool call is scripted -- only the outgoing request matters
 for this check."""
 bodies=[]
 def log_message(self,*args): pass
 def do_POST(self):
  self.bodies.append(json.loads(self.rfile.read(int(self.headers.get('Content-Length','0')))))
  body='event: response.completed\ndata: '+json.dumps({'type':'response.completed','response':{'id':'offline-cap','object':'response','status':'completed','output':[],'usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}})+'\n\n'
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(body.encode())


def _declared_exec_tool_names(body):
 # Codex's "code mode" advertises every nested tool it actually offers as a
 # ``declare const tools: { <name>(...` entry inside the exec tool's own
 # description text (not a separate top-level ``tools`` array entry per
 # name), so this is the real per-turn tool surface rather than an inferred
 # absence from an empty inventory.
 return set(re.findall(r'declare const tools: \{ ([a-zA-Z0-9_]+)\(', json.dumps(body)))


@pytest.mark.requires_bwrap
def test_installed_wrapper_advertises_apply_patch_without_shell_or_view_image_tools(tmp_path):
    """Closes the gap left unverified by r_98c4d5ec4233/r_edaa460fcf2e: proves,
    against the real installed App Server with the exact Cortex-forced config,
    that no shell/view_image tool is actually offered to the model -- not just
    that the feature flags read back false -- while apply_patch is still
    offered, matching the owner's keep-apply_patch decision."""
    binary=os.environ.get('CORTEX_PINNED_CODEX_BINARY')
    if not binary: pytest.skip('set CORTEX_PINNED_CODEX_BINARY for installed no-paid tool-inventory check')
    RecordingHandler.bodies=[]
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),RecordingHandler);threading.Thread(target=server.serve_forever,daemon=True).start()
    with tempfile.TemporaryDirectory(prefix='cortex-tool-inventory-') as d:
     root=pathlib.Path(d);home=root/'home';home.mkdir();project=root/'project';project.mkdir()
     scratch=root/'scratch';scratch.mkdir()
     codex_home=home/'.codex';codex_home.mkdir()
     environ=dict(HOME=str(home),CODEX_HOME=str(codex_home),XDG_CONFIG_HOME=str(root/'config'),XDG_STATE_HOME=str(home/'state'),PATH='/usr/bin:/bin',PYTHONDONTWRITEBYTECODE='1')
     (codex_home/'config.toml').write_text(f'model_provider="offline"\n[model_providers.offline]\nname="offline"\nbase_url="http://127.0.0.1:{server.server_port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\n')
     wrapper=_prepare_codex_runtime_wrapper(binary,'cn_abcdef012345',environ)
     with engine_jail(project,scratch,[wrapper,'app-server'],['src/**'],env=environ,state_dirs={codex_home:'codex-acp'}) as (command,fds):
      proc=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env={'PATH':'/usr/bin:/bin'},pass_fds=fds,bufsize=0)
     frames=Frames(proc.stdout)
     def send(i,m,p):
      proc.stdin.write((json.dumps(dict(jsonrpc='2.0',id=i,method=m,params=p))+'\n').encode());proc.stdin.flush()
     def response(i):
      while True:
       m=json.loads(frames.read(time.monotonic()+20))
       if m.get('id')==i:return m
     try:
      send(1,'initialize',dict(clientInfo=dict(name='cortex-offline',version='1'),capabilities=dict(experimentalApi=True)));assert 'result' in response(1)
      send(2,'thread/start',dict(model='gpt-5.6-luna',modelProvider='offline',cwd=str(project),sandbox='danger-full-access',approvalPolicy='never',ephemeral=True))
      r=response(2);assert 'result' in r,r;tid=r['result']['thread']['id']
      send(3,'turn/start',dict(threadId=tid,input=[dict(type='text',text='Offline fixture.')]))
      while True:
       m=json.loads(frames.read(time.monotonic()+30))
       assert not ('id' in m and 'method' in m), m.get('method')
       if m.get('method')=='turn/completed':
        assert m['params']['turn']['status']=='completed'
        break
      assert RecordingHandler.bodies, 'no request body was observed from the real App Server'
      declared=set()
      for captured in RecordingHandler.bodies:
       declared|=_declared_exec_tool_names(captured)
      assert 'apply_patch' in declared
      assert not (declared & {'shell','exec_command','write_stdin','view_image','local_shell'})
      dump=json.dumps(RecordingHandler.bodies)
      assert '"name": "shell_tool"' not in dump and '"name": "view_image"' not in dump
     finally:
      proc.terminate();proc.wait(5);server.shutdown()
