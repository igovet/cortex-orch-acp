"""No-paid: scheduler waits are shown in the chat as runtime cards."""
import json
from types import SimpleNamespace

import os

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.engine import process_identity
from cortex_runtime.recovery.registry import mark_terminated, record_process_launch
from cortex_runtime.orchestration import wait_notices
from cortex_runtime.orchestration.state import TaskMachine, event
from cortex_runtime.portable.identity import SourceEvent
from test_runtime_task_machine import machine, plan  # noqa: F401


def waits(state):
    with state.store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE type='SCHEDULER_WAIT_REASON' ORDER BY id")]


def second_task(state, scope):
    task = bootstrap_task(state.store, SourceEvent(origin='test', event_id='two'), 'Other chat', 'owner')
    state.create(task.task_id, 'Other chat')
    other = SimpleNamespace(task_id=task.task_id, channel_id='coordinator', role='coordinator')
    proposed = plan(1)
    proposed['tasks'][0].update(id='holder_node', role='data-engineer', scope=scope)
    state.submit_plan(other, proposed, 'plan')
    state.move(other.task_id, 'EXECUTING')
    assert state.claim_ready(other.task_id, other.channel_id)
    return other


def test_cross_chat_scope_wait_names_the_holder_and_closes_when_released(machine):
    state, ch = machine
    other = second_task(state, ['.tmp/**'])
    proposed = plan(1)
    proposed['tasks'][0].update(id='backtest', scope=['.tmp/study/**'])
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    assert state.claim_ready(ch.task_id, ch.channel_id) == []
    recorded = waits(state)[-1]
    assert recorded['reason'] == 'scope_overlap'
    assert recorded['blocked_by'] == dict(
        kind='worker', task_id=other.task_id, node='holder_node', role='worker-data-engineer',
        objective='Implement part 0', running=True, assignment_ref=recorded['blocked_by']['assignment_ref'],
        held='.tmp/**', requested='.tmp/study/**', role_waiting='worker-backend-dev')
    assert recorded['blocked_by']['assignment_ref']
    # Repeated passes over the same wait record nothing new.
    state.claim_ready(ch.task_id, ch.channel_id)
    assert waits(state)[-1] == recorded

    sent, cards = [], {}
    wait_notices.show(state.store, ch.task_id, sent.append, 'ru', cards)
    assert len(sent) == 1 and sent[0]['new'] and sent[0]['status'] == 'in_progress'
    assert 'backtest' in sent[0]['title'] and 'holder_node' in sent[0]['title']
    assert 'другого чата' in sent[0]['text'] and other.task_id in sent[0]['text']
    assert '.tmp/** ↔ .tmp/study/**' in sent[0]['text']
    wait_notices.show(state.store, ch.task_id, sent.append, 'ru', cards)
    assert len(sent) == 1

    with state.store.connection() as db:
        ref = db.execute("SELECT assignment_ref FROM runtime_nodes WHERE task_id=?", (other.task_id,)).fetchone()[0]
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (other.task_id,))
        event(db, other.task_id, 'WORKER_RELEASED', dict(assignment_ref=ref))
        db.commit()
    assert [n['node_id'] for n in state.claim_ready(ch.task_id, ch.channel_id)] == ['backtest']
    wait_notices.show(state.store, ch.task_id, sent.append, 'ru', cards)
    closed = sent[-1]
    assert closed['tool_call_id'] == sent[0]['tool_call_id'] and closed['status'] == 'completed'
    assert closed['text'].startswith(sent[0]['text']) and 'шаг запускается' in closed['text']
    assert cards == {}


def test_same_task_and_capacity_waits_are_shown_but_dependencies_are_not(machine):
    state, ch = machine
    proposed = plan(7)
    proposed['tasks'][1]['scope'] = proposed['tasks'][0]['scope']
    proposed['tasks'][2]['dependencies'] = ['N0']
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    state.claim_ready(ch.task_id, ch.channel_id)
    sent, cards = [], {}
    wait_notices.show(state.store, ch.task_id, sent.append, 'en', cards)
    titles = sorted(card['title'] for card in sent)
    assert titles == ['Waiting: N1 — paths held by N0', 'Waiting: N6 — no free worker slot']
    assert any('this task' in card['text'] for card in sent)
    wait_notices.close_all(ch.task_id, sent.append, 'en', cards)
    assert [card['status'] for card in sent[-2:]] == ['completed', 'completed']
    assert all('turn ended' in card['text'] for card in sent[-2:]) and cards == {}


def test_a_new_holder_is_a_new_wait(machine):
    state, ch = machine
    first = second_task(state, ['.tmp/**'])
    proposed = plan(1)
    proposed['tasks'][0].update(id='backtest', scope=['.tmp/study/**'])
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    state.claim_ready(ch.task_id, ch.channel_id)
    sent, cards = [], {}
    wait_notices.show(state.store, ch.task_id, sent.append, 'en', cards)
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET node_id='renamed_holder' WHERE task_id=?", (first.task_id,))
        db.commit()
    state.claim_ready(ch.task_id, ch.channel_id)
    assert [w['blocked_by']['node'] for w in waits(state)] == ['holder_node', 'renamed_holder']
    wait_notices.show(state.store, ch.task_id, sent.append, 'en', cards)
    assert sent[1]['status'] == 'completed' and 'newer waiting card' in sent[1]['text']
    assert sent[2]['new'] and 'renamed_holder' in sent[2]['title']


def holder_process(state, holder_task, *, pid=None, start_ticks=None):
    """Bind the holder's assignment to a recorded worker process."""
    pid = pid or os.getpid()
    with state.store.connection() as db:
        ref = db.execute("SELECT assignment_ref FROM runtime_nodes WHERE task_id=?", (holder_task,)).fetchone()[0]
        stamp = state.store._now()
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,"
                   "step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                   (ref, holder_task, 'a_holder', 'xa_holder', 'worker-data-engineer', 'ready', 'ready', 1, stamp, stamp))
        db.commit()
    record_process_launch(state.store, attempt_id='xa_holder', task_id=holder_task, role='worker', pid=pid,
                          pgid=pid, start_ticks=start_ticks or process_identity.read_start_ticks(pid),
                          boot_id=process_identity.read_boot_id(), launch_command_digest='test')


def waiting_backtest(state, ch):
    proposed = plan(1)
    proposed['tasks'][0].update(id='backtest', scope=['.tmp/study/**'])
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    assert state.claim_ready(ch.task_id, ch.channel_id) == []


def test_a_live_holder_in_another_chat_suspends_the_wait_deadline(machine):
    state, ch = machine
    other = second_task(state, ['.tmp/**'])
    holder_process(state, other.task_id)
    waiting_backtest(state, ch)
    assert wait_notices.waiting_on_live_holders(state.store, ch.task_id)


@pytest.mark.parametrize('ending', ['terminated', 'pid_reused', 'unrecorded'])
def test_a_gone_or_unverified_holder_keeps_the_deadline(machine, ending):
    state, ch = machine
    other = second_task(state, ['.tmp/**'])
    if ending != 'unrecorded':
        holder_process(state, other.task_id, start_ticks='1' if ending == 'pid_reused' else None)
        if ending == 'terminated':
            mark_terminated(state.store, 'xa_holder', 'process_exited')
    waiting_backtest(state, ch)
    assert not wait_notices.waiting_on_live_holders(state.store, ch.task_id)


def test_a_capacity_wait_keeps_the_deadline(machine):
    state, ch = machine
    proposed = plan(5)
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    state.claim_ready(ch.task_id, ch.channel_id)
    assert not wait_notices.waiting_on_live_holders(state.store, ch.task_id)


class _SchedulerRuntime:
    """Controller dependencies for a task with no worker of its own."""

    def __init__(self, store):
        import threading
        self._store = store
        self._running = {}
        self._lock = threading.RLock()
        self._cond = threading.Condition(self._lock)
        self._terminate_completed_workers = lambda *_args, **_kwargs: None
        self._process_new_events_safely = lambda *_args, **_kwargs: None
        self._emit_assignment_tool_calls = lambda *_args, **_kwargs: None
        self._set_state = lambda *_args, **_kwargs: None


@pytest.mark.parametrize('holder_lives', [True, False])
def test_the_scheduler_deadline_applies_only_when_the_holder_is_not_alive(machine, monkeypatch, holder_lives):
    import threading
    import time as real_time
    import cortex_runtime.orchestration.controller as controller_module
    import cortex_runtime.orchestration.pipeline as pipeline
    from cortex_runtime.portable.engine_contract import StopReason

    state, ch = machine
    other = second_task(state, ['.tmp/**'])
    holder_process(state, other.task_id)
    if not holder_lives:
        mark_terminated(state.store, 'xa_holder', 'process_exited')
    waiting_backtest(state, ch)

    # Every clock read is ten minutes later: the 30-minute deadline would
    # pass within a few scheduler passes.
    clock = {'now': real_time.monotonic()}

    def monotonic():
        clock['now'] += 600
        return clock['now']
    monkeypatch.setattr(controller_module, 'time', SimpleNamespace(
        monotonic=monotonic, monotonic_ns=real_time.monotonic_ns))
    monkeypatch.setattr(controller_module, '_POLL_SECONDS', 0.01)
    monkeypatch.setattr(pipeline, 'publish', lambda *_args, **_kwargs: None)
    monkeypatch.setattr(controller_module, '_stop_turn', lambda *_args, **_kwargs: StopReason.CANCELLED)
    cancel_event = threading.Event()
    stops = []
    original_move = TaskMachine.move

    def move(machine_self, task_id, target, **kwargs):
        if kwargs.get('blocker') == 'scheduler_capacity_timeout':
            stops.append(task_id)
            cancel_event.set()
            return None
        return original_move(machine_self, task_id, target, **kwargs)
    monkeypatch.setattr(TaskMachine, 'move', move)
    guard = threading.Timer(1.0, cancel_event.set)
    cards = []
    guard.start()
    try:
        result = controller_module.run(
            _SchedulerRuntime(state.store), ch.task_id, 'session', ch, 'coordinator-session',
            'Run the backtest.', lambda _text: True, cancel_event, 'en', emit_tool_call=cards.append)
    finally:
        guard.cancel()
    assert result == StopReason.CANCELLED
    assert stops == ([] if holder_lives else [ch.task_id])
    with state.store.connection() as db:
        assert db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (ch.task_id,)).fetchone()[0] == 'EXECUTING'
    assert cards[0]['new'] and 'holder_node' in cards[0]['title']
    assert cards[-1]['status'] == 'completed' and 'turn ended' in cards[-1]['text']
