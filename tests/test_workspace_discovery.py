"""Pre-save discovery uses real no-network sandboxed fixture servers, never an engine."""
import importlib.util
import json
import os
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mcp_discovery", ROOT / "crates/workspace-service/mcp_discovery.py")
discovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(discovery)


def draft(script):
    return dict(name="fixture", command="/usr/bin/python3", args=["-B", str(script)], env=[],
                readPaths=[str(script)], writePaths=[], network=False, runtimeDirEnv=[])


@pytest.mark.requires_bwrap
@pytest.mark.real_verification_sandbox
def test_lists_tools_without_saving_or_calling_and_reaps_server(tmp_path):
    script = tmp_path / "fixture.py"
    script.write_text('''import json, sys
for line in sys.stdin:
    m=json.loads(line)
    if "id" not in m: continue
    if m["method"]=="initialize": result={"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}
    elif m["method"]=="tools/list": result={"tools":[{"name":"read","description":"Safe listing","annotations":{"readOnlyHint":True}},{"name":"write"}]}
    else: raise RuntimeError("unexpected tool call")
    print(json.dumps({"jsonrpc":"2.0","id":m["id"],"result":result}),flush=True)
''')
    result = discovery.discover(draft(script), timeout=3)
    assert result["ok"], json.dumps(result)
    assert [t["name"] for t in result["observation"]["tools"]] == ["read", "write"]
    assert result["observation"]["tools"][0]["hint"] == "read"
    assert not list(tmp_path.glob("*.json"))


@pytest.mark.requires_bwrap
@pytest.mark.real_verification_sandbox
def test_hung_server_deadline_is_sanitized(tmp_path):
    script = tmp_path / "hung.py"
    script.write_text("import time\ntime.sleep(30)\n")
    server = draft(script)
    server["env"] = [{"name":"TOKEN","action":"replace","value":"secret-fixture-value"}]
    result = discovery.discover(server, timeout=.3)
    assert not result["ok"]
    assert "secret-fixture-value" not in json.dumps(result)
    assert result["code"] == "timeout", json.dumps(result)


@pytest.mark.real_verification_sandbox
def test_private_and_symlink_grants_are_refused_before_launch(tmp_path):
    script = tmp_path / "fixture.py"
    script.write_text("raise RuntimeError('must not launch')")
    link = tmp_path / "linked"
    link.symlink_to(script)
    server = draft(script)
    server["readPaths"] = [str(link)]
    result = discovery.discover(server)
    assert not result["ok"] and result["code"] == "path_refused"


def test_listing_protocol_and_paging_without_launching_a_process():
    import time
    from types import SimpleNamespace
    read, write = os.pipe()
    responses = [
        {"id":1,"result":{"protocolVersion":"2024-11-05"}},
        {"id":2,"result":{"tools":[{"name":"read"}],"nextCursor":"next"}},
        {"id":3,"result":{"tools":[{"name":"write"}]}}
    ]
    os.write(write, b"".join(json.dumps(r).encode()+b"\n" for r in responses))
    os.close(write)
    with os.fdopen(read, "rb", buffering=0) as output, tempfile_file() as input:
        tools = discovery.listing(SimpleNamespace(stdin=input, stdout=output), time.monotonic()+1)
        input.seek(0)
        sent = [json.loads(line) for line in input]
    assert [t["name"] for t in tools] == ["read", "write"]
    assert [m["method"] for m in sent] == ["initialize", "notifications/initialized", "tools/list", "tools/list"]
    assert sent[-1]["params"] == {"cursor":"next"}


def tempfile_file():
    import tempfile
    return tempfile.TemporaryFile()
