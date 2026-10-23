"""Focused orchestration boundary regressions; no paid engines."""
import json
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import questions_store, replay_store
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.codex_tool_cheat_sheet import codex_tool_cheat_sheet
from cortex_runtime.orchestration import access, controller, policy
from cortex_runtime.orchestration.state import TaskMachine, encoded, event
from cortex_runtime.store import Store
from test_dropped_verification_checks import (
    LINT, PLACEMENT, _failed_verification, _unacknowledged,
)
from test_foreign_project_change import _task, _reconciling, _writer_action
from test_runtime_task_machine import machine, plan  # noqa: F401
from test_runtime_gateway import context  # noqa: F401
from test_scheduler_wait_notices import second_task
from test_targeted_review_fixes import _review, _fixes
from test_worker_wait_continuation import bound, _ask  # noqa: F401


ALIASES = ['./src/shared.py', 'src/./shared.py', 'src//shared.py', 'src/shared.py/']


@pytest.mark.parametrize('alias', ALIASES)
def test_alias_writers_serialize_but_disjoint_writers_remain_parallel(machine, alias):
    state, channel = machine
    proposed = plan(3)
    for node, scope in zip(proposed['tasks'], ('src/shared.py', alias, 'docs/**')):
        node['scope'] = [scope]
    state.submit_plan(channel, proposed, 'aliases')
    state.move(channel.task_id, 'EXECUTING')
    assert {n['node_id'] for n in state.claim_ready(channel.task_id, channel.channel_id)} == {'N0', 'N2'}
    assert state.context(channel.task_id)['plan']['tasks'][1]['scope'] == ['src/shared.py']
    assert proposed['tasks'][1]['scope'] == [alias]
    assert state.submit_plan(channel, proposed, 'aliases')['state'] == 'PLAN_READY'


@pytest.mark.parametrize('alias', ALIASES)
def test_old_cross_task_alias_reservations_serialize(machine, alias):
    state, channel = machine
    other = second_task(state, ['src/shared.py'])
    # Simulate a reservation persisted by an older runtime.
    with state.store.connection() as db:
        db.execute('UPDATE runtime_nodes SET scope_json=? WHERE task_id=?',
                   (encoded([alias]), other.task_id))
        db.commit()
    proposed = plan(1)
    proposed['tasks'][0]['scope'] = ['src/shared.py']
    state.submit_plan(channel, proposed, 'plan')
    state.move(channel.task_id, 'EXECUTING')
    assert state.claim_ready(channel.task_id, channel.channel_id) == []
    with state.store.connection() as db:
        assert access.absolute_scope(db, other.task_id, [alias]) == [
            str(Path(state.store.project_root) / 'src/shared.py')]


@pytest.mark.parametrize('alias', ALIASES)
def test_alias_scope_cannot_bypass_owner_denial(context, alias):
    store, channel, state, _ = context
    policy.steer(store, channel.task_id, 'deny-write: src/shared.py', 'denial')
    assert policy.apply_pending_steering(state.store, channel.task_id)
    proposed = plan(1)
    proposed['tasks'][0]['scope'] = [alias]
    with pytest.raises(StoreError, match='scope_conflicts_with_user_policy'):
        mcp_tools.dispatch(state.store, channel, 'plan_submit', dict(plan=proposed, request_key='deny'))


def test_scope_canonicalizer_keeps_root_and_globs():
    from cortex_runtime.orchestration.project_paths import canonical_scope
    assert canonical_scope('./') == '.'
    assert canonical_scope('./src//./a*/[xy]?.py/') == 'src/a*/[xy]?.py'
    assert policy.overlaps('./src//a*/', 'src/abc/file')
    assert not policy.overlaps('./src/a', 'src/ab')


def _append(store, task_id, kind, payload):
    with store.connection() as db:
        event(db, task_id, kind, payload)
        db.commit()


def _fail_lint(store, task_id):
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", (task_id,))
        event(db, task_id, 'VERIFICATION_FAILED', dict(revision=1, checks=[
            dict(argv=LINT, exit_code=1, complete=True)]))
        db.commit()


def test_second_repair_retains_dropped_failure_across_reopen(tmp_path):
    store, state, channel = _failed_verification(tmp_path)
    state.submit_verification(channel, [LINT], 'drop')
    _fail_lint(store, channel.task_id)
    state.submit_verification(channel, [['python3', '-B', '-c', 'pass']], 'correct')
    assert _unacknowledged(store, channel.task_id) == [PLACEMENT]
    reopened = Store(tmp_path / '.cortex', project_root=tmp_path, initialize=False)
    assert _unacknowledged(reopened, channel.task_id) == [PLACEMENT]


def test_dropped_failure_needs_exact_complete_passing_rerun(tmp_path):
    store, state, channel = _failed_verification(tmp_path)
    state.submit_verification(channel, [LINT], 'drop')
    _append(store, channel.task_id, 'VERIFICATION_PASSED', dict(revision=1, checks=[
        dict(argv=PLACEMENT, exit_code=0, complete=False)]))
    assert _unacknowledged(store, channel.task_id) == [PLACEMENT]
    _append(store, channel.task_id, 'VERIFICATION_PASSED', dict(revision=1, checks=[
        dict(argv=PLACEMENT + ['--different'], exit_code=0, complete=True)]))
    assert _unacknowledged(store, channel.task_id) == [PLACEMENT]
    _append(store, channel.task_id, 'VERIFICATION_PASSED', dict(revision=1, checks=[
        dict(argv=PLACEMENT, exit_code=0, complete=True)]))
    assert _unacknowledged(store, channel.task_id) == []


def test_only_answer_to_later_global_question_acknowledges_drop(tmp_path):
    store, state, channel = _failed_verification(tmp_path)
    _append(store, channel.task_id, 'USER_QUESTION_REQUESTED', dict(question_id='old'))
    state.submit_verification(channel, [LINT], 'drop')
    for kind, payload in [
        ('USER_ANSWER_RECEIVED', dict(question_id='old')),
        ('USER_ANSWER_RECEIVED', dict(question_id='unknown')),
        ('WORKER_USER_ANSWER_RECEIVED', dict(question_id='worker')),
    ]:
        _append(store, channel.task_id, kind, payload)
        assert _unacknowledged(store, channel.task_id) == [PLACEMENT]
    _append(store, channel.task_id, 'USER_QUESTION_REQUESTED', dict(question_id='new'))
    _append(store, channel.task_id, 'USER_ANSWER_RECEIVED', dict(question_id='new'))
    assert _unacknowledged(store, channel.task_id) == []
    # A fresh removal after acknowledgement is a fresh obligation.
    _append(store, channel.task_id, 'VERIFICATION_REVISED', dict(dropped_failed=[PLACEMENT, PLACEMENT]))
    assert _unacknowledged(store, channel.task_id) == [PLACEMENT]


@pytest.mark.parametrize('transport', ['elicitation', 'chat'])
def test_worker_answer_resets_budget_once_but_does_not_acknowledge_drop(bound, transport):
    store, state, coordinator, workers = bound
    for _ in range(3):
        _append(store, coordinator.task_id, 'REPLAN_REQUEST_ACCEPTED', {})
    _append(store, coordinator.task_id, 'VERIFICATION_REVISED', dict(dropped_failed=[PLACEMENT]))
    receipt = _ask(store, workers[0])
    if transport == 'elicitation':
        assert questions_store.answer(store, receipt['question_id'], 'small', 'elicitation')
    else:
        _, answered = replay_store.record_user_message_answer_event(
            store, coordinator.task_id, 'real-user:reset', 'incoming', dict(content='small'),
            question_id=receipt['question_id'], answer_text='small')
        assert answered
    assert not questions_store.answer(store, receipt['question_id'], 'duplicate', 'text')
    with store.connection() as db:
        assert policy.replans_since_user_input(db, coordinator.task_id) == 0
        assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE type='WORKER_USER_ANSWER_RECEIVED'").fetchone()[0] == 1
    assert _unacknowledged(store, coordinator.task_id) == [PLACEMENT]
    assert mcp_tools.dispatch(store, coordinator, 'replan_request',
                             dict(reason='Use the answer.', request_key='after-answer'))['accepted']


def test_partial_batch_and_access_decisions_do_not_reset_budget(bound):
    store, _, coordinator, workers = bound
    for _ in range(3):
        _append(store, coordinator.task_id, 'REPLAN_REQUEST_ACCEPTED', {})
    receipt = mcp_tools.dispatch(store, workers[0], 'ask_question', dict(questions=[
        dict(question='Which target?', options=['small', 'large']),
        dict(question='Which format?', options=['json', 'text']),
    ], request_key='batch'))
    assert questions_store.answer(store, receipt['question_ids'][0], 'small', 'text')
    decision = mcp_tools.dispatch(store, workers[1], 'access_request', dict(
        paths=['extra/'], reason='Implement the additional file.', request_key='extra-scope'))
    assert decision['status'] == 'granted' and decision['added_paths'] == ['extra']
    with store.connection() as db:
        assert policy.replans_since_user_input(db, coordinator.task_id) == 3
    assert questions_store.answer(store, receipt['question_ids'][1], 'json', 'elicitation')
    with store.connection() as db:
        assert policy.replans_since_user_input(db, coordinator.task_id) == 0
        assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE type='WORKER_USER_ANSWER_RECEIVED'").fetchone()[0] == 1


@pytest.mark.parametrize('attributed,exhausted,blocker,expected', [
    (True, False, 'verification_review_revision_changed', 'REVIEWING'),
    (False, False, 'verification_review_revision_changed', 'RECOVERING'),
    (True, True, 'verification_review_revision_changed', 'RECOVERING'),
    (True, False, 'verification_revision_unavailable', 'RECOVERING'),
])
def test_verification_preflight_rechecks_foreign_changes(
        tmp_path, monkeypatch, attributed, exhausted, blocker, expected):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    state, task_id, channel = _task(store, 'Finish CI')
    _, other, _ = _task(store, 'Other writer')
    _reconciling(store, state, task_id, channel)
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='VERIFYING',review_cycle=? WHERE task_id=?",
                   (3 if exhausted else 1, task_id))
        db.commit()
    if attributed:
        _writer_action(store, other, 'project_edit', '2026-10-22T13:26:28+00:00')
    monkeypatch.setattr(controller, '_verification_observation', lambda *a: dict(blocker=blocker))
    controller._verification(SimpleNamespace(), task_id, state, None, threading.Event())
    assert state.get_state(task_id) == expected
    if expected == 'REVIEWING':
        assert state.context(task_id)['review_cycle'] == 2
        assert state.context(task_id)['nodes'][-1]['node_id'] == '_review_2'


@pytest.mark.parametrize('persisted', [False, True])
def test_planned_answer_with_dropped_check_ends_failed_not_crash_loop(tmp_path, monkeypatch, persisted):
    store, state, channel = _failed_verification(tmp_path)
    state.submit_verification(channel, [LINT], 'drop')
    _append(store, channel.task_id, 'VERIFICATION_PASSED', dict(revision=1, checks=[
        dict(argv=LINT, exit_code=0, complete=True)]))
    _append(store, channel.task_id, 'STEERING_APPLIED', {})
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", (channel.task_id,))
        db.commit()
    state.submit_answer(channel, 'The checks remain incomplete.', 'answer')
    if persisted:
        state = TaskMachine(Store(tmp_path / '.cortex', project_root=tmp_path, initialize=False))
    # Other certification gates are isolated here; the real completion method
    # still invokes the real dropped-check guard.
    def only_dropped(db, task_id, task):
        from cortex_runtime.orchestration.state import dropped_failed_checks_unacknowledged
        if dropped_failed_checks_unacknowledged(db, task_id):
            raise StoreError('completion_dropped_failed_check')
    monkeypatch.setattr(state, '_work_invariants', only_dropped)
    assert state.close_answer(channel.task_id) == 'FAILED'
    with store.connection() as db:
        task = db.execute('SELECT state,blocker FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
        assert tuple(task) == ('FAILED', 'answer_not_certified:completion_dropped_failed_check')
        assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE type='ANSWER_NOT_CERTIFIED'").fetchone()[0] == 1


@pytest.mark.parametrize('operation', ['replan_request', 'scope_request', 'escalation_request'])
def test_consultant_cannot_offer_or_execute_lifecycle_requests(bound, operation):
    store, state, coordinator, workers = bound
    with store.connection() as db:
        db.execute("UPDATE delegation_provisioning SET role_id='worker-senior-consultant' WHERE attempt_id=?",
                   (workers[0].attempt_id,))
        db.execute("UPDATE runtime_nodes SET role='worker-senior-consultant' WHERE task_id=? AND node_id='N0'",
                   (coordinator.task_id,))
        db.commit()
    offered = {tool['name'] for tool in mcp_tools.tool_definitions(
        'worker', role_id='worker-senior-consultant', stage='execution')}
    assert operation not in offered
    sheet = codex_tool_cheat_sheet('worker', role_id='worker-senior-consultant', stage='execution')
    assert not any(line.startswith('- ' + operation + ':') for line in sheet.splitlines())
    args = dict(reason='Need more evidence.', request_key=operation)
    if operation == 'scope_request':
        args['scope'] = ['src']
    with pytest.raises(StoreError, match='wrong_channel_role'):
        mcp_tools.dispatch(store, workers[0], operation, args)
    assert state.get_state(coordinator.task_id) == 'EXECUTING'
    with store.connection() as db:
        assert policy.replans_since_user_input(db, coordinator.task_id) == 0


def test_findings_contract_describes_coordinator_return(machine):
    from cortex_runtime.orchestration.contracts import RESULT
    description = RESULT['properties']['findings']['description']
    assert 'coordinator' in description and 'budget' in description
    state, channel = machine
    review = _review(state, channel, Path(state.store.project_root))
    state.add_fix(channel.task_id, review)
    assert state.get_state(channel.task_id) == 'RECOVERING'
    assert not _fixes(state, channel.task_id)


def test_scope_request_canonicalizes_but_preserves_delivery_identity(context):
    store, channel, _, _ = context
    args = dict(scope=['./src//shared.py/'], reason='The remedy needs this path.', request_key='scope')
    receipt = mcp_tools.dispatch(store, channel, 'scope_request', args)
    assert mcp_tools.dispatch(store, channel, 'scope_request', args) == receipt
    assert args['scope'] == ['./src//shared.py/']
    with store.connection() as db:
        saved = json.loads(db.execute(
            "SELECT payload_json FROM runtime_events WHERE type='REPLAN_REQUEST_ACCEPTED'").fetchone()[0])
    assert saved['proposal']['scope'] == ['src/shared.py']


def test_effective_scope_normalizes_older_access_grants(bound):
    store, _, coordinator, _ = bound
    with store.connection() as db:
        node = db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND node_id='N0'",
                          (coordinator.task_id,)).fetchone()
        event(db, coordinator.task_id, 'ACCESS_GRANTED', dict(
            assignment_ref=node['assignment_ref'], added_paths=['./extra//file', 'extra/file/']))
        db.commit()
        assert access.effective_scope(db, node) == ['part0.txt', 'extra/file']


@pytest.mark.usefixtures('unjailed_gateway_command')
def test_controller_delivers_uncertified_answer_after_reduced_verification(tmp_path, monkeypatch):
    from test_steering_after_verification import _drive
    original = TaskMachine.record_verification

    def record_with_prior_removal(self, task_id, checks, digest, **kwargs):
        # Seed only the prior removal history; real reports, review, passing
        # verification, steering, answer dispatch and every completion gate run.
        _append(self.store, task_id, 'VERIFICATION_REVISED', dict(dropped_failed=[PLACEMENT]))
        return original(self, task_id, checks, digest, **kwargs)

    def answer(store, channel, context, seen):
        mcp_tools.dispatch(store, channel, 'answer_submit', dict(
            text='The removed check remains unresolved.', request_key='answer'))

    monkeypatch.setattr(TaskMachine, 'record_verification', record_with_prior_removal)
    store, task, context, seen = _drive(tmp_path, monkeypatch, answer)
    assert context['state'] == 'FAILED'
    assert context['blocker'] == 'answer_not_certified:completion_dropped_failed_check'
    assert 'worker-code-reviewer' in seen['roles']
    assert _unacknowledged(store, task.task_id) == [PLACEMENT]
    assert 'The removed check remains unresolved.' in ''.join(seen['output'])
