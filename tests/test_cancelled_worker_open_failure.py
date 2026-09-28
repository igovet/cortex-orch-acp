"""A cancelled assignment's queued open is not also reported as failed.

2026-09-28: a user cancel interrupted a turn with two workers still queued
for engine capacity. Each was cancelled (assignment_cancelled) and its
queued open then ended with the cancel itself (``attempt``) or its revoked
channel (``channel_revoked``); the runtime reported both as
assignment_failed too, so cancelled workers showed as errored.
"""
import json
import threading
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.provisioning import _idempotency_key
from cortex_runtime.orchestration.state import TaskMachine, encoded
from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _runtime(tmp_path, node_status, error):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=str(uuid.uuid4())), 'Audit', 'owner')
    TaskMachine(store).create(task.task_id, 'Audit')
    channel = SimpleNamespace(channel_id='ch_coordinator')
    assignment = _idempotency_key(task.task_id, channel.channel_id, 'runtime:1:audit:0')
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET revision=1,state=? WHERE task_id=?",
                   ('RECOVERING' if node_status == 'cancelled' else 'EXECUTING', task.task_id))
        db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                   (task.task_id, 1, 'audit', 'worker-ux-designer', 'Audit', '[]', encoded(['docs']),
                    'execution', node_status, assignment, None))
        db.commit()

    def delegate(*args):
        raise error

    runtime = SimpleNamespace(_store=store, _provisioning=SimpleNamespace(delegate=delegate))
    runtime._assignment_node_cancelled = lambda task_id, ref: OrchestratingRuntime._assignment_node_cancelled(runtime, task_id, ref)
    payload = dict(assignment_ref=assignment, request_key='runtime:1:audit:0', title='Audit', brief='Audit',
                   role='worker-ux-designer', acceptance='Report')
    return store, task.task_id, runtime, channel, payload


def _failures(store, task_id):
    with store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task_id,))]


def _cancelled_open(kind):
    if kind == 'channel_revoked':
        return StoreError('channel_revoked')
    # What delegate() raises for a queued open cancelled with its attempt.
    return PortableError(category=PortableErrorCategory.TRANSPORT, code=kind, remediation='cancelled',
                         retry_eligible=False)


@pytest.mark.parametrize('kind', ['attempt', 'channel_revoked'])
def test_a_cancelled_assignments_open_failure_is_not_reported_as_failed(tmp_path, kind):
    store, task_id, runtime, channel, payload = _runtime(tmp_path, 'cancelled', _cancelled_open(kind))
    OrchestratingRuntime._provision_and_prompt_inner(runtime, task_id, channel, payload, threading.Event())
    assert _failures(store, task_id) == []


@pytest.mark.parametrize('kind', ['attempt', 'channel_revoked'])
def test_a_running_assignments_open_failure_is_still_reported(tmp_path, kind):
    store, task_id, runtime, channel, payload = _runtime(tmp_path, 'running', _cancelled_open(kind))
    OrchestratingRuntime._provision_and_prompt_inner(runtime, task_id, channel, payload, threading.Event())
    assert [failure['error'] for failure in _failures(store, task_id)] == [kind]
