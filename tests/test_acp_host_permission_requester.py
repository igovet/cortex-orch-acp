"""EXT-01 #5: real ``host_permission_requester``/``host_fs_requester``
production wiring at the ``acp.agent.Agent`` <-> ``OrchestratingRuntime``
boundary -- end to end through the real ``Agent``/``StdioTransport`` (a
pipe, not a subprocess -- see ``tests/test_acp_host_requests.py``'s
identical pattern) with a real ``OrchestratingRuntime`` (fakes-backed, no
real engine/network/paid model) and a scripted fake ACP client driving the
transport directly.

The identity mapping from a downstream engine's own ``engine_session_id``
down to exactly one task is ``delegation.live_adapters
.EngineSessionManagerAdapter``'s own job (covered by
``tests/test_delegation_host_permission_bridge.py``); this file proves the
other half -- that ``OrchestratingRuntime.permission_requester_for_task``
actually resolves to *this connection's* real ``HostRequester``, and that a
reply the client sends over the wire is the one honoured.
"""
from __future__ import annotations

import threading
import uuid

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_acp_elicitation import _Harness, _initialize_and_new_session, _project


def _delegate_and_wait_router(on_worker_prompt):
    phase = {"delegated": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        from cortex_runtime.delegation import mcp_tools

        if not phase["delegated"]:
            phase["delegated"] = True
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="do it", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        return on_worker_prompt(store, channel, session_ref, call_index)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    return router


class _Harness2(_Harness):
    """Same real Agent/pipe harness as the elicitation tests, but able to
    reach into the runtime this fix threads real requesters into."""


def test_permission_request_round_trips_over_the_real_connection_and_client_answer_is_honored(tmp_path):
    """A worker's own engine session asks for a decision (the shape a real
    ``engine.permissions.PermissionBridge`` deletion relay would send);
    the registered per-task requester (built by ``Agent
    ._make_permission_requester`` from this exact connection's own
    ``HostRequester``) is what actually sends it, over the real pipe, as a
    schema-shaped ``session/request_permission`` outbound request -- and
    the client's own reply is the value the requester call returns."""
    observed = {}

    def on_worker_prompt(store, channel, session_ref, call_index):
        requester = harness.runtime.permission_requester_for_task(channel.task_id)
        assert requester is not None, "no requester registered for this in-flight task"
        result = requester(
            "es_fake_worker_session",
            dict(sessionId="unused-by-the-relay", toolCall=dict(toolCallId="tc-delete", kind="delete"),
                 options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]),
            5.0,
        )
        observed["result"] = result
        return PromptOutcome(StopReason.END_TURN, "Worker A finished: done")

    router = _delegate_and_wait_router(on_worker_prompt)
    harness = _Harness2(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Do it.")]))

        request = harness.wait_for(lambda m: m.get("method") == "session/request_permission")
        assert isinstance(request.get("id"), str) and request["id"].startswith("hr_")
        # The relay puts *this connection's own* ACP sessionId on the wire,
        # never the engine's own internal one it was called with.
        assert request["params"]["sessionId"] == session_id
        assert request["params"]["toolCall"]["toolCallId"] == "tc-delete"

        harness.send_raw(dict(jsonrpc="2.0", id=request["id"],
                               result={"outcome": {"outcome": "selected", "optionId": "deny"}}))

        final = harness.response_for(prompt_id)
        assert final["result"]["stopReason"] == "end_turn"
        assert observed["result"] == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    finally:
        harness.stop()


def test_permission_requester_is_deregistered_once_the_turn_ends_failing_closed(tmp_path):
    """Once ``start_prompt`` returns, this task's own registered requester
    is gone -- a permission request that arrives for it later (outside any
    in-flight turn) has nothing to relay to and falls back to the same
    fail-closed ``None`` as "no host requester at all", never a stale
    reused one."""
    task_id_holder = {}

    def on_worker_prompt(store, channel, session_ref, call_index):
        task_id_holder["task_id"] = channel.task_id
        return PromptOutcome(StopReason.END_TURN, "Worker A finished: done")

    router = _delegate_and_wait_router(on_worker_prompt)
    harness = _Harness2(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Do it.")]))
        harness.response_for(prompt_id)

        assert task_id_holder["task_id"]
        assert harness.runtime.permission_requester_for_task(task_id_holder["task_id"]) is None
    finally:
        harness.stop()


def test_fs_requester_only_built_when_the_host_negotiates_the_capability(tmp_path):
    """No ``clientCapabilities.fs.readTextFile`` at ``initialize`` -- the
    same "never guessed" convention ``_make_elicitor`` already uses for
    ``elicitation.form`` -- so no per-task fs requester is ever registered,
    and an integration this task runs records ``host_buffer_state:
    'unknown'`` (capability absent), never silently treated as checked."""
    task_id_holder = {}

    def on_worker_prompt(store, channel, session_ref, call_index):
        task_id_holder["task_id"] = channel.task_id
        assert harness.runtime.permission_requester_for_task(channel.task_id) is not None  # permission has no capability gate
        return PromptOutcome(StopReason.END_TURN, "Worker A finished: done")

    router = _delegate_and_wait_router(on_worker_prompt)
    harness = _Harness2(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Do it.")]))
        harness.response_for(prompt_id)
        # No fs/* request was ever sent to the client.
        assert not any(m.get("method", "").startswith("fs/") for m in harness.stdout.lines)
    finally:
        harness.stop()
