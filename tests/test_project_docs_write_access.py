"""A writing worker can create docs/project/index.md in a project without docs/project.

Regression for production session e37b655d (project Cartoons, task t_a011ff9780d0):
project_write(docs/project/index.md, expected_sha256="absent") was refused twice with
gateway_io_failure (field operation) because docs/project did not exist yet; the
worker only succeeded after guessing project_mkdir. A missing parent is not a
reason to refuse a creation: the gateway now creates it, no-follow, and still
refuses private paths, user-protected paths, links and non-directory components.
"""
import hashlib
import json
import os

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import gateway
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

INDEX = '# Project documentation index\n'


def _worker(tmp_path, role='technical-writer'):
    from runtime_scenarios import provision_fixture
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='docs-write', event_id=role), 'Document', 'owner')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    worker, _ = provision_fixture(store, coordinator, key=role, role=role)
    return store, worker, tmp_path


def _events(store, worker, kind):
    with store.connection() as db:
        return [json.loads(row[0]) for row in db.execute(
            'SELECT payload_json FROM runtime_events WHERE actor=? AND type=? ORDER BY id', (worker.channel_id, kind))]


@pytest.mark.parametrize('request_key', ['write-project-index-1', None])
def test_session_e37b655d_docs_project_index_is_created_with_its_parent(tmp_path, request_key):
    store, worker, root = _worker(tmp_path)
    (root / 'docs' / 'concept').mkdir(parents=True)  # the session's existing sibling
    args = dict(path='docs/project/index.md', content=INDEX, expected_sha256='absent')
    if request_key:
        args['request_key'] = request_key
    result = dispatch(store, worker, 'project_write', args)
    assert result['sha256'] == hashlib.sha256(INDEX.encode()).hexdigest()
    assert (root / 'docs/project/index.md').read_text() == INDEX
    assert (root / 'docs/project').is_dir() and not (root / 'docs/project').is_symlink()
    assert not list(root.rglob('.cortex-write-*'))
    assert not _events(store, worker, 'TOOL_REJECTED')
    # An identical retry replays the receipt instead of writing again.
    assert dispatch(store, worker, 'project_write', args) == result


def test_deep_missing_parents_are_created_for_new_files(tmp_path):
    store, worker, root = _worker(tmp_path)
    dispatch(store, worker, 'project_write', dict(path='a/b/c/new.md', content='x', expected_sha256='absent'))
    assert (root / 'a/b/c/new.md').read_text() == 'x'


def test_missing_parent_with_a_hash_is_a_precise_conflict_and_creates_nothing(tmp_path):
    store, worker, root = _worker(tmp_path)
    stale = '0' * 64
    with pytest.raises(StoreError, match='gateway_write_conflict') as error:
        dispatch(store, worker, 'project_write',
                 dict(path='docs/project/index.md', content=INDEX, expected_sha256=stale, request_key='stale'))
    assert (error.value.field, error.value.received, error.value.expected) == ('expected_sha256', stale, 'absent')
    assert 'docs does not exist' in error.value.correction
    assert not (root / 'docs').exists()


@pytest.mark.parametrize('path', ['.cortex/index.md', '.git/index.md', 'docs/.git/index.md', 'docs/.cortex/index.md'])
def test_private_paths_stay_refused_and_no_parent_is_created(tmp_path, path):
    store, worker, root = _worker(tmp_path)
    before = sorted(p.relative_to(root).as_posix() for p in root.rglob('*'))
    with pytest.raises(StoreError, match='gateway_path_denied'):
        dispatch(store, worker, 'project_write', dict(path=path, content='x', expected_sha256='absent'))
    with pytest.raises(StoreError, match='gateway_path_denied'):
        dispatch(store, worker, 'project_mkdir', dict(path=path))
    assert sorted(p.relative_to(root).as_posix() for p in root.rglob('*')) == before


def test_user_protected_directory_stays_refused(tmp_path, monkeypatch):
    store, worker, root = _worker(tmp_path)
    monkeypatch.setattr(gateway, 'write_denials', lambda db, task_id: ['docs/project'])
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        dispatch(store, worker, 'project_write', dict(path='docs/project/index.md', content=INDEX, expected_sha256='absent'))
    assert not (root / 'docs').exists()


def test_read_only_role_still_cannot_create_docs(tmp_path):
    store, worker, root = _worker(tmp_path, 'code-reviewer')
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        dispatch(store, worker, 'project_write', dict(path='docs/project/index.md', content=INDEX, expected_sha256='absent'))
    assert not (root / 'docs').exists()


def test_parent_symlink_or_file_component_is_a_precise_refusal(tmp_path):
    store, worker, root = _worker(tmp_path)
    outside = tmp_path.parent / (tmp_path.name + '-outside')
    outside.mkdir()
    os.symlink(outside, root / 'docs')
    with pytest.raises(StoreError, match='gateway_parent_not_directory') as error:
        dispatch(store, worker, 'project_write', dict(path='docs/project/index.md', content=INDEX, expected_sha256='absent'))
    assert (error.value.field, error.value.received) == ('path', 'docs')
    assert not list(outside.iterdir())
    (root / 'notes').write_text('a file')
    with pytest.raises(StoreError, match='gateway_parent_not_directory') as error:
        dispatch(store, worker, 'project_write', dict(path='notes/index.md', content=INDEX, expected_sha256='absent'))
    assert error.value.received == 'notes'
    # Precise pre-effect refusals are completed rejections, never uncertain.
    assert [event['error'] for event in _events(store, worker, 'TOOL_REJECTED')] == ['gateway_parent_not_directory'] * 2


def test_mkdir_creates_parents_and_is_idempotent_for_a_directory(tmp_path):
    store, worker, root = _worker(tmp_path)
    assert dispatch(store, worker, 'project_mkdir', dict(path='docs/project/adr')) == dict(created=True)
    assert (root / 'docs/project/adr').is_dir()
    assert dispatch(store, worker, 'project_mkdir', dict(path='docs/project', request_key='again')) == dict(created=False)
    (root / 'docs/file').write_text('x')
    with pytest.raises(StoreError, match='gateway_path_exists') as error:
        dispatch(store, worker, 'project_mkdir', dict(path='docs/file'))
    assert error.value.field == 'path' and error.value.received == 'docs/file'
    assert (root / 'docs/file').read_text() == 'x'
