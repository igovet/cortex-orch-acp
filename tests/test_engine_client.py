"""EngineClient: JSON-RPC call/notify, malformed-peer safety, timeouts, EOF,
late/duplicate response handling, and agent-to-client request refusal."""
from __future__ import annotations

import json
import time

import pytest

from cortex_runtime.engine import stub_handlers
from cortex_runtime.engine.client import EngineClient, EngineProtocolError, EngineTimeout, EngineTransportClosed
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.process import EngineProcess

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


def _make_client(tmp_path, scenario, *, on_update=None, on_client_request=None, on_unknown_notification=None):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    events = []

    def default_update(params):
        events.append(params)

    def default_client_request(method, params, request_id):
        code, message = stub_handlers.unsupported_client_request_error(method)
        client.reply_error(request_id, code, message)
        events.append(("client_request", method))

    def default_unknown(method, params):
        events.append(("unknown_notification", method))

    client = EngineClient(
        process, correlation=CorrelationTables(),
        on_update=on_update or default_update,
        on_client_request=on_client_request or default_client_request,
        on_unknown_notification=on_unknown_notification or default_unknown,
        logs=process.logs,
    ).start()
    return process, client, events


def test_call_and_result(tmp_path):
    process, client, _ = _make_client(tmp_path, default_scenario())
    try:
        result = client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        assert result["agentCapabilities"]["loadSession"] is False
    finally:
        client.close()
        process.release(grace=2.0)


def test_malformed_peer_output_is_skipped_safely(tmp_path):
    scenario = default_scenario(malformed_mode="non_json")
    process, client, _ = _make_client(tmp_path, scenario)
    try:
        # The malformed line is written before the real reply; the client
        # must still receive the real initialize result afterward.
        result = client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        assert "agentCapabilities" in result
        observations = (process.logs.directory / "observations.jsonl").read_text()
        assert "malformed_peer" in observations
    finally:
        client.close()
        process.release(grace=2.0)


@pytest.mark.parametrize("mode", ["non_json", "batch", "oversize", "nesting_bomb", "invalid_utf8"])
def test_every_malformed_mode_never_crashes_the_client(tmp_path, mode):
    scenario = default_scenario(malformed_mode=mode)
    process, client, _ = _make_client(tmp_path, scenario)
    try:
        result = client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        assert "agentCapabilities" in result
    finally:
        client.close()
        process.release(grace=2.0)


def test_unsupported_agent_to_client_request_gets_dash32601(tmp_path):
    scenario = default_scenario(
        prompt_behavior="client_request",
        client_request=dict(method="request_permission", params={"toolCallId": "tc-1"}),
    )
    process, client, events = _make_client(tmp_path, scenario)
    try:
        client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        session = client.call("session/new", dict(cwd=str(tmp_path), mcpServers=[]), timeout=5.0)
        result = client.call(
            "session/prompt", dict(sessionId=session["sessionId"], prompt=[dict(type="text", text="hi")]), timeout=5.0,
        )
        assert result["stopReason"] == "end_turn"
        assert ("client_request", "request_permission") in events
        observations = (process.logs.directory / "observations.jsonl").read_text()
        assert "unexpected_client_request" not in observations  # logged by session_manager, not the raw client
    finally:
        client.close()
        process.release(grace=2.0)


def test_timeout_then_late_reply_is_recognized_and_dropped(tmp_path):
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="a", options=[dict(value="a", name="A")])],
        configure_behavior="accept",
        prompt_behavior="late_reply",
        late_reply_delay=1.0,
    )
    process, client, _ = _make_client(tmp_path, scenario)
    try:
        client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        session = client.call("session/new", dict(cwd=str(tmp_path), mcpServers=[]), timeout=5.0)
        with pytest.raises(EngineTimeout):
            client.call(
                "session/prompt", dict(sessionId=session["sessionId"], prompt=[dict(type="text", text="hi")]),
                timeout=0.2,
            )
        # Give the fake engine time to actually send its late reply.
        time.sleep(1.5)
        observations = (process.logs.directory / "observations.jsonl").read_text()
        assert "late_or_duplicate_response" in observations
    finally:
        client.close()
        process.release(grace=2.0)


def test_transport_eof_raised_on_process_exit(tmp_path):
    scenario = default_scenario(prompt_behavior="crash")
    process, client, _ = _make_client(tmp_path, scenario)
    try:
        client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        session = client.call("session/new", dict(cwd=str(tmp_path), mcpServers=[]), timeout=5.0)
        with pytest.raises(EngineTransportClosed):
            client.call(
                "session/prompt", dict(sessionId=session["sessionId"], prompt=[dict(type="text", text="hi")]),
                timeout=5.0,
            )
        assert process.wait_exited(5.0)
    finally:
        client.close()
        process.release(grace=2.0)


# --- F4 (T012 review) coverage -------------------------------------------

def test_response_to_truly_unknown_id_is_dropped_with_observation(tmp_path):
    scenario = default_scenario(send_unsolicited_response_first=True)
    process, client, _ = _make_client(tmp_path, scenario)
    try:
        result = client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=5.0)
        assert "agentCapabilities" in result  # the real reply still matched correctly
        observations = (process.logs.directory / "observations.jsonl").read_text()
        assert "unknown_response_id" in observations
    finally:
        client.close()
        process.release(grace=2.0)
