"""T015: delegation.contracts -- internal operation schemas and validation."""
from __future__ import annotations

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as dc
from cortex_runtime.delegation import mcp_tools

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
        "read_diagnostics", "read_skill", "update_plan", "read_plan", "prepare_candidate",
        "request_check", "read_check", "assess_check",
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


def test_delegate_accepts_empty_optional_plan_assignment_ref_as_omitted():
    args = dict(VALID_DELEGATE, plan_assignment_ref="")
    assert dc.validate("coordinator", "delegate", args) == args
    schema = dc.schemas_for("coordinator")["delegate"]["properties"]["plan_assignment_ref"]
    assert schema["minLength"] == 0


def test_delegate_advertises_structured_qa_handoff_contract():
    schema = dc.schemas_for("coordinator")["delegate"]
    assert schema["properties"]["handoff_mode"]["enum"] == ["independent", "report_review"]
    assert "predecessor_report_ref" in schema["properties"]
    assert "exact" in schema["properties"]["predecessor_report_ref"]["description"]
    advertised = next(
        tool for tool in mcp_tools.tool_definitions("coordinator") if tool["name"] == "delegate"
    )
    assert advertised["inputSchema"]["additionalProperties"] is False
    assert "report_review" in advertised["inputSchema"]["properties"]["handoff_mode"]["description"]


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


def test_read_skill_advertises_only_canonical_skill_field_and_rejects_topic():
    schema = dc.schemas_for("coordinator")["read_skill"]
    assert schema["required"] == ["skill"]
    assert set(schema["properties"]) == {"skill"}
    description = schema["properties"]["skill"]["description"]
    assert "only" in description and "accepted input field" in description
    assert "`topic`" in description

    advertised = next(
        tool for tool in mcp_tools.tool_definitions("coordinator")
        if tool["name"] == "read_skill"
    )
    assert "exactly one accepted field" in advertised["description"]
    assert "`topic`" in advertised["description"]
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "read_skill", {"topic": "pipeline-publication"})
    assert excinfo.value.field == "topic"

    assert dc.validate(
        "coordinator", "read_skill", {"skill": "pipeline-publication"}
    ) == {"skill": "pipeline-publication"}


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


def test_advertised_delegation_objects_are_closed_at_every_level():
    for role_kind in ("coordinator", "worker"):
        for operation, schema in dc.schemas_for(role_kind).items():
            assert schema["additionalProperties"] is False, (role_kind, operation)
            for rule in schema["properties"].values():
                if rule.get("type") == "array" and rule.get("items", {}).get("type") == "object":
                    assert rule["items"]["additionalProperties"] is False, (role_kind, operation)


def test_array_min_items_match_runtime_validation():
    schema = dc.schemas_for("coordinator")
    assert schema["prepare_candidate"]["properties"]["assignment_refs"]["minItems"] == 1
    assert schema["request_check"]["properties"]["argv"]["minItems"] == 1
    with pytest.raises(StoreError) as empty_candidate:
        dc.validate("coordinator", "prepare_candidate", {"assignment_refs": [], "request_key": "k"})
    assert empty_candidate.value.field == "assignment_refs"
    with pytest.raises(StoreError) as empty_argv:
        dc.validate("coordinator", "request_check", {
            "candidate_ref": "candidate",
            "candidate_digest": "a" * 64,
            "argv": [],
            "request_key": "k",
        })
    assert empty_argv.value.field == "argv"


def test_tools_list_delegation_schema_matches_validator_for_unknown_keys_and_min_items():
    advertised = {item["name"]: item["inputSchema"] for item in mcp_tools.tool_definitions("coordinator")}
    assert advertised["prepare_candidate"]["properties"]["assignment_refs"]["minItems"] == 1
    assert advertised["prepare_candidate"]["additionalProperties"] is False
    assert advertised["update_plan"]["properties"]["requirement_bindings"]["items"]["additionalProperties"] is False
    with pytest.raises(StoreError):
        dc.validate("coordinator", "prepare_candidate", {
            "assignment_refs": [], "request_key": "k",
        })
    with pytest.raises(StoreError):
        dc.validate("coordinator", "delegate", dict(VALID_DELEGATE, extra="rejected"))


def test_request_check_schema_guides_first_call_requirement_binding_and_reuse():
    advertised = {item["name"]: item for item in mcp_tools.tool_definitions("coordinator")}
    request_check = advertised["request_check"]
    description = request_check["description"]
    binding_description = request_check["inputSchema"]["properties"]["requirement_bindings"]["description"]
    assert "Before this first call" in description
    assert "read_plan" in description
    assert "unbound check followed by a bound repeat is unnecessary" in description
    assert "copy every applicable requirement_ref" in binding_description
    assert "Never guess a revision" in binding_description
    assert "marks that result as reused" in description


def test_update_plan_schema_guides_first_call_and_rejects_legacy_aliases():
    advertised = {item["name"]: item for item in mcp_tools.tool_definitions("coordinator")}
    update_plan = advertised["update_plan"]
    schema = update_plan["inputSchema"]
    assert schema["required"] == ["action", "request_key"]
    assert "create_requirement" in schema["properties"]["action"]["description"]
    assert "title" in schema["properties"]["action"]["description"]
    assert "statement" in schema["properties"]["statement"]["description"]
    assert "first call" in update_plan["description"]
    assert "legacy aliases" in update_plan["description"]
    assert "assignment_id returned by update_plan/action='add_assignment'" in update_plan["description"]
    assert "assignment_ref returned by delegate()" in schema["properties"]["assignment_ref"]["description"]
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "update_plan", {
            "title": "Plan", "summary": "Plan the work", "body": "Details",
            "request_key": "legacy-call",
        })
    assert excinfo.value.received == "unadvertised field(s)"
    assert excinfo.value.field == "body, summary, title"


def test_decide_schema_guides_required_assignment_target_and_rejects_omission():
    advertised = {item["name"]: item for item in mcp_tools.tool_definitions("coordinator")}
    decide = advertised["decide"]
    schema = decide["inputSchema"]
    assert "assignment_ref" in schema["required"]
    assert "copy it exactly from the relevant delegate result" in decide["description"]
    assert "literal 'task'" in schema["properties"]["assignment_ref"]["description"]
    with pytest.raises(StoreError) as excinfo:
        dc.validate("coordinator", "decide", {
            "decision": "accept", "rationale": "The retained result passes.",
            "request_key": "decision-without-target",
        })
    assert excinfo.value.field == "assignment_ref"
    assert excinfo.value.received == "missing"
