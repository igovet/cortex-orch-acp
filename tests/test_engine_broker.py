"""Bounded authenticated host MCP broker tests."""
from __future__ import annotations

import subprocess
import sys
import json

import pytest

from cortex_runtime.engine.broker import McpBroker
from cortex_runtime.engine.mcp_bridge import build_mcp_servers


def test_broker_relay_round_trip_and_cleanup(tmp_path):
    script = tmp_path / "fake_server.py"
    script.write_text(
        "import sys\n"
        "for line in sys.stdin:\n"
        " print(line, end='', flush=True)\n",
        encoding="utf-8",
    )
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    broker = McpBroker(("ch_0123456789ab", "secret-token-123456", str(project)),
                       python_executable=sys.executable, server_script=str(script))
    try:
        relay_script = str(tmp_path / "relay.py")
        # Use the production relay module, while the fake server stands in for Cortex.
        from cortex_runtime.engine import broker as broker_module
        relay_script = broker_module.__file__
        proc = subprocess.Popen(
            [sys.executable, relay_script, "--relay", broker.endpoint, broker.relay_token],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        proc.stdin.write(b'{"jsonrpc":"2.0"}\n')
        proc.stdin.flush()
        assert proc.stdout.readline() == b'{"jsonrpc":"2.0"}\n'
        proc.stdin.close()
        proc.wait(timeout=3)
        assert proc.returncode == 0
    finally:
        endpoint = broker.endpoint
        broker.close()
    assert not __import__("os").path.exists(endpoint)


def test_relay_mcp_entry_does_not_forward_credentials():
    result = build_mcp_servers(
        "ch_0123456789ab", "secret-token-123456", "/tmp",
        relay_endpoint="/tmp/cortex.sock", relay_token="one-time-token",
    )[0]
    assert result["env"] == []
    assert "secret-token-123456" not in result["args"]


def test_broker_observes_actual_relay_tools_list_and_child_exit(tmp_path, monkeypatch):
    from cortex_runtime.engine import broker as broker_module
    monkeypatch.setattr(broker_module.secrets, "token_urlsafe", lambda _: "-leading-hyphen-token")
    script = tmp_path / "server.py"
    script.write_text(
        "import json, sys\n"
        "for line in sys.stdin:\n"
        " request = json.loads(line)\n"
        " result = {'tools': [{'name': 'create_draft', 'description': 'secret description', 'inputSchema': {'type': 'object', 'x-secret': 'never-log'}}, {'name': 'write_report', 'inputSchema': {'type': 'object'}}, {'name': 'result_submit', 'inputSchema': {'type': 'object'}}]} if request['method'] == 'tools/list' else {}\n"
        " print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n",
        encoding="utf-8",
    )
    events = []
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable, server_script=str(script),
                       observe=lambda event, **fields: events.append((event, fields)))
    try:
        assert broker.relay_token == "c-leading-hyphen-token"
        assert broker.readiness(0) == "not_observed"
        requests = b"".join((json.dumps({"jsonrpc": "2.0", "id": i, "method": method}) + "\n").encode()
                            for i, method in enumerate(("initialize", "tools/list"), 1))
        proc = subprocess.Popen([sys.executable, __import__("cortex_runtime.engine.broker", fromlist=["__file__"]).__file__,
                                 "--relay", broker.endpoint, broker.relay_token],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        output, _ = proc.communicate(requests, timeout=3)
        assert proc.returncode == 0
        assert [json.loads(line)["id"] for line in output.splitlines()] == [1, 2]
        assert broker.readiness(1) == "ready"
        receipt = broker.tools_receipt
        assert receipt["state"] == "complete"
        assert receipt["tool_names"] == ["create_draft", "result_submit", "write_report"]
        assert receipt["tool_count"] == 3
        assert receipt["missing_required_tools"] == []
        assert receipt["worker_result_submit_present"] is True
        assert all("secret description" not in repr(item) and "never-log" not in repr(item)
                   for item in events)
        assert any(event == "mcp_tools_ready" and fields["tool_count"] == 3
                   and fields["tool_names"] == receipt["tool_names"]
                   for event, fields in events)
        assert any(event == "mcp_server_exited" and fields["exit_code"] == 0
                   for event, fields in events)
    finally:
        broker.close()


def test_broker_fails_closed_on_observed_incomplete_tools_list(tmp_path):
    script = tmp_path / "server.py"
    script.write_text(
        "import json, sys\n"
        "for line in sys.stdin:\n"
        " request = json.loads(line)\n"
        " result = {'tools': [{'name': 'write_report', 'inputSchema': {'type': 'object'}}]} if request['method'] == 'tools/list' else {}\n"
        " print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n",
        encoding="utf-8",
    )
    events = []
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable, server_script=str(script),
                       observe=lambda event, **fields: events.append((event, fields)))
    try:
        proc = subprocess.Popen(
            [sys.executable, __import__("cortex_runtime.engine.broker", fromlist=["__file__"]).__file__,
             "--relay", broker.endpoint, broker.relay_token],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        output, _ = proc.communicate(
            b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}\n', timeout=3,
        )
        assert json.loads(output)["result"]["tools"][0]["name"] == "write_report"
        assert broker.readiness(1) == "mcp_tools_incomplete"
        receipt = broker.tools_receipt
        assert receipt["state"] == "incomplete"
        assert receipt["missing_required_tools"] == ["create_draft"]
        assert receipt["worker_result_submit_present"] is False
        assert any(event == "mcp_tools_incomplete" for event, _ in events)
    finally:
        broker.close()


def test_broker_reports_startup_close_without_claiming_readiness(tmp_path):
    script = tmp_path / "server.py"
    script.write_text("import sys\nsys.exit(23)\n", encoding="utf-8")
    events = []
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable, server_script=str(script),
                       observe=lambda event, **fields: events.append((event, fields)))
    try:
        proc = subprocess.Popen([sys.executable, __import__("cortex_runtime.engine.broker", fromlist=["__file__"]).__file__,
                                 "--relay", broker.endpoint, broker.relay_token],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        proc.communicate(b'{"jsonrpc":"2.0","id":1,"method":"initialize"}\n', timeout=3)
        assert broker.readiness(1) == "mcp_server_exited"
        assert any(event == "mcp_server_exited" and fields["exit_code"] == 23
                   and not fields["tools_ready"] for event, fields in events)
    finally:
        broker.close()


def _crashing_server(tmp_path):
    script = tmp_path / "server.py"
    script.write_text(
        "import json, sys\n"
        "for line in sys.stdin:\n"
        " request = json.loads(line)\n"
        " if request['method'] == 'crash':\n"
        "  sys.exit(1)\n"
        " result = {'tools': [{'name': 'create_draft', 'inputSchema': {'type': 'object'}}, {'name': 'write_report', 'inputSchema': {'type': 'object'}}]} if request['method'] == 'tools/list' else {}\n"
        " print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n",
        encoding="utf-8",
    )
    return script


def _relay(broker):
    return subprocess.Popen(
        [sys.executable, __import__("cortex_runtime.engine.broker", fromlist=["__file__"]).__file__,
         "--relay", broker.endpoint, broker.relay_token],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )


def _await_readiness(broker, expected, seconds=5.0):
    import time
    deadline = time.monotonic() + seconds
    while broker.readiness(0) != expected and time.monotonic() < deadline:
        time.sleep(0.02)
    return broker.readiness(0)


def test_broker_reports_a_ready_server_crash_and_a_fresh_relay_clears_it(tmp_path):
    """A Cortex MCP server that dies after tools/list must not stay "ready".

    The engine's own MCP client cannot reconnect a stdio server, so every
    later Cortex call of that session fails with a closed transport: the
    worker can neither publish its result nor ask a question. Readiness is
    the signal the prompt turn already watches, so the lost server must be
    reported there instead of letting the turn end as if nothing happened.
    """
    events = []
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable, server_script=str(_crashing_server(tmp_path)),
                       observe=lambda event, **fields: events.append((event, fields)))
    try:
        first = _relay(broker)
        first.stdin.write(b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}\n')
        first.stdin.flush()
        assert json.loads(first.stdout.readline())["id"] == 1
        assert broker.readiness(1) == "ready"
        first.stdin.write(b'{"jsonrpc":"2.0","id":2,"method":"crash"}\n')
        first.stdin.flush()
        # The relay sees its stream end: this is the engine's "Transport closed".
        assert first.stdout.read() == b""
        first.stdin.close()
        first.wait(timeout=5)
        assert _await_readiness(broker, "mcp_server_crashed") == "mcp_server_crashed"
        assert any(event == "mcp_server_exited" and fields["exit_code"] == 1 and fields["tools_ready"]
                   for event, fields in events)

        # A fresh relay connection gets a fresh server: the loss is over.
        second = _relay(broker)
        output, _ = second.communicate(b'{"jsonrpc":"2.0","id":3,"method":"initialize"}\n', timeout=5)
        assert json.loads(output)["id"] == 3
        assert second.returncode == 0
    finally:
        broker.close()
    assert any(event == "mcp_relay_connected" for event, _ in events)


def test_broker_ready_server_clean_exit_and_close_are_not_reported_as_a_crash(tmp_path):
    """An engine closing its own MCP client (clean EOF, exit 0) and the
    runtime's own release are not server losses."""
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable, server_script=str(_crashing_server(tmp_path)))
    try:
        output, _ = _relay(broker).communicate(b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}\n', timeout=5)
        assert json.loads(output)["id"] == 1
        assert broker.readiness(1) == "ready"
        import time
        time.sleep(0.3)
        assert broker.readiness(0) == "ready"
        live = _relay(broker)
        live.stdin.write(b'{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n')
        live.stdin.flush()
        assert json.loads(live.stdout.readline())["id"] == 2
    finally:
        broker.close()
    assert broker.readiness(0) == "ready"
    live.stdin.close()
    live.wait(timeout=5)


# These fixtures are fake Python processes, never installed engines or models.
def _replacement_server(tmp_path):
    script = tmp_path / "replacement_server.py"
    script.write_text(
        "import json, sys\n"
        "for line in sys.stdin:\n"
        " request = json.loads(line)\n"
        " if request['method'] == 'crash': sys.exit(7)\n"
        " result = request.get('params', {}).get('reply', {})\n"
        " print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n",
        encoding="utf-8",
    )
    return script


def _exchange(proc, method, result=None, request_id=1):
    request = dict(jsonrpc="2.0", id=request_id, method=method)
    if result is not None:
        request["params"] = dict(reply=result)
    proc.stdin.write((json.dumps(request) + "\n").encode())
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())


def _complete_tools(marker="fresh"):
    return dict(tools=[dict(name=name, inputSchema=dict(type="object", title=marker))
                       for name in ("create_draft", "write_report")])


def _crash_relay(proc, broker):
    proc.stdin.write(b'{"jsonrpc":"2.0","id":99,"method":"crash"}\n')
    proc.stdin.flush()
    assert proc.stdout.read() == b""
    proc.stdin.close()
    proc.wait(timeout=5)
    assert proc.returncode == 0
    assert _await_readiness(broker, "mcp_server_crashed") == "mcp_server_crashed"


@pytest.mark.parametrize("replacement", ["complete", "incomplete", "malformed", "schema_missing", "crash"])
def test_replacement_requires_its_own_tools_list(tmp_path, replacement):
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable,
                       server_script=str(_replacement_server(tmp_path)))
    second = None
    try:
        first = _relay(broker)
        _exchange(first, "tools/list", _complete_tools("old"))
        assert broker.readiness(1) == "ready"
        old_digest = broker.tools_receipt["schema_digests"]
        _crash_relay(first, broker)

        second = _relay(broker)
        _exchange(second, "initialize")
        # An authenticated replacement is not evidence that its tools work.
        assert broker.readiness(0) == "mcp_server_crashed"
        assert broker.tools_receipt["state"] == "not_observed"
        assert broker.tools_receipt["schema_digests"] == {}

        if replacement == "crash":
            second.stdin.write(b'{"jsonrpc":"2.0","id":2,"method":"crash"}\n')
            second.stdin.flush()
            assert second.stdout.read() == b""
            assert _await_readiness(broker, "mcp_server_exited") == "mcp_server_exited"
            assert broker.tools_receipt["state"] == "not_observed"
            return
        result = {
            "complete": _complete_tools(),
            "incomplete": dict(tools=[dict(name="write_report", inputSchema=dict(type="object"))]),
            "malformed": dict(tools="not a list"),
            "schema_missing": dict(tools=[dict(name="create_draft"), dict(name="write_report")]),
        }[replacement]
        # Even a tool-shaped reply to a different RPC is not tools/list proof.
        _exchange(second, "unrelated", _complete_tools())
        assert broker.tools_receipt["state"] == "not_observed"
        _exchange(second, "tools/list", result, request_id=3)
        if replacement == "complete":
            assert broker.readiness(1) == "ready"
            assert broker.tools_receipt["state"] == "complete"
            assert broker.tools_receipt["schema_digests"] != old_digest
            _crash_relay(second, broker)
        else:
            assert broker.readiness(0) == "mcp_tools_incomplete"
            assert broker.tools_receipt["state"] == "incomplete"
            assert broker.tools_receipt["missing_required_tools"]
    finally:
        if second is not None:
            if not second.stdin.closed:
                second.stdin.close()
            second.wait(timeout=5)
        broker.close()


def _memory_broker():
    import threading
    broker = object.__new__(McpBroker)
    broker._state_lock = threading.RLock()
    broker._generation = 0
    broker._ready = threading.Event()
    broker._closed = threading.Event()
    broker._stop = threading.Event()
    broker._failure = None
    broker._tools_receipt = None
    broker._child = None
    broker._observe = None
    return broker


def test_late_old_generation_reply_and_exit_cannot_change_replacement():
    broker = _memory_broker()
    old = broker._begin_generation()
    broker._observe_tools_list(dict(result=_complete_tools("old")), 1, old)
    broker._server_exited(old, 1, 7)
    assert broker.readiness(0) == "mcp_server_crashed"
    current = broker._begin_generation()
    broker._observe_tools_list(dict(result=_complete_tools("late")), 1, old)
    assert broker.readiness(0) == "mcp_server_crashed"
    assert broker.tools_receipt["state"] == "not_observed"
    broker._observe_tools_list(dict(result=_complete_tools()), 2, current)
    receipt = broker.tools_receipt
    assert broker.readiness(0) == "ready"
    broker._server_exited(old, 1, 7)
    broker._observe_tools_list(dict(result=dict(tools=[])), 1, old)
    assert broker.readiness(0) == "ready"
    assert broker.tools_receipt == receipt
    broker._server_exited(current, 2, 7)
    broker._observe_tools_list(dict(result=_complete_tools()), 2, current)
    assert broker.readiness(0) == "mcp_server_crashed"


def test_malformed_current_generation_cannot_be_healed_by_late_old_reply():
    broker = _memory_broker()
    old = broker._begin_generation()
    broker._observe_tools_list(dict(result=_complete_tools()), 1, old)
    broker._server_exited(old, 1, 7)
    current = broker._begin_generation()
    broker._observe_tools_list(dict(result=dict(tools=[])), 2, current)
    receipt = broker.tools_receipt
    broker._observe_tools_list(dict(result=_complete_tools()), 1, old)
    broker._server_exited(old, 1, 0)
    assert broker.readiness(0) == "mcp_tools_incomplete"
    assert broker.tools_receipt == receipt


@pytest.mark.parametrize("reply", [
    dict(result=None),
    dict(error=dict(code=-32603)),
    dict(result=dict(tools=[None])),
    dict(result=dict(tools=[dict(name=23, inputSchema=dict(type="object"))])),
    dict(result=dict(tools=[dict(name="create_draft", inputSchema=[]),
                           dict(name="write_report", inputSchema=dict(type="object"))])),
    dict(result=dict(**_complete_tools(), nextCursor="next-page")),
    dict(result=dict(tools=_complete_tools()["tools"] * 2)),
])
def test_malformed_tools_evidence_fails_closed_without_healing(reply):
    broker = _memory_broker()
    generation = broker._begin_generation()
    broker._observe_tools_list(reply, 1, generation)
    assert broker.readiness(0) == "mcp_tools_incomplete"
    assert broker.tools_receipt["state"] == "incomplete"
    broker._observe_tools_list(dict(result=_complete_tools()), 1, generation)
    broker._server_exited(generation, 1, 0)
    assert broker.readiness(0) == "mcp_tools_incomplete"


def test_fresh_complete_relay_can_recover_an_incomplete_replacement(tmp_path):
    broker = McpBroker(("ch_0123456789ab", "fake-secret-123456", str(tmp_path)),
                       python_executable=sys.executable,
                       server_script=str(_replacement_server(tmp_path)))
    third = None
    try:
        first = _relay(broker)
        _exchange(first, "tools/list", _complete_tools("old"))
        _crash_relay(first, broker)
        second = _relay(broker)
        output, _ = second.communicate((json.dumps(dict(
            jsonrpc="2.0", id=1, method="tools/list", params=dict(reply=dict(tools=[])))) + "\n").encode(),
            timeout=5)
        assert json.loads(output)["id"] == 1
        assert broker.readiness(0) == "mcp_tools_incomplete"
        assert broker.tools_receipt["state"] == "incomplete"
        third = _relay(broker)
        _exchange(third, "initialize")
        assert broker.readiness(0) == "mcp_tools_incomplete"
        assert broker.tools_receipt["state"] == "not_observed"
        _exchange(third, "tools/list", _complete_tools("third"), request_id=2)
        assert broker.readiness(1) == "ready"
        assert broker.tools_receipt["state"] == "complete"
    finally:
        if third is not None:
            third.stdin.close()
            third.wait(timeout=5)
        broker.close()
