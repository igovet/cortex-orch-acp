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

import pytest

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
            from runtime_scenarios import propose_inspection
            propose_inspection(store, channel)
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='RECONCILING':
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Worker and runtime checks observed.'))
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        from runtime_scenarios import publish_result
        with store.connection() as db:
            role=db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',(channel.attempt_id,)).fetchone()[0]
        if role=='worker-code-reviewer': return publish_result(store,channel)
        outcome=on_worker_prompt(store, channel, session_ref, call_index)
        publish_result(store,channel)
        return outcome

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    return router


class _Harness2(_Harness):
    """Same real Agent/pipe harness as the elicitation tests, but able to
    reach into the runtime this fix threads real requesters into."""


@pytest.mark.parametrize("kind", ["outside_write", "outside_command"])
@pytest.mark.parametrize("mode,answer", [("guarded", "allow"), ("guarded", "deny"), ("auto", None)])
def test_outside_consent_requests_host_only_in_guarded_over_real_acp_connection(tmp_path, kind, mode, answer):
    from cortex_runtime.configuration import access_mode
    from cortex_runtime.orchestration import consent

    observed = {}

    def on_worker_prompt(store, channel, session_ref, call_index):
        if call_index >= 1:
            return PromptOutcome(StopReason.END_TURN, "Done.")
        access_mode.switch(store, channel.task_id, mode, origin="acp_set_mode")
        with store.connection() as db:
            consent._ensure(db)
            db.execute("INSERT INTO consent_requests VALUES(?,?,?,?,?,?,?,NULL)",
                       ("cr_wire", channel.task_id, channel.attempt_id, kind, "/outside/target", "pending", "now"))
            db.commit()
        row, = consent._claim_pending(store, channel.task_id)
        consent._ask(harness.runtime, channel.task_id, row, "en")
        with store.connection() as db:
            observed["status"] = db.execute("SELECT status FROM consent_requests WHERE id='cr_wire'").fetchone()[0]
        return PromptOutcome(StopReason.END_TURN, "Done.")

    harness = _Harness2(tmp_path, _delegate_and_wait_router(on_worker_prompt))
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Do it.")]))
        if mode == "guarded":
            request = harness.wait_for(lambda m: m.get("method") == "session/request_permission")
            assert request["params"]["sessionId"] == session_id
            assert request["params"]["toolCall"]["toolCallId"] == "cortex-consent:cr_wire"
            assert request["params"]["toolCall"]["rawInput"]["kind"] == kind
            harness.send_raw(dict(jsonrpc="2.0", id=request["id"],
                                  result={"outcome": {"outcome": "selected", "optionId": answer}}))
        assert harness.response_for(prompt_id)["result"]["stopReason"] == "end_turn"
        assert observed["status"] == ("approved" if mode == "auto" or answer == "allow" else "denied")
        if mode == "auto":
            with harness.stdout._lock:
                assert not any(m.get("method") == "session/request_permission" for m in harness.stdout.lines)
    finally:
        harness.stop()


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
        # A worker that ends without a durable result now receives one bounded
        # same-session completion follow-up.  The permission round-trip belongs
        # to the original turn; the follow-up should finish without issuing a
        # second unrelated permission request (and overwriting this assertion
        # with the follow-up timeout).
        if call_index >= 1:
            return PromptOutcome(StopReason.END_TURN, "Worker A finished: done")
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


def test_permission_requester_survives_coordinator_turn_for_background_worker(tmp_path):
    """A worker can ask the user after the coordinator yielded.

    ``start_prompt`` returning only ends the coordinator turn; it does not
    close the ACP connection or the task.  The old per-turn cleanup removed
    the requester here, causing a worker permission request to silently
    select ``reject_once``.  The requester is released by connection close
    or a terminal task transition instead.
    """
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
        task_id = task_id_holder["task_id"]
        requester = harness.runtime.permission_requester_for_task(task_id)
        assert requester is not None

        result_holder = {}
        request_thread = threading.Thread(
            target=lambda: result_holder.setdefault("result", requester(
                "es_late_worker_session",
                dict(sessionId="unused", toolCall=dict(toolCallId="tc-late", kind="delete"),
                     options=[dict(optionId="allow", kind="allow_once"),
                              dict(optionId="deny", kind="reject_once")]),
                5.0,
            )),
            daemon=True,
        )
        request_thread.start()
        request = harness.wait_for(lambda m: m.get("method") == "session/request_permission")
        assert request["params"]["sessionId"] == session_id
        harness.send_raw(dict(jsonrpc="2.0", id=request["id"],
                              result={"outcome": {"outcome": "selected", "optionId": "deny"}}))
        request_thread.join(2.0)
        assert result_holder["result"] == {"outcome": {"outcome": "selected", "optionId": "deny"}}

        # The terminal/connection cleanup path still releases the closure.
        harness.runtime._release_task_host_requesters(task_id)
        assert harness.runtime.permission_requester_for_task(task_id) is None
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
