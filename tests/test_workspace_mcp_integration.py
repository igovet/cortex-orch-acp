"""Retired integration/delegation APIs cannot bypass scoped runtime writes.

Scope conflicts and revision drift are exercised through the gateway and the
complete runtime cycle in test_runtime_gateway and test_runtime_task_machine.
"""
import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.contracts import schemas_for
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel


@pytest.mark.parametrize('operation', ['delegate', 'integrate', 'decide', 'publish_result', 'assess_check', 'complete_assignment'])
def test_retired_model_operation_cannot_create_a_compatibility_route(tmp_path, operation):
    target=tmp_path/'shared.txt';target.write_text('user edit')
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id=operation),'Inspect','owner')
    TaskMachine(store).create(task.task_id,'Inspect')
    channel=resolve_channel(store,task.channel_id,task.secret)
    assert operation not in schemas_for('coordinator')
    assert operation not in schemas_for('worker')
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store,channel,operation,dict(request_key='retired',assignment_ref='invented'))
    assert target.read_text()=='user edit'
    assert TaskMachine(store).context(task.task_id)['state']=='PLANNING'
