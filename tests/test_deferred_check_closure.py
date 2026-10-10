"""Runtime verification closes only exact, author-bound, current deferred commands."""
import json
import os
from types import SimpleNamespace
from threading import Event
from unittest.mock import patch

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.revision import capture_revision
from cortex_runtime.orchestration.controller import _verification_observation
from cortex_runtime.orchestration.recovery import reconcile_results
from cortex_runtime.orchestration.state import TaskMachine, event
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report

ARGV = ['python3', '-B', '-c', 'assert 2 + 2 == 4']


def setup(tmp_path, *, command=ARGV, outcome='not_run', verification_argv=ARGV):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='deferred', event_id='one'), 'Verify', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    state = TaskMachine(store)
    with store.connection() as db:
        db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?',
                   (json.dumps(dict(verification=[verification_argv])), task.task_id))
        db.commit()
    checks = [dict(name='documented tests', outcome=outcome, blocker='Reserved for runtime verification')]
    if command is not None:
        checks[0]['command'] = command
    return store, task, coordinator, worker, assignment, state, checks


def submit(store, worker, checks, *, interrupted=False):
    report = publish_fixture_report(store, worker)
    args = dict(report=report, summary='Deferred', checks=checks, request_key='result')
    if interrupted:
        with patch.object(TaskMachine, 'record_result', side_effect=RuntimeError('interrupted')):
            with pytest.raises(RuntimeError, match='interrupted'):
                mcp_tools.dispatch(store, worker, 'result_submit', args)
        reconcile_results(store, worker.task_id)
    else:
        mcp_tools.dispatch(store, worker, 'result_submit', args)
    return args


def verification(store, task, state):
    digest = capture_revision(store.project_root)['dirty_digest']
    with store.connection() as db:
        event(db, task.task_id, 'REVIEW_NOT_REQUIRED', dict(revision=1, revision_digest=digest))
        db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id=?", (task.task_id,))
        db.commit()
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda task_id: store.project_root)
    observed = _verification_observation(
        runtime, task.task_id, state.context(task.task_id), digest, None, Event(), 123456)
    assert observed['checks'][0]['exit_code'] == 0
    assert not observed['stale']
    return observed, digest


@pytest.mark.parametrize('case', [
    'missing', 'short', 'long', 'symlink', 'hardlink', 'directory', 'fifo',
    'unreadable', 'public_mode', 'directory_symlink', 'unsafe_filename',
])
def test_lost_or_unsafe_retained_body_reopens_gap_and_requires_disclosure(tmp_path, case):
    argv = ['python3', '-B', '-c', 'print("complete retained output")']
    store, task, coordinator, worker, assignment, state, checks = setup(
        tmp_path, command=argv, verification_argv=argv)
    submit(store, worker, checks)
    observed, digest = verification(store, task, state)
    assert state.record_verification(task.task_id, observed['checks'], digest)
    assert 'closed_by' in node(state, task, assignment)['result']['checks'][0]
    with store.connection() as db:
        output = db.execute('SELECT * FROM large_outputs WHERE id=?',
                            (observed['checks'][0]['large_output_id'],)).fetchone()
        body = store._task_directory(db, task.task_id) / 'large_outputs' / output['filename']
    content = body.read_bytes()
    assert content
    if case == 'missing':
        body.unlink()
    elif case == 'short':
        body.write_bytes(content[:-1])
    elif case == 'long':
        body.write_bytes(content + b'x')
    elif case == 'symlink':
        target = body.with_suffix('.target')
        body.rename(target)
        body.symlink_to(target)
    elif case == 'hardlink':
        os.link(body, body.with_suffix('.link'))
    elif case == 'directory':
        body.unlink()
        body.mkdir(mode=0o700)
    elif case == 'fifo':
        body.unlink()
        os.mkfifo(body, 0o600)
    elif case == 'unreadable':
        body.chmod(0)
    elif case == 'public_mode':
        body.chmod(0o644)
    elif case == 'directory_symlink':
        directory = body.parent
        target = directory.with_name('moved_outputs')
        directory.rename(target)
        directory.symlink_to(target, target_is_directory=True)
    else:
        with store.connection() as db:
            db.execute('UPDATE large_outputs SET filename=? WHERE id=?',
                       ('../' + output['filename'], output['id']))
            db.commit()
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (task.task_id,))
        db.commit()
    try:
        recovered = TaskMachine(Store(tmp_path / '.cortex', project_root=tmp_path))
        assert 'closed_by' not in node(recovered, task, assignment)['result']['checks'][0]
        with pytest.raises(StoreError, match='synthesis_gaps_undisclosed'):
            recovered.submit_synthesis(coordinator, 'Verified')
        assert recovered.submit_synthesis(coordinator, 'Retained evidence unavailable',
            not_verified=[dict(item='documented tests', blocker='Retained body unavailable')])['recorded']
        # Validation does not recreate a missing file or repair permissions.
        if case == 'missing':
            assert not body.exists()
        elif case == 'unreadable':
            assert body.stat().st_mode & 0o777 == 0
    finally:
        if case == 'unreadable':
            body.chmod(0o600)


@pytest.mark.parametrize('case', ['read_error', 'short_read', 'replaced_during_read'])
def test_retained_body_read_failure_or_race_is_not_complete(tmp_path, case):
    from cortex_runtime.orchestration.check_closure import _retained_output_complete

    argv = ['python3', '-B', '-c', 'print("output")']
    store, task, coordinator, worker, assignment, state, checks = setup(
        tmp_path, command=argv, verification_argv=argv)
    submit(store, worker, checks)
    observed, digest = verification(store, task, state)
    with store.connection() as db:
        output = db.execute('SELECT * FROM large_outputs WHERE id=?',
                            (observed['checks'][0]['large_output_id'],)).fetchone()
        body = store._task_directory(db, task.task_id) / 'large_outputs' / output['filename']
        real_read = os.read

        def read(descriptor, count):
            if case == 'read_error':
                raise OSError('unreadable output')
            if case == 'short_read':
                return b''
            content = real_read(descriptor, count)
            if body.exists():
                replacement = body.with_suffix('.replacement')
                replacement.write_bytes(body.read_bytes())
                replacement.chmod(0o600)
                replacement.replace(body)
            return content

        with patch('cortex_runtime.orchestration.check_closure.os.read', side_effect=read):
            assert not _retained_output_complete(db, task.task_id, output)


def node(state, task, assignment):
    return next(n for n in state.context(task.task_id)['nodes'] if n['assignment_ref'] == assignment)


@pytest.mark.parametrize('outcome', ['not_run', 'unverified'])
def test_exact_runtime_command_closes_durably_and_synthesis_needs_no_disclosure(tmp_path, outcome):
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path, outcome=outcome)
    args = submit(store, worker, checks)
    observed, digest = verification(store, task, state)
    assert state.record_verification(task.task_id, observed['checks'], digest)
    ref = observed['checks'][0]['command_ref']
    retained = node(state, task, assignment)['result']['checks'][0]
    assert retained['closed_by'] == ref
    assert retained['outcome'] == outcome
    assert retained['blocker'] == checks[0]['blocker']
    assert retained['evidence_strength'] == 'none'  # Not a fabricated worker pass.
    with store.connection() as db:
        saved = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                     (assignment,)).fetchone()[0])
        assert saved['checks'][0]['closed_by'] == ref
        publication = json.loads(db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task.task_id,)).fetchone()[0])
        assert publication['checks'][0]['command'] == ARGV
        assert 'closed_by' not in publication['checks'][0]
    # Exact keyed result replay is unchanged even after derived closure.
    assert mcp_tools.dispatch(store, worker, 'result_submit', args)['recorded']
    assert mcp_tools.dispatch(store, coordinator, 'synthesis_submit', dict(text='Verified'))['recorded']
    with store.connection() as db:
        assert db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?',
                          (task.task_id,)).fetchone()[0] == 'Verified'


@pytest.mark.parametrize('case', [
    'different_argv', 'whitespace_argv', 'no_command', 'failed', 'incomplete',
    'truncated', 'output_failed', 'output_missing', 'foreign_output', 'stale_revision',
    'old_plan', 'model_event', 'model_start', 'missing_start', 'other_run_digest',
    'mismatched_start', 'duplicate_start', 'duplicate_output', 'later_failure',
    'unrelated_node', 'forged_closed_by', 'other_author_digest',
    'missing_author_digest', 'unavailable_author_digest', 'duplicate_author_digest',
])
def test_negative_evidence_keeps_gap_open(tmp_path, case):
    command = (['python3', '-B', '-c', 'assert 2 + 2 == 5'] if case == 'different_argv'
               else ['python3', '-B', '-c', 'assert 2 + 2 == 4 '] if case == 'whitespace_argv'
               else None if case == 'no_command' else ARGV)
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path, command=command)
    submit(store, worker, checks)
    if case == 'other_author_digest':
        (tmp_path / 'new_revision.py').write_text('changed = True\n')
    observed, digest = verification(store, task, state)
    check = observed['checks'][0]
    if case in {'failed', 'incomplete', 'old_plan', 'missing_start', 'mismatched_start', 'other_run_digest'}:
        if case == 'failed':
            check['exit_code'] = 1
        elif case == 'incomplete':
            check['complete'] = False
        elif case == 'old_plan':
            check['command_ref'] = 'verification:0:123456:0'
        elif case == 'missing_start':
            check.pop('started_event_id')
        elif case == 'other_run_digest':
            check['revision_digest'] = '0' * 64
        else:
            check['started_event_id'] += 1
    with store.connection() as db:
        if case == 'missing_author_digest':
            db.execute("DELETE FROM attempt_revisions WHERE phase LIKE 'worker_publish:%'")
        elif case == 'unavailable_author_digest':
            db.execute("UPDATE attempt_revisions SET kind='unavailable' WHERE phase LIKE 'worker_publish:%'")
        elif case == 'duplicate_author_digest':
            db.execute("INSERT INTO attempt_revisions SELECT * FROM attempt_revisions WHERE phase LIKE 'worker_publish:%'")
        elif case == 'truncated':
            db.execute('UPDATE large_outputs SET truncated=1 WHERE id=?', (check['large_output_id'],))
        elif case == 'output_failed':
            db.execute('UPDATE large_outputs SET exit_code=1 WHERE id=?', (check['large_output_id'],))
        elif case == 'output_missing':
            db.execute('DELETE FROM large_outputs WHERE id=?', (check['large_output_id'],))
        elif case == 'foreign_output':
            db.execute("UPDATE large_outputs SET owner_scope=? WHERE id=?", (assignment, check['large_output_id']))
        elif case == 'model_start':
            db.execute("UPDATE runtime_events SET actor='model' WHERE id=?", (check['started_event_id'],))
        elif case == 'duplicate_start':
            row = db.execute('SELECT payload_json FROM runtime_events WHERE id=?', (check['started_event_id'],)).fetchone()
            event(db, task.task_id, 'CHECK_EXECUTION_STARTED', json.loads(row[0]))
        elif case == 'duplicate_output':
            db.execute("INSERT INTO large_outputs SELECT 'lo_duplicate',task_id,owner_scope,source_kind,source_ref,"
                       "filename,byte_count,truncated,exit_code,signal,failure,created_at FROM large_outputs WHERE id=?",
                       (check['large_output_id'],))
        db.commit()
    state.record_verification(task.task_id, observed['checks'], digest)
    with store.connection() as db:
        if case == 'model_event':
            db.execute("UPDATE runtime_events SET actor='model' WHERE task_id=? AND type='VERIFICATION_PASSED'",
                       (task.task_id,))
        elif case == 'later_failure':
            event(db, task.task_id, 'VERIFICATION_FAILED', dict(revision=1, revision_digest=digest, checks=[]))
        elif case == 'unrelated_node':
            # A same-command disclosure grafted onto a different assignment has
            # no matching runtime completion event and cannot inherit its evidence.
            db.execute("UPDATE runtime_nodes SET assignment_ref='unrelated' WHERE assignment_ref=?", (assignment,))
            assignment = 'unrelated'
        elif case == 'forged_closed_by':
            db.execute("DELETE FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", (task.task_id,))
            result = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                          (assignment,)).fetchone()[0])
            result['checks'][0]['closed_by'] = check['command_ref']
            db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?', (json.dumps(result), assignment))
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (task.task_id,))
        db.commit()
    if case == 'stale_revision':
        (tmp_path / 'changed.py').write_text('changed = True\n')
    retained = node(state, task, assignment)['result']['checks'][0]
    assert 'closed_by' not in retained
    with pytest.raises(StoreError, match='synthesis_gaps_undisclosed'):
        mcp_tools.dispatch(store, coordinator, 'synthesis_submit', dict(text='Verified'))
    assert mcp_tools.dispatch(store, coordinator, 'synthesis_submit', dict(
        text='Gap disclosed', not_verified=[dict(item='documented tests', blocker='Exact evidence unavailable')]))['recorded']


def test_recovery_and_replay_rederive_closure_not_stored_reference(tmp_path):
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path)
    submit(store, worker, checks, interrupted=True)
    observed, digest = verification(store, task, state)
    assert state.record_verification(task.task_id, observed['checks'], digest)
    with store.connection() as db:
        saved = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                     (assignment,)).fetchone()[0])
        saved['checks'][0]['closed_by'] = 'verification:forged'
        db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?', (json.dumps(saved), assignment))
        request = db.execute("SELECT result_json FROM runtime_requests WHERE task_id=? AND operation='result_submit'",
                             (task.task_id,)).fetchone()
        pending = json.loads(request[0])
        pending['complete'] = False  # Delivery was lost after the node committed.
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND operation='result_submit'",
                   (json.dumps(pending), task.task_id))
        db.commit()
    state = TaskMachine(Store(tmp_path / '.cortex', project_root=tmp_path))
    assert node(state, task, assignment)['result']['checks'][0]['closed_by'] == observed['checks'][0]['command_ref']
    reconcile_results(state.store, task.task_id)
    assert state.submit_synthesis(coordinator, 'Verified after recovery')['recorded']
    with store.connection() as db:
        retained = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                        (assignment,)).fetchone()[0])
        assert retained['checks'][0]['closed_by'] == observed['checks'][0]['command_ref']


@pytest.mark.parametrize('speculative', [False, True])
def test_review_disclosure_closes_on_same_digest_regardless_of_execution_order(tmp_path, speculative):
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path)
    submit(store, worker, checks)
    reviewer, review_assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_assignment,))
        db.commit()
    state.start_review(task.task_id, review_assignment)
    if speculative:
        observed, digest = verification(store, task, state)
    report = publish_fixture_report(store, reviewer)
    mcp_tools.dispatch(store, reviewer, 'result_submit', dict(
        report=report, summary='Independent review', verdict='APPROVE', checks=checks, request_key='review-result'))
    if not speculative:
        observed, digest = verification(store, task, state)
    assert state.record_verification(task.task_id, observed['checks'], digest, speculative_binding=(
        dict(revision=1, assignment_ref=review_assignment, digest=digest) if speculative else None))
    assert node(state, task, assignment)['result']['checks'][0]['closed_by'] == observed['checks'][0]['command_ref']
    review_check = node(state, task, review_assignment)['result']['checks'][0]
    assert review_check['closed_by'] == observed['checks'][0]['command_ref']
    assert state.submit_synthesis(coordinator, 'Verified')['recorded']


@pytest.mark.parametrize('scenario', ['approve', 'discarded', 'stale', 'changed_after_credit'])
def test_speculative_execution_closes_gap_only_after_authoritative_credit(tmp_path, scenario):
    from cortex_runtime.orchestration.controller import _SpeculativeVerification

    # Exact repilot command: it completes on D before the reviewer records its gap.
    argv = ['python3', '-m', 'unittest', 'discover', '-s', 'tests', '-v']
    (tmp_path / 'tests').mkdir()
    (tmp_path / 'tests' / 'test_example.py').write_text(
        'import unittest\nclass Example(unittest.TestCase):\n    def test_example(self):\n        self.assertEqual(2 + 2, 4)\n')
    store, task, coordinator, worker, assignment, state, checks = setup(
        tmp_path, command=argv, verification_argv=argv)
    submit(store, worker, checks)
    reviewer, review_assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_assignment,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    digest = state.start_review(task.task_id, review_assignment)
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: store.project_root)
    candidate = _SpeculativeVerification(runtime, task.task_id, None)
    try:
        candidate.start(state.context(task.task_id), review_assignment, digest)
        candidate.thread.join(15)
        assert not candidate.thread.is_alive()
        assert candidate.observed['checks'][0]['complete'] is True
        ref = candidate.observed['checks'][0]['command_ref']
        with store.connection() as db:
            events = [row[0] for row in db.execute(
                'SELECT type FROM runtime_events WHERE task_id=?', (task.task_id,))]
        assert 'CHECK_EXECUTION_STARTED' in events
        assert not any(kind.startswith('VERIFICATION_') for kind in events)
        assert 'closed_by' not in node(state, task, assignment)['result']['checks'][0]

        if scenario == 'discarded':
            candidate.close('controller_turn_ended')
        elif scenario == 'stale':
            (tmp_path / 'late.py').write_text('changed = True\n')
        report = publish_fixture_report(store, reviewer)
        args = dict(report=report, summary='Independent review', verdict='APPROVE',
                    checks=checks, request_key='review-result')
        if scenario == 'stale':
            with pytest.raises(StoreError, match='review_project_revision_changed'):
                mcp_tools.dispatch(store, reviewer, 'result_submit', args)
            args['request_key'] = 'rechecked-review'
        mcp_tools.dispatch(store, reviewer, 'result_submit', args)
        state.move(task.task_id, 'VERIFYING')
        assert candidate.credit(state.context(task.task_id), state) is (scenario in {'approve', 'changed_after_credit'})
        if scenario == 'changed_after_credit':
            (tmp_path / 'after_credit.py').write_text('changed = True\n')
        retained = node(state, task, assignment)['result']['checks'][0]
        assert retained['outcome'] == 'not_run'
        if scenario == 'approve':
            assert retained['closed_by'] == ref
            assert node(state, task, review_assignment)['result']['checks'][0]['closed_by'] == ref
            recovered = TaskMachine(Store(tmp_path / '.cortex', project_root=tmp_path))
            assert node(recovered, task, review_assignment)['result']['checks'][0]['closed_by'] == ref
            assert recovered.submit_synthesis(coordinator, 'Verified')['recorded']
        elif scenario == 'changed_after_credit':
            assert 'closed_by' not in retained
            assert 'closed_by' not in node(state, task, review_assignment)['result']['checks'][0]
            with pytest.raises(StoreError, match='synthesis_gaps_undisclosed'):
                state.submit_synthesis(coordinator, 'Verified')
        else:
            assert 'closed_by' not in retained
            assert 'closed_by' not in node(state, task, review_assignment)['result']['checks'][0]
            with store.connection() as db:
                assert not db.execute(
                    "SELECT 1 FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'",
                    (task.task_id,)).fetchone()
    finally:
        candidate.close()


@pytest.mark.parametrize('outcome', ['not_run', 'unverified'])
def test_execution_before_publication_closes_on_same_author_digest(tmp_path, outcome):
    # Result publication protects the runtime store; seed that rule before D.
    (tmp_path / '.gitignore').write_text('/.cortex/\n')
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path, outcome=outcome)
    observed, digest = verification(store, task, state)
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id=?", (task.task_id,))
        db.commit()
    submit(store, worker, checks)
    state.move(task.task_id, 'VERIFYING')
    assert state.record_verification(task.task_id, observed['checks'], digest)
    assert node(state, task, assignment)['result']['checks'][0]['closed_by'] == observed['checks'][0]['command_ref']
    assert state.submit_synthesis(coordinator, 'Verified')['recorded']


@pytest.mark.parametrize('author_digest', [None, '0' * 64])
def test_review_gap_without_matching_author_binding_stays_open(tmp_path, author_digest):
    store, task, coordinator, worker, assignment, state, checks = setup(tmp_path)
    submit(store, worker, checks)
    reviewer, review_assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_assignment,))
        db.commit()
    state.start_review(task.task_id, review_assignment)
    report = publish_fixture_report(store, reviewer)
    mcp_tools.dispatch(store, reviewer, 'result_submit', dict(
        report=report, summary='Independent review', verdict='APPROVE', checks=checks, request_key='review-result'))
    observed, digest = verification(store, task, state)
    assert state.record_verification(task.task_id, observed['checks'], digest)
    with store.connection() as db:
        row = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_COMPLETED' "
                         "AND json_extract(payload_json,'$.assignment_ref')=?", (task.task_id, review_assignment)).fetchone()
        completion = json.loads(row['payload_json'])
        completion['result']['project_digest'] = author_digest
        db.execute('UPDATE runtime_events SET payload_json=? WHERE id=?', (json.dumps(completion), row['id']))
        db.commit()
    assert 'closed_by' not in node(state, task, review_assignment)['result']['checks'][0]
    with pytest.raises(StoreError, match='synthesis_gaps_undisclosed'):
        state.submit_synthesis(coordinator, 'Verified')
