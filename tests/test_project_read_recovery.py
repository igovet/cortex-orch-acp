"""Scripted real gateway/wire recovery, not evidence of real-model compliance."""
import hashlib
import json
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt, prompt_access_class
from cortex_runtime.orchestration import consent
from cortex_runtime.orchestration.state import event
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import composed_instructions, known_role_ids, load_role
from cortex_runtime.server import Server
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel, resolve_channel
from runtime_scenarios import provision_fixture
from test_stdio import exchange


READ_PRECONDITIONS = {
    "gateway_read_required", "gateway_complete_read_required", "gateway_stale_read",
}


@pytest.fixture
def workflow(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin="read-recovery", event_id="one"), "Build", "owner")
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    worker, _ = provision_fixture(store, coordinator, role="backend-dev")
    ident, secret = issue_channel(store, boot.task_id, "worker", worker.owner_epoch,
                                  parent_channel_id=boot.channel_id,
                                  assignment_id=worker.assignment_id, attempt_id=worker.attempt_id)
    credentials = SimpleNamespace(channel_id=ident, secret=secret)
    server = Server(channel_credentials=(ident, secret, str(tmp_path)))
    return store, worker, coordinator, tmp_path, credentials, server


def data(reply):
    return reply.get("structuredContent") or json.loads(reply["content"][0]["text"])


def call(workflow, name, **arguments):
    return workflow[-1].dispatch("tools/call", dict(name=name, arguments=arguments))


def edits(old="before", new="after", **extra):
    return dict(path="file.txt", edits=[dict(old_string=old, new_string=new)], **extra)


def assert_precondition(reply, code):
    body = data(reply)
    assert reply["isError"] and body["error"] == code
    assert body["write_state"] == "rejected" and body["field"] == "path"
    assert "not an access refusal" in body["correction"]
    assert "owner question or extra approval" in body["message"]
    return body


def decision_counts(store):
    with store.connection() as db:
        counts = []
        for table in ("pending_questions", "consent_requests"):
            exists = db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)).fetchone()
            counts.append(db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] if exists else 0)
        return tuple(counts)


@pytest.mark.parametrize("wire", [False, True], ids=["dispatch", "stdio"])
def test_missing_read_then_exact_edit_without_owner_question_or_approval(workflow, monkeypatch, wire):
    store, _, _, root, credentials, _ = workflow
    target = root / "file.txt"
    target.write_text("before\nunseen tail\n")
    before = decision_counts(store)
    monkeypatch.setattr(consent, "request", lambda *a, **k: pytest.fail("unexpected approval"))
    def invoke(name, **args):
        if not wire:
            return call(workflow, name, **args)
        return exchange(root, [dict(jsonrpc="2.0", id="recovery", method="tools/call",
                                   params=dict(name=name, arguments=args))], channel=credentials)[0]["result"]
    failed = invoke("project_edit", **edits())
    assert_precondition(failed, "gateway_read_required")
    assert target.read_text() == "before\nunseen tail\n"
    assert not list(root.glob(".cortex-write-*"))
    observed = data(invoke("project_read", path="file.txt", limit=1, line_numbers=False))
    assert observed["content"] == "before\n" and not observed["read_coverage_complete"]
    changed = invoke("project_edit", **edits())
    assert not changed["isError"], data(changed)
    assert target.read_text() == "after\nunseen tail\n"
    assert data(changed)["edits_applied"] == 1
    assert data(changed)["sha256"] == hashlib.sha256(target.read_bytes()).hexdigest()
    assert data(invoke("project_edit", **edits())) == data(changed)  # replay, not a second effect
    assert decision_counts(store) == before
    with store.connection() as db:
        assert db.execute("SELECT count(*) FROM runtime_events WHERE type='TOOL_REJECTED' "
                          "AND json_extract(payload_json,'$.error')='gateway_read_required'").fetchone()[0] == 1


def test_explicit_key_retains_precondition_but_corrected_proposal_needs_no_approval(workflow):
    store, _, _, root, _, _ = workflow
    target = root / "file.txt"
    target.write_text("before")
    before = decision_counts(store)
    failed = call(workflow, "project_edit", **edits(request_key="failed"))
    assert_precondition(failed, "gateway_read_required")
    call(workflow, "project_read", path="file.txt")
    assert call(workflow, "project_edit", **edits(request_key="failed")) == failed
    assert target.read_text() == "before"
    changed = call(workflow, "project_edit", **edits(request_key="corrected"))
    assert not changed["isError"] and target.read_text() == "after"
    assert decision_counts(store) == before


def test_stale_patch_is_rebuilt_after_current_read_and_exact_match_is_still_required(workflow):
    store, _, _, root, _, _ = workflow
    target = root / "file.txt"
    target.write_text("before\noriginal tail\n")
    call(workflow, "project_read", path="file.txt")
    target.write_text("concurrent content\nexternal tail\n")
    before = decision_counts(store)
    assert_precondition(call(workflow, "project_edit", **edits()), "gateway_stale_read")
    assert target.read_text() == "concurrent content\nexternal tail\n"
    page = data(call(workflow, "project_read", path="file.txt", line_numbers=False))
    assert page["content"] == target.read_text()
    # A read is not permission to blindly apply the stale patch.
    unmatched = call(workflow, "project_edit", **edits())
    assert data(unmatched)["error"] == "gateway_edit_not_found"
    assert target.read_text() == "concurrent content\nexternal tail\n"
    changed = call(workflow, "project_edit", **edits("concurrent content", "rebuilt"))
    assert not changed["isError"] and target.read_text() == "rebuilt\nexternal tail\n"
    assert decision_counts(store) == before


def test_partial_and_other_channel_reads_never_authorize_replacement(workflow):
    store, other_channel, _, root, _, _ = workflow
    target = root / "file.txt"
    target.write_text("before\ntail\n")
    # The provisioned channel is not the wire channel used by this workflow.
    mcp_tools.dispatch(store, other_channel, "project_read", dict(path="file.txt"))
    assert_precondition(call(workflow, "project_edit", **edits()), "gateway_read_required")
    page = data(call(workflow, "project_read", path="file.txt", limit=1))
    args = dict(path="file.txt", content="replacement\n", expected_sha256=page["sha256"])
    assert_precondition(call(workflow, "project_write", **args), "gateway_complete_read_required")
    assert target.read_text() == "before\ntail\n"
    page = data(call(workflow, "project_read", path="file.txt", offset=2, limit=1))
    assert page["read_coverage_complete"] and page["full_content_sha256"]
    changed = call(workflow, "project_write", **dict(args, expected_sha256=page["full_content_sha256"]))
    assert not changed["isError"] and target.read_text() == "replacement\n"
    assert decision_counts(store) == (0, 0)


def test_hard_denial_stays_denied_after_read_and_fresh_delivery_key(workflow):
    store, _, _, root, _, _ = workflow
    target = root / "file.txt"
    target.write_text("before")
    with store.connection() as db:
        event(db, workflow[1].task_id, "USER_STEERING", dict(deny_write=["file.txt"]), "user")
        event(db, workflow[1].task_id, "STEERING_APPLIED", {}, "user")
        db.commit()
    denied = call(workflow, "project_edit", **edits())
    assert data(denied)["error"] == "gateway_scope_denied"
    assert data(denied)["error"] not in READ_PRECONDITIONS
    assert "No owner question" not in data(denied)["correction"]
    call(workflow, "project_read", path="file.txt")
    denied_again = call(workflow, "project_edit", **edits(request_key="not-a-cure"))
    assert data(denied_again)["error"] == "gateway_scope_denied"
    assert target.read_text() == "before" and decision_counts(store) == (0, 0)


@pytest.mark.parametrize("edition", [1, 2])
@pytest.mark.parametrize("code_mode", [False, True])
def test_both_prompt_editions_and_all_profiles_distinguish_read_preconditions(edition, code_mode):
    profiles = [name for name in known_role_ids() if name.startswith("worker-")]
    assert len(profiles) == 23
    for name in ["orchestrator", *profiles]:
        role = load_role(name)
        instructions = " ".join(composed_instructions(role).split())
        assert "preconditions are not access refusals" in instructions
        assert "without an owner question or extra approval" in instructions
        assert "never reach the same effect another way" in instructions
        prompt = cortex_system_prompt("coordinator" if name == "orchestrator" else "worker", None,
                                      access_class=prompt_access_class("coordinator" if name == "orchestrator" else "worker", name),
                                      edition=edition, code_mode=code_mode, environ={})
        assert "file-read preconditions are different" in prompt
        assert "Rebuild stale patches; never blindly retry them" in prompt
        assert "access, channel, role, scope or policy refusal" in prompt


def test_stdio_partial_other_channel_and_hard_denials_remain_distinct(workflow):
    store, other_channel, _, root, credentials, _ = workflow
    target = root / "file.txt"
    target.write_text("before\ntail\n")
    mcp_tools.dispatch(store, other_channel, "project_read", dict(path="file.txt"))
    def wire_calls(calls):
        return [reply["result"] for reply in exchange(root, [
            dict(jsonrpc="2.0", id=index, method="tools/call",
                 params=dict(name=name, arguments=args))
            for index, (name, args) in enumerate(calls)
        ], channel=credentials)]
    replies = wire_calls([
        ("project_edit", edits()),
        ("project_read", dict(path="file.txt", limit=1)),
        ("project_write", dict(path="file.txt", content="oops",
                              expected_sha256=hashlib.sha256(target.read_bytes()).hexdigest())),
        ("project_edit", dict(path=".cortex/private", edits=edits()["edits"])),
    ])
    assert_precondition(replies[0], "gateway_read_required")
    assert_precondition(replies[2], "gateway_complete_read_required")
    assert data(replies[3])["error"] == "gateway_path_denied"
    assert target.read_text() == "before\ntail\n"
    # A real user deny-write decision cannot be cured by a read or a new key.
    with store.connection() as db:
        event(db, other_channel.task_id, "USER_STEERING", dict(deny_write=["file.txt"]), "user")
        event(db, other_channel.task_id, "STEERING_APPLIED", {}, "user")
        db.commit()
    replies = wire_calls([
        ("project_read", dict(path="file.txt")),
        ("project_edit", edits(request_key="still-denied")),
    ])
    assert not replies[0]["isError"]
    assert data(replies[1])["error"] == "gateway_scope_denied"
    assert data(replies[1])["write_state"] == "rejected"
    assert target.read_text() == "before\ntail\n" and decision_counts(store) == (0, 0)


def test_live_descriptions_keep_read_recovery_and_key_rules_with_hard_boundary():
    tools = {tool["name"]: tool for tool in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev")}
    for name in ("project_write", "project_edit"):
        description = tools[name]["description"]
        assert "gateway_stale_read" in description and "owner question or extra approval" in description
        assert "explicit key" in description and "fresh" in description
        assert "never blindly retry" in description
        assert "Actual access/channel/role/scope/policy refusals remain final" in description
        key = tools[name]["inputSchema"]["properties"]["request_key"]["description"]
        assert "rebuild stale patches, never blindly retry" in key
        assert "Actual access/channel/role/scope/policy refusals remain final" in key
