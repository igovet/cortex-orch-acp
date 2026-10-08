"""Every advertised Cortex MCP argument contract agrees with its validator."""

import pytest
from jsonschema import Draft202012Validator

from cortex_runtime.contracts import BY_NAME, StoreError, TOOLS, validate as validate_base
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.server import DELEGATION_READ_OPERATIONS
from cortex_runtime.channeled_tools import channeled_base_tools
from cortex_runtime.orchestration import mcp,gateway


READ_NAMES = {
    "read_draft", "list_reports", "read_report", "task_get_context", "project_read", "project_git", "list_sources",
    "read_source", "read_capsule", "read_large_output", "read_diagnostics",
    "read_skill", "list_route_options",
}


def _sample(rule):
    kind = rule["type"]
    if kind == "integer":
        return rule["minimum"]
    if kind == "boolean":
        return False
    if kind == "array":
        return [_sample(rule["items"]) for _ in range(rule.get("minItems", 0))]
    if kind == "object":
        return {name: _sample(rule["properties"][name]) for name in rule["required"]}
    if "enum" in rule:
        return rule["enum"][0]
    pattern = rule.get("pattern", "")
    if pattern == r"^r_[0-9a-f]{12}$":
        return "r_" + "a" * 12
    if pattern == r"^d_[0-9a-f]{12}$":
        return "d_" + "a" * 12
    if pattern == r"^[0-9a-f]{64}$":
        return "a" * 64
    if pattern == r"^[1-9][0-9]*$":
        return "1"
    if pattern.startswith("^("):
        return pattern[2:].split("|")[0]
    return "x" * max(1, rule.get("minLength", 1))


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_every_advertised_tool_uses_its_current_validator_and_read_classification(role):
    entries = list(channeled_base_tools()) + mcp_tools.tool_definitions(role)
    advertised = {entry["name"]: entry for entry in entries}
    expected_base = {entry["name"] for entry in channeled_base_tools()}
    expected_delegation = set(delegation_contracts.schemas_for(role))
    # A coordinator has no write grant and may not execute: the gateway
    # refuses those tools every time, so they are not offered to it.
    never_usable = ({"project_write", "project_mkdir", "project_delete", "project_skill_link", "outside_write", "outside_command",
                     "command_run"} if role == "coordinator" else set())
    expected_names = expected_base | (expected_delegation - never_usable)
    # Keep this contract-derived: adding a new role operation must not leave a
    # stale hand-count in this coverage test (for example complete_assignment).
    assert len(advertised) == len(entries) == len(expected_names)
    assert set(advertised) == expected_names
    assert set(mcp_tools._ROLE_HANDLERS[role]) | set(mcp.schemas(role)) | set(gateway.SCHEMAS) == expected_delegation
    # dispatch() tries gateway.SCHEMAS, then mcp.schemas(role), then
    # _ROLE_HANDLERS[role] in that fixed order and returns on the first
    # match: a name present in more than one of these groups is routed only
    # to the earliest one, silently shadowing any handler in a later group
    # (this is exactly how list_route_options -- advertised and present in
    # _ROLE_HANDLERS -- once fell into mcp.schemas('coordinator') too and hit
    # that module's own `raise StoreError('unknown_operation')` fall-through
    # instead of ever calling its real handler). Every advertised operation
    # must therefore belong to exactly one of these three dispatch groups.
    handler_names = set(mcp_tools._ROLE_HANDLERS[role])
    runtime_names = set(mcp.schemas(role))
    gateway_names = set(gateway.SCHEMAS)
    assert not (handler_names & runtime_names), handler_names & runtime_names
    assert not (handler_names & gateway_names), handler_names & gateway_names
    assert not (runtime_names & gateway_names), runtime_names & gateway_names
    for name, entry in advertised.items():
        schema = entry["inputSchema"]
        Draft202012Validator.check_schema(schema)
        assert schema["additionalProperties"] is False
        assert entry["annotations"]["readOnlyHint"] == (name in READ_NAMES)
        args = {key: _sample(schema["properties"][key]) for key in schema["required"]}
        if name in {"user_question_request", "ask_question"}:
            args.update(question="Which target?", options=["Small target", "Large target"])
        if name == "result_submit":
            args["report"] = "# Result\n\nObserved.\n"
        assert not list(Draft202012Validator(schema).iter_errors(args)), name
        validator = (lambda a: validate_base(name, a)) if name in BY_NAME else (lambda a: delegation_contracts.validate(role, name, a))
        validator(args)
        bad = dict(args, invented_selector="x")
        assert list(Draft202012Validator(schema).iter_errors(bad)), name
        with pytest.raises(StoreError, match="invalid_arguments"):
            validator(bad)
        if name in READ_NAMES - {"project_git"}:
            assert "request_key" not in schema["properties"], name
        elif name != "synthesis_submit":
            assert "request_key" in schema["properties"], name


def test_every_delegation_read_bypasses_mutation_epoch_gate():
    assert DELEGATION_READ_OPERATIONS == READ_NAMES - {"read_draft", "list_reports", "read_report", "project_git"}


@pytest.mark.parametrize("args, valid", [
    ({"report": "# Report", "report_title": "Report", "summary": "Done", "checks": [], "request_key": "once"}, True),
    ({"report": "# Report", "summary": "Done", "checks": [], "request_key": "once"}, True),
    ({"report": "", "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": {"title": "Report", "markdown": "# Report"}, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": "r_" + "a" * 12, "summary": "Done", "checks": [], "request_key": "once"}, True),
    ({"report": {"report_ref": "r_" + "a" * 12}, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": {}, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": {"title": "Report"}, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report_ref": "r_" + "a" * 12, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": {"title": "Report", "markdown": "# Report", "report_ref": "r_" + "a" * 12}, "summary": "Done", "checks": [], "request_key": "once"}, False),
    ({"report": "r_" + "a" * 12, "summary": "Done", "checks": [{"name": "GUI", "outcome": "not_run"}], "request_key": "once"}, False),
    ({"report": "r_" + "a" * 12, "summary": "Done", "checks": [{"name": "GUI", "outcome": "unverified", "blocker": "   "}], "request_key": "once"}, False),
    ({"report": "r_" + "a" * 12, "summary": "Done", "checks": [{"name": "pytest", "outcome": "passed", "command": ["pytest"]}], "request_key": "once"}, True),
])
def test_result_report_and_check_combinations_match_validator(args, valid):
    schema = next(t["inputSchema"] for t in mcp_tools.tool_definitions("worker") if t["name"] == "result_submit")
    assert (not list(Draft202012Validator(schema).iter_errors(args))) is valid
    if valid:
        delegation_contracts.validate("worker", "result_submit", args)
    else:
        with pytest.raises(StoreError, match="invalid_arguments"):
            delegation_contracts.validate("worker", "result_submit", args)


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_read_skill_selector_keeps_the_validated_single_field_contract(role):
    (entry,) = [tool for tool in mcp_tools.tool_definitions(role) if tool["name"] == "read_skill"]
    schema = entry["inputSchema"]
    assert schema["required"] == ["skill"]
    assert set(schema["properties"]) == {"skill"}
    assert "documentation-sync: before assessing documentation impact" in schema["properties"]["skill"]["description"]
    assert not list(Draft202012Validator(schema).iter_errors({"skill": "documentation-sync"}))
    delegation_contracts.validate(role, "read_skill", {"skill": "documentation-sync"})


def test_review_findings_contract_agrees_with_its_validator():
    """The optional review findings (with a boolean) are checked the same way
    by the advertised JSON Schema and the runtime validator."""
    (entry,) = [t for t in mcp_tools.tool_definitions("worker", role_id="worker-code-reviewer", stage="review")
                if t["name"] == "result_submit"]
    schema = entry["inputSchema"]
    finding_schema = schema["properties"]["findings"]["items"]
    finding = {name: _sample(finding_schema["properties"][name]) for name in finding_schema["required"]}
    args = dict({key: _sample(schema["properties"][key]) for key in schema["required"]},
                report="r_" + "a" * 12, findings=[finding])
    assert not list(Draft202012Validator(schema).iter_errors(args))
    delegation_contracts.validate("worker", "result_submit", args)
    for bad in (dict(finding, cross_cutting="false"), dict(finding, files=[]), dict(finding, node="_fix_1_x")):
        wrong = dict(args, findings=[bad])
        assert list(Draft202012Validator(schema).iter_errors(wrong))
        with pytest.raises(StoreError, match="invalid_arguments"):
            delegation_contracts.validate("worker", "result_submit", wrong)
