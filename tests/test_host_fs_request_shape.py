"""The host `fs/*` relay must send requests a real client accepts.

Found live, in Zed 1.20.1, on the first run where a worker's change ever
reached integration at all::

    Handler errored method="fs/read_text_file" id=Some(Str("hr_1"))
    err=Error { code: -32602, message: "Invalid params",
                data: {"error": "missing field `sessionId`",
                       "json": {"path": "/var/tmp/.../calc.py"}} }

`acp.agent.Agent._make_fs_requester` sent only the path. ACP's own
`ReadTextFileRequest` requires `sessionId`, so every such request was
rejected outright; `delegation.integration.integrate` could only read that
rejection as a failed round-trip (which is the correct, conservative
reading of an untrusted answer), recorded
`host_buffer_state: "check_failed"`, conflicted every changed path with
`host_buffer_check_failed`, and refused to apply anything. No delegated
change could reach the project through a real client, ever.

Nothing covered the shape of that request, which is why it shipped. This
validates it against the pinned schema fixture rather than against an
expectation written by the same hand that wrote the code.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = REPO_ROOT / "tests" / "fixtures" / "acp" / "schema-1.4.0.json"
sys.path.insert(0, str(REPO_ROOT / "plugins" / "cortex" / "scripts"))

from cortex_runtime.acp.agent import Agent  # noqa: E402


class _RecordingHostRequester:
    def __init__(self):
        self.calls: list[tuple[str, dict]] = []

    def request(self, method, params):
        self.calls.append((method, params))
        return {"content": "x = 1\n"}


def _agent_with_fs_capability(host_requester):
    agent = Agent.__new__(Agent)
    agent.client_capabilities = {"fs": {"readTextFile": True, "writeTextFile": False}}
    agent.host_requester = host_requester
    return agent


def test_read_text_file_request_carries_the_session_id():
    host = _RecordingHostRequester()
    agent = _agent_with_fs_capability(host)

    request_fs = agent._make_fs_requester("acps_abc123")
    assert request_fs is not None, "the capability was advertised; a relay must be built"

    result = request_fs("fs/read_text_file", {"path": "/project/calc.py"})
    assert result == {"content": "x = 1\n"}

    assert len(host.calls) == 1
    method, params = host.calls[0]
    assert method == "fs/read_text_file"
    assert params["sessionId"] == "acps_abc123"
    assert params["path"] == "/project/calc.py"


def test_a_caller_supplied_session_id_is_never_overridden():
    host = _RecordingHostRequester()
    request_fs = _agent_with_fs_capability(host)._make_fs_requester("acps_relay")
    request_fs("fs/read_text_file", {"path": "/p/x.py", "sessionId": "acps_explicit"})
    assert host.calls[0][1]["sessionId"] == "acps_explicit"


def test_the_request_validates_against_the_pinned_schema():
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads(SCHEMA_PATH.read_text())
    defs = schema.get("$defs") or schema.get("definitions") or {}
    assert "ReadTextFileRequest" in defs, "the pinned schema no longer defines this request"

    host = _RecordingHostRequester()
    request_fs = _agent_with_fs_capability(host)._make_fs_requester("acps_abc123")
    request_fs("fs/read_text_file", {"path": "/project/calc.py"})
    _method, params = host.calls[0]

    request_schema = dict(defs["ReadTextFileRequest"])
    request_schema["$defs"] = defs
    jsonschema.validate(instance=params, schema=request_schema)


def test_no_relay_at_all_when_the_client_never_advertised_the_capability():
    """Unchanged by this fix, and the reason a missing capability is
    recorded as 'unknown' rather than 'no buffer': no relay is built."""
    agent = Agent.__new__(Agent)
    agent.client_capabilities = {"fs": {"readTextFile": False}}
    agent.host_requester = _RecordingHostRequester()
    assert agent._make_fs_requester("acps_abc123") is None

    agent.client_capabilities = {}
    assert agent._make_fs_requester("acps_abc123") is None
