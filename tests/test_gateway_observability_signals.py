"""Stage 2 gateway instrumentation (G1/G2/G3): metadata-only observability
for project_read calls, pre-authorization refusals and project_write/edit
change size -- never file content or rejected arguments.
"""
import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.observability import events as observability_events
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

from test_worker_wait_continuation import bound


@pytest.fixture
def context(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='gateway-signals-test', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    channel = resolve_channel(store, boot.channel_id, boot.secret)
    return store, channel, machine, tmp_path


def _read_rows(store, task_id):
    return observability_events.list_events(store, task_id, category='project_read')


def _refusal_rows(store, task_id):
    return observability_events.list_events(store, task_id, category='gateway_refusal')


# --- G1: project_read -------------------------------------------------

def test_two_reads_of_the_same_file_produce_two_rows_with_the_same_sha(context):
    store, ch, _, root = context
    (root / 'file.txt').write_text('hello world\n')
    first = dispatch(store, ch, 'project_read', dict(path='file.txt'))
    second = dispatch(store, ch, 'project_read', dict(path='file.txt'))
    rows = _read_rows(store, ch.task_id)
    assert len(rows) == 2
    assert rows[0]['detail']['kind'] == rows[1]['detail']['kind'] == 'text'
    assert rows[0]['outcome'] == rows[1]['outcome'] == 'text'
    assert rows[0]['detail']['sha256'] == rows[1]['detail']['sha256'] == first['sha256'] == second['sha256']
    assert rows[0]['basis'] == 'observed'
    assert rows[0]['attempt_id'] == ch.attempt_id


def test_missing_path_produces_a_missing_row(context):
    store, ch, _, root = context
    dispatch(store, ch, 'project_read', dict(path='absent.txt'))
    rows = _read_rows(store, ch.task_id)
    assert len(rows) == 1
    assert rows[0]['detail']['kind'] == 'missing'
    assert rows[0]['outcome'] == 'missing'
    assert rows[0]['detail']['path'] == 'absent.txt'


def test_no_project_read_detail_ever_carries_a_content_key(context):
    store, ch, _, root = context
    (root / 'file.txt').write_text('hello world\n')
    dispatch(store, ch, 'project_read', dict(path='file.txt'))
    dispatch(store, ch, 'project_read', dict(path='.'))
    dispatch(store, ch, 'project_read', dict(path='absent.txt'))
    rows = _read_rows(store, ch.task_id)
    assert len(rows) == 3
    for row in rows:
        assert 'content' not in row['detail']
        assert 'text' not in row['detail']
        assert 'entries' not in row['detail']


def test_sha_changes_after_project_edit(bound):
    store, _machine, _coordinator, workers = bound
    ch = workers[0]
    dispatch(store, ch, 'project_write',
             dict(path='part0.txt', content='first\n', expected_sha256='absent', request_key='create'))
    first = dispatch(store, ch, 'project_read', dict(path='part0.txt'))
    dispatch(store, ch, 'project_edit',
             dict(path='part0.txt', edits=[dict(old_string='first\n', new_string='second\n')], request_key='edit'))
    second = dispatch(store, ch, 'project_read', dict(path='part0.txt'))
    assert first['sha256'] != second['sha256']
    rows = [row for row in _read_rows(store, ch.task_id) if row['detail']['kind'] == 'text']
    assert [row['detail']['sha256'] for row in rows] == [first['sha256'], second['sha256']]


# --- G2: gateway refusals ----------------------------------------------

def test_schema_invalid_write_scope_denial_and_key_conflict_each_give_one_refusal_row(context):
    store, ch, _, root = context
    with pytest.raises(StoreError, match='invalid_arguments'):
        # expected_sha256 is required and omitted here.
        dispatch(store, ch, 'project_write', dict(path='file.txt', content='x'))
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        # The coordinator channel holds no writable scope at all.
        dispatch(store, ch, 'project_write',
                 dict(path='file.txt', content='x', expected_sha256='absent', request_key='scope'))
    draft = store.call('create_draft', dict(template='general', request_key='d'),
                       ch.principal_thread_id, ch.parent_principal_thread_id, trusted=True)
    body = draft['required_first_line'] + '\n\n# Report\n\nActual evidence.\n'
    dispatch(store, ch, 'draft_write', dict(draft_id=draft['draft_id'], content=body, request_key='write'))
    with pytest.raises(StoreError, match='request_key_conflict'):
        dispatch(store, ch, 'draft_write', dict(draft_id=draft['draft_id'], content=body + 'more', request_key='write'))

    rows = _refusal_rows(store, ch.task_id)
    assert len(rows) == 3
    assert [row['detail']['operation'] for row in rows] == ['project_write', 'project_write', 'draft_write']
    assert [row['detail']['error'] for row in rows] == \
        ['invalid_arguments', 'gateway_scope_denied', 'request_key_conflict']
    for row in rows:
        # Metadata only: operation + error code, never the rejected arguments.
        assert set(row['detail'].keys()) == {'operation', 'error'}
        assert row['basis'] == 'observed'
        assert row['attempt_id'] == ch.attempt_id


# --- G3: project_write/project_edit change size -------------------------

def test_creation_sets_changed_bytes_to_the_full_size(bound):
    store, _machine, _coordinator, workers = bound
    ch = workers[0]
    content = '\n'.join(f'line{i}' for i in range(1000)) + '\n'
    result = dispatch(store, ch, 'project_write',
                      dict(path='part0.txt', content=content, expected_sha256='absent', request_key='create'))
    assert result['changed_bytes'] == result['byte_count'] == result['size_bytes']
    assert result['changed_lines'] == result['new_lines'] == 1000


def test_one_line_change_in_a_1000_line_file_gives_changed_lines_one_and_a_small_changed_bytes(bound):
    store, _machine, _coordinator, workers = bound
    ch = workers[0]
    content = '\n'.join(f'line{i}' for i in range(1000)) + '\n'
    dispatch(store, ch, 'project_write',
             dict(path='part0.txt', content=content, expected_sha256='absent', request_key='create'))
    # A prior (partial, but current) read is required before project_edit.
    dispatch(store, ch, 'project_read', dict(path='part0.txt'))
    result = dispatch(store, ch, 'project_edit',
                      dict(path='part0.txt', edits=[dict(old_string='line500\n', new_string='CHANGED\n')],
                           request_key='edit'))
    assert result['changed_lines'] == 1
    # byte_count reflects the whole (large) file; changed_bytes only the
    # small edited region -- a high byte_count/changed_bytes ratio.
    assert result['byte_count'] > 1000
    assert result['changed_bytes'] < result['byte_count'] / 10
