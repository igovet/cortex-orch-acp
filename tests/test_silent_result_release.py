"""A worker that published a report but ended its turn without result_submit
is still shut down and released, and a close-out held by a live worker names it.

Live (2026-11-02): the silent-result fallback keyed its worker_result by the
assignment's idempotency key, the finished-worker shutdown treated that key as
an attempt id, the real engine was never stopped, and both close-outs stopped
with ``workers_still_live_or_unknown`` and a question that named no worker.
No real engine subprocess and no paid model call: in-process fakes only.
"""
import json
import threading
import uuid

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration import controller
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report


class _OwnedEngine:
    """Owns exactly the attempts it was given; a stop under any other id is a no-op."""

    def __init__(self, owned):
        self.owned = set(owned)
        self.finished = []
        self.terminated = []

    def finish_owned(self, attempt_id):
        self.finished.append(attempt_id)
        self.owned.discard(attempt_id)

    def terminate_owned(self, attempt_id):
        self.terminated.append(attempt_id)
        self.owned.discard(attempt_id)

    def lookup_owned(self, attempt_id):
        return 'alive' if attempt_id in self.owned else 'absent'


def _silent_worker(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='silent-release', event_id=str(uuid.uuid4())), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    with store.connection() as db:
        db.execute('INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   ('ara_' + uuid.uuid4().hex[:12], worker.attempt_id, task.task_id, 'worker-backend-dev',
                    'codex-acp', 'cn_fixture', 'fixture-model', 'fp', None, 'authorized', '{}', None, store._now()))
        db.commit()
    publish_fixture_report(store, worker, 'Finished the work.')
    runtime = object.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._lock = threading.Lock()
    runtime._worker_turns_in_flight = set()
    runtime._terminated_attempts = set()
    runtime._cleanup_isolated_workspace_if_integrated = lambda *_: None
    runtime._notify = lambda: None
    runtime._engine = _OwnedEngine({worker.attempt_id})
    # The turn ended without result_submit: the runtime's fallback records
    # the published report as the result, keyed by the assignment ref.
    runtime._close_assignment_that_produced_nothing(task.task_id, assignment)
    with store.connection() as db:
        silent = db.execute("SELECT assignment_ref FROM task_events WHERE task_id=? AND dedup_key=?",
                            (task.task_id, f'assignment_silent_result:{assignment}')).fetchone()
    assert silent['assignment_ref'] == assignment != worker.attempt_id
    return store, task, worker, assignment, runtime


def test_a_silent_published_result_stops_and_releases_the_real_engine(tmp_path):
    store, task, worker, assignment, runtime = _silent_worker(tmp_path)
    runtime._terminate_completed_workers(task.task_id)
    # Before the fix the stop went to the assignment ref, which no engine
    # owns, and the real attempt stayed alive and unreleased.
    assert runtime._engine.finished == [worker.attempt_id]
    assert assignment not in runtime._engine.finished + runtime._engine.terminated
    assert runtime._engine.lookup_owned(worker.attempt_id) == 'absent'
    with store.connection() as db:
        released = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_RELEASED'", (task.task_id,))]
    assert released == [dict(assignment_ref=assignment, attempt_id=worker.attempt_id, observed_session='absent')]
    # The close-out liveness gate no longer holds the task.
    assert controller._live_or_unknown_workers(runtime, task.task_id) == []


def test_a_provisioned_assignment_without_an_attempt_is_never_stopped_under_its_key(tmp_path):
    store, task, worker, assignment, runtime = _silent_worker(tmp_path)
    with store.connection() as db:
        db.execute('UPDATE delegation_provisioning SET attempt_id=NULL WHERE idempotency_key=?', (assignment,))
        db.commit()
    runtime._terminate_completed_workers(task.task_id)
    assert runtime._engine.finished == [] and runtime._engine.terminated == []
    # Nothing was released; the engine the runtime still owns stays owned.
    assert runtime._engine.lookup_owned(worker.attempt_id) == 'alive'


def test_the_stop_question_names_the_worker_that_holds_the_close_out(tmp_path):
    store, task, worker, assignment, runtime = _silent_worker(tmp_path)
    [blocking] = controller._live_or_unknown_workers(runtime, task.task_id)
    node_id = 'fixture_' + assignment[:12]
    assert blocking == f'{node_id} (worker-backend-dev, codex-acp): alive'
    blocker = controller._live_workers_blocker([blocking])
    assert blocker == f'workers_still_live_or_unknown:{blocking}'
    for language, reason in (('en', 'some workers could not be confirmed stopped'),
                             ('ru', 'не удалось подтвердить, что все исполнители остановлены')):
        question = controller._stop_question(dict(blocker=blocker, question_id='q'), dict(nodes=[]), language)['question']
        assert f'{reason} ({blocking})' in question
        assert 'codex-acp' in question and node_id in question


def test_the_live_workers_blocker_is_bounded():
    many = [f'node_{index} (worker-backend-dev, codex-acp): alive' for index in range(9)]
    blocker = controller._live_workers_blocker(many)
    assert blocker.startswith('workers_still_live_or_unknown:node_0 ')
    assert 'node_4 ' in blocker and 'node_5 ' not in blocker and blocker.endswith('; +4 more')
