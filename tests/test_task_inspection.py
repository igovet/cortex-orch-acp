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


def test_oversized_evidence_is_refused_before_materializing_history(bound, monkeypatch):
    import cortex_runtime.storage.inspection as inspector
    project, store, sid, task, epoch = bound
    monkeypatch.setattr(inspector, 'MAX_SOURCE_BYTES', 1)
    with pytest.raises(StoreError, match='inspection_evidence_overflow'):
        read_binding_page(str(project), sid, task, epoch)
