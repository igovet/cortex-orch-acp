"""Deny-write paths and outside-write protection:

- Only an explicit ``deny-write: <path>`` steering directive is ever
  recognized as a denial; natural-language phrasing and wildcard/masked
  paths never are. A wildcard entry an older runtime already stored still
  guards project writes, matched conservatively.
- Deny-write matching normalizes the write-effect path the same way the
  gateway's own path validation does, so './secrets/key.txt' and
  'secrets/key.txt' denote one denial regardless of which form is stored or
  requested.
- ``outside_write``'s inside-project refusal follows real paths, so a
  parent-directory symlink alias cannot land inside the project unnoticed,
  and the project's git metadata (including an external linked worktree's
  gitdir/commondir) is protected control-plane state.
"""
import os
import subprocess

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.policy import parse_denials
from cortex_runtime.orchestration.sandbox import control_plane_paths
from cortex_runtime.orchestration.state import event
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture


@pytest.fixture
def worker(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='deny-write-tests', event_id='one'), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    channel, _ = provision_fixture(store, coordinator)
    return store, channel, coordinator, tmp_path


def call(worker, operation, **args):
    return mcp_tools.dispatch(worker[0], worker[1], operation, args)


def _deny(worker, *paths):
    store, channel, _, _ = worker
    with store.connection() as db:
        event(db, channel.task_id, 'USER_STEERING', dict(deny_write=list(paths)), 'user-denial')
        event(db, channel.task_id, 'STEERING_APPLIED', {}, 'user-denial')
        db.commit()


# --- Deny-write matching uses the same normalized path parts as the effect.

def test_deny_write_refuses_a_dot_slash_prefixed_write_path(worker):
    store, channel, coordinator, root = worker
    (root / 'secrets').mkdir()
    (root / 'secrets' / 'key.txt').write_text('s')
    _deny(worker, 'secrets/key.txt')
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        call(worker, 'project_write', path='./secrets/key.txt', expected_sha256='absent', content='x')
    assert (root / 'secrets' / 'key.txt').read_text() == 's'


def test_deny_write_matches_even_when_the_stored_denial_has_a_dot_prefix(worker):
    """A denial recorded with a redundant './' prefix (older payload shape)
    still refuses the equivalent normalized effect path."""
    store, channel, coordinator, root = worker
    (root / 'secrets').mkdir()
    (root / 'secrets' / 'key.txt').write_text('s')
    _deny(worker, './secrets/key.txt')
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        call(worker, 'project_write', path='secrets/key.txt', expected_sha256='absent', content='x')


def test_deny_write_still_refuses_an_exact_match(worker):
    store, channel, coordinator, root = worker
    (root / 'file.txt').write_text('s')
    _deny(worker, 'file.txt')
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        call(worker, 'project_write', path='file.txt', expected_sha256='absent', content='x')


@pytest.mark.parametrize('stored,path', [
    ('secrets/*', 'secrets/key.txt'),
    ('secrets/*', './secrets/key.txt'),
    ('secrets/**', 'secrets/deep/key.txt'),
    ('*.env', 'config/prod.env'),
    ('config?.yaml', 'config1.yaml'),
    ('**', 'anything.txt'),
])
@pytest.mark.parametrize('operation', ['project_write', 'project_mkdir'])
def test_a_stored_legacy_wildcard_denial_still_guards_project_writes(worker, stored, path, operation):
    """Denials stored by an older runtime's parser may hold wildcards; new
    input never adds one, but an existing one keeps refusing writes."""
    store, channel, coordinator, root = worker
    _deny(worker, stored)
    args = dict(path=path, expected_sha256='absent', content='x') if operation == 'project_write' else dict(path=path)
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        call(worker, operation, **args)
    assert not (root / path).exists()


def test_a_stored_legacy_wildcard_denial_leaves_unmatched_paths_writable(worker):
    store, channel, coordinator, root = worker
    _deny(worker, 'secrets/*', '*.env')
    call(worker, 'project_write', path='src/app.py', expected_sha256='absent', content='x')
    assert (root / 'src' / 'app.py').read_text() == 'x'


def test_write_denied_matches_exact_and_legacy_entries():
    from cortex_runtime.orchestration.gateway import write_denied
    assert write_denied('./a/b.txt', ['a'])
    assert write_denied('a/b.txt', ['./a/b.txt'])
    assert write_denied('anything.txt', ['.'])
    assert write_denied('docs/x.md', ['docs/*.md'])
    assert not write_denied('ab/c.txt', ['a'])
    assert not write_denied('src/x.py', ['docs/*'])


# --- Only an explicit 'deny-write: <path>' directive is recognized.

def test_parse_denials_accepts_explicit_directive_quoted_or_not():
    assert parse_denials('deny-write: secrets/key.txt') == ['secrets/key.txt']
    assert parse_denials('deny-write: "my secrets/key.txt"') == ['my secrets/key.txt']
    assert parse_denials("deny-write: 'my secrets/key.txt'") == ['my secrets/key.txt']


def test_parse_denials_rejects_natural_language_phrasing():
    assert parse_denials('do not modify the existing database schema') == []
    assert parse_denials('Не меняй database') == []
    assert parse_denials('не трогай config.yaml') == []


def test_parse_denials_rejects_wildcards_and_masks():
    assert parse_denials('deny-write: *') == []
    assert parse_denials('deny-write: src/**') == []
    assert parse_denials('deny-write: config?.yaml') == []


# --- outside_write follows real paths; project git metadata is protected.

def test_outside_write_refuses_through_a_parent_symlink_alias_into_the_project(worker, tmp_path):
    store, channel, coordinator, root = worker
    outside = tmp_path / 'outside'
    outside.mkdir()
    alias = outside / 'alias'
    alias.symlink_to(root)
    with pytest.raises(StoreError, match='outside_path_inside_project'):
        call(worker, 'outside_write', path=str(alias / 'planted.txt'), content='x', request_key='alias')
    assert not (root / 'planted.txt').exists()


def test_outside_write_still_refuses_a_literal_in_project_path(worker):
    with pytest.raises(StoreError, match='outside_path_inside_project'):
        call(worker, 'outside_write', path=str(worker[3] / 'planted.txt'), content='x', request_key='literal')


def _git(cwd, home, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1', HOME=str(home))
    subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
                    cwd=cwd, env=env, check=True, capture_output=True)


def test_control_plane_paths_protects_a_linked_worktrees_external_gitdir(tmp_path):
    if not __import__('shutil').which('git'):
        pytest.skip('git is required')
    home = tmp_path / 'home'
    home.mkdir()
    repo = tmp_path / 'main'
    repo.mkdir()
    _git(repo, home, 'init', '-q')
    (repo / 'f.txt').write_text('hi')
    _git(repo, home, 'add', 'f.txt')
    _git(repo, home, 'commit', '-q', '-m', 'init')
    worktree = tmp_path / 'wt'
    _git(repo, home, 'worktree', 'add', '-q', str(worktree), '-b', 'feature')
    paths = control_plane_paths(worktree, dict(os.environ, HOME=str(home)))
    gitdir = next((p for p in paths if p.name == 'wt' and 'worktrees' in p.parts), None)
    assert gitdir is not None, paths
    assert (gitdir / 'HEAD').exists()
    commondir = next((p for p in paths if p == repo / '.git'), None)
    assert commondir is not None, paths
