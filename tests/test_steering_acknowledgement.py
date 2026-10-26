"""A message during busy work is acknowledged at once (2026-10-22: it waited
13-35 minutes for a safe point with no sign it was received)."""
import threading
import uuid

import pytest

from cortex_runtime.acp import steering
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.acp.steering import busy_acknowledgement
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


@pytest.mark.parametrize('state,expected', [('EXECUTING', 'Принято. Учту это после текущего шага.'),
                                            ('REVIEWING', 'Принято. Учту это после текущего шага.'),
                                            ('VERIFYING', 'Принято. Учту это после текущего шага.'),
                                            ('PLANNING', None), ('WAITING_USER', None)])
def test_only_a_busy_task_acknowledges_a_queued_message(tmp_path, state, expected):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=uuid.uuid4().hex), 'Work', 'owner')
    TaskMachine(store).create(task.task_id, 'Work')
    with store.connection() as db:
        db.execute('UPDATE runtime_tasks SET state=? WHERE task_id=?', (state, task.task_id))
        db.commit()
    assert busy_acknowledgement(store, task.task_id, 'ru') == expected


@pytest.mark.parametrize('state', ['EXECUTING', 'REVIEWING', 'VERIFYING', 'PLANNING'])
@pytest.mark.parametrize('display', [True, False])
def test_busy_acknowledgement_ends_before_worker_continuation(tmp_path, monkeypatch, state, display):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=uuid.uuid4().hex), 'Work', 'owner')
    TaskMachine(store).create(task.task_id, 'Work')
    with store.connection() as db:
        db.execute('UPDATE runtime_tasks SET state=? WHERE task_id=?', (state, task.task_id))
        db.commit()
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._pending_recovery_notice = {}
    monkeypatch.setattr(steering, 'orchestration_exists', lambda *_: True)
    monkeypatch.setattr(runtime, '_ensure_coordinator_retrying', lambda *_args, **_kwargs: (None, None))
    monkeypatch.setattr(runtime, '_resolve_language', lambda *_args, **_kwargs: 'ru')
    for name in ('_publish_initial_pipeline', '_emit_pending_pause_notice',
                 '_emit_resume_context', '_recover_unrecorded_source_events'):
        monkeypatch.setattr(runtime, name, lambda *_args, **_kwargs: None)

    events = []
    busy = state != 'PLANNING'

    def continuation(*_args, **_kwargs):
        # No engine message follows the acknowledgement while existing workers
        # run. A future worker start/outcome must not wait for this ACP turn.
        if busy:
            assert events[0] == ('text', '\n\n' + busy_acknowledgement(store, task.task_id, 'ru') + '\n\n')
        if busy and display:
            assert events[-1][0] == 'boundary', 'Busy acknowledgement left worker cards queued'
            marker = events[-1][1]
            assert marker['sessionUpdate'] == 'agent_message_chunk'
            assert marker['messageId'].startswith('__cortex_message_boundary__:')
            assert marker['content'] == dict(type='text', text='')
        else:
            assert len(events) == int(busy)
        return StopReason.END_TURN

    monkeypatch.setattr(runtime, '_continuation_loop', continuation)
    markers = []
    for _ in range(2):
        events.clear()
        assert runtime._run(
            task.task_id, 'session', 'Keep working', lambda text: events.append(('text', text)),
            threading.Event(), source_origin='test', source_event_id=uuid.uuid4().hex,
            emit_display_update=(lambda update: events.append(('boundary', update))) if display else None,
        ) == StopReason.END_TURN
        if busy and display:
            markers.append(events[-1][1]['messageId'])
    assert len(markers) == len(set(markers))
