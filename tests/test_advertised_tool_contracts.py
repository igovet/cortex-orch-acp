"""What every channel is offered: complete schemas, and only tools it can use.

A model can follow only the schema it is shown. Placeholder descriptions,
rules that lived only in server code, a field advertised to roles that are
always refused for sending it, and tools a channel can never use each turned
into failed calls (live, 2026-09-22).
"""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.gateway import READ_ONLY
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.server import channeled_base_tools
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel

GENERIC = {"Structured fields; use the exact advertised properties.",
           "Bounded list of items described by the item schema."}
PROFILES = [
    ("coordinator", None, None),
    ("worker", "worker-debugger", "execution"),
    ("worker", "worker-explorer", "execution"),
    ("worker", "worker-code-reviewer", "review"),
    ("worker", "worker-senior-consultant", "execution"),
    ("worker", None, None),
]


def _problems(tool, path, schema):
    if not isinstance(schema, dict):
        return
    kind, description = schema.get("type"), schema.get("description", "")
    if (kind in ("string", "integer", "number", "boolean") or "enum" in schema) and (
            len(description) < 12 or description in GENERIC):
        yield f"{tool} {path}: no real description"
    if kind == "string" and "enum" not in schema and "maxLength" not in schema:
        yield f"{tool} {path}: string without maxLength"
    # A command's own timeout is deliberately unbounded: a long build or
    # backfill must be able to run to completion
    # (test_worker_running_handle_instruction). Every other number is bounded.
    unbounded_by_design = path == "args.timeout_seconds"
    if kind in ("integer", "number") and ("minimum" not in schema or ("maximum" not in schema and not unbounded_by_design)):
        yield f"{tool} {path}: number without bounds"
    if kind == "object":
        if schema.get("additionalProperties") is not False:
            yield f"{tool} {path}: object open to undeclared fields"
        if "required" not in schema:
            yield f"{tool} {path}: object without a required list"
        for name, child in (schema.get("properties") or {}).items():
            yield from _problems(tool, f"{path}.{name}", child)
    if kind == "array":
        if "maxItems" not in schema:
            yield f"{tool} {path}: array without maxItems"
        # A code-mode host (e.g. Codex) shows the model only this array-level
        # description, never minItems/maxItems or the item schema's own
        # description, so the bounds and the item's meaning must both live
        # here (investigation r_af50d7468665).
        if len(description) < 40 or description in GENERIC:
            yield f"{tool} {path}: no real description"
        min_items, max_items = schema.get("minItems", 0), schema.get("maxItems")
        if min_items > 0 and not (str(min_items) in description and (max_items is None or str(max_items) in description)):
            yield f"{tool} {path}: array description missing its minItems/maxItems bounds"
        yield from _problems(tool, f"{path}[]", schema.get("items"))


@pytest.mark.parametrize("kind,role_id,stage", PROFILES)
def test_every_offered_tool_is_fully_described(kind, role_id, stage):
    tools = list(channeled_base_tools()) + mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage)
    problems = []
    for tool in tools:
        description = tool.get("description", "")
        if description.startswith("Delegation operation") or len(description) < 40:
            problems.append(f"{tool['name']}: placeholder tool description")
        problems.extend(_problems(tool["name"], "args", tool.get("inputSchema")))
    assert problems == []


@pytest.mark.parametrize("kind,role_id,stage", PROFILES[:-1])
def test_no_channel_is_offered_a_tool_the_gateway_always_refuses(kind, role_id, stage):
    names = {t["name"] for t in mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage)}
    writes = {"project_write", "project_mkdir", "project_delete", "project_skill_link", "outside_write", "outside_command"}
    may_write = kind == "worker" and stage != "review" and role_id not in READ_ONLY
    assert (names & writes == writes) if may_write else not names & writes
    may_run = kind == "worker" and "consultant" not in role_id
    assert ("command_run" in names) == may_run


@pytest.mark.parametrize("kind,role_id,stage", [("coordinator", None, None)])
def test_user_question_request_forbids_a_recap_after_the_call(kind, role_id, stage):
    (tool,) = [t for t in mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage) if t["name"] == "user_question_request"]
    description = tool["inputSchema"]["properties"]["question"]["description"]
    assert "exactly once" in description
    assert "without restating" in description or "not restating" in description


def test_verdict_is_offered_exactly_where_it_is_accepted():
    def result_schema(stage):
        (tool,) = [t for t in mcp_tools.tool_definitions("worker", role_id="worker-debugger", stage=stage)
                   if t["name"] == "result_submit"]
        return tool["inputSchema"]
    review, execution = result_schema("review"), result_schema("execution")
    assert "verdict" in review["properties"] and "verdict" in review["required"]
    assert "verdict" not in execution["properties"] and "verdict" not in execution["required"]


def _channels(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    coordinator = resolve_channel(store, result.channel_id, result.secret)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        channel_id, secret = issue_channel_tx(db, result.task_id, "worker", current_owner_epoch(db, result.task_id),
                                              parent_channel_id=coordinator.channel_id,
                                              assignment_id="a_test1", attempt_id="xa_test1")
        db.commit()
    return store, coordinator, resolve_channel(store, channel_id, secret)


def test_every_offered_skill_id_is_actually_readable(tmp_path):
    store, coordinator, worker = _channels(tmp_path)
    for channel, kind in ((coordinator, "coordinator"), (worker, "worker")):
        (tool,) = [t for t in mcp_tools.tool_definitions(kind) if t["name"] == "read_skill"]
        for skill in tool["inputSchema"]["properties"]["skill"]["enum"]:
            result = mcp_tools.dispatch(store, channel, "read_skill", dict(skill=skill))
            assert result["body"].strip(), (kind, skill)
