"""RECOVERING cancels exactly the assignments that were live when stopped.

2026-10-31: every replan wrote assignment_cancelled (and cancelled and
terminated the attempt) for every assignment the task ever had, including
ones that had already completed or failed: 193 of one task's 219
cancellations. The stop/replan transaction now captures the live
assignments it cancels; RECOVERING acts on that durable capture only.

No real engine subprocess and no paid model call: in-process fakes only.
"""
import json
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import controller
from cortex_runtime.orchestration.policy import interrupt
from cortex_runtime.orchestration.state import TaskMachine, encoded
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel


class Engine:
    """Owned attempts by liveness; records every cancel/terminate call."""

    def __init__(self, alive=(), stubborn=()):
        self.alive = set(alive)
        self.stubborn = set(stubborn)
        self.cancelled, self.terminated = [], []

    def cancel_attempt(self, attempt_id):
        self.cancelled.append(attempt_id)

    def lookup_owned(self, attempt_id):
        return 'alive' if attempt_id in self.alive else 'absent'

    def terminate_owned(self, attempt_id):
        self.terminated.append(attempt_id)
        if attempt_id not in self.stubborn:
            self.alive.discard(attempt_id)


def _runtime(store, engine):
    # The real cancellation; only the host-facing card and graph mirrors are stubbed.
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._engine = engine
    runtime._lock = threading.Lock()
    runtime._worker_card_emitters = {}
    runtime._mark_work_attempt_failed = lambda *args, **kwargs: None
    runtime._durable_worker_card_sessions = lambda *args, **kwargs: {}
    return runtime


def _plan(machine, coordinator, names):
    machine.submit_plan(coordinator, dict(
        tasks=[dict(id=name, role='backend-dev', objective=f'Implement {name}', dependencies=[],
                    scope=[f'{name}.txt'], model='auto') for name in names],
        verification=[['python3', '-B', '-c', 'print("verified")']]), f'plan-{"-".join(names)}')
    machine.move(coordinator.task_id, 'EXECUTING')
    nodes = machine.claim_ready(coordinator.task_id, coordinator.channel_id)
    assert sorted(node['node_id'] for node in nodes) == sorted(names)
    refs, workers = {}, {}
    with machine.store.connection() as db:
        for node in nodes:
            name = node['node_id']
            channel_id, secret = issue_channel_tx(
                db, coordinator.task_id, 'worker', coordinator.owner_epoch,
                parent_channel_id=coordinator.channel_id, assignment_id=f'a_{name}', attempt_id=f'xa_{name}')
            stamp = machine.store._now()
            # 'provisioning' attempts are still being opened; 'ready' ones run.
            step, status = ('provision', 'provisioning') if name.startswith('P') else ('ready', 'ready')
            db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                       (node['assignment_ref'], coordinator.task_id, f'a_{name}', f'xa_{name}',
                        resolve_role('backend-dev'), step, status, 'shared_project_root', channel_id,
                        None, None, f'es_{name}', 'qualified', None, coordinator.owner_epoch, stamp, stamp))
            refs[name] = node['assignment_ref']
            workers[name] = (channel_id, secret)
        db.commit()
    return refs, {name: resolve_channel(machine.store, *pair) for name, pair in workers.items()}


def _finish(store, task_id, assignment_ref, status):
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND assignment_ref=?',
                   (status, encoded(dict(summary=status, checks=[])), task_id, assignment_ref))
        db.commit()


def _cancelled(store, task_id):
    with store.connection() as db:
        return sorted(row[0] for row in db.execute(
            "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='assignment_cancelled'",
            (task_id,)))


@pytest.fixture
def task(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='recovery-targets', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    return store, machine, coordinator


def _stop(path, store, machine, task_id, worker):
    if path == 'replan_request':
        assert mcp_tools.dispatch(store, worker, 'replan_request',
                                  dict(reason='Needs another approach.', request_key='replan'))['accepted']
    elif path == 'return_to_coordinator':
        assert machine.return_to_coordinator(task_id, 'worker_failed')
    elif path == 'interrupt':
        interrupt(store, task_id)
    elif path == 'stop_question_by_coordinator':
        # A work-level stop the coordinator asks about: straight to RECOVERING.
        machine.move(task_id, 'BLOCKED', blocker='worker_failed')
    else:
        # The runtime's own stop question; its answer resumes in RECOVERING.
        machine.move(task_id, 'BLOCKED', blocker='coordinator_prompt_failed:RuntimeError')
        assert machine.get_state(task_id) == 'WAITING_USER'
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute("UPDATE runtime_tasks SET state='RECOVERING' WHERE task_id=?", (task_id,))
            db.commit()
    assert machine.get_state(task_id) == 'RECOVERING'


PATHS = ['replan_request', 'return_to_coordinator', 'interrupt', 'stop_question_by_coordinator', 'stop_question']


@pytest.mark.parametrize('path', PATHS)
def test_recovery_cancels_and_terminates_only_the_assignments_that_were_live(task, path):
    store, machine, coordinator = task
    task_id = coordinator.task_id
    refs, workers = _plan(machine, coordinator, ['done', 'broke', 'live', 'P_open'])
    _finish(store, task_id, refs['done'], 'completed')
    _finish(store, task_id, refs['broke'], 'failed')
    # A completed worker can still be settling; it is not this recovery's to kill.
    engine = Engine(alive={'xa_done', 'xa_live', 'xa_P_open'})
    _stop(path, store, machine, task_id, workers['live'])
    assert controller._recover(_runtime(store, engine), task_id, machine, None) is True
    assert machine.get_state(task_id) == 'PLANNING'
    # Before the fix 'done' and 'broke' were cancelled too, and terminated.
    assert _cancelled(store, task_id) == sorted([refs['live'], refs['P_open']])
    assert sorted(engine.cancelled) == ['xa_P_open', 'xa_live']
    assert sorted(set(engine.terminated)) == ['xa_P_open', 'xa_live']
    assert 'xa_done' in engine.alive


def test_a_later_recovery_leaves_assignments_an_earlier_one_settled(task):
    store, machine, coordinator = task
    task_id = coordinator.task_id
    first, workers = _plan(machine, coordinator, ['one'])
    _stop('replan_request', store, machine, task_id, workers['one'])
    assert controller._recover(_runtime(store, Engine(alive={'xa_one'})), task_id, machine, None)
    second, _ = _plan(machine, coordinator, ['two'])
    interrupt(store, task_id)
    engine = Engine(alive={'xa_two'})
    assert controller._recover(_runtime(store, engine), task_id, machine, None)
    assert engine.cancelled == ['xa_two'] and set(engine.terminated) == {'xa_two'}
    assert _cancelled(store, task_id) == sorted([first['one'], second['two']])


def test_a_restart_in_recovering_still_cancels_the_captured_live_set(task, tmp_path):
    store, machine, coordinator = task
    task_id = coordinator.task_id
    refs, workers = _plan(machine, coordinator, ['done', 'live'])
    _finish(store, task_id, refs['done'], 'completed')
    _stop('replan_request', store, machine, task_id, workers['live'])
    # The process restarts before RECOVERING ran: a fresh store, machine and
    # engine whose durable registry still reports the live worker.
    restarted = Store(tmp_path / '.cortex', project_root=tmp_path)
    machine = TaskMachine(restarted)
    engine = Engine(alive={'xa_done', 'xa_live'}, stubborn={'xa_live'})
    assert controller._recover(_runtime(restarted, engine), task_id, machine, None) is False
    # Termination unconfirmed: a stop question; its answer re-enters RECOVERING.
    assert machine.get_state(task_id) == 'WAITING_USER'
    assert _cancelled(restarted, task_id) == [refs['live']]
    with restarted.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute("UPDATE runtime_tasks SET state='RECOVERING' WHERE task_id=?", (task_id,))
        db.commit()
    engine.stubborn.clear()
    assert controller._recover(_runtime(restarted, engine), task_id, machine, None) is True
    # The unsettled capture was checked again; the completed worker never was.
    assert engine.terminated.count('xa_live') >= 2 and 'xa_done' not in engine.terminated
    assert _cancelled(restarted, task_id) == [refs['live']]


def test_a_stop_recorded_before_captures_existed_is_still_cancelled(task):
    store, machine, coordinator = task
    task_id = coordinator.task_id
    refs, _ = _plan(machine, coordinator, ['done', 'live'])
    _finish(store, task_id, refs['done'], 'completed')
    # What an older runtime committed: the nodes cancelled, nothing captured.
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",
                   (task_id,))
        db.execute("UPDATE runtime_tasks SET state='RECOVERING' WHERE task_id=?", (task_id,))
        db.commit()
    engine = Engine(alive={'xa_done', 'xa_live'})
    assert controller._recover(_runtime(store, engine), task_id, machine, None) is True
    assert _cancelled(store, task_id) == [refs['live']]
    assert set(engine.terminated) == {'xa_live'}
    with store.connection() as db:
        captured = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='LIVE_ASSIGNMENTS_CANCELLED'",
            (task_id,))]
    assert captured == [dict(assignment_refs=[refs['live']], reason='unrecorded')]
