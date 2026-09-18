"""Validate every emitted ACP message against the pinned SDK schema.

Fixture: ``tests/fixtures/acp/schema-1.4.0.json`` (verbatim copy of
``@agentclientprotocol/sdk@1.4.0``'s ``schema/schema.json``; see
``tests/fixtures/acp/NOTICE``).
"""
import json
from pathlib import Path

import pytest

jsonschema = pytest.importorskip("jsonschema")

from acp_fakes import spawn_fake_agent, spawn_production_agent

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "acp" / "schema-1.4.0.json"


def _schema_document():
    return json.loads(FIXTURE.read_text())


def _validator(def_name):
    document = _schema_document()
    resolver = jsonschema.RefResolver.from_schema(document)
    schema = {"$ref": f"#/$defs/{def_name}"}
    return jsonschema.Draft202012Validator(schema, resolver=resolver)


def _send(proc, message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _recv(proc):
    return json.loads(proc.stdout.readline())


def test_schema_fixture_is_valid_and_has_expected_defs():
    document = _schema_document()
    assert document["$defs"]["InitializeResponse"]
    assert document["$defs"]["NewSessionResponse"]
    assert document["$defs"]["PromptResponse"]
    assert document["$defs"]["SessionNotification"]


def test_every_emitted_message_matches_the_pinned_schema(tmp_path):
    response_validator = _validator("AgentResponse")
    notification_validator = _validator("AgentNotification")

    proc = spawn_fake_agent(chunks=("hi",))
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        init_reply = _recv(proc)
        response_validator.validate(init_reply)

        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        new_reply = _recv(proc)
        response_validator.validate(new_reply)
        session_id = new_reply["result"]["sessionId"]
        commands_update = _recv(proc)
        notification_validator.validate(commands_update)

        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="go")])))
        update = _recv(proc)
        notification_validator.validate(update)

        final = _recv(proc)
        response_validator.validate(final)
        assert final["result"]["stopReason"] == "end_turn"
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0


def test_tool_call_cards_match_schema():
    """T035: the worker-assignment tool-call/tool-call-update notifications
    (``acp.protocol.tool_call``/``tool_call_update``) validate against the
    pinned schema's ``AgentNotification`` -- covering every status this
    runtime actually emits, including the cancelled-as-failed mapping."""
    from cortex_runtime.acp import protocol

    notification_validator = _validator("AgentNotification")
    create = protocol.tool_call(
        "s1", "worker:abc123", "Worker: worker-backend-dev — Add a cache", "execute", "pending",
        text="Queued: add a cache to the repository",
    )
    notification_validator.validate(create)
    assert create["params"]["update"]["sessionUpdate"] == "tool_call"

    for status in ("in_progress", "completed", "failed"):
        update = protocol.tool_call_update("s1", "worker:abc123", status=status, text="some bounded account text")
        notification_validator.validate(update)
        assert update["params"]["update"]["sessionUpdate"] == "tool_call_update"

    # The cancelled-assignment mapping this runtime uses: no 'cancelled'
    # ToolCallStatus exists in the pinned schema, so it renders as 'failed'
    # -- assert that mapped value itself still validates.
    cancelled_as_failed = protocol.tool_call_update("s1", "worker:abc123", status="failed", text="Cancelled: no longer needed")
    notification_validator.validate(cancelled_as_failed)


def test_error_response_matches_schema():
    response_validator = _validator("AgentResponse")
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="not/a/real/method"))
        reply = _recv(proc)
        assert "error" in reply
        response_validator.validate(reply)
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0
