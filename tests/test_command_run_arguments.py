"""command_run's optional timeout_seconds through the real dispatch paths.

timeout_seconds advertises only a minimum. Omitted, it defaults to
gateway.COMMAND_RUN_DEFAULT_TIMEOUT_SECONDS (30 minutes; outside_command keeps
600 s); an enormous value still runs. An out-of-range value below the minimum, or a non-integer, is refused
on the exact field. An enormous in-range value is clamped internally
(gateway.TIMEOUT_CEILING_SECONDS) so it can never overflow the deadline or
subprocess-timeout arithmetic after the command already started.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

import pytest

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import gateway
from cortex_runtime.orchestration.contracts import validate
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from runtime_scenarios import reserve_assignment

CORTEX_PY = Path(__file__).resolve().parents[1] / 'src/cortex.py'
CEILING = gateway.TIMEOUT_CEILING_SECONDS


def _worker(tmp_path):
    """A running worker assignment and its channel credentials."""
    project = tmp_path / 'project'
    project.mkdir()
    store = Store(project / '.cortex', project_root=project)
    boot = bootstrap_task(store, SourceEvent(origin='command-run', event_id='one'), 'Check', 'owner')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    reserved = reserve_assignment(store, coordinator, dict(title='w', brief='w', role='backend-dev',
                                                           acceptance='Exact report', request_key='w'))
    assignment, attempt = 'a_' + uuid.uuid4().hex[:12], 'xa_' + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        channel, secret = issue_channel_tx(db, coordinator.task_id, 'worker', coordinator.owner_epoch,
                                           parent_channel_id=coordinator.channel_id,
                                           assignment_id=assignment, attempt_id=attempt)
        stamp = store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (reserved['assignment_ref'], coordinator.task_id, assignment, attempt,
                    resolve_role('backend-dev'), 'ready', 'ready', 'shared_project_root', channel, None, None, None,
                    'qualified', None, coordinator.owner_epoch, stamp, stamp))
        db.commit()
    return project, store, channel, secret


def test_both_timeouts_advertise_only_a_minimum():
    for operation in ('command_run', 'outside_command'):
        rule = gateway.SCHEMAS[operation]['properties']['timeout_seconds']
        assert rule['minimum'] == 1
        assert 'maximum' not in rule


@pytest.mark.parametrize('extra', [{}, dict(timeout_seconds=1), dict(timeout_seconds=600),
                                   dict(timeout_seconds=CEILING + 1), dict(timeout_seconds=10 ** 400)],
                         ids=['bare', 'minimum', 'typical', 'above_ceiling', 'huge'])
def test_valid_command_run_returns_a_receipt(tmp_path, extra):
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    receipt = mcp_tools.dispatch(store, worker, 'command_run',
                                 dict(argv=['echo', 'hello'], request_key='echo', **extra))
    assert receipt['exit_code'] == 0 and receipt['complete'] and receipt['failure'] is None, receipt
    assert receipt['output'] == 'hello\n' and receipt['tool_call_ref'].startswith('gateway:')


@pytest.mark.parametrize('value', [0, -5, '30', 1.5, True, None])
def test_invalid_timeout_is_refused_on_its_field(tmp_path, value):
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    for operation in ('command_run', 'outside_command'):
        with pytest.raises(StoreError, match='invalid_arguments') as refused:
            mcp_tools.dispatch(store, worker, operation,
                               dict(argv=['echo', 'x'], request_key=operation, timeout_seconds=value))
        assert refused.value.field == 'arguments.timeout_seconds'
        assert refused.value.expected == 'integer of at least 1'
    with store.connection() as db:  # refused before any durable action started
        assert db.execute('SELECT COUNT(*) FROM runtime_requests').fetchone()[0] == 0


def test_timeout_still_ends_a_long_command(tmp_path):
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    receipt = mcp_tools.dispatch(store, worker, 'command_run',
                                 dict(argv=['sleep', '30'], request_key='slow', timeout_seconds=1))
    assert receipt['failure'] == 'timeout' and not receipt['complete']


def test_integer_without_an_advertised_bound_is_still_validated():
    schema = dict(type='integer', minimum=1)
    validate(schema, 5)
    with pytest.raises(StoreError, match='invalid_arguments') as refused:
        validate(schema, 0, 'arguments.n')
    assert refused.value.field == 'arguments.n' and refused.value.expected == 'integer of at least 1'


def _mcp(project, channel, secret, tmp_path, calls):
    home = tmp_path / 'home' / 'cortex'
    home.mkdir(parents=True, exist_ok=True)
    home.chmod(0o700)
    (home / 'config.toml').write_text('schema_version = 1\n[gateway]\nenabled = false\n')
    (home / 'config.toml').chmod(0o600)
    messages = [dict(jsonrpc='2.0', id=1, method='initialize',
                     params=dict(protocolVersion='2025-11-25', capabilities={}, clientInfo=dict(name='t', version='1'))),
                dict(jsonrpc='2.0', method='notifications/initialized')]
    messages += [dict(jsonrpc='2.0', id=index + 2, method='tools/call', params=dict(name='command_run', arguments=args))
                 for index, args in enumerate(calls)]
    env = os.environ | dict(CODEX_HOME=str(home.parent), CORTEX_CHANNEL_ID=channel,
                            CORTEX_CHANNEL_SECRET=secret, CORTEX_CHANNEL_PROJECT_ROOT=str(project))
    done = subprocess.run([sys.executable, '-B', str(CORTEX_PY)], text=True, capture_output=True, env=env, timeout=60,
                          input='\n'.join(json.dumps(message) for message in messages) + '\n')
    assert done.returncode == 0 and done.stderr == '', done.stderr
    return [json.loads(line)['result'] for line in done.stdout.splitlines()][1:]


@pytest.mark.requires_bwrap
def test_mcp_wire_runs_with_timeout_and_names_the_invalid_field(tmp_path):
    """The real cortex.py MCP server, as an engine's Cortex child process."""
    project, _, channel, secret = _worker(tmp_path)
    bare, timed, invalid = _mcp(project, channel, secret, tmp_path, [
        dict(argv=['echo', 'bare'], request_key='bare'),
        dict(argv=['echo', 'timed'], request_key='timed', timeout_seconds=120),
        dict(argv=['echo', 'bad'], request_key='bad', timeout_seconds=0)])
    for reply, text in ((bare, 'bare\n'), (timed, 'timed\n')):
        assert not reply['isError'], reply
        assert reply['structuredContent']['exit_code'] == 0 and reply['structuredContent']['output'] == text
    assert invalid['isError']
    error = json.loads(invalid['content'][0]['text'])
    assert error['error'] == 'invalid_arguments' and error['field'] == 'arguments.timeout_seconds'
    assert error['received'] == '0' and error['write_state'] == 'rejected'


def test_a_genuine_failure_stays_storage_error_and_names_its_cause(tmp_path, monkeypatch, capsys):
    from cortex_runtime import server
    project, _, channel, secret = _worker(tmp_path)

    def fails(*args, **kwargs):
        raise OSError(28, 'No space left on device')  # e.g. the retained output could not be written
    monkeypatch.setattr('cortex_runtime.recovery.large_output_store.persist_output', fails)
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='command_run', arguments=dict(argv=['echo', 'x'], request_key='full')))
    error = json.loads(reply['content'][0]['text'])
    assert reply['isError'] and error['error'] == 'storage_error' and error['write_state'] == 'uncertain'
    assert (error['field'], error['received']) == ('cause', 'OSError: No space left on device')


def test_an_unexpected_exception_is_storage_error_naming_its_class(tmp_path, monkeypatch, capsys):
    from cortex_runtime import server
    project, _, channel, secret = _worker(tmp_path)

    def breaks(*args, **kwargs):
        raise KeyError('internal detail')
    monkeypatch.setattr(gateway, 'validate', breaks)
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='command_run', arguments=dict(argv=['echo', 'x'], request_key='bug')))
    error = json.loads(reply['content'][0]['text'])
    assert (error['error'], error['field'], error['received']) == ('storage_error', 'cause', 'KeyError')
    assert 'internal detail' not in reply['content'][0]['text']
    assert "KeyError: 'internal detail'" in capsys.readouterr().err  # the traceback stays in the log


def test_omitted_timeout_defaults_to_thirty_minutes(tmp_path, monkeypatch):
    """A hung command without timeout_seconds held its worker for 30 silent
    minutes (2026-10-21)."""
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    limits = []
    real = gateway._run_command_process

    def recording(store_, channel_, launch, fds, output, effects, limit, **kwargs):
        limits.append(limit)
        return real(store_, channel_, launch, fds, output, effects, limit, **kwargs)

    monkeypatch.setattr(gateway, '_run_command_process', recording)
    mcp_tools.dispatch(store, worker, 'command_run', dict(argv=['true'], request_key='bare'))
    mcp_tools.dispatch(store, worker, 'command_run', dict(argv=['true'], request_key='long', timeout_seconds=7200))
    assert limits == [gateway.COMMAND_RUN_DEFAULT_TIMEOUT_SECONDS, 7200] and limits[0] == 1800


def test_a_command_never_shares_the_mcp_servers_stdin(tmp_path):
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    receipt = mcp_tools.dispatch(store, worker, 'command_run', dict(
        argv=[sys.executable, '-B', '-c', "import os; print(os.path.realpath('/proc/self/fd/0'))"],
        request_key='stdin'))
    assert receipt['exit_code'] == 0 and receipt['output'].strip() == '/dev/null', receipt


def test_a_timeout_stops_a_child_that_left_the_process_group(tmp_path):
    """Playwright's webServer runs detached; after a timed-out run it kept
    its port and broke the next run and final verification (2026-10-22)."""
    import time
    _, store, channel, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel, secret)
    script = ("import subprocess, time\n"
              "child = subprocess.Popen(['sleep', '300'], start_new_session=True)\n"
              "print(child.pid, flush=True)\n"
              "time.sleep(300)\n")
    receipt = mcp_tools.dispatch(store, worker, 'command_run', dict(
        argv=[sys.executable, '-B', '-c', script], request_key='detached', timeout_seconds=2))
    assert receipt['failure'] == 'timeout', receipt
    detached = int(receipt['output'].split()[0])
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            with open(f'/proc/{detached}/stat') as handle:
                if handle.read().split(')')[-1].split()[0] == 'Z':
                    break
        except FileNotFoundError:
            break
        time.sleep(0.05)
    else:
        pytest.fail(f'detached child {detached} survived the timeout')


def test_the_mcp_server_survives_a_non_blocking_stdin():
    """A SIGKILLed Node child that shared the server's stdin left it
    O_NONBLOCK; the next empty read ended the server with exit 0, and every
    later call of that worker failed "Transport closed" (2026-10-22)."""
    import time
    src = Path(__file__).resolve().parents[1] / 'src'
    code = ("import os, sys; sys.path.insert(0, %r); os.set_blocking(0, False)\n"
            "from cortex_runtime.server import main; main()\n") % str(src)
    env = {k: v for k, v in os.environ.items() if not k.startswith('CORTEX_CHANNEL')}
    proc = subprocess.Popen([sys.executable, '-B', '-c', code], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, env=env)
    try:
        time.sleep(0.5)
        assert proc.poll() is None, 'server exited on an empty non-blocking read'
        proc.stdin.write(b'{"jsonrpc":"2.0","id":1,"method":"ping"}\n')
        proc.stdin.flush()
        reply = json.loads(proc.stdout.readline())
        assert reply == {'jsonrpc': '2.0', 'id': 1, 'result': {}}
    finally:
        proc.kill()
        proc.wait()
