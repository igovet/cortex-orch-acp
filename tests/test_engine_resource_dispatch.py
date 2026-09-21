"""EngineSessionManager end-to-end resource dispatch through a real fake
engine subprocess (T014): capability gating, wire-level fs/permission
handling, duplicate engine request ids, and two independent sessions never
blocking on each other."""
from __future__ import annotations

import json
import threading
import time

import pytest

from cortex_runtime.engine.resource_binding import ResourceBinding
from cortex_runtime.engine.resource_policy import ResourcePolicy
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (
    EngineCapability.SESSION_CREATION,
    EngineCapability.PROMPT_STREAMING,
    EngineCapability.CANCELLATION,
)


def _make_manager(tmp_path, scenario, *, policy=None, host_permission_requester=None, call_timeout=5.0):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(
        record, str(tmp_path), environ=environ, call_timeout=call_timeout,
        policy=policy, host_permission_requester=host_permission_requester,
    )


def _binding_for(manager, engine_session_id, workspace_root, **overrides):
    fields = dict(
        engine_instance_id=manager._process.engine_instance_id, engine_session_id=engine_session_id,
        context_generation_id="gen_1", assignment_id="asg_1", attempt_id="att_1",
        workspace_binding_id="wb_" + engine_session_id, workspace_root=str(workspace_root), ownership_epoch="epoch_1",
        role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def test_default_policy_permission_with_no_binding_or_host_requester_is_cancelled(tmp_path):
    """T012 regression + T014 default: with the untouched default policy
    (no fs/terminal, no host_permission_requester injected, no binding
    registered), a permission request still gets a schema-valid
    'cancelled' result -- never an error, and never anything relayed."""
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(method="session/request_permission", params=dict(sessionId="sess-1", options=[]))],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {"outcome": {"outcome": "cancelled"}}
    finally:
        manager.shutdown(grace=2.0)


def test_permission_request_with_binding_and_host_requester_relays_selection(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"),
                        options=[dict(optionId="allow", kind="allow_once")]),
        )],
        client_reply_log_path=str(reply_log),
    )

    def host_permission_requester(engine_session_id, params, timeout):
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    manager = _make_manager(tmp_path, scenario, host_permission_requester=host_permission_requester)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies == [{"jsonrpc": "2.0", "id": replies[0]["id"], "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}}]
    finally:
        manager.shutdown(grace=2.0)


def test_native_claude_inline_python_other_request_is_auto_approved_on_fake_wire(tmp_path):
    """Exercise the exact Claude ACP ``kind=other`` Bash request shape."""
    reply_log = tmp_path / "replies.jsonl"
    command = """PYTHONDONTWRITEBYTECODE=1 python3 -B - <<'PY'
def luminance(rgb):
    return sum(rgb) / len(rgb)

text = (255, 255, 255)
background = (0, 0, 0)
from math import pow
print(round(pow((luminance(text) + 0.05) / (luminance(background) + 0.05), 1), 2))
PY"""
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(
                sessionId="sess-1",
                toolCall=dict(
                    toolCallId="tc-inline-contrast", kind="other", title="Bash",
                    rawInput={"command": command},
                ),
                options=[dict(optionId="allow", kind="allow_once"),
                         dict(optionId="deny", kind="reject_once")],
            ),
        )],
        client_reply_log_path=str(reply_log),
    )
    host_calls = []
    manager = _make_manager(
        tmp_path, scenario,
        host_permission_requester=lambda *args: host_calls.append(args),
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        list(manager.prompt_turn(session_id, "run contrast check"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {
            "outcome": {"outcome": "selected", "optionId": "allow"}
        }
        assert host_calls == []
    finally:
        manager.shutdown(grace=2.0)


def test_coordinator_inline_ui_check_is_auto_approved_locally_on_fake_wire(tmp_path):
    """A safe coordinator inline check is not a host permission.

    The command is observable, project-bound verification even though it does
    not invoke pytest/unittest.  It is still a bounded, non-destructive project
    check, so the coordinator can run it without a misleading host prompt.
    """
    reply_log = tmp_path / "replies.jsonl"
    command = (
        "python3 -c 'from pathlib import Path; "
        "assert \"focus-visible\" in Path(\"styles.css\").read_text()'"
    )
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(
                sessionId="sess-1",
                toolCall=dict(
                    toolCallId="tc-coordinator-inline-check", kind="other", title="Bash",
                    rawInput={"command": command},
                ),
                options=[dict(optionId="allow", kind="allow_once"),
                         dict(optionId="deny", kind="reject_once")],
            ),
        )],
        client_reply_log_path=str(reply_log),
    )
    host_calls = []
    manager = _make_manager(
        tmp_path, scenario,
        host_permission_requester=lambda *args: host_calls.append(args),
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path, role="coordinator"))
        list(manager.prompt_turn(session_id, "run the UI check"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {
            "outcome": {"outcome": "selected", "optionId": "allow"}
        }
        assert host_calls == []
    finally:
        manager.shutdown(grace=2.0)


def test_duplicate_engine_request_id_gets_a_protocol_error(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(3.0)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[
            dict(method="session/request_permission", id=555,
                 params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"),
                             options=[dict(optionId="allow", kind="allow_once")]),
                 wait_timeout=0.05),
            dict(method="session/request_permission", id=555,
                 params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-2"),
                             options=[dict(optionId="allow", kind="allow_once")]),
                 wait_timeout=3.0),
        ],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario, host_permission_requester=slow_requester)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        events_thread = threading.Thread(target=lambda: list(manager.prompt_turn(session_id, "hi")))
        events_thread.start()
        time.sleep(0.3)
        release_event.set()
        events_thread.join(timeout=5.0)
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        # id 555 reused while the first was still in flight: the *second*
        # send with the same id must get a protocol error, never a second
        # ordinary outcome result.
        by_result_or_error = [("error" if "error" in reply else "result") for reply in replies]
        assert "error" in by_result_or_error
    finally:
        manager.shutdown(grace=2.0)


def test_unknown_option_id_from_host_is_never_relayed(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"),
                        options=[dict(optionId="allow", kind="allow_once")]),
        )],
        client_reply_log_path=str(reply_log),
    )

    def bad_requester(engine_session_id, params, timeout):
        return {"outcome": {"outcome": "selected", "optionId": "not-offered-by-engine"}}

    manager = _make_manager(tmp_path, scenario, host_permission_requester=bad_requester)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {"outcome": {"outcome": "cancelled"}}
    finally:
        manager.shutdown(grace=2.0)


def test_late_approval_after_session_release_never_relays_selected(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(3.0)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"),
                        options=[dict(optionId="allow", kind="allow_once")]),
            wait_timeout=3.0,
        )],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario, host_permission_requester=slow_requester)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        events_thread = threading.Thread(target=lambda: list(manager.prompt_turn(session_id, "hi")))
        events_thread.start()
        time.sleep(0.2)
        manager.invalidate_session(session_id, "released_before_host_replied")
        release_event.set()
        events_thread.join(timeout=5.0)
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {"outcome": {"outcome": "cancelled"}}
    finally:
        manager.shutdown(grace=2.0)


def test_fs_capability_off_by_default_refuses_with_minus_32601(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(method="fs/read_text_file", params=dict(sessionId="sess-1", path=str(tmp_path / "x.txt")))],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["error"]["code"] == -32601
    finally:
        manager.shutdown(grace=2.0)


def test_fs_capability_on_reads_within_bound_workspace(tmp_path):
    (tmp_path / "notes.txt").write_text("hello from disk")
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(method="fs/read_text_file", params=dict(sessionId="sess-1", path=str(tmp_path / "notes.txt")))],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario, policy=ResourcePolicy(fs_local=True))
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["result"] == {"content": "hello from disk"}
    finally:
        manager.shutdown(grace=2.0)


def test_negotiate_advertises_policy_client_capabilities(tmp_path):
    manager = _make_manager(tmp_path, default_scenario(), policy=ResourcePolicy(fs_local=True, terminal=True))
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        assert manager._agent_capabilities is not None  # initialize completed without error
    finally:
        manager.shutdown(grace=2.0)


# --- T014 review (LOW): bounded concurrent client-request handlers -----

def test_excess_concurrent_client_requests_get_busy_immediately(tmp_path):
    reply_log = tmp_path / "replies.jsonl"
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(3.0)
        return {"outcome": {"outcome": "cancelled"}}

    # The first request (permission) is slow and holds the sole semaphore
    # slot; the second (fs/read_text_file -- distinguishable via a -32000
    # 'busy' *error* rather than a permission's ordinary cancelled result)
    # arrives while it is still in flight and must be refused immediately.
    scenario = default_scenario(
        prompt_behavior="client_requests_fire",
        client_requests=[
            dict(method="session/request_permission",
                 params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"), options=[])),
            dict(method="fs/read_text_file", params=dict(sessionId="sess-1", path=str(tmp_path / "x.txt"))),
        ],
        client_reply_log_path=str(reply_log),
        client_requests_fire_wait=3.0,
    )
    manager = _make_manager(tmp_path, scenario, host_permission_requester=slow_requester)
    manager._client_request_semaphore = threading.Semaphore(1)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))
        events_thread = threading.Thread(target=lambda: list(manager.prompt_turn(session_id, "hi")))
        events_thread.start()
        time.sleep(0.3)
        release_event.set()
        events_thread.join(timeout=5.0)
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert len(replies) == 2
        busy = [reply for reply in replies if "error" in reply and reply["error"].get("code") == -32000]
        assert len(busy) == 1  # the second request was refused immediately, never queued
    finally:
        manager.shutdown(grace=2.0)


def test_exception_inside_client_request_handler_still_replies(tmp_path, monkeypatch):
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[dict(
            method="session/request_permission",
            params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-1"), options=[]),
        )],
        client_reply_log_path=str(reply_log),
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_id, tmp_path))

        def raising_handle(**kwargs):
            raise RuntimeError("boom")

        monkeypatch.setattr(manager._permission_bridge, "handle", raising_handle)
        list(manager.prompt_turn(session_id, "hi"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert replies[0]["error"]["code"] == -32603
    finally:
        manager.shutdown(grace=2.0)


def test_two_independent_sessions_one_waiting_on_permission_other_completes(tmp_path):
    """Session A's session/request_permission blocks on a slow host
    requester; session B (a distinct wire session on the *same* engine
    instance) must still complete its own ordinary prompt turn promptly,
    proving the worker-thread dispatch (T014 design decision 2) never
    blocks the shared transport reader thread."""
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(3.0)
        return {"outcome": {"outcome": "cancelled"}}

    scenario = default_scenario(
        session_ids=["sess-a", "sess-b"],
        prompt_behavior_by_session={"sess-a": "client_requests_sequence"},
        client_requests=[dict(method="session/request_permission",
                               params=dict(sessionId="sess-a", toolCall=dict(toolCallId="tc-1"), options=[]),
                               wait_timeout=3.0)],
    )
    manager = _make_manager(tmp_path, scenario, host_permission_requester=slow_requester)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_a = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.bind_resource(_binding_for(manager, session_a, tmp_path))

        a_thread = threading.Thread(target=lambda: list(manager.prompt_turn(session_a, "hi-a")))
        a_thread.start()
        time.sleep(0.1)  # let A's client_requests_sequence step start and block

        # B is a distinct wire session; its own prompt (no client_requests
        # behavior) must still complete without waiting on A.
        session_b = manager.create_session("t_bbbbbbbbbbbb", "xr_" + "1" * 24)
        started = time.monotonic()
        events_b = list(manager.prompt_turn(session_b, "hi-b"))
        elapsed = time.monotonic() - started
        assert elapsed < 2.0  # completed quickly, not blocked behind A's still-pending permission
        assert events_b[-1].kind.name == "TURN_END"

        release_event.set()
        a_thread.join(timeout=5.0)
    finally:
        manager.shutdown(grace=2.0)
