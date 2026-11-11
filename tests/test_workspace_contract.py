"""Cross-language Workspace v1 conformance; never launches a real ACP engine."""
import copy
import importlib.util
import json
from pathlib import Path
import select
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]


def contract():
    from cortex_runtime import workspace_contract
    return workspace_contract


def vectors():
    return json.loads((ROOT / "packages/workspace-contract/fixtures.json").read_text())


def test_shared_vectors():
    c = contract()
    for vector in vectors():
        if vector["valid"]:
            c.validate(vector["type"], vector["value"])
        else:
            with pytest.raises(c.ContractError):
                c.validate(vector["type"], vector["value"])


def test_json_schema_integer_semantics():
    c = contract()
    c.validate("Protocol", {"major": 1.0, "minor": 0})
    with pytest.raises(c.ContractError):
        c.validate("Protocol", {"major": 1.5, "minor": 0})
    with pytest.raises(c.ContractError):
        c.validate("Protocol", {"major": True, "minor": 0})


def test_schema_is_in_python_payload():
    from cortex_package import payload_paths
    paths = dict(payload_paths(ROOT))
    for name in ["schema.json", "schema.sha256", "schema-lock.json", "__init__.py"]:
        assert "src/cortex_runtime/workspace_contract/" + name in paths


def test_negotiation():
    c = contract()
    local = c.capabilities(build_identity="test-build", generation="7",
                           operations=["capabilities.get", "tasks.get", "tasks.create"])
    remote = c.capabilities(build_identity="peer-build", generation="8",
                            operations=["capabilities.get", "tasks.get", "events.page"])
    remote["protocol"]["minor"] = 99
    remote["futureField"] = True
    negotiated = c.negotiate(local, remote)
    assert negotiated["operations"] == ["capabilities.get", "tasks.get"]
    assert negotiated["protocol"] == c.PROTOCOL
    assert "futureField" not in negotiated
    remote["protocol"]["major"] = 2
    with pytest.raises(c.ContractError, match="version_mismatch"):
        c.negotiate(local, remote)


def test_unknown_response_enum_is_not_success():
    c = contract()
    receipt = next(v["value"] for v in vectors() if v["type"] == "Receipt" and v["valid"])
    receipt = dict(receipt, outcome="future_success", futureField={"kept": True})
    value, warnings = c.decode_response("Receipt", receipt)
    assert value["outcome"] == "unknown"
    assert warnings == ["$.outcome:unknown_enum"]
    assert value["futureField"] == {"kept": True}
    assert receipt["outcome"] == "future_success"


def test_limits_and_strict_commands():
    c = contract()
    command = copy.deepcopy(next(v["value"] for v in vectors()
                                 if v["type"] == "Command" and v["valid"]))
    c.validate("Command", command)
    for key, value in [("extra", 1), ("expectedVersion", 9007199254740993),
                       ("requestId", "x" * 129), ("expectedVersion", "01")]:
        bad = dict(command, **{key: value})
        with pytest.raises(c.ContractError):
            c.validate("Command", bad)
    command["payload"]["description"] = "😀" * 32000
    c.validate("Command", command)
    command["payload"]["description"] += "x"
    with pytest.raises(c.ContractError):
        c.validate("Command", command)


def test_older_client_limits_and_optional_limit_negotiation(tmp_path):
    from test_workspace_api import api_for, query
    c = contract()
    api = api_for(tmp_path)
    old = api.handle(query("capabilities.get"))
    assert old["limits"]["frameBytes"] == 1024 * 1024
    current_query = dict(query("capabilities.get"), protocol=dict(c.PROTOCOL))
    current = api.handle(current_query)
    assert current["limits"]["frameBytes"] == c.LIMITS["frameBytes"]
    old["limits"].pop("attachmentBytes")
    old["limits"].pop("attachmentFiles")
    result = c.negotiate(current, old)
    assert result["limits"]["frameBytes"] == 1024 * 1024
    assert "attachmentBytes" not in result["limits"]


def test_local_frame_limits_and_corruption():
    c = contract()
    value = {"message": "line\n😀"}
    encoded = c.encode_frame(value)
    assert c.decode_frame(encoded) == value
    for bad in [b"", b"\0\0\0\x01", encoded[:-1], encoded + b"x",
                b"\0\x10\0\x01", b"\0\0\0\x01\xff",
                b'\0\0\0\x07{"a":1,']:
        with pytest.raises(c.ContractError):
            c.decode_frame(bad)
    with pytest.raises(c.ContractError):
        c.encode_frame({"text": "x" * c.LIMITS["frameBytes"]})
    nested = None
    for _ in range(65):
        nested = [nested]
    with pytest.raises(c.ContractError, match="nesting_limit"):
        c.encode_frame(nested)


def test_unknown_nested_state_and_page_row_limits():
    c = contract()
    task = copy.deepcopy(next(v["value"] for v in vectors() if v["type"] == "Task" and v["valid"]))
    task["phase"] = "FUTURE_COMPLETED"
    task["executionState"] = "future_done"
    decoded, warnings = c.decode_response("Task", task)
    assert decoded["phase"] == decoded["executionState"] == "unknown"
    assert len(warnings) == 2
    for page_type, row_type in [("EventPage", "Event"), ("HistoryPage", "HistoryRow")]:
        row = next(v["value"] for v in vectors() if v["type"] == row_type and v["valid"])
        page = {"items": [row] * 100, "nextCursor": None, "highWatermark": "1"}
        c.validate(page_type, page)
        with pytest.raises(c.ContractError):
            c.validate(page_type, dict(page, items=[row] * 101))


def test_page_byte_limit():
    c = contract()
    page = {"items": [], "nextCursor": None, "highWatermark": "0"}
    c.validate("TaskPage", page)
    c.validate("TaskPage", dict(page, future="x" * 65000))
    with pytest.raises(c.ContractError, match="page_too_large"):
        c.validate("TaskPage", dict(page, future="é" * 33000))


def load_generator():
    spec = importlib.util.spec_from_file_location("workspace_generator",
        ROOT / "scripts/generate_workspace_contract.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_deterministic_generation_and_drift(tmp_path):
    generator = load_generator()
    schema = json.loads((ROOT / generator.SCHEMA_PATH).read_text())
    lock = json.loads((ROOT / generator.LOCK_PATH).read_text())
    first = generator.render(schema, lock)
    assert first == generator.render(schema, lock)
    for path, content in first.items():
        assert (ROOT / path).read_text() == content, path
    # A loose fingerprint edit alone also appears as generated drift.
    assert first[generator.FINGERPRINT_PATH].strip() == contract().SCHEMA_FINGERPRINT
    schema["$defs"]["Identity"]["maxLength"] = 127
    with pytest.raises(ValueError, match="version"):
        generator.render(schema, lock)
    # A new minor version is required for a changed fingerprint.
    schema["x-wire-version"]["minor"] += 1
    changed = generator.render(schema, lock)
    assert changed != first


def test_typescript_fixtures_and_runtime():
    result = subprocess.run(["node", "packages/workspace-contract/test.mjs"],
                            cwd=ROOT, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout + result.stderr
    tsc = ROOT / "packages/workspace-ui/node_modules/.bin/tsc"
    assert tsc.is_file(), "existing checkout TypeScript compiler unavailable"
    result = subprocess.run([str(tsc), "--noEmit", "--strict", "--skipLibCheck",
                             "--target", "ES2022", "--module", "NodeNext",
                             "--moduleResolution", "NodeNext",
                             "packages/workspace-contract/index.ts",
                             "packages/workspace-contract/fixtures.ts"],
                            cwd=ROOT, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout + result.stderr


def test_scripted_acp_peer_replays_malformed_and_silence():
    peer = ROOT / "tests/workspace_support/scripted_acp_peer.py"
    script = [
        {"expect": {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
         "send": [{"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}}]},
        {"raw": "{malformed"},
        {"expect": {"jsonrpc": "2.0", "id": 2, "method": "_cortex/workspace"},
         "stopResponding": True},
    ]
    with subprocess.Popen([sys.executable, "-B", str(peer)], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
        process.stdin.write((json.dumps(script) + "\n").encode())
        process.stdin.write(b'{"jsonrpc":"2.0","id":1,"method":"initialize"}\n')
        process.stdin.flush()
        assert json.loads(process.stdout.readline())["result"]["protocolVersion"] == 1
        assert process.stdout.readline() == b"{malformed\n"
        process.stdin.write(b'{"jsonrpc":"2.0","id":2,"method":"_cortex/workspace"}\n')
        process.stdin.flush()
        assert not select.select([process.stdout], [], [], 0.1)[0]
        process.stdin.close()
        process.wait(timeout=5)
        assert process.returncode == 0, process.stderr.read()
