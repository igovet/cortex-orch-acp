"""A model hint the router ignored is said in the chat once (2026-10-22: an
audit the owner assigned to Opus ran on Sol, visible only in the route audit)."""
import json
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.route_notices import ignored_hint_notices
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _audit(store, task_id, hint, status, engine='codex-acp', model='gpt-6.1-sol'):
    with store.connection() as db:
        db.execute("INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   ('ra_' + uuid.uuid4().hex[:12], 'xa_' + uuid.uuid4().hex[:12], task_id, 'worker-code-reviewer',
                    engine, 'cn_1', model, 'fp', None, 'authorized',
                    json.dumps(dict(route_decision=dict(model_hint=hint, model_hint_status=status))), None,
                    '2026-10-22T12:59:20+00:00'))
        db.commit()


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
