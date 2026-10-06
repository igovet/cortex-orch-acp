"""No-paid assignment-local wait, same-session continuation and failure fences."""
import threading

import pytest

from cortex_runtime.acp import questions_store, replay_store
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome, EngineTurnFailed
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.orchestration import worker_waits
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason, EngineEvent, EngineEventKind
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _plan(count=1):
    return dict(tasks=[dict(id=f'N{i}', role='backend-dev', objective=f'Implement part {i}',
                            dependencies=[], scope=[f'part{i}.txt'], model='auto')
                       for i in range(count)], verification=[['python3', '-B', '-c', 'print("verified")']])


@pytest.fixture
def bound(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='wait-test', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    machine.submit_plan(coordinator, _plan(2), 'plan')
    machine.move(boot.task_id, 'EXECUTING')
    workers = []
    for index, node in enumerate(machine.claim_ready(boot.task_id, coordinator.channel_id)):
        attempt = f'xa_test{index}'
        with store.connection() as db:
            channel_id, secret = issue_channel_tx(
                db, boot.task_id, 'worker', coordinator.owner_epoch,
                parent_channel_id=coordinator.channel_id, assignment_id=f'a_test{index}', attempt_id=attempt)
            stamp = store._now()
            db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                       (node['assignment_ref'], boot.task_id, f'a_test{index}', attempt,
                        resolve_role('backend-dev'), 'ready', 'ready', 'shared_project_root', channel_id,
                        None, None, f'es_test{index}', 'qualified', None, coordinator.owner_epoch, stamp, stamp))
            db.commit()
        workers.append(resolve_channel(store, channel_id, secret))
    return store, machine, coordinator, workers


def _ask(store, channel, key='question'):
    return mcp_tools.dispatch(store, channel, 'ask_question',
                              dict(question='Which target?', options=['small', 'large'], request_key=key))


def test_registration_replay_and_mutation_fence_are_assignment_local(bound):
    store, machine, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    assert receipt == _ask(store, workers[0])
    assert receipt['complete'] and receipt['status'] == 'waiting_for_answer'
    with pytest.raises(StoreError, match='request_key_conflict'):
        mcp_tools.dispatch(store, workers[0], 'ask_question', dict(question='Changed?', options=['small', 'large'], request_key='question'))
    assert machine.context(coordinator.task_id)['state'] == 'EXECUTING'
    assert [n['status'] for n in machine.context(coordinator.task_id)['nodes']] == ['running', 'running']
    with store.connection() as db:
        assert not db.execute('SELECT 1 FROM trusted_channels WHERE revoked_at IS NOT NULL').fetchone()
        assert len(worker_waits.projection(db, coordinator.task_id)) == 1
    for operation, args in [
            ('project_write', dict(path='part0.txt', content='bad', expected_sha256='absent', request_key='write')),
            ('command_run', dict(argv=['true'], request_key='command')),
            ('result_submit', dict(report=dict(title='Incomplete', markdown='Incomplete'), summary='Incomplete',
                                   checks=[], request_key='result'))]:
        with pytest.raises(StoreError, match='worker_waiting_for_answer'):
            mcp_tools.dispatch(store, workers[0], operation, args)
    assert dispatch(store, workers[0], 'project_read', dict(path='.'))
    assert dispatch(store, workers[1], 'project_write',
                    dict(path='part1.txt', content='sibling', expected_sha256='absent', request_key='sibling'))


def test_answer_and_single_continuation_claim_commit_atomically(bound):
    store, machine, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    assert questions_store.answer(store, receipt['question_id'], 'small', 'text', session_id='incoming')
    assert not questions_store.answer(store, receipt['question_id'], 'replay', 'text')
    with pytest.raises(StoreError, match='worker_waiting_for_answer'):
        dispatch(store, workers[0], 'project_write',
                 dict(path='part0.txt', content='too early', expected_sha256='absent', request_key='early'))
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait['status'] == 'ready' and wait['answer_session_id'] == 'incoming'
    claimed = worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0')
    assert claimed[1] == 'small'
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0') is None
    dispatch(store, workers[0], 'project_write',
             dict(path='part0.txt', content='resumed', expected_sha256='absent', request_key='resumed'))
    worker_waits.finish(store, coordinator.task_id, wait['wait_id'], claimed[0]['delivery_id'],
                        stop_reason='end_turn')
    assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'resumed'


def test_ordinary_answer_ledger_uses_same_wait_transaction(bound):
    store, _, coordinator, workers = bound
    question = _ask(store, workers[0])['question_id']
    _, answered = replay_store.record_user_message_answer_event(
        store, coordinator.task_id, 'real-user:1', 'incoming', dict(content='small'),
        question_id=question, answer_text='small')
    assert answered
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait['status'] == 'ready'
    assert wait['answer_session_id'] == 'incoming'


@pytest.mark.parametrize('reason', ['stop', 'cancel', 'replacement'])
def test_late_answers_cannot_revive_stopped_or_replaced_workers(bound, reason):
    store, machine, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    if reason == 'stop':
        from cortex_runtime.acp.steering import mark_stopped
        mark_stopped(store, coordinator.task_id, source='cortex_stop_command', session_id='incoming')
    elif reason == 'cancel':
        machine.cancel(coordinator.task_id)
    else:
        with store.connection() as db:
            db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?',
                       (store._now(), workers[0].channel_id))
            db.commit()
    assert not questions_store.answer(store, receipt['question_id'], 'late', 'text')
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait['status'] == 'cancelled'
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0') is None


def test_wrong_session_or_ambiguous_started_delivery_is_never_replayed(bound):
    store, _, coordinator, workers = bound
    receipt = _ask(store, workers[0])
    questions_store.answer(store, receipt['question_id'], 'answer', 'text')
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'replacement-session') is None
    assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'orphaned'


def _runtime_flow(tmp_path, *, questions=1, elicitor=True, early=False, sibling=False, stopped=False, sibling_fails=False, batch=False, language='en'):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='flow-test', event_id='one'), 'Implement', 'owner')
    from cortex_runtime.acp.steering import set_language_override
    set_language_override(store, boot.task_id, language)
    question = 'Какую цель выбрать?' if language == 'ru' else 'Which target?'
    options = ['Малую', 'Большую'] if language == 'ru' else ['small', 'large']
    prompts, identities, cards, output = [], [], [], []
    card_entered, sibling_done = threading.Event(), threading.Event()
    def router(store, channel, ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                context = TaskMachine(store).context(boot.task_id)
                if context['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit',
                                       dict(plan=_plan(2 if sibling else 1), request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            with store.connection() as db:
                node = db.execute('SELECT n.node_id,n.stage FROM runtime_nodes n JOIN delegation_provisioning p '
                                  'ON p.idempotency_key=n.assignment_ref WHERE p.attempt_id=?',
                                  (channel.attempt_id,)).fetchone()
            if node['node_id'] == 'N1' and sibling_fails:
                raise EngineTurnFailed('refusal')
            if node['node_id'] == 'N0':
                assert 'user_question_language: ' + language in message
                assert 'Worker response language: English only' in message
                assert 'all progress messages, summaries, reports and final responses' in message
                assert 'every selectable option use' in message
                prompts.append(message)
                identities.append((channel.attempt_id, channel.channel_id, ref))
                if call_index == 0:
                    mcp_tools.dispatch(store, channel, 'project_write',
                                       dict(path='part0.txt', content='before', expected_sha256='absent', request_key='before'))
                if call_index < questions:
                    args = dict(request_key=f'question{call_index}')
                    if batch:
                        args['questions'] = [dict(question=question, options=options),
                                             dict(question='Какой формат?' if language == 'ru' else 'Which format?',
                                                  options=['JSON', 'CSV'])]
                    else:
                        args.update(question=question, options=options)
                    mcp_tools.dispatch(store, channel, 'ask_question', args)
                    if early:
                        assert card_entered.wait(10)
                        assert len(prompts) == 1  # answer cannot prompt into the busy session
                    return PromptOutcome(StopReason.END_TURN, '')
                assert 'Runtime answer continuation' in message and options[0] in message
                if batch:
                    assert 'custom format' in message and 'batch_id' in message and 'answer_ref' in message
                read = mcp_tools.dispatch(store, channel, 'project_read', dict(path='part0.txt'))
                mcp_tools.dispatch(store, channel, 'project_write',
                                   dict(path='part0.txt', content='after', expected_sha256=read['sha256'], request_key='after'))
            args = dict(report=dict(title='Observed', markdown='# Observed\n\nFinished.'), summary='Finished.',
                        checks=[], request_key='result')
            if node['stage'] == 'review':
                args['verdict'] = 'APPROVE'
            mcp_tools.dispatch(store, channel, 'result_submit', args)
            if node['node_id'] == 'N1':
                sibling_done.set()
            return PromptOutcome(StopReason.END_TURN, 'Finished.')
        return behavior
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    class Elicitor:
        def elicit_batch(self, session_id, saved):
            card_entered.set()
            first, second = saved['questions']
            assert first['question'] == question and first['options'] == options
            assert questions_store.answer(store, first['question_id'], options[0], 'elicitation', session_id=session_id)
            wait = worker_waits.list_waits(store, boot.task_id)[0]
            assert wait['status'] == 'open' and len(prompts) == 1
            if sibling:
                assert sibling_done.wait(10), 'sibling was blocked by the partial batch'
            assert questions_store.answer(store, second['question_id'], 'custom format', 'elicitation', session_id=session_id)
        def elicit(self, session_id, shown_question, shown_options):
            assert shown_question == question and shown_options == options
            card_entered.set()
            if sibling:
                assert sibling_done.wait(10), 'scheduler was blocked by the question card'
            if elicitor == 'declined':
                return None
            if elicitor == 'timeout':
                from cortex_runtime.acp.host_requests import HostRequestTimeout
                raise HostRequestTimeout('timed out')
            return options[0]
    try:
        reason = runtime.start_prompt(boot.task_id, 'incoming', 'Implement', output.append, threading.Event(),
                                      emit_tool_call=cards.append, host_elicitor=Elicitor() if elicitor else None)
        if not elicitor or elicitor in {'declined', 'timeout'}:
            assert TaskMachine(store).context(boot.task_id)['state'] == 'EXECUTING'
            assert worker_waits.list_waits(store, boot.task_id)[0]['status'] == 'open'
            if stopped:
                if sibling_fails:
                    with store.connection() as db:
                        assert db.execute("SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
                                          (boot.task_id,)).fetchone()
                with runtime._lock:
                    runners = list(runtime._running.get(boot.task_id, ()))
                runtime.stop_task(boot.task_id, 'incoming', output.append, emit_tool_call=cards.append)
                for runner in runners:
                    runner.result(timeout=10)
                assert worker_waits.list_waits(store, boot.task_id)[0]['status'] == 'cancelled'
                assert len(prompts) == 1 and (tmp_path / 'part0.txt').read_text() == 'before'
                return
            assert question in ''.join(output) and all(option in ''.join(output) for option in options)
            reason = runtime.start_prompt(boot.task_id, 'incoming', options[0], output.append, threading.Event(),
                                          emit_tool_call=cards.append)
        assert reason == StopReason.END_TURN
        assert TaskMachine(store).context(boot.task_id)['state'] == 'COMPLETED'
        assert (tmp_path / 'part0.txt').read_text() == 'after'
        assert len(set(identities)) == 1
        assert len(prompts) == questions + 1
        assert len([s for s in engine.sessions.values() if s['channel'].role == 'worker']) == (3 if sibling else 2)
        waits = worker_waits.list_waits(store, boot.task_id)
        assert len(waits) == questions and all(w['status'] == 'resumed' for w in waits)
        with store.connection() as db:
            assert not db.execute("SELECT 1 FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'").fetchone()
            assert not db.execute("SELECT 1 FROM task_events WHERE event_type='assignment_failed'").fetchone()
            assert len(db.execute("SELECT 1 FROM runtime_events WHERE type='WORKER_CONTINUATION_STARTED'").fetchall()) == questions
        assert not any('Runtime completion follow-up' in p for p in prompts)
        assert any('Waiting for' in str(card) for card in cards)
    finally:
        runtime.shutdown()


@pytest.mark.parametrize('mode', ['card', 'text', 'early', 'sibling', 'repeated', 'declined', 'timeout', 'stop', 'sibling_failure'])
def test_same_assignment_session_and_context_continue_after_real_answer(tmp_path, mode):
    _runtime_flow(tmp_path, elicitor=(mode if mode in {'declined', 'timeout'} else mode not in {'text', 'stop', 'sibling_failure'}),
                  early=mode == 'early', sibling=mode in {'sibling', 'sibling_failure'},
                  questions=5 if mode == 'repeated' else 1, stopped=mode in {'stop', 'sibling_failure'}, sibling_fails=mode == 'sibling_failure')


@pytest.mark.parametrize('poisoned', [False, True])
def test_uncooperative_question_turn_is_parked_only_after_observed_cancel(bound, poisoned):
    store, _, coordinator, workers = bound
    _ask(store, workers[0])
    class Manager:
        def __init__(self):
            self.cancelled = threading.Event()
            self.prompts = []
        def prompt_turn(self, ref, message, *, prompt_blocks=None):
            self.prompts.append(message)
            assert self.cancelled.wait(10)
            if poisoned:
                from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
                raise PortableError(PortableErrorCategory.ENGINE_FAILURE, 'engine_session_unusable', 'poisoned', False)
            yield EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.CANCELLED)
        def cancel(self, ref):
            self.cancelled.set()
    manager = Manager()
    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._store = store
    adapter._session_to_attempt = {'es_test0': workers[0].attempt_id}
    adapter._attempt_task = {workers[0].attempt_id: coordinator.task_id}
    adapter._managers = {workers[0].attempt_id: manager}
    adapter._session_meta = {}
    adapter._WORKER_WAIT_GRACE_SECONDS = .01
    adapter._STALL_NUDGE_SECONDS = .01
    if poisoned:
        with pytest.raises(EngineTurnFailed, match='poisoned'):
            adapter.prompt('es_test0', 'question turn', threading.Event())
    else:
        assert adapter.prompt('es_test0', 'question turn', threading.Event()).stop_reason == StopReason.END_TURN
    assert manager.prompts == ['question turn']  # no generic stall/completion prompt


@pytest.mark.parametrize('stage', ['ready', 'resuming'])
def test_stop_wins_after_answer_and_after_continuation_claim(bound, stage):
    store, machine, coordinator, workers = bound
    question = _ask(store, workers[0])['question_id']
    questions_store.answer(store, question, 'real answer', 'text', session_id='incoming')
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    claimed = worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0') if stage == 'resuming' else None
    machine.cancel(coordinator.task_id)
    if claimed:
        worker_waits.finish(store, coordinator.task_id, wait['wait_id'], claimed[0]['delivery_id'], stop_reason='end_turn')
    assert questions_store.get(store, question).answer_text == 'real answer'
    assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'cancelled'
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0') is None


def test_an_unconfirmed_parking_cancel_cannot_be_reported_as_a_turn_end(bound):
    store, _, coordinator, workers = bound
    _ask(store, workers[0])
    class Manager:
        def __init__(self):
            self.closed = threading.Event()
        def prompt_turn(self, ref, message, *, prompt_blocks=None):
            assert self.closed.wait(10)
            return
            yield
        def cancel(self, ref):
            pass  # transport never acknowledges this cancel
        def shutdown(self):
            self.closed.set()
    manager = Manager()
    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._store = store
    adapter._session_to_attempt = {'es_test0': workers[0].attempt_id}
    adapter._attempt_task = {workers[0].attempt_id: coordinator.task_id}
    adapter._managers = {workers[0].attempt_id: manager}
    adapter._session_meta = {}
    adapter._WORKER_WAIT_GRACE_SECONDS = .01
    adapter._WORKER_WAIT_CANCEL_SECONDS = .01
    with pytest.raises(EngineTurnFailed, match='worker_wait_cancel_unconfirmed'):
        adapter.prompt('es_test0', 'question turn', threading.Event())


def test_two_visible_questions_are_fifo_without_overwriting_the_waiter(bound):
    store, _, coordinator, workers = bound
    first = _ask(store, workers[0])
    second = _ask(store, workers[1])
    entered, release = threading.Event(), threading.Event()
    shown = []
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, lambda *args: None),
                                   FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    class Elicitor:
        def set_question_id(self, question_id):
            self.question_id = question_id
        def elicit(self, session_id, text, options):
            shown.append(self.question_id)
            if len(shown) == 1:
                entered.set()
                assert release.wait(10)
            return 'small'
    try:
        assert runtime._service_worker_questions(coordinator.task_id, 'incoming', lambda text: True, 'en', Elicitor())
        assert entered.wait(10)
        assert shown == [first['question_id']]
        release.set()
        for future in runtime._worker_question_deliveries.values():
            future.result(timeout=10)
        assert shown == [first['question_id'], second['question_id']]
        assert all(w['status'] == 'ready' for w in worker_waits.list_waits(store, coordinator.task_id))
    finally:
        release.set()
        runtime.shutdown()


@pytest.mark.parametrize('elicitor_kind', ['paseo', 'form'])
def test_shutdown_cancels_owned_question_delivery_not_the_durable_question(bound, elicitor_kind):
    from cortex_runtime.acp.agent import _PaseoQuestionElicitor, _RealHostElicitor
    from cortex_runtime.acp.host_requests import HostRequestClosed
    store, _, coordinator, workers = bound
    _ask(store, workers[0])
    entered = threading.Event()
    class Requester:
        def request(self, method, params, timeout, cancelled=None, id_suffix=None):
            entered.set()
            # This is the existing HostRequester cancellation contract.
            while not cancelled():
                threading.Event().wait(.01)
            raise HostRequestClosed()
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, lambda *args: None),
                                   FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    elicitor = (_PaseoQuestionElicitor(Requester(), session_id='incoming') if elicitor_kind == 'paseo'
                else _RealHostElicitor(Requester()))
    try:
        runtime._service_worker_questions(coordinator.task_id, 'incoming', lambda text: True, 'en', elicitor)
        assert entered.wait(10)
        runtime.shutdown()
        for future in runtime._worker_question_deliveries.values():
            future.result(timeout=10)
        assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'open'
        assert questions_store.list_open(store, coordinator.task_id)
    finally:
        runtime.shutdown()


def test_late_instructional_free_text_card_click_is_not_an_answer(bound):
    from cortex_runtime.acp.agent import Agent
    store, _, coordinator, workers = bound
    legacy = questions_store.create_if_absent(store, coordinator.task_id, 'legacy-free-text', 'Which?')
    agent = Agent.__new__(Agent)
    agent.sessions = {'incoming': dict(task_id=coordinator.task_id, project=str(store.project_root))}
    agent._store_for = lambda project: store
    agent._run_late_answer = lambda *args: pytest.fail('instructional action was delivered as an answer')
    agent._on_late_host_response(
        'hr_1_q.' + legacy.id,
        dict(result=dict(outcome=dict(outcome='selected', optionId='cortex-question-1'))))
    assert questions_store.get(store, legacy.id).status == 'open'


def test_active_prompt_requires_question_selection_and_routes_exact_recipient(bound, monkeypatch):
    from contextlib import nullcontext
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp import agent as agent_module
    from cortex_runtime.acp.protocol import RpcError
    store, _, coordinator, workers = bound
    _ask(store, workers[0])
    _ask(store, workers[1])
    questions = questions_store.list_open(store, coordinator.task_id)
    delivered, responses = [], []
    agent = Agent.__new__(Agent)
    session = dict(lock=threading.Lock(), active=threading.Event(), project=str(store.project_root),
                   task_id=coordinator.task_id,
                   question_waiter=(questions[0].id, lambda text: delivered.append(text) or True))
    agent.sessions = {'incoming': session}
    agent._store_for = lambda project: store
    agent._history_lock = lambda session_id: nullcontext()
    # History persistence is separately exercised by the answer-ledger tests.
    monkeypatch.setattr(agent_module.acp_replay_store, 'append_host_transcript', lambda *args, **kwargs: None)
    class Transport:
        def send_response(self, body):
            responses.append(body)
    agent.transport = Transport()
    with pytest.raises(RpcError, match='Multiple questions'):
        agent._session_prompt(1, dict(sessionId='incoming', prompt=[dict(type='text', text='Ambiguous')]))
    assert delivered == [] and all(q.status == 'open' for q in questions_store.list_open(store, coordinator.task_id))
    completed = threading.Event()
    original = agent._run_late_answer
    def route(*args):
        try:
            original(*args)
        finally:
            completed.set()
    agent._run_late_answer = route
    agent._session_prompt(2, dict(sessionId='incoming', prompt=[dict(type='text', text='/cortex-answer 2 chosen')]))
    assert completed.wait(10)
    assert questions_store.get(store, questions[1].id).answer_text == 'chosen'
    assert questions_store.get(store, questions[0].id).status == 'open'
    assert delivered == [] and responses


def test_process_recovery_orphans_started_delivery_and_retains_saved_answer(bound):
    from cortex_runtime.recovery.worker_reconciliation import _write_recovery_failure_if_still_lost
    store, _, coordinator, workers = bound
    question = _ask(store, workers[0])['question_id']
    questions_store.answer(store, question, 'saved', 'text', session_id='incoming')
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0')
    _write_recovery_failure_if_still_lost(
        store, coordinator.task_id, wait['assignment_ref'], workers[0].attempt_id,
        workers[0].attempt_id, 'absent')
    assert worker_waits.list_waits(store, coordinator.task_id)[0]['status'] == 'orphaned'
    assert questions_store.get(store, question).answer_text == 'saved'
    assert not questions_store.list_open(store, coordinator.task_id)
    assert worker_waits.claim(store, coordinator.task_id, wait['assignment_ref'], 'es_test0') is None


def test_explicit_stop_is_not_consumed_as_the_visible_question_answer(bound):
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp import commands
    store, _, coordinator, workers = bound
    question = _ask(store, workers[0])['question_id']
    consumed, launched = [], []
    agent = Agent.__new__(Agent)
    session = dict(lock=threading.Lock(), active=threading.Event(), project=str(store.project_root),
                   task_id=coordinator.task_id,
                   question_waiter=(question, lambda text: consumed.append(text) or True))
    agent.sessions = {'incoming': session}
    def cancel(params):
        session['active'] = None
    agent._session_cancel = cancel
    agent._launch_prompt = launched.append
    agent._session_prompt(1, dict(sessionId='incoming', prompt=[dict(type='text', text='/cortex-stop')]))
    assert consumed == []
    assert launched[0][5][0] == commands.CORTEX_STOP
    assert questions_store.get(store, question).status == 'open'


@pytest.mark.parametrize('phase', ['EXECUTING', 'REVIEWING'])
def test_worker_without_retained_session_uses_question_fallback_not_mutation_failure(bound, phase):
    from cortex_runtime.orchestration import questions

    store, machine, coordinator, workers = bound
    worker = workers[0]
    if phase == 'REVIEWING':
        machine.move(coordinator.task_id, phase)
    with store.connection() as db:
        db.execute('UPDATE delegation_provisioning SET engine_session_ref=NULL WHERE attempt_id=?',
                   (worker.attempt_id,))
        db.commit()
    # Ordinary tools do not require the optional continuation binding.
    mcp_tools.dispatch(store, worker, 'project_write',
                       dict(path='part0.txt', content='before', expected_sha256='absent',
                            request_key='before'))
    # Access continuation still needs an exact session; no grant is invented.
    with pytest.raises(StoreError, match='worker_wait_session_missing'):
        mcp_tools.dispatch(store, worker, 'access_request',
                           dict(paths=['extra.txt'], reason='Needed', request_key='no-session-access'))
    receipt = _ask(store, worker)
    assert receipt == _ask(store, worker)
    assert receipt['complete'] and receipt['state'] == 'WAITING_USER'
    assert 'wait_id' not in receipt
    assert not worker_waits.list_waits(store, coordinator.task_id)
    with pytest.raises(StoreError, match='request_key_conflict'):
        mcp_tools.dispatch(store, worker, 'ask_question',
                           dict(question='Changed?', options=['small', 'large'], request_key='question'))
    assert machine.context(coordinator.task_id)['state'] == 'WAITING_USER'
    assert questions_store.answer(store, receipt['question_id'], 'Use the smaller target', 'text')
    assert questions.resume_answered(store, coordinator.task_id)
    assert machine.context(coordinator.task_id)['state'] == phase
    target = dispatch(store, worker, 'project_read', dict(path='part0.txt'))
    mcp_tools.dispatch(store, worker, 'project_write',
                       dict(path='part0.txt', content='after', expected_sha256=target['sha256'],
                            request_key='after'))


def test_complete_batch_resumes_same_worker_session_without_blocking_sibling(tmp_path):
    _runtime_flow(tmp_path, batch=True, sibling=True)


@pytest.mark.parametrize('language', ['en', 'ru'])
@pytest.mark.parametrize('mode', ['card', 'text', 'batch'])
def test_user_selected_language_reaches_worker_question_and_options_verbatim(tmp_path, language, mode):
    # Scripted generation verifies prompt delivery and transport, not a claim
    # that a paid model always obeys the language contract.
    _runtime_flow(tmp_path, language=language, elicitor=mode != 'text', batch=mode == 'batch')
