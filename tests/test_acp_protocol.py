"""Protocol-level tests: initialize, session/new, session/prompt, session/cancel.

Covers T009 acceptance: "A local ACP test client can initialize, create a
Cortex session, submit a source event, receive streamed updates, and cancel
with valid protocol semantics."
"""
import json
import os
import sqlite3

import pytest

from acp_fakes import spawn_fake_agent, spawn_production_agent


def _send(proc, message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _recv(proc):
    return json.loads(proc.stdout.readline())


def test_initialize_session_prompt_stream_and_cancel(tmp_path):
    proc = spawn_fake_agent(chunks=("a", "b", "c"), delay=0.05)
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="test-client", version="1"))))
        init_reply = _recv(proc)
        assert init_reply["result"]["protocolVersion"] == 1
        assert init_reply["result"]["agentInfo"]["name"] == "cortex"

        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        assert session_id.startswith("acps_")

        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id,
                                      prompt=[dict(type="text", text="Build a greeting.")])))
        update1 = _recv(proc)
        assert update1["method"] == "session/update"
        assert update1["params"]["sessionId"] == session_id
        assert update1["params"]["update"]["sessionUpdate"] == "agent_message_chunk"

        _send(proc, dict(jsonrpc="2.0", method="session/cancel", params=dict(sessionId=session_id)))

        final = None
        for _ in range(10):
            item = _recv(proc)
            if item.get("id") == 3:
                final = item
                break
        assert final is not None
        assert final["result"]["stopReason"] == "cancelled"
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    tasks = db.execute("SELECT id FROM tasks").fetchall()
    assert len(tasks) == 1
    sessions = db.execute("SELECT session_id, task_id FROM acp_host_sessions").fetchall()
    assert sessions == [(session_id, tasks[0][0])]


def test_methods_before_initialize_are_rejected(tmp_path):
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        reply = _recv(proc)
        assert reply["error"]["code"] == -32600
        _send(proc, dict(jsonrpc="2.0", id=2, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        reply2 = _recv(proc)
        assert reply2["result"]["protocolVersion"] == 1
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_session_new_requires_absolute_canonical_existing_directory(tmp_path):
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        _recv(proc)
        missing = str(tmp_path / "does-not-exist")
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=missing, mcpServers=[])))
        reply = _recv(proc)
        assert reply["error"]["code"] == -32602
        relative = "relative/path"
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/new",
                          params=dict(cwd=relative, mcpServers=[])))
        reply2 = _recv(proc)
        assert reply2["error"]["code"] == -32602
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_unknown_method_with_id_and_notification(tmp_path):
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        _recv(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="totally/unknown"))
        reply = _recv(proc)
        assert reply["error"]["code"] == -32601
        # A notification for an unknown method is ignored (no reply at all);
        # confirm the connection is still alive by sending one more request.
        _send(proc, dict(jsonrpc="2.0", method="totally/unknown/notification"))
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        reply2 = _recv(proc)
        assert reply2["result"]["sessionId"].startswith("acps_")
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0
