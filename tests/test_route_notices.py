"""A model hint the router ignored is said in the chat once (2026-10-22: an
audit the owner assigned to Opus ran on Sol, visible only in the route audit)."""
import json
import uuid
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.route_notices import ignored_hint_notices
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _audit(store, task_id, hint, status, engine='codex-acp', model='gpt-6.1-sol', *,
           effort=None, requested_effort=None, attempt_id=None, connection_id='cn_1', decision_fields=None):
    attempt_id = attempt_id or 'xa_' + uuid.uuid4().hex[:12]
    decision = dict(model_hint=hint, model_hint_status=status, **(decision_fields or {}))
    if requested_effort is not None:
        decision['user_model_request'] = dict(effort=requested_effort)
    detail = dict(route_decision=decision)
    if effort is not None:
        detail['effort'] = effort
    with store.connection() as db:
        db.execute("INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   ('ra_' + uuid.uuid4().hex[:12], attempt_id, task_id, 'worker-code-reviewer',
                    engine, connection_id, model, 'fp', None, 'authorized',
                    json.dumps(detail), None,
                    '2026-10-22T12:59:20+00:00'))
        db.commit()
    return attempt_id


@pytest.fixture
def routed_task(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='pin'), 'Audit', 'owner')
    TaskMachine(store).create(task.task_id, 'Audit')
    return store, task.task_id


def test_an_ignored_hint_is_told_once_and_an_honoured_one_never(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='hint'), 'Audit', 'owner')
    TaskMachine(store).create(task.task_id, 'Audit')
    _audit(store, task.task_id, 'claude-agent-acp/opus[1m]', 'ignored:owner_preferred')
    _audit(store, task.task_id, 'codex-acp/gpt-6.1-sol', 'honoured')
    _audit(store, task.task_id, 'auto', 'auto')
    [line] = ignored_hint_notices(store, task.task_id, 'ru')
    assert 'claude-agent-acp/opus[1m]' in line and 'codex-acp/gpt-6.1-sol' in line
    assert 'предпочитают для этой работы другую модель' in line
    assert ignored_hint_notices(store, task.task_id, 'ru') == []


@pytest.mark.parametrize(('requested', 'effective', 'count'), [
    (None, 'high', 0), ('high', 'high', 0), ('medium', 'high', 1), ('high', None, 1),
])
@pytest.mark.parametrize('language', ['en', 'ru'])
def test_same_model_compares_explicit_effort(routed_task, requested, effective, count, language):
    store, task_id = routed_task
    _audit(store, task_id, 'codex-acp/gpt-6.1-sol', 'ignored:pinned',
           effort=effective, requested_effort=requested)
    lines = ignored_hint_notices(store, task_id, language)
    assert len(lines) == count
    if count:
        assert f'codex-acp/gpt-6.1-sol:{requested}' in lines[0]
        assert ('dispatch selected' if language == 'en' else 'для отправки выбран') in lines[0]
        assert ('different model' if language == 'en' else 'другая модель') not in lines[0]
        if effective:
            assert f'codex-acp/gpt-6.1-sol:{effective}' in lines[0]
        else:
            assert ('effort could not be verified' if language == 'en'
                    else 'effort не удалось проверить') in lines[0]
    assert ignored_hint_notices(store, task_id, language) == []


@pytest.mark.parametrize('fields', [
    dict(engine=''), dict(model=''), dict(connection_id=''),
    dict(decision_fields=dict(chosen='other/model')),
    dict(decision_fields=dict(chosen_connection_id='cn_other')),
])
@pytest.mark.parametrize('language', ['en', 'ru'])
def test_missing_or_conflicting_actual_binding_is_not_a_match(routed_task, fields, language):
    store, task_id = routed_task
    _audit(store, task_id, 'codex-acp/gpt-6.1-sol', 'ignored:pinned', **fields)
    [line] = ignored_hint_notices(store, task_id, language)
    assert ('effective route could not be verified' if language == 'en'
            else 'фактический маршрут не удалось проверить') in line
    assert 'different model' not in line and 'dispatch selected' not in line
    with store.connection() as db:
        payload = json.loads(db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                                        "AND type='MODEL_HINT_IGNORED_SHOWN'", (task_id,)).fetchone()[0])
    assert payload['used'] is None
    assert ignored_hint_notices(store, task_id, language) == []


@pytest.mark.parametrize('conflict', [
    dict(engine='other-engine'), dict(model='other-model'),
    dict(effort='medium'), dict(connection_id='cn_other'),
])
def test_ambiguous_attempt_audits_stay_visible_once(routed_task, conflict):
    store, task_id = routed_task
    attempt = _audit(store, task_id, 'codex-acp/gpt-6.1-sol', 'ignored:pinned', effort='high')
    fields = dict(effort='high')
    fields.update(conflict)
    _audit(store, task_id, 'codex-acp/gpt-6.1-sol', 'ignored:pinned', attempt_id=attempt, **fields)
    [line] = ignored_hint_notices(store, task_id, 'en')
    assert 'effective route could not be verified' in line
    assert ignored_hint_notices(store, task_id, 'en') == []


def test_different_engine_with_same_model_id_is_not_a_match(routed_task):
    store, task_id = routed_task
    _audit(store, task_id, 'other-engine/gpt-6.1-sol', 'ignored:pinned', effort='high')
    [line] = ignored_hint_notices(store, task_id, 'en')
    assert 'other-engine/gpt-6.1-sol' in line and 'codex-acp/gpt-6.1-sol:high' in line


def test_duplicate_audit_rows_and_concurrent_recovered_controllers_do_not_repeat(routed_task):
    store, task_id = routed_task
    attempt = _audit(store, task_id, 'other-engine/model', 'ignored:pinned')
    _audit(store, task_id, 'other-engine/model', 'ignored:pinned', attempt_id=attempt)
    barrier = Barrier(2)

    def notify(_):
        recovered = Store(store.directory, project_root=store.project_root)
        barrier.wait(timeout=10)
        return ignored_hint_notices(recovered, task_id, 'en')

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(notify, range(2)))
    assert sorted(map(len, results)) == [0, 1]
    assert ignored_hint_notices(store, task_id, 'ru') == []
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? "
                          "AND type='MODEL_HINT_IGNORED_SHOWN'", (task_id,)).fetchone()[0] == 1


def test_old_notice_is_immutable_and_a_new_attempt_gets_its_own_notice(routed_task):
    from cortex_runtime.orchestration.state import event
    store, task_id = routed_task
    old_attempt = _audit(store, task_id, 'codex-acp/gpt-6.1-sol', 'ignored:pinned')
    old_payload = dict(attempt_id=old_attempt, hint='codex-acp/gpt-6.1-sol',
                       status='ignored:pinned', used='codex-acp/gpt-6.1-sol')
    with store.connection() as db:
        event(db, task_id, 'MODEL_HINT_IGNORED_SHOWN', old_payload)
        db.commit()
        before = [tuple(row) for row in db.execute(
            "SELECT * FROM runtime_events WHERE task_id=?", (task_id,))]
    assert ignored_hint_notices(store, task_id, 'en') == []
    with store.connection() as db:
        assert [tuple(row) for row in db.execute(
            "SELECT * FROM runtime_events WHERE task_id=?", (task_id,))] == before
    new_attempt = _audit(store, task_id, 'other-engine/model', 'ignored:pinned')
    assert new_attempt != old_attempt
    assert len(ignored_hint_notices(store, task_id, 'en')) == 1
    assert ignored_hint_notices(store, task_id, 'en') == []


def test_failed_notice_transaction_can_retry_without_a_false_shown_marker(routed_task, monkeypatch):
    from cortex_runtime.orchestration import route_notices
    store, task_id = routed_task
    _audit(store, task_id, 'other-engine/model', 'ignored:pinned')
    original = route_notices.event

    def fail_after_insert(*args, **kwargs):
        original(*args, **kwargs)
        raise RuntimeError('test rollback')

    with monkeypatch.context() as context:
        context.setattr(route_notices, 'event', fail_after_insert)
        with pytest.raises(RuntimeError, match='test rollback'):
            ignored_hint_notices(store, task_id, 'en')
    assert len(ignored_hint_notices(store, task_id, 'en')) == 1
    assert ignored_hint_notices(store, task_id, 'en') == []
