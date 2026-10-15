"""The runtime stop question's own Finish/Continue and honest answer closure.

Finish (by number, stored or rendered en/ru text, or label) force-finishes:
CANCELLED with USER_FORCE_FINISHED. Continue resumes the runtime flow and
never releases the stopped work to a direct answer. No direct answer turns
work whose review or verification did not pass into COMPLETED.
"""
import json
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import messages, questions_store
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome, _question_answer_value
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.questions import (
    is_force_finish_answer, pending, resume_answered, stop_choice,
)
from cortex_runtime.orchestration.state import STOP_OPTIONS, TaskMachine, released_by_user
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_acp_steering_and_questions import _make_runtime, _task

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

STOP_TEXT = 'The task stopped: worker_failed. What should happen next?'
FINISH_FORMS = [
    STOP_OPTIONS[1],
    messages.render('stop_option_finish', 'en'),
    messages.render('stop_option_finish', 'ru'),
    'Finish', 'finish.', 'Завершить', '2',
    # Wording an earlier build stored or rendered for a still-open question.
    'Finish: accept the current results and close the task',
    'Завершить: принять текущие результаты и закрыть задачу',
]
CONTINUE_FORMS = [
    STOP_OPTIONS[0],
    messages.render('stop_option_continue', 'en'),
    messages.render('stop_option_continue', 'ru'),
    'Continue', 'Продолжить', '1',
]


class ScriptedElicitor:
    def __init__(self, answers):
        self.answers = iter(answers)
        self.calls = []

    def elicit(self, session_id, question, options):
        self.calls.append((question, options))
        return next(self.answers, None)


@pytest.fixture
def machine(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='one'), 'Implement feature', 'owner')
    state = TaskMachine(store)
    state.create(task.task_id, 'Implement feature')
    return state, SimpleNamespace(task_id=task.task_id, channel_id='coordinator', role='coordinator')


def _plan():
    return dict(tasks=[dict(id='N0', role='backend-dev', objective='Implement', dependencies=[],
                            scope=['src/**'], model='auto')],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])


def _runtime_stop_question(state, channel, blocker='structured_proposal_missing'):
    """Planned, unverified work stopped with the runtime's own question."""
    state.submit_plan(channel, _plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    assert state.claim_ready(channel.task_id, channel.channel_id)
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='failed' WHERE task_id=?", (channel.task_id,)); db.commit()
    state.move(channel.task_id, 'BLOCKED', blocker=blocker)
    question = pending(state.store, channel.task_id)
    assert question['blocker'] == blocker and state.context(channel.task_id)['state'] == 'WAITING_USER'
    return question


def _events(store, task_id, kind):
    with store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            'SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id', (task_id, kind))]


@pytest.mark.parametrize('answer', FINISH_FORMS)
def test_every_form_of_the_stop_question_finish_is_force_finish(answer):
    assert stop_choice(answer, STOP_TEXT, runtime_stop=True) == 'finish'
    assert is_force_finish_answer(answer, STOP_TEXT, runtime_stop=True)


@pytest.mark.parametrize('answer', CONTINUE_FORMS)
def test_every_form_of_the_stop_question_continue_is_not_force_finish(answer):
    assert stop_choice(answer, STOP_TEXT, runtime_stop=True) == 'continue'
    assert not is_force_finish_answer(answer, STOP_TEXT, runtime_stop=True)


def test_stop_options_match_the_rendered_english_options_and_other_text_is_not_a_choice():
    assert STOP_OPTIONS == (messages.render('stop_option_continue', 'en'), messages.render('stop_option_finish', 'en'))
    assert stop_choice('Fix the failing test and finish', STOP_TEXT, runtime_stop=True) is None
    assert stop_choice('3', STOP_TEXT, runtime_stop=True) is None
    # Another question's options are not the runtime's: no number or label match.
    assert stop_choice('Finish', 'Anything else?', runtime_stop=True) is None
    assert not is_force_finish_answer('2', 'Anything else?')
    assert not is_force_finish_answer('Finish', 'Anything else?')


@pytest.mark.parametrize('answer', ['2', 'Finish', 'Завершить', STOP_OPTIONS[1]])
def test_the_stop_question_text_without_the_runtime_marker_is_not_a_stop_choice(answer):
    """Only the runtime's structured marker makes a question its stop
    question; the same wording from anyone else is an ordinary question."""
    assert stop_choice(answer, STOP_TEXT) is None
    assert not is_force_finish_answer(answer, STOP_TEXT)


def test_the_runtime_stop_question_carries_its_structured_markers(machine):
    from cortex_runtime.orchestration.questions import runtime_stop_dedup_key, runtime_stop_request
    state, channel = machine
    question = _runtime_stop_question(state, channel)
    assert question['runtime_stop'] is True and runtime_stop_request(question)
    with state.store.connection() as db:
        dedup_key = db.execute('SELECT dedup_key FROM pending_questions WHERE id=?',
                               (question['question_id'],)).fetchone()[0]
    assert runtime_stop_dedup_key(dedup_key)


@pytest.mark.parametrize('answer', ['2', 'Finish: close the task as is, without certifying unfinished work'])
def test_a_coordinator_question_worded_like_the_stop_question_never_force_finishes(machine, answer):
    from cortex_runtime.orchestration.questions import request, runtime_stop_dedup_key
    state, channel = machine
    receipt = request(state.store, channel, dict(question=STOP_TEXT, options=list(STOP_OPTIONS), request_key='mimic'))
    row = questions_store.get(state.store, receipt['question_id'])
    with state.store.connection() as db:
        dedup_key = db.execute('SELECT dedup_key FROM pending_questions WHERE id=?', (row.id,)).fetchone()[0]
    assert not runtime_stop_dedup_key(dedup_key)
    assert questions_store.answer(state.store, receipt['question_id'], _question_answer_value(row, answer), 'text')
    # An ordinary answer: the task resumes, it is not force-finished.
    assert resume_answered(state.store, channel.task_id) is True
    assert state.context(channel.task_id)['state'] != 'CANCELLED'
    assert not _events(state.store, channel.task_id, 'USER_FORCE_FINISHED')


@pytest.mark.parametrize('answer, source', [
    ('2', 'text'),  # a typed number resolves to the stored option
    (messages.render('stop_option_finish', 'en'), 'elicitation'),
    (messages.render('stop_option_finish', 'ru'), 'elicitation'),
    ('Завершить', 'text'),
])
def test_finish_force_finishes_the_task_cancelled_without_certifying_it(machine, answer, source):
    state, channel = machine
    question = _runtime_stop_question(state, channel)
    row = questions_store.get(state.store, question['question_id'])
    value = _question_answer_value(row, answer) if source == 'text' else answer
    assert questions_store.answer(state.store, question['question_id'], value, source)
    assert resume_answered(state.store, channel.task_id) == ('force_finish', question['question_id'])
    runtime, _engine = _make_runtime(state.store, lambda *_args: None)
    try:
        assert runtime.stop_task(channel.task_id, 's1', lambda _text: True,
                                 stop_source='user_answer_force_finish', question_id=question['question_id'])
    finally:
        runtime.shutdown()
    assert state.context(channel.task_id)['state'] == 'CANCELLED'
    [force] = _events(state.store, channel.task_id, 'USER_FORCE_FINISHED')
    assert force['question_id'] == question['question_id']
    assert force['review_verification_synthesis'] == 'not_certified_by_force_finish'
    assert not _events(state.store, channel.task_id, 'VERIFICATION_PASSED')


@pytest.mark.parametrize('answer', [STOP_OPTIONS[0], messages.render('stop_option_continue', 'ru'), 'Continue'])
def test_continue_resumes_the_runtime_flow_and_never_releases_a_direct_answer(machine, answer):
    state, channel = machine
    question = _runtime_stop_question(state, channel)
    assert questions_store.answer(state.store, question['question_id'], answer, 'elicitation')
    assert resume_answered(state.store, channel.task_id) is True
    assert state.context(channel.task_id)['state'] == 'RECOVERING'
    state.move(channel.task_id, 'PLANNING')
    with state.store.connection() as db:
        assert not released_by_user(db, db.execute('SELECT * FROM runtime_tasks WHERE task_id=?',
                                                   (channel.task_id,)).fetchone())
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Done.', 'continue-answer')
    # The coordinator still continues through a plan.
    assert state.submit_plan(channel, _plan(), 'next')['state'] == 'PLAN_READY'


def test_own_words_release_closes_unverified_work_failed_never_completed(machine):
    state, channel = machine
    question = _runtime_stop_question(state, channel)
    assert questions_store.answer(state.store, question['question_id'],
                                  'Stop here and summarize what failed', 'text')
    assert resume_answered(state.store, channel.task_id) is True
    state.move(channel.task_id, 'PLANNING')
    text = 'Report r_x: the change is incomplete; verification did not pass.'
    assert state.submit_answer(channel, text, 'final')['state'] == 'ANSWER_READY'
    with pytest.raises(StoreError, match='completion_nodes_incomplete'):
        state.move(channel.task_id, 'COMPLETED')
    assert state.close_answer(channel.task_id) == 'FAILED'
    context = state.context(channel.task_id)
    assert (context['state'], context['blocker'], context['synthesis']) == (
        'FAILED', 'answer_not_certified:completion_nodes_incomplete', text)
    assert _events(state.store, channel.task_id, 'ANSWER_NOT_CERTIFIED') == [
        dict(revision=context['revision'], reason='completion_nodes_incomplete')]
    assert not _events(state.store, channel.task_id, 'VERIFICATION_PASSED')


def test_released_answer_after_a_failed_verification_is_not_completed(machine):
    state, channel = machine
    state.submit_plan(channel, _plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    digest = 'd' * 64
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed', stage='review', result_json=? WHERE task_id=?",
                   (json.dumps(dict(project_digest=digest, verdict='APPROVE')), channel.task_id))
        db.commit()
    state.move(channel.task_id, 'REVIEWING'); state.move(channel.task_id, 'VERIFYING')
    bad = dict(argv=['python3', '-m', 'pytest'], exit_code=1, complete=True, revision_digest=digest)
    assert state.record_verification(channel.task_id, [bad], digest) is False
    # The coordinator was asked to ask, did not; the runtime asks next.
    state.move(channel.task_id, 'BLOCKED', blocker='verification_failed')
    state.move(channel.task_id, 'PLANNING')
    state.move(channel.task_id, 'BLOCKED', blocker='verification_failed')
    question = pending(state.store, channel.task_id)
    assert question['blocker'] == 'verification_failed'
    assert questions_store.answer(state.store, question['question_id'], 'Close it and tell me what failed', 'text')
    assert resume_answered(state.store, channel.task_id) is True
    state.move(channel.task_id, 'PLANNING')
    assert state.submit_answer(channel, 'Verification failed: pytest exit 1.', 'final')['state'] == 'ANSWER_READY'
    assert state.close_answer(channel.task_id) == 'FAILED'
    assert state.context(channel.task_id)['blocker'].startswith('answer_not_certified:completion_')
    assert state.get_state(channel.task_id) != 'COMPLETED'


def test_an_informational_answer_still_completes(machine):
    state, channel = machine
    assert state.submit_answer(channel, 'The project is a static site.', 'info')['state'] == 'ANSWER_READY'
    assert state.close_answer(channel.task_id) == 'COMPLETED'
    assert state.context(channel.task_id)['state'] == 'COMPLETED'


def _no_proposal_router(store, channel, *_):
    def behavior(*_):
        return PromptOutcome(StopReason.END_TURN, 'No proposal')
    return behavior


@pytest.mark.parametrize('answer', [messages.render('stop_option_finish', 'en'),
                                    messages.render('stop_option_finish', 'ru')])
def test_card_finish_on_the_runtime_stop_question_cancels_in_the_same_turn(tmp_path, answer):
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, _no_proposal_router)
    elicitor = ScriptedElicitor([answer])
    try:
        assert runtime.start_prompt(task_id, 's1', 'Inspect', lambda _: True, threading.Event(),
                                    host_elicitor=elicitor) == StopReason.CANCELLED
    finally:
        runtime.shutdown()
    assert len(elicitor.calls) == 1
    assert TaskMachine(store).context(task_id)['state'] == 'CANCELLED'
    assert len(_events(store, task_id, 'USER_FORCE_FINISHED')) == 1


@pytest.mark.parametrize('reply', ['2', 'Завершить'])
def test_typed_finish_on_the_runtime_stop_question_cancels_the_task(tmp_path, reply):
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, _no_proposal_router)
    try:
        runtime.start_prompt(task_id, 's1', 'Inspect', lambda _: True, threading.Event())
        context = TaskMachine(store).context(task_id)
        assert (context['state'], context['blocker']) == ('WAITING_USER', 'structured_proposal_missing')
        assert runtime.start_prompt(task_id, 's1', reply, lambda _: True, threading.Event()) == StopReason.CANCELLED
    finally:
        runtime.shutdown()
    assert TaskMachine(store).context(task_id)['state'] == 'CANCELLED'
    assert len(_events(store, task_id, 'USER_FORCE_FINISHED')) == 1


def test_controller_shows_an_uncertified_final_answer_and_ends_failed(tmp_path):
    """A worker's failed check, then a coordinator that never proposes: the
    runtime asks; an answer in the user's own words releases the final
    answer, which reaches the chat while the task ends FAILED, not COMPLETED."""
    from runtime_scenarios import publish_fixture_report
    store, task_id = _task(tmp_path)
    final = 'The change failed its unit check; nothing was verified.'
    refused = []

    def router(store, channel, *_):
        def behavior(store, channel, ref, message, index):
            context = TaskMachine(store).context(task_id)
            if channel.role != 'coordinator':
                failed = mcp_tools.dispatch(store, channel, 'command_run', dict(
                    argv=['python3', '-B', '-c', 'import sys; sys.exit(1)'], request_key='unit'))
                mcp_tools.dispatch(store, channel, 'result_submit', dict(
                    report=publish_fixture_report(store, channel, 'Unit check failed.'), summary='Unit check failed.',
                    checks=[dict(name='unit', outcome='failed', tool_call_ref=failed['tool_call_ref'])],
                    assignment_outcome='failed', request_key='result'))
            elif context['state'] == 'PLANNING' and context['revision'] == 0:
                mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                    tasks=[dict(id='implement', role='backend-dev', objective='Implement', dependencies=[],
                                scope=['src/**'], model='auto')],
                    verification=[['python3', '-B', '-c', 'print(1)']]), request_key='plan'))
            elif context['state'] == 'PLANNING':
                try:
                    mcp_tools.dispatch(store, channel, 'answer_submit', dict(text=final, request_key='final'))
                except StoreError as error:
                    refused.append(str(error))  # not released yet: no proposal this turn
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior

    runtime, _engine = _make_runtime(store, router)
    elicitor = ScriptedElicitor(['Stop and summarize what you found'])
    output = []
    try:
        assert runtime.start_prompt(task_id, 's1', 'Implement', output.append, threading.Event(),
                                    host_elicitor=elicitor) == StopReason.END_TURN
    finally:
        runtime.shutdown()
    context = TaskMachine(store).context(task_id)
    assert (context['state'], context['blocker']) == ('FAILED', 'answer_not_certified:completion_nodes_incomplete')
    assert refused and all('direct_answer_not_available' in error for error in refused)
    assert len(elicitor.calls) == 1
    assert any(final in text for text in output)
    assert any('Runtime: FAILED' in text for text in output)
    assert not _events(store, task_id, 'USER_FORCE_FINISHED')
    assert len(_events(store, task_id, 'ANSWER_NOT_CERTIFIED')) == 1


def test_finish_after_verified_work_completes_with_the_coordinators_answer(tmp_path):
    """The legitimate path stays: a closing question after passed review and
    verification, then the coordinator's final answer, completes the task."""
    from test_runtime_composed_synthesis import ARGV, submit
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='after-verification', event_id='one'), 'Implement', 'owner')
    asked = []

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            context = TaskMachine(store).context(task.task_id)
            if channel.role == 'coordinator':
                if context['state'] == 'PLANNING' and context['revision'] == 0:
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                        tasks=[dict(id='implement', role='backend-dev', objective='Implement', dependencies=[],
                                    scope=['.'], model='auto')], verification=[ARGV]), request_key='plan'))
                elif context['state'] == 'RECONCILING' and not asked:
                    asked.append(True)
                    mcp_tools.dispatch(store, channel, 'user_question_request', dict(
                        question='Anything else before I close?', options=['Finish', 'Continue'],
                        request_key='closing'))
                elif context['state'] == 'PLANNING':
                    # An owner saying Finish to an ordinary closing question does
                    # not certify the worker's unrun live check: the coordinator's
                    # closing answer must disclose that retained gap by its exact
                    # name and blocker, or the runtime refuses it.
                    mcp_tools.dispatch(store, channel, 'answer_submit', dict(
                        text='Implemented and verified.',
                        not_verified=[dict(item='live check', blocker='Missing isolated host')],
                        request_key='final'))
            else:
                with store.connection() as db:
                    role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                      (channel.attempt_id,)).fetchone()[0]
                if role != 'worker-code-reviewer':
                    (tmp_path / 'feature.py').write_text('new\n')
                    submit(store, channel, checks=[dict(name='live check', outcome='not_run',
                                                        blocker='Missing isolated host')])
                else:
                    submit(store, channel, verdict='APPROVE')
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='cn_after_verification', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    output = []
    try:
        assert runtime.start_prompt(task.task_id, 'incoming', 'Implement', output.append, threading.Event(),
                                    emit_tool_call=lambda update: True,
                                    host_elicitor=ScriptedElicitor(['Finish'])) == StopReason.END_TURN
    finally:
        runtime.shutdown()
    context = TaskMachine(store).context(task.task_id)
    assert asked and context['state'] == 'COMPLETED', (context['state'], context['blocker'])
    assert context['synthesis'] == ('Implemented and verified.\n\n## Not verified\n\n'
                                    '- live check — blocker: Missing isolated host')
    assert not _events(store, task.task_id, 'USER_FORCE_FINISHED')
    assert not _events(store, task.task_id, 'ANSWER_NOT_CERTIFIED')


# Codes raised by orchestration.recovery (an interrupted result delivery
# that could not be reconciled against the worker's actual publication, or a
# lost-worker recovery that could not finish cleanly) and by the
# controller's own coordinator/verification crash paths. Before messages.py
# carried a stop_reason.<code> entry for each of these, the catalog lookup
# in _stop_question raised KeyError and fell back to showing the bare code
# as the entire reason -- exactly the raw-code leak this is fixed to avoid.
_PREVIOUSLY_UNCOVERED_STOP_CODES = [
    'result_publication_mismatch',
    'result_provision_missing',
    'ambiguous_result_publication',
    'lost_worker_result_changed',
    'lost_worker_termination_unconfirmed',
    'verification_execution_error:RuntimeError',
    'coordinator_prompt_failed:RuntimeError',
    'workers_still_live_or_unknown',
]


@pytest.mark.parametrize('blocker', _PREVIOUSLY_UNCOVERED_STOP_CODES)
@pytest.mark.parametrize('language', ['en', 'ru'])
def test_a_previously_uncovered_stop_code_renders_plain_language_not_a_bare_code(blocker, language):
    from cortex_runtime.orchestration.controller import _stop_question
    question = _stop_question(dict(blocker=blocker, question_id='q'), dict(nodes=[]), language)['question']
    code = blocker.split(':', 1)[0]
    # The code stays visible in brackets for diagnosis, next to the plain
    # reason -- never as the whole message a KeyError fallback would show.
    assert f'[{blocker}]' in question
    assert f'stopped: {code}.' not in question
    assert f'остановилась: {code}.' not in question


def test_finish_with_an_undisclosed_gap_in_the_closing_answer_cannot_complete(tmp_path):
    """The identical legitimate path, but the coordinator's closing answer
    omits the worker's retained not_run check: the runtime refuses the answer
    with synthesis_gaps_undisclosed and the task never reaches COMPLETED. An
    owner saying Finish to an ordinary closing question does not certify an
    unrun check."""
    from test_runtime_composed_synthesis import ARGV, submit
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='after-verification', event_id='two'), 'Implement', 'owner')
    asked = []

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            context = TaskMachine(store).context(task.task_id)
            if channel.role == 'coordinator':
                if context['state'] == 'PLANNING' and context['revision'] == 0:
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                        tasks=[dict(id='implement', role='backend-dev', objective='Implement', dependencies=[],
                                    scope=['.'], model='auto')], verification=[ARGV]), request_key='plan'))
                elif context['state'] == 'RECONCILING' and not asked:
                    asked.append(True)
                    mcp_tools.dispatch(store, channel, 'user_question_request', dict(
                        question='Anything else before I close?', options=['Finish', 'Continue'],
                        request_key='closing'))
                elif context['state'] == 'PLANNING':
                    # Deliberately omits the worker's retained 'live check' gap.
                    mcp_tools.dispatch(store, channel, 'answer_submit', dict(text='Implemented and verified.',
                                                                          request_key='final'))
            else:
                with store.connection() as db:
                    role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                      (channel.attempt_id,)).fetchone()[0]
                if role != 'worker-code-reviewer':
                    (tmp_path / 'feature.py').write_text('new\n')
                    submit(store, channel, checks=[dict(name='live check', outcome='not_run',
                                                        blocker='Missing isolated host')])
                else:
                    submit(store, channel, verdict='APPROVE')
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='cn_after_verification_gap', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    output = []
    try:
        runtime.start_prompt(task.task_id, 'incoming', 'Implement', output.append, threading.Event(),
                             emit_tool_call=lambda update: True,
                             host_elicitor=ScriptedElicitor(['Finish']))
    finally:
        runtime.shutdown()
    context = TaskMachine(store).context(task.task_id)
    assert asked and context['state'] == 'WAITING_USER'
    assert context['blocker'] == 'synthesis_gaps_undisclosed'
    assert context.get('synthesis') != 'Implemented and verified.'
    assert not _events(store, task.task_id, 'USER_FORCE_FINISHED')
    assert not _events(store, task.task_id, 'ANSWER_NOT_CERTIFIED')
