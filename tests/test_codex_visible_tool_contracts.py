"""What Codex code mode actually shows the model, modeled from one observed
projection (investigation r_af50d7468665, session c2016029, transcript line
59): Codex 0.155.1 renders each MCP tool as a TypeScript declaration that
keeps every property's own ``description`` and its type/enum/optionality, but
drops ``minItems``, ``maxItems``, ``minLength``, ``maxLength`` and
``pattern``, and drops the ``description`` of a scalar array's items.

A required array (``minItems`` >= 1) whose visible projected text does not
state that minimum lets the model send ``[]`` and fail plan_submit, exactly
as it did in that session (empty ``scope``/``verification``). This test
walks every tool offered to the coordinator and worker profiles and fails if
any such array's own description omits its minimum.
"""
from __future__ import annotations

import pytest

from cortex_runtime.delegation import mcp_tools
from cortex_runtime.server import channeled_base_tools

PROFILES = [
    ("coordinator", None, None),
    ("worker", "worker-debugger", "execution"),
    ("worker", "worker-code-reviewer", "review"),
    ("worker", None, None),
]


def _codex_visible_projection(schema):
    """The text Codex code mode still shows the model for one schema node:
    its own ``description`` only (bounds, pattern and item descriptions are
    dropped by the projection, not by this helper)."""
    return schema.get("description", "") if isinstance(schema, dict) else ""


def _required_array_gaps(tool, path, schema):
    if not isinstance(schema, dict):
        return
    if schema.get("type") == "array":
        min_items = schema.get("minItems", 0)
        if min_items > 0:
            visible = _codex_visible_projection(schema)
            if str(min_items) not in visible:
                yield f"{tool} {path}: minItems={min_items} not stated in the array's own description"
        yield from _required_array_gaps(tool, f"{path}[]", schema.get("items"))
    elif schema.get("type") == "object":
        for name, child in (schema.get("properties") or {}).items():
            yield from _required_array_gaps(tool, f"{path}.{name}", child)


@pytest.mark.parametrize("kind,role_id,stage", PROFILES)
def test_every_required_array_states_its_minimum_in_its_own_description(kind, role_id, stage):
    tools = list(channeled_base_tools()) + mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage)
    gaps = []
    for tool in tools:
        gaps.extend(_required_array_gaps(tool["name"], "args", tool.get("inputSchema")))
    assert gaps == []


# Live qualification r_236acb13fabb (task t_c6513be0469a): the Codex rollout
# shows the model received the whole verification description, "Send 1 to 16
# items." included, and still sent ``verification: []``. The project
# documented no verification, and the description only said to copy commands
# "exactly from the project's documented verification", so it gave the model
# nothing to send. A trailing count was too weak to override that. Every
# required array must now open with an explicit refusal of ``[]``, and
# verification must say what to send when nothing is documented.


def _required_array_refusal_gaps(tool, path, schema):
    if not isinstance(schema, dict):
        return
    if schema.get("type") == "array":
        min_items = schema.get("minItems", 0)
        visible = _codex_visible_projection(schema)
        if min_items > 0 and not visible.startswith(f"Required: {min_items} to {schema['maxItems']} items; [] is rejected."):
            yield f"{tool} {path}: required array does not open by rejecting []"
        yield from _required_array_refusal_gaps(tool, f"{path}[]", schema.get("items"))
    elif schema.get("type") == "object":
        for name, child in (schema.get("properties") or {}).items():
            yield from _required_array_refusal_gaps(tool, f"{path}.{name}", child)


@pytest.mark.parametrize("kind,role_id,stage", PROFILES)
def test_every_required_array_opens_by_rejecting_an_empty_list(kind, role_id, stage):
    tools = list(channeled_base_tools()) + mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage)
    gaps = []
    for tool in tools:
        gaps.extend(_required_array_refusal_gaps(tool["name"], "args", tool.get("inputSchema")))
    assert gaps == []


def _coordinator_tool(name):
    return next(tool for tool in mcp_tools.tool_definitions("coordinator") if tool["name"] == name)


@pytest.mark.parametrize("tool,path", [("plan_submit", ("plan", "verification")), ("verification_submit", ("verification",))])
def test_verification_description_says_what_to_send_without_documented_commands(tool, path):
    schema = _coordinator_tool(tool)["inputSchema"]
    for name in path:
        schema = schema["properties"][name]
    visible = _codex_visible_projection(schema)
    assert visible.startswith("Required: 1 to 16 items; [] is rejected.")
    assert "investigation-only" in visible
    assert "When the project documents no verification" in visible
    assert "exits nonzero if the requested result is wrong" in visible


def test_plan_submit_tool_description_states_both_required_lists():
    description = _coordinator_tool("plan_submit")["description"]
    assert "plan.tasks and plan.verification each need at least one item" in description
    assert "[] is rejected" in description


def test_empty_verification_rejection_repeats_what_to_send():
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.orchestration.contracts import COORDINATOR, validate

    arguments = dict(request_key="k", plan=dict(
        tasks=[dict(id="a", role="general", objective="Do it.", dependencies=[], scope=["x.txt"], model="auto")],
        verification=[]))
    with pytest.raises(StoreError) as caught:
        validate(COORDINATOR["plan_submit"], arguments)
    correction = caught.value.correction
    assert caught.value.field == "arguments.plan.verification"
    assert "When the project documents no verification" in correction
