"""command_run's optional timeout_seconds through the real dispatch paths.

timeout_seconds advertises only a minimum: the runtime stays unbounded by
default (omit the field, or send an enormous value, and the command still
runs). An out-of-range value below the minimum, or a non-integer, is refused
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
