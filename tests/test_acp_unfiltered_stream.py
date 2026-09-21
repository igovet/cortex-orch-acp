import threading
import inspect
import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime


@pytest.mark.parametrize('title,kind', [
    ('mcp__cortex__read_report','read'), ('mcp.cortex.plan_submit','other'),
    ('Read /project/.cortex/pipeline.md','read'), ('wait','other'),
    ('Guardian Review','think'), ('mcp.cua_repl.js','execute'),
    ('Editing files','edit'), (None,'other'),
])
def test_every_observed_tool_lifecycle_is_forwarded(title,kind):
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    updates = []
    emit = runtime._engine_tool_call_emitter(updates.append)
    emit('exact-id',kind,title,'pending')
    emit('exact-id',kind,title,'completed',raw_input={'command':'printf hello'},raw_output='hello')
    assert len(updates) == 2
    assert updates[0]['new'] is True
    assert updates[1]['new'] is False
    assert updates[0]['tool_call_id'] == updates[1]['tool_call_id']
    assert updates[1]['raw_output'] == 'hello'


def test_status_only_update_preserves_identity_without_inventing_title():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    updates = []
    emit = runtime._engine_tool_call_emitter(updates.append)
    emit('exact-id','read','Read README.md','in_progress')
    emit('exact-id',None,None,'completed')
    assert 'title' not in updates[-1]


def test_production_forwards_display_updates():
    runtime = ProductionOrchestratingRuntime()
    seen = {}
    class Runtime:
        def start_prompt(self,*args,**kwargs):
            seen.update(kwargs)
    runtime._runtime_for_project = lambda _: Runtime()
    output = []
    runtime.start_prompt('t','s','hello',lambda _: True,threading.Event(),
                         project_root='/project',emit_display_update=output.append)
    seen['emit_display_update']({'sessionUpdate':'agent_thought_chunk','content':{'type':'text','text':'Observed thought'}})
    assert len(output) == 1
