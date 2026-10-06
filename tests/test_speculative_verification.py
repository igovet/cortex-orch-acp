"""Speculation is candidate evidence until independent review approves it."""
import json
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools, revision
from cortex_runtime.orchestration import controller
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from runtime_scenarios import provision_fixture, publish_fixture_report


def _submit(store, channel, key='result', **extra):
    return mcp_tools.dispatch(store, channel, 'result_submit', dict(
        report=dict(report_ref=publish_fixture_report(store, channel)), summary='Observed', checks=[],
        request_key=key, **extra))


def _events(store, task_id):
    with store.connection() as db:
        return [(r[0], json.loads(r[1])) for r in db.execute(
            'SELECT type,payload_json FROM runtime_events WHERE task_id=? ORDER BY id', (task_id,))]


@pytest.mark.parametrize('scenario', ['approve', 'changed', 'request_changes', 'failure', 'in_flight', 'stop'])
def test_speculation_composed_runtime(tmp_path, monkeypatch, scenario):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='speculation', event_id=scenario), 'Implement', 'owner')
    finished = threading.Event()
    reviewer_approved = threading.Event()
    observed = []
    reviews = []
    original = controller._verification_observation
    cancelled = threading.Event()
    if scenario == 'stop':
        popen = controller.subprocess.Popen
        def launch(*args, **kwargs):
            proc = popen(*args, **kwargs)
            if kwargs.get('start_new_session'):
                cancelled.set()
            return proc
        monkeypatch.setattr(controller.subprocess, 'Popen', launch)

    def observe(*args):
        context = args[2]
        if scenario == 'in_flight':
            assert reviewer_approved.wait(15)
        result = original(*args)
        observed.append((context['revision'], args[3], result))
        finished.set()
        return result

    monkeypatch.setattr(controller, '_verification_observation', observe)
    repaired = []
    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            machine = TaskMachine(store)
            context = machine.context(task.task_id)
            if channel.role == 'coordinator':
                if context['state'] == 'PLANNING':
                    if context['revision'] == 0:
                        script = ('import time; time.sleep(60)' if scenario == 'stop' else
                                  'raise SystemExit(7)' if scenario == 'failure' else
                                  'from pathlib import Path; assert Path("feature.py").exists(); print("verified")')
                        mcp_tools.dispatch(store, channel, 'plan_submit', dict(
                            plan=dict(tasks=[dict(id='implement', role='backend-dev', objective='Implement',
                                                  dependencies=[], scope=['.'], model='auto')],
                                      verification=[['python3', '-B', '-c', script]]), request_key='plan'))
                    else:
                        assert scenario == 'failure'
                        assert _events(store, task.task_id)[-1][0] != 'VERIFICATION_PASSED'
                        repaired.append(True)
                        mcp_tools.dispatch(store, channel, 'verification_submit', dict(
                            verification=[['python3', '-B', '-c', 'from pathlib import Path; assert Path("feature.py").exists()']],
                            request_key='repair-verification'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            with store.connection() as db:
                stage = db.execute(
                    'SELECT n.stage FROM runtime_nodes n JOIN delegation_provisioning p ON p.idempotency_key=n.assignment_ref '
                    'WHERE p.attempt_id=?', (channel.attempt_id,)).fetchone()[0]
            if stage == 'execution':
                (tmp_path / 'feature.py').write_text('fixed\n' if reviews else 'new\n')
                _submit(store, channel)
            else:
                reviews.append(channel.attempt_id)
                if scenario == 'stop':
                    assert cancelled.wait(15)
                    return PromptOutcome(StopReason.CANCELLED, '')
                if scenario == 'in_flight':
                    _submit(store, channel, verdict='APPROVE')
                    reviewer_approved.set()
                else:
                    assert finished.wait(15), 'verification did not overlap review'
                    assert not any(t.startswith('VERIFICATION_') for t, _ in _events(store, task.task_id)), (
                        'speculation became authoritative before approval')
                    if scenario == 'changed' and len(reviews) == 1:
                        (tmp_path / 'feature.py').write_text('late native edit\n')
                        with pytest.raises(StoreError, match='review_project_revision_changed'):
                            _submit(store, channel, key='stale-verdict', verdict='APPROVE')
                        _submit(store, channel, key='rechecked-verdict', verdict='APPROVE')
                    elif scenario == 'request_changes' and len(reviews) == 1:
                        finished.clear()
                        _submit(store, channel, verdict='REQUEST_CHANGES')
                    else:
                        _submit(store, channel, verdict='APPROVE')
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='cn_speculation', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        outcome = runtime.start_prompt(task.task_id, 'incoming', 'Implement', lambda text: True,
                                       cancelled, emit_tool_call=lambda update: True)
    finally:
        reviewer_approved.set()
        runtime.shutdown()
    context = TaskMachine(store).context(task.task_id)
    if scenario == 'stop':
        assert outcome == StopReason.CANCELLED
        assert not any(t in {'VERIFICATION_PASSED', 'VERIFICATION_FAILED'} for t, _ in _events(store, task.task_id))
        assert any(t == 'SPECULATIVE_VERIFICATION_DISCARDED' for t, _ in _events(store, task.task_id))
        assert not any(t.name == 'cortex-speculative-verification' for t in threading.enumerate())
        return
    assert outcome == StopReason.END_TURN
    assert context['state'] == 'COMPLETED', context
    events = _events(store, task.task_id)
    authoritative = [(t, p) for t, p in events if t in {'VERIFICATION_PASSED', 'VERIFICATION_FAILED'}]
    assert authoritative[-1][0] == 'VERIFICATION_PASSED'
    assert authoritative[-1][1]['revision_digest'] == revision.capture_revision(str(tmp_path))['dirty_digest']
    if scenario in {'approve', 'in_flight'}:
        assert len(observed) == len(authoritative) == 1
        assert any(t == 'SPECULATIVE_VERIFICATION_CREDITED' for t, _ in events)
    elif scenario in {'changed', 'request_changes'}:
        assert len(observed) >= 2
        assert observed[0][1] != observed[-1][1]
        assert any(t == 'SPECULATIVE_VERIFICATION_DISCARDED' for t, _ in events)
        assert len(authoritative) == 1
    else:
        assert repaired == [True]
        assert [t for t, _ in authoritative] == ['VERIFICATION_FAILED', 'VERIFICATION_PASSED']
        assert authoritative[0][1]['checks'][0]['exit_code'] == 7
    assert engine.fallback_calls == []


def _candidate(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='candidate', event_id='one'), 'Implement', 'owner')
    from cortex_runtime.trusted_channel import resolve_channel
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, _ = provision_fixture(store, coordinator)
    _submit(store, worker)
    reviewer, assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (assignment,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    machine = TaskMachine(store)
    digest = machine.start_review(task.task_id, assignment)
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: str(tmp_path))
    return runtime, task, machine, reviewer, assignment, digest


def test_cancel_kills_speculative_process_and_keeps_no_authoritative_result(tmp_path, monkeypatch):
    runtime, task, machine, _, assignment, digest = _candidate(tmp_path)
    context = machine.context(task.task_id)
    context['plan'] = dict(verification=[['python3', '-B', '-c', 'import time; time.sleep(60)']])
    started = threading.Event()
    finished = threading.Event()
    procs = []
    popen = controller.subprocess.Popen
    def launch(*args, **kwargs):
        proc = popen(*args, **kwargs)
        procs.append(proc)
        started.set()
        return proc
    monkeypatch.setattr(controller.subprocess, 'Popen', launch)
    def emit(update):
        if update.get('status') == 'failed':
            finished.set()
    candidate = controller._SpeculativeVerification(runtime, task.task_id, emit)
    candidate.start(context, assignment, digest)
    assert started.wait(15)
    candidate.close('task_stopped')
    assert finished.is_set()
    assert candidate.thread is None
    assert procs and all(p.poll() is not None for p in procs)
    events = _events(runtime._store, task.task_id)
    assert not any(t in {'VERIFICATION_PASSED', 'VERIFICATION_FAILED'} for t, _ in events)
    assert events[-1][0] == 'SPECULATIVE_VERIFICATION_DISCARDED'


def test_candidate_digest_drift_is_not_credited(tmp_path, monkeypatch):
    runtime, task, machine, reviewer, assignment, digest = _candidate(tmp_path)
    context = machine.context(task.task_id)
    candidate = controller._SpeculativeVerification(runtime, task.task_id, None)
    def mutate(*args):
        (tmp_path / 'late.txt').write_text('changed')
        return dict(checks=[], digest=digest, stale=True)
    monkeypatch.setattr(controller, '_verification_observation', mutate)
    candidate.start(context, assignment, digest)
    candidate.thread.join(15)
    # Even a subsequently rechecked/approved reviewer cannot credit the old run.
    with pytest.raises(StoreError, match='review_project_revision_changed'):
        _submit(runtime._store, reviewer, key='stale', verdict='APPROVE')
    _submit(runtime._store, reviewer, key='fresh', verdict='APPROVE')
    machine.move(task.task_id, 'VERIFYING')
    assert candidate.credit(machine.context(task.task_id), machine) is False
    assert not any(t.startswith('VERIFICATION_') for t, _ in _events(runtime._store, task.task_id))


def test_lost_candidate_requires_ordinary_verification(tmp_path):
    runtime, task, machine, _, _, _ = _candidate(tmp_path)
    candidate = controller._SpeculativeVerification(runtime, task.task_id, None)
    assert candidate.credit(machine.context(task.task_id), machine) is False


@pytest.mark.parametrize('mismatch', ['revision', 'assignment', 'verdict', 'state'])
def test_transactional_credit_rejects_changed_binding(tmp_path, mismatch):
    runtime, task, machine, reviewer, assignment, digest = _candidate(tmp_path)
    _submit(runtime._store, reviewer, verdict='APPROVE')
    machine.move(task.task_id, 'VERIFYING')
    binding = dict(revision=machine.context(task.task_id)['revision'],
                   assignment_ref=assignment, digest=digest)
    with runtime._store.connection() as db:
        if mismatch == 'revision':
            db.execute('UPDATE runtime_tasks SET revision=revision+1 WHERE task_id=?', (task.task_id,))
        elif mismatch == 'assignment':
            db.execute("UPDATE runtime_nodes SET assignment_ref='new-review' WHERE assignment_ref=?", (assignment,))
        elif mismatch == 'verdict':
            db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.verdict','REQUEST_CHANGES') "
                       "WHERE assignment_ref=?", (assignment,))
        else:
            db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", (task.task_id,))
        db.commit()
    assert machine.record_verification(task.task_id, [], digest, speculative_binding=binding) is None
    assert not any(t in {'VERIFICATION_PASSED', 'VERIFICATION_FAILED', 'SPECULATIVE_VERIFICATION_CREDITED'}
                   for t, _ in _events(runtime._store, task.task_id))


def test_pending_reviewer_does_not_launch_speculation(tmp_path, monkeypatch):
    runtime, task, machine, _, assignment, _ = _candidate(tmp_path)
    with runtime._store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='pending' WHERE assignment_ref=?", (assignment,))
        db.commit()
    def forbidden(*args):
        pytest.fail('pending/capacity-queued reviewer cannot start speculative commands')
    candidate = controller._SpeculativeVerification(runtime, task.task_id, None)
    monkeypatch.setattr(candidate, 'start', forbidden)
    candidate.refresh(machine.context(task.task_id))
    assert candidate.thread is None


def test_controller_stop_discards_and_joins_candidate(tmp_path, monkeypatch):
    runtime, task, machine, _, assignment, digest = _candidate(tmp_path)
    stopped = threading.Event()
    finished = threading.Event()
    def check(*args):
        stopped.set()
        assert args[5].wait(15), 'controller stop did not cancel speculative verification'
        finished.set()
        return dict(stale=True)
    monkeypatch.setattr(controller, '_verification_observation', check)
    def drive(*args, speculation, **kwargs):
        speculation.start(machine.context(task.task_id), assignment, digest)
        assert stopped.wait(15)
        # Model the cancelled-turn unwind. run's finally must join it.
        return StopReason.CANCELLED
    monkeypatch.setattr(controller, '_run', drive)
    assert controller.run(runtime, task.task_id, None, None, None, '', None,
                          threading.Event(), 'en') == StopReason.CANCELLED
    assert finished.is_set()
    assert _events(runtime._store, task.task_id)[-1][0] == 'SPECULATIVE_VERIFICATION_DISCARDED'


def test_runner_marks_late_native_digest_change_stale(tmp_path):
    runtime, task, machine, _, _, digest = _candidate(tmp_path)
    context = machine.context(task.task_id)
    context['plan'] = dict(verification=[['python3', '-B', '-c', 'assert True']])
    def emit(update):
        if update.get('status') == 'completed':
            # A native/late writer, not a purported sandbox-write bypass.
            (tmp_path / 'late-native.txt').write_text('changed after command')
    observed = controller._verification_observation(
        runtime, task.task_id, context, digest, emit, threading.Event(), 'late-mutation')
    assert observed['stale'] is True
    assert observed['checks'][0]['exit_code'] == 0
    assert observed['checks'][0]['complete'] is False
    assert not any(t.startswith('VERIFICATION_') for t, _ in _events(runtime._store, task.task_id))
