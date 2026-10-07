"""Hash immutable publications without following replacement symlinks."""
import hashlib
import os
import stat

from ..contracts import StoreError


def report_digest(store, task_id, filename):
    if any(not value or '/' in value or value in {'.', '..'} for value in (task_id, filename)):
        raise StoreError('invalid_report_path')
    try:
        root = os.open(store.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            directory = os.open(task_id, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=root)
            try:
                fd = os.open(filename, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
                with os.fdopen(fd, 'rb') as report:
                    before = os.fstat(report.fileno())
                    if not stat.S_ISREG(before.st_mode):
                        raise StoreError('assignment_report_unavailable')
                    digest = hashlib.file_digest(report, 'sha256').hexdigest()
                    after = os.fstat(report.fileno())
                    current = os.stat(filename, dir_fd=directory, follow_symlinks=False)
                    identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
                    if identity(before) != identity(after) or identity(after) != identity(current):
                        raise StoreError('assignment_report_changed')
                    return digest
            finally:
                os.close(directory)
        finally:
            os.close(root)
    except OSError as error:
        raise StoreError('assignment_report_unavailable') from error


# F3: the same inline bound ``command_run`` uses (gateway.py's ``_command``),
# so a small git status/diff never forces a second ``read_large_output`` call
# just to see output that already fit in this one response.
_INLINE_OUTPUT_BOUND = 65536


def project_git_evidence(store, task_id, owner_scope, root, kind):
    """Observe fixed read-only git commands, outside the model's own sandbox.

    The diff is the entire working tree against HEAD, including pre-existing
    edits. It is review context, never evidence of a particular worker's authorship.
    Untracked names appear in status; their content is read with project_read.
    """
    from pathlib import Path
    import subprocess
    import tempfile
    from .sandbox import verification_command
    from ..recovery.large_output_store import persist_output
    from ..delegation.revision import git_metadata_dirs
    root = Path(root).resolve(strict=True)
    # A .git directory, or a worktree's gitdir pointer and its common dir.
    metadata = git_metadata_dirs(str(root))
    if metadata is None:
        return dict(available=False, diagnostic='git_directory_unavailable', kind=kind,
                    evidence='context only, not a check receipt')
    # Fixed executable and argv, no pager, hooks, fsmonitor, external diff or
    # text conversion. The child's root and project are read-only, with no home.
    argv = ['/usr/bin/git', '--no-optional-locks', '-c', 'core.fsmonitor=false',
            '-c', 'core.hooksPath=/dev/null', '-c', 'core.pager=cat']
    argv += (['status', '--porcelain=v1', '--untracked-files=all'] if kind == 'status'
             else ['diff', '--no-ext-diff', '--no-textconv', 'HEAD'])
    argv += ['--', '.', ':(exclude).cortex', ':(exclude)**/.cortex/**',
             ':(exclude).codex', ':(exclude)**/.codex/**',
             ':(exclude).claude', ':(exclude)**/.claude/**']
    with tempfile.TemporaryDirectory(prefix='cortex-git-read-') as scratch:
        command = verification_command(root, scratch, argv)
        index = command.index('--chdir')
        command[index:index] = [*(arg for directory in metadata for arg in ('--ro-bind', directory, directory)),
                               '--setenv', 'GIT_DIR', metadata[0], '--setenv', 'GIT_WORK_TREE', str(root),
                               '--setenv', 'GIT_CONFIG_NOSYSTEM', '1',
                               '--setenv', 'GIT_CONFIG_GLOBAL', '/dev/null',
                               '--setenv', 'GIT_TERMINAL_PROMPT', '0']
        with tempfile.TemporaryFile() as output:
            proc = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, env={'PATH':'/usr/bin:/bin'})
            failure = None
            try:
                code = proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                proc.kill(); code = proc.wait(); failure = 'git_read_timeout'
            output.seek(0); data = output.read()
    saved = persist_output(store, task_id=task_id, owner_scope=owner_scope, source_kind='tool_result',
                           source_ref='project_git:'+kind, data=data, exit_code=code, failure=failure)
    # Inline the output up to the same bound command_run uses; large_output_id
    # always stays valid so a caller can still page the full retained bytes
    # (the remainder past this bound, or the whole output again later).
    output_truncated = len(data) > _INLINE_OUTPUT_BOUND
    return dict(available=code == 0 and failure is None, kind=kind,
                evidence='context only, not a check receipt',
                diagnostic=failure or (f'git_exit_{code}' if code else None),
                large_output_id=saved['id'], byte_count=len(data),
                output=data[:_INLINE_OUTPUT_BOUND].decode('utf-8', errors='replace'),
                output_truncated=output_truncated,
                comparison='Current project against HEAD; includes pre-existing edits. Untracked content is not in diff.')
