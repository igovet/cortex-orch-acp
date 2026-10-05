"""No-paid: scheduler waits are shown in the chat as runtime cards."""
import json
from types import SimpleNamespace

from cortex_runtime.bootstrap import bootstrap_task
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
        objective='Implement part 0', running=True, held='.tmp/**', requested='.tmp/study/**',
        role_waiting='worker-backend-dev')
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
