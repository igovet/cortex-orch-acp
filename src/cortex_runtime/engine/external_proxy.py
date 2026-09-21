"""Runtime-owned stdio MCP gateway. Never give engines a raw external server.

The private broker supplies channel identity and explicit administrator grants.
Every forwarded call is checked and retained; the child is OS-contained and is
terminated on channel revocation. No MCP annotation grants a capability.
"""
import json
import hashlib
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0,str(Path(__file__).resolve().parents[2]))
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.orchestration.gateway import _authority, PRIVATE, READ_ONLY
from cortex_runtime.orchestration.sandbox import gateway_command
from cortex_runtime.orchestration.state import event, encoded
from cortex_runtime.contracts import StoreError


def _ready(fd, event, timeout):
    """poll, not select: the sandbox pins one descriptor per granted path, so
    the child's pipes routinely sit above select()'s FD_SETSIZE of 1024."""
    poller = select.poll()
    poller.register(fd, event)
    return bool(poller.poll(max(0, timeout)*1000))


class Frames:
    """Bound each full JSON line, including a peer that writes only one byte."""
    def __init__(self, stream):
        self.stream = stream
        self.buffer = b''

    def read(self, deadline):
        if time.monotonic() >= deadline:
            raise StoreError('external_mcp_timeout')
        while b'\n' not in self.buffer:
            remaining = deadline-time.monotonic()
            if remaining <= 0 or not _ready(self.stream.fileno(), select.POLLIN, remaining):
                raise StoreError('external_mcp_timeout')
            chunk = os.read(self.stream.fileno(), 65536)
            if not chunk:
                raise StoreError('external_mcp_closed')
            self.buffer += chunk
            if len(self.buffer) > 64*1024*1024:
                raise StoreError('external_mcp_frame_too_large')
        frame, self.buffer = self.buffer.split(b'\n', 1)
        return frame+b'\n'


def write_frame(stream, data, deadline):
    """A non-reading peer cannot hold the authorization lock indefinitely."""
    fd = stream.fileno()
    os.set_blocking(fd, False)
    view = memoryview(data)
    while view:
        remaining = deadline-time.monotonic()
        if remaining <= 0 or not _ready(fd, select.POLLOUT, remaining):
            raise StoreError('external_mcp_write_timeout')
        try:
            count = os.write(fd, view)
        except BlockingIOError:
            continue
        view = view[count:]


def main():
    config=json.loads(os.environ.pop('CORTEX_EXTERNAL_SERVER'))
    root=os.environ.pop('CORTEX_CHANNEL_PROJECT_ROOT')
    channel_id=os.environ.pop('CORTEX_CHANNEL_ID');secret=os.environ.pop('CORTEX_CHANNEL_SECRET')
    store=Store(Path(root)/'.cortex',initialize=False,project_root=root)
    channel=resolve_channel(store,channel_id,secret)
    allowed=config.get('allowed_tools')
    if not isinstance(allowed,list) or not allowed or any(not isinstance(n,str) or not n for n in allowed):
        raise StoreError('external_mcp_explicit_tool_grants_required')
    with store.connection() as db:
        scopes,owner=_authority(db,channel)
        role=db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',(channel.attempt_id,)).fetchone()
        if channel.role=='coordinator' or role and role[0] in READ_ONLY:
            allowed=[tool for tool in allowed if tool in config.get('read_only_tools',[])]
    with tempfile.TemporaryDirectory(prefix='cortex-external-') as scratch:
        with gateway_command(root,scratch,[config['command'],*config.get('args',[])],scopes) as (command,fds):
            # Administrator-owned registry values are the only extra grants.
            index=command.index('--chdir')
            grants=[]
            project=Path(root).resolve()
            for key,bind,code in (('read_paths','--ro-bind','external_mcp_read_grant_invalid'),
                                  ('write_paths','--bind','external_mcp_write_grant_invalid')):
                for value in config.get(key,[]):
                    path=Path(value)
                    if (not path.is_absolute() or not path.exists() or PRIVATE.intersection(path.parts)
                        or any(parent.is_symlink() for parent in (path,*path.parents))
                        or path==project or path in project.parents or project in path.parents):
                        raise StoreError(code)
                    grants+=[bind,str(path),str(path)]
            # The owner's real HOME, never the sandbox scratch: a server's
            # per-user state (an index keyed by project path) must be the one
            # the owner already has. Only granted paths under it are mounted.
            if os.environ.get('HOME','').startswith('/'):
                grants+=['--setenv','HOME',os.environ['HOME']]
            for item in config.get('env',[]):
                grants+=['--setenv',item['name'],item['value']]
            command[index:index]=grants
            if config.get('network') is True:
                command.remove('--unshare-net')
                # Sharing the network namespace alone leaves DNS and TLS broken
                # in the otherwise empty /etc. These are public system files,
                # never a mount of the user's home or the entire /etc tree.
                network_files=[]
                for name in ('/etc/resolv.conf','/etc/hosts','/etc/nsswitch.conf',
                             '/etc/gai.conf','/etc/ssl/certs/ca-certificates.crt'):
                    path=Path(name)
                    if path.is_file():
                        network_files+=['--ro-bind',str(path.resolve(strict=True)),name]
                command[command.index('--chdir'):command.index('--chdir')]=network_files
            child=subprocess.Popen(command,pass_fds=fds,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL,bufsize=0,env={'PATH':'/usr/bin:/bin'})
            stopped=threading.Event()
            frames=Frames(child.stdout)
            def monitor():
                while not stopped.wait(.05) and child.poll() is None:
                    try:
                        with store.connection() as db: _authority(db,channel)
                    except Exception:
                        child.kill();return
            watcher=threading.Thread(target=monitor,daemon=True);watcher.start()
            try:
                for line in sys.stdin.buffer:
                    request=json.loads(line)
                    method=request.get('method');identifier=request.get('id')
                    try:
                        with store.connection() as db: _authority(db,channel)
                        if method not in {'initialize','notifications/initialized','ping','tools/list','tools/call','notifications/cancelled'}:
                            raise StoreError('external_mcp_method_denied')
                        tool=request.get('params',{}).get('name')
                        if method=='tools/call':
                            receipt_key='external:'+config['name']+':'+str(identifier)
                            digest=hashlib.sha256(encoded(request.get('params',{})).encode()).hexdigest()
                            if tool not in allowed: raise StoreError('external_mcp_tool_denied')
                            with store.connection() as db:
                                db.execute('BEGIN IMMEDIATE')
                                _authority(db,channel)
                                previous=db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                                    (channel.task_id,channel.channel_id,receipt_key)).fetchone()
                                if previous:
                                    if previous['operation']!='external_call' or previous['digest']!=digest:
                                        raise StoreError('external_mcp_request_identity_conflict')
                                    saved=json.loads(previous['result_json'])
                                    if not saved.get('complete'): raise StoreError('external_mcp_action_outcome_unknown')
                                    print(json.dumps(saved['receipt']),flush=True)
                                    continue
                                db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                                           (channel.task_id,receipt_key,channel.channel_id,'external_call',digest,encoded(dict(complete=False))))
                                event(db,channel.task_id,'EXTERNAL_TOOL_AUTHORIZED',dict(server=config['name'],tool=tool,request_id=identifier),channel.channel_id)
                                db.commit()
                        with store.connection() as db:
                            db.execute('BEGIN IMMEDIATE')
                            _authority(db,channel)
                            write_frame(child.stdin,line,time.monotonic()+5)
                            db.commit()
                        if identifier is None: continue
                        deadline=time.monotonic()+300
                        while True:
                            frame=frames.read(deadline)
                            reply=json.loads(frame)
                            # Server-initiated sampling/elicitation cannot bypass runtime.
                            if reply.get('method'):
                                if 'id' in reply:
                                    write_frame(child.stdin,(json.dumps(dict(jsonrpc='2.0',id=reply['id'],error=dict(code=-32601,message='Runtime gateway does not grant this method')))+'\n').encode(), min(deadline,time.monotonic()+5))
                                else:
                                    sys.stdout.buffer.write(frame);sys.stdout.buffer.flush()
                                continue
                            if reply.get('id')!=identifier: raise StoreError('external_mcp_response_identity_mismatch')
                            if method=='tools/list' and 'result' in reply:
                                reply['result']['tools']=[t for t in reply['result'].get('tools',[]) if t.get('name') in allowed]
                            if method=='tools/call':
                                with store.connection() as db:
                                    db.execute('BEGIN IMMEDIATE')
                                    db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                                               (encoded(dict(complete=True,receipt=reply)),channel.task_id,channel.channel_id,receipt_key))
                                    event(db,channel.task_id,'EXTERNAL_TOOL_RESULT',dict(server=config['name'],tool=tool,request_id=identifier,result=reply),channel.channel_id)
                                    db.commit()
                            print(json.dumps(reply,ensure_ascii=False),flush=True)
                            break
                    except (StoreError,OSError,ValueError) as error:
                        if identifier is not None:
                            print(json.dumps(dict(jsonrpc='2.0',id=identifier,error=dict(code=-32001,message=str(error) if isinstance(error,StoreError) else 'external_mcp_transport_failure'))),flush=True)
                        if isinstance(error, OSError) or str(error) in {'external_mcp_timeout','external_mcp_write_timeout','external_mcp_frame_too_large','external_mcp_response_identity_mismatch'}:
                            if child.poll() is None: child.kill()
                            break
                        if child.poll() is not None: break
            finally:
                stopped.set()
                if child.poll() is None: child.kill()
                child.wait(timeout=5)


if __name__=='__main__':
    try: main()
    except Exception:
        print('cortex external MCP gateway refused startup',file=sys.stderr)
        raise SystemExit(65)
