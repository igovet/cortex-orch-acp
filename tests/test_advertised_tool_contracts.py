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
from cortex_runtime.orchestration import skill_proposals
from cortex_runtime.orchestration.contracts import COORDINATOR
from cortex_runtime.orchestration.gateway import READ_ONLY
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.server import channeled_base_tools
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel

GENERIC = {"Structured fields; use the exact advertised properties.",
           "Bounded list of items described by the item schema."}


@pytest.mark.parametrize('operation', ['answer_submit', 'synthesis_submit'])
def test_final_answer_contract_reserves_gap_rendering_for_runtime(operation):
    entry = next(t for t in mcp_tools.tool_definitions('coordinator') if t['name'] == operation)
    text_description = entry['inputSchema']['properties']['text']['description']
    if operation == 'synthesis_submit':
        assert 'only in not_verified' in text_description
    else:
        assert 'Put gap disclosures only in structured not_verified' in text_description
    if operation == 'synthesis_submit':
        assert 'runtime rejects other check-disclosure sections' in text_description
        assert 'Do not explain internal bookkeeping or list successful checks or internal approvals' in text_description
        assert 'unresolved checks only in not_verified' in text_description
    else:
        assert 'refused, not deleted' in text_description
    assert ('keep other context' if operation == 'synthesis_submit' else 'preserve other context') in text_description
    assert 'not_verified' in entry['description']
    assert 'refused' in entry['description']


def test_result_submit_describes_receipt_sources_and_exclusive_report_forms():
    entry = next(t for t in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev", stage="execution")
                 if t["name"] == "result_submit")
    schema = entry["inputSchema"]
    # The report is one required plain string carrying both forms (a root
    # oneOf hides every field from Codex code mode, models fill the plain
    # required list rather than evaluate a conditional, and Claude engines
    # hand-escape object values as JSON but emit top-level strings verbatim).
    assert not {"oneOf", "if", "then", "else"} & set(schema) and "report" in schema["required"]
    report = schema["properties"]["report"]
    assert report["type"] == "string" and not {"oneOf", "anyOf", "if", "properties"} & set(report)
    assert "only that write_report's report_id" in entry["description"]
    assert "command_run" in entry["description"] and "tool_call_ref" in entry["description"]
    assert "write_report" in report["description"] and "r_ plus 12 lowercase hex" in report["description"]
    assert "same attempt" in schema["properties"]["checks"]["items"]["properties"]["tool_call_ref"]["description"]
    assert "closed_by" in schema["properties"]["checks"]["items"]["properties"]["tool_call_ref"]["description"]
    assert "identical" in schema["properties"]["request_key"]["description"]
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
        # One declarative forwarding envelope; every member is preflighted
        # against the currently offered inner schema before any dispatch.
        native_arguments = tool == "read_batch" and path == "args.calls[].arguments"
        if schema.get("additionalProperties") is not False and not native_arguments:
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
        # here (observed in a live Codex session).
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
    writes = {"project_write", "project_edit", "project_mkdir", "project_delete", "project_skill_link", "outside_write", "outside_command"}
    may_write = kind == "worker" and stage != "review" and role_id not in READ_ONLY
    assert (names & writes == writes) if may_write else not names & writes
    may_run = kind == "worker" and "consultant" not in role_id
    assert ("command_run" in names) == may_run
    # The senior consultant works from its packet and retained reports only.
    reads_source = not (kind == "worker" and "consultant" in role_id)
    assert ({"project_read", "project_git"} <= names) == reads_source
    assert bool(names & {"project_read", "project_git"}) == reads_source


def test_write_paths_state_the_real_work_area_not_an_enforced_scope():
    """Writers hold the whole project (gateway._authority); the declared scope
    coordinates writers. No write tool may claim it is enforced."""
    from cortex_runtime.orchestration.gateway import SCHEMAS, WORK_AREA
    for name in ("project_write", "project_edit", "project_mkdir", "project_delete"):
        description = SCHEMAS[name]["properties"]["path"]["description"]
        assert "assignment scope" not in description, name
        assert WORK_AREA in description and "not an enforced boundary" in description, name
    tools = mcp_tools.tool_definitions("worker", role_id="worker-backend-dev", stage="execution")
    (link,) = [t for t in tools if t["name"] == "project_skill_link"]
    assert "no particular declared scope is required" in link["description"]
    assert "whole-project" not in link["description"]


def test_base_tool_descriptions_carry_no_plugin_era_residue():
    import json
    advertised = json.dumps([dict(name=t["name"], description=t["description"], inputSchema=t["inputSchema"])
                             for t in channeled_base_tools()])
    for phrase in (".codex/cortex", "CODEX_HOME", "hook", "native source capture"):
        assert phrase not in advertised, phrase
    (draft,) = [t for t in channeled_base_tools() if t["name"] == "create_draft"]
    template = draft["inputSchema"]["properties"]["template"]["description"]
    # The server refuses a pipeline draft from every channel, coordinator included.
    assert "Only the bound coordinator" not in template
    assert "runtime_owned_operation" in template and "coordinator included" in template
    assert "The runtime alone creates and publishes pipeline editions" in draft["description"]
    # redact_values is still consumed by the store (Store._call), so it stays.
    assert "redact_values" in draft["inputSchema"]["properties"]


def test_blocker_descriptions_accept_a_documented_owner_request_policy():
    from cortex_runtime.contracts import ERROR_HELP
    from cortex_runtime.orchestration.contracts import RESULT
    synthesis = COORDINATOR["synthesis_submit"]["properties"]["not_verified"]["items"]["properties"]["blocker"]["description"]
    worker = RESULT["properties"]["checks"]["items"]["properties"]["blocker"]["description"]
    for text in (synthesis, worker):
        assert '"not requested" alone is not a blocker' in text or '"not requested" alone is not enough' in text
        assert ('documented project policy that requires an explicit owner request which was not given' in text
                or 'rule requiring owner approval is a real reason' in text)
        assert 'paid live check not requested by the owner' in text
    assert "alone is not a blocker" in ERROR_HELP["check_blocker_required"]


def test_not_verified_item_requires_a_localized_description_and_blocker():
    """item stays the exact (English) check name for gap matching; the
    schema asks for description/blocker in the task's selected chat language
    so the rendered final-message block can be fully localized while gap
    closure still depends only on item's exact spelling."""
    item_schema = COORDINATOR["synthesis_submit"]["properties"]["not_verified"]["items"]
    assert item_schema["required"] == ["item", "blocker"]
    assert item_schema["additionalProperties"] is False
    item_description = item_schema["properties"]["item"]["description"]
    assert "Exact name" in item_description and "selected chat language" in item_description
    description_description = item_schema["properties"]["description"]["description"]
    assert "Short, plain wording for the user" in description_description
    assert "user sees this description only" in description_description
    assert "exact name in item for matching" in description_description
    blocker_description = item_schema["properties"]["blocker"]["description"]
    assert "briefly say what stopped the check" in blocker_description
    assert "selected chat language" in blocker_description
    list_description = COORDINATOR["synthesis_submit"]["properties"]["not_verified"]["description"]
    assert "List every unresolved check" in list_description
    assert "Do not explain internal bookkeeping" in list_description
    # answer_submit shares the exact same not_verified schema object.
    assert COORDINATOR["answer_submit"]["properties"]["not_verified"] is COORDINATOR["synthesis_submit"]["properties"]["not_verified"]


def test_worker_question_schema_never_names_skill_proposal():
    import json
    (tool,) = [t for t in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev", stage="execution")
               if t["name"] == "ask_question"]
    assert "skill_proposal" not in json.dumps(tool)
    (coordinator,) = [t for t in mcp_tools.tool_definitions("coordinator") if t["name"] == "user_question_request"]
    assert "skill_proposal" in coordinator["inputSchema"]["properties"]
    assert {"required": ["skill_proposal"]} in coordinator["inputSchema"]["then"]["not"]["anyOf"]


@pytest.mark.parametrize("kind,role_id,stage", [("coordinator", None, None)])
def test_user_question_request_forbids_a_recap_after_the_call(kind, role_id, stage):
    (tool,) = [t for t in mcp_tools.tool_definitions(kind, role_id=role_id, stage=stage) if t["name"] == "user_question_request"]
    description = tool["inputSchema"]["properties"]["question"]["description"]
    assert "exactly once" in description
    assert "without restating" in description or "not restating" in description
    # A same-length rewrite is banned too, not only a shorter recap
    # (observed live); the tool description closes the turn as well.
    assert "write nothing more" in description and "rewriting" in description
    assert "After the call end the turn without writing anything more" in tool["description"]


def test_orchestrator_skill_bans_any_rewrite_after_a_question():
    from pathlib import Path

    skill = (Path(__file__).resolve().parents[1] / "skills/orchestrator/SKILL.md").read_text(encoding="utf-8")
    text = " ".join(skill.split())
    assert "recap, summary or rewritten version of any length" in text
    assert "end the turn right after it without writing anything more" in text


def test_skill_proposal_guidance_matches_the_enforced_action_pairs():
    from pathlib import Path

    (tool,) = [t for t in mcp_tools.tool_definitions("coordinator")
               if t["name"] == "user_question_request"]
    schema = COORDINATOR["user_question_request"]["properties"]["skill_proposal"]["description"]
    advertised = tool["description"]
    root = Path(__file__).resolve().parents[1]
    skill = (root / "skills/skill-management/SKILL.md").read_text(encoding="utf-8")
    guidance = " ".join(skill.split())
    assert "Follow its live schema and approval semantics; do not invent approval labels" in guidance
    proposal = tool["inputSchema"]["properties"]["skill_proposal"]
    assert proposal == COORDINATOR["user_question_request"]["properties"]["skill_proposal"]
    assert set(proposal["required"]) == {"name", "action", "purpose", "evidence"}
    evidence = proposal["properties"]["evidence"]
    assert "distinct retained report" in evidence["description"]
    assert evidence["minItems"] < evidence["maxItems"]
    assert "minItems" not in skill and "maxItems" not in skill
    assert "accepted action-matched pair" in schema and "accepted action-matched pair" in advertised
    assert "An arbitrary first option never approves" in guidance
    for action, accepted in skill_proposals._SAFE_OPTION_PAIRS.items():
        for approval, decline in accepted:
            exact_pair = f"[{approval}, {decline}]"
            assert exact_pair in schema, (action, exact_pair)
            assert exact_pair in advertised, (action, exact_pair)
            assert exact_pair not in skill, (action, exact_pair)


def test_verdict_is_offered_exactly_where_it_is_accepted():
    def result_schema(stage):
        (tool,) = [t for t in mcp_tools.tool_definitions("worker", role_id="worker-debugger", stage=stage)
                   if t["name"] == "result_submit"]
        return tool["inputSchema"]
    review, execution = result_schema("review"), result_schema("execution")
    assert "verdict" in review["properties"] and "verdict" in review["required"]
    assert "verdict" not in execution["properties"] and "verdict" not in execution["required"]
    # Structured findings route fixes; like verdict they are review-only,
    # and optional there.
    assert "findings" in review["properties"] and "findings" not in review["required"]
    assert "findings" not in execution["properties"]


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
        named, _listed_paths = tool["inputSchema"]["properties"]["skill"]["anyOf"]
        for skill in named["enum"]:
            result = mcp_tools.dispatch(store, channel, "read_skill", dict(skill=skill))
            assert result["body"].strip(), (kind, skill)


@pytest.mark.parametrize("kind,role_id", [
    ("coordinator", None), ("worker", "worker-backend-dev"),
    ("worker", "worker-senior-consultant"),
])
def test_read_skill_advertises_only_matching_situational_triggers(kind, role_id):
    from cortex_runtime.portable.roles import situational_skill_triggers

    (entry,) = [t for t in mcp_tools.tool_definitions(kind, role_id=role_id)
                if t["name"] == "read_skill"]
    selector = entry["inputSchema"]["properties"]["skill"]
    assert entry["inputSchema"]["required"] == ["skill"]
    assert "enum" not in selector  # project/user paths are session-specific
    named, listed_paths = selector["anyOf"]
    assert set(named["enum"]) >= {skill for skill, _ in situational_skill_triggers(kind, role_id)}
    assert "session" in listed_paths["description"]
    for skill, trigger in situational_skill_triggers(kind, role_id):
        assert f"{skill}: {trigger}" in entry["description"]
        assert f"{skill}: {trigger}" in selector["description"]
    assert "already-included skills" in entry["description"]
    assert "tool-discipline:" not in entry["description"]
    if role_id == "worker-senior-consultant":
        assert "documentation-sync:" not in entry["description"]
        assert "interactive-resources:" not in entry["description"]
    if kind == "worker":
        assert "cortex-control:" not in entry["description"]


def test_the_consultant_is_offered_no_question_or_access_route():
    """The consultant asks the user nothing; missing
    evidence is a limitation in its report."""
    names = {t["name"] for t in mcp_tools.tool_definitions(
        "worker", role_id="worker-senior-consultant", stage="execution")}
    assert not names & {"ask_question", "access_request"}


@pytest.mark.parametrize("role_id,stage", [("worker-backend-dev", "execution"),
                                         ("worker-explorer", "execution"),
                                         ("worker-code-reviewer", "review")])
def test_worker_question_contract_routes_access_and_localizes_all_form_fields(role_id, stage):
    tool = next(t for t in mcp_tools.tool_definitions("worker", role_id=role_id, stage=stage)
                if t["name"] == "ask_question")
    props = tool["inputSchema"]["properties"]
    for description in (tool["description"], props["question"]["description"],
                        props["questions"]["description"],
                        props["questions"]["items"]["properties"]["question"]["description"]):
        assert "genuine user decision" in description and "access_request" in description
        assert "runtime" in description and "user_question_language" in description
    for field in (props["options"], props["options"]["items"],
                  props["questions"]["items"]["properties"]["options"],
                  props["questions"]["items"]["properties"]["options"]["items"]):
        assert "user_question_language" in field["description"]
        assert "verbatim" in field["description"]
    # A worker-specific rewrite must not mutate coordinator or later tool lists.
    assert "user_question_language" not in COORDINATOR["user_question_request"]["properties"]["options"]["description"]


def test_access_contract_promises_runtime_decision_not_user_question():
    tool = next(t for t in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev", stage="execution")
                if t["name"] == "access_request")
    assert "without a user question" in tool["description"]
    assert "grant" in tool["description"] and "refuse" in tool["description"]


def test_missing_path_absent_round_trip_and_keyless_mutations_are_advertised():
    tools={t['name']:t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')}
    assert 'sha256: "absent"' in tools['project_read']['description']
    assert 'project_write.expected_sha256' in tools['project_read']['description']
    props=tools['project_write']['inputSchema']['properties']
    assert 'returned by project_read' in props['expected_sha256']['description']
    assert 'pass that value directly' in props['expected_sha256']['description']
    for name in ('project_write','project_mkdir','project_delete'):
        schema=tools[name]['inputSchema']
        assert 'request_key' not in schema['required']
        assert 'identical keyless retry' in schema['properties']['request_key']['description']


def test_result_advertises_open_failure_remedies_and_explicit_failed_assignment():
    tool=next(t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')
              if t['name']=='result_submit')
    assert 'open_failed_check' in tool['description'] and 'node_status' in tool['description']
    prop=tool['inputSchema']['properties']['assignment_outcome']
    assert prop['enum']==['failed'] and 'assignment_outcome' not in tool['inputSchema']['required']
    assert 'open_failed_check' in prop['description']


def test_inline_serialization_recovery_keeps_the_owned_reference_route():
    tool=next(t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')
              if t['name']=='result_submit')
    description=tool['description']
    assert 'could not be parsed, nothing reached Cortex' in description
    assert 'create_draft, draft_write and write_report' in description
    assert "only that write_report's report_id" in description
    assert 'report={' not in description


def test_report_brief_sections_and_owned_draft_reference():
    tool=next(t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')
              if t['name']=='result_submit')
    report=tool['inputSchema']['properties']['report']
    assert report['type']=='string' and 'properties' not in report
    for description in (tool['description'],report['description']):
        assert 'one plain string, never an object' in description
        assert 'create_draft, draft_write and write_report' in description
        for section in ('conclusion','evidence','checks','contradictions','limits','next action'):
            assert section in description
    assert 'Markdown sections inside the report, never separate fields' in tool['description']


def test_mistyped_check_invocation_guidance_keeps_exact_identity_closure():
    tool=next(t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')
              if t['name']=='result_submit')
    outcome=tool['inputSchema']['properties']['checks']['items']['properties']['outcome']['description']
    for description in (tool['description'],outcome):
        assert 'never started' in description and 'mistyped' in description
    assert 'a different argv never closes it' in outcome
    assert 'always a real failed check' in outcome
    assert 'changing argv never closes a failure' in tool['description']


def test_plan_scope_is_a_write_reservation_not_read_access():
    tools={t['name']:t for t in mcp_tools.tool_definitions('coordinator')}
    scope=tools['plan_submit']['inputSchema']['properties']['plan']['properties']['tasks']['items']['properties']['scope']
    description=scope['description']
    assert 'Write reservation' in description and 'scheduling overlap' in description
    assert 'Reading needs no scope' in description and 'do not list read-only paths' in description
    assert 'needs to read or change' not in description
    assert 'minItems' in scope and scope['minItems']==1  # existing plan shape is preserved


def test_project_git_context_only_is_explicit_on_result_check_surfaces():
    tools={t['name']:t for t in mcp_tools.tool_definitions('worker',role_id='worker-backend-dev',stage='execution')}
    assert 'context only, not a check receipt' in tools['project_git']['description']
    result=tools['result_submit']
    checks=result['inputSchema']['properties']['checks']
    for description in (result['description'],checks['description'],
                        checks['items']['properties']['tool_call_ref']['description']):
        assert 'project_git' in description and 'context only' in description

def test_safe_file_tool_contracts_and_every_writing_role():
    from cortex_runtime.orchestration.gateway import SCHEMAS
    read = SCHEMAS['project_read']['properties']
    assert read['offset']['minimum'] == read['limit']['minimum'] == 1
    assert read['limit']['maximum'] == 2000
    assert 'line_numbers' in read
    assert 'partial' in SCHEMAS['project_write']['properties']['expected_sha256']['description']
    edit = SCHEMAS['project_edit']['properties']['edits']
    assert edit['minItems'] == 1 and edit['maxItems'] == 64
    assert edit['items']['properties']['new_string']['minLength'] == 0
    import json
    from pathlib import Path
    # tool_definitions owns the role filter; existing profiles need no tool list.
    for profile in json.loads((Path(__file__).resolve().parents[1]/'profiles.json').read_text())['profiles']:
        role = 'worker-' + profile['name'].replace('_', '-')
        names = {t['name'] for t in mcp_tools.tool_definitions('worker', role_id=role, stage='execution')}
        assert ('project_edit' in names) == ('project_write' in names), role
