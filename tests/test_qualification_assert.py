"""The qualification assertion exits nonzero unless the bound task's complete
metadata shows every expected outcome. Fresh controller-driven fixtures only."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.delegation import events

from test_steering_after_verification import _drive, _misused_then_answered, _try

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'cortex_qualification_assert.py'


def _bind(store, tmp_path, task):
    sid = 'acps_' + 'b' * 12
    sessions.create_session(store, sid, str(tmp_path), {'name': 'fixture'}, {}, [])
    sessions.bind_task(store, sid, task.task_id)
    return [sys.executable, '-B', str(SCRIPT), '--project', str(tmp_path), '--session', sid,
            '--task', task.task_id, '--owner-epoch', str(task.owner_epoch.epoch)]


def _run(argv):
    result = subprocess.run(argv, capture_output=True, text=True)
    return result.returncode, json.loads(result.stdout) if result.stdout.strip() else result.stderr


def _files(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (root / '.cortex').rglob('*') if p.is_file()}


def test_completed_task_passes_and_each_unmet_expectation_exits_nonzero(tmp_path, monkeypatch):
    store, task, context, seen = _drive(tmp_path, monkeypatch, _misused_then_answered)
    assert context['state'] == 'COMPLETED'
    argv = _bind(store, tmp_path, task)
    before = _files(tmp_path)
    code, result = _run(argv)
    assert (code, result['passed'], result['failures']) == (0, True, [])
    assert _files(tmp_path) == before
    assert result['binding']['task_id'] == task.task_id and len(result['snapshot_sha256']) == 64
    # Dispatch alone is not a route: without a native confirmation it fails.
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:high'])
    assert code == 1 and [f['expectation'] for f in result['failures']] == ['route']
    with store.connection() as db:
        attempts = [r['attempt_id'] for r in db.execute(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id='worker-backend-dev'",
            (task.task_id,))]
    for attempt in attempts:
        events.append_event(store, task.task_id, 'progress', f'confirmed-{attempt}', dict(
            kind='worker_configuration_confirmed', attempt_id=attempt, engine='claude-agent-acp', model='haiku',
            parameters=dict(thought_level='high'), confirmation_evidence_ref='fixture', engine_session_ref=None,
            config_revision_id=None, connection_id=None), 'fixture')
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:high'])
    assert (code, result['failures']) == (0, [])
    code, result = _run(argv + ['--expect-route', 'worker-backend-dev=claude-agent-acp/haiku:low'])
    assert code == 1 and result['failures'][0]['observed'] == ['claude-agent-acp/haiku:high']
    # This fixture ran no failing check and retained no large output.
    code, result = _run(argv + ['--expect-closed-failure', '--expect-retained-output-bytes', '65537'])
    assert code == 1 and [f['expectation'] for f in result['failures']] == [
        'failed_check_closed_by_exact_rerun', 'complete_retained_output']
    code, result = _run(argv[:-1] + [str(task.owner_epoch.epoch + 1)])
    assert (code, result) == (2, dict(passed=False, error='inspection_binding_mismatch'))
    code, _ = _run(argv + ['--expect-route', 'backend=claude-agent-acp/haiku'])
    assert code == 2


def test_task_stuck_waiting_for_the_user_is_a_failed_assertion(tmp_path, monkeypatch):
    def synthesis_only(store, channel, context, seen):
        _try(seen, store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, synthesis_only)
    assert context['state'] == 'WAITING_USER'
    code, result = _run(_bind(store, tmp_path, task))
    assert code == 1 and result['passed'] is False
    assert result['failures'][0] == dict(expectation='state', expected='COMPLETED', observed='WAITING_USER')
    code, result = _run(_bind_again(store, tmp_path, task) + ['--expect-state', 'WAITING_USER'])
    assert (code, result['failures']) == (0, [])


def _bind_again(store, tmp_path, task):
    sid = 'acps_' + 'b' * 12
    return [sys.executable, '-B', str(SCRIPT), '--project', str(tmp_path), '--session', sid,
            '--task', task.task_id, '--owner-epoch', str(task.owner_epoch.epoch)]
