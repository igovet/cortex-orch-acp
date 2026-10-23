"""No-paid qualification observer: real lock contention, exact attempt joins,
one steer, bounded waits and cleanup. Fresh local stores and controlled local
processes only; no host, account, model or daemon.
"""
import importlib.util
import json
from pathlib import Path
import select
import subprocess
import sys
import threading
import time

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events
from cortex_runtime.storage.inspection import discover_binding, read_binding_page
from test_task_inspection import bound, files  # noqa: F401 - fixture

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("consent", [None, {}, dict(mode="desktop", owner_request="owner", route="explicit"),
                                     dict(mode="cli", route="explicit")])
def test_supervisor_has_no_paid_authority_defaults(consent):
    result = observer.supervise_cli(consent, lambda _: pytest.fail("preflight"), None, None, None)
    assert result["phase"] == "consent_required"
    assert result["dispatched"] is False and result["accepted"] is False


def test_supervisor_distinguishes_preflight_dispatch_assertion_and_uncertainty():
    consent = dict(mode="cli", owner_request="explicit-owner-request", route="owner-selected-route")
    checks = {key: True for key in ("isolation", "native_login", "build", "plugin", "supported_cli", "route", "expectations")}
    ready = dict(ready=True, route=consent["route"], checks=checks)
    binding = dict(task_id="fixture-observed-binding")
    cleaned = []
    cleanup = lambda budget: (cleaned.append(True) or dict(ok=True))
    no_launch = lambda *a: pytest.fail("dispatched before ready")
    result = observer.supervise_cli(consent, lambda _: dict(ready=False), no_launch, None, cleanup)
    assert result["phase"] == "preflight" and not result["passed"] and result["dispatched"] is False
    for launch, assertion, phase, dispatched, passed in (
        (lambda *a: {}, None, "launch_attempted", None, False),
        (lambda *a: dict(dispatched=True, binding=binding), lambda *a: dict(passed=False),
         "dispatched", True, False),
        (lambda *a: dict(dispatched=True, binding=binding),
         lambda *a: dict(passed=True, binding=binding), "asserted", True, True),
    ):
        result = observer.supervise_cli(consent, lambda _: ready, launch, assertion, cleanup)
        assert (result["phase"], result["dispatched"], result["passed"]) == (phase, dispatched, passed)
        assert result["accepted"] is False
    assert len(cleaned) == 4


def test_supervisor_real_subprocess_timeout_and_supported_flags():
    budget = observer.Budget()
    result = observer.bounded_command([sys.executable, "-B", "-c", "print('fixture')"], budget)
    assert result == dict(exit_code=0, complete=True, byte_count=8, output=b"fixture\n")
    with pytest.raises(subprocess.TimeoutExpired):
        observer.bounded_command([sys.executable, "-B", "-c", "import time;time.sleep(10)"], budget, timeout=0.05)
    # Read-only installed wrapper help is not dispatched/accepted work.
    help_run = observer.bounded_command([sys.executable, "-B", str(ROOT / "scripts/cortex_paseo_dev.py"), "--help"], budget)
    assert help_run["exit_code"] == 0 and b"--check" in help_run["output"]
_spec = importlib.util.spec_from_file_location('cortex_qualification_observer',
                                               ROOT / 'scripts' / 'cortex_qualification_observer.py')
observer = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(observer)

# The exact shared, nonblocking access-lock call of Store.connection(), in a
# separate process, without touching the database (so file hashes stay exact).
HOLDER = '''
import fcntl, os, sys, time
fd = os.open(os.path.join(sys.argv[1], '.cortex', '.access.lock'), os.O_RDWR | os.O_NOFOLLOW)
fcntl.flock(fd, getattr(fcntl, sys.argv[3]) | fcntl.LOCK_NB)
print('held', flush=True)
time.sleep(float(sys.argv[2]))
fcntl.flock(fd, fcntl.LOCK_UN)
os.close(fd)
print('released', flush=True)
'''


class FakeClock:
    def __init__(self):
        self.now = 1000.0
        self.sleeps = []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        assert 0 <= seconds <= observer.MAX_WAIT_SECONDS
        self.sleeps.append(seconds)
        self.now += seconds


def _binding(project, sid, task, epoch):
    return discover_binding(str(project), sid)['binding'] | dict(task_id=task, owner_epoch=epoch)


def _hold_lock(project, seconds, mode='LOCK_SH'):
    """A separate process holding the store's real access lock (shared, or
    exclusive like the bound-task inspector's page snapshot)."""
    process = subprocess.Popen([sys.executable, '-B', '-c', HOLDER, str(project), str(seconds), mode],
                               stdout=subprocess.PIPE, text=True)
    ready, _, _ = select.select([process.stdout], [], [], 20)
    assert ready and process.stdout.readline().strip() == 'held'
    return process


def _stop(process):
    if process.poll() is None:
        process.kill()
    process.wait(timeout=10)


def test_real_cross_process_busy_lock_is_classified_and_retried_until_graceful_release(bound):
    project, store, sid, task, epoch = bound
    binding = _binding(project, sid, task, epoch)
    before = files(project)
    holder = _hold_lock(project, 1.0)
    try:
        # The inspector's own fail-closed refusal is unchanged: busy is raised.
        with pytest.raises(BlockingIOError):
            read_binding_page(str(project), sid, task, epoch)
        started = time.monotonic()
        result = observer.observe(binding, deadline=started + 30)
        assert time.monotonic() - started < 30
    finally:
        _stop(holder)
    assert result['status'] == 'complete' and result['counts']['busy'] >= 1
    assert result['counts']['restarts'] == 0 and result['total'] == len(result['rows'])
    assert holder.returncode == 0
    assert files(project) == before


def test_live_store_connection_waits_out_a_short_inspection_lock_but_not_a_long_one(bound):
    """An observer's exclusive snapshot of a running store must not fail the
    runtime's next ordinary connection (it raised storage_busy at once)."""
    from cortex_runtime import store as store_module
    project, store, sid, task, epoch = bound
    holder = _hold_lock(project, 0.5, 'LOCK_EX')
    try:
        with store.connection() as db:
            assert db.execute('SELECT 1').fetchone()[0] == 1
    finally:
        _stop(holder)
    assert holder.returncode == 0
    holder = _hold_lock(project, 60, 'LOCK_EX')
    try:
        # Exclusive maintenance access stays fail-fast.
        with pytest.raises(StoreError, match='storage_busy'):
            with store.connection(exclusive=True):
                pytest.fail('exclusive access must not wait')
        started = time.monotonic()
        with pytest.raises(StoreError, match='storage_busy'):
            with store.connection():
                pytest.fail('a persistent exclusive holder must still refuse')
        assert time.monotonic() - started >= store_module.SHARED_ACCESS_WAIT_SECONDS
    finally:
        _stop(holder)


def test_persistent_busy_ends_at_the_deadline_with_a_bounded_number_of_reads(bound):
    project, store, sid, task, epoch = bound
    binding = _binding(project, sid, task, epoch)
    clock = FakeClock()
    before = files(project)
    with store.connection():
        result = observer.observe(binding, deadline=clock() + 10, clock=clock, sleep=clock.sleep)
    assert result == dict(status='busy_expired', complete=False, blocker='inspection_busy',
                          counts=dict(reads=result['counts']['reads'], busy=result['counts']['busy'], restarts=0))
    assert result['counts']['busy'] == result['counts']['reads'] <= 15
    assert clock() <= 1010 and max(clock.sleeps) <= 1.0
    assert files(project) == before


@pytest.mark.parametrize('error, status, code', [
    (StoreError('inspection_binding_mismatch'), 'refused', 'inspection_binding_mismatch'),
    (StoreError('inspection_uncheckpointed_wal'), 'refused', 'inspection_uncheckpointed_wal'),
    (StoreError('storage_busy'), 'refused', 'inspection_refused'),
    (PermissionError(13, 'private'), 'error', 'unexpected_error:PermissionError'),
    (ValueError('private text'), 'error', 'unexpected_error:ValueError'),
])
def test_refusals_and_unexpected_errors_are_never_retried(error, status, code):
    calls = []

    def read_page(cursor):
        calls.append(cursor)
        raise error
    clock = FakeClock()
    result = observer.collect_snapshot(read_page, {}, deadline=clock() + 30, clock=clock, sleep=clock.sleep)
    assert (result['status'], result['blocker'], calls, clock.sleeps) == (status, code, [None], [])
    assert 'private' not in json.dumps(result)


def test_real_wrong_epoch_refusal_is_read_once(bound):
    project, store, sid, task, epoch = bound
    result = observer.observe(_binding(project, sid, task, epoch + 1), deadline=time.monotonic() + 10)
    assert (result['status'], result['blocker'], result['counts']['reads']) == (
        'refused', 'inspection_binding_mismatch', 1)


def test_stale_cursor_restarts_through_the_contract_and_mixed_pages_are_rejected(bound):
    project, store, sid, task, epoch = bound
    binding = _binding(project, sid, task, epoch)
    real = observer.inspector_pages(binding, limit=1)
    appended = []

    def changing(cursor):
        page = real(cursor)
        if cursor is not None and not appended:
            appended.append(True)
            events.append_event(store, task, 'user_message', 'late', {'content': 'late body'}, 'fixture')
        return page
    result = observer.collect_snapshot(changing, binding, deadline=time.monotonic() + 30)
    assert result['status'] == 'complete' and result['counts']['restarts'] == 1
    assert result['snapshot_sha256'] == read_binding_page(str(project), sid, task, epoch, limit=200)['snapshot_sha256']
    # A later stale cursor without restart allowance stays refused.
    first = real(None)
    events.append_event(store, task, 'user_message', 'later', {'content': 'later body'}, 'fixture')
    pages = iter([first])
    result = observer.collect_snapshot(lambda cursor: next(pages) if cursor is None else real(cursor), binding,
                                       deadline=time.monotonic() + 30, restarts=0)
    assert (result['status'], result['blocker']) == ('refused', 'inspection_snapshot_changed')
    # Pages from different snapshots, or of another binding, never combine.
    first, second = real(None), real(real(None)['next_cursor'])
    mixed = iter([first, dict(second, snapshot_sha256='0' * 64)])
    result = observer.collect_snapshot(lambda cursor: next(mixed), binding, deadline=time.monotonic() + 30)
    assert (result['status'], result['blocker']) == ('refused', 'inspection_mixed_snapshot')
    other = iter([dict(first, binding=dict(binding, owner_epoch=epoch + 1))])
    result = observer.collect_snapshot(lambda cursor: next(other), binding, deadline=time.monotonic() + 30)
    assert (result['status'], result['blocker']) == ('refused', 'inspection_binding_changed')
    skipped = iter([first, dict(second, offset=2)])
    result = observer.collect_snapshot(lambda cursor: next(skipped), binding, deadline=time.monotonic() + 30)
    assert (result['status'], result['blocker']) == ('refused', 'inspection_page_offset')


REF = 'f' * 64


def _snapshot(**changes):
    rows = [dict(section='runtime', state='EXECUTING', revision=2),
            dict(section='nodes', revision=2, node_id='fix_check', role='qa-engineer', stage='execution',
                 status='running', assignment_ref=REF),
            # The provisioning work-graph id is a different identity domain.
            dict(section='attempts', idempotency_key=REF, attempt_id='xa_000000000001', assignment_id='a_000000000001',
                 role_id='worker-qa-engineer', status='ready', step='ready', owner_epoch=1,
                 engine_session_ref='es_000000000001'),
            dict(section='task_events', kind='worker_configuration_confirmed', attempt_id='xa_000000000001',
                 engine_session_ref='es_000000000001', assignment_ref=REF),
            dict(section='capacity', attempt_id='xa_000000000001', kind='worker', state='reserved')]
    for index, update in changes.items():
        rows[int(index[1:])] = dict(rows[int(index[1:])], **update)
    return dict(status='complete', binding=dict(project_root='/fixture', task_id='t_' + '1' * 12,
                                                session_id='acps_' + '1' * 12, owner_epoch=1),
                snapshot_sha256='1' * 64, rows=rows)


def test_selector_joins_node_ref_to_the_provisioning_key_never_the_work_graph_id():
    selected = observer.select_running_attempt(_snapshot())
    assert selected == dict(status='selected', snapshot_sha256='1' * 64, binding=_snapshot()['binding'],
                            event_high_watermark=0, node_id='fix_check', revision=2,
                            assignment_ref=REF, attempt_id='xa_000000000001',
                            engine_session_ref='es_000000000001', role_id='worker-qa-engineer')
    # An opaque-ID equality guess must not select a row bound to another node.
    trap = _snapshot(r2=dict(idempotency_key='e' * 64, assignment_id=REF))
    assert observer.select_running_attempt(trap)['reason'] == 'no_running_attempt'


@pytest.mark.parametrize('changes, reason', [
    (dict(r0=dict(state='PLANNING')), 'not_executing'),
    (dict(r1=dict(status='completed')), 'no_running_attempt'),
    (dict(r1=dict(revision=1)), 'no_running_attempt'),
    (dict(r2=dict(status='in_progress', step='engine_session')), 'no_running_attempt'),
    (dict(r2=dict(owner_epoch=2)), 'no_running_attempt'),
    (dict(r2=dict(engine_session_ref=None)), 'no_running_attempt'),
    (dict(r3=dict(engine_session_ref='es_000000000002')), 'no_running_attempt'),
    (dict(r3=dict(assignment_ref=None)), 'no_running_attempt'),
    (dict(r3=dict(kind='progress')), 'no_running_attempt'),
    (dict(r4=dict(state='released')), 'no_running_attempt'),
])
def test_selector_refuses_without_every_exact_identity(changes, reason):
    assert observer.select_running_attempt(_snapshot(**changes))['reason'] == reason


def test_selector_refuses_two_running_attempts_and_a_terminal_one():
    snapshot = _snapshot()
    second = [dict(r, assignment_ref='d' * 64) if r['section'] in ('nodes', 'task_events') else
              dict(r, idempotency_key='d' * 64) if r['section'] == 'attempts' else dict(r)
              for r in snapshot['rows'][1:]]
    for row in second:
        if row['section'] == 'nodes':
            row['node_id'] = 'second'
        if 'attempt_id' in row:
            row['attempt_id'] = 'xa_000000000002'
    ambiguous = dict(snapshot, rows=snapshot['rows'] + second)
    assert observer.select_running_attempt(ambiguous)['reason'] == 'ambiguous_running_attempts'
    terminal = dict(snapshot, rows=snapshot['rows'] + [dict(section='runtime_events', type='WORKER_FAILED',
                                                             attempt_id='xa_000000000001')])
    assert observer.select_running_attempt(terminal)['node_reasons'] == ['attempt_terminal']
    assert observer.select_running_attempt(dict(snapshot, status='busy_expired'))['reason'] == 'snapshot_incomplete'


def test_steer_is_sent_at_most_once_and_never_replayed_after_an_uncertain_send():
    selected = observer.select_running_attempt(_snapshot())
    sent = []
    steer = observer.SteerOnce(lambda text: sent.append(text) or dict(acknowledged=True))
    with pytest.raises(ValueError):
        steer(dict(status='refused'))
    assert steer(selected) == dict(acknowledged=True) and sent == [observer.DECISION_FREE_STEER]
    with pytest.raises(RuntimeError, match='steer_already_sent'):
        steer(selected)

    def uncertain(text):
        sent.append(text)
        raise TimeoutError
    steer = observer.SteerOnce(uncertain)
    with pytest.raises(TimeoutError):
        steer(selected)
    with pytest.raises(RuntimeError, match='steer_already_sent'):
        steer(selected)
    assert len(sent) == 2


def _sent(selection=None):
    selection = selection or observer.select_running_attempt(_snapshot())
    steer = observer.SteerOnce(lambda text: dict(acknowledged=True))
    steer(selection, origin='acp:test', event_id='exact-steer')
    return selection, steer.sent


def _captured(sent, **changes):
    snapshot = _snapshot(**changes)
    snapshot['snapshot_sha256'] = '2' * 64
    snapshot['rows'].append(dict(section='runtime_events', id=10, type='USER_STEERING',
                                 actor_sha256=sent['actor_sha256']))
    return snapshot


def _worker_event(kind='TOOL_COMPLETED'):
    return dict(section='runtime_events', id=11, type=kind, attempt_id='xa_000000000001',
                channel_attempt_id='xa_000000000001', channel_owner_epoch=1)


def test_continuation_rejects_unchanged_presteer_and_capture_without_progress():
    selected, sent = _sent()
    assert observer.check_continuation(selected, _snapshot()) == dict(
        status='unverified', reason='steering_identity_unavailable')
    assert observer.check_continuation(selected, _snapshot(), steering=sent) == dict(
        status='pending', reason='steering_capture_not_observed')
    assert observer.check_continuation(selected, _captured(sent), steering=sent) == dict(
        status='pending', reason='continuation_not_observed')
    missing_identity = observer.SteerOnce(lambda text: True)
    missing_identity(selected)
    assert observer.check_continuation(selected, _captured(sent), steering=missing_identity.sent)['status'] == 'unverified'


@pytest.mark.parametrize('case, reason', [
    ('incomplete', 'snapshot_incomplete'), ('no_binding', 'baseline_binding_unavailable'),
    ('no_watermark', 'baseline_events_unavailable'), ('wrong_send', 'steering_identity_unavailable'),
])
def test_continuation_keeps_missing_observability_unverified(case, reason):
    selected, sent = _sent()
    snapshot = _captured(sent)
    if case == 'incomplete':
        snapshot['status'] = 'busy_expired'
    elif case == 'no_binding':
        selected.pop('binding')
    elif case == 'no_watermark':
        selected.pop('event_high_watermark')
    else:
        sent['selection']['attempt_id'] = 'xa_other'
    assert observer.check_continuation(selected, snapshot, steering=sent) == dict(status='unverified', reason=reason)


def test_selector_refuses_ambiguous_baseline_events():
    snapshot = _snapshot()
    snapshot['rows'] += [dict(section='runtime_events', type='USER_STEERING', id=1)] * 2
    assert observer.select_running_attempt(snapshot) == dict(status='refused', reason='ambiguous_runtime_events')


def test_continuation_requires_progress_after_exact_capture_not_after_application():
    selected, sent = _sent()
    progressed = _captured(sent)
    progressed['rows'].append(_worker_event())
    assert observer.check_continuation(selected, progressed, steering=sent) == dict(
        status='continued', reason='progressed_same_attempt')
    done = _captured(sent, r1=dict(status='completed'))
    done['rows'] += [_worker_event('WORKER_COMPLETED'),
                     dict(section='runtime_events', id=12, type='STEERING_APPLIED', actor_sha256=sent['actor_sha256'])]
    assert observer.check_continuation(selected, done, steering=sent) == dict(
        status='continued', reason='completed_same_attempt')


@pytest.mark.parametrize('changes, reason', [
    (dict(r2=dict(attempt_id='xa_000000000009')), 'attempt_replaced'),
    (dict(r2=dict(engine_session_ref='es_000000000009')), 'attempt_replaced'),
    (dict(r3=dict(engine_session_ref='es_000000000009')), 'native_confirmation_mismatch'),
    (dict(r3=dict(assignment_ref=None)), 'native_confirmation_mismatch'),
    (dict(r2=dict(owner_epoch=2)), 'owner_changed'),
])
def test_continuation_rejects_replacement_and_identity_mismatch(changes, reason):
    selected, sent = _sent()
    assert observer.check_continuation(selected, _captured(sent, **changes), steering=sent) == dict(
        status='failed', reason=reason)


@pytest.mark.parametrize('change', [dict(task_id='t_other'), dict(owner_epoch=2), dict(session_id='acps_other')])
def test_continuation_rejects_wrong_task_owner_or_acp_session(change):
    selected, sent = _sent()
    snapshot = _captured(sent)
    snapshot['binding'] = dict(snapshot['binding'], **change)
    assert observer.check_continuation(selected, snapshot, steering=sent) == dict(status='failed', reason='binding_changed')


@pytest.mark.parametrize('case', ['wrong_actor', 'pre_capture_progress', 'other_attempt', 'other_owner', 'old_terminal'])
def test_unrelated_or_pre_capture_events_are_not_continuation(case):
    selected, sent = _sent()
    snapshot = _captured(sent)
    progress = _worker_event()
    if case == 'wrong_actor':
        snapshot['rows'][-1]['actor_sha256'] = 'wrong'
    elif case == 'pre_capture_progress':
        progress['id'] = 9
    elif case == 'other_attempt':
        progress['channel_attempt_id'] = 'xa_other'
    elif case == 'other_owner':
        progress['channel_owner_epoch'] = 2
    elif case == 'old_terminal':
        progress.update(type='WORKER_COMPLETED', id=9)
    snapshot['rows'].append(progress)
    assert observer.check_continuation(selected, snapshot, steering=sent)['status'] == 'pending'


@pytest.mark.parametrize('case, reason', [
    ('provisioning', 'attempt_replaced'), ('confirmation', 'native_confirmation_mismatch'),
    ('capture', 'steering_replayed_or_ambiguous'), ('event_id', 'ambiguous_runtime_events'),
    ('session_confirmation', 'native_confirmation_mismatch'), ('terminal', 'ambiguous_terminal_result'),
])
def test_continuation_refuses_ambiguous_records(case, reason):
    selected, sent = _sent()
    snapshot = _captured(sent)
    if case == 'provisioning':
        snapshot['rows'].append(dict(snapshot['rows'][2]))
    elif case == 'confirmation':
        snapshot['rows'].append(dict(snapshot['rows'][3]))
    elif case == 'session_confirmation':
        snapshot['rows'].append(dict(snapshot['rows'][3], attempt_id='xa_other'))
    elif case == 'capture':
        snapshot['rows'].append(dict(snapshot['rows'][-1], id=12))
    elif case == 'event_id':
        event = _worker_event()
        event['id'] = 10
        snapshot['rows'].append(event)
    else:
        snapshot['rows'] += [_worker_event('WORKER_COMPLETED'), dict(_worker_event('WORKER_COMPLETED'), id=12)]
    assert observer.check_continuation(selected, snapshot, steering=sent) == dict(status='failed', reason=reason)


def test_baseline_watermark_prevents_replayed_steering_and_old_completion():
    baseline = _snapshot()
    baseline['rows'].append(dict(section='runtime_events', id=20, type='USER_STEERING', actor_sha256='old'))
    selected, sent = _sent(observer.select_running_attempt(baseline))
    assert selected['event_high_watermark'] == 20
    assert observer.check_continuation(selected, _captured(sent), steering=sent) == dict(
        status='failed', reason='steering_replayed_or_ambiguous')
    failed = _captured(sent)
    failed['rows'].append(_worker_event('WORKER_FAILED'))
    assert observer.check_continuation(selected, failed, steering=sent)['reason'] == 'attempt_failed'
    unsuccessful = _captured(sent)
    unsuccessful['rows'].append(dict(_worker_event('WORKER_COMPLETED'), assignment_outcome='failed'))
    assert observer.check_continuation(selected, unsuccessful, steering=sent)['reason'] == 'attempt_failed'


@pytest.mark.usefixtures('unjailed_gateway_command')
def test_real_controller_running_worker_is_selected_steered_once_and_continues(tmp_path):
    """The previously unexercised branch on a real controller-driven attempt."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.acp.steering import record_steering
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store
    from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
    from test_steering_after_verification import _misused_then_answered, _plan

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='observed'), 'Implement part 0', 'owner')
    sid = 'acps_' + 'c' * 12
    sessions.create_session(store, sid, str(tmp_path), {'name': 'fixture'}, {}, [])
    sessions.bind_task(store, sid, task.task_id)
    binding = discover_binding(str(tmp_path), sid)['binding']
    seen = dict(refusals=[], planned=False)
    sends = []
    steer = observer.SteerOnce(lambda text: sends.append(text) or record_steering(
        store, task.task_id, 'acp:test', 'observer-steer', text))

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
                if context['state'] == 'PLANNING' and not seen['planned']:
                    seen['planned'] = True
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=_plan(), request_key='plan-0'))
                elif context['state'] == 'PLANNING':
                    _misused_then_answered(store, channel, context, seen)
                elif context['state'] == 'RECONCILING':
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))
                return PromptOutcome(StopReason.END_TURN, 'Turn ended')
            with store.connection() as db:
                provision = db.execute('SELECT idempotency_key,assignment_id,role_id FROM delegation_provisioning '
                                       'WHERE attempt_id=?', (channel.attempt_id,)).fetchone()
            reviewer = provision['role_id'] == 'worker-code-reviewer'
            if not reviewer and 'selection' not in seen:
                # What the live adapter appends after the engine's own confirmation.
                events.append_event(store, task.task_id, 'progress', f'worker-config:{channel.attempt_id}:{ref}:fixture',
                                    dict(kind='worker_configuration_confirmed', attempt_id=channel.attempt_id,
                                         engine_session_ref=ref, config_revision_id=None, engine='claude-agent-acp',
                                         connection_id='fixture-connection', model='haiku',
                                         parameters=dict(thought_level='high'), confirmation_evidence_ref='fixture'),
                                    'engine_configuration', assignment_ref=provision['idempotency_key'])
                deadline = time.monotonic() + 30
                live = observer.observe(binding, deadline=deadline)
                seen['live'] = live
                seen['provision'] = dict(provision)
                seen['selection'] = observer.select_running_attempt(live)
                seen['expected'] = (provision['idempotency_key'], channel.attempt_id, ref)
                if seen['selection']['status'] == 'selected':
                    steer(seen['selection'], origin='acp:test', event_id='observer-steer')
                    seen['continued'] = observer.check_continuation(
                        seen['selection'], observer.observe(binding, deadline=deadline), steering=steer.sent)
            if not reviewer:
                (tmp_path / 'src' / 'part0').mkdir(parents=True, exist_ok=True)
                (tmp_path / 'src' / 'part0' / f'{channel.attempt_id}.txt').write_text('implemented')
            draft = store.call('create_draft', dict(template='general', request_key='draft-' + channel.attempt_id),
                               channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
            path = Path(draft['draft_path'])
            path.write_bytes(path.read_bytes().split(b'\n\n', 1)[0] + b'\n\n# Report\n\nObserved result.\n')
            report = store.call('write_report', dict(draft_id=draft['draft_id'], title='Result', summary='Observed result',
                                                     author='worker', request_key='report-' + channel.attempt_id),
                                channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
            args = dict(report=report['report_id'], summary='Observed result', checks=[], request_key='result')
            if reviewer:
                args['verdict'] = 'APPROVE'
            mcp_tools.dispatch(store, channel, 'result_submit', args)
            return PromptOutcome(StopReason.END_TURN, 'Worker turn ended')
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='fixture-connection')
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        runtime.start_prompt(task.task_id, 'incoming', 'Implement part 0', lambda chunk: None,
                             threading.Event(), emit_tool_call=lambda update: True)
    finally:
        runtime.shutdown()
    assert seen['live']['status'] == 'complete', seen['live']
    selection = seen['selection']
    assert selection['status'] == 'selected', selection
    assert (selection['assignment_ref'], selection['attempt_id'], selection['engine_session_ref']) == seen['expected']
    # The broken supervisor compared these two different identity domains.
    assert seen['provision']['assignment_id'].startswith('a_')
    assert seen['provision']['assignment_id'] != selection['assignment_ref']
    assert sends == [observer.DECISION_FREE_STEER]
    assert seen['continued'] == dict(status='pending', reason='continuation_not_observed')
    final = observer.observe(binding, deadline=time.monotonic() + 30)
    assert observer.check_continuation(selection, final, steering=steer.sent) == dict(
        status='continued', reason='completed_same_attempt')
    assert not any(row['section'] == 'attempts' and row['attempt_id'] != selection['attempt_id']
                   and row['idempotency_key'] == selection['assignment_ref'] for row in final['rows'])


def test_case_budget_bounds_every_wait_and_keeps_the_cleanup_reserve():
    clock = FakeClock()
    with pytest.raises(ValueError):
        observer.Budget(limit=601, clock=clock)
    with pytest.raises(ValueError):
        observer.Budget(cleanup_reserve=89, clock=clock)
    budget = observer.Budget(clock=clock)
    assert budget.case_deadline - budget.start == 600 and budget.case_deadline - budget.observe_deadline == 90
    assert budget.wait_deadline(300) - clock() == 60
    clock.now = budget.observe_deadline - 5
    assert budget.wait_deadline() == budget.observe_deadline


def _cleanup(clock, seconds=1, ok=True, calls=None):
    def cleanup(budget):
        calls.append(budget.clock())
        clock.now += seconds
        return dict(ok=ok)
    return cleanup


@pytest.mark.parametrize('exercise, blockers', [
    (lambda budget: dict(status='passed'), []),
    (lambda budget: 1 / 0, ['exercise_error']),
    (lambda budget: dict(status='timeout', blocker='condition_not_observed'), ['exercise_timeout']),
    (lambda budget: None, ['exercise_invalid']),
])
def test_cleanup_runs_after_every_outcome(exercise, blockers):
    clock, calls = FakeClock(), []
    result = observer.run_case(observer.Budget(clock=clock), exercise, _cleanup(clock, calls=calls))
    assert len(calls) == 1 and result['blockers'] == blockers and result['passed'] is (not blockers)
    if blockers == ['exercise_error']:
        assert result['outcome'] == dict(status='error', blocker='unexpected_error:ZeroDivisionError')


def test_overrun_cleanup_failure_and_interrupt_still_clean_up():
    clock, calls = FakeClock(), []

    def overrun(budget):
        clock.now = budget.observe_deadline + 1
        return dict(status='passed')
    result = observer.run_case(observer.Budget(clock=clock), overrun, _cleanup(clock, seconds=100, calls=calls))
    assert result['blockers'] == ['observation_overran_cleanup_reserve', 'case_deadline_exceeded'] and len(calls) == 1

    def broken(budget):
        raise OSError('private')
    result = observer.run_case(observer.Budget(clock=clock), lambda budget: dict(status='passed'), broken)
    assert result['cleanup'] == dict(ok=False, blocker='cleanup_error:OSError') and not result['passed']
    result = observer.run_case(observer.Budget(clock=clock), lambda budget: dict(status='passed'),
                               _cleanup(clock, ok=False, calls=calls))
    assert result['blockers'] == ['cleanup_failed']

    def interrupted(budget):
        raise KeyboardInterrupt
    with pytest.raises(KeyboardInterrupt):
        observer.run_case(observer.Budget(clock=clock), interrupted, _cleanup(clock, calls=calls))
    assert len(calls) == 3


def test_poll_until_is_bounded_and_stops_on_a_refusal():
    clock = FakeClock()
    seen = []

    def observe_once(deadline):
        seen.append(deadline)
        return dict(status='complete', rows=[])
    result = observer.poll_until(observe_once, lambda o: False, deadline=clock() + 10, interval=2,
                                 clock=clock, sleep=clock.sleep)
    assert result['status'] == 'timeout' and len(seen) == 5 and clock() <= 1010
    result = observer.poll_until(lambda d: dict(status='refused', blocker='inspection_binding_mismatch'),
                                 lambda o: True, deadline=clock() + 10, clock=clock, sleep=clock.sleep)
    assert (result['status'], result['blocker']) == ('failed', 'inspection_binding_mismatch')
    result = observer.poll_until(lambda d: dict(status='busy_expired', blocker='inspection_busy'),
                                 lambda o: True, deadline=clock() + 10, clock=clock, sleep=clock.sleep)
    assert (result['status'], result['blocker']) == ('timeout', 'inspection_busy')


def test_owned_helper_cleanup_reports_grace_truthfully():
    def child(seconds):
        return subprocess.Popen([sys.executable, '-B', '-c', f'import time; time.sleep({seconds})'])
    gone, slow, stuck = child(0), child(0.5), child(30)
    processes = {p.pid: p for p in (gone, slow, stuck)}
    alive = lambda pid: processes[pid].poll() is None
    try:
        gone.wait(timeout=10)
        assert observer.wait_absent([gone.pid], alive, grace=5) == dict(
            absent=True, immediate=True, elapsed=0.0, remaining=[])
        result = observer.wait_absent([slow.pid], alive, grace=10)
        assert result['absent'] and not result['immediate'] and 0 < result['elapsed'] < 10
        result = observer.wait_absent([stuck.pid], alive, grace=0.3)
        assert result['absent'] is False and result['remaining'] == [stuck.pid] and result['elapsed'] >= 0.3
    finally:
        for process in processes.values():
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
