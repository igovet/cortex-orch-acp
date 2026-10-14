"""Runtime verification runs in the read-only host profile in every access
mode, and gap closure accepts that profile.

Launch preparation and lifecycle are checked here without namespaces; the
``requires_bwrap`` cases execute real containment and skip where bubblewrap
cannot create namespaces (a nested sandbox).
"""
import json
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration import access_mode
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import revision
from cortex_runtime.orchestration import controller, sandbox
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

ENV = {'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
       'GIT_AUTHOR_NAME': 'Test', 'GIT_AUTHOR_EMAIL': 'test@example.invalid',
       'GIT_COMMITTER_NAME': 'Test', 'GIT_COMMITTER_EMAIL': 'test@example.invalid'}


def _windows(command, size=3):
    return [command[i:i + size] for i in range(len(command) - size + 1)]


def _writable(command):
    return {window[1] for window in _windows(command) if window[0] == '--bind' and window[1] == window[2]}


def _isolated_home(monkeypatch, home):
    monkeypatch.setenv('HOME', str(home))
    monkeypatch.setenv('XDG_CONFIG_HOME', str(home / '.config'))
    monkeypatch.setenv('XDG_STATE_HOME', str(home / '.local/state'))
    for key in ('CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'XDG_DATA_HOME', *sandbox.READ_ONLY_CACHE_ENV):
        monkeypatch.delenv(key, raising=False)


def _fake_bwrap(monkeypatch):
    # Preparation only needs a bubblewrap path; nothing launches bwrap.
    real = shutil.which
    monkeypatch.setattr(sandbox.shutil, 'which',
                        lambda name, *a, **kw: '/usr/bin/bwrap' if name == 'bwrap' else real(name, *a, **kw))


def _git(*args, cwd):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True,
                          env={**ENV, 'PATH': '/usr/bin:/bin', 'HOME': str(cwd)}).stdout


@pytest.fixture
def home(tmp_path, monkeypatch):
    home = tmp_path / 'home'
    for relative in ('.cache', '.npm/_logs', '.nvm', '.docker/buildx/activity', '.local/state', '.config'):
        (home / relative).mkdir(parents=True)
    (home / '.bashrc').write_text('# owner\n')
    _isolated_home(monkeypatch, home)
    return home


@pytest.fixture
def project(tmp_path):
    root = tmp_path / 'project'
    root.mkdir()
    _git('init', '-q', '-b', 'main', cwd=root)
    (root / 'a.txt').write_text('one\n')
    (root / '.gitignore').write_text('.cortex/\nbuild/\n')
    (root / 'build').mkdir()
    _git('add', '.', cwd=root)
    _git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'baseline', cwd=root)
    return root


def _runtime(tmp_path, root, mode):
    runtime_root = tmp_path / 'runtime'
    runtime_root.mkdir()
    store = Store(runtime_root / '.cortex', project_root=runtime_root)
    task = bootstrap_task(store, SourceEvent(origin='verify-profile', event_id=uuid4().hex), 'Verify', 'owner')
    TaskMachine(store).create(task.task_id, 'Verify')
    access_mode.switch(store, task.task_id, mode, origin='acp_set_mode')
    runtime = SimpleNamespace(_store=store, _task_project_root=lambda _: str(root),
                              _task_workspace_root=lambda _: str(root))
    return runtime, store, task


def _digest(root):
    before = revision.capture_revision(str(root))
    return before.get('dirty_digest') or before.get('digest')


def _observe(runtime, task, root, checks, cancel=None, run_id=1):
    context = dict(revision=1, plan=dict(verification=checks))
    return controller._verification_observation(runtime, task.task_id, context, _digest(root), None,
                                                cancel or threading.Event(), run_id)


@pytest.mark.real_verification_sandbox
def test_verification_launch_is_the_read_only_profile(tmp_path, home, project, monkeypatch):
    _fake_bwrap(monkeypatch)
    monkeypatch.setattr(sandbox, 'development_command', lambda *a, **kw: pytest.fail('host-writable profile'))
    monkeypatch.setattr(revision, 'ignored_directories', lambda root: ['build'])
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    with controller._verification_command(project, scratch, ['python3', '-c', 'pass']) as (command, fds):
        windows = _windows(command)
        # Its own PID namespace; the host and HOME read-only.
        assert command[1:3] == ['--die-with-parent', '--unshare-pid']
        assert ['--ro-bind', '/', '/'] in windows and ['--bind', '/', '/'] not in windows
        assert ['--ro-bind', str(home), str(home)] in windows
        # Only the explicit caches, scratch and /tmp are writable host paths.
        assert _writable(command) == {str(home / '.cache'), str(home / '.npm'),
                                      str(home / '.docker/buildx/activity'), str(scratch), '/tmp'}
        assert ['--bind', str(scratch / 'var-tmp'), '/var/tmp'] in windows
        # The project stays read-only except its git-ignored directories.
        project_at = sandbox._project_mount(command, project.resolve())
        grants = [w for w in windows[project_at:] if w[0] == '--bind' and w[1].startswith('/proc/self/fd/')]
        assert [w[2] for w in grants] == [str(project / 'build')]
        # pytest's cache goes to scratch (the last --setenv wins over an
        # inherited PYTEST_ADDOPTS); TMPDIR is the private scratch.
        addopts = command[len(command) - command[::-1].index('PYTEST_ADDOPTS')]
        assert addopts.endswith(f'cache_dir={scratch}/pytest-cache')
        assert command[command.index('TMPDIR') + 1] == str(scratch)
        assert command[command.index('--') + 1:] == sandbox.with_umask(['python3', '-c', 'pass'])
        assert fds


@pytest.mark.parametrize('before,after', [('development', 'guarded'), ('guarded', 'auto'), ('auto', 'development')])
def test_verification_records_the_read_only_profile_and_survives_a_mode_switch(tmp_path, project, before, after):
    """A mode switch no longer changes a check's profile, so it never stops
    one (it used to kill the check as execution_policy_changed)."""
    runtime, store, task = _runtime(tmp_path, project, before)
    started, switched = tmp_path / 'started', tmp_path / 'switched'
    script = ('import pathlib, time\n'
              f'pathlib.Path({str(started)!r}).write_text("yes")\n'
              'deadline = time.monotonic() + 20\n'
              f'while not pathlib.Path({str(switched)!r}).exists() and time.monotonic() < deadline:\n'
              '    time.sleep(.02)\n'
              f'raise SystemExit(0 if pathlib.Path({str(switched)!r}).exists() else 9)\n')

    def switch():
        deadline = time.monotonic() + 20
        while not started.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        access_mode.switch(store, task.task_id, after, origin='acp_set_mode')
        time.sleep(.3)  # several of the runner's poll intervals
        switched.write_text('yes')

    thread = threading.Thread(target=switch)
    thread.start()
    observed = _observe(runtime, task, project, [[sys.executable, '-B', '-c', script]])
    thread.join()
    [check] = observed['checks']
    assert not observed['stale'] and check['exit_code'] == 0 and check['complete'], observed
    assert check['profile'] == access_mode.READ_ONLY_COMMAND_PROFILE == 'read_only'
    with store.connection() as db:
        started_event = json.loads(db.execute(
            "SELECT payload_json FROM runtime_events WHERE type='CHECK_EXECUTION_STARTED'").fetchone()[0])
        failure = db.execute('SELECT failure FROM large_outputs WHERE id=?', (check['large_output_id'],)).fetchone()[0]
    assert started_event['profile'] == 'read_only'
    assert failure is None


@pytest.mark.parametrize('mode', ['development', 'auto', 'guarded'])
@pytest.mark.real_verification_sandbox
def test_missing_bubblewrap_blocks_verification_in_every_mode(tmp_path, home, project, monkeypatch, mode):
    runtime, store, task = _runtime(tmp_path, project, mode)
    monkeypatch.setattr(sandbox.shutil, 'which', lambda name, *a, **kw: None)
    observed = _observe(runtime, task, project, [['true']])
    assert observed == dict(blocker='verification_sandbox_unavailable', stale=False)
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE type='CHECK_EXECUTION_STARTED'").fetchone()[0] == 0


@pytest.mark.parametrize('mode', ['development', 'auto', 'guarded'])
@pytest.mark.requires_bwrap
@pytest.mark.real_verification_sandbox
def test_real_verification_writes_caches_and_temp_not_home_or_project(tmp_path, home, project, mode):
    runtime, store, task = _runtime(tmp_path, project, mode)
    (project / 'test_sample.py').write_text('def test_ok(tmp_path):\n    (tmp_path / "x").write_text("ok")\n')
    _git('add', 'test_sample.py', cwd=project)
    _git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'sample', cwd=project)
    import os
    host_pid = os.getpid()
    script = (
        'import os, pathlib, tempfile\n'
        f'home = pathlib.Path({str(home)!r})\n'
        'writable = [home / ".cache/tool", home / ".npm/_logs/run.log",\n'
        '            home / ".docker/buildx/activity/default", pathlib.Path("build/out"),\n'
        '            pathlib.Path(tempfile.gettempdir()) / "t", pathlib.Path("/var/tmp/v")]\n'
        'for path in writable:\n'
        '    path.write_text("ok")\n'
        'for path in (home / ".bashrc", home / ".nvm/x", home / ".config/x", home / ".local/state/x",\n'
        '             home / "new", pathlib.Path("a.txt"), pathlib.Path("new.txt")):\n'
        '    try: path.write_text("forbidden")\n'
        '    except OSError: pass\n'
        '    else: raise AssertionError(path)\n'
        f'assert not os.path.exists("/proc/{host_pid}"), "host processes visible"\n')
    observed = _observe(runtime, task, project, [
        [sys.executable, '-B', '-c', script],
        [sys.executable, '-B', '-m', 'pytest', '-q', 'test_sample.py'],
        ['git', 'status', '--short'],
    ])
    assert not observed.get('blocker') and not observed['stale'], observed
    assert [c['exit_code'] for c in observed['checks']] == [0, 0, 0], observed
    assert all(c['complete'] and c['profile'] == 'read_only' for c in observed['checks'])
    assert (home / '.cache/tool').read_text() == 'ok' and (project / 'build/out').read_text() == 'ok'
    assert (home / '.bashrc').read_text() == '# owner\n' and not (home / 'new').exists()
    assert (project / 'a.txt').read_text() == 'one\n' and not (project / '.pytest_cache').exists()


# --- Gap closure (state.synthesis_checks) -----------------------------------

def _plan():
    return dict(tasks=[dict(id='N0', role='backend-dev', objective='Implement', dependencies=[],
                            scope=['src/**'], model='auto')],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])


def _pass_for_node(state, coordinator, node_id, role, command, key):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.roles import resolve_role
    from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
    attempt = 'xa_' + uuid4().hex[:12]
    with state.store.connection() as db:
        node = db.execute('SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND node_id=?',
                          (coordinator.task_id, node_id)).fetchone()
        assignment = node['assignment_ref'] or uuid4().hex
        db.execute("UPDATE runtime_nodes SET status='running',assignment_ref=? WHERE task_id=? AND node_id=?",
                   (assignment, coordinator.task_id, node_id))
        assignment_id = 'a_' + uuid4().hex[:12]
        worker_id, secret = issue_channel_tx(db, coordinator.task_id, 'worker', coordinator.owner_epoch,
                                             parent_channel_id=coordinator.channel_id,
                                             assignment_id=assignment_id, attempt_id=attempt)
        stamp = state.store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (assignment, coordinator.task_id, assignment_id, attempt, resolve_role(role), 'ready', 'ready',
                    'shared_project_root', worker_id, None, None, None, 'qualified', None,
                    coordinator.owner_epoch, stamp, stamp))
        db.commit()
    worker = resolve_channel(state.store, worker_id, secret)
    receipt = mcp_tools.dispatch(state.store, worker, 'command_run', dict(argv=command, request_key=key))
    assert receipt['exit_code'] == 0 and receipt['complete'], receipt
    return receipt


def _gap_task(tmp_path, monkeypatch):
    from cortex_runtime.trusted_channel import resolve_channel
    home = tmp_path / 'home'
    home.mkdir()
    _isolated_home(monkeypatch, home)
    _fake_bwrap(monkeypatch)
    root = tmp_path / 'project'
    root.mkdir()
    store = Store(root / '.cortex', project_root=root)
    task = bootstrap_task(store, SourceEvent(origin='gap-profile', event_id=uuid4().hex), 'Verify', 'owner')
    state = TaskMachine(store)
    state.create(task.task_id, 'Verify')
    channel = resolve_channel(store, task.channel_id, task.secret)
    state.submit_plan(channel, _plan(), 'gap-profile-plan')
    state.move(channel.task_id, 'EXECUTING')
    state.claim_ready(channel.task_id, channel.channel_id)
    with store.connection() as db:
        db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                   (channel.task_id, 1, '_review_1', 'worker-code-reviewer', 'Review', '["N0"]', '["**"]',
                    'review', 'pending', None, None))
        db.commit()
    return state, channel


def _declare(state, channel, gap_command, passes):
    """N0 declares a not_run gap with ``gap_command``; each (node, ref) of
    ``passes`` adds a passed check citing that node's own receipt."""
    results = {'N0': [{'name': 'backend tests', 'outcome': 'not_run',
                       'blocker': 'Blocked earlier', 'command': gap_command}]}
    for node, ref in passes:
        results.setdefault(node, []).append({'name': f'{node} rerun', 'outcome': 'passed', 'tool_call_ref': ref})
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (channel.task_id,))
        for node, checks in results.items():
            db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND node_id=?",
                       (json.dumps({'checks': checks}), channel.task_id, node))
        db.commit()


@pytest.mark.usefixtures('unjailed_gateway_command')
@pytest.mark.parametrize('mode', ['development', 'auto', 'guarded'])
def test_reviewer_read_only_pass_closes_a_writer_gap_in_every_mode(tmp_path, monkeypatch, mode):
    state, channel = _gap_task(tmp_path, monkeypatch)
    access_mode.switch(state.store, channel.task_id, mode, origin='acp_set_mode')
    command = ['python3', '-B', '-c', 'assert 2 + 2 == 4']
    receipt = _pass_for_node(state, channel, '_review_1', 'code-reviewer', command, 'reviewer-pass')
    # The reviewer's own command runs in the read-only profile in every mode.
    assert receipt['profile'] == access_mode.READ_ONLY_COMMAND_PROFILE
    _declare(state, channel, command, [('_review_1', receipt['tool_call_ref'])])
    assert state.submit_synthesis(channel, 'Verified')['recorded']


@pytest.mark.usefixtures('unjailed_gateway_command')
def test_host_writable_pass_never_closes_a_gap_once_guarded(tmp_path, monkeypatch):
    state, channel = _gap_task(tmp_path, monkeypatch)
    access_mode.switch(state.store, channel.task_id, 'development', origin='acp_set_mode')
    command = ['python3', '-B', '-c', 'assert 2 + 2 == 4']
    writer = _pass_for_node(state, channel, 'N0', 'backend-dev', command, 'writer-pass')
    assert writer['profile'] == 'development'
    # In Development that host-writable pass closes the writer's own gap.
    _declare(state, channel, command, [('N0', writer['tool_call_ref'])])
    from cortex_runtime.orchestration.state import synthesis_checks
    with state.store.connection() as db:
        assert synthesis_checks(db, channel.task_id, 1)[1] == {}
    access_mode.switch(state.store, channel.task_id, 'guarded', origin='acp_set_mode')
    with pytest.raises(StoreError, match='synthesis_gaps_undisclosed') as error:
        state.submit_synthesis(channel, 'Verified')
    assert 'backend tests' in error.value.correction
    # A reviewer's read-only pass of the same argv closes it in Guarded.
    reviewer = _pass_for_node(state, channel, '_review_1', 'code-reviewer', command, 'reviewer-pass')
    assert reviewer['profile'] == 'read_only'
    _declare(state, channel, command, [('N0', writer['tool_call_ref']), ('_review_1', reviewer['tool_call_ref'])])
    assert state.submit_synthesis(channel, 'Verified')['recorded']


@pytest.mark.usefixtures('unjailed_gateway_command')
def test_a_guarded_writer_pass_still_closes_a_gap_in_every_mode(tmp_path, monkeypatch):
    """The read-only profile's own name changes nothing about which passes
    close a gap: a Guarded writer pass (``restricted``, the name read-only
    passes were also recorded under before) closes it in Guarded and after a
    switch to Development."""
    state, channel = _gap_task(tmp_path, monkeypatch)
    access_mode.switch(state.store, channel.task_id, 'guarded', origin='acp_set_mode')
    command = ['python3', '-B', '-c', 'assert 2 + 2 == 4']
    writer = _pass_for_node(state, channel, 'N0', 'backend-dev', command, 'guarded-writer-pass')
    assert writer['profile'] == access_mode.GUARDED_COMMAND_PROFILE == 'restricted'
    _declare(state, channel, command, [('N0', writer['tool_call_ref'])])
    from cortex_runtime.orchestration.state import synthesis_checks
    for mode in ('guarded', 'development'):
        access_mode.switch(state.store, channel.task_id, mode, origin='acp_set_mode')
        with state.store.connection() as db:
            assert synthesis_checks(db, channel.task_id, 1)[1] == {}, mode
