"""A writing worker's sandboxed git updates its repository; git configuration,
hooks, the Cortex store and direct file tools stay protected; tool caches in
the project are writable."""
import os
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration.gateway import _write, _parts
from cortex_runtime.orchestration.sandbox import gateway_command, engine_jail, git_metadata_mounts

pytestmark = [pytest.mark.engine_jail,
              pytest.mark.skipif(shutil.which('bwrap') is None or shutil.which('git') is None,
                                 reason='bubblewrap and git are required')]

IDENTITY = ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid']
ENV = {'PATH': '/usr/bin:/bin', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
       'GIT_AUTHOR_NAME': 'Test', 'GIT_AUTHOR_EMAIL': 'test@example.invalid',
       'GIT_COMMITTER_NAME': 'Test', 'GIT_COMMITTER_EMAIL': 'test@example.invalid'}


def git(*args, cwd):
    return subprocess.run(['git', *IDENTITY, *args], cwd=cwd, check=True, capture_output=True, text=True,
                          env={**ENV, 'HOME': str(cwd)}).stdout


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / 'project'
    root.mkdir()
    git('init', '-q', '-b', 'main', cwd=root)
    (root / 'a.txt').write_text('one\n')
    (root / '.cortex').mkdir()
    (root / '.cortex' / 'cortex.sqlite3').write_text('store\n')
    (root / '.gitignore').write_text('.cortex/\n')
    git('add', '.', cwd=root)
    git('commit', '-qm', 'baseline', cwd=root)
    return root


def sandboxed(root, scratch, script, scopes=('.',), **kwargs):
    scratch.mkdir(exist_ok=True)
    env = [arg for name, value in ENV.items() for arg in ('--setenv', name, value)]
    with gateway_command(root, scratch, ['/bin/sh', '-c', script], list(scopes), system_root=True,
                         reveal=frozenset({'.git'}), git_writable=True, **kwargs) as (command, fds):
        at = command.index('--chdir')
        command[at:at] = env
        # The test tree lives under the shared, writable /tmp: make it
        # read-only like any other host path, so only the sandbox's own
        # grants (project, git metadata, scratch) are writable in it.
        at = next(i for i in range(len(command)) if command[i:i + 3] == ['--bind', '/tmp', '/tmp']) + 3
        command[at:at] = ['--ro-bind', str(scratch.parent), str(scratch.parent)]
        return subprocess.run(command, pass_fds=fds, capture_output=True, text=True,
                              env={'PATH': '/usr/bin:/bin'}, timeout=60)


@pytest.mark.requires_bwrap
def test_sandboxed_git_commits_branches_and_stashes(tmp_path, repo):
    done = sandboxed(repo, tmp_path / 'scratch', '''set -e
        echo two >> a.txt
        git add a.txt && git commit -qm second
        git switch -qc feature && echo three >> a.txt && git stash -q && git stash pop -q
        git commit -qam third && git switch -q main && git merge -q --no-edit feature
        git log --format=%s | tr '\\n' ' '
    ''')
    assert done.returncode == 0, done.stdout + done.stderr
    assert done.stdout.split() == ['third', 'second', 'baseline']
    assert git('log', '--format=%s', cwd=repo).split() == ['third', 'second', 'baseline']
    assert git('status', '--porcelain', cwd=repo) == ''


@pytest.mark.requires_bwrap
def test_git_hooks_config_and_pointers_stay_protected(tmp_path, repo):
    hooks = repo / '.git' / 'hooks'
    hooks.mkdir(exist_ok=True)
    config = (repo / '.git' / 'config').read_text()
    planted = []
    done = sandboxed(repo, tmp_path / 'scratch', '''
        echo 'touch /tmp/pwned' > .git/hooks/post-commit && echo HOOK-WRITTEN
        git config core.fsmonitor 'touch /tmp/pwned' && echo CONFIG-WRITTEN
        echo evil >> .git/config && echo CONFIG-APPENDED
        echo ../evil > .git/commondir; mkdir -p .git/remotes
        echo ok > .git/objects/probe && echo DATA-WRITTEN
        ls -A .cortex | wc -l
        echo bad > .cortex/new && echo CORTEX-WRITTEN
        true
    ''', git_guard=planted)
    out = done.stdout
    assert 'DATA-WRITTEN' in out
    for marker in ('HOOK-WRITTEN', 'CONFIG-WRITTEN', 'CONFIG-APPENDED', 'CORTEX-WRITTEN'):
        assert marker not in out, out + done.stderr
    assert out.split()[-1] == '0'  # the store is hidden
    assert not (hooks / 'post-commit').exists()
    assert (repo / '.git' / 'config').read_text() == config
    # Created where none existed, then removed by the guard and reported.
    assert not (repo / '.git' / 'commondir').exists() and not (repo / '.git' / 'remotes').exists()
    assert sorted(Path(p).name for p in planted) == ['commondir', 'remotes']
    assert (repo / '.cortex' / 'cortex.sqlite3').read_text() == 'store\n'
    assert not (repo / '.cortex' / 'new').exists()


@pytest.mark.requires_bwrap
def test_linked_worktree_commits_through_its_common_directory(tmp_path, repo):
    worktree = tmp_path / 'wt'
    git('worktree', 'add', '-q', '-b', 'side', str(worktree), cwd=repo)
    other = tmp_path / 'other-wt'
    git('worktree', 'add', '-q', '-b', 'other', str(other), cwd=repo)
    writable, readonly, _ = git_metadata_mounts(worktree.resolve())
    common = (repo / '.git').resolve()
    assert writable == [common / 'worktrees' / 'wt', common]
    assert common / 'worktrees' / 'other-wt' in readonly and common / 'config' in readonly
    done = sandboxed(worktree, tmp_path / 'scratch', '''set -e
        echo side >> a.txt && git commit -qam 'from worktree'
        git log -1 --format=%s
        echo x > ../other-wt/a.txt 2>/dev/null && echo OTHER-TREE-WRITTEN || true
        echo x > COMMON/worktrees/other-wt/HEAD 2>/dev/null && echo OTHER-META-WRITTEN || true
        echo x > COMMON/hooks/pre-commit 2>/dev/null && echo HOOK-WRITTEN || true
    '''.replace('COMMON', str(common)))
    assert done.returncode == 0, done.stdout + done.stderr
    assert 'from worktree' in done.stdout
    for marker in ('OTHER-TREE-WRITTEN', 'OTHER-META-WRITTEN', 'HOOK-WRITTEN'):
        assert marker not in done.stdout
    assert git('log', '-1', '--format=%s', 'side', cwd=repo).strip() == 'from worktree'


@pytest.mark.requires_bwrap
def test_without_a_writing_grant_git_is_read_only(tmp_path, repo):
    head = (repo / '.git' / 'HEAD').read_text()
    done = sandboxed(repo, tmp_path / 'scratch', '''
        git log -1 --format=%s
        echo two >> a.txt 2>/dev/null; git commit -qam no 2>/dev/null && echo COMMITTED
        true
    ''', scopes=())
    assert done.stdout.split() == ['baseline']
    assert (repo / '.git' / 'HEAD').read_text() == head


@pytest.mark.requires_bwrap
def test_tool_state_directories_in_the_project_are_writable(tmp_path, repo):
    done = sandboxed(repo, tmp_path / 'scratch', '''set -e
        for dir in .venv/lib node_modules/.cache/tool .pytest_cache __pycache__ .mypy_cache target/debug build; do
            mkdir -p "$dir" && echo ok > "$dir/state"
        done
        echo written
    ''')
    assert done.stdout.strip() == 'written', done.stderr
    for directory in ('.venv/lib', 'node_modules/.cache/tool', '.pytest_cache', '__pycache__', '.mypy_cache',
                      'target/debug', 'build'):
        assert (repo / directory / 'state').read_text() == 'ok\n'


def test_direct_file_tools_never_reach_git_or_cortex(tmp_path, repo):
    for path in ('.git/hooks/post-commit', '.git/config', '.git/HEAD', '.cortex/cortex.sqlite3', 'sub/.git/x'):
        with pytest.raises(StoreError, match='gateway_path_denied'):
            _parts(path)
        with pytest.raises(StoreError, match='gateway_path_denied'):
            _write(repo, dict(path=path, content='bad', expected_sha256='absent'))
    assert not (repo / '.git' / 'hooks' / 'post-commit').exists()


@pytest.mark.requires_bwrap
def test_engine_jail_native_shell_commits_but_cannot_touch_hooks(tmp_path, repo):
    scratch = tmp_path / 'jail'
    scratch.mkdir()
    (tmp_path / 'home').mkdir()
    env = {**ENV, 'HOME': str(tmp_path / 'home')}
    script = '''
        echo two >> a.txt && git commit -qam jailed && echo COMMITTED
        echo x > .git/hooks/pre-commit 2>/dev/null && echo HOOK-WRITTEN
        true
    '''
    with engine_jail(repo, scratch, ['sh', '-c', script], ['.'], env=env) as (command, fds):
        done = subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                              capture_output=True, text=True, timeout=60)
    assert 'COMMITTED' in done.stdout and 'HOOK-WRITTEN' not in done.stdout, done.stdout + done.stderr
    assert git('log', '-1', '--format=%s', cwd=repo).strip() == 'jailed'


@pytest.mark.requires_bwrap
def test_command_run_commits_and_reports_planted_git_configuration(tmp_path, repo, monkeypatch):
    """The real command_run path (owner shell, retained output): a commit
    lands; a planted repository pointer is removed and fails the command."""
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.orchestration import gateway
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store
    from cortex_runtime.trusted_channel import resolve_channel
    home = tmp_path / 'home'
    home.mkdir()
    monkeypatch.setenv('HOME', str(home))
    monkeypatch.setenv('XDG_CONFIG_HOME', str(home / '.config'))
    for name, value in ENV.items():
        monkeypatch.setenv(name, value)
    (tmp_path / 'runtime').mkdir()
    store = Store(tmp_path / 'runtime' / '.cortex', project_root=tmp_path / 'runtime')
    boot = bootstrap_task(store, SourceEvent(origin='git-test', event_id='one'), 'Implement', 'owner')
    channel = resolve_channel(store, boot.channel_id, boot.secret)
    channel = SimpleNamespace(task_id=channel.task_id, attempt_id='xa_test', role='worker')
    monkeypatch.setattr(gateway, '_authority', lambda db, channel: (['.'], 'owner'))

    def run(argv, ref):
        return gateway._command(store, channel, str(repo), ['.'], dict(argv=argv, request_key=ref), ref, 'owner', {})

    done = run(['/bin/sh', '-c', 'echo two >> a.txt && git commit -qam via-command-run && git log -1 --format=%s'],
               'commit')
    assert done['exit_code'] == 0 and done['failure'] is None, done['output']
    assert done['output'].strip() == 'via-command-run'
    planted = run(['/bin/sh', '-c', 'echo ../evil > .git/commondir'], 'plant')
    assert planted['failure'] == 'git_configuration_protected' and not planted['complete']
    assert 'commondir' in planted['output']
    assert not (repo / '.git' / 'commondir').exists()
    assert git('log', '-1', '--format=%s', cwd=repo).strip() == 'via-command-run'
