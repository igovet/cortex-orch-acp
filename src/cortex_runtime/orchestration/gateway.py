"""Channel-bound action gateway. Native engines receive no filesystem tools.

Authorization comes from the current durable assignment, never tool arguments.
File operations use no-follow directory descriptors. Commands get a read-only
project with only explicitly granted directories/files mounted writable.
"""
from contextlib import contextmanager
import fnmatch
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import tempfile
import time
import uuid

from ..contracts import StoreError
from ..ownership import current_owner_epoch
from .contracts import obj, text, array, validate
from .state import encoded, event, TERMINAL

PRIVATE = {'.cortex', '.git', '.codex', '.claude'}
READ_ONLY = {'worker-architect', 'worker-ux-designer', 'worker-code-reviewer', 'worker-senior-consultant', 'senior-consultant',
             'worker-explorer', 'worker-planner', 'worker-security-auditor', 'worker-accessibility-auditor'}


def integer(description, maximum):
    return dict(type='integer', description=description, minimum=0, maximum=maximum)


SCHEMAS = {
    'project_git': obj(dict(kind=dict(type='string',enum=['status','diff'],description='Read-only project status or complete working-tree diff against HEAD, including pre-existing edits. Returns an exact retained output for read_large_output. Untracked file content is read through project_read.'),request_key=text('Unique observation identity; retries return the same retained snapshot.',128))),
    'project_delete': obj(dict(path=text('Existing project-relative regular file inside the assignment scope. No directory recursion.',512),
                               expected_sha256=text('SHA-256 of the exact file to delete.',64),
                               request_key=text('Unique action identity.',128))),
    'project_mkdir': obj(dict(path=text('New project-relative directory within the assignment scope. Parent must exist.',512),
                              request_key=text('Unique action identity.',128))),
    'project_read': obj(dict(path=text('Project-relative regular file or directory. Private runtime/configuration paths are excluded.',512),
                             offset=integer('UTF-8 character offset for file reads.',1000000000),
                             limit=integer('Maximum characters or directory entries; response supplies the next offset.',65536)), required=['path']),
    'project_write': obj(dict(path=text('Project-relative file inside this assignment scope. Symlinks are rejected.',512),
                              content=text('Complete new UTF-8 file content.',1000000),
                              expected_sha256=text('SHA-256 of existing file; use absent to create a new file.',64),
                              request_key=text('Unique action identity. Exact retries reuse the retained result.',128))),
    'draft_write': obj(dict(draft_id=text('Exact unpublished draft_id owned by this channel.',14),
                            content=text('Complete Markdown including the required first line from create_draft.',1000000),
                            request_key=text('Unique action identity.',128))),
    'command_run': obj(dict(argv=array(text('Literal command argument. Only scoped workers may run commands; coordinators and consultants cannot execute them. No implicit shell.',16384), maximum=128, minimum=1),
                            timeout_seconds=dict(type='integer', minimum=1, maximum=300, description='Maximum runtime, including child processes.'),
                            request_key=text('Unique action identity; ambiguous interrupted calls are never re-executed automatically.',128)), required=['argv','request_key']),
}
SCHEMAS['draft_write']['properties']['draft_id']['pattern'] = r'^d_[0-9a-f]{12}$'
# Empty files are legitimate writes.
SCHEMAS['project_write']['properties']['content']['minLength'] = 0


def _parts(value):
    path = PurePosixPath(value)
    if path.is_absolute() or '..' in path.parts or '\\' in value or '\x00' in value or PRIVATE.intersection(path.parts):
        raise StoreError('gateway_path_denied')
    return path.parts


@contextmanager
def _parent(root, value):
    parts = _parts(value)
    if not parts:
        raise StoreError('gateway_file_required')
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd, parts[-1]
    finally:
        os.close(fd)


def _authority(db, channel):
    task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
    token = db.execute('SELECT revoked_at,expires_at FROM trusted_channels WHERE channel_id=?', (channel.channel_id,)).fetchone()
    from .state import now
    if not task or task['state'] in TERMINAL | {'BLOCKED','RECOVERING','WAITING_USER'}:
        raise StoreError('gateway_task_inactive')
    if not token or token['revoked_at'] or (token['expires_at'] and token['expires_at'] <= now()):
        raise StoreError('gateway_channel_inactive')
    if current_owner_epoch(db, channel.task_id) != channel.owner_epoch:
        raise StoreError('stale_ownership_epoch')
    if channel.role == 'coordinator':
        return [], 'coordinator'
    node = db.execute('SELECT n.* FROM runtime_nodes n JOIN delegation_provisioning p '
                      'ON n.assignment_ref=p.idempotency_key AND n.task_id=p.task_id '
                      'WHERE n.task_id=? AND p.attempt_id=? AND n.revision=?',
                      (channel.task_id,channel.attempt_id,task['revision'])).fetchone()
    if not node or node['status'] != 'running':
        raise StoreError('gateway_assignment_inactive')
    return ([] if node['stage']=='review' or node['role'] in READ_ONLY else json.loads(node['scope_json'])), node['assignment_ref']


def _matches(path, scopes):
    return any(scope in {'.','**','**/*'} or path == scope or path.startswith(scope.rstrip('/')+'/')
               or fnmatch.fnmatchcase(path,scope) or (scope.endswith('/**') and path==scope[:-3]) for scope in scopes)


def _read(root, args):
    parts = _parts(args['path'])
    if not parts:
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    else:
        with _parent(root,args['path']) as (parent,leaf):
            fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
    try:
        info = os.fstat(fd)
        offset, limit = args.get('offset',0), max(1,args.get('limit',16000))
        if stat.S_ISDIR(info.st_mode):
            entries = sorted(n for n in os.listdir(fd) if n not in PRIVATE)
            return dict(entries=entries[offset:offset+limit],next_offset=offset+limit if offset+limit<len(entries) else None)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise StoreError('gateway_regular_file_required')
        # Hash/read one stable descriptor; no symlink or hard-link targets.
        with os.fdopen(os.dup(fd),'rb') as stream:
            data = stream.read(16000001)
        if len(data)>16000000:
            raise StoreError('gateway_file_too_large')
        body = data.decode('utf-8')
        return dict(content=body[offset:offset+limit],sha256=hashlib.sha256(data).hexdigest(),
                    total_characters=len(body),next_offset=offset+limit if offset+limit<len(body) else None)
    finally:
        os.close(fd)


def _write(root,args):
    data = args['content'].encode('utf-8')
    with _parent(root,args['path']) as (parent,leaf):
        mode = 0o600
        try:
            fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
        except FileNotFoundError:
            observed = 'absent'
        else:
            with os.fdopen(fd,'rb') as stream:
                info=os.fstat(stream.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1:
                    raise StoreError('gateway_regular_file_required')
                mode=stat.S_IMODE(info.st_mode) & 0o777
                observed=hashlib.file_digest(stream,'sha256').hexdigest()
        if observed != args['expected_sha256']:
            raise StoreError('gateway_write_conflict')
        temporary='.cortex-write-'+uuid.uuid4().hex
        fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,mode,dir_fd=parent)
        try:
            with os.fdopen(fd,'wb') as stream:
                stream.write(data); stream.flush(); os.fsync(stream.fileno())
            os.replace(temporary,leaf,src_dir_fd=parent,dst_dir_fd=parent)
            os.fsync(parent)
        finally:
            try: os.unlink(temporary,dir_fd=parent)
            except FileNotFoundError: pass
    return dict(sha256=hashlib.sha256(data).hexdigest(),byte_count=len(data))


def _command(store,channel,root,scopes,args,ref,owner,effects):
    from .sandbox import as_owner, gateway_command
    with tempfile.TemporaryDirectory(prefix='cortex-command-') as scratch, tempfile.TemporaryFile() as output:
        with gateway_command(root,scratch,args['argv'],scopes) as (command,fds):
            # A worker's shell is the owner's shell, still scope-confined.
            as_owner(command,os.environ)
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                _authority(db,channel)
                effects['possible'] = True
                proc=subprocess.Popen(command,stdout=output,stderr=subprocess.STDOUT,pass_fds=fds,env={'PATH':'/usr/bin:/bin'},start_new_session=True)
                db.commit()
            failure=None
            deadline=time.monotonic()+args.get('timeout_seconds',300)
            try:
                while proc.poll() is None:
                    if time.monotonic()>=deadline:
                        failure='timeout'; break
                    with store.connection() as db:
                        try: _authority(db,channel)
                        except StoreError: failure='authority_revoked'; break
                    time.sleep(.05)
            finally:
                if proc.poll() is None:
                    import signal
                    os.killpg(proc.pid,signal.SIGKILL)
                code=proc.wait()
        output.seek(0)
        data=output.read()
    from ..recovery.large_output_store import persist_output
    retained=persist_output(store,task_id=channel.task_id,owner_scope=owner,source_kind='terminal',
                     source_ref=ref,data=data,exit_code=code,failure=failure)
    from ..delegation.evidence import record_tool_result_observed
    record_tool_result_observed(store,channel.task_id,channel.attempt_id,ref,'execute','Gateway command',
                                'completed' if code==0 and not failure else 'failed')
    return dict(tool_call_ref=ref,exit_code=code,complete=failure is None,failure=failure,
                output=data.decode('utf-8',errors='replace'),large_output_id=retained['id'])


def dispatch(store,channel,operation,args):
    validate(SCHEMAS[operation],args)
    digest=hashlib.sha256(encoded(args).encode()).hexdigest()
    key=args.get('request_key')
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        scopes,owner=_authority(db,channel)
        root=db.execute('SELECT project_root FROM tasks WHERE id=?',(channel.task_id,)).fetchone()[0]
        if operation=='project_read':
            try: return _read(root,args)
            except FileNotFoundError as error:
                raise StoreError('gateway_path_not_found', 'path', args['path'],
                    'an existing project-relative file or directory',
                    'The requested path does not exist. List its existing parent with project_read and select an observed path.') from error
            except (OSError,UnicodeError) as error:
                raise StoreError('gateway_read_failure', 'path', args['path'],
                    'a readable regular UTF-8 file or directory',
                    'The project read failed; no mutation was attempted. Check the path, file type and read access.') from error
        if operation in {'project_write','project_mkdir','project_delete'} and not _matches(args['path'],scopes):
            raise StoreError('gateway_scope_denied')
        role=db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',(channel.attempt_id,)).fetchone() if channel.attempt_id else None
        if operation=='command_run' and (channel.role=='coordinator' or (role and 'consultant' in role['role_id'])):
            raise StoreError('gateway_command_denied')
        row=db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                        (channel.task_id,channel.channel_id,key)).fetchone()
        if row:
            if row['digest']!=digest or row['operation']!=operation:
                raise StoreError('request_key_conflict')
            receipt=json.loads(row['result_json'])
            if not receipt.get('complete'):
                raise StoreError('gateway_action_outcome_unknown')
            if receipt.get('error'): raise StoreError(receipt['error'])
            return receipt['receipt']
        ref='gateway:'+uuid.uuid4().hex
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id,key,channel.channel_id,operation,digest,encoded(dict(complete=False,tool_call_ref=ref))))
        event(db,channel.task_id,'TOOL_AUTHORIZED',dict(operation=operation,tool_call_ref=ref),channel.channel_id)
        db.commit()
    # A durable started receipt fences uncertain crash/retry outcomes. Never
    # blindly repeat a command or write after a lost acknowledgement.
    effects = {'possible': False}
    try:
        if operation=='project_git':
            from .artifacts import project_git_evidence
            result=project_git_evidence(store,channel.task_id,owner,root,args['kind'])
        elif operation=='command_run':
            result=_command(store,channel,root,scopes,args,ref,owner,effects)
        else:
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                scopes,owner=_authority(db,channel)
                if operation=='project_delete':
                    observed=_read(root,dict(path=args['path'],limit=1))
                    if observed.get('sha256')!=args['expected_sha256']:
                        raise StoreError('gateway_write_conflict')
                    with _parent(root,args['path']) as (parent,leaf):
                        os.unlink(leaf,dir_fd=parent); os.fsync(parent)
                    result=dict(deleted=True)
                elif operation=='project_mkdir':
                    with _parent(root,args['path']) as (parent,leaf):
                        os.mkdir(leaf,0o755,dir_fd=parent)
                        os.fsync(parent)
                    result=dict(created=True)
                elif operation=='project_write':
                    result=_write(root,args)
                else:
                    draft,path=store._draft_source(db,channel.task_id,channel.principal_thread_id,args['draft_id'])
                    data=args['content'].encode('utf-8')
                    if not args['content'].startswith(f"Cortex draft ID: `{args['draft_id']}`\n"):
                        raise StoreError('gateway_draft_marker_required')
                    fd=os.open(path,os.O_WRONLY|os.O_NOFOLLOW)
                    with os.fdopen(fd,'wb') as stream:
                        info=os.fstat(stream.fileno())
                        if info.st_nlink!=1 or not stat.S_ISREG(info.st_mode):
                            raise StoreError('gateway_regular_file_required')
                        stream.truncate(); stream.write(data); stream.flush(); os.fsync(stream.fileno())
                    result=dict(sha256=hashlib.sha256(data).hexdigest(),byte_count=len(data))
                db.commit()
    except StoreError as error:
        if effects['possible']:
            # Execution may have succeeded even when evidence persistence failed.
            # Keep the durable pending receipt: retry must never repeat it.
            raise
        # These explicit preconditions run before the operation's side effect.
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                       (encoded(dict(complete=True,error=str(error))),channel.task_id,channel.channel_id,key))
            event(db,channel.task_id,'TOOL_REJECTED',dict(operation=operation,tool_call_ref=ref,error=str(error)),channel.channel_id)
            db.commit()
        raise
    except (OSError,UnicodeError) as error:
        raise StoreError('gateway_io_failure', 'operation', operation) from error
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                    (encoded(dict(complete=True,receipt=result)),channel.task_id,channel.channel_id,key))
        event(db,channel.task_id,'TOOL_COMPLETED',dict(operation=operation,tool_call_ref=ref,result=result),channel.channel_id)
        db.commit()
    return result


def verified_check(db,task_id,attempt_id,ref):
    """A successful native MCP envelope is not proof that its command passed."""
    if not isinstance(ref,str) or not ref.startswith('gateway:'): return False
    rows=db.execute("SELECT r.result_json FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor "
                    "WHERE r.task_id=? AND c.attempt_id=? AND r.operation='command_run'",(task_id,attempt_id))
    matches=[]
    for row in rows:
        saved=json.loads(row[0]);result=saved.get('receipt',{})
        if result.get('tool_call_ref')==ref: matches.append((saved,result))
    return (len(matches)==1 and matches[0][0].get('complete') is True
            and matches[0][1].get('complete') is True and matches[0][1].get('exit_code')==0
            and matches[0][1].get('failure') is None and bool(matches[0][1].get('large_output_id')))
