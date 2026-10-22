"""Fully closed synthesis uses observed receipts, never a model acceptance."""
import json
import os
import sqlite3
import subprocess
import sys
import time
import threading
from contextlib import contextmanager
from types import SimpleNamespace
from pathlib import Path

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools, revision
from cortex_runtime.orchestration import controller, synthesis
from cortex_runtime.orchestration.state import TaskMachine, event, _record_execution_baseline
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from runtime_scenarios import provision_fixture, publish_fixture_report

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')
ARGV = ['python3', '-B', '-c', 'print("observed verification")']


def submit(store, worker, *, checks=(), verdict=None,
           summary='Submitted summary <script> ```\n[link](unsafe)'):
    args = dict(report=publish_fixture_report(store, worker, 'REPORT_BODY_MUST_NOT_APPEAR'),
                summary=summary,
                checks=list(checks), request_key='result')
    if verdict:
        args['verdict'] = verdict
    return mcp_tools.dispatch(store, worker, 'result_submit', args)


@pytest.fixture
def ready(tmp_path, request):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='synthesis', event_id='one'), 'Implement', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    machine = TaskMachine(store)
    with store.connection() as db:
        node = db.execute('SELECT node_id FROM runtime_nodes WHERE assignment_ref=?', (assignment,)).fetchone()[0]
        plan = dict(tasks=[dict(id=node)], verification=[ARGV])
        db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?', (json.dumps(plan), task.task_id))
        _record_execution_baseline(db, task.task_id)
        db.commit()
    (tmp_path / 'feature.py').write_text('FILE_CONTENT_MUST_NOT_APPEAR\n')
    parameter = getattr(request, 'param', False)
    check_argv = ARGV
    failed_check = None
    if parameter == 'closed_failure':
        check_argv = ['python3', '-B', '-c',
                      "from pathlib import Path; import sys; p=Path('check.marker'); existed=p.exists(); p.touch(); sys.exit(0 if existed else 1)"]
        failed_check = mcp_tools.dispatch(store, worker, 'command_run',
                                         dict(argv=check_argv, request_key='worker-check-failure'))
        assert failed_check['exit_code'] == 1
    check = mcp_tools.dispatch(store, worker, 'command_run',
                              dict(argv=check_argv, request_key='worker-check'))
    worker_checks = ([dict(name='scoped test', outcome='not_run', command=ARGV,
                           blocker='Deferred to digest-bound runtime verification')]
                     if parameter is True else
                     [dict(name='scoped test', outcome='passed', tool_call_ref=check['tool_call_ref'])])
    if failed_check:
        worker_checks.insert(0, dict(name='scoped test', outcome='failed',
                                    tool_call_ref=failed_check['tool_call_ref']))
    submit(store, worker, checks=worker_checks,
           summary='Enable the feature in Settings to use it')
    reviewer, review_ref = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_ref,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    digest = machine.start_review(task.task_id, review_ref)
    submit(store, reviewer, verdict='APPROVE')
    machine.move(task.task_id, 'VERIFYING')
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: str(tmp_path))
    observation = controller._verification_observation(
        runtime, task.task_id, machine.context(task.task_id), digest, None, threading.Event(), 12345)
    assert machine.record_verification(task.task_id, observation['checks'], digest)
    return machine, task, assignment, review_ref, check, observation


def kinds(machine, task):
    with machine.store.connection() as db:
        return [r[0] for r in db.execute('SELECT type FROM runtime_events WHERE task_id=? ORDER BY id',
                                       (task.task_id,))]


def test_plain_language_completion_and_replay(ready):
    machine, task, _, _, _, _ = ready
    with machine.store.connection() as db:
        db.execute('BEGIN')
        row = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task.task_id,)).fetchone()
        en = synthesis.compose(db, row, 'en')
        ru = synthesis.compose(db, row, 'ru')
        assert synthesis.compose(db, row, 'en') == en
    assert en.startswith('# Done')
    assert ru.startswith('# Готово')
    for text in (en, ru):
        assert 'feature.py' in text
        assert '> Enable the feature in Settings to use it' in text
        assert 'The requested work is done' in text or 'Работа завершена' in text
        assert 'review' not in text.lower() and 'check' not in text.lower() and 'провер' not in text
        assert 'tool_call_ref' not in text and 'command_ref' not in text
        assert 'project_digest' not in text and 'receipt' not in text
        assert 'APPROVE' not in text and 'Submitted summary' not in text
        assert '<script>' not in text
        assert 'REPORT_BODY_MUST_NOT_APPEAR' not in text
        assert 'FILE_CONTENT_MUST_NOT_APPEAR' not in text
        assert 'observed verification\n' not in text  # no terminal output body
        assert 'worker-backend-dev' not in text and 'worker-code-reviewer' not in text
        assert 'report_ref' not in text and 'verification' not in text
    first = synthesis.complete(machine, task.task_id, 'ru', lambda db: True)
    assert first == ru
    # A recovered process reads the committed result; it never recomposes it.
    recovered = TaskMachine(Store(machine.store.directory, project_root=machine.store.project_root))
    assert synthesis.complete(recovered, task.task_id, 'en', lambda db: True) is None
    assert recovered.context(task.task_id)['synthesis'] == ru
    assert kinds(machine, task).count('SYNTHESIS_RUNTIME_COMPOSED') == 1
    assert machine.get_state(task.task_id) == 'COMPLETED'
    coordinator = resolve_channel(machine.store, task.channel_id, task.secret)
    engine = FakeSessionRuntime(machine.store, lambda *args: pytest.fail('recovery prompted a model'),
                                connection_id='cn_recovered')
    runtime = OrchestratingRuntime(machine.store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    visible = []
    try:
        assert controller.run(runtime, task.task_id, 'incoming', coordinator, None, '', visible.append,
                              threading.Event(), 'ru') == StopReason.END_TURN
    finally:
        runtime.shutdown()
    # Own paragraph after any streamed coordinator prose.
    assert visible.count('\n\n' + ru) == 1
    assert '> Enable the feature in Settings to use it' in ''.join(visible)
    assert kinds(machine, task).count('SYNTHESIS_RUNTIME_COMPOSED') == 1


@pytest.mark.parametrize('language', ['en', 'ru'])
@pytest.mark.parametrize('path', [
    'notes`\n\n# FORGED HEADING\n\n`x.txt',
    'tick`name.txt', 'line\nname.txt', 'tab\tname.txt',
    'bidi\u202ename.txt', 'line\u2028name.txt', 'paragraph\u2029name.txt',
    'carriage\rname.txt',
])
def test_unsafe_filename_keeps_coordinator_reconciliation(ready, monkeypatch, language, path):
    machine, task, *_ = ready
    # Exercise the rendering boundary after the real evidence gates; path_metadata
    # intentionally preserves these valid POSIX names.
    monkeypatch.setattr(synthesis, 'path_metadata', lambda observed: {path: dict(kind='f')})
    assert synthesis.complete(machine, task.task_id, language, lambda db: True) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)


@pytest.mark.parametrize('language', ['en', 'ru'])
def test_filename_markdown_punctuation_stays_in_code(ready, monkeypatch, language):
    machine, task, *_ = ready
    path = '[notes](https://invalid.example)/<b>*_#.txt'
    monkeypatch.setattr(synthesis, 'path_metadata', lambda observed: {path: dict(kind='f')})
    with machine.store.connection() as db:
        row = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task.task_id,)).fetchone()
        text = synthesis.compose(db, row, language)
    assert '`' + path + '`' in text


@pytest.mark.parametrize('summary', [None, '', ' ', 'x' * 1601, 'bad\x00text', 'bad\u202etext'])
def test_missing_or_long_outcome_keeps_coordinator_reconciliation(ready, summary):
    machine, task, assignment, *_ = ready
    with machine.store.connection() as db:
        result = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                      (assignment,)).fetchone()[0])
        result['summary'] = summary
        db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?',
                   (json.dumps(result), assignment))
        db.commit()
    assert synthesis.complete(machine, task.task_id, 'en', lambda db: True) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None


@pytest.mark.parametrize('language', ['en', 'ru'])
def test_outcome_prose_cannot_add_markdown_or_html(ready, language):
    machine, task, assignment, *_ = ready
    with machine.store.connection() as db:
        result = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                      (assignment,)).fetchone()[0])
        result['summary'] = 'Use <script>\n\n# Heading [link](https://invalid.example) `code`'
        db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?',
                   (json.dumps(result), assignment))
        row = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task.task_id,)).fetchone()
        text = synthesis.compose(db, row, language)
    assert r'> Use \<script\> \# Heading \[link\]\(https\:\/\/invalid\.example\) \`code\`' in text
    assert '\n\n# Heading' not in text
    assert '<script>' not in text


def test_all_execution_outcomes_survive_without_review_prose(ready, monkeypatch):
    machine, task, *_ = ready
    original = synthesis.MAX_OUTCOME_CHARS
    with machine.store.connection() as db:
        row = dict(db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task.task_id,)).fetchone())
        nodes = [dict(n) for n in db.execute('SELECT * FROM runtime_nodes WHERE task_id=? ORDER BY rowid',
                                           (task.task_id,))]
        extra = dict(nodes[0])
        extra.update(node_id='extra', result_json=json.dumps(dict(summary='Keep the old setting off')))
        nodes.append(extra)
        plan = json.loads(row['plan_json'])
        plan['tasks'].append(dict(id='extra'))
        row['plan_json'] = json.dumps(plan)

        class WithExtraOutcome:
            def execute(self, sql, args=()):
                if sql.startswith('SELECT * FROM runtime_nodes'):
                    return SimpleNamespace(fetchall=lambda: nodes)
                return db.execute(sql, args)

        text = synthesis.compose(WithExtraOutcome(), row, 'en')
        assert '> Enable the feature in Settings to use it' in text
        assert '> Keep the old setting off' in text
        assert 'Submitted summary' not in text  # review-only prose
        monkeypatch.setattr(synthesis, 'MAX_OUTCOME_CHARS', len('Keep the old setting off'))
        assert synthesis.compose(WithExtraOutcome(), row, 'en') is None
        monkeypatch.setattr(synthesis, 'MAX_OUTCOME_CHARS', original)


@pytest.mark.parametrize('delivery', ['streamed', 'delayed', 'new_tool', 'final_only', 'no_callbacks',
                                      'rejected_then_retry', 'disclosure', 'disclosure_retry'])
def test_committed_synthesis_is_the_only_final_response(ready, monkeypatch, delivery):
    machine, task, *_ = ready
    store = machine.store
    monkeypatch.setattr(synthesis, 'complete', lambda *args: None)
    details = 'Evidence inspected before submission.'
    final = 'Implemented, reviewed and verified.'
    rewrite = 'A differently worded recap of the completed work.'
    correction = 'The rejected submission needs correction.'

    def router(_store, channel, _ref, _launch, _instructions):
        def behavior(_store, _channel, _ref, _message, _index, *,
                     on_delta=None, on_tool_call=None, cancel_event=None):
            if delivery != 'final_only':
                on_delta(details)
            if delivery == 'streamed':
                on_tool_call('synthesis', 'other', 'synthesis_submit', 'in_progress')
            if delivery == 'rejected_then_retry':
                with pytest.raises(StoreError, match='synthesis_blocker_required'):
                    mcp_tools.dispatch(store, channel, 'synthesis_submit',
                                       dict(text=final, not_verified=[dict(item='Manual check', blocker=' ')]))
                on_tool_call('rejected', 'other', 'synthesis_submit', 'failed')
                on_delta(correction)
                assert machine.context(task.task_id)['synthesis'] is None
            args = dict(text=final)
            if delivery in {'disclosure', 'disclosure_retry'}:
                args['not_verified'] = [dict(item='Manual check', blocker='Unavailable device')]
            if delivery == 'disclosure_retry':
                with pytest.raises(StoreError, match='synthesis_disclosure_in_text'):
                    mcp_tools.dispatch(store, channel, 'synthesis_submit',
                                       dict(args, text=final + '\n\n### Что не проверено\n- Manual check.'))
                assert machine.context(task.task_id)['synthesis'] is None
                on_tool_call('duplicate', 'other', 'synthesis_submit', 'failed')
            mcp_tools.dispatch(store, channel, 'synthesis_submit', args)
            if delivery == 'rejected_then_retry':
                mcp_tools.dispatch(store, channel, 'synthesis_submit', args)
            if delivery == 'streamed':
                on_tool_call('synthesis', None, None, 'completed')
            if delivery != 'final_only':
                on_delta(rewrite)
            if delivery in {'delayed', 'new_tool'}:
                identity = 'synthesis' if delivery == 'delayed' else 'unrelated'
                on_tool_call(identity, 'other', 'Tool call', 'in_progress')
                on_delta(rewrite)
            return PromptOutcome(StopReason.END_TURN, rewrite)
        return behavior

    engine = FakeSessionRuntime(store, router, emit_outcome_text_as_delta=False)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    visible = []
    try:
        channel, session_ref = runtime._ensure_coordinator(task.task_id)
        result = controller.run(runtime, task.task_id, 'incoming', channel, session_ref, '',
                                visible.append, threading.Event(), 'en')
    finally:
        runtime.shutdown()
    shown = ''.join(visible)
    assert result == StopReason.END_TURN
    assert machine.get_state(task.task_id) == 'COMPLETED'
    assert shown.count(final) == 1
    assert rewrite not in shown
    assert shown.count(details) == (delivery != 'final_only')
    assert shown.count(correction) == (delivery == 'rejected_then_retry')
    assert shown.count('Unavailable device') == (delivery in {'disclosure', 'disclosure_retry'})
    assert 'Что не проверено' not in shown
    assert kinds(machine, task).count('SYNTHESIS_SUBMITTED') == 1


CASES = [
    'incomplete_node', 'missing_result', 'no_review', 'reject_review', 'stale_review',
    'verification_missing', 'verification_failed', 'verification_stale', 'verification_incomplete',
    'verification_truncated', 'verification_no_start', 'verification_duplicate_start',
    'verification_no_output', 'verification_output_truncated', 'verification_missing_body',
    'verification_changed_argv', 'unverified_receipt', 'not_run', 'unverified', 'failed',
    'review_cycle', 'verification_repair', 'replan_history', 'pending_steering',
    'user_message', 'claimed_user_message', 'question', 'permission', 'access', 'proposal',
    'runtime_proposal', 'ask_user_required', 'stopped', 'blocker', 'changed_project', 'changed_report',
    'baseline_missing', 'existing_synthesis', 'oversized', 'live_workers',
]


@pytest.mark.parametrize('case', CASES)
def test_each_ineligible_condition_falls_back_without_mutation(ready, case, monkeypatch):
    machine, task, assignment, review_ref, receipt, observation = ready
    store = machine.store
    with store.connection() as db:
        if case == 'incomplete_node':
            db.execute("UPDATE runtime_nodes SET status='failed' WHERE assignment_ref=?", (assignment,))
        elif case == 'missing_result':
            db.execute("UPDATE runtime_nodes SET result_json=NULL WHERE assignment_ref=?", (assignment,))
        elif case in {'no_review', 'reject_review', 'stale_review'}:
            if case == 'no_review':
                db.execute("DELETE FROM runtime_nodes WHERE assignment_ref=?", (review_ref,))
            else:
                row = db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?', (review_ref,)).fetchone()
                result = json.loads(row[0])
                result['verdict' if case == 'reject_review' else 'project_digest'] = ('REQUEST_CHANGES' if case == 'reject_review' else '0' * 64)
                db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?', (json.dumps(result), review_ref))
        elif case.startswith('verification_') and case != 'verification_repair':
            row = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", (task.task_id,)).fetchone()
            payload = json.loads(row['payload_json'])
            check = payload['checks'][0]
            if case == 'verification_missing':
                db.execute('DELETE FROM runtime_events WHERE id=?', (row['id'],))
            elif case == 'verification_failed':
                event(db, task.task_id, 'VERIFICATION_FAILED', payload)
            elif case == 'verification_no_start':
                db.execute("DELETE FROM runtime_events WHERE type='CHECK_EXECUTION_STARTED'")
            elif case == 'verification_duplicate_start':
                start = db.execute("SELECT payload_json FROM runtime_events WHERE type='CHECK_EXECUTION_STARTED'").fetchone()
                event(db, task.task_id, 'CHECK_EXECUTION_STARTED', json.loads(start[0]))
            elif case == 'verification_no_output':
                db.execute('DELETE FROM large_outputs WHERE id=?', (check['large_output_id'],))
            elif case == 'verification_output_truncated':
                db.execute('UPDATE large_outputs SET truncated=1 WHERE id=?', (check['large_output_id'],))
            elif case == 'verification_missing_body':
                output = db.execute('SELECT filename FROM large_outputs WHERE id=?', (check['large_output_id'],)).fetchone()
                (store._task_directory(db, task.task_id) / 'large_outputs' / output[0]).unlink()
            else:
                if case == 'verification_stale':
                    payload['revision_digest'] = '0' * 64
                elif case == 'verification_changed_argv':
                    check['argv'] = ['different']
                elif case == 'verification_incomplete':
                    check['complete'] = False
                elif case == 'verification_truncated':
                    check['truncated'] = True
                db.execute('UPDATE runtime_events SET payload_json=? WHERE id=?', (json.dumps(payload), row['id']))
        elif case in {'not_run', 'unverified', 'failed', 'unverified_receipt'}:
            result = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?', (assignment,)).fetchone()[0])
            if case == 'unverified_receipt':
                result['checks'][0]['tool_call_ref'] = 'gateway:' + '0' * 32
            else:
                result['checks'] = [dict(name='open check', outcome=case, blocker='Concrete missing environment')]
            db.execute('UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?', (json.dumps(result), assignment))
        elif case == 'review_cycle':
            db.execute('UPDATE runtime_tasks SET review_cycle=2 WHERE task_id=?', (task.task_id,))
        elif case in {'verification_repair', 'replan_history', 'pending_steering', 'ask_user_required'}:
            event(db, task.task_id, dict(verification_repair='VERIFICATION_REVISED', replan_history='REPLAN_REQUEST_ACCEPTED',
                                       pending_steering='USER_STEERING', ask_user_required='ASK_USER_REQUIRED')[case], dict(blocker='decision'))
        elif case in {'user_message', 'claimed_user_message'}:
            from cortex_runtime.delegation.events import append_event_tx
            append_event_tx(db, task.task_id, 'user_message', 'pending-user', dict(content='new owner input'), 'owner')
            if case == 'claimed_user_message':
                db.execute("UPDATE task_events SET consumed_by='crashed-turn' WHERE dedup_key='pending-user'")
        elif case == 'runtime_proposal':
            from cortex_runtime.delegation.events import append_event_tx
            append_event_tx(db, task.task_id, 'progress', 'pending-proposal', dict(kind='candidate_ready'), 'runtime')
        elif case == 'question':
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) "
                       "VALUES('pq_open',?,'open','Question?','[]','open','now')", (task.task_id,))
        elif case == 'permission':
            from cortex_runtime.orchestration.consent import _ensure
            _ensure(db)
            db.execute("INSERT INTO consent_requests VALUES('p',?,NULL,'outside_write','subject','asked','now',NULL)", (task.task_id,))
        elif case == 'access':
            event(db, task.task_id, 'WORKER_WAIT_CHANGED', dict(wait_id='w', task_id=task.task_id,
                  assignment_ref=assignment, status='open', kind='access'))
        elif case == 'proposal':
            monkeypatch.setattr('cortex_runtime.orchestration.skill_proposals.task_proposals',
                                lambda db, task_id: [dict(status='pending', awaiting_plan=False)])
        elif case == 'stopped':
            db.execute("INSERT INTO task_stopped(task_id,stopped_at) VALUES(?,'now')", (task.task_id,))
        elif case == 'blocker':
            db.execute("UPDATE runtime_tasks SET blocker='decision' WHERE task_id=?", (task.task_id,))
        elif case == 'changed_project':
            (Path(store.project_root) / 'feature.py').write_text('late change')
        elif case == 'changed_report':
            filename = db.execute("SELECT r.filename FROM reports r JOIN runtime_nodes n ON r.id=json_extract(n.result_json,'$.report_ref') WHERE r.task_id=? ORDER BY n.rowid LIMIT 1", (task.task_id,)).fetchone()[0]
            (store._task_directory(db, task.task_id) / filename).write_text('changed')
        elif case == 'baseline_missing':
            db.execute("DELETE FROM runtime_events WHERE type='EXECUTION_BASELINE'")
        elif case == 'existing_synthesis':
            db.execute("UPDATE runtime_tasks SET synthesis='coordinator-owned' WHERE task_id=?", (task.task_id,))
        elif case == 'oversized':
            monkeypatch.setattr(synthesis, 'MAX_SYNTHESIS_BYTES', 1)
        db.commit()
    before = machine.context(task.task_id)['synthesis']
    assert synthesis.complete(machine, task.task_id, 'en', lambda db: case != 'live_workers') is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] == before
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)
    if case in {'baseline_missing', 'oversized', 'changed_report', 'existing_synthesis'}:
        # These current-build returns/rollbacks have no vocabulary entry.
        assert 'SYNTHESIS_RUNTIME_DECLINED' not in kinds(machine, task)


DECLINE_SETUPS = {
    'worker_live': lambda db, task, assignment, review_ref: None,
    'replan_history': lambda db, task, assignment, review_ref: event(
        db, task.task_id, 'REPLAN_REQUEST_ACCEPTED', dict(blocker='decision')),
    'repair_path': lambda db, task, assignment, review_ref: event(
        db, task.task_id, 'VERIFICATION_REVISED', dict(blocker='decision')),
    'pending_input': lambda db, task, assignment, review_ref: event(
        db, task.task_id, 'USER_STEERING', dict(blocker='decision')),
    'node_incomplete': lambda db, task, assignment, review_ref: db.execute(
        "UPDATE runtime_nodes SET status='failed' WHERE assignment_ref=?", (assignment,)),
    'review_not_approved': lambda db, task, assignment, review_ref: db.execute(
        'DELETE FROM runtime_nodes WHERE assignment_ref=?', (review_ref,)),
    'verification_not_passed': lambda db, task, assignment, review_ref: db.execute(
        "DELETE FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", (task.task_id,)),
    'open_check': lambda db, task, assignment, review_ref: db.execute(
        'UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?',
        (json.dumps(dict(checks=[dict(name='open check', outcome='not_run',
                                      blocker='Concrete missing environment')])), assignment)),
}


@pytest.mark.parametrize('reason_code', list(DECLINE_SETUPS))
def test_decline_records_one_reason_code_idempotently(ready, reason_code):
    """Each of the eight deterministic decline reason codes is recorded once,
    durably, as the first entry of SYNTHESIS_RUNTIME_DECLINED's `reasons`, and
    never duplicated by a later recovery/replay attempt."""
    machine, task, assignment, review_ref, receipt, observation = ready
    store = machine.store
    with store.connection() as db:
        DECLINE_SETUPS[reason_code](db, task, assignment, review_ref)
        db.commit()
    workers_absent = lambda db: reason_code != 'worker_live'
    assert synthesis.complete(machine, task.task_id, 'en', workers_absent) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'

    def declines():
        with store.connection() as db:
            return db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SYNTHESIS_RUNTIME_DECLINED'",
                (task.task_id,)).fetchall()
    rows = declines()
    assert len(rows) == 1
    payload = json.loads(rows[0][0])
    assert payload['reason'] == reason_code
    assert payload['reasons'][0]['code'] == reason_code
    assert all(isinstance(r, dict) and 'code' in r for r in payload['reasons'])
    # Idempotent on recovery: a fresh TaskMachine over the same store (a
    # recovered process) and a plain replay both leave exactly one event.
    recovered = TaskMachine(Store(store.directory, project_root=store.project_root))
    assert synthesis.complete(recovered, task.task_id, 'en', workers_absent) is None
    assert len(declines()) == 1
    assert synthesis.complete(machine, task.task_id, 'en', workers_absent) is None
    assert len(declines()) == 1


@pytest.mark.parametrize('ready', [True], indirect=True)
def test_exact_deferred_closure_allows_runtime_synthesis(ready):
    machine, task, assignment, *_ = ready
    before = machine.context(task.task_id)
    check = next(n for n in before['nodes'] if n['assignment_ref'] == assignment)['result']['checks'][0]
    assert check['outcome'] == 'not_run' and check['closed_by'].startswith('verification:')
    text = synthesis.complete(machine, task.task_id, 'en', lambda db: True)
    assert text and text.startswith('# Done')
    assert 'submitted_outcome' not in text and 'current_outcome' not in text
    assert 'current approved digest' not in text


@pytest.mark.parametrize('ready', ['closed_failure'], indirect=True)
def test_exact_failed_check_rerun_keeps_history_and_allows_synthesis(ready):
    machine, task, assignment, _, receipt, _ = ready
    before = machine.context(task.task_id)
    checks = next(n for n in before['nodes'] if n['assignment_ref'] == assignment)['result']['checks']
    assert checks[0]['outcome'] == 'failed' and checks[0]['closed_by'] == receipt['tool_call_ref']
    text = synthesis.complete(machine, task.task_id, 'en', lambda db: True)
    assert text and text.startswith('# Done')
    assert 'submitted_outcome' not in text and 'current_outcome' not in text
    assert receipt['tool_call_ref'] not in text


def test_transaction_failure_rolls_back_and_recovery_is_idempotent(ready, monkeypatch):
    machine, task, *_ = ready
    original = synthesis.transition
    def crash(*args, **kwargs):
        raise RuntimeError('process lost before commit')
    monkeypatch.setattr(synthesis, 'transition', crash)
    with pytest.raises(RuntimeError, match='process lost'):
        synthesis.complete(machine, task.task_id, 'en', lambda db: True)
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)
    assert 'SYNTHESIS_RUNTIME_DECLINED' not in kinds(machine, task)
    monkeypatch.setattr(synthesis, 'transition', original)
    assert synthesis.complete(machine, task.task_id, 'en', lambda db: True)
    assert kinds(machine, task).count('SYNTHESIS_RUNTIME_COMPOSED') == 1


@pytest.mark.parametrize('eligible', [True, False])
@pytest.mark.parametrize('delayed_exit', [False, True])
def test_controller_skips_reconciliation_only_when_eligible(tmp_path, eligible, delayed_exit):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='composed-controller', event_id='one'), 'Implement', 'owner')
    coordinator_states = []
    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            machine = TaskMachine(store)
            context = machine.context(task.task_id)
            if channel.role == 'coordinator':
                coordinator_states.append(context['state'])
                if context['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=dict(
                        tasks=[dict(id='implement', role='backend-dev', objective='Implement', dependencies=[], scope=['.'], model='auto')],
                        verification=[ARGV]), request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(
                        text='Coordinator fallback',
                        not_verified=[dict(item='live check', blocker='Missing isolated host')] if not eligible else []))
            else:
                with store.connection() as db:
                    role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                      (channel.attempt_id,)).fetchone()[0]
                if role != 'worker-code-reviewer':
                    (tmp_path / 'feature.py').write_text('new\n')
                    submit(store, channel, checks=[] if eligible else [
                        dict(name='live check', outcome='not_run', blocker='Missing isolated host')])
                else:
                    submit(store, channel, verdict='APPROVE')
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

    class DelayedReviewerExit(FakeSessionRuntime):
        result_settlement_pending = EngineSessionManagerAdapter.result_settlement_pending

        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self._result_settlements = {}
            self.settlement_checked = threading.Event()
            self.cleanup = None
            self.cleanup_errors = []

        def finish_owned(self, attempt_id):
            with store.connection() as db:
                role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                  (attempt_id,)).fetchone()[0]
            if role != 'worker-code-reviewer':
                self.terminate_owned(attempt_id)
                return
            with self._lock:
                if attempt_id in self._result_settlements:
                    return
                self._result_settlements[attempt_id] = True

            def exit_after_verification():
                try:
                    assert self.settlement_checked.wait(5)
                    time.sleep(.05)  # consumer settled; process teardown is still live
                    with store.connection() as db:
                        db.execute('PRAGMA busy_timeout=100')
                        db.execute('BEGIN IMMEDIATE')
                        assert db.execute("SELECT 1 FROM runtime_events WHERE task_id=? "
                                          "AND type='VERIFICATION_PASSED'", (task.task_id,)).fetchone()
                        assert db.execute('SELECT state FROM runtime_tasks WHERE task_id=?',
                                          (task.task_id,)).fetchone()[0] == 'RECONCILING'
                        event(db, task.task_id, 'TEST_REVIEWER_EXIT', {})
                        db.commit()
                    self.terminate_owned(attempt_id)
                except BaseException as error:
                    self.cleanup_errors.append(error)
                finally:
                    with self._lock:
                        self._result_settlements.pop(attempt_id, None)

            self.cleanup = threading.Thread(target=exit_after_verification)
            self.cleanup.start()

        def lookup_owned(self, attempt_id):
            state = super().lookup_owned(attempt_id)
            if (state == 'alive' and self.result_settlement_pending(attempt_id)
                    and TaskMachine(store).get_state(task.task_id) == 'RECONCILING'):
                self.settlement_checked.set()
            return state

    engine_class = DelayedReviewerExit if delayed_exit else FakeSessionRuntime
    engine = engine_class(store, router, connection_id='cn_synthesis', capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    messages = []
    try:
        assert runtime.start_prompt(task.task_id, 'incoming', 'Implement', messages.append,
                                    threading.Event(), emit_tool_call=lambda update: True) == StopReason.END_TURN
    finally:
        runtime.shutdown()
        if delayed_exit and engine.cleanup:
            engine.settlement_checked.set()
            engine.cleanup.join(timeout=5)
    if delayed_exit:
        assert engine.settlement_checked.is_set()
        assert not engine.cleanup.is_alive()
        assert not engine.cleanup_errors
    machine = TaskMachine(store)
    assert machine.get_state(task.task_id) == 'COMPLETED', machine.context(task.task_id)
    assert coordinator_states == (['PLANNING'] if eligible else ['PLANNING', 'RECONCILING'])
    assert ('SYNTHESIS_RUNTIME_COMPOSED' in kinds(machine, task)) is eligible
    if eligible:
        assert 'SYNTHESIS_RUNTIME_DECLINED' not in kinds(machine, task)
    if delayed_exit:
        assert kinds(machine, task).index('VERIFICATION_PASSED') < kinds(machine, task).index('TEST_REVIEWER_EXIT')
    # A completed task ends on its answer, with no closing label.
    assert not any('Runtime: COMPLETED' in m or 'Завершено' in m for m in messages)
    if eligible:
        assert any('# Done' in m for m in messages)
    else:
        # The coordinator's own synthesis is the chat's last word.
        final = [i for i, m in enumerate(messages) if m.startswith('\n\nCoordinator fallback')]
        assert len(final) == 1 and 'Not verified' in messages[final[0]], messages


def registry_engine(machine, task, *, terminated=False, alive=False):
    """Real adapter liveness/registry code; no native account or model process."""
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.engine import process_identity
    from cortex_runtime.recovery import registry

    with machine.store.connection() as db:
        attempt = db.execute("SELECT attempt_id FROM delegation_provisioning "
                             "WHERE task_id=? AND role_id!='orchestrator' LIMIT 1",
                             (task.task_id,)).fetchone()[0]
    if alive:
        identity = process_identity.capture(os.getpid(), os.getpgrp())
    else:
        child = subprocess.Popen([sys.executable, '-B', '-c', 'import sys; sys.stdin.read()'],
                                 stdin=subprocess.PIPE)
        try:
            identity = process_identity.capture(child.pid, os.getpgid(child.pid))
        finally:
            child.stdin.close()
            child.wait(timeout=5)
    assert identity is not None
    registry.record_process_launch(machine.store, attempt_id=attempt, task_id=task.task_id,
                                   role='worker', pid=identity.pid, pgid=identity.pgid,
                                   start_ticks=identity.start_ticks, boot_id=identity.boot_id,
                                   launch_command_digest='stub-child')
    if terminated:
        registry.mark_terminated(machine.store, attempt, 'confirmed_stub_exit')
    # Only the adapter's liveness dependencies are wired. Its methods, the
    # registry, OS identity probe and separate SQLite connections remain real.
    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._store = machine.store
    adapter._lock = threading.RLock()
    adapter._sessions = {}
    return adapter, attempt


@pytest.mark.parametrize('terminated', [False, True])
def test_real_registry_completion_does_not_nest_writers(ready, monkeypatch, terminated):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task, terminated=terminated)
    original_connection = machine.store.connection

    @contextmanager
    def bounded_connection():
        with original_connection() as db:
            db.execute('PRAGMA busy_timeout=100')
            yield db

    monkeypatch.setattr(machine.store, 'connection', bounded_connection)

    runtime = SimpleNamespace(_engine=adapter, _lock=threading.RLock(), _running={})
    cancel = threading.Event()
    connections = []
    original_tx = adapter.lookup_owned_tx

    def lookup_tx(attempt_id, db):
        connections.append(db)
        return original_tx(attempt_id, db)

    monkeypatch.setattr(adapter, 'lookup_owned_tx', lookup_tx)

    def workers_absent(db):
        return controller._synthesis_workers_absent(runtime, task.task_id, cancel, db)

    started = time.monotonic()
    assert synthesis.complete(machine, task.task_id, 'en', workers_absent)
    assert time.monotonic() - started < 5
    assert connections and all(db is connections[0] for db in connections)
    from cortex_runtime.recovery import registry
    assert registry.lookup_process(machine.store, attempt)['terminated_at'] is not None
    assert machine.get_state(task.task_id) == 'COMPLETED'
    assert kinds(machine, task).count('SYNTHESIS_RUNTIME_COMPOSED') == 1


def test_real_registry_live_worker_falls_back_without_lock(ready):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task, alive=True)
    runtime = SimpleNamespace(_engine=adapter, _lock=threading.RLock(), _running={})
    started = time.monotonic()
    assert synthesis.complete(machine, task.task_id, 'en',
                              lambda db: controller._synthesis_workers_absent(
                                  runtime, task.task_id, threading.Event(), db)) is None
    assert time.monotonic() - started < 5
    assert adapter.lookup_owned(attempt) == 'alive'
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)


@pytest.mark.parametrize('race', ['registry', 'adapter', 'running', 'cancel'])
def test_live_worker_or_cancellation_during_composition_rolls_back(ready, monkeypatch, race):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task)
    runtime = SimpleNamespace(_engine=adapter, _lock=threading.RLock(), _running={})
    cancel = threading.Event()
    original = machine._completion_invariants

    def late_change(db, task_id):
        original(db, task_id)
        if race == 'registry':
            from cortex_runtime.engine import process_identity
            identity = process_identity.capture(os.getpid(), os.getpgrp())
            db.execute("UPDATE attempt_process_registry SET terminated_at=NULL, pid=?, "
                       "start_ticks=?, boot_id=? WHERE attempt_id=?",
                       (identity.pid, identity.start_ticks, identity.boot_id, attempt))
        elif race == 'adapter':
            adapter._sessions[attempt] = 'became-live'
        elif race == 'running':
            runtime._running[task_id] = {'new-worker'}
        else:
            cancel.set()

    monkeypatch.setattr(machine, '_completion_invariants', late_change)
    assert synthesis.complete(machine, task.task_id, 'en',
                              lambda db: controller._synthesis_workers_absent(
                                  runtime, task.task_id, cancel, db)) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)
    from cortex_runtime.recovery import registry
    assert 'SYNTHESIS_RUNTIME_DECLINED' not in kinds(machine, task)
    # Even termination bookkeeping belonged to the rolled-back transaction.
    assert registry.lookup_process(machine.store, attempt)['terminated_at'] is None


@pytest.mark.parametrize('eligible', [True, False])
def test_restart_after_legacy_lock_crash_reconciles_once(ready, monkeypatch, eligible):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task)
    original_connection = machine.store.connection

    @contextmanager
    def bounded_connection():
        with original_connection() as db:
            db.execute('PRAGMA busy_timeout=100')
            yield db

    monkeypatch.setattr(machine.store, 'connection', bounded_connection)
    # Execute the old production ordering: synthesis owns writer A and the
    # real lookup_owned/mark_terminated opens writer B. This leaves the exact
    # durable crash boundary: VERIFICATION_PASSED, RECONCILING, no synthesis.
    with pytest.raises(sqlite3.OperationalError, match='database is locked'):
        with machine.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            adapter.lookup_owned(attempt)
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert 'VERIFICATION_PASSED' in kinds(machine, task)
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)

    store = Store(machine.store.directory, project_root=machine.store.project_root)
    recovered = TaskMachine(store)
    if not eligible:
        with store.connection() as db:
            db.execute('UPDATE runtime_tasks SET review_cycle=2 WHERE task_id=?', (task.task_id,))
            db.commit()
    adapter._store = store
    prompted = []

    def router(store, channel, ref, launch, instructions):
        def reconcile(store, channel, ref, message, call_index):
            prompted.append(recovered.get_state(task.task_id))
            mcp_tools.dispatch(store, channel, 'synthesis_submit',
                               dict(text='Recovered coordinator synthesis', not_verified=[]))
            return PromptOutcome(StopReason.END_TURN, '')
        return reconcile

    engine = FakeSessionRuntime(store, router, connection_id='cn_lock_recovery')
    engine.lookup_owned = adapter.lookup_owned
    engine.lookup_owned_tx = adapter.lookup_owned_tx
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    visible = []
    try:
        # A restarted runtime provisions its coordinator even if composition
        # ultimately needs no model turn. The fallback must have a real fake
        # session/channel binding, not a None session_ref.
        coordinator, session_ref = runtime._ensure_coordinator(task.task_id)
        for _ in range(2):
            assert controller.run(runtime, task.task_id, 'incoming', coordinator, session_ref, '',
                                  visible.append, threading.Event(), 'en') == StopReason.END_TURN
    finally:
        runtime.shutdown()
    assert recovered.get_state(task.task_id) == 'COMPLETED'
    assert prompted == ([] if eligible else ['RECONCILING'])
    assert kinds(recovered, task).count('SYNTHESIS_RUNTIME_COMPOSED') == int(eligible)
    with store.connection() as db:
        transitions = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TASK_STATE_CHANGED'",
            (task.task_id,))]
    assert sum(value['state'] == 'COMPLETED' for value in transitions) == 1


@pytest.mark.parametrize('stop', ['timeout', 'cancel', 'transport', 'enduring', 'unknown'])
def test_settlement_wait_is_bounded_and_never_claims_absence(ready, monkeypatch, stop):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task, alive=True)
    adapter._result_settlements = {attempt: object()} if stop != 'enduring' else {}
    runtime = SimpleNamespace(_store=machine.store, _engine=adapter,
                              _lock=threading.RLock(), _running={})
    cancel = threading.Event()
    polls = []
    original_pending = adapter.result_settlement_pending

    def pending(attempt_id):
        polls.append(attempt_id)
        if stop == 'cancel':
            cancel.set()
        elif stop == 'transport':
            runtime.transport_lost = lambda _: True
        return original_pending(attempt_id)

    monkeypatch.setattr(adapter, 'result_settlement_pending', pending)
    monkeypatch.setattr(controller, '_SYNTHESIS_SETTLE_SECONDS', .03)
    if stop == 'unknown':
        monkeypatch.setattr(adapter, 'lookup_owned', lambda _: 'unknown')
    started = time.monotonic()
    controller._await_synthesis_settlement(runtime, task.task_id, cancel)
    assert time.monotonic() - started < 1
    if stop == 'timeout':
        assert polls and time.monotonic() - started >= .03
    elif stop == 'unknown':
        assert not polls
    assert synthesis.complete(machine, task.task_id, 'en',
                              lambda db: controller._synthesis_workers_absent(
                                  runtime, task.task_id, cancel, db)) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)


@pytest.mark.parametrize('changed', ['digest', 'pending_input', 'open_check'])
def test_settlement_retry_preserves_other_composition_gates(ready, monkeypatch, changed):
    machine, task, assignment, review_ref, *_ = ready
    cancel = threading.Event()
    # The attempt, not the assignment identity, is the liveness lookup key.
    with machine.store.connection() as db:
        attempt = db.execute('SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?',
                             (assignment,)).fetchone()[0]
    live = {attempt}

    def pending(attempt_id):
        if changed == 'digest':
            (Path(machine.store.project_root) / 'feature.py').write_text('changed after verification')
        else:
            with machine.store.connection() as db:
                db.execute('BEGIN IMMEDIATE')  # wait never holds the completion writer
                DECLINE_SETUPS[changed](db, task, assignment, review_ref)
                db.commit()
        live.clear()
        return True

    engine = SimpleNamespace(lookup_owned=lambda ref: 'alive' if ref in live else 'absent',
                             lookup_owned_tx=lambda ref, db: 'alive' if ref in live else 'absent',
                             result_settlement_pending=pending)
    runtime = SimpleNamespace(_store=machine.store, _engine=engine,
                              _lock=threading.RLock(), _running={})
    monkeypatch.setattr(controller, '_POLL_SECONDS', .001)
    controller._await_synthesis_settlement(runtime, task.task_id, cancel)
    assert not live
    assert synthesis.complete(machine, task.task_id, 'en',
                              lambda db: controller._synthesis_workers_absent(
                                  runtime, task.task_id, cancel, db)) is None
    assert machine.get_state(task.task_id) == 'RECONCILING'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)


def test_early_worker_live_decline_does_not_evaluate_other_reasons(ready):
    machine, task, assignment, review_ref, *_ = ready
    with machine.store.connection() as db:
        DECLINE_SETUPS['repair_path'](db, task, assignment, review_ref)
        DECLINE_SETUPS['open_check'](db, task, assignment, review_ref)
        db.commit()
    assert synthesis.complete(machine, task.task_id, 'en', lambda db: False) is None
    with machine.store.connection() as db:
        row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                         "AND type='SYNTHESIS_RUNTIME_DECLINED'", (task.task_id,)).fetchone()
    assert json.loads(row[0])['reasons'] == [dict(code='worker_live')]


@pytest.mark.parametrize('stop', ['timeout', 'cancel', 'transport'])
def test_controller_settlement_timeout_or_interrupt_keeps_fallback(ready, monkeypatch, stop):
    machine, task, *_ = ready
    adapter, attempt = registry_engine(machine, task, alive=True)
    adapter._result_settlements = {attempt: object()}
    prompted = []

    def router(store, channel, ref, launch, instructions):
        def reconcile(store, channel, ref, message, call_index):
            prompted.append(machine.get_state(task.task_id))
            # End this test turn, without pretending the live worker is absent.
            return PromptOutcome(StopReason.CANCELLED, '')
        return reconcile

    engine = FakeSessionRuntime(machine.store, router, connection_id='cn_settlement_fallback')
    engine.lookup_owned = adapter.lookup_owned
    engine.lookup_owned_tx = adapter.lookup_owned_tx
    cancel = threading.Event()
    runtime = OrchestratingRuntime(machine.store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)

    def pending(attempt_id):
        if stop == 'cancel':
            cancel.set()
        elif stop == 'transport':
            runtime.transport_lost = lambda _: True
        return adapter.result_settlement_pending(attempt_id)

    engine.result_settlement_pending = pending
    monkeypatch.setattr(controller, '_SYNTHESIS_SETTLE_SECONDS', .02)
    try:
        coordinator, session_ref = runtime._ensure_coordinator(task.task_id)
        assert controller.run(runtime, task.task_id, 'incoming', coordinator, session_ref, '',
                              lambda _: True, cancel, 'en') == StopReason.CANCELLED
    finally:
        runtime.shutdown()
    assert prompted == (['RECONCILING'] if stop == 'timeout' else [])
    assert machine.get_state(task.task_id) != 'COMPLETED'
    assert machine.context(task.task_id)['synthesis'] is None
    assert 'SYNTHESIS_RUNTIME_COMPOSED' not in kinds(machine, task)
