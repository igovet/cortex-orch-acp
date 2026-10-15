"""Executable, fresh no-paid inspection fixtures; never use an old live store."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.storage.inspection import read_binding_page
from cortex_runtime.store import Store


@pytest.fixture
def bound(tmp_path):
    project = tmp_path / 'project'
    project.mkdir()
    store = Store(project / '.cortex', project_root=project)
    task = bootstrap_task(store, SourceEvent(origin='fixture', event_id='first'), 'private goal sentinel', 'fixture-owner')
    sid = 'acps_' + 'a' * 12
    sessions.create_session(store, sid, str(project), {'name': 'fixture'}, {}, [])
    sessions.bind_task(store, sid, task.task_id)
    TaskMachine(store).create(task.task_id, 'private goal sentinel')
    for i in range(4):
        events.append_event(store, task.task_id, 'user_message', f'fixture-{i}',
                            {'content': 'private body sentinel', 'model': 'private fake credential sentinel'}, 'fixture')
    return project, store, sid, task.task_id, task.owner_epoch.epoch


def files(project):
    return {str(p.relative_to(project)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in project.rglob('*') if p.is_file()}


def test_existing_public_reads_and_complete_hashed_pagination_are_non_mutating(bound):
    project, store, sid, task, epoch = bound
    before = files(project)
    # Demonstrate the existing non-initializing Store route on a CURRENT fresh
    # fixture, not a legacy store or the checkout, before the metadata helper.
    inspection = Store(project / '.cortex', initialize=False, project_root=project)
    assert sessions.get_session(inspection, sid)['task_id'] == task
    assert TaskMachine(inspection).context(task)['state'] == 'PLANNING'
    # Store's non-initializing constructor still runs additive schema guards;
    # it is not our mutation-free reader. Refresh the baseline AFTER it.
    before = files(project)
    result = read_binding_page(str(project), sid, task, epoch, limit=2)
    digest = result['snapshot_sha256']
    rows = result['rows'][:]
    while result['next_cursor']:
        assert not result['complete']
        result = read_binding_page(str(project), sid, task, epoch, cursor=result['next_cursor'], limit=2)
        assert result['snapshot_sha256'] == digest
        rows.extend(result['rows'])
    assert result['complete'] and len(rows) == result['total']
    inputs = [r for r in rows if r['section'] == 'task_events']
    assert [r['id'] for r in inputs] == sorted({r['id'] for r in inputs})
    assert len(inputs) == 4
    assert 'private' not in json.dumps(rows)
    assert files(project) == before


def test_actual_cli_reads_new_fixture_and_refuses_wrong_binding(bound):
    project, store, sid, task, epoch = bound
    script = Path(__file__).resolve().parents[1] / 'scripts' / 'cortex_inspect_task.py'
    argv = [sys.executable, '-B', str(script), '--project', str(project), '--session', sid,
            '--task', task, '--owner-epoch', str(epoch)]
    before = files(project)
    result = subprocess.run(argv, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    page = json.loads(result.stdout)
    assert page['binding']['task_id'] == task and page['complete']
    first = subprocess.run(argv + ['--limit', '1'], capture_output=True, text=True)
    first_page = json.loads(first.stdout)
    assert first.returncode == 0 and not first_page['complete']
    second = subprocess.run(argv + ['--limit', '1', '--cursor', first_page['next_cursor']], capture_output=True, text=True)
    assert second.returncode == 0 and json.loads(second.stdout)['offset'] == 1
    result = subprocess.run(argv[:-1] + [str(epoch + 1)], capture_output=True, text=True)
    assert result.returncode == 1 and json.loads(result.stdout)['available'] is False
    assert files(project) == before


def test_incomplete_action_is_retained_without_output_or_invented_pass(bound):
    project, store, sid, task, epoch = bound
    with store.connection() as db:
        db.execute('INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)',
                   (task, 'fake-pending-request', 'fake-channel', 'command_run', 'fake-digest',
                    json.dumps({'complete': False, 'tool_call_ref': 'gateway:' + 'a' * 32,
                                'output': 'private sentinel', 'argv': ['private sentinel']})))
    page = read_binding_page(str(project), sid, task, epoch)
    retained = next(r for r in page['rows'] if r['section'] == 'action_receipts')
    assert retained['complete'] is False and retained['exit_code'] is None
    assert 'private' not in json.dumps(page)


@pytest.mark.parametrize('change', ['session', 'task', 'epoch', 'relative', 'limit', 'cursor'])
def test_bad_missing_stale_ambiguous_binding_fails_closed(bound, change):
    project, store, sid, task, epoch = bound
    kwargs = {}
    if change == 'session': sid = 'acps_' + 'b' * 12
    if change == 'task': task = 't_' + 'b' * 12
    if change == 'epoch': epoch += 1
    if change == 'relative': project = Path('.')
    if change == 'limit': kwargs['limit'] = 0
    if change == 'cursor': kwargs['cursor'] = 'ambiguous'
    with pytest.raises(StoreError):
        read_binding_page(str(project), sid, task, epoch, **kwargs)


def test_cursor_is_bound_to_exact_snapshot_and_cannot_hide_new_input(bound):
    project, store, sid, task, epoch = bound
    page = read_binding_page(str(project), sid, task, epoch, limit=1)
    events.append_event(store, task, 'user_message', 'new', {'content': 'another private body'}, 'fixture')
    with pytest.raises(StoreError, match='inspection_snapshot_changed'):
        read_binding_page(str(project), sid, task, epoch, cursor=page['next_cursor'])


def test_missing_project_store_never_initialized(tmp_path):
    project = tmp_path / 'missing'
    project.mkdir()
    with pytest.raises(FileNotFoundError):
        read_binding_page(str(project), 'acps_' + 'a' * 12, 't_' + 'a' * 12, 1)
    assert not (project / '.cortex').exists()


def test_legacy_format_is_not_migrated(bound):
    project, store, sid, task, epoch = bound
    with store.connection() as db:
        db.execute('PRAGMA user_version=11')
    before = files(project)
    with pytest.raises(StoreError, match='legacy_storage_migration_required'):
        read_binding_page(str(project), sid, task, epoch)
    assert files(project) == before and not (project / '.cortex' / '.migration-backups').exists()


def test_private_directory_and_symlink_are_refused(bound, tmp_path):
    project, store, sid, task, epoch = bound
    (project / '.cortex').chmod(0o755)
    with pytest.raises(StoreError, match='unsafe_storage'):
        read_binding_page(str(project), sid, task, epoch)
    (project / '.cortex').chmod(0o700)
    link = tmp_path / 'linked'
    link.symlink_to(project, target_is_directory=True)
    with pytest.raises(StoreError):
        read_binding_page(str(link), sid, task, epoch)


def test_closed_session_is_not_a_disconnect_and_release_remains_observed(bound):
    from cortex_runtime.delegation import capacity
    from cortex_runtime.recovery.worker_reconciliation import record_host_disconnect
    project, store, sid, task, epoch = bound
    reservation = capacity.reserve(store, 'fixture-connection', task, 'fixture-attempt', 'worker')
    sessions.close_session(store, sid)
    page = read_binding_page(str(project), sid, task, epoch)
    assert not any(r.get('kind') == 'host_disconnected' for r in page['rows'])
    assert any(r['section'] == 'capacity' and r['state'] == 'reserved' for r in page['rows'])
    record_host_disconnect(store, task, sid, 'fixture-disconnect', trigger='fixture_eof')
    capacity.release_confirmed(store, reservation, 'fixture-observed-exit')
    page = read_binding_page(str(project), sid, task, epoch)
    assert any(r.get('kind') == 'host_disconnected' for r in page['rows'])
    assert any(r['section'] == 'capacity' and r['state'] == 'released'
               and r['release_observation'] == 'fixture-observed-exit' for r in page['rows'])


def test_reader_never_checkpoints_live_wal_or_bypasses_busy_lock(bound):
    project, store, sid, task, epoch = bound
    with store.connection() as db:
        with pytest.raises(BlockingIOError):
            read_binding_page(str(project), sid, task, epoch)
    wal = project / '.cortex' / 'cortex.sqlite3-wal'
    wal.write_bytes(b'fake-nonempty-wal-sentinel')
    wal.chmod(0o600)
    before = files(project)
    with pytest.raises(StoreError, match='inspection_uncheckpointed_wal'):
        read_binding_page(str(project), sid, task, epoch)
    assert files(project) == before


def test_overflow_is_refusal_not_truncated_success(bound, monkeypatch):
    import cortex_runtime.storage.inspection as inspector
    project, store, sid, task, epoch = bound
    monkeypatch.setattr(inspector, 'MAX_ROWS', 1)
    with pytest.raises(StoreError, match='inspection_evidence_overflow'):
        read_binding_page(str(project), sid, task, epoch)


# Exits 7 until the fix (fixed.flag) exists; its argv and output carry sentinels.
SENTINEL_CHECK = ['python3', '-B', '-c', 'import pathlib,sys; print("private output sentinel"); '
                  'sys.exit(0 if pathlib.Path("fixed.flag").exists() else 7)  # private argv sentinel']


@pytest.fixture
def worker_bound(tmp_path):
    from cortex_runtime.trusted_channel import resolve_channel
    from runtime_scenarios import provision_fixture
    project = tmp_path / 'project'
    project.mkdir()
    store = Store(project / '.cortex', project_root=project)
    task = bootstrap_task(store, SourceEvent(origin='fixture', event_id='worker'), 'private goal sentinel', 'fixture-owner')
    sid = 'acps_' + 'c' * 12
    sessions.create_session(store, sid, str(project), {'name': 'fixture'}, {}, [])
    sessions.bind_task(store, sid, task.task_id)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    return project, store, sid, task, worker, assignment


def _all_rows(project, sid, task):
    page = read_binding_page(str(project), sid, task.task_id, task.owner_epoch.epoch, limit=200)
    rows = page['rows'][:]
    while page['next_cursor']:
        page = read_binding_page(str(project), sid, task.task_id, task.owner_epoch.epoch,
                                 cursor=page['next_cursor'], limit=200)
        rows.extend(page['rows'])
    assert page['complete']
    return rows


@pytest.mark.usefixtures('unjailed_gateway_command')
def test_completed_receipts_and_failure_closure_chain_are_projected(worker_bound):
    from cortex_runtime.delegation import mcp_tools
    from runtime_scenarios import publish_fixture_report
    project, store, sid, task, worker, assignment = worker_bound
    failed = mcp_tools.dispatch(store, worker, 'command_run', dict(argv=SENTINEL_CHECK, request_key='first'))
    (project / 'fixed.flag').write_text('fixed')
    passed = mcp_tools.dispatch(store, worker, 'command_run', dict(argv=SENTINEL_CHECK, request_key='rerun'))
    assert (failed['exit_code'], passed['exit_code']) == (7, 0)
    report = publish_fixture_report(store, worker, 'private report body sentinel')
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', dict(
        report=report, summary='private summary sentinel', request_key='result', checks=[
            dict(name='private check name sentinel', outcome='failed', tool_call_ref=failed['tool_call_ref']),
            dict(name='private check name sentinel', outcome='passed', tool_call_ref=passed['tool_call_ref'])]))
    assert receipt['node_status'] == 'completed'
    rows = _all_rows(project, sid, task)
    commands = {r['tool_call_ref']: r for r in rows
                if r['section'] == 'action_receipts' and r['operation'] == 'command_run'}
    for ref, code in ((failed['tool_call_ref'], 7), (passed['tool_call_ref'], 0)):
        row = commands[ref]
        assert (row['complete'], row['run_complete'], row['exit_code'], row['failure']) == (True, True, code, None)
        assert row['channel_attempt_id'] == worker.attempt_id and row['channel_owner_epoch'] == task.owner_epoch.epoch
        assert row['retained_output']['bound'] and row['retained_output']['truncated'] is False
        assert row['retained_output']['exit_code'] == code
    presented = [r for r in rows if r.get('type') == 'CHECK_FAILURE_PRESENTED']
    assert [(r['attempt_id'], r['tool_call_ref']) for r in presented] == [(worker.attempt_id, failed['tool_call_ref'])]
    completed = [r for r in rows if r.get('type') == 'WORKER_COMPLETED']
    assert len(completed) == 1 and completed[0]['attempt_id'] == worker.attempt_id
    assert [(c['outcome'], c['tool_call_ref'], c['closed_by']) for c in completed[0]['checks']] == [
        ('failed', failed['tool_call_ref'], passed['tool_call_ref']), ('passed', passed['tool_call_ref'], None)]
    authorized = [r for r in rows if r.get('type') == 'TOOL_COMPLETED' and r['operation'] == 'command_run']
    assert {r['tool_call_ref'] for r in authorized} == set(commands)
    assert all(r['channel_attempt_id'] == worker.attempt_id for r in authorized)
    assert 'private' not in json.dumps(rows) and 'sentinel' not in json.dumps(rows)


def test_rejections_forged_refs_route_and_question_metadata_fail_closed(worker_bound):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.orchestration.state import event
    from cortex_runtime.routing import store as routing_store
    from runtime_scenarios import publish_fixture_report
    project, store, sid, task, worker, assignment = worker_bound
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store, worker, 'command_run', dict(argv='private rejected argv sentinel', request_key='bad'))
    routing_store.record_attempt_audit(
        store, attempt_id=worker.attempt_id, task_id=task.task_id, role='worker-backend-dev', engine='claude-agent-acp',
        connection_id='fixture-connection', model='haiku', params_fingerprint='fp', config_revision_id=None,
        decision='authorized', detail=dict(route_basis='auto:user_request', effort='high', effort_source='auto_route',
                                           route_decision=dict(basis='user_request', decided_by='user_request', tier='light',
                                                               chosen='claude-agent-acp/haiku', model_hint_reason='private reason sentinel',
                                                               candidates=[dict(route='private candidate sentinel', used_percent=12)],
                                                               user_model_request=dict(status='honoured', text='private request sentinel'))))
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        event(db, task.task_id, 'USER_QUESTION_REQUESTED', dict(
            question_id='pq_' + 'd' * 12, question='private question sentinel', options=['private a', 'private b'],
            resume_state='RECOVERING', blocker='review_cycle_limit', revision=1, runtime_stop=True))
        db.commit()
    report = publish_fixture_report(store, worker)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', dict(
        report=report, summary='Observed', request_key='forged', checks=[
            dict(name='private check name sentinel', outcome='passed', tool_call_ref='private forged ref sentinel')]))
    rows = _all_rows(project, sid, task)
    rejected = [r for r in rows if r['section'] == 'observability']
    assert rejected and all(r['attempt_id'] == worker.attempt_id for r in rejected)
    assert {r['category'] for r in rejected} <= {'validation', 'submission_rejection', 'gateway_refusal'}
    assert any(r['operation'] == 'command_run' for r in rejected)
    dispatch = next(r for r in rows if r['section'] == 'dispatch')
    assert dispatch['route'] == dict(route_basis='auto:user_request', effort='high', effort_source='auto_route',
                                     basis='user_request', decided_by='user_request', role_tier=None, tier='light',
                                     preference=None, preference_status=None, model_hint_status=None,
                                     cross_provider_review=None, chosen='claude-agent-acp/haiku',
                                     user_model_request_status='honoured', pin_effort_clamped=False)
    question = next(r for r in rows if r.get('type') == 'USER_QUESTION_REQUESTED')
    assert (question['question_id'], question['runtime_stop'], question['blocker'], question['options_count']) == (
        'pq_' + 'd' * 12, True, 'review_cycle_limit', 2)
    terminal = next(r for r in rows if r.get('type') in ('WORKER_COMPLETED', 'WORKER_FAILED'))
    assert receipt['node_status'] in ('completed', 'failed')
    assert [(c['tool_call_ref'], c['claimed_outcome']) for c in terminal['checks']] == [('unrecognized', 'passed')]
    assert 'private' not in json.dumps(rows) and 'sentinel' not in json.dumps(rows)


def test_oversized_evidence_is_refused_before_materializing_history(bound, monkeypatch):
    import cortex_runtime.storage.inspection as inspector
    project, store, sid, task, epoch = bound
    monkeypatch.setattr(inspector, 'MAX_SOURCE_BYTES', 1)
    with pytest.raises(StoreError, match='inspection_evidence_overflow'):
        read_binding_page(str(project), sid, task, epoch)
