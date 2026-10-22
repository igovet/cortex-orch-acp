"""Two Cortex tasks in one project: a change by the other task's writer is
attributed and the work is reviewed and verified again, without a question.

Observed 2026-10-22: one task's edit invalidated another task's verified
completion three seconds before it finished; the user was asked what to do
while the runtime's own store named the writer, and two "stop" answers then
looped into a force-cancel.
"""
import json
import uuid
from types import SimpleNamespace

from cortex_runtime.bootstrap import bootstrap_task

from cortex_runtime.orchestration.state import TaskMachine, encoded, foreign_project_writers
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(store, goal):
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=str(uuid.uuid4())), goal, 'owner')
    machine = TaskMachine(store)
    machine.create(task.task_id, goal)
    channel = SimpleNamespace(task_id=task.task_id, channel_id='coord-' + task.task_id, role='coordinator',
                              principal_thread_id='owner', parent_principal_thread_id=None)
    return machine, task.task_id, channel


def _plan():
    return {'tasks': [{'id': 'N0', 'role': 'backend-dev', 'objective': 'Change part', 'dependencies': [],
                       'scope': ['src/**'], 'model': 'auto'}],
            'verification': [['python3', '-B', '-c', 'pass']]}


def _reconciling(store, machine, task_id, channel):
    machine.submit_plan(channel, _plan(), 'plan-' + task_id)
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (task_id,))
        db.execute("UPDATE runtime_tasks SET state='RECONCILING',synthesis='Done.',review_cycle=1 WHERE task_id=?",
                   (task_id,))
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                   "VALUES(?,'REVIEW_STARTED','runtime',?,'2026-10-22T13:00:00+00:00')",
                   (task_id, encoded(dict(assignment_ref='r', revision_digest='a' * 64))))
        db.commit()


def _writer_action(store, task_id, operation, created_at, role='worker-backend-dev'):
    channel = 'ch_' + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (uuid.uuid4().hex, task_id, 'a_' + uuid.uuid4().hex[:12], 'xa_' + uuid.uuid4().hex[:12], role,
                    'ready', 'ready', 'shared_project_root', channel, None, None, None, 'qualified', None, 1,
                    created_at, created_at))
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                   "VALUES(?,'TOOL_AUTHORIZED',?,?,?)",
                   (task_id, channel, json.dumps(dict(operation=operation, tool_call_ref='gateway:x')), created_at))
        db.commit()


def test_another_tasks_write_starts_a_new_review_instead_of_a_question(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    machine, finishing, channel = _task(store, 'Finish CI')
    _, other, _ = _task(store, 'Remediate audit')
    _reconciling(store, machine, finishing, channel)
    _writer_action(store, other, 'project_edit', '2026-10-22T13:26:28+00:00')

    assert machine.recheck_after_foreign_change(finishing, 'completion_project_revision_changed') == [other]
    with store.connection() as db:
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (finishing,)).fetchone()
        review = db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND stage='review'", (finishing,)).fetchone()
        changed = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                             "AND type='PROJECT_CHANGED_BY_OTHER_TASK'", (finishing,)).fetchone()
        asked = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='ASK_USER_REQUIRED'",
                           (finishing,)).fetchone()[0]
    assert task['state'] == 'REVIEWING' and task['synthesis'] is None and task['review_cycle'] == 2
    assert review['node_id'] == '_review_2' and review['status'] == 'pending'
    assert json.loads(changed[0])['tasks'] == [other]
    assert asked == 0


def test_an_unattributed_change_still_stops_for_the_user(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    machine, finishing, channel = _task(store, 'Finish CI')
    _, other, _ = _task(store, 'Read only audit')
    _reconciling(store, machine, finishing, channel)
    # Before the review bound the revision, and a read-only role's command:
    # neither explains the change.
    _writer_action(store, other, 'project_write', '2026-10-22T12:00:00+00:00')
    _writer_action(store, other, 'command_run', '2026-10-22T13:30:00+00:00', role='worker-code-reviewer')

    with store.connection() as db:
        assert foreign_project_writers(db, finishing, '2026-10-22T13:00:00+00:00') == []
    assert machine.recheck_after_foreign_change(finishing, 'completion_project_revision_changed') is None
    assert machine.get_state(finishing) == 'RECONCILING'
