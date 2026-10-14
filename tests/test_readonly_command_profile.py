"""The read-only host command profile of roles without a write grant.

Launch preparation is inspected here (it needs only the bubblewrap binary);
real containment runs in tests/test_development_commands.py
``requires_bwrap`` cases, which skip where namespaces are unavailable.
"""
from pathlib import Path

import pytest

from cortex_runtime.configuration import access_mode
from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import gateway, sandbox
from cortex_runtime.trusted_channel import resolve_channel
from test_command_run_arguments import _worker


def _windows(command, size=3):
    return [command[i:i + size] for i in range(len(command) - size + 1)]


def _writable(command):
    return {window[1] for window in _windows(command) if window[0] == '--bind' and window[1] == window[2]}


@pytest.fixture
def layout(tmp_path):
    home = tmp_path / 'home'
    for relative in ('.cache', '.npm', '.nvm/versions', '.oh-my-zsh/custom', '.ssh', '.cargo/registry',
                     '.cargo/bin', '.docker/buildx/activity', '.config/autostart', '.local/state/cortex',
                     '.local/share/virtualenv', '.claude', 'go/pkg/mod'):
        (home / relative).mkdir(parents=True)
    (home / '.bashrc').write_text('# owner\n')
    (home / '.cargo/.package-cache').write_text('')
    root = tmp_path / 'project'
    (root / 'src').mkdir(parents=True)
    (root / '.cortex').mkdir()
    moved = tmp_path / 'moved-cache'
    moved.mkdir()
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    environ = dict(HOME=str(home), PATH='/usr/bin:/bin', XDG_CACHE_HOME=str(moved),
                   XDG_CONFIG_HOME=str(home / '.config'), XDG_STATE_HOME=str(home / '.local/state'),
                   PIP_CACHE_DIR=str(home), UV_CACHE_DIR=str(root / 'src'), npm_config_cache='/',
                   GOCACHE=str(home / '.ssh'), PRE_COMMIT_HOME=str(home / '.local/state/cortex'),
                   CORTEX_CHANNEL_SECRET='fixture', PRIVATE_TOKEN='fixture')
    return home, root, scratch, moved, environ


def _isolated_home(monkeypatch, home):
    monkeypatch.setenv('HOME', str(home))
    monkeypatch.setenv('XDG_CONFIG_HOME', str(home / '.config'))
    monkeypatch.setenv('XDG_STATE_HOME', str(home / '.local/state'))
    for key in ('CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'XDG_DATA_HOME', 'XDG_CACHE_HOME'):
        monkeypatch.delenv(key, raising=False)


@pytest.fixture
def bwrap_binary(monkeypatch):
    # Preparation only needs a bubblewrap path; nothing is launched.
    monkeypatch.setattr(sandbox.shutil, 'which', lambda name: '/usr/bin/bwrap' if name == 'bwrap' else None)


@pytest.mark.usefixtures('bwrap_binary')
def test_home_is_read_only_except_explicit_caches(layout):
    home, root, scratch, moved, environ = layout
    with sandbox.readonly_command(root, scratch, ['true'], environ=environ) as (command, fds):
        writable = _writable(command)
        assert ['--ro-bind', str(home), str(home)] in _windows(command)
        assert ['--bind', '/', '/'] not in _windows(command)
        expected = {str(home / relative) for relative in ('.cache', '.npm', '.cargo/registry', '.cargo/.package-cache',
                                                         '.docker/buildx/activity', '.local/share/virtualenv',
                                                         'go/pkg/mod')}
        expected |= {str(moved), str(scratch), '/tmp', str(scratch / 'var-tmp')}
        # var-tmp is bound at /var/tmp, not over itself.
        expected.discard(str(scratch / 'var-tmp'))
        assert writable == expected
        # /tmp precedes the read-only HOME mount, so a HOME under /tmp stays read-only.
        assert _windows(command).index(['--bind', '/tmp', '/tmp']) < _windows(command).index(
            ['--ro-bind', str(home), str(home)])
        for denied in ('.nvm', '.oh-my-zsh', '.ssh', '.cargo/bin', '.config', '.local/state', '.local/state/cortex',
                       '.claude', '.bashrc'):
            target = str(home / denied)
            assert not any(target == path or target.startswith(path + '/')
                           for path in writable - {'/tmp'}), denied
        # The project is never writable; no grant bind of the root exists.
        assert not any(window[0] == '--bind' and window[2] == str(root) for window in _windows(command))
        assert fds


@pytest.mark.usefixtures('bwrap_binary')
def test_own_pid_namespace_and_ordered_protection(layout):
    home, root, scratch, moved, environ = layout
    with sandbox.readonly_command(root, scratch, ['true'], environ=environ) as (command, fds):
        assert command[1:3] == ['--die-with-parent', '--unshare-pid']
        home_at = _windows(command).index(['--ro-bind', str(home), str(home)])
        cache_at = _windows(command).index(['--bind', str(home / '.cache'), str(home / '.cache')])
        protected_at = _windows(command).index(['--ro-bind', str(home / '.claude'), str(home / '.claude')])
        project_at = sandbox._project_mount(command, root.resolve())
        assert home_at < cache_at < protected_at < project_at
        # The project's private store stays masked after the project mount.
        assert any(window[:2] == ['--tmpfs', str(root / '.cortex')] for window in _windows(command)[project_at:])
        assert 'CORTEX_CHANNEL_SECRET' not in command and 'PRIVATE_TOKEN' not in command


def test_cache_candidates_are_rejected_when_unsafe(layout):
    home, root, scratch, moved, environ = layout
    protected = sandbox.protected_host_paths(root, environ)
    found = sandbox.read_only_caches(environ, root, protected)
    assert str(moved) in found
    for rejected in (home, root / 'src', Path('/'), home / '.ssh', home / '.local/state/cortex'):
        assert str(rejected) not in found
    # A symlinked cache binds its real target; a link to HOME is refused.
    (home / '.npm').rmdir()
    (home / '.npm').symlink_to(moved)
    (home / '.cache').rmdir()
    (home / '.cache').symlink_to(home)
    found = sandbox.read_only_caches(environ, root, protected)
    assert str(moved) in found and str(home) not in found


def test_missing_bubblewrap_fails_closed(layout, monkeypatch):
    home, root, scratch, moved, environ = layout
    monkeypatch.setattr(sandbox.shutil, 'which', lambda name: None)
    with pytest.raises(StoreError, match='verification_sandbox_unavailable'):
        with sandbox.readonly_command(root, scratch, ['true'], environ=environ):
            pass


@pytest.mark.parametrize('mode', ['development', 'auto', 'guarded'])
def test_gateway_never_gives_a_read_only_role_the_host_writable_profile(tmp_path, monkeypatch, mode):
    root, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    home = tmp_path / 'home'
    home.mkdir()
    _isolated_home(monkeypatch, home)
    access_mode.switch(store, worker.task_id, mode, origin='acp_set_mode')
    monkeypatch.setattr(gateway, '_authority', lambda db, ch: ([], 'read-only'))
    launched = []
    monkeypatch.setattr(sandbox.shutil, 'which', lambda name: '/usr/bin/bwrap' if name == 'bwrap' else None)
    monkeypatch.setattr(sandbox, 'development_command', lambda *a, **kw: pytest.fail('host-writable profile'))

    watched = []

    def fake_run(store_, channel_, launch, fds, output, effects, limit, **kw):
        launched.append(launch)
        watched.append(kw.get('policy'))
        return 0, None
    monkeypatch.setattr(gateway, '_run_command_process', fake_run)
    receipt = gateway.dispatch(store, worker, 'command_run', dict(argv=['true'], request_key='ro'))
    assert receipt['profile'] == 'read_only' and receipt['complete'], receipt
    # Its profile does not depend on the mode: a mode switch never stops it.
    assert watched == [None]
    [launch] = launched
    assert '--unshare-pid' in launch and ['--bind', '/', '/'] not in _windows(launch)
    assert ['--ro-bind', str(home), str(home)] in _windows(launch)


def test_read_only_role_without_bubblewrap_degrades_like_every_command(tmp_path, monkeypatch):
    root, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    home = tmp_path / 'home'
    home.mkdir()
    _isolated_home(monkeypatch, home)
    monkeypatch.setattr(sandbox.shutil, 'which', lambda name: None)
    errors = []
    for scopes, key in (([], 'reader'), (['.'], 'writer')):
        monkeypatch.setattr(gateway, '_authority', lambda db, ch, scopes=scopes: (scopes, 'owner'))
        with pytest.raises(StoreError) as raised:
            gateway.dispatch(store, worker, 'command_run', dict(argv=['true'], request_key=key))
        errors.append(str(raised.value))
    assert errors[0] == errors[1] and 'verification_sandbox_unavailable' in errors[0]
