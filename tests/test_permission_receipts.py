"""A host response is tied to one observed command, not worker prose."""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import completion, evidence
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import PermissionBridge
from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Delete only a sentinel.", "owner")
    return store, task.task_id


def _binding(tmp_path):
    return ResourceBinding(
        engine_instance_id="eirt_1", engine_session_id="es_1", context_generation_id="gen_1",
        assignment_id="assignment_1", attempt_id="attempt_1", workspace_binding_id="binding_1",
        workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
    )


def _permission(store, task_id, tmp_path, command, host_response):
    binding = _binding(tmp_path)
    bindings = BindingRegistry()
    bindings.bind(binding)
    receipts = []

    def sink(owner, action_ref, exact_command, outcome):
        assert owner == binding
        receipts.append((action_ref, exact_command, outcome))
        evidence.record_permission_response(store, task_id, owner.attempt_id, action_ref, exact_command, outcome)

    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *_: host_response,
        permission_receipt_sink=sink,
    )
    params = {
        "toolCall": {"toolCallId": "wire-rm-1", "kind": "execute", "rawInput": {"command": command}},
        "options": [
            {"optionId": "yes", "kind": "allow_once"},
            {"optionId": "no", "kind": "reject_once"},
        ],
    }
    result = bridge.handle(engine_session_id="es_1", wire_session_id="wire_1", wire_request_id=1, params=params)
    return result, receipts


def test_exact_rm_host_allow_once_produces_bound_durable_receipt(tmp_path):
    store, task_id = _task(tmp_path)
    command = "rm -- sentinel.txt"
    result, receipts = _permission(store, task_id, tmp_path, command, {"outcome": {"outcome": "selected", "optionId": "yes"}})
    assert result == {"outcome": {"outcome": "selected", "optionId": "yes"}}
    assert len(receipts) == 1
    action_ref, exact_command, outcome = receipts[0]
    assert (exact_command, outcome) == (command, "allow_once")
    evidence.record_tool_result_observed(store, task_id, "attempt_1", action_ref, "execute", command, "in_progress")
    evidence.record_tool_result_observed(store, task_id, "attempt_1", action_ref, None, None, "completed")
    assert not any("unconfirmed_rm_permission" in reason for reason in completion.gate(store, task_id)[1])
    with store.connection() as db:
        row = db.execute("SELECT task_id,attempt_id,tool_call_ref,command_text,host_outcome FROM permission_receipts").fetchone()
    assert tuple(row) == (task_id, "attempt_1", action_ref, command, "allow_once")


def test_observed_rm_without_matching_host_receipt_blocks_acceptance(tmp_path):
    store, task_id = _task(tmp_path)
    evidence.record_tool_result_observed(store, task_id, "attempt_1", "tc_unapproved", "execute", "rm -- sentinel.txt", "completed")
    ok, reasons = completion.gate(store, task_id)
    assert not ok
    assert "unconfirmed_rm_permission:attempt_1:tc_unapproved" in reasons
    evidence.record_permission_response(store, task_id, "attempt_2", "tc_unapproved", "rm -- sentinel.txt", "allow_once")
    assert "unconfirmed_rm_permission:attempt_1:tc_unapproved" in completion.gate(store, task_id)[1]
    evidence.record_permission_response(store, task_id, "attempt_1", "tc_unapproved", "rm -- other.txt", "allow_once")
    assert "unconfirmed_rm_permission:attempt_1:tc_unapproved" in completion.gate(store, task_id)[1]


def test_denial_never_becomes_an_approved_receipt(tmp_path):
    store, task_id = _task(tmp_path)
    result, receipts = _permission(store, task_id, tmp_path, "rm -- sentinel.txt", {"outcome": {"outcome": "selected", "optionId": "no"}})
    assert result == {"outcome": {"outcome": "selected", "optionId": "no"}}
    action_ref = receipts[0][0]
    assert receipts[0][2] == "reject_once"
    evidence.record_tool_result_observed(store, task_id, "attempt_1", action_ref, "execute", "rm -- sentinel.txt", "failed")
    # A failed/denied command did not delete anything. It must not poison a
    # later successful, separately confirmed attempt of the task.
    assert f"unconfirmed_rm_permission:attempt_1:{action_ref}" not in completion.gate(store, task_id)[1]


def test_conflicting_response_for_same_identity_is_refused(tmp_path):
    store, task_id = _task(tmp_path)
    evidence.record_permission_response(store, task_id, "attempt_1", "tc_1", "rm -- a", "reject_once")
    with pytest.raises(ValueError, match="identity conflict"):
        evidence.record_permission_response(store, task_id, "attempt_1", "tc_1", "rm -- a", "allow_once")


def test_receipt_persistence_failure_cancels_the_engine_action(tmp_path):
    binding = _binding(tmp_path)
    bindings = BindingRegistry()
    bindings.bind(binding)
    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *_: {"outcome": {"outcome": "selected", "optionId": "yes"}},
        permission_receipt_sink=lambda *_: (_ for _ in ()).throw(OSError("disk full")),
    )
    result = bridge.handle(
        engine_session_id="es_1", wire_session_id="wire_1", wire_request_id=1,
        params={
            "toolCall": {"toolCallId": "rm-one", "kind": "execute", "rawInput": {"command": "rm -- sentinel.txt"}},
            "options": [{"optionId": "yes", "kind": "allow_once"}],
        },
    )
    assert result == {"outcome": {"outcome": "cancelled"}}
