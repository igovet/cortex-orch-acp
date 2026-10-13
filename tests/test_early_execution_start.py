"""An accepted plan starts its workers while the coordinator's turn is still
streaming its closing text, not after that turn ends (live: 2-3 s per plan)."""
import threading

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.portable.engine_contract import StopReason


def test_workers_start_before_the_planning_turn_ends(tmp_path):
    from runtime_scenarios import propose_inspection, publish_result
    from test_acp_elicitation import _Harness, _project
    from cortex_runtime.delegation import mcp_tools

    worker_started = threading.Event()
    observed = {}

    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call):
        if 'planned' not in observed:
            observed['planned'] = True
            propose_inspection(store, channel)
            # The engine reports the plan_submit call completed, then keeps
            # writing its closing text.
            on_tool_call('plan', 'other', 'mcp.cortex.plan_submit', 'completed', None, None, None, None)
            observed['started_during_turn'] = worker_started.wait(20)
            return PromptOutcome(StopReason.END_TURN, 'Delegated.')
        context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
        if context['state'] == 'RECONCILING':
            mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Checked.'))
        return PromptOutcome(StopReason.END_TURN, '')

    def worker(store, channel, session_ref, message, call_index, *, on_delta, on_tool_call):
        worker_started.set()
        return publish_result(store, channel)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator if channel and channel.role == 'coordinator' else worker

    harness = _Harness(tmp_path, router)
    harness.start()
    try:
        init_id = harness.send('initialize', dict(protocolVersion=1, clientCapabilities={},
                                                  clientInfo=dict(name='paseo', version='test')))
        harness.response_for(init_id)
        new_id = harness.send('session/new', dict(cwd=str(_project(tmp_path)), mcpServers=[]))
        session_id = harness.response_for(new_id)['result']['sessionId']
        prompt_id = harness.send('session/prompt', dict(sessionId=session_id,
                                                        prompt=[dict(type='text', text='Inspect it.')]))
        harness.response_for(prompt_id, timeout=60.0)
    finally:
        harness.stop()
    assert observed.get('started_during_turn') is True
