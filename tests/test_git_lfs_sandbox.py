"""Real LFS clean-filter behavior and read-only git sandbox regression coverage."""
import hashlib
import os
from types import SimpleNamespace
from pathlib import Path
import shutil
import subprocess

import pytest

from cortex_runtime.delegation import revision
from cortex_runtime.orchestration import artifacts, sandbox

PRODUCTION_GIT = revision.sandboxed_git
PRODUCTION_EVIDENCE = artifacts._project_git_command
ENV = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'GIT_CONFIG_NOSYSTEM': '1',
       'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_TERMINAL_PROMPT': '0'}


def run(root, *args, env=None):
    return subprocess.run(args, cwd=root, env={**ENV, **(env or {})},
                          capture_output=True, text=True, timeout=30)


@pytest.fixture
def lfs_repo(tmp_path):
    if shutil.which('git-lfs') is None:
        pytest.skip('real git-lfs is not installed')
    root = tmp_path / 'repo'
    root.mkdir()
    def git(*args):
        done = run(root, 'git', *args)
        assert done.returncode == 0, done.stderr
        return done.stdout
    git('init', '-q', '-b', 'main')
    git('lfs', 'install', '--local', '--skip-smudge')
    git('lfs', 'track', '*.bin')
    (root / 'image.bin').write_bytes(b'old image\0' * 100)
    git('add', '.')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
        'commit', '-qm', 'baseline')
    (root / 'image.bin').write_bytes(b'new image\0' * 101)
    assert git('diff', '--no-ext-diff', '--no-textconv', 'HEAD')
    return root


def snapshot(metadata):
    return {str(p.relative_to(metadata)): p.read_bytes()
            for p in metadata.rglob('*') if p.is_file()}


@pytest.mark.skipif(os.geteuid() == 0, reason='chmod permission reproduction requires a non-root user')
@pytest.mark.parametrize('linked', [False, True])
def test_real_lfs_readonly_metadata_trigger_and_scratch_fix(lfs_repo, tmp_path, linked):
    """Permission reproduction independent of unavailable nested namespaces."""
    root = lfs_repo
    if linked:
        root = tmp_path / 'linked'
        done = run(lfs_repo, 'git', 'worktree', 'add', '-qb', 'side', str(root))
        assert done.returncode == 0, done.stderr
        (root / 'image.bin').write_bytes(b'new image\0' * 101)
    metadata = Path(revision.git_metadata_dirs(str(root))[-1])
    before = snapshot(metadata)
    directories = [metadata, *(p for p in metadata.rglob('*') if p.is_dir())]
    modes = {p: p.stat().st_mode & 0o777 for p in directories}
    try:
        for p in directories:
            p.chmod(0o555)
        failed = run(root, 'git', 'diff', '--no-ext-diff', '--no-textconv', 'HEAD')
        assert failed.returncode != 0
        assert 'lfs/tmp' in failed.stderr and 'permission denied' in failed.stderr.lower()
        command = sandbox.with_git_lfs_storage(
            ['git', 'diff', '--no-ext-diff', '--no-textconv', 'HEAD'], tmp_path / 'scratch space')
        passed = run(root, *command)
        assert passed.returncode == 0, passed.stderr
        assert '-size 1000' in passed.stdout and '+size 1010' in passed.stdout
        assert '+oid sha256:' + hashlib.sha256(b'new image\0' * 101).hexdigest() in passed.stdout
        status = run(root, *sandbox.with_git_lfs_storage(
            ['git', 'status', '--porcelain=v1'], tmp_path / 'scratch space'))
        assert status.returncode == 0, status.stderr
        assert ' M image.bin' in status.stdout
        assert snapshot(metadata) == before
        assert list((tmp_path / 'scratch space' / 'lfs' / 'objects').rglob('*'))
    finally:
        for p, mode in modes.items():
            p.chmod(mode)


@pytest.mark.parametrize('linked', [False, True])
@pytest.mark.parametrize('route', ['runtime', 'evidence', 'gateway', 'development'])
@pytest.mark.requires_bwrap
def test_lfs_diff_and_status_preserve_real_metadata(lfs_repo, tmp_path, linked, route):
    root = lfs_repo
    if linked:
        worktree = tmp_path / 'linked'
        done = run(root, 'git', 'worktree', 'add', '-qb', 'side', str(worktree))
        assert done.returncode == 0, done.stderr
        root = worktree
        (root / 'image.bin').write_bytes(b'new image\0' * 101)
    metadata = revision.git_metadata_dirs(str(root))
    common = Path(metadata[-1])
    before = snapshot(common)
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    for args in (['diff', '--no-ext-diff', '--no-textconv', 'HEAD'],
                 ['status', '--porcelain=v1']):
        if route == 'runtime':
            output = PRODUCTION_GIT(str(root), args)
            assert output is not None
            output = output.decode()
        elif route == 'evidence':
            command = PRODUCTION_EVIDENCE(root, scratch, ['git', '--no-optional-locks', *args], metadata)
            done = run(root, *command)
            assert done.returncode == 0, done.stderr
            output = done.stdout
        elif route == 'development':
            with sandbox.development_command(root, scratch, ['git', '--no-optional-locks', *args], ['image.bin'],
                                             environ={**ENV, 'HOME': str(tmp_path)}) as (command, fds):
                done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=30)
            assert done.returncode == 0, done.stderr
            output = done.stdout
        else:
            with sandbox.gateway_command(root, scratch, ['git', *args], [],
                                         system_root=True, reveal=frozenset({'.git'}),
                                         git_writable=True) as (command, fds):
                done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=30)
            assert done.returncode == 0, done.stderr
            output = done.stdout
        assert 'image.bin' in output
        if args[0] == 'diff':
            assert '-size 1000' in output and '+size 1010' in output
            assert '+oid sha256:' + hashlib.sha256(b'new image\0' * 101).hexdigest() in output
        assert snapshot(common) == before


@pytest.mark.requires_bwrap
@pytest.mark.parametrize('linked', [False, True])
@pytest.mark.parametrize('route', ['runtime', 'evidence', 'gateway', 'development'])
def test_readonly_lfs_gateway_cannot_write_git_control_or_data(lfs_repo, tmp_path, linked, route):
    root = lfs_repo
    if linked:
        worktree = tmp_path / 'linked'
        assert run(root, 'git', 'worktree', 'add', '-qb', 'side', str(worktree)).returncode == 0
        root = worktree
    metadata = revision.git_metadata_dirs(str(root))
    common = Path(metadata[-1])
    before = snapshot(common)
    targets = [Path(metadata[0]) / name for name in ('HEAD', 'index')]
    targets += [common / name for name in ('config', 'refs/heads/main', 'hooks/pre-commit')]
    script = 'from pathlib import Path\n'
    script += 'for name in ' + repr([str(p) for p in targets]) + ':\n'
    script += ' try: Path(name).write_text("forbidden")\n except OSError: pass\n else: raise AssertionError(name)\n'
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    if route == 'development':
        # Narrow Development grants already keep all metadata read-only.
        with sandbox.development_command(root, scratch, ['python3', '-c', script], [],
                                         environ={**ENV, 'HOME': str(tmp_path)}) as (command, fds):
            done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=30)
        assert done.returncode == 0, done.stderr
        assert snapshot(common) == before
        return
    if route != 'gateway':
        command = PRODUCTION_EVIDENCE(root, scratch, ['python3', '-c', script], metadata)
        done = run(root, *command)
        assert done.returncode == 0, done.stderr
        assert snapshot(common) == before
        if route == 'runtime':
            # sandboxed_git accepts git args only; git's alias runs the same
            # denied-write probe without changing the fixed runtime git flags.
            import shlex
            alias = '!python3 -c ' + shlex.quote(script)
            assert PRODUCTION_GIT(str(root), ['-c', 'alias.probe=' + alias, 'probe']) == b''
            assert snapshot(common) == before
        return
    with sandbox.gateway_command(root, scratch, ['python3', '-c', script], [],
                                 system_root=True, reveal=frozenset({'.git'}),
                                 git_writable=True) as (command, fds):
        # /tmp is intentionally shared; pin the fixture tree read-only first,
        # before the explicit project/metadata/scratch overlays.
        at = command.index('--bind', command.index('--dev-bind') + 1) + 3
        command[at:at] = ['--ro-bind', str(tmp_path), str(tmp_path)]
        done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    assert snapshot(common) == before


@pytest.mark.parametrize('linked', [False, True])
def test_lfs_launch_assembly_uses_scratch_only_for_readonly_git(lfs_repo, tmp_path, monkeypatch, linked):
    if linked:
        root = tmp_path / 'linked'
        assert run(lfs_repo, 'git', 'worktree', 'add', '-qb', 'side', str(root)).returncode == 0
        lfs_repo = root
    metadata = revision.git_metadata_dirs(str(lfs_repo))
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    command = PRODUCTION_EVIDENCE(lfs_repo, scratch, ['git', 'diff', 'HEAD'], metadata)
    assert command[command.index('--') + 1:] == sandbox.with_git_lfs_storage(
        ['git', 'diff', 'HEAD'], scratch)
    for directory in metadata:
        assert ['--ro-bind', directory, directory] in [
            command[i:i + 3] for i in range(len(command))]
    with sandbox.gateway_command(lfs_repo, scratch, ['git', 'diff', 'HEAD'], [],
                                 reveal=frozenset({'.git'}), git_writable=True) as (command, _):
        assert command[command.index('--') + 1:] == sandbox.with_git_lfs_storage(
            ['git', 'diff', 'HEAD'], scratch)
    with sandbox.gateway_command(lfs_repo, scratch, ['git', 'diff', 'HEAD'], ['.'],
                                 reveal=frozenset({'.git'}), git_writable=True) as (command, _):
        assert command[command.index('--') + 1:] == ['git', 'diff', 'HEAD']
    with sandbox.development_command(lfs_repo, scratch, ['git', 'diff', 'HEAD'], ['.'],
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        assert command[command.index('--') + 1:] == sandbox.with_git_safety(
            ['git', 'diff', 'HEAD'])
        for directory in metadata:
            mounts = [command[i:i + 3] for i in range(command.index('--chdir'))
                      if command[i] in ('--bind', '--ro-bind') and command[i + 2] == directory]
            assert mounts[-1][0] == '--bind'
    calls = []
    def capture(command, **kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0, stdout=b'observed')
    monkeypatch.setattr(subprocess, 'run', capture)
    assert PRODUCTION_GIT(str(lfs_repo), ['diff', 'HEAD']) == b'observed'
    tail = calls[0][calls[0].index('--') + 1:]
    assert tail[:2] == ['/bin/sh', '-c']
    assert 'lfs.storage=' in tail[4]
    assert tail[-2:] == ['diff', 'HEAD']


@pytest.mark.parametrize('linked', [False, True])
@pytest.mark.parametrize('scopes', [['.'], ['**'], ['**/*'], ['image.bin'], []])
def test_development_lfs_assembly_keeps_granted_git_modes(tmp_path, linked, scopes):
    """Development and Auto share this builder; no namespace or LFS needed.

    Only narrow grants need LFS scratch; whole writers retain persistent objects.
    Whole-project Development/Auto grants now allow repository-data writes.
    """
    repo = tmp_path / 'repo'
    repo.mkdir()
    assert run(repo, 'git', 'init', '-q', '-b', 'main').returncode == 0
    assert run(repo, 'git', '-c', 'user.name=Test', '-c',
               'user.email=test@example.invalid', 'commit', '--allow-empty',
               '-qm', 'baseline').returncode == 0
    root = repo
    if linked:
        root = tmp_path / 'linked'
        assert run(repo, 'git', 'worktree', 'add', '-qb', 'side', str(root)).returncode == 0
    (root / 'image.bin').write_bytes(b'fixture')
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    argv = ['git', 'diff', 'HEAD']
    whole = any(scope in {'.', '**', '**/*'} for scope in scopes)
    with sandbox.development_command(root, scratch, argv, scopes,
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        mounts = [command[i:i + 3] for i in range(command.index('--chdir'))
                  if command[i] in ('--bind', '--ro-bind')]
        # Exact mount modes catch accidental re-masking after writable grants.
        for directory in revision.git_metadata_dirs(str(root)):
            expected = ['--ro-bind', '--bind'] if whole else ['--ro-bind', '--ro-bind']
            if not linked and not whole:
                expected += ['--ro-bind', '--ro-bind']
            assert [mode for mode, _, target in mounts if target == directory] == expected
        if linked:
            assert [mode for mode, _, target in mounts
                    if target == str(root / '.git')] == ['--ro-bind']
        # Config/hooks must remain read-only even when external data is writable.
        for directory in revision.git_metadata_dirs(str(root)):
            for name in ('config', 'hooks', 'commondir', 'gitdir'):
                target = Path(directory) / name
                if whole and target.exists():
                    assert ['--ro-bind', str(target), str(target)] in mounts
        tail = command[command.index('--') + 1:]
        assert tail == (sandbox.with_git_safety(argv) if whole else
                        sandbox.with_git_lfs_storage(argv, scratch, safe=True))
        assert sum('lfs.storage=' in arg for arg in tail) == (0 if whole else 1)


@pytest.mark.parametrize('scopes', [['.'], ['source.txt'], []])
def test_development_without_repository_wraps_lfs_once(tmp_path, scopes):
    root = tmp_path / 'plain'
    root.mkdir()
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    argv = ['git', 'status']
    with sandbox.development_command(root, scratch, argv, scopes,
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, _):
        assert command[command.index('--') + 1:] == sandbox.with_git_lfs_storage(argv, scratch, safe=True)


@pytest.mark.requires_bwrap
@pytest.mark.parametrize('linked', [False, True])
def test_development_whole_grant_writes_data_not_config(lfs_repo, tmp_path, linked):
    root = lfs_repo
    if linked:
        root = tmp_path / 'linked'
        assert run(lfs_repo, 'git', 'worktree', 'add', '-qb', 'side', str(root)).returncode == 0
    metadata = revision.git_metadata_dirs(str(root))
    targets = [(str(Path(metadata[0]) / 'HEAD'), True),
               (str(Path(metadata[-1]) / 'config'), False)]
    script = 'from pathlib import Path\n'
    script += 'for name, writable in ' + repr(targets) + ':\n'
    script += ' p = Path(name)\n try: p.write_bytes(p.read_bytes())\n'
    script += ' except OSError: assert not writable, name\n else: assert writable, name\n'
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    with sandbox.development_command(root, scratch, ['python3', '-c', script], ['.'],
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, fds):
        done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr


@pytest.mark.parametrize('linked', [False, True])
@pytest.mark.parametrize('namespaced', [False, pytest.param(True, marks=pytest.mark.requires_bwrap)])
def test_development_lfs_commit_retains_objects(lfs_repo, tmp_path, linked, namespaced):
    root = lfs_repo
    if linked:
        root = tmp_path / 'linked'
        assert run(lfs_repo, 'git', 'worktree', 'add', '-qb', 'side', str(root)).returncode == 0
    payload = b'persistent LFS change\\0' * 103
    (root / 'image.bin').write_bytes(payload)
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    script = ['sh', '-c', 'git add image.bin && git -c user.name=Test '
              '-c user.email=test@example.invalid commit -qm persistent']
    with sandbox.development_command(root, scratch, script, ['.'],
                                     environ={**ENV, 'HOME': str(tmp_path)}) as (command, fds):
        assert not any('lfs.storage=' in arg for arg in command[command.index('--') + 1:])
        # The non-namespace control proves wrapper/storage behavior only,
        # never filesystem enforcement. The marked counterpart executes bwrap.
        launch = command if namespaced else command[command.index('--') + 1:]
        done = subprocess.run(launch, cwd=root, env=ENV, pass_fds=fds,
                              capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    oid = hashlib.sha256(payload).hexdigest()
    common = Path(revision.git_metadata_dirs(str(root))[-1])
    assert (common / 'lfs' / 'objects' / oid[:2] / oid[2:4] / oid).read_bytes() == payload
    assert not (scratch / 'lfs').exists()
    assert 'oid sha256:' + oid in run(root, 'git', 'show', 'HEAD:image.bin').stdout


def test_lfs_wrapper_preserves_environment_config(tmp_path):
    argv = sandbox.with_git_lfs_storage(['git', 'config', '--get', 'test.kept'], tmp_path / "scratch 'quote")
    done = run(tmp_path, *argv, env={'GIT_CONFIG_PARAMETERS': "'test.kept=value with spaces'",
                                   'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'other.kept',
                                   'GIT_CONFIG_VALUE_0': 'also kept'})
    assert done.returncode == 0, done.stderr
    assert done.stdout.strip() == 'value with spaces'
    argv = sandbox.with_git_lfs_storage(['git', 'config', '--get', 'other.kept'], tmp_path / 'scratch')
    done = run(tmp_path, *argv, env={'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'other.kept',
                                   'GIT_CONFIG_VALUE_0': 'also kept'})
    assert done.returncode == 0 and done.stdout.strip() == 'also kept'
    argv = sandbox.with_git_lfs_storage(['git', 'config', '--get', 'lfs.storage'], tmp_path / "scratch 'quote")
    done = run(tmp_path, *argv)
    assert done.returncode == 0, done.stderr
    assert done.stdout.strip() == str(tmp_path / "scratch 'quote" / 'lfs')
