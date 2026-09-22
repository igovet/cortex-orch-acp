"""No-paid wire trace through the installed codex-acp bundle and fake App Server.

Set CORTEX_CODEX_ACP_TEST_PACKAGE to an existing, operator-inspected 1.12.0
package directory to run the real-bundle test; ordinary CI is independent of
an external npm install. The fake Codex process never starts a model turn.
"""
from __future__ import annotations

import hashlib
import json
import os
import select
import shutil
import sqlite3
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.engine.process import (
    _CODEX_AGENT_REVIEWER_MANUAL, _CODEX_NEW_SESSION_PATCHED, _CODEX_SESSION_CONFIG_PATCHED,
    _CODEX_SEND_PROMPT_PATCHED,
    _prepare_codex_manual_review_adapter,
)
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.instruction_bundle import InstructionBundleLedger, capture_instruction_bundle
from cortex_runtime.portable.roles import load_role
from engine_fakes import isolated_environ, new_connection_id


FAKE_APP_SERVER = r'''#!/usr/bin/env python3
import json
import os
import sys

with open(os.environ["CORTEX_APP_SERVER_TRACE"], "w", encoding="utf-8") as trace:
    for line in sys.stdin:
        request = json.loads(line)
        method = request.get("method")
        if method in ("thread/start", "turn/start"):
            trace.write(json.dumps(request, sort_keys=True) + "\n")
            trace.flush()
        results = {
            "initialize": {"codexHome": os.environ["HOME"]},
            "account/read": {"account": {"type": "chatgpt"}, "requiresOpenaiAuth": False},
            "config/read": {"config": {"model_provider": "openai", "mcp_servers": {
                "native": {"command": "/bin/false", "enabled": False}
            }, **({} if os.environ.get("CORTEX_TEST_NATIVE") == "omit" else {"developer_instructions": "Native developer rule."})}},
            "skills/list": {"data": []},
            "thread/start": {"thread": {"id": "thread-fixture"}, "model": "gpt-5.6-luna", "reasoningEffort": "medium", "modelProvider": "openai"},
            "turn/start": {"turn": {"id": "turn-fixture"}},
            "model/list": {"data": [{"id": "gpt-5.6-luna", "displayName": "gpt-5.6-luna", "description": "fixture", "isDefault": True, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [{"reasoningEffort": "medium", "description": "fixture"}]}], "nextCursor": None},
        }
        if "id" in request:
            print(json.dumps({"id": request["id"], "result": results.get(method, {})}), flush=True)
            if method == "turn/start":
                print(json.dumps({"method": "turn/started", "params": {
                    "threadId": "thread-fixture", "turn": {"id": "turn-fixture"},
                }}), flush=True)
                if os.environ.get("CORTEX_TEST_RAW_CUSTOM_TOOL") == "1":
                    print(json.dumps({"method": "rawResponseItem/completed", "params": {
                        "threadId": "thread-fixture", "item": {
                            "type": "custom_tool_call", "call_id": "exec-call-1",
                            "name": "exec", "input": {"cmd": "python3 tests/check_focus_render.py", "workdir": os.getcwd()},
                        },
                    }}), flush=True)
                    print(json.dumps({"method": "rawResponseItem/completed", "params": {
                        "threadId": "thread-fixture", "item": {
                            "type": "custom_tool_call_output", "call_id": "exec-call-1",
                            "output": {"stdout": "ok", "exit_code": 0},
                        },
                    }}), flush=True)
                if os.environ.get("CORTEX_TEST_ITEM_CUSTOM_TOOL") == "1":
                    item_call_id = "exec-call-1" if os.environ.get("CORTEX_TEST_DUPLICATE_CUSTOM_TOOL") == "1" else "item-call-1"
                    print(json.dumps({"method": "item/started", "params": {
                        "threadId": "thread-fixture", "item": {
                            "type": "custom_tool_call", "id": item_call_id,
                            "call_id": item_call_id, "name": "exec",
                            "input": {"cmd": "python3 tests/check_focus_render.py", "workdir": os.getcwd()},
                        },
                    }}), flush=True)
                    print(json.dumps({"method": "item/completed", "params": {
                        "threadId": "thread-fixture", "item": {
                            "type": "custom_tool_call", "id": item_call_id,
                            "call_id": item_call_id, "name": "exec",
                            "status": os.environ.get("CORTEX_TEST_ITEM_CUSTOM_TOOL_STATUS", "completed"), "output": {"stdout": "ok", "exit_code": 0},
                        },
                    }}), flush=True)
                lifecycle = os.environ.get("CORTEX_TEST_CUSTOM_LIFECYCLE")
                if lifecycle:
                    call_id = "lifecycle-call-1"
                    start_item = {"type": "custom_tool_call", "id": call_id, "call_id": call_id,
                                  "name": "exec", "input": {"cmd": "python3 check.py"}}
                    output_item = {"type": "custom_tool_call_output", "id": call_id, "call_id": call_id,
                                   "output": {"stdout": "rich output", "exit_code": 0}}
                    if lifecycle == "reversed":
                        for item in (output_item, start_item):
                            print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": item}}), flush=True)
                    elif lifecycle == "richer":
                        start_item["status"] = "completed"
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": start_item}}), flush=True)
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": output_item}}), flush=True)
                    elif lifecycle == "rich_then_empty":
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": start_item}}), flush=True)
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": output_item}}), flush=True)
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": {"type": "custom_tool_call_output", "id": call_id, "call_id": call_id}}}), flush=True)
                    else:
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": start_item}}), flush=True)
                        print(json.dumps({"method": "rawResponseItem/completed", "params": {"item": output_item}}), flush=True)
                print(json.dumps({"method": "turn/completed", "params": {
                    "threadId": "thread-fixture",
                    "turn": {"id": "turn-fixture", "status": "completed", "items": []},
                }}), flush=True)
'''


def test_codex_session_metadata_has_an_exact_durable_sent_hash(tmp_path):
    role = load_role("worker-backend-dev")
    plan = delivery_for("codex-acp", "1.12.0").plan(role, [])
    metadata = json.dumps(plan.session_new_meta, sort_keys=True, ensure_ascii=False)
    bundle = capture_instruction_bundle(
        task_id="task-fixture", assignment_ref="assignment-fixture", attempt_id="attempt-fixture",
        role_id=role.role_id, role_kind="worker", engine="codex-acp", engine_version="1.12.0",
        assembled_text=role.body_portable,
    )

    class Store:
        def connection(self):
            return sqlite3.connect(tmp_path / "bundle.sqlite3")

    ledger = InstructionBundleLedger(Store())
    ledger.save_intended(bundle)
    bound = ledger.record_sent(
        bundle, metadata, engine="codex-acp", engine_version="1.12.0",
        attempt_id="attempt-fixture", session_ref="replacement-session-fixture",
    )
    assert bound.sent_representation_sha256 == hashlib.sha256(metadata.encode("utf-8")).hexdigest()
    assert ledger.get(bundle.bundle_id).sent_representation_sha256 == bound.sent_representation_sha256


def _read_response(proc: subprocess.Popen, response_id: int, seen=None) -> dict:
    assert proc.stdout is not None
    for _ in range(30):
        ready, _, _ = select.select([proc.stdout], [], [], 1)
        if not ready:
            continue
        line = proc.stdout.readline()
        if not line:
            raise AssertionError("codex-acp exited before its ACP response")
        value = json.loads(line)
        if seen is not None:
            seen.append(value)
        if value.get("id") == response_id:
            return value
    raise AssertionError("codex-acp did not respond within 30 seconds")


def test_installed_adapter_keeps_explicit_mcp_on_disabled_native_name(tmp_path):
    """The real ACP parser and private adapter must re-enable a scoped server."""
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    assert json.loads((package / "package.json").read_text())["version"] == "1.12.0"
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original),
        package_json=str(package / "package.json"),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(
        environ, CODEX_PATH=str(fake_binary), CORTEX_APP_SERVER_TRACE=str(trace),
        NO_BROWSER="1", CODEX_CONFIG=json.dumps({
            "features": {"plugins": True},
            "mcp_servers": {"native": {"command": "/bin/false", "enabled": False}},
        }),
    )
    proc = subprocess.Popen(
        [shutil.which("node"), str(copied)], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, env=process_env, cwd=tmp_path, bufsize=1,
    )
    try:
        assert proc.stdin is not None
        for request_id, method, params in (
            (1, "initialize", {
                "protocolVersion": 1, "clientCapabilities": {},
                "clientInfo": {"name": "cortex-mcp-trace", "version": "1"},
            }),
            (2, "session/new", {
                "cwd": str(tmp_path),
                "mcpServers": [{
                    "name": "native", "command": "/bin/true", "args": [], "env": [],
                }],
                "_meta": {"systemPrompt": "Cortex role rule."},
            }),
        ):
            proc.stdin.write(json.dumps({
                "jsonrpc": "2.0", "id": request_id, "method": method, "params": params,
            }) + "\n")
            proc.stdin.flush()
            assert "error" not in _read_response(proc, request_id)
        sent = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
        assert len(sent) == 1 and sent[0]["method"] == "thread/start"
        config = sent[0]["params"]["config"]
        assert config["features"]["plugins"] is False
        assert config["mcp_servers"]["native"] == {
            "command": "/bin/true", "args": [], "env": {}, "enabled": True, "default_tools_approval_mode": "approve",
            "tool_timeout_sec": 31536000,
        }
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


@pytest.mark.parametrize("native_mode", ["present", "omit"])
def test_installed_private_adapter_forwards_and_preserves_native_developer_instructions(tmp_path, native_mode):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    assert json.loads((package / "package.json").read_text())["version"] == "1.12.0"
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    assert _CODEX_AGENT_REVIEWER_MANUAL in copied.read_bytes()
    assert _CODEX_NEW_SESSION_PATCHED in copied.read_bytes()
    assert _CODEX_SESSION_CONFIG_PATCHED in copied.read_bytes()
    adapter_text = copied.read_bytes()
    assert _CODEX_SEND_PROMPT_PATCHED in adapter_text
    assert b'sandboxPolicy: { type: "dangerFullAccess" }' in adapter_text
    assert b'approvalPolicy: "never"' in adapter_text
    assert original.read_bytes() == source

    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_APP_SERVER_TRACE=str(trace),
                       CORTEX_TEST_NATIVE=native_mode, NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({
                           "agents": {"enabled": True, "max_depth": 2},
                           "features": {"multi_agent": True, "multi_agent_v2": True, "js_repl": True},
                           "agents.enabled": True, "features.multi_agent": True,
                           "features.multi_agent_v2": True,
                           "model_reasoning_effort": "medium",
                       }))
    node_binary = shutil.which("node")
    assert node_binary is not None
    proc = subprocess.Popen(
        [node_binary, str(copied)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, env=process_env, cwd=tmp_path, bufsize=1,
    )
    try:
        assert proc.stdin is not None
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"},
        }}) + "\n")
        proc.stdin.flush()
        initialized = _read_response(proc, 1)
        assert "error" not in initialized, initialized
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {
            "cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."},
        }}) + "\n")
        proc.stdin.flush()
        result = _read_response(proc, 2)
        assert "error" not in result, result
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 3, "method": "session/new", "params": {
            "cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Replacement role rule."},
        }}) + "\n")
        proc.stdin.flush()
        replacement = _read_response(proc, 3)
        assert "error" not in replacement, replacement
        sent = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
        assert len(sent) == 2
        assert [entry["method"] for entry in sent] == ["thread/start", "thread/start"]
        native_prefix = "Native developer rule.\n\n" if native_mode == "present" else ""
        assert [entry["params"]["developerInstructions"] for entry in sent] == [
            native_prefix + "Cortex role rule.",
            native_prefix + "Replacement role rule.",
        ]
        for entry in sent:
            config = entry["params"]["config"]
            assert "agents.enabled" not in config
            assert "features.multi_agent" not in config
            assert "features.multi_agent_v2" not in config
            assert config["agents"] == {"enabled": False, "max_depth": 2}
            for feature in ('multi_agent','multi_agent_v2','hooks','apps','plugins'):
                assert config['features'][feature] is False
            # Native action tools are the engine's own; the engine jail bounds them.
            assert config['features']['js_repl'] is True
            assert 'shell_tool' not in config['features'] and 'web_search' not in config
            assert config["model_reasoning_effort"] == "medium"

        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 4, "method": "session/prompt", "params": {
            "sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect the project"}],
        }}) + "\n")
        proc.stdin.flush()
        prompted = _read_response(proc, 4)
        assert "error" not in prompted, prompted
        traces_after_prompt = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
        turn_starts = [entry for entry in traces_after_prompt if entry["method"] == "turn/start"]
        assert len(turn_starts) == 1
        assert turn_starts[0]["params"]["sandboxPolicy"] == {"type": "dangerFullAccess"}
        assert turn_starts[0]["params"]["approvalPolicy"] == "never"

    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


def test_installed_private_adapter_forwards_raw_custom_exec_tool_with_exact_call_id(tmp_path):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    assert json.loads((package / "package.json").read_text())['version'] == '1.12.0'
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_APP_SERVER_TRACE=str(trace),
                       CORTEX_TEST_RAW_CUSTOM_TOOL="1", NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({"model_reasoning_effort": "medium"}))
    node_binary = shutil.which("node")
    assert node_binary is not None
    proc = subprocess.Popen([node_binary, str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=process_env, cwd=tmp_path, bufsize=1)
    seen = []
    try:
        assert proc.stdin is not None
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"},
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 1, seen)
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {
            "cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."},
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 2, seen)
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
            "sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect the project"}],
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 3, seen)
        updates = [value.get("params", {}).get("update", {}) for value in seen if value.get("method") == "session/update"]
        custom = [update for update in updates if update.get("toolCallId") == "exec-call-1"]
        assert [update.get("sessionUpdate") for update in custom] == ["tool_call", "tool_call_update"]
        assert custom[0]["kind"] == "execute"
        assert custom[0]["title"] == "python3 tests/check_focus_render.py"
        assert custom[0]["rawInput"]["input"]["cmd"] == "python3 tests/check_focus_render.py"
        assert custom[1]["status"] == "completed"
        assert custom[1]["rawOutput"]["output"]["exit_code"] == 0
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


@pytest.mark.parametrize("item_status,expected_status", [("completed", "completed"), ("failed", "failed")])
def test_installed_private_adapter_forwards_item_custom_exec_tool_with_exact_call_id(tmp_path, item_status, expected_status):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_TEST_ITEM_CUSTOM_TOOL="1",
                       CORTEX_TEST_ITEM_CUSTOM_TOOL_STATUS=item_status,
                       CORTEX_APP_SERVER_TRACE=str(trace), NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({"model_reasoning_effort": "medium"}))
    node_binary = shutil.which("node")
    assert node_binary is not None
    proc = subprocess.Popen([node_binary, str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=process_env, cwd=tmp_path, bufsize=1)
    seen = []
    try:
        assert proc.stdin is not None
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"},
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 1, seen)
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {
            "cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."},
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 2, seen)
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
            "sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect the project"}],
        }}) + "\n")
        proc.stdin.flush()
        assert "error" not in _read_response(proc, 3, seen)
        updates = [value.get("params", {}).get("update", {}) for value in seen if value.get("method") == "session/update"]
        custom = [update for update in updates if update.get("toolCallId") == "item-call-1"]
        assert [update.get("sessionUpdate") for update in custom] == ["tool_call", "tool_call_update"]
        assert custom[0]["title"] == "python3 tests/check_focus_render.py"
        assert custom[1]["status"] == expected_status
        assert custom[1]["rawOutput"]["output"]["exit_code"] == 0
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


def test_installed_private_adapter_deduplicates_raw_and_item_custom_exec_updates(tmp_path):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_TEST_RAW_CUSTOM_TOOL="1",
                       CORTEX_TEST_ITEM_CUSTOM_TOOL="1", CORTEX_TEST_DUPLICATE_CUSTOM_TOOL="1",
                       CORTEX_APP_SERVER_TRACE=str(trace), NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({"model_reasoning_effort": "medium"}))
    node_binary = shutil.which("node")
    assert node_binary is not None
    proc = subprocess.Popen([node_binary, str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=process_env, cwd=tmp_path, bufsize=1)
    seen = []
    try:
        assert proc.stdin is not None
        for request_id, method, params in [
            (1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"}}),
            (2, "session/new", {"cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."}}),
            (3, "session/prompt", {"sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect the project"}]}),
        ]:
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n")
            proc.stdin.flush()
            assert "error" not in _read_response(proc, request_id, seen)
        updates = [value.get("params", {}).get("update", {}) for value in seen if value.get("method") == "session/update"]
        custom = [update for update in updates if update.get("toolCallId") == "exec-call-1"]
        assert [update.get("sessionUpdate") for update in custom] == ["tool_call", "tool_call_update"]
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


@pytest.mark.parametrize("lifecycle,expected_updates", [("richer", ["tool_call", "tool_call_update"]), ("reversed", ["tool_call"]), ("rich_then_empty", ["tool_call", "tool_call_update"])])
def test_installed_private_adapter_preserves_custom_tool_lifecycle_order_and_rich_output(tmp_path, lifecycle, expected_updates):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    install = SimpleNamespace(package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
                              adapter_entry_sha256=hashlib.sha256(source).hexdigest())
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_TEST_CUSTOM_LIFECYCLE=lifecycle,
                       CORTEX_APP_SERVER_TRACE=str(trace), NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({"model_reasoning_effort": "medium"}))
    proc = subprocess.Popen([shutil.which("node"), str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=process_env, cwd=tmp_path, bufsize=1)
    seen = []
    try:
        assert proc.stdin is not None
        requests = [
            (1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"}}),
            (2, "session/new", {"cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."}}),
            (3, "session/prompt", {"sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect"}]}),
        ]
        for request_id, method, params in requests:
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n")
            proc.stdin.flush()
            assert "error" not in _read_response(proc, request_id, seen)
        updates = [value.get("params", {}).get("update", {}) for value in seen if value.get("method") == "session/update"]
        custom = [update for update in updates if update.get("toolCallId") == "lifecycle-call-1"]
        assert [update.get("sessionUpdate") for update in custom] == expected_updates
        if lifecycle == "reversed":
            assert custom[0]["status"] == "completed"
            assert custom[0]["rawOutput"]["output"]["stdout"] == "rich output"
        else:
            assert custom[1]["rawOutput"]["output"]["stdout"] == "rich output"
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)


def test_installed_private_adapter_resets_custom_tool_ids_each_turn(tmp_path):
    package_text = os.environ.get("CORTEX_CODEX_ACP_TEST_PACKAGE")
    if not package_text:
        pytest.skip("set CORTEX_CODEX_ACP_TEST_PACKAGE to run the real-bundle no-paid trace")
    package = Path(package_text).resolve()
    original = package / "dist" / "index.js"
    source = original.read_bytes()
    install = SimpleNamespace(package_version="1.12.0", adapter_entry=str(original), package_json=str(package / "package.json"),
                              adapter_entry_sha256=hashlib.sha256(source).hexdigest())
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    fake_binary = tmp_path / "fake-codex"
    fake_binary.write_text(FAKE_APP_SERVER, encoding="utf-8")
    fake_binary.chmod(0o700)
    trace = tmp_path / "app-server-trace.jsonl"
    process_env = dict(environ, CODEX_PATH=str(fake_binary), CORTEX_TEST_CUSTOM_LIFECYCLE="reuse",
                       CORTEX_APP_SERVER_TRACE=str(trace), NO_BROWSER="1",
                       CODEX_CONFIG=json.dumps({"model_reasoning_effort": "medium"}))
    proc = subprocess.Popen([shutil.which("node"), str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=process_env, cwd=tmp_path, bufsize=1)
    seen = []
    try:
        assert proc.stdin is not None
        for request_id, method, params in [
            (1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-trace", "version": "1"}}),
            (2, "session/new", {"cwd": str(tmp_path), "mcpServers": [], "_meta": {"systemPrompt": "Cortex role rule."}}),
            (3, "session/prompt", {"sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect"}]}),
            (4, "session/prompt", {"sessionId": "thread-fixture", "prompt": [{"type": "text", "text": "inspect again"}]}),
        ]:
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n")
            proc.stdin.flush()
            assert "error" not in _read_response(proc, request_id, seen)
        updates = [value.get("params", {}).get("update", {}) for value in seen if value.get("method") == "session/update"]
        custom = [update for update in updates if update.get("toolCallId") == "lifecycle-call-1"]
        assert [update.get("sessionUpdate") for update in custom] == ["tool_call", "tool_call_update", "tool_call", "tool_call_update", "tool_call", "tool_call_update"]
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate(timeout=3)
