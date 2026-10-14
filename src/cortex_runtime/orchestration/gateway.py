"""Channel-bound action gateway. Native engines receive no filesystem tools.

Authorization comes from the current durable assignment, never tool arguments.
File operations use no-follow directory descriptors. Commands get a read-only
project with only explicitly granted directories/files mounted writable.
"""
from contextlib import contextmanager
import errno
import fnmatch
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import sqlite3
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
AUTO_KEY = ('Optional unique action identity. Omit to derive a stable identity from the operation and payload; '
            'an identical keyless retry reuses a retained effect receipt, and a different payload is a new action. '
            'A precondition refusal before any possible effect has no replayable derived-key receipt: fix the '
            'precondition, then retry identically. An explicit key retains its structured refusal; use a new '
            'key after correction.')

WORK_AREA = ("A writing worker may change any project path except private storage and paths the user protected. "
             "The assignment's declared scope is the agreed work area that coordinates parallel writers, not an "
             "enforced boundary: reserve any other path with access_request before changing it.")


SCHEMAS = {
    'project_git': obj(dict(kind=dict(type='string',enum=['status','diff'],description='Read-only project status or complete working-tree diff against HEAD, including pre-existing edits. Inlines output up to 64 KiB (same bound as command_run), with output_truncated marking the rest; large_output_id always pages the complete retained bytes with read_large_output. Untracked file content is read through project_read.'),request_key=text('Unique observation identity; retries return the same retained snapshot.',128))),
    'project_delete': obj(dict(path=text('Existing project-relative regular file. '+WORK_AREA+' Binary files are supported; no directory recursion. Root .gitignore deletion retains only the required /.cortex/ rule and returns deleted: false.',512),
                               expected_sha256=text('Lowercase hex SHA-256 of the file raw bytes as they are now (project_read returns it as sha256). A mismatch refuses the deletion.',64),
                               request_key=text(AUTO_KEY,128)), required=['path','expected_sha256']),
    'project_mkdir': obj(dict(path=text('Project-relative directory; missing parent directories are created too, and an existing directory returns created: false. '+WORK_AREA,512),
                              request_key=text(AUTO_KEY,128)), required=['path']),
    'project_read': obj(dict(path=text('Project-relative regular file or directory; \'.\' is the project root. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write. Missing paths return exists: false, sha256: "absent" (pass directly to project_write.expected_sha256), and parent_exists (informational: project_write creates missing parent directories). UTF-8 files return text and SHA-256; a supported image (PNG, JPEG, GIF or WEBP, detected by content, not extension) returns an MCP image content block plus sha256/size_bytes/mime/path as text, up to a 10 MiB cap (gateway_image_too_large above it); any other non-UTF-8 file returns only SHA-256, byte size and binary: true.',512),
                             offset=dict(type='integer',minimum=1,maximum=1000000000,description='1-based line offset for text or 1-based entry offset for directories; default 1. Never a character offset.'),
                             limit=dict(type='integer',minimum=1,maximum=2000,description='Maximum lines or directory entries; default and maximum 2000. The inline byte budget may return fewer; follow next_offset. Lines longer than 2000 characters, or a single line exceeding the inline budget, are explicitly truncated and never count toward complete read coverage.'),
                             line_numbers=dict(type='boolean',description='Default true: prefix text lines with their 1-based number and a tab. Set false for exact raw text. Never include number prefixes in project_edit strings or project_write content.')), required=['path']),
    'project_write': obj(dict(path=text('Project-relative file; missing parent directories are created (never through a symlink or file); symlinks are rejected. '+WORK_AREA,512),
                              content=text('Complete replacement UTF-8 content, never a page or append. Existing files require complete, non-truncated project_read coverage of the current bytes on this same channel. Use project_edit for partial changes or appending. Root .gitignore writes retain a final /.cortex/ rule; returned hash describes the protected bytes.',1000000),
                              expected_sha256=text('Current SHA-256 from project_read.full_content_sha256 after complete read coverage on this channel, or "absent" returned by project_read for a missing path; pass that value directly for creation. A partial read sha256 is only identity and cannot authorize whole-file overwrite; neither a supplied hash nor a drastic shrink bypasses the complete-read guard. A mismatch changes nothing.',64),
                              request_key=text(AUTO_KEY,128)), required=['path','content','expected_sha256']),
    'project_edit': obj(dict(
        path=text('Existing project-relative UTF-8 regular file. '+WORK_AREA+' Requires a prior text read on this channel and an unchanged SHA-256; a partial read suffices. Refuses links and binary files.',512),
        edits=array(obj(dict(
            old_string=text('Exact nonempty text to replace, including whitespace but excluding read line-number prefixes; must occur once unless replace_all is true.',1000000),
            new_string=dict(type='string',minLength=0,maxLength=1000000,description='Replacement text, possibly empty to remove the match; must differ from old_string. To append, match a unique tail anchor and repeat it followed by the added text.'),
            replace_all=dict(type='boolean',description='Default false: require exactly one match. True replaces all exact occurrences, never regex or fuzzy matching.')),
            required=['old_string','new_string']),
            'Ordered exact replacements, each with old_string, new_string and optional replace_all. Validated against the evolving content and committed atomically only if every edit succeeds. To append, replace a unique tail anchor with itself plus the appended text.',
            minimum=1,maximum=64),
        request_key=text(AUTO_KEY,128)),required=['path','edits']),
    'project_skill_link': obj(dict(action=dict(type='string',enum=['link','unlink'],description='link: make .claude/skills/<name> the relative symlink ../../.agents/skills/<name>, creating .claude/skills when absent; an exact existing link is left unchanged. unlink: remove only that exact link (the skill in .agents/skills stays).'),
                                   name=text('Skill directory name under .agents/skills: 1-64 lowercase letters, digits and single hyphens. For link, .agents/skills/<name> must be a real directory holding a regular SKILL.md.',64),
                                   replace_symlink_target=text('link only, and only to replace an existing symlink at .claude/skills/<name> with a different target: its exact current link text (a refusal names it). The user confirms the replacement. Omit otherwise; a file or directory there is never replaced.',4096),
                                   request_key=text('Unique action identity.',128)), required=['action','name','request_key']),
    'draft_write': obj(dict(draft_id=text('Exact unpublished draft_id owned by this channel.',14),
                            content=text('Complete Markdown including the required first line from create_draft.',1000000),
                            request_key=text('Unique action identity.',128))),
    'command_run': obj(dict(argv=array(text('Literal command argument. Only scoped workers may run commands; coordinators and consultants cannot execute them. No implicit shell.',16384),
                                       'Command as argv, run without a shell. Only scoped workers may run commands. For the current reviewer, exact plan verification argv at the same project digest reuses a runtime-produced receipt; an in-flight run is awaited for at most five seconds (or the shorter requested timeout). Otherwise the command executes normally.', maximum=128, minimum=1),
                            cwd=text('Absolute existing working directory. Omitted: task working tree. Outside-tree cwd requires a writing worker in Development or Auto; private and protected targets are refused, never retried outside the sandbox.',4096),
                            timeout_seconds=timeout('Optional maximum runtime in seconds, including child processes. Omit to run until the command exits.'),
                            request_key=text('Unique action identity; ambiguous interrupted calls are never re-executed automatically.',128)), required=['argv','request_key']),
    'outside_write': obj(dict(path=text('Absolute path OUTSIDE the project (inside it, write with your own tools). In Guarded the user confirms first; in Development or Auto only protected control-plane paths ask. The parent directory must exist.',4096),
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


_DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW


def _open_directory(fd, part, component, precise):
    try:
        return os.open(part, _DIRECTORY, dir_fd=fd)
    except OSError as error:
        if not precise or error.errno not in (errno.ENOTDIR, errno.ELOOP):
            raise
        raise StoreError('gateway_parent_not_directory', 'path', component,
                         'a real directory (not a file or symbolic link)',
                         f'{component} is a file or symbolic link; project tools never follow links or '
                         'write through a file. Choose a path under a real directory.') from None


@contextmanager
def _parent(root, value, *, create_parents=None):
    """Open the parent directory of a project path without following links.

    With ``create_parents`` (project_write and project_mkdir), each missing
    directory component is created no-follow after the callable accepted it
    (it may refuse with a StoreError and marks the possible effect); a file
    or link component is a precise refusal. Without it a missing component
    raises FileNotFoundError for the caller to interpret.
    """
    parts = _parts(value)
    if not parts:
        raise StoreError('gateway_file_required')
    precise = create_parents is not None
    fd = os.open(root, _DIRECTORY)
    try:
        for index, part in enumerate(parts[:-1]):
            component = '/'.join(parts[:index+1])
            try:
                child = _open_directory(fd, part, component, precise)
            except FileNotFoundError:
                if not precise:
                    raise
                create_parents(component)
                try:
                    os.mkdir(part, 0o755, dir_fd=fd)
                    os.fsync(fd)
                except FileExistsError:
                    pass  # created concurrently: still opened no-follow below
                child = _open_directory(fd, part, component, precise)
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
    # Writing grants cover the project; declared scope coordinates the work.
    # Host command capability and action consent are handled separately.
    # Reviewers and read-only roles receive no writing grant.
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


# Image sniffing is by magic bytes, not extension (a renamed/extensionless
# screenshot must still be recognized). The cap matches the ACP prompt-image
# cap (acp.protocol.MAX_IMAGE_BYTES) so a worker cannot read back more image
# data through project_read than a user could ever send in through a prompt.
MAX_PROJECT_READ_IMAGE_BYTES = 10 * 1024 * 1024


def _detect_image_mime(data):
    """Sniff a supported image format from its leading magic bytes."""
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return 'image/png'
    if data.startswith(b"\xff\xd8\xff"):
        return 'image/jpeg'
    if data[:6] in (b'GIF87a', b'GIF89a'):
        return 'image/gif'
    if len(data) >= 12 and data[:4] == b'RIFF' and data[8:12] == b'WEBP':
        return 'image/webp'
    return None


def _read_file_snapshot(root, path):
    """Internal exact bytes/digest snapshot, never a public display page."""
    with _parent(root,path) as (parent,leaf):
        fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
    try:
        data, digest = _read_regular_file(fd)
    finally:
        os.close(fd)
    try:
        return dict(content=data.decode('utf-8'), sha256=digest, size_bytes=len(data))
    except UnicodeDecodeError:
        return dict(binary=True, sha256=digest, size_bytes=len(data))


def _read(root, args):
    parts = _parts(args['path'])
    if not parts:
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    else:
        with _parent(root,args['path']) as (parent,leaf):
            fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
    try:
        info = os.fstat(fd)
        from .project_files import directory_page, text_page
        if stat.S_ISDIR(info.st_mode):
            entries = sorted(n.encode('utf-8','surrogateescape').decode('utf-8','replace') for n in os.listdir(fd) if n not in PRIVATE)
            return directory_page(entries, args)
        data, digest = _read_regular_file(fd)
        mime = _detect_image_mime(data)
        if mime is not None:
            if len(data) > MAX_PROJECT_READ_IMAGE_BYTES:
                raise StoreError('gateway_image_too_large', 'path', args['path'],
                    f'an image at or under {MAX_PROJECT_READ_IMAGE_BYTES} bytes',
                    'This image exceeds the readable size cap. Read a smaller/cropped copy instead.')
            import base64
            return dict(image_base64=base64.b64encode(data).decode('ascii'),mime=mime,
                        sha256=digest,size_bytes=len(data),path=args['path'])
        try:
            body = data.decode('utf-8')
        except UnicodeDecodeError:
            return dict(sha256=digest,size_bytes=len(data),binary=True)
        return text_page(body, digest, args, len(data))
    finally:
        os.close(fd)


def _common_affix(old, new):
    """G3: length of the common leading and trailing run between two
    equal-index-addressable sequences (bytes or a list of lines), computed
    in O(n) -- one forward scan and one backward scan, never a full diff."""
    limit = min(len(old), len(new))
    prefix = 0
    while prefix < limit and old[prefix] == new[prefix]:
        prefix += 1
    remaining = limit - prefix
    suffix = 0
    while suffix < remaining and old[len(old)-1-suffix] == new[len(new)-1-suffix]:
        suffix += 1
    return prefix, suffix


def _write(root,args,effects=None, *, read_state=None, require_complete_read=False):
    from .ignore_guard import protected_content
    content = protected_content(args['content']) if _parts(args['path']) == ('.gitignore',) else args['content']
    data = content.encode('utf-8')
    def create_parent(component):
        # A missing parent means the file is absent: only a creation may add it.
        if args['expected_sha256'] != 'absent':
            raise StoreError('gateway_write_conflict', 'expected_sha256', args['expected_sha256'], 'absent',
                             f'{component} does not exist, so the file is absent. Pass expected_sha256 "absent" '
                             'to create it; project_write creates missing parent directories.')
        if effects is not None:
            effects['possible']=True
    with _parent(root,args['path'],create_parents=create_parent) as (parent,leaf):
        mode = 0o600
        previous = b''
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
                previous, observed = _read_regular_file(stream.fileno())
        if observed != args['expected_sha256']:
            raise StoreError('gateway_write_conflict')
        if observed != 'absent' and require_complete_read:
            from .project_files import require_read
            require_read(read_state, observed, complete=True)
        temporary='.cortex-write-'+uuid.uuid4().hex
        if effects is not None:
            effects['possible']=True
        fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,mode,dir_fd=parent)
        try:
            with os.fdopen(fd,'wb') as stream:
                stream.write(data); stream.flush(); os.fsync(stream.fileno())
            os.replace(temporary,leaf,src_dir_fd=parent,dst_dir_fd=parent)
            os.fsync(parent)
        finally:
            try: os.unlink(temporary,dir_fd=parent)
            except FileNotFoundError: pass
    # G3: changed_bytes/changed_lines from the common prefix/suffix only
    # (O(n), no full diff); a creation has an empty previous, so both
    # affixes are zero and changed_bytes collapses to the new size.
    previous_text_lines = previous.decode('utf-8', errors='replace').splitlines()
    new_text_lines = content.splitlines()
    byte_prefix, byte_suffix = _common_affix(previous, data)
    changed_bytes = (len(previous) - byte_prefix - byte_suffix) + (len(data) - byte_prefix - byte_suffix)
    line_prefix, line_suffix = _common_affix(previous_text_lines, new_text_lines)
    changed_lines = max(len(previous_text_lines) - line_prefix - line_suffix,
                        len(new_text_lines) - line_prefix - line_suffix)
    return dict(sha256=hashlib.sha256(data).hexdigest(),byte_count=len(data),
                previous_size_bytes=len(previous), size_bytes=len(data),
                previous_lines=len(previous_text_lines),
                new_lines=len(new_text_lines),
                drastic_shrink=bool(previous and len(data) < len(previous) / 2),
                changed_bytes=changed_bytes, changed_lines=changed_lines)


def _edit(root,args,state,effects):
    from .project_files import require_read, apply_edits, compact_diff
    with _parent(root,args['path']) as (parent,leaf):
        fd = os.open(leaf,os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,dir_fd=parent)
    try:
        data, digest = _read_regular_file(fd)
    finally:
        os.close(fd)
    require_read(state, digest, complete=False)
    try:
        before = data.decode('utf-8')
    except UnicodeDecodeError:
        raise StoreError('gateway_text_required', 'path',
                         correction='project_edit requires a UTF-8 text file.') from None
    from .ignore_guard import protected_content
    after = apply_edits(before, args['edits'])
    if _parts(args['path']) == ('.gitignore',):
        after = protected_content(after)
    if len(after.encode('utf-8')) > 16000000:
        raise StoreError('gateway_file_too_large')
    # Bound presentation work before the effect, using the exact protected bytes.
    presentation = compact_diff(before,after,args['path'])
    # All replacements are validated in memory before one durable atomic write.
    result = _write(root,dict(path=args['path'],content=after,expected_sha256=digest),effects)
    result.update(presentation)
    result['edits_applied'] = len(args['edits'])
    from .project_files import inline_size, MAX_INLINE_BYTES
    if inline_size(result) > MAX_INLINE_BYTES-2048:
        result.pop('oldText', None)
        result.pop('newText', None)
    while inline_size(result) > MAX_INLINE_BYTES-2048:
        result['diff'] = result['diff'][:len(result['diff'])//2]
        result['diff_truncated'] = True
    return result


def _command_policy(db, task_id):
    from ..configuration import access_mode
    try:
        row = db.execute("SELECT id,mode,origin FROM access_mode_events WHERE task_id=? "
                         "AND kind IN ('initialized','switched') ORDER BY id DESC LIMIT 1",
                         (task_id,)).fetchone()
    except sqlite3.Error:
        row = None
    return dict(mode=access_mode.normalize(row['mode'] if row else None),
                revision=row['id'] if row else None, origin=row['origin'] if row else 'legacy_guarded',
                denied=list(write_denials(db, task_id)))


@contextmanager
def _command_cwd(root, args, development):
    path = Path(args.get('cwd') or root)
    if not path.is_absolute() or '..' in path.parts or any(p in PRIVATE for p in path.parts):
        raise StoreError('gateway_command_cwd_denied')
    path = Path(os.path.normpath(str(path)))
    root = Path(root).resolve(strict=True)
    if path != root and root not in path.parents and not development:
        raise StoreError('gateway_command_cwd_denied', 'cwd', str(path), 'a directory inside the task working tree',
                         'Outside the project, a command in Development or Auto may use this cwd; in Guarded '
                         'use outside_command, which the user confirms.')
    from .sandbox import protected_host_paths
    if development:
        for protected in protected_host_paths(root, os.environ):
            in_task = path == root or root in path.parents
            if path == protected or (protected in path.parents and not (in_task and protected in root.parents)):
                raise StoreError('gateway_command_cwd_denied')
    fd = os.open('/', _DIRECTORY)
    try:
        for part in path.parts[1:]:
            next_fd = os.open(part, _DIRECTORY, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        yield str(path), fd
    finally:
        os.close(fd)


def _run_command_process(store, channel, launch, fds, output, effects, limit, *,
                         policy=None, env=None, cwd=None, directory=None):
    """One authority-watched process group for both command routes."""
    from .sandbox import compact_argv
    proc = None
    failure = None
    try:
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            granted, _ = _authority(db, channel)
            # A host-wide command of a writing worker stops with its grant; a
            # read-only role's never had one.
            writer = bool(granted)
            if policy and _command_policy(db, channel.task_id) != policy:
                raise StoreError('execution_policy_changed')
            if directory:
                path, fd = directory
                saved, current = os.fstat(fd), os.stat(path, follow_symlinks=False)
                if (saved.st_dev, saved.st_ino) != (current.st_dev, current.st_ino):
                    raise StoreError('gateway_command_cwd_changed')
            actual, actual_fds = compact_argv(launch, fds)
            effects['possible'] = True
            try:
                proc = subprocess.Popen(actual, stdout=output, stderr=subprocess.STDOUT,
                                        pass_fds=actual_fds, env=env or {'PATH':'/usr/bin:/bin'},
                                        cwd=cwd, start_new_session=True)
            except OSError:
                effects['possible'] = False
                raise
            finally:
                for extra in set(actual_fds) - set(fds):
                    os.close(extra)
            db.commit()
        deadline = None if limit is None else time.monotonic() + limit
        while proc.poll() is None:
            if deadline is not None and time.monotonic() >= deadline:
                failure = 'timeout'; break
            with store.connection() as db:
                try:
                    granted, _ = _authority(db, channel)
                except StoreError:
                    failure = 'authority_revoked'; break
                if policy and writer and not granted:
                    failure = 'authority_revoked'; break
                if policy and _command_policy(db, channel.task_id) != policy:
                    failure = 'execution_policy_changed'; break
            time.sleep(.05)
    finally:
        # Includes launch-transaction/observation failure after Popen succeeded.
        if proc is not None:
            if proc.poll() is None:
                import signal
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            proc.wait()
    return proc.returncode, failure


def _command(store,channel,root,scopes,args,ref,owner,effects):
    from .sandbox import as_owner, gateway_command, development_command, readonly_command, with_umask
    from . import ssh_askpass
    from .ignore_guard import snapshot, enforce
    ignore_before = snapshot(root) if scopes else None
    with store.connection() as db:
        _authority(db, channel)
        policy = _command_policy(db, channel.task_id)
    # Development and Auto: a writing worker's commands work on the whole
    # computer (one host-writable profile, named 'development' in receipts).
    # A role without a write grant (reviewer, auditor, explorer...) gets the
    # read-only host profile in every mode (sandbox.readonly_command); the
    # mode only lets it choose a cwd outside the task tree.
    from ..configuration.access_mode import command_profile, READ_ONLY_COMMAND_PROFILE
    read_only = not scopes
    development = command_profile(policy['mode']) == 'development'
    from ..engine.permissions import declared_command_deletion
    if declared_command_deletion(args['argv']):
        if not scopes:
            raise StoreError('gateway_scope_denied')
        from . import consent
        def still_allowed():
            with store.connection() as db: _authority(db, channel)
        if not consent.request(store, channel, 'command_delete',
                               consent.subject_of('command_delete', args), still_allowed=still_allowed):
            raise StoreError('deletion_not_confirmed')
    with _command_cwd(root, args, development) as (cwd, cwd_fd), tempfile.TemporaryDirectory(prefix='cortex-command-') as scratch, tempfile.TemporaryFile() as output, \
            tempfile.TemporaryDirectory(prefix='cortex-askpass-') as askpass_dir:
        # The command profile is separate from the unchanged engine jail.
        with store.connection() as db: denied=write_denials(db,channel.task_id)
        # Git sees the project's repository: read-only, or with a writing
        # grant updatable except its configuration and hooks (see sandbox).
        planted=[]
        if read_only:
            mount = readonly_command(root, scratch, with_umask(args['argv']), environ=os.environ,
                                     readonly=denied, git_guard=planted)
        elif development:
            mount = development_command(root, scratch, with_umask(args['argv']), scopes,
                                        environ=os.environ, readonly=denied, git_guard=planted)
        else:
            mount = gateway_command(root, scratch, with_umask(args['argv']), scopes,
                                    system_root=True, readonly=denied, reveal=frozenset({'.git'}),
                                    git_writable=True, git_guard=planted)
        with mount as (command,fds):
            if not development and not read_only:
                as_owner(command,os.environ)
            command[command.index('--chdir') + 1] = cwd
            if not scopes:
                from .sandbox import readonly_test_cache
                readonly_test_cache(command, scratch, os.environ)
            # Opt-in: ssh asks for a password in a desktop dialog.
            ssh_askpass.apply(command,os.environ,Path(askpass_dir))
            identity = os.fstat(cwd_fd)
            current = os.stat(cwd, follow_symlinks=False)
            if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino):
                raise StoreError('gateway_command_cwd_changed')
            code, failure = _run_command_process(
                store, channel, command, (*fds, cwd_fd), output, effects,
                clamp_timeout(args.get('timeout_seconds')), policy=policy if development else None,
                directory=(cwd, cwd_fd))
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
    return dict(tool_call_ref=ref,argv=list(args['argv']),cwd=cwd,
                profile=(READ_ONLY_COMMAND_PROFILE if read_only else 'development' if development else 'restricted'),
                access_policy=policy,
                exit_code=code,complete=failure is None,failure=failure,
                output=data.decode('utf-8',errors='replace'),large_output_id=retained['id'])


def _protected_outside(root, path):
    """True when ``path`` (or the real path its parent leads to) is, or lies
    under, a protected control-plane location; unresolvable is protected."""
    from .sandbox import control_plane_paths, protected_host_paths
    try:
        candidates={path, path.parent.resolve(strict=True)/path.name}
        if path.is_symlink() or path.exists():
            candidates.add(path.resolve(strict=True))
        # As named too: a control-plane file that does not exist yet
        # (~/.gitconfig) is planted by creating it.
        protected=[*protected_host_paths(root, os.environ),
                   *(Path(os.path.normpath(p)) for p in control_plane_paths(root, os.environ))]
    except (OSError, RuntimeError, StoreError):
        return True
    return any(candidate==guarded or guarded in candidate.parents
               for candidate in candidates for guarded in protected)


def _outside(store,channel,root,operation,args,ref,owner,effects):
    """Write a file or run a command outside the sandbox, as the user, once
    the user allows it (orchestration.consent)."""
    from . import consent
    target=Path(args['path'] if operation=='outside_write' else (args.get('cwd') or root))
    if not target.is_absolute():
        raise StoreError('outside_path_must_be_absolute','path',str(target),'an absolute path','Give an absolute path.')
    project=Path(root).resolve()
    resolved=Path(os.path.normpath(str(target)))
    if operation=='outside_write':
        # Resolve real paths too (C-12): a parent-directory symlink alias
        # can make a literally-outside path land inside the project, which
        # the normalized path alone would miss.
        candidates={resolved}
        try:
            candidates.add(target.parent.resolve(strict=True)/target.name)
        except (OSError, RuntimeError):
            pass
        if target.is_symlink() or target.exists():
            try:
                candidates.add(target.resolve(strict=True))
            except (OSError, RuntimeError):
                pass
        if any(candidate==project or project in candidate.parents for candidate in candidates):
            raise StoreError('outside_path_inside_project','path',str(target),'a path outside the project',
                             'Inside the project, write with your own tools or project_write.')
    def still_allowed():
        with store.connection() as db: _authority(db,channel)
    with store.connection() as db:
        mode=_command_policy(db,channel.task_id)['mode']
    from ..configuration.access_mode import HOST_WIDE
    # Development and Auto: an outside file write is ordinary work, except a
    # control-plane path (engine/Cortex/Paseo state, git credentials and
    # configuration), which always asks. A command outside the sandbox
    # bypasses those protections, so it keeps its card in every mode (Auto
    # answers it itself).
    routine=(operation=='outside_write' and mode in HOST_WIDE
             and not _protected_outside(root,resolved))
    if not routine and not consent.request(store,channel,operation,consent.subject_of(operation,args),still_allowed=still_allowed):
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
            from .sandbox import _SECRET_ENV
            from .ssh_askpass import host_agent_env
            environment = {k:v for k,v in os.environ.items() if not _SECRET_ENV.search(k)}
            environment.update(host_agent_env(os.environ))
            code, failure = _run_command_process(
                store, channel, args['argv'], (), output, effects,
                clamp_timeout(args.get('timeout_seconds')) or 600,
                env=environment, cwd=str(resolved))
        except OSError as error:
            if effects['possible']:
                raise  # A late error cannot be relabelled as a non-start.
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
                failure=failure,profile='outside_confirmed',output=data.decode('utf-8',errors='replace'),large_output_id=retained['id'])


def _record_gateway_refusal(store, channel, operation, error):
    """G2: a pre-authorization refusal before any mutation transaction is
    open (schema validation). Operation + error code only, never the
    rejected arguments; best-effort, so a recording failure never masks
    the real refusal raised by the caller."""
    try:
        from ..observability.events import append_event
        code = str(error)
        append_event(store, channel.task_id, 'gateway_refusal', code, 'observed',
                     attempt_id=channel.attempt_id, detail=dict(operation=operation, error=code))
    except Exception:
        pass


def _record_gateway_refusal_tx(store, db, channel, operation, error):
    """Same as :func:`_record_gateway_refusal`, on an already-open
    transaction (mutation wait, scope denial, request-key conflict)."""
    try:
        from ..observability.events import append_event_tx
        code = str(error)
        append_event_tx(db, channel.task_id, 'gateway_refusal', code, 'observed',
                         attempt_id=channel.attempt_id, detail=dict(operation=operation, error=code),
                         created_at=store._now())
    except Exception:
        pass


def _read_event_kind(result):
    if 'entries' in result:
        return 'dir'
    if 'image_base64' in result:
        return 'image'
    if result.get('binary'):
        return 'binary'
    return 'text'


def _record_project_read(store, db, channel, path, result, kind, *, error=None):
    """G1: one observability_events row per project_read call -- bounded at
    exactly one row per call (never per line/byte read), so write volume
    scales with call count, not file size. Metadata only: path, sha256,
    line range, completeness and size -- never file content."""
    try:
        from ..observability.events import append_event_tx
        detail = dict(path=path, kind=kind)
        if error is not None:
            detail['error'] = error
        else:
            detail['sha256'] = result.get('sha256')
            if kind == 'text':
                detail['start_line'] = result.get('start_line')
                detail['end_line'] = result.get('end_line')
                detail['complete'] = result.get('complete')
                detail['size_bytes'] = result.get('size_bytes')
            elif kind in ('image', 'binary'):
                detail['size_bytes'] = result.get('size_bytes')
        append_event_tx(db, channel.task_id, 'project_read', kind if error is None else 'failed', 'observed',
                         attempt_id=channel.attempt_id, detail=detail, created_at=store._now())
    except Exception:
        pass


def dispatch(store,channel,operation,args):
    try:
        validate(SCHEMAS[operation],args)
    except StoreError as error:
        _record_gateway_refusal(store,channel,operation,error)
        raise
    digest=hashlib.sha256(encoded(args).encode()).hexdigest()
    key=args.get('request_key')
    auto_key=key is None and operation in {'project_write','project_edit','project_mkdir','project_delete'}
    if auto_key:
        key='auto:'+hashlib.sha256(encoded(dict(operation=operation, payload=args)).encode()).hexdigest()
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
            try:
                result = _read(root,args)
                from .project_files import record_read
                record_read(db,channel,root,args,result)
                _record_project_read(store,db,channel,args['path'],result,_read_event_kind(result))
                db.commit()
                return result
            except FileNotFoundError:
                # _read validated the path and traversed it without following links.
                # Observe the parent with the same no-follow policy, never Path.exists().
                try:
                    with _parent(root,args['path']):
                        parent_exists=True
                except FileNotFoundError:
                    parent_exists=False
                except OSError as error:
                    raise StoreError('gateway_read_failure', 'path', args['path']) from error
                missing=dict(exists=False,path=args['path'],sha256='absent',parent_exists=parent_exists)
                _record_project_read(store,db,channel,args['path'],missing,'missing')
                db.commit()
                return missing
            except (OSError,UnicodeError) as error:
                _record_project_read(store,db,channel,args['path'],{},'failed',error='gateway_read_failure')
                db.commit()
                raise StoreError('gateway_read_failure', 'path', args['path'],
                    'a readable regular file or directory',
                    'The project read failed; no mutation was attempted. Check the path, file type and read access.') from error
        from .worker_waits import check_mutation
        if operation != 'project_git':
            try:
                check_mutation(db, channel)
            except StoreError as error:
                _record_gateway_refusal_tx(store,db,channel,operation,error)
                db.commit()
                raise
        if operation in {'project_write','project_edit','project_mkdir','project_delete'}:
            # A private path is refused before anything else (a deletion
            # there once asked the user to confirm what was then refused).
            parts=_parts(args['path'])
            # Compare the same normalized path parts as the write effect
            # itself (C-10), not the raw argument string: './secrets/key.txt'
            # and 'secrets/key.txt' must match one deny-write entry, however
            # the denial itself happens to be stored.
            normalized='/'.join(parts)
            def _normalized_denial(value):
                try:
                    return '/'.join(_parts(value))
                except StoreError:
                    return value
            denied=any(normalized==denial or normalized.startswith(denial+'/')
                       for denial in map(_normalized_denial,write_denials(db,channel.task_id)))
            if not _matches(args['path'],scopes) or denied:
                error=StoreError('gateway_scope_denied')
                _record_gateway_refusal_tx(store,db,channel,operation,error)
                db.commit()
                raise error
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
                error=StoreError('request_key_conflict')
                _record_gateway_refusal_tx(store,db,channel,operation,error)
                db.commit()
                raise error
            receipt=json.loads(row['result_json'])
            if not receipt.get('complete'):
                raise StoreError('gateway_action_outcome_unknown')
            if auto_key and receipt.get('error'):
                # Retire pre-effect auto refusals written by an older runtime
                # too. Its existing TOOL_REJECTED event remains the audit.
                db.execute('DELETE FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                           (channel.task_id,channel.channel_id,key))
            else:
                if receipt.get('error'):
                    raise StoreError(receipt['error'], **receipt.get('error_detail', {}))
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
            from .reviewer_verification import reuse
            result=reuse(store,channel,root,args,ref,owner)
            if result is None:
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
                        result = _write(root, dict(args, content=''), effects)
                        result['deleted'] = False
                        result['gitignore_guard'] = True
                    else:
                        with _parent(root,args['path']) as (parent,leaf):
                            effects['possible']=True
                            os.unlink(leaf,dir_fd=parent); os.fsync(parent)
                        result=dict(deleted=True)
                elif operation=='project_mkdir':
                    def create_parent(component):
                        effects['possible']=True
                    with _parent(root,args['path'],create_parents=create_parent) as (parent,leaf):
                        try:
                            info=os.stat(leaf,dir_fd=parent,follow_symlinks=False)
                        except FileNotFoundError:
                            effects['possible']=True
                            os.mkdir(leaf,0o755,dir_fd=parent)
                            os.fsync(parent)
                            result=dict(created=True)
                        else:
                            if not stat.S_ISDIR(info.st_mode):
                                raise StoreError('gateway_path_exists','path',args['path'],'a directory or a missing path',
                                                 'A file or symbolic link already occupies this path; nothing was '
                                                 'created. Choose another directory path.')
                            # Already a real directory: the requested state holds.
                            result=dict(created=False)
                elif operation in {'project_write','project_edit'}:
                    # Recheck deny-write policy at the effect boundary, not just dispatch.
                    if not _matches(args['path'],scopes) or _matches(args['path'],write_denials(db,channel.task_id)):
                        raise StoreError('gateway_scope_denied')
                    from .project_files import read_state
                    state = read_state(db,channel,root,args['path'])
                    if operation == 'project_write':
                        result=_write(root,args,effects,read_state=state,require_complete_read=True)
                    else:
                        result=_edit(root,args,state,effects)
                    # The channel knows the bytes it just produced: its next
                    # edit needs no re-read (engines' own edit tools behave so).
                    # Whole-file knowledge only after a full write, or an edit
                    # of a completely read file; .gitignore gains protected
                    # lines the model did not send.
                    from .project_files import record_own_write
                    complete = ((operation == 'project_write' or bool(state and state['complete']))
                                and _parts(args['path']) != ('.gitignore',))
                    record_own_write(db,channel,root,args['path'],result['sha256'],
                                     complete=complete,total_lines=result['new_lines'])
                    result['read_coverage_complete'] = complete
                    result['full_content_sha256'] = result['sha256'] if complete else None
                elif operation=='project_skill_link':
                    result=skill_links.apply(root,args,effects)
                else:
                    result=write_draft(store,db,channel,args['draft_id'],args['content'])
                db.commit()
            if operation in {'project_write','project_edit','project_mkdir'}:
                try:
                    from .skill_gate import required_before_result_submit
                    with store.connection() as db:
                        node=db.execute('SELECT n.stage,n.role FROM runtime_nodes n JOIN delegation_provisioning p '
                                        'ON n.assignment_ref=p.idempotency_key AND n.task_id=p.task_id '
                                        'WHERE n.task_id=? AND p.attempt_id=?',
                                        (channel.task_id,channel.attempt_id)).fetchone()
                    hints=required_before_result_submit(store,channel,stage=node['stage'],
                                                       role_ids=(node['role'],role['role_id'] if role else None), advisory=True,
                                                       # A created directory is never a changed file.
                                                       successful_paths=() if operation=='project_mkdir' else (args['path'],))
                except Exception:
                    # The write already committed. An advisory observation failure
                    # must never pretend the mutation failed or invite a new write.
                    result['skill_hint_unavailable']='observation_failed; result_submit still enforces the gate'
                else:
                    if hints:
                        result['required_before_result_submit']=hints
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
            if auto_key:
                # A keyless precondition refusal has no effect to replay. Keep
                # its rejection event, but let correction + identical retry run.
                db.execute('DELETE FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                           (channel.task_id,channel.channel_id,key))
            else:
                detail={name:getattr(error,name) for name in
                        ('field','received','expected','correction','violations')}
                db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                           (encoded(dict(complete=True,error=str(error),error_detail=detail)),
                            channel.task_id,channel.channel_id,key))
            event(db,channel.task_id,'TOOL_REJECTED',dict(operation=operation,tool_call_ref=ref,error=str(error)),channel.channel_id)
            db.commit()
        raise
    except (OSError,UnicodeError) as error:
        if effects['possible']:
            # The command may have run: an uncertain outcome, never "nothing changed".
            raise storage_failure(error) from error
        raise StoreError('gateway_io_failure', 'operation', operation) from error
    if operation in {'project_write','project_edit'}:
        from .project_files import bound_mutation_result
        bound_mutation_result(result)
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                    (encoded(dict(complete=True,receipt=result)),channel.task_id,channel.channel_id,key))
        event(db,channel.task_id,'TOOL_COMPLETED',dict(operation=operation,tool_call_ref=ref,result=result,
                                               **({'path':args['path']} if operation in
                                                  {'project_write','project_edit','project_delete','project_mkdir'} else {})),channel.channel_id)
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
