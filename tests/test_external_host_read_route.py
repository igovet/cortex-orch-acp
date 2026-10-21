"""No-paid host reads through the real consent/gateway/process route, never a remount."""
import hashlib
import json
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import consent, gateway
from test_runtime_gateway import _role_worker


def _decide(store, worker, future, approved):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        pending = consent._claim_pending(store, worker.task_id)
        if pending:
            assert len(pending) == 1 and pending[0]["kind"] == "outside_command"
            consent.decide(store, pending[0]["id"], approved)
            return future.result(timeout=10)
        if future.done():
            return future.result()
        time.sleep(.01)
    pytest.fail("no consent request")


@pytest.mark.parametrize("case", ["exact", "missing", "denied"])
def test_host_sibling_read_uses_real_outside_consent_route(tmp_path, monkeypatch, case):
    # Deliberately retain /var/tmp: moving the fixture to shared /tmp would
    # conceal the live namespace bug. This route launches no bwrap at all.
    with tempfile.TemporaryDirectory(prefix="cortex-host-read-", dir="/var/tmp") as directory:
        base = Path(directory)
        project = base / "project"
        project.mkdir()
        outside = base / "outside"
        outside.mkdir()
        selected = outside / "inventory.json"
        data = json.dumps({"id": "exact-fixture", "amber": 7, "teal": 11}).encode()
        selected.write_bytes(data)
        before = hashlib.sha256(selected.read_bytes()).hexdigest()
        store, worker, _ = _role_worker(project, "backend-dev")
        target = selected if case != "missing" else outside / "absent.json"
        argv = [sys.executable, "-B", "-c",
                "import json,sys; f=open(sys.argv[1],'rb'); b=f.read(1024); "
                "assert not f.read(1); r=json.loads(b); assert r['id']=='exact-fixture'; "
                "print(r['amber']+r['teal'])", str(target)]
        if case == "denied":
            monkeypatch.setattr(gateway.subprocess, "Popen", lambda *a, **kw: pytest.fail("denied action launched"))
        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(mcp_tools.dispatch, store, worker, "outside_command",
                                     dict(argv=argv, cwd=str(project), request_key=case))
            if case == "denied":
                with pytest.raises(StoreError, match="outside_action_not_confirmed"):
                    _decide(store, worker, future, False)
                with store.connection() as db:
                    assert db.execute("select status from consent_requests").fetchone()[0] == "denied"
            else:
                result = _decide(store, worker, future, True)
                assert result["complete"] and not result["output_truncated"]
                assert result["profile"] == "outside_confirmed"
                assert result["argv"] == argv and result["cwd"] == str(project)
                if case == "exact":
                    assert result["exit_code"] == 0 and result["output"] == "18\n"
                else:
                    assert result["exit_code"] != 0 and "FileNotFoundError" in result["output"]
        assert hashlib.sha256(selected.read_bytes()).hexdigest() == before


@pytest.mark.parametrize("role", ["explorer", "senior-consultant"])
def test_read_only_roles_do_not_offer_or_escalate_to_host_commands(tmp_path, monkeypatch, role):
    store, worker, _ = _role_worker(tmp_path, role)
    assert "outside_command" not in {t["name"] for t in mcp_tools.tool_definitions("worker", role_id="worker-" + role)}
    monkeypatch.setattr(consent, "request", lambda *a, **kw: pytest.fail("no consent escalation"))
    monkeypatch.setattr(gateway.subprocess, "Popen", lambda *a, **kw: pytest.fail("no launch"))
    with pytest.raises(StoreError, match="gateway_scope_denied"):
        gateway.dispatch(store, worker, "outside_command", dict(argv=["true"], request_key="unsupported"))


def test_discovery_and_schemas_name_namespace_limit_without_granting_coordinator_execution():
    from cortex_runtime.engine.unified_system_prompt import native_worker_capabilities_section
    from cortex_runtime.delegation.mcp_tools import _OPERATION_DESCRIPTIONS
    note = native_worker_capabilities_section()
    assert "not proof of host" in note and "writing specialist" in note
    assert "private scratch" in _OPERATION_DESCRIPTIONS["command_run"]
    assert "bounded outside-file reads/searches" in _OPERATION_DESCRIPTIONS["outside_command"]
    offered = {t["name"] for t in mcp_tools.tool_definitions("coordinator")}
    assert not {"command_run", "outside_command"} & offered
