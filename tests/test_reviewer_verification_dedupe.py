"""Exact reviewer argv reuse is runtime-attributed, never a second execution."""
import json
import threading
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import evidence, mcp_tools
from cortex_runtime.orchestration import controller, gateway, reviewer_verification
from cortex_runtime.orchestration.state import TaskMachine, encoded, event
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.large_output_store import read_output
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report

ARGV = ['python3', '-B', '-c', 'assert 2 + 2 == 4; print("verified")']


def fixture(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='dedupe', event_id='one'), 'Implement', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, _ = provision_fixture(store, coordinator)
    reviewer, assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (assignment,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',plan_json=? WHERE task_id=?",
                   (encoded(dict(verification=[ARGV])), task.task_id))
        db.commit()
    machine = TaskMachine(store)
    digest = machine.start_review(task.task_id, assignment)
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: str(tmp_path),
                              _shutting_down=threading.Event())
    return runtime, task, machine, worker, reviewer, assignment, digest


def observe(runtime, task, machine, digest, run_id=123):
    return controller._verification_observation(runtime, task.task_id, machine.context(task.task_id),
                                               digest, None, threading.Event(), run_id)


def command(store, caller, argv=ARGV, key='command'):
    return mcp_tools.dispatch(store, caller, 'command_run', dict(argv=argv, request_key=key))


def no_execution(*args):
    pytest.fail('reviewer executed an already retained runtime check')


@pytest.mark.parametrize('exit_code', [0, 7])
def test_finished_receipt_and_result_attribution(tmp_path, monkeypatch, exit_code):
    runtime, task, machine, worker, reviewer, _, digest = fixture(tmp_path)
    if exit_code:
        argv = ['python3', '-B', '-c', 'raise SystemExit(7)']
        with runtime._store.connection() as db:
            db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
                       (encoded(dict(verification=[argv])), task.task_id))
            db.commit()
        # Nonzero is a genuine retained check outcome, not a launch failure.
    else:
        argv = ARGV
    observed = observe(runtime, task, machine, digest)
    monkeypatch.setattr(gateway, '_command', no_execution)
    receipt = command(runtime._store, reviewer, argv)
    assert receipt['exit_code'] == exit_code
    assert receipt['complete'] is True
    assert receipt['revision_digest'] == digest
    assert receipt['runtime_verification']['command_ref'] == observed['checks'][0]['command_ref']
    assert receipt['runtime_verification']['source'] == 'runtime'
    with runtime._store.connection() as db:
        owner = db.execute('SELECT owner_scope FROM large_outputs WHERE id=?', (receipt['large_output_id'],)).fetchone()[0]
        assessed = evidence.assess_check(db, task.task_id, reviewer.attempt_id,
                                         'failed' if exit_code else 'passed', receipt['tool_call_ref'])
        assert assessed['strength'] == 'verified'
        assert assessed['evidence_source'] == 'runtime_verification'
        assert evidence.assess_check(db, task.task_id, worker.attempt_id, 'passed',
                                    receipt['tool_call_ref'])['gap'] == 'foreign_attempt'
    output = read_output(runtime._store, task_id=task.task_id, owner_scope=owner,
                         record_id=receipt['large_output_id'])
    assert output['truncated'] is False
    if not exit_code:
        report = publish_fixture_report(runtime._store, reviewer)
        result = mcp_tools.dispatch(runtime._store, reviewer, 'result_submit', dict(
            report=report, summary='Checked diff independently', verdict='APPROVE', request_key='result',
            checks=[dict(name='planned verification', outcome='passed', tool_call_ref=receipt['tool_call_ref'])]))
        with runtime._store.connection() as db:
            rows = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
                              (task.task_id,)).fetchall()
        checks = json.loads(rows[-1][0])['checks']
        assert checks[0]['evidence_source'] == 'runtime_verification'
        assert checks[0]['runtime_command_ref'] == observed['checks'][0]['command_ref']
        assert result
        context_checks = machine.context(task.task_id)['nodes'][-1]['result']['checks']
        assert context_checks[0]['evidence_source'] == 'runtime_verification'
        assert context_checks[0]['runtime_command_ref'] == observed['checks'][0]['command_ref']


def test_in_flight_waits_without_reexecuting(tmp_path, monkeypatch):
    runtime, task, machine, _, reviewer, assignment, digest = fixture(tmp_path)
    started, release = threading.Event(), threading.Event()
    original = controller.subprocess.Popen
    def launch(*args, **kwargs):
        started.set()
        assert release.wait(3)
        return original(*args, **kwargs)
    monkeypatch.setattr(controller.subprocess, 'Popen', launch)
    candidate = controller._SpeculativeVerification(runtime, task.task_id, None)
    candidate.start(machine.context(task.task_id), assignment, digest)
    assert started.wait(3)
    monkeypatch.setattr(gateway, '_command', no_execution)
    timer = threading.Timer(.1, release.set)
    timer.start()
    try:
        receipt = command(runtime._store, reviewer)
        assert receipt['runtime_verification']['source'] == 'runtime'
    finally:
        release.set()
        timer.join()
        candidate.close()


@pytest.mark.parametrize('reason', ['digest', 'argv', 'worker', 'discarded', 'not_started', 'timeout', 'incomplete', 'output_shortened', 'stale_run'])
def test_fallback_executes_normally(tmp_path, monkeypatch, reason):
    runtime, task, machine, worker, reviewer, assignment, digest = fixture(tmp_path)
    if reason not in {'not_started', 'timeout'}:
        observed = observe(runtime, task, machine, digest)
    else:
        with runtime._store.connection() as db:
            event(db, task.task_id, 'CHECK_EXECUTION_STARTED', dict(
                command_ref='verification:pending', argv=ARGV, revision=1,
                revision_digest=digest, run_id=123))
            if reason == 'not_started':
                event(db, task.task_id, 'CHECK_EXECUTION_ABORTED', dict(run_id=123))
            db.commit()
    if reason == 'digest':
        (tmp_path / 'changed.py').write_text('changed')
    if reason == 'discarded':
        with runtime._store.connection() as db:
            event(db, task.task_id, 'SPECULATIVE_VERIFICATION_DISCARDED', dict(run_id=123))
            db.commit()
    if reason == 'incomplete':
        with runtime._store.connection() as db:
            db.execute("UPDATE large_outputs SET truncated=1 WHERE id=?",
                       (observed['checks'][0]['large_output_id'],))
            db.commit()
    if reason == 'output_shortened':
        with runtime._store.connection() as db:
            saved = db.execute('SELECT filename FROM large_outputs WHERE id=?',
                               (observed['checks'][0]['large_output_id'],)).fetchone()
        (tmp_path / '.cortex' / task.task_id / 'large_outputs' / saved[0]).write_bytes(b'')
    if reason == 'stale_run':
        with runtime._store.connection() as db:
            event(db, task.task_id, 'SPECULATIVE_VERIFICATION_FINISHED', dict(run_id=123, stale=True))
            db.commit()
    monkeypatch.setattr(reviewer_verification, 'WAIT_SECONDS', .1)
    calls = []
    def execute(*args):
        calls.append(args[4]['argv'])
        return dict(tool_call_ref=args[5], complete=True, exit_code=0, output='normal')
    monkeypatch.setattr(gateway, '_command', execute)
    argv = ARGV + ['different'] if reason == 'argv' else ARGV
    began = time.monotonic()
    result = command(runtime._store, worker if reason == 'worker' else reviewer, argv)
    assert calls == [argv]
    assert 'runtime_verification' not in result
    assert time.monotonic() - began < 1


def test_shutdown_interrupts_wait_without_launch(tmp_path, monkeypatch):
    runtime, task, machine, _, reviewer, assignment, digest = fixture(tmp_path)
    started, release = threading.Event(), threading.Event()
    original = controller.subprocess.Popen
    def launch(*args, **kwargs):
        started.set()
        assert release.wait(3)
        return original(*args, **kwargs)
    monkeypatch.setattr(controller.subprocess, 'Popen', launch)
    candidate = controller._SpeculativeVerification(runtime, task.task_id, None)
    candidate.start(machine.context(task.task_id), assignment, digest)
    assert started.wait(3)
    monkeypatch.setattr(gateway, '_command', no_execution)
    # The real broker serves MCP in a separate process: no shared Event/map.
    import os
    import sys
    from pathlib import Path
    marker_exits = []
    def shutdown_from_other_process():
        script = ("import sys; from pathlib import Path; from cortex_runtime.store import Store; "
                  "from cortex_runtime.orchestration.reviewer_verification import cancel_waits; "
                  "root=Path(sys.argv[1]); cancel_waits(Store(root/'.cortex', project_root=root, initialize=False), [sys.argv[2]])")
        env = dict(os.environ, PYTHONPATH=str(Path(controller.__file__).resolve().parents[2]),
                   PYTHONDONTWRITEBYTECODE='1')
        proc = original([sys.executable, '-B', '-c', script, str(tmp_path), task.task_id], env=env)
        marker_exits.append(proc.wait(timeout=3))
    timer = threading.Timer(.1, shutdown_from_other_process)
    timer.start()
    began = time.monotonic()
    try:
        with pytest.raises(StoreError, match='gateway_runtime_stopping'):
            command(runtime._store, reviewer)
        assert time.monotonic() - began < 1
    finally:
        release.set()
        timer.join()
        candidate.close()
    assert marker_exits == [0]


def test_reused_source_cannot_verify_two_checks(tmp_path):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    observe(runtime, task, machine, digest)
    first = command(runtime._store, reviewer, key='one')
    second = command(runtime._store, reviewer, key='two')
    with runtime._store.connection() as db:
        checks = evidence.assess_checks(db, task.task_id, reviewer.attempt_id, [
            dict(outcome='passed', tool_call_ref=r['tool_call_ref']) for r in (first, second)])
    assert checks[0]['strength'] == 'verified'
    assert checks[1]['gap'] == 'ref_already_cited'


@pytest.mark.parametrize('tamper', ['origin', 'finished_actor', 'start_missing', 'alias_size'])
def test_provenance_cannot_be_asserted_or_tampered(tmp_path, tamper):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    observed = observe(runtime, task, machine, digest)
    receipt = command(runtime._store, reviewer)
    with runtime._store.connection() as db:
        if tamper == 'origin':
            saved = db.execute("SELECT request_key,result_json FROM runtime_requests WHERE actor=? AND operation='command_run'",
                               (reviewer.channel_id,)).fetchone()
            value = json.loads(saved['result_json'])
            value['receipt']['runtime_verification']['source'] = 'worker'
            db.execute("UPDATE runtime_requests SET result_json=? WHERE actor=? AND request_key=?",
                       (encoded(value), reviewer.channel_id, saved['request_key']))
        elif tamper == 'finished_actor':
            db.execute("UPDATE runtime_events SET actor=? WHERE task_id=? AND type='CHECK_EXECUTION_FINISHED'",
                       (reviewer.channel_id, task.task_id))
        elif tamper == 'start_missing':
            db.execute("DELETE FROM runtime_events WHERE task_id=? AND type='CHECK_EXECUTION_STARTED'", (task.task_id,))
        else:
            db.execute("UPDATE large_outputs SET byte_count=byte_count+1 WHERE id=?", (receipt['large_output_id'],))
        db.commit()
        assessed = evidence.assess_check(db, task.task_id, reviewer.attempt_id, 'passed', receipt['tool_call_ref'])
    assert assessed['strength'] == 'weak'


def test_actual_launch_failure_falls_back(tmp_path, monkeypatch):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    def failed_launch(*args, **kwargs):
        raise OSError('fixture launch failed')
    monkeypatch.setattr(controller.subprocess, 'Popen', failed_launch)
    with pytest.raises(OSError, match='fixture launch failed'):
        observe(runtime, task, machine, digest)
    calls = []
    def normal(*args):
        calls.append(True)
        return dict(tool_call_ref=args[5], complete=True, exit_code=0)
    monkeypatch.setattr(gateway, '_command', normal)
    assert 'runtime_verification' not in command(runtime._store, reviewer)
    assert calls == [True]


def test_reuse_keeps_original_execution_order_and_replays_only_alias(tmp_path):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    observed = observe(runtime, task, machine, digest)
    first = command(runtime._store, reviewer)
    repeated = command(runtime._store, reviewer)
    assert repeated == first
    with runtime._store.connection() as db:
        history = evidence.command_history(db, task.task_id, reviewer.attempt_id)
        assert len(history) == 1
        assert history[0]['order'] == observed['checks'][0]['started_event_id']


def test_composed_runtime_reviewer_reuses_candidate_and_final_credit(tmp_path, monkeypatch):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.portable.engine_contract import StopReason
    from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='dedupe-composed', event_id='one'), 'Implement', 'owner')
    finished = threading.Event()
    original = controller._verification_observation
    observations, receipts = [], []
    def observe_check(*args):
        result = original(*args)
        observations.append(result)
        finished.set()
        return result
    monkeypatch.setattr(controller, '_verification_observation', observe_check)
    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            machine = TaskMachine(store)
            context = machine.context(task.task_id)
            if channel.role == 'coordinator':
                if context['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                        tasks=[dict(id='implement', role='backend-dev', objective='Implement',
                                    dependencies=[], scope=['.'], model='auto')],
                        verification=[ARGV]), request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Verified.'))
            else:
                with store.connection() as db:
                    stage = db.execute(
                        'SELECT n.stage FROM runtime_nodes n JOIN delegation_provisioning p '
                        'ON p.idempotency_key=n.assignment_ref WHERE p.attempt_id=?',
                        (channel.attempt_id,)).fetchone()[0]
                args = dict(report=publish_fixture_report(store, channel), summary='Observed',
                            checks=[], request_key='result')
                if stage == 'execution':
                    (tmp_path / 'feature.py').write_text('answer = 42\n')
                else:
                    assert finished.wait(5)
                    receipt = command(store, channel)
                    receipts.append(receipt)
                    assert receipt['runtime_verification']['source'] == 'runtime'
                    args.update(verdict='APPROVE', checks=[dict(
                        name='verification', outcome='passed', tool_call_ref=receipt['tool_call_ref'])])
                mcp_tools.dispatch(store, channel, 'result_submit', args)
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior
    engine = FakeSessionRuntime(store, router, connection_id='cn_dedupe', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        outcome = runtime.start_prompt(task.task_id, 'incoming', 'Implement', lambda _: True,
                                       threading.Event(), emit_tool_call=lambda _: True)
    finally:
        runtime.shutdown()
    assert outcome == StopReason.END_TURN
    assert TaskMachine(store).context(task.task_id)['state'] == 'COMPLETED'
    assert len(observations) == len(receipts) == 1
    with store.connection() as db:
        events = [row[0] for row in db.execute('SELECT type FROM runtime_events WHERE task_id=?', (task.task_id,))]
    assert events.count('CHECK_EXECUTION_STARTED') == 1
    assert events.count('VERIFICATION_PASSED') == 1
    assert 'SPECULATIVE_VERIFICATION_CREDITED' in events


def test_runtime_shutdown_marks_owned_tasks_before_engine_teardown(tmp_path):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace
    fixture_runtime, task, machine, _, _, _, digest = fixture(tmp_path)
    with fixture_runtime._store.connection() as db:
        event(db, task.task_id, 'CHECK_EXECUTION_STARTED', dict(
            command_ref='verification:pending', argv=ARGV, revision=1, revision_digest=digest, run_id=123))
        db.commit()
    class Engine:
        def shutdown(self):
            with fixture_runtime._store.connection() as db:
                assert db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND "
                                  "type='REVIEWER_VERIFICATION_WAIT_CANCELLED'", (task.task_id,)).fetchone()
    runtime = OrchestratingRuntime(fixture_runtime._store, Engine(), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    runtime._coordinator_sessions[task.task_id] = 'owned'
    runtime.shutdown()
    runtime.shutdown()
    with fixture_runtime._store.connection() as db:
        assert db.execute("SELECT count(*) FROM runtime_events WHERE task_id=? AND "
                          "type='REVIEWER_VERIFICATION_WAIT_CANCELLED'", (task.task_id,)).fetchone()[0] == 1


def test_cancellation_marker_does_not_affect_another_attempt(tmp_path):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    observe(runtime, task, machine, digest)
    with runtime._store.connection() as db:
        event(db, task.task_id, 'REVIEWER_VERIFICATION_WAIT_CANCELLED', dict(attempt_ids=['another-attempt']))
        db.commit()
    assert command(runtime._store, reviewer)['runtime_verification']['source'] == 'runtime'


def test_unicode_argv_is_matched_exactly(tmp_path):
    runtime, task, machine, _, reviewer, _, digest = fixture(tmp_path)
    argv = ['python3', '-B', '-c', 'assert "λ" == "λ"']
    with runtime._store.connection() as db:
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
                   (encoded(dict(verification=[argv])), task.task_id))
        db.commit()
    observe(runtime, task, machine, digest)
    assert command(runtime._store, reviewer, argv)['runtime_verification']['source'] == 'runtime'
