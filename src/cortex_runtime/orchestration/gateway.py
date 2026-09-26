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

from .. import task_workspace
from ..contracts import StoreError, storage_failure
from ..ownership import current_owner_epoch
from .contracts import obj, text, array, validate
from .state import encoded, event, TERMINAL

from .project_paths import PRIVATE, permitted_path
READ_ONLY = {'worker-architect', 'worker-ux-designer', 'worker-code-reviewer', 'worker-senior-consultant', 'senior-consultant',
             'worker-explorer', 'worker-planner', 'worker-security-auditor', 'worker-accessibility-auditor'}


def integer(description, maximum):
    return dict(type='integer', description=description, minimum=0, maximum=maximum)


# Internal ceiling for a computed deadline/timeout: any advertised value above
# this (the schema itself is unbounded above 1, so the runtime is unbounded by
# default) is clamped here instead, so an enormous integer never overflows the
# monotonic-clock deadline arithmetic or subprocess timeout math after the
# command already started. ~68 years; effectively unbounded for any real run.
TIMEOUT_CEILING_SECONDS = 2 ** 31 - 1


def clamp_timeout(value):
    if value is None:
        return None
    return min(value, TIMEOUT_CEILING_SECONDS)


def timeout(description):
    return dict(type='integer', description=description, minimum=1)


SCHEMAS = {
    'project_git': obj(dict(kind=dict(type='string',enum=['status','diff'],description='Read-only project status or complete working-tree diff against HEAD, including pre-existing edits. Returns an exact retained output for read_large_output. Untracked file content is read through project_read.'),request_key=text('Unique observation identity; retries return the same retained snapshot.',128))),
    'project_delete': obj(dict(path=text('Existing project-relative regular file inside the assignment scope. No directory recursion.',512),
                               expected_sha256=text('Lowercase hex SHA-256 of the file as it is now (project_read returns it as sha256). A mismatch refuses the deletion.',64),
                               request_key=text('Unique action identity.',128))),
    'project_mkdir': obj(dict(path=text('New project-relative directory within the assignment scope. Parent must exist.',512),
                              request_key=text('Unique action identity.',128))),
    'project_read': obj(dict(path=text('Project-relative regular file or directory; \'.\' is the project root. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write.',512),
                             offset=integer('UTF-8 character offset for file reads.',1000000000),
                             limit=integer('Maximum characters or directory entries; response supplies the next offset.',65536)), required=['path']),
    'project_write': obj(dict(path=text('Project-relative file inside this assignment scope. Symlinks are rejected.',512),
                              content=text('Complete new UTF-8 file content.',1000000),
                              expected_sha256=text('Lowercase hex SHA-256 of the file as it is now (project_read returns it as sha256), or the literal string "absent" to create a new file. A mismatch refuses the write and changes nothing.',64),
                              request_key=text('Unique action identity. Exact retries reuse the retained result.',128))),
    'draft_write': obj(dict(draft_id=text('Exact unpublished draft_id owned by this channel.',14),
                            content=text('Complete Markdown including the required first line from create_draft.',1000000),
                            request_key=text('Unique action identity.',128))),
    'command_run': obj(dict(argv=array(text('Literal command argument. Only scoped workers may run commands; coordinators and consultants cannot execute them. No implicit shell.',16384), maximum=128, minimum=1),
                            timeout_seconds=timeout('Optional maximum runtime in seconds, including child processes. Omit to run until the command exits.'),
                            request_key=text('Unique action identity; ambiguous interrupted calls are never re-executed automatically.',128)), required=['argv','request_key']),
    'outside_write': obj(dict(path=text('Absolute path OUTSIDE the project (inside it, write with your own tools). The user confirms first; the parent directory must exist.',4096),
                              content=text('Complete new UTF-8 file content.',1000000),
                              request_key=text('Unique action identity.',128))),
    'outside_command': obj(dict(argv=array(text('Literal command argument, run as the user outside the sandbox after the user confirms (e.g. a global install, a change in HOME). No implicit shell.',16384), maximum=128, minimum=1),
                                cwd=text('Absolute working directory; defaults to the project root.',4096),
                                timeout_seconds=timeout('Optional maximum runtime in seconds; default 600.'),
                                request_key=text('Unique action identity.',128)), required=['argv','request_key']),
}
SCHEMAS['outside_write']['properties']['content']['minLength'] = 0
SCHEMAS['draft_write']['properties']['draft_id']['pattern'] = r'^d_[0-9a-f]{12}$'
# Empty files are legitimate writes.
SCHEMAS['project_write']['properties']['content']['minLength'] = 0


def _parts(value):
    path = PurePosixPath(value)
    if not permitted_path(value):
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


def channel_authority(db, channel):
    """The channel itself is live: not revoked, not expired, current owner."""
    token = db.execute('SELECT revoked_at,expires_at FROM trusted_channels WHERE channel_id=?', (channel.channel_id,)).fetchone()
    from .state import now
    if not token or token['revoked_at'] or (token['expires_at'] and token['expires_at'] <= now()):
        raise StoreError('gateway_channel_inactive')
    if current_owner_epoch(db, channel.task_id) != channel.owner_epoch:
        raise StoreError('stale_ownership_epoch')


def _authority(db, channel):
    task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
    if task and task['state'] == 'RECOVERING':
        raise StoreError('gateway_task_recovering')
    if not task or task['state'] in TERMINAL | {'BLOCKED','WAITING_USER'}:
        raise StoreError('gateway_task_inactive')
    channel_authority(db, channel)
    if channel.role == 'coordinator':
        return [], 'coordinator'
    node = db.execute('SELECT n.* FROM runtime_nodes n JOIN delegation_provisioning p '
                      'ON n.assignment_ref=p.idempotency_key AND n.task_id=p.task_id '
                      'WHERE n.task_id=? AND p.attempt_id=? AND n.revision=?',
                      (channel.task_id,channel.attempt_id,task['revision'])).fetchone()
    if not node or node['status'] != 'running':
        raise StoreError('gateway_assignment_inactive')
    # A writing worker may change the whole project (the owner's rule: all of
    # the project, nothing outside it without consent); the plan's scope only
    # describes the work. Reviewers and read-only roles write nothing.
    return ([] if node['stage']=='review' or node['role'] in READ_ONLY else ['.']), node['assignment_ref']


def write_denials(db, task_id):
    """The user's deny-write paths: read-only even inside a writing grant."""
    from .policy import denied_paths
    return denied_paths(db, task_id)


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
    from .sandbox import as_owner, gateway_command, with_umask
    from . import ssh_askpass
    with tempfile.TemporaryDirectory(prefix='cortex-command-') as scratch, tempfile.TemporaryFile() as output, \
            tempfile.TemporaryDirectory(prefix='cortex-askpass-') as askpass_dir:
        # The whole host read-only, as the worker's own jailed shell sees it.
        with store.connection() as db: denied=write_denials(db,channel.task_id)
        # Git sees the project's repository: read-only, or with a writing
        # grant updatable except its configuration and hooks (see sandbox).
        planted=[]
        with gateway_command(root,scratch,with_umask(args['argv']),scopes,system_root=True,readonly=denied,
                             reveal=frozenset({'.git'}),git_writable=True,git_guard=planted) as (command,fds):
            # A worker's shell is the owner's shell, still scope-confined.
            as_owner(command,os.environ)
            if not scopes:
                from .sandbox import readonly_test_cache
                readonly_test_cache(command, scratch, os.environ)
            # Opt-in: ssh asks for a password in a desktop dialog.
            ssh_askpass.apply(command,os.environ,Path(askpass_dir))
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                _authority(db,channel)
                effects['possible'] = True
                from .sandbox import compact_argv
                launch,launch_fds=compact_argv(command,fds)
                try:
                    proc=subprocess.Popen(launch,stdout=output,stderr=subprocess.STDOUT,pass_fds=launch_fds,env={'PATH':'/usr/bin:/bin'},start_new_session=True)
                finally:
                    for extra in set(launch_fds)-set(fds): os.close(extra)
                db.commit()
            failure=None
            # No default limit: a long build or backfill runs to completion;
            # a stop/cancel still ends it through the authority check below.
            limit=clamp_timeout(args.get('timeout_seconds'))
            deadline=None if limit is None else time.monotonic()+limit
            try:
                while proc.poll() is None:
                    if deadline is not None and time.monotonic()>=deadline:
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
    if planted:
        # Removed again: git configuration, hooks or a repository pointer the
        # command created would otherwise run code later, outside the sandbox.
        failure=failure or 'git_configuration_protected'
        data+=('\n[cortex] removed git configuration the command created (protected: it would run '
               'code outside the sandbox later): '+', '.join(planted)+'\n').encode()
    from ..recovery.large_output_store import persist_output
    retained=persist_output(store,task_id=channel.task_id,owner_scope=owner,source_kind='terminal',
                     source_ref=ref,data=data,exit_code=code,failure=failure)
    from ..delegation.evidence import record_tool_result_observed
    record_tool_result_observed(store,channel.task_id,channel.attempt_id,ref,'execute','Gateway command',
                                'completed' if code==0 and not failure else 'failed')
    # argv is retained so a check citing this receipt can be matched against it.
    return dict(tool_call_ref=ref,argv=list(args['argv']),exit_code=code,complete=failure is None,failure=failure,
                output=data.decode('utf-8',errors='replace'),large_output_id=retained['id'])


def _outside(store,channel,root,operation,args,ref,owner,effects):
    """Write a file or run a command outside the sandbox, as the user, once
    the user allows it (orchestration.consent)."""
    from . import consent
    target=Path(args['path'] if operation=='outside_write' else (args.get('cwd') or root))
    if not target.is_absolute():
        raise StoreError('outside_path_must_be_absolute','path',str(target),'an absolute path','Give an absolute path.')
    project=Path(root).resolve()
    resolved=Path(os.path.normpath(str(target)))
    if operation=='outside_write' and (resolved==project or project in resolved.parents):
        raise StoreError('outside_path_inside_project','path',str(target),'a path outside the project',
                         'Inside the project, write with your own tools or project_write.')
    def still_allowed():
        with store.connection() as db: _authority(db,channel)
    if not consent.request(store,channel,operation,consent.subject_of(operation,args),still_allowed=still_allowed):
        raise StoreError('outside_action_not_confirmed','operation',operation,'the user allowing it',
                         'The user did not allow this action; do the work another way or explain what you need.')
    effects['possible']=True
    if operation=='outside_write':
        data=args['content'].encode('utf-8')
        temporary=resolved.with_name(resolved.name+'.cortex-'+uuid.uuid4().hex)
        with open(temporary,'xb') as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(temporary,resolved)
        return dict(path=str(resolved),byte_count=len(data),sha256=hashlib.sha256(data).hexdigest())
    with tempfile.TemporaryFile() as output:
        try:
            done=subprocess.run(args['argv'],cwd=str(resolved),stdout=output,stderr=subprocess.STDOUT,
                                timeout=clamp_timeout(args.get('timeout_seconds')) or 600)
            code,failure=done.returncode,None
        except subprocess.TimeoutExpired:
            code,failure=None,'timeout'
        except OSError as error:
            code,failure=None,f'not_started:{error.strerror}'
        output.seek(0); data=output.read()
    from ..recovery.large_output_store import persist_output
    retained=persist_output(store,task_id=channel.task_id,owner_scope=owner,source_kind='terminal',
                            source_ref=ref,data=data,exit_code=code,failure=failure)
    # cwd completes the command's identity (delegation.evidence.command_identity).
    return dict(tool_call_ref=ref,argv=list(args['argv']),cwd=str(resolved),exit_code=code,complete=failure is None,
                failure=failure,output=data.decode('utf-8',errors='replace')[-65536:],large_output_id=retained['id'])


def dispatch(store,channel,operation,args):
    validate(SCHEMAS[operation],args)
    digest=hashlib.sha256(encoded(args).encode()).hexdigest()
    key=args.get('request_key')
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        scopes,owner=_authority(db,channel)
        # The task's working tree: its session's worktree, else the project.
        root=task_workspace.task_root(db,channel.task_id)
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
        if operation in {'project_write','project_mkdir','project_delete'} and (
                not _matches(args['path'],scopes) or _matches(args['path'],write_denials(db,channel.task_id))):
            raise StoreError('gateway_scope_denied')
        if operation in {'outside_write','outside_command'} and not scopes:
            raise StoreError('gateway_scope_denied')  # writing workers only
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
        elif operation in {'outside_write','outside_command'}:
            result=_outside(store,channel,root,operation,args,ref,owner,effects)
        else:
            if operation=='project_delete':
                # A deletion is the user's to confirm (never inside the
                # write transaction below: the wait is a person's).
                from . import consent
                def still_allowed():
                    with store.connection() as db: _authority(db,channel)
                if not consent.request(store,channel,'project_delete',args['path'],still_allowed=still_allowed):
                    raise StoreError('deletion_not_confirmed')
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
        if effects['possible']:
            # The command may have run: an uncertain outcome, never "nothing changed".
            raise storage_failure(error) from error
        raise StoreError('gateway_io_failure', 'operation', operation) from error
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                    (encoded(dict(complete=True,receipt=result)),channel.task_id,channel.channel_id,key))
        event(db,channel.task_id,'TOOL_COMPLETED',dict(operation=operation,tool_call_ref=ref,result=result),channel.channel_id)
        db.commit()
    return result


def verified_check(db,task_id,attempt_id,ref,outcome='passed'):
    """Whether one check outcome is verified by its cited receipt.

    See ``delegation.evidence.assess_check``: only this exact attempt's
    complete ``command_run``/``outside_command`` receipt with a matching exit
    status, a complete retained output and (for a pass) a nontrivial command
    verifies a check. A missing ref or an engine-native call never does.
    """
    from ..delegation.evidence import assess_check
    return assess_check(db,task_id,attempt_id,outcome,ref)['strength']=='verified'
