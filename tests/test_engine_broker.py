"""Bounded authenticated host MCP broker tests."""
from __future__ import annotations

import subprocess
import sys
import json

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
        " result = {'tools': [{'name': 'create_draft', 'description': 'secret description', 'inputSchema': {'type': 'object', 'x-secret': 'never-log'}}, {'name': 'write_report', 'inputSchema': {'type': 'object'}}, {'name': 'publish_result', 'inputSchema': {'type': 'object'}}]} if request['method'] == 'tools/list' else {}\n"
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
        assert receipt["tool_names"] == ["create_draft", "publish_result", "write_report"]
        assert receipt["tool_count"] == 3
        assert receipt["missing_required_tools"] == []
        assert receipt["worker_publish_result_present"] is True
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
        assert receipt["worker_publish_result_present"] is False
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
