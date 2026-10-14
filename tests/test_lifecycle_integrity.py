"""Lifecycle integrity regressions: one test per audited defect.

No real engine subprocess and no paid model call: in-process fakes only.
"""
import json
import threading
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import questions_store, replay_store
from cortex_runtime.acp import messages as acp_messages
from cortex_runtime.acp import steering as acp_steering
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation import revision
from cortex_runtime.orchestration import controller, worker_waits
from cortex_runtime.orchestration.policy import interrupt
from cortex_runtime.orchestration.recovery import reconcile_results
from cortex_runtime.orchestration.state import TaskMachine, event
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from runtime_scenarios import provision_fixture, publish_fixture_report


class Crash(Exception):
    pass


def _fixture(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='integrity', event_id=str(uuid.uuid4())), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    return store, task, coordinator


def _request(store, task_id, actor, key):
    with store.connection() as db:
        row = db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                         (task_id, actor, key)).fetchone()
    return json.loads(row[0])


def _worker_results(store, task_id, attempt_id):
    with store.connection() as db:
        return [dict(row) for row in db.execute(
            "SELECT id, payload_json, superseded FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='worker_result'", (task_id, attempt_id))]


def _node(store, task_id, assignment):
    return next(n for n in TaskMachine(store).context(task_id)['nodes'] if n['assignment_ref'] == assignment)


def _result_args(key):
    return dict(report='# Result\n\nDone.\n', report_title='Result', summary='Done.', checks=[], request_key=key)


# A refused result never blocks later reconciliation ---------------------------

def test_a_result_refused_for_good_after_publication_never_blocks_later_reconciliation(tmp_path, monkeypatch):
    store, task, coordinator = _fixture(tmp_path)
    stale, stale_assignment = provision_fixture(store, coordinator, key='stale')
    live, live_assignment = provision_fixture(store, coordinator, key='live')
    real = TaskMachine.record_result
    crashes = []

    def crash_first_of_each(self, *args, **kwargs):
        if len(crashes) < 2:
            crashes.append(args[1])
            raise Crash('record_result')
        return real(self, *args, **kwargs)
    monkeypatch.setattr(TaskMachine, 'record_result', crash_first_of_each)
    # Both workers publish their result; neither is recorded (a crash
    # between publication and recording).
    for channel in (stale, live):
        with pytest.raises(Crash):
            mcp_tools.dispatch(store, channel, 'result_submit', _result_args('result'))
    # The stale worker's node is then cancelled (a Stop or replan committed
    # between its publication and its recording).
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND assignment_ref=?",
                   (task.task_id, stale_assignment))
        db.commit()
    # Before the fix: assignment_not_running escaped, aborted the loop (the
    # live result stayed unrecorded) and BLOCKED the task on every turn.
    reconcile_results(store, task.task_id)
    reconcile_results(store, task.task_id)
    assert _node(store, task.task_id, live_assignment)['status'] == 'completed'
    closed = _request(store, task.task_id, stale.channel_id, 'result')
    assert closed['complete'] and closed['error'] == 'assignment_not_running'
    (orphan,) = _worker_results(store, task.task_id, stale.attempt_id)
    assert orphan['superseded'] == 1
    assert _node(store, task.task_id, stale_assignment)['status'] == 'cancelled'
    with store.connection() as db:
        refused = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='RESULT_DELIVERY_REFUSED'",
                             (task.task_id,)).fetchall()
    assert [json.loads(row[0])['error'] for row in refused] == ['assignment_not_running']


def test_a_dispatch_refused_after_publication_closes_its_request_with_the_refusal(tmp_path, monkeypatch):
    store, task, coordinator = _fixture(tmp_path)
    worker, assignment = provision_fixture(store, coordinator)
    real_publish = mcp_tools._worker_publish_result

    def publish_then_cancel(*args, **kwargs):
        receipt = real_publish(*args, **kwargs)
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND assignment_ref=?",
                       (task.task_id, assignment))
            db.commit()
        return receipt
    monkeypatch.setattr(mcp_tools, '_worker_publish_result', publish_then_cancel)
    with pytest.raises(StoreError, match='assignment_not_running'):
        mcp_tools.dispatch(store, worker, 'result_submit', _result_args('result'))
    closed = _request(store, task.task_id, worker.channel_id, 'result')
    assert closed['complete'] and closed['error'] == 'assignment_not_running'
    assert [row['superseded'] for row in _worker_results(store, task.task_id, worker.attempt_id)] == [1]
    # An identical retry gets the same refusal, never a success receipt.
    monkeypatch.setattr(mcp_tools, '_worker_publish_result', real_publish)
    with pytest.raises(StoreError, match='assignment_not_running'):
        mcp_tools.dispatch(store, worker, 'result_submit', _result_args('result'))
    reconcile_results(store, task.task_id)


# verification_submit uses the plan's argv contract -----------------------------

@pytest.mark.parametrize('argv, code', [
    (['python3', '-c', 'print(1)\x00'], 'invalid_verification_command'),
    (['python3', '-c', 'print(1)', '|', 'cat'], 'verification_shell_syntax'),
])
def test_verification_submit_uses_the_plan_argv_contract(tmp_path, argv, code):
    store, task, coordinator = _fixture(tmp_path)
    TaskMachine(store).create(task.task_id, 'Build')
    # Before the fix both were stored (the check came only after the state
    # gate, and a NUL byte then crashed every VERIFYING turn in Popen).
    with pytest.raises(StoreError, match=code):
        mcp_tools.dispatch(store, coordinator, 'verification_submit',
                           dict(verification=[argv], request_key='repair'))
    with store.connection() as db:
        assert not db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='VERIFICATION_REVISED'",
                              (task.task_id,)).fetchone()


def test_a_verification_launch_failure_is_a_visible_stop_not_a_crash(tmp_path, monkeypatch):
    project = tmp_path / 'project'
    project.mkdir()
    store = Store(project / '.cortex', project_root=project)
    task = bootstrap_task(store, SourceEvent(origin='integrity', event_id='launch'), 'Verify.', 'owner')
    TaskMachine(store).create(task.task_id, 'Verify.')
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: project)
    before = revision.capture_revision(str(project))
    expected = before.get('dirty_digest') or before.get('digest')

    def refuse(*args, **kwargs):
        raise ValueError('embedded null byte')
    monkeypatch.setattr(controller.subprocess, 'Popen', refuse)
    cards = []
    observed = controller._verification_observation(
        runtime, task.task_id, {'revision': 1, 'plan': {'verification': [['python3', '-c', 'print(1)']]}},
        expected, cards.append, threading.Event(), 'run')
    assert observed == dict(blocker='verification_execution_error:ValueError', stale=False)
    assert cards[-1]['status'] == 'failed'
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='CHECK_EXECUTION_ABORTED'",
                          (task.task_id,)).fetchone()


def test_any_verification_runner_exception_stops_the_task_with_its_reason(tmp_path, monkeypatch):
    store, task, coordinator = _fixture(tmp_path)
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Build')
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id=?", (task.task_id,))
        db.commit()

    def broken(*args, **kwargs):
        raise OSError('output spool unavailable')
    monkeypatch.setattr(controller, '_verification_observation', broken)
    controller._verification(SimpleNamespace(_store=store), task.task_id, machine, None, threading.Event())
    context = machine.context(task.task_id)
    assert context['state'] == 'WAITING_USER'
    assert context['blocker'] == 'verification_execution_error:OSError'


# Events claimed by a failed coordinator prompt are released --------------------

def test_events_claimed_for_a_coordinator_prompt_that_raised_are_released(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task_id = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())),
                             'Build a page.', 'owner-1').task_id

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        raise RuntimeError('engine went away mid-turn')

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == 'coordinator' else None
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
                                   wait_timeout_seconds=5.0)
    runtime.start_prompt(task_id, 's1', 'Build a page.', lambda _text: True, threading.Event())
    with store.connection() as db:
        leaked = db.execute("SELECT event_type, consumed_by FROM task_events WHERE task_id=? AND superseded=0 "
                            "AND consumed_by IS NOT NULL AND delivered_at IS NULL", (task_id,)).fetchall()
        message = db.execute("SELECT consumed_by FROM task_events WHERE task_id=? AND event_type='user_message'",
                             (task_id,)).fetchone()
    # Before the fix the user's message stayed claimed by the failed prompt
    # and was never delivered again in this process.
    assert [dict(row) for row in leaked] == []
    assert message['consumed_by'] is None


# Cancelling a worker also cancels its open question -----------------------------

@pytest.fixture
def waiting_worker(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='wait-test', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    machine.submit_plan(coordinator, dict(
        tasks=[dict(id='N0', role='backend-dev', objective='Implement part 0', dependencies=[],
                    scope=['part0.txt'], model='auto')],
        verification=[['python3', '-B', '-c', 'print("verified")']]), 'plan')
    machine.move(boot.task_id, 'EXECUTING')
    (node,) = machine.claim_ready(boot.task_id, coordinator.channel_id)
    with store.connection() as db:
        channel_id, secret = issue_channel_tx(
            db, boot.task_id, 'worker', coordinator.owner_epoch,
            parent_channel_id=coordinator.channel_id, assignment_id='a_test0', attempt_id='xa_test0')
        stamp = store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (node['assignment_ref'], boot.task_id, 'a_test0', 'xa_test0', resolve_role('backend-dev'),
                    'ready', 'ready', 'shared_project_root', channel_id, None, None, 'es_test0', 'qualified',
                    None, coordinator.owner_epoch, stamp, stamp))
        db.commit()
    worker = resolve_channel(store, channel_id, secret)
    receipt = mcp_tools.dispatch(store, worker, 'ask_question',
                                 dict(question='Which target?', options=['small', 'large'], request_key='q'))
    return store, machine, boot.task_id, node['assignment_ref'], receipt['question_id']


def _cancel_assignment(store, task_id, assignment):
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._engine = SimpleNamespace(terminate_owned=lambda attempt: None, lookup_owned=lambda attempt: 'absent')
    runtime._lock = threading.Lock()
    runtime._worker_card_emitters = {}
    runtime._mark_work_attempt_failed = lambda *args, **kwargs: None
    runtime._durable_worker_card_sessions = lambda *args, **kwargs: {}
    runtime._cancel_assignment(task_id, assignment, 'replanning')


@pytest.mark.parametrize('path', ['interrupt', 'return_to_coordinator', 'stop_question', 'cancel_assignment'])
def test_every_path_that_cancels_a_worker_also_cancels_its_open_question(waiting_worker, path):
    store, machine, task_id, assignment, question_id = waiting_worker
    assert [q.id for q in questions_store.list_open(store, task_id)] == [question_id]
    if path == 'interrupt':
        interrupt(store, task_id)
    elif path == 'return_to_coordinator':
        assert machine.return_to_coordinator(task_id, 'worker_failed')
    elif path == 'stop_question':
        machine.move(task_id, 'BLOCKED', blocker='coordinator_prompt_failed:RuntimeError')
    else:
        _cancel_assignment(store, task_id, assignment)
    # Before the fix the question stayed open, so the next user message was
    # routed to it and swallowed as "question_already_answered".
    assert question_id not in [q.id for q in questions_store.list_open(store, task_id)]
    assert questions_store.get(store, question_id).status == 'cancelled'
    assert [w['status'] for w in worker_waits.list_waits(store, task_id)][-1] == 'cancelled'


# A queued message never becomes a false answer ----------------------------------

def test_a_queued_message_replay_reports_that_it_never_answered_the_question(tmp_path):
    store, task, _coordinator = _fixture(tmp_path)
    question = questions_store.create_if_absent(store, task.task_id, 'ask_user:env', 'Which environment?')
    origin, message_id = 'paseo-steer:s1', 'm1'
    key = f'user_message:{origin}:{message_id}'
    # The Paseo queue path records the message as steering, not as an answer.
    replay_store.record_user_message_event(store, task.task_id, key, 's1', dict(content='staging', session_id='s1'))
    event, answered = replay_store.record_user_message_answer_event(
        store, task.task_id, key, 's1', dict(content='staging', session_id='s1'),
        question_id=question.id, answer_text='staging')
    # Before the fix the replay said answered=True without answering.
    assert answered is False and event.superseded is False
    assert questions_store.get(store, question.id).status == 'open'
    # A real answer still replays as answered.
    other = questions_store.create_if_absent(store, task.task_id, 'ask_user:size', 'Which size?')
    _first, won = replay_store.record_user_message_answer_event(
        store, task.task_id, 'user_message:acp:s1:m2', 's1', dict(content='small', session_id='s1'),
        question_id=other.id, answer_text='small')
    _again, replayed = replay_store.record_user_message_answer_event(
        store, task.task_id, 'user_message:acp:s1:m2', 's1', dict(content='small', session_id='s1'),
        question_id=other.id, answer_text='small')
    assert won is True and replayed is True


def test_a_queued_message_continuation_is_steering_not_a_false_answer(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task_id = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())),
                             'Build a page.', 'owner-1').task_id
    prompts = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        prompts.append(message)
        return PromptOutcome(StopReason.END_TURN, 'Noted.')

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == 'coordinator' else None
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
                                   wait_timeout_seconds=5.0)
    question = questions_store.create_if_absent(store, task_id, 'ask_user:env', 'Which environment?')
    origin, message_id, text = 'paseo-steer:s1', 'queued-1', 'Also add a footer.'
    # The Paseo queue path captured this message as steering; its queued
    # continuation then starts a prompt with the same source identity.
    replay_store.record_user_message_event(store, task_id, f'user_message:{origin}:{message_id}', 's1',
                                           dict(content=text, session_id='s1'))
    acp_steering.record_steering(store, task_id, origin, message_id, text)
    updates = []
    runtime.start_prompt(task_id, 's1', text, updates.append, threading.Event(),
                         source_origin=origin, source_event_id=message_id)
    with store.connection() as db:
        false_answer = db.execute("SELECT 1 FROM task_events WHERE task_id=? AND dedup_key=?",
                                  (task_id, f'question_answered:{question.id}')).fetchone()
    # Before the fix the replay claimed an answer that was never recorded.
    assert false_answer is None
    assert questions_store.get(store, question.id).status == 'open'
    assert prompts  # the continuation ran as steering
    already = acp_messages.render('question_already_answered', 'en')
    assert not any(already in update for update in updates)


# A silently published result becomes the node's result -------------------------

def test_a_silent_published_result_is_recorded_as_the_nodes_result(tmp_path):
    store, task, coordinator = _fixture(tmp_path)
    worker, assignment = provision_fixture(store, coordinator)
    report_ref = publish_fixture_report(store, worker, 'Finished the work.')
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._close_assignment_that_produced_nothing(task.task_id, assignment)
    node = _node(store, task.task_id, assignment)
    # Before the fix the node stayed running with no result, and lost-worker
    # recovery reset and re-dispatched the finished work.
    assert node['status'] == 'completed' and node['result']['report_ref'] == report_ref
    with store.connection() as db:
        silent = db.execute("SELECT superseded FROM task_events WHERE task_id=? AND dedup_key=?",
                            (task.task_id, f'assignment_silent_result:{assignment}')).fetchone()
        revoked = db.execute('SELECT revoked_at FROM trusted_channels WHERE channel_id=?',
                             (worker.channel_id,)).fetchone()[0]
    assert silent['superseded'] == 0 and revoked is not None
