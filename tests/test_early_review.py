"""Review may overlap a result-bearing worker's remaining engine turn."""
import json
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import (
    EngineTurnFailed, OrchestratingRuntime, PromptOutcome,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import capacity, mcp_tools
from cortex_runtime.orchestration.recovery import release_observed
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from runtime_scenarios import provision_fixture, publish_fixture_report


def _submit(store, worker, key='result', **extra):
    report = publish_fixture_report(store, worker)
    args = dict(report=report, summary='Observed result', checks=[], request_key=key, **extra)
    return mcp_tools.dispatch(store, worker, 'result_submit', args)


@pytest.mark.parametrize('ending', ['normal', 'transport_eof', 'prompt_timeout', 'exception', 'cancelled'])
def test_review_starts_before_result_worker_exits(tmp_path, ending):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='early-review', event_id=ending), 'Implement feature', 'owner')
    reviewer_started = threading.Event()
    worker_finished = threading.Event()
    observations = []
    implementation = {}

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            machine = TaskMachine(store)
            if channel.role == 'coordinator':
                context = machine.context(task.task_id)
                if context['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(
                        plan=dict(tasks=[dict(id='implement', role='backend-dev', objective='Implement feature',
                                              dependencies=[], scope=['.'], model='auto')],
                                  verification=[['python3', '-B', '-c',
                                                 'from pathlib import Path; assert Path("feature.py").read_text() == "new\\n"']]),
                        request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            with store.connection() as db:
                stage = db.execute('SELECT n.stage FROM runtime_nodes n JOIN delegation_provisioning p '
                                   'ON p.idempotency_key=n.assignment_ref WHERE p.attempt_id=?',
                                   (channel.attempt_id,)).fetchone()[0]
            if stage == 'execution':
                implementation['channel'] = channel
                (tmp_path / 'feature.py').write_text('new\n')
                _submit(store, channel)
                # The real controller must dispatch review while this prompt
                # and its capacity reservation remain live, not after a sleep.
                try:
                    assert reviewer_started.wait(15), 'review waited for worker teardown'
                    if ending in {'transport_eof', 'prompt_timeout'}:
                        raise EngineTurnFailed(ending)
                    if ending == 'exception':
                        raise RuntimeError('late engine crash')
                    return PromptOutcome(StopReason.CANCELLED if ending == 'cancelled' else StopReason.END_TURN, '')
                finally:
                    worker_finished.set()
            worker = implementation['channel']
            with store.connection() as db:
                assert db.execute("SELECT state FROM capacity_reservations WHERE attempt_id=?",
                                  (worker.attempt_id,)).fetchone()[0] == 'reserved'
                assert db.execute("SELECT revoked_at FROM trusted_channels WHERE channel_id=?",
                                  (worker.channel_id,)).fetchone()[0] is not None
                released = db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='WORKER_RELEASED' "
                                      "AND json_extract(payload_json,'$.attempt_id')=?",
                                      (task.task_id, worker.attempt_id)).fetchone()
                assert released is None
            assert engine.lookup_owned(worker.attempt_id) == 'alive'
            with pytest.raises(StoreError, match='gateway_channel_inactive'):
                mcp_tools.dispatch(store, worker, 'project_write', dict(
                    path='late.txt', content='late', expected_sha256='absent', request_key='late-write'))
            observations.append('review-before-release')
            reviewer_started.set()
            assert worker_finished.wait(15)
            _submit(store, channel, verdict='APPROVE')
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='cn_early_review', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        outcome = runtime.start_prompt(task.task_id, 'incoming', 'Implement feature', lambda text: True,
                                       threading.Event(), emit_tool_call=lambda update: True)
    finally:
        reviewer_started.set()
        runtime.shutdown()
    assert outcome == StopReason.END_TURN
    assert observations == ['review-before-release']
    assert not (tmp_path / 'late.txt').exists()
    context = TaskMachine(store).context(task.task_id)
    assert context['state'] == 'COMPLETED', context
    assert len([n for n in context['nodes'] if n['stage'] == 'review']) == 1
    assert all(n['status'] == 'completed' for n in context['nodes'])
    assert engine.fallback_calls == []
    with store.connection() as db:
        rows = db.execute('SELECT type,payload_json FROM runtime_events WHERE task_id=? ORDER BY id',
                          (task.task_id,)).fetchall()
    started = next(i for i, row in enumerate(rows) if row[0] == 'REVIEW_STARTED')
    released = next(i for i, row in enumerate(rows) if row[0] == 'WORKER_RELEASED'
                    and json.loads(row[1])['attempt_id'] == implementation['channel'].attempt_id)
    assert started < released


def test_lingering_reservations_still_limit_reviewer_capacity(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='capacity', event_id='one'), 'Implement', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    workers = [provision_fixture(store, coordinator, key=f'worker-{i}')[0] for i in range(4)]
    reservations = [capacity.reserve(store, 'cn_slots', task.task_id, w.attempt_id, 'worker') for w in workers]
    assert all(reservations)
    for worker in workers:
        _submit(store, worker)
    machine = TaskMachine(store)
    machine.add_review(task.task_id)
    assert machine.context(task.task_id)['state'] == 'REVIEWING'
    assert machine.claim_ready(task.task_id, coordinator.channel_id) == []
    capacity.mark_ambiguous(store, reservations[0], 'timeout_alive')
    assert machine.claim_ready(task.task_id, coordinator.channel_id) == []
    observed = SimpleNamespace(_store=store, _engine=SimpleNamespace(lookup_owned=lambda attempt: 'unknown'))
    assert release_observed(observed, task.task_id, workers[0].attempt_id) is False
    assert capacity.count_active(store, 'cn_slots', 'worker') == 4
    capacity.release_confirmed(store, reservations[0], 'process_exited')
    observed._engine.lookup_owned = lambda attempt: 'absent'
    assert release_observed(observed, task.task_id, workers[0].attempt_id)
    claimed = machine.claim_ready(task.task_id, coordinator.channel_id)
    assert len(claimed) == 1 and claimed[0]['stage'] == 'review'
    with store.connection() as db:
        assert machine._session_worker_slots(db, task.task_id, 1) == 4
    # Three lingering engines plus the claimed reviewer: no fifth slot.
    assert capacity.count_active(store, 'cn_slots', 'worker') == 3


def test_native_late_change_still_rebinds_review(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='late-change', event_id='one'), 'Implement', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    _submit(store, worker)
    reviewer, review_assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_assignment,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    machine = TaskMachine(store)
    original = machine.start_review(task.task_id, review_assignment)
    report = publish_fixture_report(store, reviewer)
    # No universal native-action interception exists. Simulate a lingering
    # process's native edit rather than claiming MCP revocation blocks it.
    (tmp_path / 'late-native.py').write_text('late = True\n')
    args = dict(report=report, summary='Reviewed', checks=[], verdict='APPROVE', request_key='first')
    with pytest.raises(StoreError, match='review_project_revision_changed'):
        mcp_tools.dispatch(store, reviewer, 'result_submit', args)
    assert next(n for n in machine.context(task.task_id)['nodes']
                if n['assignment_ref'] == assignment)['status'] == 'completed'
    assert next(n for n in machine.context(task.task_id)['nodes']
                if n['assignment_ref'] == review_assignment)['status'] == 'running'
    args['request_key'] = 'rechecked'
    assert mcp_tools.dispatch(store, reviewer, 'result_submit', args)['recorded']
    review = next(n for n in machine.context(task.task_id)['nodes'] if n['stage'] == 'review')
    assert review['result']['project_digest'] != original


def test_unchanged_project_waits_for_teardown_before_verification(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import controller
    from runtime_scenarios import publish_result

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='waiver', event_id='one'), 'Inspect', 'owner')
    waiver_observed = threading.Event()
    attempts = []
    verified = []

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            context = TaskMachine(store).context(task.task_id)
            if channel.role == 'coordinator':
                if context['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                        tasks=[dict(id='inspect', role='explorer', objective='Inspect',
                                    dependencies=[], scope=['.'], model='auto')],
                        verification=[['python3', '-B', '-c', 'assert True']]), request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Inspected.'))
                return PromptOutcome(StopReason.END_TURN, '')
            attempts.append(channel.attempt_id)
            outcome = publish_result(store, channel)
            assert waiver_observed.wait(15), 'unchanged decision waited for engine exit'
            return outcome
        return behavior

    engine = FakeSessionRuntime(store, router, connection_id='cn_waiver', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    terminate = runtime._terminate_completed_workers
    verification = controller._verification

    def reap(task_id):
        if TaskMachine(store).get_state(task_id) == 'VERIFYING':
            waiver_observed.set()
        terminate(task_id)

    def verify(*args):
        assert attempts and all(engine.lookup_owned(a) == 'absent' for a in attempts)
        verified.append(True)
        return verification(*args)

    monkeypatch.setattr(runtime, '_terminate_completed_workers', reap)
    monkeypatch.setattr(controller, '_verification', verify)
    try:
        runtime.start_prompt(task.task_id, 'incoming', 'Inspect', lambda text: True,
                             threading.Event(), emit_tool_call=lambda update: True)
    finally:
        waiver_observed.set()
        runtime.shutdown()
    context = TaskMachine(store).context(task.task_id)
    assert context['state'] == 'COMPLETED', context
    assert verified == [True]
    assert context['review_not_required']['reason'] == 'no_project_changes'
    assert not any(n['stage'] == 'review' for n in context['nodes'])
