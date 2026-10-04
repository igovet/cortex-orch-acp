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
# The read-only roles live with the plan contract that has to name them.
from .contracts import READ_ONLY  # noqa: E402


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


# What a write tool's path may name (gateway._authority): a writing worker holds
# the whole project; the plan's declared scope coordinates writers only.
WORK_AREA = ("A writing worker may change any project path except private storage and paths the user protected. "
             "The assignment's declared scope is the agreed work area that coordinates parallel writers, not an "
             "enforced boundary: reserve any other path with access_request before changing it.")


SCHEMAS = {
    'project_git': obj(dict(kind=dict(type='string',enum=['status','diff'],description='Read-only project status or complete working-tree diff against HEAD, including pre-existing edits. Returns an exact retained output for read_large_output. Untracked file content is read through project_read.'),request_key=text('Unique observation identity; retries return the same retained snapshot.',128))),
    'project_delete': obj(dict(path=text('Existing project-relative regular file. '+WORK_AREA+' Binary files are supported; no directory recursion. Root .gitignore deletion retains only the required /.cortex/ rule and returns deleted: false.',512),
                               expected_sha256=text('Lowercase hex SHA-256 of the file raw bytes as they are now (project_read returns it as sha256). A mismatch refuses the deletion.',64),
                               request_key=text('Unique action identity.',128))),
    'project_mkdir': obj(dict(path=text('New project-relative directory; its parent must exist. '+WORK_AREA,512),
                              request_key=text('Unique action identity.',128))),
    'project_read': obj(dict(path=text('Project-relative regular file or directory; \'.\' is the project root. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write. UTF-8 files return text and SHA-256; non-UTF-8 files return only SHA-256, byte size and binary: true.',512),
                             offset=integer('UTF-8 character offset for file reads.',1000000000),
                             limit=integer('Maximum characters or directory entries; response supplies the next offset.',65536)), required=['path']),
    'project_write': obj(dict(path=text('Project-relative file; symlinks are rejected. '+WORK_AREA,512),
                              content=text('Complete new UTF-8 file content. Root .gitignore writes retain a final /.cortex/ rule; returned hash describes the protected bytes.',1000000),
                              expected_sha256=text('Lowercase hex SHA-256 of the file as it is now (project_read returns it as sha256), or the literal string "absent" to create a new file. A mismatch refuses the write and changes nothing.',64),
                              request_key=text('Unique action identity. Exact retries reuse the retained result.',128))),
    'project_skill_link': obj(dict(action=dict(type='string',enum=['link','unlink'],description='link: make .claude/skills/<name> the relative symlink ../../.agents/skills/<name>, creating .claude/skills when absent; an exact existing link is left unchanged. unlink: remove only that exact link (the skill in .agents/skills stays).'),
                                   name=text('Skill directory name under .agents/skills: 1-64 lowercase letters, digits and single hyphens. For link, .agents/skills/<name> must be a real directory holding a regular SKILL.md.',64),
                                   replace_symlink_target=text('link only, and only to replace an existing symlink at .claude/skills/<name> with a different target: its exact current link text (a refusal names it). The user confirms the replacement. Omit otherwise; a file or directory there is never replaced.',4096),
                                   request_key=text('Unique action identity.',128)), required=['action','name','request_key']),
    'draft_write': obj(dict(draft_id=text('Exact unpublished draft_id owned by this channel.',14),
                            content=text('Complete Markdown including the required first line from create_draft.',1000000),
                            request_key=text('Unique action identity.',128))),
    'command_run': obj(dict(argv=array(text('Literal command argument. Only scoped workers may run commands; coordinators and consultants cannot execute them. No implicit shell.',16384),
                                       'Command as argv, run without a shell. Only scoped workers may run commands.', maximum=128, minimum=1),
                            timeout_seconds=timeout('Optional maximum runtime in seconds, including child processes. Omit to run until the command exits.'),
                            request_key=text('Unique action identity; ambiguous interrupted calls are never re-executed automatically.',128)), required=['argv','request_key']),
    'outside_write': obj(dict(path=text('Absolute path OUTSIDE the project (inside it, write with your own tools). The user confirms first; the parent directory must exist.',4096),
                              content=text('Complete new UTF-8 file content.',1000000),
                              request_key=text('Unique action identity.',128))),
    'outside_command': obj(dict(argv=array(text('Literal command argument, run as the user outside the sandbox after the user confirms (e.g. a global install, a change in HOME). No implicit shell.',16384),
                                           'Command as argv, run as the user outside the sandbox without a shell, after the user confirms.', maximum=128, minimum=1),
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


def _read_regular_file(fd):
    """Read and hash one regular, single-link file through its open descriptor."""
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise StoreError('gateway_regular_file_required')
    with os.fdopen(os.dup(fd),'rb') as stream:
        data = stream.read(16000001)
    if len(data)>16000000:
        raise StoreError('gateway_file_too_large')
    return data, hashlib.sha256(data).hexdigest()


def _file_sha256(root, path):
    """Hash raw file bytes through a no-follow descriptor under the project root."""
    with _parent(root,path) as (parent,leaf):
        fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
    try:
        _, digest = _read_regular_file(fd)
        return digest
    finally:
        os.close(fd)


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
        data, digest = _read_regular_file(fd)
        try:
            body = data.decode('utf-8')
        except UnicodeDecodeError:
            return dict(sha256=digest,size_bytes=len(data),binary=True)
        return dict(content=body[offset:offset+limit],sha256=digest,
                    total_characters=len(body),next_offset=offset+limit if offset+limit<len(body) else None)
    finally:
        os.close(fd)


def _write(root,args):
    from .ignore_guard import protected_content
    content = protected_content(args['content']) if _parts(args['path']) == ('.gitignore',) else args['content']
    data = content.encode('utf-8')
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
    from .ignore_guard import snapshot, enforce
    ignore_before = snapshot(root) if scopes else None
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
    if scopes:
        try:
            if snapshot(root) != ignore_before:
                enforce(store, channel)
        except (StoreError, OSError, UnicodeError):
            failure = failure or 'gitignore_guard_failed'
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
    from .ignore_guard import snapshot, enforce
    ignore_before = snapshot(root) if operation == 'outside_command' else None
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
    try:
        if snapshot(root) != ignore_before:
            enforce(store, channel)
    except (StoreError, OSError, UnicodeError):
        failure = failure or 'gitignore_guard_failed'
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
        role=db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',(channel.attempt_id,)).fetchone() if channel.attempt_id else None
        consultant=bool(role and 'consultant' in role['role_id'])
        if consultant and operation in {'project_read','project_git'}:
            # The senior consultant works only from its consultation packet
            # and retained reports, never from the project's source.
            raise StoreError('gateway_read_denied')
        if operation=='project_read':
            try: return _read(root,args)
            except FileNotFoundError as error:
                raise StoreError('gateway_path_not_found', 'path', args['path'],
                    'an existing project-relative file or directory',
                    'The requested path does not exist. List its existing parent with project_read and select an observed path.') from error
            except (OSError,UnicodeError) as error:
                raise StoreError('gateway_read_failure', 'path', args['path'],
                    'a readable regular file or directory',
                    'The project read failed; no mutation was attempted. Check the path, file type and read access.') from error
        from .worker_waits import check_mutation
        if operation != 'project_git':
            check_mutation(db, channel)
        if operation in {'project_write','project_mkdir','project_delete'}:
            # A private path is refused before anything else (a deletion
            # there once asked the user to confirm what was then refused).
            _parts(args['path'])
            if not _matches(args['path'],scopes) or _matches(args['path'],write_denials(db,channel.task_id)):
                raise StoreError('gateway_scope_denied')
        if operation=='project_skill_link':
            # The one .claude write: a skill link, never a generic grant
            # (skill_links). Every writing worker holds the whole-project
            # grant (_authority); read-only roles and reviews hold none, and
            # a user deny-write path covering the link refuses it.
            from .skill_links import check_name, link_path
            if args['action']=='unlink' and 'replace_symlink_target' in args:
                raise StoreError('invalid_arguments','replace_symlink_target',args['replace_symlink_target'][:40],
                                 'omitted for unlink','Remove replace_symlink_target; it applies only to link.')
            linked=link_path(check_name(args['name']))
            if '.' not in scopes or _matches(linked,write_denials(db,channel.task_id)):
                raise StoreError('gateway_scope_denied')
        if operation in {'outside_write','outside_command'} and not scopes:
            raise StoreError('gateway_scope_denied')  # writing workers only
        if operation=='command_run' and (channel.role=='coordinator' or consultant):
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
            from . import skill_links
            if (operation=='project_skill_link' and 'replace_symlink_target' in args
                    and args['replace_symlink_target']!=skill_links.link_target(args['name'])):
                # Replacing another link is the user's to confirm. The link
                # must still be exactly the confirmed one when it is replaced
                # (skill_links.apply observes it again below).
                from . import consent
                observed=skill_links.inspect(root,args['name'])
                if observed!=('symlink',args['replace_symlink_target']):
                    raise StoreError('skill_link_conflict','replace_symlink_target',args['replace_symlink_target'][:160],
                                     observed[1] or observed[0],'Nothing was replaced. Pass the exact current link text, or omit it.')
                def still_allowed():
                    with store.connection() as db: _authority(db,channel)
                subject=(f"{skill_links.link_path(args['name'])} -> {args['replace_symlink_target']}"
                         f" becomes -> {skill_links.link_target(args['name'])}")
                if not consent.request(store,channel,'project_skill_link',subject,still_allowed=still_allowed):
                    raise StoreError('skill_link_replace_not_confirmed')
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                scopes,owner=_authority(db,channel)
                if operation=='project_delete':
                    observed=_file_sha256(root,args['path'])
                    if observed!=args['expected_sha256']:
                        raise StoreError('gateway_write_conflict')
                    if _parts(args['path']) == ('.gitignore',):
                        result = _write(root, dict(args, content=''))
                        result['deleted'] = False
                        result['gitignore_guard'] = True
                    else:
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
                elif operation=='project_skill_link':
                    result=skill_links.apply(root,args,effects)
                else:
                    result=write_draft(store,db,channel,args['draft_id'],args['content'])
                db.commit()
            if operation=='project_skill_link' and args['action']=='link':
                # Observed after the commit: a git subprocess never runs
                # under the store's write lock. Read-only and never raises.
                result['git']=skill_links.git_retention(root,args['name'])
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


def write_draft(store,db,channel,draft_id,content):
    """Replace one of this channel's own unpublished drafts, inside the
    caller's open transaction. Shared by draft_write and the inline report of
    result_submit, so both keep the same ownership, marker and file checks."""
    draft,path=store._draft_source(db,channel.task_id,channel.principal_thread_id,draft_id)
    data=content.encode('utf-8')
    if not content.startswith(f"Cortex draft ID: `{draft_id}`\n"):
        raise StoreError('gateway_draft_marker_required')
    fd=os.open(path,os.O_WRONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,'wb') as stream:
        info=os.fstat(stream.fileno())
        if info.st_nlink!=1 or not stat.S_ISREG(info.st_mode):
            raise StoreError('gateway_regular_file_required')
        stream.truncate(); stream.write(data); stream.flush(); os.fsync(stream.fileno())
    return dict(sha256=hashlib.sha256(data).hexdigest(),byte_count=len(data))


def verified_check(db,task_id,attempt_id,ref,outcome='passed'):
    """Whether one check outcome is verified by its cited receipt.

    See ``delegation.evidence.assess_check``: only this exact attempt's
    complete ``command_run``/``outside_command`` receipt with a matching exit
    status, a complete retained output and (for a pass) a nontrivial command
    verifies a check. A missing ref or an engine-native call never does.
    """
    from ..delegation.evidence import assess_check
    return assess_check(db,task_id,attempt_id,outcome,ref)['strength']=='verified'
