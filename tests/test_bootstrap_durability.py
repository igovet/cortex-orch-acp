"""Durable, idempotent bootstrap: crash boundaries and replay identity."""
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def event(origin='cli', event_id=None):
    return SourceEvent(origin=origin, event_id=event_id or str(uuid.uuid4()))


def counts(store):
    with store.connection() as db:
        tasks = db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0]
        events = db.execute('SELECT COUNT(*) FROM source_events').fetchone()[0]
        revisions = db.execute('SELECT COUNT(*) FROM source_revisions').fetchone()[0]
    return tasks, events, revisions


def test_replay_of_identical_event_does_not_duplicate_the_task(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    ev = event()
    first = bootstrap_task(store, ev, 'Build a page.', 'owner-1')
    assert not first.replayed and first.channel_id and first.secret
    second = bootstrap_task(store, ev, 'Build a page.', 'owner-1')
    assert second.replayed and second.task_id == first.task_id
    assert second.channel_id is None and second.secret is None
    assert counts(store) == (1, 1, 1)


def test_identical_text_distinct_event_ids_remain_distinct_tasks(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    first = bootstrap_task(store, event(), 'Same text.', 'owner-1')
    second = bootstrap_task(store, event(), 'Same text.', 'owner-1')
    assert first.task_id != second.task_id
    assert counts(store) == (2, 2, 2)


def test_recoverable_task_and_one_source_event_survive_a_crash_after_commit(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    ev = event()
    result = bootstrap_task(store, ev, 'Build a page.', 'owner-1')
    # Simulate "crash after commit, before delivery to the coordinator": the
    # caller loses the secret but the durable task/event already committed.
    restarted = Store(tmp_path/'.cortex', project_root=tmp_path)
    replay = bootstrap_task(restarted, ev, 'Build a page.', 'owner-1')
    assert replay.replayed and replay.task_id == result.task_id
    assert counts(restarted) == (1, 1, 1)


def test_crash_before_commit_persists_nothing(tmp_path, monkeypatch):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)

    def explode(*a, **k):
        raise RuntimeError('injected failure before commit')

    monkeypatch.setattr(store, 'bootstrap_source', explode)
    with pytest.raises(RuntimeError):
        bootstrap_task(store, event(), 'Build a page.', 'owner-1')
    assert counts(store) == (0, 0, 0)
    task_dir_entries = list((tmp_path/'.cortex').glob('t_*'))
    assert task_dir_entries == []


class _Loose:
    """A minimal source-event-shaped object bypassing SourceEvent's own
    non-empty-string validation, to exercise bootstrap_task's own bounds."""
    def __init__(self, origin, event_id):
        self.origin = origin
        self.event_id = event_id


def test_bounded_event_identity_validation(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    with pytest.raises(StoreError):
        bootstrap_task(store, _Loose('', 'x'), 'Text', 'owner-1')
    with pytest.raises(StoreError):
        bootstrap_task(store, _Loose('cli', 'x'*600), 'Text', 'owner-1')
    with pytest.raises(StoreError):
        bootstrap_task(store, event(), '', 'owner-1')
    with pytest.raises(StoreError):
        bootstrap_task(store, event(), 'Text', '')
