"""Real sandbox regressions for developer commands; no model or real daemon."""
import hashlib
import json
import sys
import shutil
import subprocess
import threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler
from socketserver import UnixStreamServer
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import revision
from cortex_runtime.orchestration import gateway
from cortex_runtime.orchestration.controller import _verification
from cortex_runtime.orchestration.project_paths import PROJECT_NOTES
from cortex_runtime.orchestration.sandbox import engine_jail, gateway_command
from cortex_runtime.orchestration.state import TaskMachine, validate_scope
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from test_sandbox_git_and_tool_state import ENV, git, repo


@pytest.fixture
def worker(tmp_path, repo, monkeypatch):
    home = tmp_path / 'home'
    home.mkdir()
    monkeypatch.setenv('HOME', str(home))
    monkeypatch.setenv('XDG_CONFIG_HOME', str(home / '.config'))
    for name, value in ENV.items():
        monkeypatch.setenv(name, value)
    (repo / '.cortex/cortex.sqlite3').unlink()  # repo fixture's dummy private sentinel
    (repo / '.cortex').chmod(0o700)  # Store requires owner-only private storage.
    store = Store(repo / '.cortex', project_root=repo)
    task = bootstrap_task(store, SourceEvent(origin='developer-test', event_id='one'), 'Develop', 'owner')
    TaskMachine(store).create(task.task_id, 'Develop')
    monkeypatch.setenv('PYTHONPATH', str(Path(pytest.__file__).parent.parent))
    channel = resolve_channel(store, task.channel_id, task.secret)
    channel = SimpleNamespace(task_id=channel.task_id, attempt_id='xa_test',
                              channel_id=channel.channel_id, role='worker')
    # Isolate durable authorization setup; command dispatch, sandbox, argv
    # validation, subprocess and retained output remain production code.
    monkeypatch.setattr(gateway, '_authority', lambda db, ch: (['.'], 'owner'))
    return store, channel


def run(worker, argv, key):
    store, channel = worker
    return gateway.dispatch(store, channel, 'command_run',
                            dict(argv=argv, timeout_seconds=60, request_key=key))


@pytest.mark.requires_bwrap
def test_worker_git_test_and_build_commands(worker, repo):
    (repo / 'test_example.py').write_text(
        'def test_build():\n'
        '    from pathlib import Path\n'
        '    Path("build").mkdir(exist_ok=True)\n'
        '    Path("build/result").write_text("built")\n')
    result = run(worker, [sys.executable, '-B', '-m', 'pytest', '-q', 'test_example.py'], 'pytest')
    assert result['exit_code'] == 0 and result['complete'], result
    assert (repo / 'build/result').read_text() == 'built'
    result = run(worker, ['sh', '-c',
                         'git status --short && git diff --check && git add test_example.py && '
                         'git commit -m tests && git branch feature && git log -1 --format=%s'], 'git')
    assert result['exit_code'] == 0 and result['complete'], result
    assert git('log', '-1', '--format=%s', cwd=repo).strip() == 'tests'


@pytest.mark.requires_bwrap
def test_worker_real_docker_client_reaches_unix_socket(worker, tmp_path):
    docker = shutil.which('docker')
    if docker is None:
        pytest.skip('real Docker CLI is not installed')
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            requests.append(self.path)
            body = b'OK' if self.path == '/_ping' else json.dumps(
                {'Version': '27.0.0', 'ApiVersion': '1.47', 'MinAPIVersion': '1.24',
                 'Os': 'linux', 'Arch': 'amd64'}).encode()
            self.send_response(200)
            self.send_header('API-Version', '1.47')
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            if self.command != 'HEAD':
                self.wfile.write(body)

    socket = tmp_path / 'docker.sock'
    with UnixStreamServer(str(socket), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            result = run(worker, [docker, '-H', f'unix://{socket}', 'version',
                                  '--format', '{{.Server.Version}}'], 'docker')
        finally:
            server.shutdown()
            thread.join()
    assert result['exit_code'] == 0 and result['complete'], result
    assert result['output'].strip() == '27.0.0'
    assert any(path.endswith('/version') for path in requests)


@pytest.mark.parametrize('linked', [False, True])
@pytest.mark.requires_bwrap
@pytest.mark.real_verification_sandbox
def test_automatic_verification_can_inspect_git_but_cannot_write(tmp_path, repo, linked):
    root = repo
    if linked:
        root = tmp_path / 'worktree'
        git('worktree', 'add', '-q', '-b', 'side', str(root), cwd=repo)
    runtime_root = tmp_path / 'runtime'
    runtime_root.mkdir()
    store = Store(runtime_root / '.cortex', project_root=runtime_root)
    task = bootstrap_task(store, SourceEvent(origin='verify-test', event_id='one'), 'Verify', 'owner')
    before = revision.capture_revision(str(root))
    digest = before.get('dirty_digest') or before.get('digest')
    common = repo / '.git'
    script = (
        'from pathlib import Path\n'
        f'for name in {["a.txt", str(common / "HEAD"), ".cortex/private"]!r}:\n'
        '    try: Path(name).write_text("forbidden")\n'
        '    except OSError: pass\n'
        '    else: raise AssertionError(name)\n')
    checks = [['git', 'diff', '--check'], ['git', 'rev-parse', 'HEAD'], ['python3', '-B', '-c', script]]
    context = dict(revision=1, nodes=[dict(stage='review', result=dict(project_digest=digest))],
                   plan=dict(verification=checks))
    recorded = []
    machine = SimpleNamespace(context=lambda _: context,
                              move=lambda *args, **kwargs: pytest.fail(str(kwargs)),
                              record_verification=lambda *args: recorded.append(args))
    runtime = SimpleNamespace(_store=store, _task_project_root=lambda _: str(root),
                              _task_workspace_root=lambda _: str(root))
    _verification(runtime, task.task_id, machine, None, threading.Event())
    assert len(recorded) == 1
    assert len(recorded[0][1]) == 3
    assert all(check['exit_code'] == 0 and check['complete'] for check in recorded[0][1])
    assert (root / 'a.txt').read_text() == 'one\n'


@pytest.mark.parametrize('relative', sorted(PROJECT_NOTES))
def test_note_scope_and_file_tools_keep_engine_configuration_private(tmp_path, relative):
    root = tmp_path / 'project'
    (root / relative).parent.mkdir(parents=True)
    validate_scope([relative])
    result = gateway._write(root, dict(path=relative, content='notes', expected_sha256='absent'))
    assert result['sha256'] == hashlib.sha256(b'notes').hexdigest()
    assert gateway._read(root, dict(path=relative))['content'] == 'notes'
    prefix = relative.split('/')[0]
    for denied in (prefix, prefix + '/**', prefix + '/settings.json', prefix + '/hooks/x',
                   prefix + '/CLAUDE.md', prefix + '/credentials.json', 'nested/' + relative):
        with pytest.raises(StoreError):
            validate_scope([denied])
        with pytest.raises(StoreError):
            gateway._parts(denied)


@pytest.mark.parametrize('native', [False, True])
@pytest.mark.parametrize('writing,denied', [(True, False), (False, False), (True, 'file'), (True, 'directory')])
@pytest.mark.requires_bwrap
def test_note_mounts_obey_role_denials_and_hide_other_state(tmp_path, native, writing, denied):
    root = tmp_path / 'project'
    (root / '.claude').mkdir(parents=True)
    note = root / '.claude/session-history.md'
    note.write_text('before')
    settings = root / '.claude/settings.json'
    settings.write_text('{}')
    (root / '.cortex').mkdir()
    (root / '.cortex/secret').write_text('private')
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    scopes = ['.'] if writing else []
    readonly = (['.claude/**'] if denied == 'directory' else ['.claude/session-history.md']) if denied else []
    script = (
        'from pathlib import Path\n'
        'assert not Path(".cortex/secret").exists()\n'
        'p = Path(".claude/session-history.md")\n'
        'assert p.read_text() == "before"\n'
        'try: p.write_text("after")\n'
        'except OSError: print("readonly")\n'
        'else: print("written")\n'
        'try: Path(".claude/settings.json").write_text("bad")\n'
        'except OSError: pass\n'
        'else: raise AssertionError("settings writable")\n')
    argv = ['python3', '-B', '-c', script]
    context = (engine_jail(root, scratch, argv, scopes, env=ENV, readonly=readonly) if native else
               gateway_command(root, scratch, argv, scopes, readonly=readonly, system_root=True))
    with context as (command, fds):
        result = subprocess.run(command, pass_fds=fds, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == ('written' if writing and not denied else 'readonly')
    assert settings.read_text() == '{}'


@pytest.mark.requires_bwrap
def test_readonly_engine_pytest_cache_is_private_and_writable(tmp_path):
    root = tmp_path / 'project'
    root.mkdir()
    (root / 'test_cache.py').write_text(
        'def test_cache(pytestconfig):\n'
        '    pytestconfig.cache.set("example/value", 42)\n'
        '    assert pytestconfig.cache.get("example/value", None) == 42\n')
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    with engine_jail(root, scratch, ['python3', '-B', '-m', 'pytest', '-q', '-W', 'error'],
                     [], env={**ENV, 'PYTEST_ADDOPTS': '--strict-markers',
                              'PYTHONPATH': str(Path(pytest.__file__).parent.parent)}) as (command, fds):
        result = subprocess.run(command, pass_fds=fds, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    assert not (root / '.pytest_cache').exists()
    assert (scratch / 'pytest-cache/v/example/value').read_text() == '42'


@pytest.mark.parametrize('linked_parent', [False, True])
def test_note_exceptions_do_not_follow_symlinks_or_hardlinks(tmp_path, linked_parent):
    root = tmp_path / 'project'
    root.mkdir()
    outside = tmp_path / 'outside'
    outside.mkdir()
    private = outside / 'session-history.md'
    private.write_text('secret')
    if linked_parent:
        (root / '.claude').symlink_to(outside, target_is_directory=True)
    else:
        (root / '.claude').mkdir()
        (root / '.claude/session-history.md').hardlink_to(private)
    for operation, args in [
        (gateway._read, dict(path='.claude/session-history.md')),
        (gateway._write, dict(path='.claude/session-history.md', content='bad',
                              expected_sha256=hashlib.sha256(b'secret').hexdigest())),
    ]:
        with pytest.raises((StoreError, OSError)):
            operation(root, args)
    assert private.read_text() == 'secret'


@pytest.mark.requires_bwrap
def test_readonly_gateway_pytest_cache_is_private(worker, repo, monkeypatch):
    monkeypatch.setattr(gateway, '_authority', lambda db, ch: ([], 'owner'))
    (repo / 'test_cache.py').write_text(
        'def test_cache(pytestconfig):\n'
        '    pytestconfig.cache.set("example/value", 42)\n'
        '    assert pytestconfig.cache.get("example/value", None) == 42\n')
    result = run(worker, [sys.executable, '-B', '-m', 'pytest', '-q', '-W', 'error'], 'review-pytest')
    assert result['exit_code'] == 0 and result['complete'], result
    assert not (repo / '.pytest_cache').exists()


@pytest.mark.parametrize('command', [
    'git status --short', 'git diff --check', 'git add src/app.py',
    'git commit -m update', 'git branch feature', 'git log -1',
    'git stash push', 'python3 -m pytest -q', 'docker compose build',
])
def test_guarded_native_developer_commands_do_not_require_a_permission_card(tmp_path, command):
    from test_engine_permissions import _binding, _bridge, call, PARAMS
    from cortex_runtime.engine.resource_binding import BindingRegistry
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requests = []
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: requests.append(args))
    params = dict(PARAMS, toolCall=dict(name='Bash', kind='execute', rawInput=dict(command=command)))
    assert call(bridge, params)['outcome']['optionId'] == 'allow'
    assert requests == []
