"""A message during busy work is acknowledged at once (2026-10-22: it waited
13-35 minutes for a safe point with no sign it was received)."""
import uuid

import pytest

from cortex_runtime.acp.steering import busy_acknowledgement
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
