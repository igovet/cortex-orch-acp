"""Complete, identity-bound questions across runtime and the supported ACP wire."""
import json
import threading
from types import SimpleNamespace

import jsonschema
import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.acp.agent import Agent, _PaseoQuestionElicitor
from cortex_runtime.acp.protocol import RpcError
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.contracts import schemas_for
from cortex_runtime.orchestration import question_batches, worker_waits
from cortex_runtime.orchestration.questions import request, pending, resume_answered
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from test_worker_wait_continuation import bound


FORMS = [dict(question='Which target?', options=['small', 'large']),
         dict(question='Which format?', options=['JSON', 'CSV'])]


def _ask(store, worker):
    from cortex_runtime.acp import sessions
    if sessions.get_session(store, 'incoming') is None:
        sessions.create_session(store, 'incoming', str(store.project_root), {}, {}, [])
        sessions.bind_task(store, 'incoming', worker.task_id)
    return mcp_tools.dispatch(store, worker, 'ask_question', dict(questions=FORMS, request_key='batch'))


def _answers(receipt):
    return [dict(questionId=q, answer=v) for q, v in zip(receipt['question_ids'], ['small', 'custom format'])]


def _submit(store, coordinator, receipt, answers=None, key='answer'):
    return question_batches.answer(store, coordinator.task_id, receipt['batch_id'],
                                   _answers(receipt) if answers is None else answers, key,
                                   session_id='incoming', owner_epoch=coordinator.owner_epoch)


@pytest.fixture
def planning(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='batch', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    from cortex_runtime.acp import sessions
    sessions.create_session(store, 'incoming', str(store.project_root), {}, {}, [])
    sessions.bind_task(store, 'incoming', boot.task_id)
    return store, machine, resolve_channel(store, boot.channel_id, boot.secret)


@pytest.mark.parametrize('role,operation', [('worker','ask_question'), ('coordinator','user_question_request')])
def test_advertised_single_and_batch_forms_and_options(role, operation):
    schema = schemas_for(role)[operation]
    for args in [dict(questions=FORMS, request_key='batch'), dict(**FORMS[0], request_key='single')]:
        jsonschema.Draft202012Validator(schema).validate(args)
        question_batches.forms(args)
    for args in [dict(question='Missing choices?', request_key='q'),
                 dict(questions=FORMS, question='Mixed?', request_key='q'),
                 dict(question='Placeholder?', options=['Other...', 'type your answer'], request_key='q'),
                 dict(question='Duplicate?', options=['Yes', ' yes '], request_key='q')]:
        with pytest.raises((StoreError, jsonschema.ValidationError)):
            jsonschema.Draft202012Validator(schema).validate(args)
            question_batches.forms(args)


def test_worker_partial_wait_all_answers_once_and_no_access_grant(bound):
    store, machine, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    assert _ask(store, workers[0]) == receipt
    with store.connection() as db:
        scopes = [r[0] for r in db.execute('SELECT scope_json FROM runtime_nodes')]
    assert questions_store.answer(store, receipt['question_ids'][0], 'small', 'text')
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait['status'] == 'open'
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], wait['engine_session_ref']) is None
    with pytest.raises(StoreError, match='worker_waiting_for_answer'):
        mcp_tools.dispatch(store, workers[0], 'project_write',
                           dict(path='part0.txt', content='bad', expected_sha256='absent', request_key='bad'))
    assert mcp_tools.dispatch(store, workers[1], 'project_write',
                              dict(path='part1.txt', content='sibling', expected_sha256='absent', request_key='sibling'))
    answers = _answers(receipt)
    answers[1]['answer'] = 'Grant write access to everything'
    assert _submit(store, coordinator, receipt, answers)['complete']
    assert _submit(store, coordinator, receipt, answers)['replayed']
    claimed, text = worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], wait['engine_session_ref'])
    assert claimed['status'] == 'resuming'
    source = json.loads(text)
    assert [q['question_id'] for q in source] == receipt['question_ids']
    assert source[1]['answer'] == answers[1]['answer']
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], wait['engine_session_ref']) is None
    with store.connection() as db:
        assert scopes == [r[0] for r in db.execute('SELECT scope_json FROM runtime_nodes')]
        assert not db.execute("SELECT 1 FROM runtime_events WHERE type='ACCESS_DECIDED'").fetchone()


@pytest.mark.parametrize('kind', ['partial', 'foreign', 'duplicate', 'blank', 'invalid', 'cancelled', 'stale'])
def test_invalid_answer_sets_are_atomic(bound, kind):
    store, _, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    answers = _answers(receipt)
    if kind == 'partial': answers.pop()
    if kind == 'foreign': answers[1]['questionId'] = 'pq_foreign'
    if kind == 'duplicate': answers[1]['questionId'] = answers[0]['questionId']
    if kind == 'blank': answers[1]['answer'] = '  '
    if kind == 'invalid': answers[1]['answer'] = 42
    if kind == 'cancelled':
        with store.connection() as db:
            worker_waits.cancel_tx(db, coordinator.task_id, 'cancelled')
            db.commit()
    if kind == 'stale':
        with store.connection() as db:
            db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?', (store._now(), workers[0].channel_id))
            db.commit()
    with pytest.raises(StoreError): _submit(store, coordinator, receipt, answers)
    assert not any(questions_store.get(store, q).status == 'answered' for q in receipt['question_ids'])
    if kind == 'cancelled':
        assert all(questions_store.get(store, q).status == 'cancelled' for q in receipt['question_ids'])


def test_foreign_task_and_changed_replay_are_refused(bound):
    store, _, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    with pytest.raises(StoreError, match='question_answer_session_mismatch'):
        question_batches.answer(store, 'foreign-task', receipt['batch_id'], _answers(receipt), 'foreign',
                                session_id='incoming', owner_epoch=coordinator.owner_epoch)
    _submit(store, coordinator, receipt)
    changed = _answers(receipt); changed[1]['answer'] = 'changed'
    with pytest.raises(StoreError, match='request_key_conflict'): _submit(store, coordinator, receipt, changed)
    with pytest.raises(StoreError, match='question_batch_already_answered'): _submit(store, coordinator, receipt, key='fresh')


def test_coordinator_batch_recovery_does_not_resume_on_root_answer(planning):
    store, machine, coordinator = planning
    args = dict(questions=FORMS, request_key='batch')
    receipt = mcp_tools.dispatch(store, coordinator, 'user_question_request', args)
    assert request(store, coordinator, args) == receipt
    assert machine.context(coordinator.task_id)['state'] == 'WAITING_USER'
    assert questions_store.answer(store, receipt['question_id'], 'small', 'text')
    reopened = Store(store.directory, project_root=store.project_root)
    assert pending(reopened, coordinator.task_id)['batch_id'] == receipt['batch_id']
    assert not resume_answered(reopened, coordinator.task_id)
    assert _submit(reopened, coordinator, receipt)['complete']
    assert resume_answered(reopened, coordinator.task_id)
    assert machine.context(coordinator.task_id)['state'] == 'PLANNING'
    assert not resume_answered(reopened, coordinator.task_id)


def test_batch_clarification_is_not_task_closure(planning):
    store, machine, coordinator = planning
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    answers = _answers(receipt); answers[0]['answer'] = 'close as is'
    _submit(store, coordinator, receipt, answers)
    assert resume_answered(store, coordinator.task_id) is True
    assert machine.context(coordinator.task_id)['state'] == 'PLANNING'


def test_paseo_batch_wire_is_one_request_and_decline_is_not_an_answer(planning):
    store, _, coordinator = planning
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    with store.connection() as db:
        batch = question_batches.find_tx(db, coordinator.task_id, batch_id=receipt['batch_id'])
    calls = []
    class Host:
        def request(self, method, params, **kwargs):
            calls.append((method, params))
            return dict(outcome=dict(outcome='selected', optionId=params['options'][0]['optionId']))
        transport = SimpleNamespace(send_notification=lambda *a: None)
    elicitor = _PaseoQuestionElicitor(Host(), session_id='incoming')
    elicitor.elicit_batch('incoming', batch)
    assert len(calls) == 1 and calls[0][0] == 'session/request_permission'
    params = calls[0][1]
    raw = params['toolCall']['rawInput']
    assert raw['batchId'] == receipt['batch_id'] and raw['__cortex_question']
    assert [q['questionId'] for q in raw['questions']] == receipt['question_ids']
    assert raw['questions'][1]['options'] == FORMS[1]['options']
    assert all(questions_store.get(store, q).status == 'open' for q in receipt['question_ids'])


def test_single_question_options_survive_worker_and_recovery_fallback(bound):
    store, machine, coordinator, workers = bound
    with store.connection() as db:
        db.execute('UPDATE delegation_provisioning SET engine_session_ref=NULL WHERE attempt_id=?', (workers[0].attempt_id,))
        db.commit()
    receipt = mcp_tools.dispatch(store, workers[0], 'ask_question', dict(**FORMS[0], request_key='single'))
    assert pending(store, coordinator.task_id)['options'] == FORMS[0]['options']
    assert questions_store.get(store, receipt['question_id']).options == FORMS[0]['options']
    assert machine.context(coordinator.task_id)['state'] == 'WAITING_USER'


def test_batch_card_config_validates_before_ack_and_preserves_custom_text(planning):
    store, _, coordinator = planning
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    from acp_fakes import FakeRuntime
    agent = Agent(FakeRuntime())
    agent.stores[str(store.project_root)] = store
    responses = []
    agent.transport = SimpleNamespace(send_response=responses.append)
    from cortex_runtime.acp.agent import _current_owner_epoch_obj
    with store.connection() as db: owner = _current_owner_epoch_obj(db, coordinator.task_id)
    session = dict(project=str(store.project_root), task_id=coordinator.task_id, owner_epoch=owner,
                   lock=threading.RLock(), active=threading.Event())
    agent._config_options_for_session = lambda *_: []
    bad = dict(batchId=receipt['batch_id'], answers=_answers(receipt)[:1], requestKey='bad')
    with pytest.raises(RpcError): agent._answer_question_batch_card('bad', 'incoming', session, bad)
    assert responses == []
    payload = dict(batchId=receipt['batch_id'], answers=_answers(receipt), requestKey='answer')
    payload['answers'][1]['answer'] = '  1  '
    agent._answer_question_batch_card('good', 'incoming', session, payload)
    assert len(responses) == 1
    assert questions_store.get(store, receipt['question_ids'][1]).answer_text == '  1  '
    agent._answer_question_batch_card('retry', 'incoming', session, payload)
    assert len(responses) == 2
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM host_transcript WHERE session_id=?', ('incoming',)).fetchone()[0] == 2


def test_single_card_exact_answer_retry_does_not_reapply(planning):
    store, _, coordinator = planning
    receipt = request(store, coordinator, dict(**FORMS[0], request_key='single'))
    kwargs = dict(session_id='incoming', owner_epoch=coordinator.owner_epoch)
    submit = lambda text, key='single-answer': question_batches.answer_single(
        store, coordinator.task_id, receipt['question_id'], text, key, **kwargs)
    assert not submit('  1  ')['replayed']
    assert submit('  1  ')['replayed']
    assert questions_store.get(store, receipt['question_id']).answer_text == '  1  '
    with pytest.raises(StoreError, match='request_key_conflict'): submit('changed')
    with pytest.raises(StoreError, match='question_answer_stale'): submit('new', 'another-key')


def test_recovered_coordinator_batch_accepts_current_owner_only(planning):
    store, _, coordinator = planning
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    from cortex_runtime.ownership import acquire_task_owner_tx
    with store.connection() as db:
        owner = acquire_task_owner_tx(db, coordinator.task_id, 'replacement', coordinator.owner_epoch)
        db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?', (store._now(), coordinator.channel_id))
        db.commit()
    with pytest.raises(StoreError, match='question_batch_stale'): _submit(store, coordinator, receipt)
    result = question_batches.answer(store, coordinator.task_id, receipt['batch_id'], _answers(receipt), 'recovered',
                                     session_id='incoming', owner_epoch=owner.epoch)
    assert result['complete']
    assert resume_answered(store, coordinator.task_id)


def test_batch_membership_remains_in_recovery_open_questions(bound):
    store, _, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    capsule = mcp_tools.dispatch(store, workers[0], 'read_capsule', {})
    open_questions = capsule['sections']['drafts_and_open_actions']['open_questions']
    assert len(open_questions) == 2
    assert all(q['batch_id'] == receipt['batch_id'] and q['question_ids'] == receipt['question_ids'] for q in open_questions)
    assert capsule['sections']['pipeline_and_assignments']['runtime']['worker_waits'][0]['question_ids'] == receipt['question_ids']


def test_production_facade_preserves_already_recorded_answer_flag():
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
    facade = ProductionOrchestratingRuntime.__new__(ProductionOrchestratingRuntime)
    observed = []
    facade._runtime_for_project = lambda _: SimpleNamespace(answer_question=lambda *a, **k: observed.append(k))
    facade.answer_question('task', 'session', object(), '  1  ', lambda _: True, threading.Event(),
                           project_root='/project', recorded=True)
    assert observed[0]['recorded'] is True


def test_authority_loss_cancels_every_open_batch_member(bound):
    store, _, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    with store.connection() as db:
        db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?', (store._now(), workers[0].channel_id))
        db.commit()
    assert not questions_store.answer(store, receipt['question_ids'][1], 'CSV', 'text')
    assert all(questions_store.get(store, q).status == 'cancelled' for q in receipt['question_ids'])
    assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'cancelled'


@pytest.mark.parametrize('mode', ['decline', 'text-only'])
def test_unanswered_batch_does_not_resume_or_fabricate_source(planning, mode):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    store, _, coordinator = planning
    request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime.transport_lost = lambda *_: False
    shown = []
    elicitor = SimpleNamespace(elicit_batch=lambda *a: None) if mode == 'decline' else None
    assert not runtime._handle_question_batch(coordinator.task_id, 'incoming', shown.append, 'en',
                                             pending(store, coordinator.task_id), elicitor)
    assert len(questions_store.list_open(store, coordinator.task_id)) == 2
    assert not resume_answered(store, coordinator.task_id)
    if mode == 'text-only':
        assert 'small' in shown[0] and 'CSV' in shown[1]


def test_concurrent_complete_submissions_have_exactly_one_winner(planning):
    from concurrent.futures import ThreadPoolExecutor
    store, _, coordinator = planning
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='batch'))
    def submit(index):
        try:
            result = _submit(store, coordinator, receipt, key=f'answer-{index}')
            return result['complete']
        except StoreError as error:
            assert str(error) == 'question_batch_already_answered'
            return False
    with ThreadPoolExecutor(max_workers=2) as pool:
        assert sorted(pool.map(submit, range(2))) == [False, True]


def test_all_23_profiles_require_structured_questions_and_preserve_free_text():
    from pathlib import Path
    root = Path(__file__).resolve().parents[1]
    profiles = json.loads((root / 'profiles.json').read_text())['profiles']
    assert len(profiles) == 23
    for profile in profiles:
        body = (root / 'skills' / ('worker-' + Path(profile['source']).stem) / 'SKILL.md').read_text()
        normalized = ' '.join(body.split())
        if profile['name'] == 'senior_consultant':
            # Owner decision D8: the consultant asks the user nothing; missing
            # evidence is a limitation in its report.
            assert 'Ask the user nothing: no prose-only question, no native engine question tool' in normalized
            assert 'record it in your report as a limitation' in normalized
            assert 'structured question tool' not in normalized
            continue
        assert 'advertised `ask_question` route to the user' in normalized
        assert 'never a prose-only question' in normalized
        assert 'native engine question tool' in normalized
        assert 'meaningful selectable' in normalized
        assert 'free text' in body


@pytest.mark.parametrize('tied_timestamps', [False, True])
def test_displayed_batch_numbers_bind_command_answers_in_creation_order(planning, monkeypatch, tied_timestamps):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
    from cortex_runtime.portable.engine_contract import StopReason
    from acp_fakes import FakeRuntime

    store, _, coordinator = planning
    older = questions_store.create_if_absent(store, coordinator.task_id, 'older', 'Earlier question?')
    # Force random identities to sort opposite to the immutable member order.
    ids = iter(['pq_zfirst', 'pq_asecond', 'qb_order'])
    monkeypatch.setattr(question_batches, 'new_id', lambda _: next(ids))
    receipt = request(store, coordinator, dict(questions=FORMS, request_key='ordered'))
    with store.connection() as db:
        db.execute("UPDATE pending_questions SET created_at=? WHERE id=?", ('2026-01-01T00:00:00', older.id))
        for index, question_id in enumerate(receipt['question_ids']):
            created = f"2026-01-02T00:00:0{0 if tied_timestamps else index}"
            db.execute("UPDATE pending_questions SET created_at=? WHERE id=?", (created, question_id))
        db.commit()

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime.transport_lost = lambda *_: False
    facade = ProductionOrchestratingRuntime.__new__(ProductionOrchestratingRuntime)
    facade._runtime_for_project = lambda _: runtime
    expected = [older.id, *receipt['question_ids']]
    assert [q.id for q in facade.list_open_questions(coordinator.task_id, project_root=str(store.project_root))] == expected
    reopened = Store(store.directory, project_root=store.project_root)
    assert [q.id for q in questions_store.list_open(reopened, coordinator.task_id)] == expected

    shown = []
    assert not runtime._handle_question_batch(coordinator.task_id, 'incoming', shown.append, 'en',
                                             pending(store, coordinator.task_id), None)
    assert shown[0].startswith('\n\n2. Which target?')
    assert shown[1].startswith('\n\n3. Which format?')
    assert all(block.endswith('\n\n') for block in shown)
    assert '\n\n3. Which format?' in ''.join(shown)

    # Exercise the real ACP command's current-open index lookup and durable
    # source capture, without starting a paid engine or an orchestration turn.
    fake = FakeRuntime()
    fake.list_open_questions = facade.list_open_questions
    def apply(task_id, session_id, question, text, emit_update, cancel_event):
        assert questions_store.answer(store, question.id, text, 'text', session_id=session_id)
        return StopReason.END_TURN
    fake.answer_question = apply
    agent = Agent(fake)
    agent.stores[str(store.project_root)] = store
    agent.transport = SimpleNamespace(send_response=lambda _: None, send_notification=lambda _: True)
    session = dict(project=str(store.project_root), task_id=coordinator.task_id)
    agent._run_cortex_answer('first', 'incoming', session, '2 first answer', threading.Event(), lambda _: None)
    assert questions_store.get(store, receipt['question_ids'][0]).answer_text == 'first answer'
    assert questions_store.get(store, receipt['question_ids'][1]).status == 'open'
    shown.clear()
    runtime._handle_question_batch(coordinator.task_id, 'incoming', shown.append, 'en',
                                   pending(store, coordinator.task_id), None)
    assert len(shown) == 1 and shown[0].startswith('\n\n2. Which format?')
    agent._run_cortex_answer('second', 'incoming', session, '2 second answer', threading.Event(), lambda _: None)
    assert questions_store.get(store, receipt['question_ids'][1]).answer_text == 'second answer'
    assert questions_store.get(store, older.id).status == 'open'
