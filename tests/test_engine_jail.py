"""An engine process runs in the jail: native tools write only the granted scope."""
import shutil
import subprocess
from pathlib import Path

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import _jail_for
from cortex_runtime.engine.process import EngineProcess
from cortex_runtime.orchestration.sandbox import engine_jail
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import revoke_channel
from engine_fakes import isolated_environ, make_connection_record, new_connection_id

pytestmark = [pytest.mark.engine_jail,
              pytest.mark.skipif(shutil.which('bwrap') is None, reason='bubblewrap is required')]


def _run(tmp_path, script, scopes, state=None):
    project = tmp_path / 'project'
    scratch = tmp_path / 'scratch'
    scratch.mkdir(exist_ok=True)
    env = {'HOME': str(tmp_path / 'home'), 'PATH': '/usr/bin:/bin', 'ENGINE_SECRET': 'kept'}
    state_dirs = {state: 'claude-agent-acp'} if state else None
    with engine_jail(project, scratch, ['sh', '-c', script], scopes, env=env, state_dirs=state_dirs) as (command, fds):
        return subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                              capture_output=True, text=True, timeout=30)


@pytest.fixture
def tree(tmp_path):
    project = tmp_path / 'project'
    for path in ['src/a.py', 'docs/b.md', '.git/config', '.cortex/cortex.sqlite3', '.claude/skills/s/SKILL.md']:
        (project / path).parent.mkdir(parents=True, exist_ok=True)
        (project / path).write_text('original\n')
    (tmp_path / 'home').mkdir()
    return project


@pytest.mark.requires_bwrap
def test_only_the_scope_is_writable_and_private_state_is_hidden(tmp_path, tree):
    result = _run(tmp_path, '''
        echo new > src/a.py && echo new > src/c.py && echo scope-ok
        echo bad > docs/b.md || echo docs-ro
        echo bad > .git/config || echo git-ro
        cat .git/config .claude/skills/s/SKILL.md
        ls -A .cortex | wc -l
        touch "$HOME/x" || echo home-ro
        echo "$ENGINE_SECRET"
    ''', ['src/**'])
    out = result.stdout.split()
    assert ['scope-ok', 'docs-ro', 'git-ro', 'original', 'original', '0', 'home-ro', 'kept'] == out, result.stderr
    assert (tree / 'src/c.py').read_text() == 'new\n'
    assert (tree / 'docs/b.md').read_text() == 'original\n'


@pytest.mark.requires_bwrap
def test_no_scope_leaves_the_whole_project_read_only(tmp_path, tree):
    result = _run(tmp_path, 'echo bad > src/a.py || echo ro', [])
    assert result.stdout.split() == ['ro']
    assert (tree / 'src/a.py').read_text() == 'original\n'


@pytest.mark.requires_bwrap
def test_engine_state_is_writable_but_config_and_plugins_are_not(tmp_path, tree):
    state = tmp_path / 'home' / '.claude'
    (state / 'plugins' / 'p').mkdir(parents=True)
    (state / 'settings.json').write_text('{"a": 1}\n')
    result = _run(tmp_path, f'''
        cd {state}
        echo s > sessions.json && echo state-ok
        echo bad > settings.json || echo settings-ro
        echo bad > settings.local.json || echo local-ro
        mkdir skills/planted || echo skills-ro
        ls -A plugins | wc -l
    ''', [], state=state)
    assert result.stdout.split() == ['state-ok', 'settings-ro', 'local-ro', 'skills-ro', '0'], result.stderr
    assert (state / 'settings.json').read_text() == '{"a": 1}\n'
    # Absent config is created neutral on the host so none can be planted.
    assert (state / 'settings.local.json').read_text() == '{}\n'
    assert (state / 'skills').is_dir() and not (state / 'skills' / 'planted').exists()
    assert (state / 'plugins' / 'p').is_dir()


@pytest.mark.requires_bwrap
def test_owner_runtime_sockets_are_visible_like_on_the_host(tmp_path, tree):
    """Everything that works on the host works in the sandbox: the session
    bus, audio, display and keyring live under /run/user."""
    if not Path('/run/user').is_dir():
        pytest.skip('no /run/user on this host')
    result = _run(tmp_path, 'ls -A /run/user | wc -l', [])
    assert result.stdout.split() != ['0']


@pytest.mark.requires_bwrap
def test_revoked_channel_terminates_the_jailed_engine(tmp_path, tree):
    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(), ('/bin/sleep', '60'), environ=environ)
    alive = [True]
    process = EngineProcess(record, str(tree), environ=environ,
                            jail={'scopes': [], 'alive': lambda: alive[0]})
    try:
        assert not process.wait_exited(1.5)
        alive[0] = False
        assert process.wait_exited(10)
    finally:
        process.release(grace=1.0)


def test_jail_grants_the_coordinator_no_scope_and_follows_the_channel(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='jail-test', event_id='one'), 'Implement', 'owner')
    jail = _jail_for(store, (boot.channel_id, boot.secret, str(tmp_path)))
    assert jail['scopes'] == []
    assert jail['alive']()
    revoke_channel(store, boot.channel_id)
    assert not jail['alive']()


def test_transient_store_error_is_not_read_as_channel_revocation(tmp_path):
    """A ``storage_busy`` from routine lock contention on the 1s authority
    poll must never kill a live engine (that surfaced as spurious
    transport_eof); only a definitive channel invalidation reports lost."""
    from cortex_runtime.contracts import StoreError

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='jail-test', event_id='busy'), 'Implement', 'owner')
    jail = _jail_for(store, (boot.channel_id, boot.secret, str(tmp_path)))
    assert jail['alive']()  # baseline: channel is live

    def busy():
        raise StoreError("storage_busy")

    store.connection = busy
    assert jail['alive']()  # transient error -> stay alive, re-check next tick


@pytest.mark.requires_bwrap
def test_a_linked_engine_state_dir_and_config_still_run_locked_down(tmp_path, tree):
    """A dotfile manager links ~/.claude and its settings elsewhere; that
    refused every engine. It must run, with the config still read-only."""
    real = tmp_path / 'dotfiles' / 'claude'
    real.mkdir(parents=True)
    # The dangerous case: the config links to a file inside the writable
    # state dir (a target elsewhere on the host is read-only already).
    (real / 'managed-settings.json').write_text('{"a": 1}\n')
    (real / 'settings.json').symlink_to(real / 'managed-settings.json')
    state = tmp_path / 'home' / '.claude'
    state.symlink_to(real)
    result = _run(tmp_path, f'''
        cd {state}
        echo s > sessions.json && echo state-ok
        echo bad > settings.json || echo settings-ro
    ''', [], state=state)
    assert result.stdout.split() == ['state-ok', 'settings-ro'], result.stderr
    assert (real / 'sessions.json').read_text() == 's\n'
    assert (real / 'managed-settings.json').read_text() == '{"a": 1}\n'
