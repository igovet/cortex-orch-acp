"""Capability-accuracy tests.

Covers T009 acceptance: "Unsupported optional capabilities are not invoked;
extensions are namespaced/versioned and separately negotiated," and the
non-negotiable "do not mistake a protocol echo fixture for Cortex
orchestration" boundary (no client-request methods -- fs/*, terminal/*,
request_permission -- are ever emitted).
"""
import json

from acp_fakes import spawn_fake_agent, spawn_production_agent


def _send(proc, message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _recv(proc):
    return json.loads(proc.stdout.readline())


def _initialize(proc):
    _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                      params=dict(protocolVersion=1, clientCapabilities={},
                                  clientInfo=dict(name="t", version="1"))))
    reply = _recv(proc)
    caps = reply["result"]["agentCapabilities"]
    # T018: loadSession/embeddedContext are now truthfully advertised --
    # session/load (display-only replay) and EmbeddedResource prompt
    # content blocks are implemented; see
    # docs/acp/input-history-and-replay.md.
    assert caps["loadSession"] is True
    assert caps["promptCapabilities"] == dict(image=True, audio=False, embeddedContext=True)
    assert caps["mcpCapabilities"] == dict(http=False, sse=False)
    assert reply["result"]["authMethods"] == []
    return reply


def test_unsupported_session_lifecycle_methods_return_method_not_found():
    proc = spawn_production_agent()
    try:
        _initialize(proc)
        for index, method in enumerate((
            # session/set_config_option, session/set_mode (the access
            # mode), and session/load are implemented, so
            # none is in protocol.UNSUPPORTED_METHODS.
            "session/resume",
            "session/list", "session/fork", "session/close",
        ), start=2):
            _send(proc, dict(jsonrpc="2.0", id=index, method=method, params={}))
            reply = _recv(proc)
            assert reply["error"]["code"] == -32601, method
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_image_content_block_rejected_with_invalid_base64(tmp_path):
    proc = spawn_production_agent()
    try:
        _initialize(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _recv(proc)  # available_commands_update notification (T010)
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id,
                                      prompt=[dict(type="image", data="not-base64!", mimeType="image/png")])))
        reply = _recv(proc)
        assert reply["error"]["code"] == -32602
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_oversized_image_is_rejected_before_capture_or_forwarding(tmp_path):
    """An image over the attachment cap fails closed at the ACP boundary.

    In particular, a too-large image must not become an uncaptured raw ACP
    block that can reach an engine session.
    """
    import base64

    proc = spawn_production_agent()
    try:
        _initialize(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _recv(proc)
        oversized = base64.b64encode(b"x" * (1024 * 1024 + 1)).decode("ascii")
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id,
                                      prompt=[dict(type="image", data=oversized, mimeType="image/png")])))
        reply = _recv(proc)
        assert reply["error"]["code"] == -32602
        assert "large" in reply["error"]["message"].lower() or "cap" in reply["error"]["message"].lower()
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_resource_link_and_text_blocks_accepted(tmp_path):
    proc = spawn_fake_agent(chunks=("ok",))
    try:
        _initialize(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _recv(proc)  # available_commands_update notification (T010)
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[
                              dict(type="text", text="Look at this:"),
                              dict(type="resource_link", name="notes.md", uri="file:///tmp/notes.md"),
                          ])))
        update = _recv(proc)
        assert update["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
        final = _recv(proc)
        assert final["id"] == 3 and final["result"]["stopReason"] == "end_turn"
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_agent_never_emits_client_request_methods(tmp_path):
    """No fs/*, terminal/*, or request_permission request/notification is
    ever sent to the client -- there is no client-request code path in
    T009 at all."""
    proc = spawn_fake_agent(chunks=("hello",))
    try:
        _initialize(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id,
                                      prompt=[dict(type="text", text="go")])))
        seen_methods = set()
        while True:
            item = _recv(proc)
            if "method" in item:
                seen_methods.add(item["method"])
            if item.get("id") == 3:
                break
        assert seen_methods == {"session/update"}
        for forbidden_prefix in ("fs/", "terminal/"):
            assert not any(method.startswith(forbidden_prefix) for method in seen_methods)
        assert "request_permission" not in seen_methods
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0
