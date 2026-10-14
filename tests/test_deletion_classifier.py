"""Lexical deletion classification of brokered command argv."""
import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.mcp_tools import _OPERATION_DESCRIPTIONS
from cortex_runtime.engine.permissions import declared_command_deletion
from cortex_runtime.orchestration import gateway


DELETIONS = [
    ['unlink', 'file'], ['/usr/bin/unlink', 'file'], ['rmdir', 'dir'], ['rmdir', '-p', 'a/b'],
    ['find', '.', '-name', '*.pyc', '-delete'], ['find', '.', '-ok', 'rm', '{}', ';'],
    ['find', '.', '-okdir', 'unlink', '{}', ';'], ['find', '.', '-exec', 'sh', '-c', 'rmdir "$1"', 'sh', '{}', ';'],
    ['git', 'clean', '-fdx'], ['git', 'clean', '-f'], ['git', 'clean', '-e', '*.log', '-f'],
    ['git', '-C', 'repo', 'clean', '-fd'], ['git', '-c', 'core.x=y', 'clean', '-f'],
    ['git', '--git-dir=.git', 'clean', '--force'], ['git', 'rm', 'file'], ['git', 'rm', '-r', 'dir'],
    ['shred', '-u', 'file'], ['shred', '-zu', 'file'], ['shred', '--remove', 'file'],
    ['shred', '--remove=wipesync', 'file'], ['shred', '-n', '3', '-u', 'file'],
    ['rsync', '-a', '--delete', 'src/', 'dst/'], ['rsync', '-a', '--delete-after', 'src/', 'dst/'],
    ['rsync', '--delete-missing-args', 'a', 'b'], ['rsync', '--remove-source-files', 'a', 'b'],
    ['rsync', '--del', 'a', 'b'],
    ['timeout', '5', 'rm', 'file'], ['timeout', '-s', 'KILL', '5', 'rm', 'file'],
    ['timeout', '--kill-after=1', '5', 'unlink', 'file'], ['nice', 'rm', 'file'], ['nice', '-n', '10', 'rm', 'file'],
    ['nice', '-5', 'rm', 'file'], ['ionice', '-c', '3', 'rm', 'file'], ['nohup', 'rm', 'file'],
    ['setsid', '-f', 'rm', 'file'], ['stdbuf', '-oL', 'rm', 'file'], ['stdbuf', '-o', 'L', 'rm', 'file'],
    ['time', '-v', 'rm', 'file'], ['/usr/bin/time', '-f', '%e', 'rm', 'file'], ['doas', '-u', 'root', 'rm', 'file'],
    ['command', 'rm', 'file'], ['command', '-p', 'rm', 'file'], ['busybox', 'rm', 'file'],
    ['busybox', 'sh', '-c', 'rm file'], ['sudo', 'timeout', '5', 'nice', 'git', 'clean', '-fd'],
    ['env', 'LABEL=x', 'timeout', '5', 'find', '.', '-delete'], ['xargs', '-0', 'unlink'],
    ['sh', '-c', 'find . -delete'], ['bash', '-c', 'echo before && git clean -fdx'],
    ['sh', '-c', 'timeout 5 rsync -a --delete a/ b/'], ['sh', '-c', 'if true; then rmdir dir; fi'],
]

NOT_DELETIONS = [
    ['git', 'clean', '-n'], ['git', 'clean', '-dn'], ['git', 'clean', '--dry-run', '-f'],
    ['git', 'rm', '--cached', 'file'], ['git', 'rm', '-n', 'file'], ['git', 'status'], ['git', 'log', '--', 'clean'],
    ['git', '-C', 'clean', 'status'], ['shred', 'file'], ['shred', '-n', '3', 'file'], ['shred', '--', '-u'],
    ['rsync', '-a', 'src/', 'dst/'], ['rsync', '--max-delete=5', 'a', 'b'], ['find', '.', '-name', 'delete'],
    ['find', '.', '-exec', 'cat', '{}', ';'], ['timeout', '5', 'sleep', '1'], ['nice', 'make'],
    ['ionice', '-p', '123'], ['command', '-v', 'rm'], ['command', '-V', 'unlink'], ['busybox', '--list'],
    ['doas', '-C', 'doas.conf', 'rm', 'file'], ['echo', 'git clean -fdx'], ['echo', 'find . -delete'],
    ['printf', '%s', 'unlink file'], ['sh', '-c', "echo 'rmdir dir'"], ['python3', '-c', "import shutil"],
    ['make', 'clean'], ['npm', 'run', 'clean'],
]


@pytest.mark.parametrize('argv', DELETIONS)
def test_deletion_commands_and_wrappers_are_recognized(argv):
    assert declared_command_deletion(argv)


@pytest.mark.parametrize('argv', NOT_DELETIONS)
def test_non_deleting_or_literal_commands_are_not(argv):
    assert not declared_command_deletion(argv)


@pytest.mark.parametrize('argv', [['find', '.', '-delete'], ['unlink', 'f'], ['git', 'clean', '-fd'],
                                  ['timeout', '5', 'rm', 'f'], ['rsync', '--delete', 'a/', 'b/']])
def test_read_only_role_is_refused_a_newly_recognized_deletion(tmp_path, monkeypatch, argv):
    from test_command_run_arguments import _worker
    from cortex_runtime.trusted_channel import resolve_channel
    root, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    monkeypatch.setattr(gateway, '_authority', lambda db, ch: ([], 'read-only'))
    monkeypatch.setattr(gateway.subprocess, 'Popen', lambda *a, **kw: pytest.fail('launched'))
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        gateway.dispatch(store, worker, 'command_run', dict(argv=argv, request_key='delete'))


@pytest.mark.parametrize('argv', [['find', '.', '-delete'], ['git', 'clean', '-fd'], ['nice', 'unlink', 'f']])
def test_writer_needs_consent_for_a_newly_recognized_deletion(tmp_path, monkeypatch, argv):
    from test_command_run_arguments import _worker
    from cortex_runtime.orchestration import consent
    from cortex_runtime.trusted_channel import resolve_channel
    root, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    asked = []
    monkeypatch.setattr(consent, 'request', lambda *a, **kw: asked.append(a[2]) or False)
    monkeypatch.setattr(gateway.subprocess, 'Popen', lambda *a, **kw: pytest.fail('launched'))
    with pytest.raises(StoreError, match='deletion_not_confirmed'):
        gateway.dispatch(store, worker, 'command_run', dict(argv=argv, request_key='delete'))
    assert asked == ['command_delete']


def test_command_run_description_states_lexical_coverage():
    text = _OPERATION_DESCRIPTIONS['command_run']
    assert 'Explicit deletion commands require consent' not in text
    for named in ('unlink', 'find -delete', 'git clean', 'rsync --delete', 'not recognized'):
        assert named in text
