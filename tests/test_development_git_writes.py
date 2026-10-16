"""Development/Auto repository data grants; assembly never needs namespaces."""
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from cortex_runtime.delegation.revision import git_metadata_dirs
from cortex_runtime.orchestration import sandbox

ENV = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'GIT_CONFIG_NOSYSTEM': '1',
       'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_AUTHOR_NAME': 'Test',
       'GIT_AUTHOR_EMAIL': 'test@example.invalid', 'GIT_COMMITTER_NAME': 'Test',
       'GIT_COMMITTER_EMAIL': 'test@example.invalid'}
GUARDED = ('config', 'config.worktree', 'commondir', 'gitdir', 'hooks', 'remotes', 'branches', 'info')
pytestmark = pytest.mark.skipif(shutil.which('git') is None or shutil.which('bwrap') is None,
                                reason='git and bubblewrap are required')


def git(root, *args):
    done = subprocess.run(['git', *args], cwd=root, env=ENV,
                          capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    return done.stdout.strip()


@pytest.fixture(params=['ordinary', 'linked', 'submodule', 'linked_submodule'])
def checkout(tmp_path, request):
    main = tmp_path / 'main'
    main.mkdir()
    git(main, 'init', '-qb', 'main')
    (main / 'file.txt').write_text('baseline\n')
    git(main, 'add', '.')
    git(main, 'commit', '-qm', 'baseline')
    root = main
    if request.param in ('submodule', 'linked_submodule'):
        source = tmp_path / 'source'
        git(tmp_path, 'clone', '-q', str(main), str(source))
        git(main, '-c', 'protocol.file.allow=always', 'submodule', 'add', '-q',
            str(source), 'child')
        git(main, 'commit', '-qam', 'submodule')
        # Submodule roots are deliberately rejected by project_storage;
        # exercise submodule commands inside the supported superproject.
        root = main
    if request.param in ('linked', 'linked_submodule'):
        root = tmp_path / 'linked'
        git(main, 'worktree', 'add', '-qb', 'side', str(root))
        git(main, 'worktree', 'add', '-qb', 'other', str(tmp_path / 'other'))
        if request.param == 'linked_submodule':
            git(root, '-c', 'protocol.file.allow=always', 'submodule', 'update', '--init', '-q')
    (root / '.cortex').mkdir()
    (root / '.cortex' / 'private').write_text('private')
    return root


def mounts(command):
    """Normalize fd sources while they remain open; retain complete ordering."""
    return [(command[i], os.readlink(command[i + 1])
             if command[i + 1].startswith('/proc/self/fd/') else command[i + 1], command[i + 2])
            for i in range(command.index('--chdir'))
            if command[i] in ('--bind', '--ro-bind')]


@pytest.mark.parametrize('scopes', [('.',), ('**',), ('**/*',), ('file.txt',), ('*',), ()])
def test_exact_development_git_mounts(checkout, tmp_path, scopes):
    root = checkout
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    metadata = [Path(p) for p in git_metadata_dirs(str(root))]
    whole = scopes in [('.',), ('**',), ('**/*',)]
    with sandbox.development_command(root, scratch, ['git', 'status'], scopes,
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        actual = mounts(command)
        for directory in metadata:
            modes = [mode for mode, _, target in actual if target == str(directory)]
            expected = ['--ro-bind', '--bind'] if whole else ['--ro-bind', '--ro-bind']
            if root / '.git' == directory and not whole:
                expected += ['--ro-bind', '--ro-bind']
            assert modes == expected
        # No additional metadata path receives a writable bind.
        git_binds = {(source, target) for mode, source, target in actual if mode == '--bind'
                     and any(str(d) == target or str(d) + '/' in target for d in metadata)}
        expected_writable = metadata + [d / 'modules' / 'child' for d in metadata
                                        if (d / 'modules' / 'child').exists()]
        assert git_binds == ({(str(d), str(d)) for d in expected_writable} if whole else set())
        if whole:
            writable, protected, watched = sandbox.git_metadata_mounts(root, development=True)
            assert writable == expected_writable
            expected_protected = {d / name for d in expected_writable
                                  for name in GUARDED
                                  if (d / name).exists()}
            containers = {d / name for d in expected_writable for name in ('modules', 'worktrees')
                          if (d / name).exists()}
            expected_protected.update(containers)
            assert set(protected) == expected_protected
            # Per-repository guards follow data grants; modules containers
            # precede the child data grants (which must remain writable).
            last_write = max(i for i, (mode, _, target) in enumerate(actual)
                             if mode == '--bind' and target in {str(d) for d in writable})
            assert {Path(target) for mode, _, target in actual
                    if mode == '--ro-bind' and Path(target) in expected_protected} == expected_protected
            for container in expected_protected:
                children = [d for d in writable if container in d.parents]
                if children:
                    position = actual.index(('--ro-bind', str(container), str(container)))
                    assert all(position < actual.index(('--bind', str(d), str(d))) for d in children)
            for path in expected_protected - containers:
                assert ('--ro-bind', str(path), str(path)) in actual[last_write + 1:]
            assert all(not p.exists() for p in watched)
            assert all(d / 'config.worktree' in watched for d in metadata)
        assert ['--tmpfs', str(root / '.cortex'), '--remount-ro', str(root / '.cortex')] in [
            command[i:i + 4] for i in range(command.index('--chdir'))]
        assert command[command.index('--') + 1:] == (
            sandbox.with_git_safety(['git', 'status']) if whole else
            sandbox.with_git_lfs_storage(['git', 'status'], scratch, safe=True))


def test_readonly_profile_keeps_metadata_readonly(checkout, tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    with sandbox.readonly_command(checkout, scratch, ['git', 'status'],
                                  environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        actual = mounts(command)
        for directory in git_metadata_dirs(str(checkout)):
            expected = ['--ro-bind'] * (3 if Path(directory) == checkout / '.git' else 2)
            assert [mode for mode, _, target in actual if target == directory] == expected
        assert not any(mode == '--bind' and '.git' in target for mode, _, target in actual)


def test_development_guard_watches_missing_configuration(checkout):
    _, protected, watched = sandbox.git_metadata_mounts(checkout, development=True)
    for directory in map(Path, git_metadata_dirs(str(checkout))):
        for name in GUARDED:
            path = directory / name
            assert path in (protected if path.exists() else watched)


def test_safe_git_defaults_preserve_other_config(tmp_path):
    argv = sandbox.with_git_lfs_storage(
        ['git', 'config', '--get-regexp', '^(core.hookspath|core.fsmonitor|test.kept)$'],
        tmp_path, safe=True)
    done = subprocess.run(argv, cwd=tmp_path, env={**ENV,
        'GIT_CONFIG_PARAMETERS': "'core.hooksPath=unsafe' 'core.fsmonitor=unsafe' 'test.kept=value'"},
        capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    assert done.stdout.splitlines()[-2:] == ['core.hookspath /dev/null', 'core.fsmonitor false']
    assert 'test.kept value' in done.stdout


def test_existing_worktree_config_is_mounted_readonly(checkout, tmp_path):
    for directory in map(Path, git_metadata_dirs(str(checkout))):
        (directory / 'config.worktree').write_text('# existing configuration\n')
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    with sandbox.development_command(checkout, scratch, ['git', 'status'], ['.'],
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        actual = mounts(command)
        for directory in git_metadata_dirs(str(checkout)):
            path = str(Path(directory) / 'config.worktree')
            assert ('--ro-bind', path, path) in actual


def test_git_safety_defaults_disable_hooks_and_fsmonitor(checkout, tmp_path):
    hooks = checkout / 'custom-hooks'
    hooks.mkdir()
    marker = checkout / 'executed'
    hook = hooks / 'pre-commit'
    hook.write_text('#!/bin/sh\ntouch "' + str(marker) + '"\nexit 91\n')
    hook.chmod(0o755)
    git(checkout, 'config', 'core.hooksPath', str(hooks))
    monitor = checkout / 'fsmonitor'
    monitor.write_text('#!/bin/sh\ntouch "' + str(marker) + '"\nexit 92\n')
    monitor.chmod(0o755)
    git(checkout, 'config', 'core.fsmonitor', str(monitor))
    argv = sandbox.with_git_safety(['git', 'commit', '--allow-empty', '-qm', 'safe'])
    done = subprocess.run(argv, cwd=checkout, env=ENV, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    assert not marker.exists()


@pytest.mark.requires_bwrap
def test_development_git_operations_and_config_protection(checkout, tmp_path):
    root = checkout
    metadata = [Path(p) for p in git_metadata_dirs(str(root))]
    hook = metadata[-1] / 'hooks' / 'pre-commit'
    hook.parent.mkdir(exist_ok=True)
    hook.write_text('#!/bin/sh\nexit 91\n')
    hook.chmod(0o755)
    before = (metadata[-1] / 'config').read_bytes()
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    probe = """import errno, os, pathlib, sys
config = pathlib.Path(sys.argv[1])
replacement = config.with_name('config.rename-probe')
replacement.write_bytes(config.read_bytes())
try:
    os.replace(replacement, config)
except OSError as error:
    assert error.errno in (errno.EBUSY, errno.EROFS, errno.EACCES), error
else:
    raise AssertionError('rename over protected config succeeded')
finally:
    replacement.unlink(missing_ok=True)
"""
    script = """set -eu
echo changed >> file.txt
git add file.txt
git commit -qm changed
git switch -qc feature
echo stashed >> file.txt
git stash -q
git stash pop -q
git commit -qam stashed
git switch -q -
git merge -q --no-edit feature
git rebase -q HEAD~1
if test -d child; then
    echo changed >> child/file.txt
    git -C child add file.txt
    git -C child commit -qm child-changed
    if git -C child config x.y z; then exit 75; fi
fi
if git config x.y z; then exit 71; fi
if echo bad > "$1/hooks/pre-commit"; then exit 72; fi
if echo bad >> "$1/config"; then exit 73; fi
if echo bad > "$1/info/exclude"; then exit 74; fi
python3 -c "$2" "$1/config"
"""
    with sandbox.development_command(root, scratch, ['/bin/sh', '-c', script, 'sh',
                                      str(metadata[-1]), probe], ['.'],
                                      environ={**ENV, 'HOME': str(tmp_path)}) as (command, fds):
        done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    assert git(root, 'log', '-1', '--format=%s') == 'stashed'
    assert (metadata[-1] / 'config').read_bytes() == before
    assert hook.read_text() == '#!/bin/sh\nexit 91\n'
