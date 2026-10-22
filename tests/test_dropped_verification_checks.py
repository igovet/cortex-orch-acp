"""A failed verification check removed before completion needs the user.

Twice a desktop placement check failed in verification only for environment
reasons; the coordinator removed it and the task completed on lint and unit
tests, so the "fixed" popup still moved (observed 2026-10-20 and 2026-10-22).
Its environment cause (a private dbus-daemon's AppArmor query) is covered too.
"""
import json
import shutil
import subprocess
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration import controller
from cortex_runtime.orchestration.state import TaskMachine, dropped_failed_checks_unacknowledged, encoded
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

PLACEMENT = ['python3', 'scripts/check-shell-placement.py', '--capture']
LINT = ['./verify.sh']


def _failed_verification(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=str(uuid.uuid4())), 'Fix popup', 'owner')
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Fix popup')
    channel = SimpleNamespace(task_id=task.task_id, channel_id='coord', role='coordinator',
                              principal_thread_id='owner', parent_principal_thread_id=None)
    machine.submit_plan(channel, {'tasks': [{'id': 'N0', 'role': 'backend-dev', 'objective': 'Fix', 'dependencies': [],
                                             'scope': ['src/**'], 'model': 'auto'}],
                                  'verification': [LINT, PLACEMENT]}, 'plan')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (task.task_id,))
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", (task.task_id,))
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                   "VALUES(?,'VERIFICATION_FAILED','runtime',?,'2026-10-22T00:56:29+00:00')",
                   (task.task_id, encoded(dict(revision=1, revision_digest='a' * 64, checks=[
                       dict(argv=LINT, exit_code=0, complete=True), dict(argv=PLACEMENT, exit_code=1, complete=True)]))))
        db.commit()
    return store, machine, channel


def _unacknowledged(store, task_id):
    with store.connection() as db:
        return dropped_failed_checks_unacknowledged(db, task_id)


def test_removing_a_failed_check_needs_the_users_answer_before_completion(tmp_path):
    store, machine, channel = _failed_verification(tmp_path)
    machine.submit_verification(channel, [LINT], 'drop')
    assert _unacknowledged(store, channel.task_id) == [PLACEMENT]
    with store.connection() as db:
        revised = json.loads(db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                                        "AND type='VERIFICATION_REVISED'", (channel.task_id,)).fetchone()[0])
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                   "VALUES(?,'USER_ANSWER_RECEIVED','runtime','{}','2026-10-22T01:00:00+00:00')", (channel.task_id,))
        db.commit()
    assert revised['dropped_failed'] == [PLACEMENT]
    assert _unacknowledged(store, channel.task_id) == []


def test_correcting_a_failed_command_one_for_one_is_not_a_removal(tmp_path):
    store, machine, channel = _failed_verification(tmp_path)
    machine.submit_verification(channel, [LINT, ['python3', '-B', 'scripts/check-shell-placement.py', '--capture']],
                                'correct')
    assert _unacknowledged(store, channel.task_id) == []


def test_completion_names_the_dropped_check_for_the_coordinators_question():
    request = controller._ask_user_request(dict(ask_user_required='completion_dropped_failed_check'))
    assert 'removed a check that had failed' in request and 'user_question_request' in request


@pytest.mark.requires_bwrap
@pytest.mark.real_verification_sandbox
def test_a_private_session_bus_works_in_runtime_verification(tmp_path):
    if not shutil.which('dbus-run-session') or not shutil.which('gdbus'):
        pytest.skip('dbus-run-session or gdbus is not installed')
    root = tmp_path / 'project'
    root.mkdir()
    subprocess.run(['git', 'init', '-q', str(root)], check=True)
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    argv = ['dbus-run-session', '--', 'gdbus', 'call', '--session', '--dest', 'org.freedesktop.DBus',
            '--object-path', '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.GetId']
    with controller._verification_command(root, scratch, argv) as (command, fds):
        result = subprocess.run(command, pass_fds=fds, capture_output=True, text=True,
                                env={'PATH': '/usr/bin:/bin'}, stdin=subprocess.DEVNULL, timeout=60)
    assert result.returncode == 0, result.stderr[-400:]
    assert 'AppArmor' not in result.stderr
