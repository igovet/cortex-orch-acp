"""T015: delegation.contracts -- internal operation schemas and validation."""
from __future__ import annotations

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as dc

VALID_DELEGATE = dict(
    title="Fix the thing", brief="Do the work.", role="backend-dev",
    acceptance="Tests pass.", request_key="11111111-1111-1111-1111-111111111111",
)


def test_schemas_for_returns_fresh_copies_each_call():
    first = dc.schemas_for("coordinator")
    second = dc.schemas_for("coordinator")
    assert first == second
    assert first is not second
    first["delegate"]["properties"]["title"]["maxLength"] = 1
    # Mutating the first copy must never affect a freshly-requested schema
    # (models "repeated session/generation starts get current schemas").
    assert dc.schemas_for("coordinator")["delegate"]["properties"]["title"]["maxLength"] != 1


def test_schemas_for_unknown_role_kind():
    with pytest.raises(StoreError) as excinfo:
        dc.schemas_for("shared")
    assert excinfo.value.field == "role_kind"


def test_coordinator_operations_closed_set():
    # T016 adds 'decide' (accept/revise/ask_user) alongside T015's original
    # four coordinator operations; T018 adds the shared read-only
    # 'list_sources'/'read_source' source-history operations (see
    # delegation.contracts module docstring and docs/acp/input-history-and-replay.md);
    # T021 adds the shared read-only 'read_capsule' recovery-capsule operation;
    # T024 adds the shared read-only 'read_large_output' operation.
    # T025 adds 'integrate' (technical application of an isolated worker
    # workspace onto the main project root). T029 adds the coordinator-only
    # read-only 'read_diagnostics' observability/usage summary. T035 adds
    # the shared read-only 'read_skill': a role's instructions are handed to
    # the engine that runs it, but the situational shared skills the role
    # text names ("documentation-sync", "knowledge-harvest") had no way to
    # be reached at all -- the plugin host that once loaded a skill by name
    # is gone.
    assert set(dc.schemas_for("coordinator")) == {
        "delegate", "list_events", "clarify", "cancel", "decide", "integrate",
        "list_sources", "read_source", "read_capsule", "read_large_output",
        "read_diagnostics", "read_skill",
    }


def test_worker_operations_closed_set():
    # 'read_skill' (T035) is shared: a worker's own role text tells it to
    # apply `context-compaction` after a context loss, which nothing could
    # deliver once the plugin host that loaded skills by name was gone.
    assert set(dc.schemas_for("worker")) == {
        "ask_question", "publish_result", "list_sources", "read_source", "read_capsule",
        "read_large_output", "read_skill",
    }


def test_no_operation_accepts_a_task_attempt_session_channel_workspace_epoch_id_field():
    forbidden = {"task_id", "attempt_id", "session_id", "channel_id", "workspace_id",
                 "workspace_binding_id", "epoch", "owner_epoch", "assignment_id"}
    for role_kind in ("coordinator", "worker"):
        for operation, schema in dc.schemas_for(role_kind).items():
            overlap = forbidden & set(schema["properties"])
            assert not overlap, f"{role_kind}.{operation} exposes id field(s) {overlap}"


def test_delegate_valid():
    assert dc.validate("coordinator", "delegate", VALID_DELEGATE) == VALID_DELEGATE


def test_delegate_rejects_unadvertised_id_field():
    args = dict(VALID_DELEGATE, task_id="t_deadbeefcafe")
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "delegate", args)
    error = excinfo.value
    assert error.field == "task_id"
    assert "task/attempt/session/channel/workspace/epoch" in error.correction


def test_delegate_rejects_missing_required_field():
    args = {k: v for k, v in VALID_DELEGATE.items() if k != "role"}
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "delegate", args)
    assert excinfo.value.field == "role"
    assert excinfo.value.received == "missing"


def test_delegate_rejects_wrong_type():
    args = dict(VALID_DELEGATE, title=123)
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "delegate", args)
    assert excinfo.value.field == "title"


def test_delegate_rejects_out_of_bound_string():
    args = dict(VALID_DELEGATE, title="x" * 500)
    with pytest.raises(StoreError):
        dc.validate("coordinator", "delegate", args)


def test_delegate_accepts_optional_engine_model_effort():
    # D-T035-04: the coordinator's own route choice for this delegation --
    # all three optional, never required (VALID_DELEGATE carries none of
    # them and still validates, see test_delegate_valid).
    args = dict(VALID_DELEGATE, engine="codex-acp", model="gpt-5.6-terra", effort="medium")
    assert dc.validate("coordinator", "delegate", args) == args
    assert "engine" not in dc.schemas_for("coordinator")["delegate"]["required"]
    assert "model" not in dc.schemas_for("coordinator")["delegate"]["required"]
    assert "effort" not in dc.schemas_for("coordinator")["delegate"]["required"]


def test_unknown_operation_for_role_kind():
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "publish_result", dict(report_ref="r_" + "a" * 12, summary="x", request_key="k"))
    assert excinfo.value.field == "operation"


def test_clarify_requires_assignment_ref():
    with pytest.raises(StoreError):
        dc.validate("coordinator", "clarify", dict(message="hi", request_key="k" * 8))


def test_publish_result_report_ref_pattern():
    good = dict(report_ref="r_0123456789ab", summary="done", request_key="k" * 8)
    assert dc.validate("worker", "publish_result", good) == good
    bad = dict(good, report_ref="not-a-report-id")
    with pytest.raises(StoreError):
        dc.validate("worker", "publish_result", bad)


def test_ask_question_valid():
    args = dict(question="What should I do about X?", request_key="k" * 8)
    assert dc.validate("worker", "ask_question", args) == args


def test_validate_rejects_non_dict_arguments():
    with pytest.raises(StoreError):
        dc.validate("coordinator", "delegate", ["not", "a", "dict"])
