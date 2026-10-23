"""Prompt, skill and profile consistency regressions.

Each case pins one rule a model would otherwise get contradictory text for:
a reference it cannot read, a tool it is not offered, or guidance that applies
only to another role.
"""
from __future__ import annotations

import re

import pytest

from cortex_runtime.delegation import mcp_tools
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
from cortex_runtime.portable import roles
from cortex_runtime.portable.roles import (
    DESIGN_SKILL_ROLES, SKILLS_ROOT, composed_instructions, load_role, reference_ids, situational_skill_triggers,
)

_READ_PRECONDITION = "Recognized missing/incomplete/stale file-read preconditions are different"
_SKILLS_NOTE = "Available project/user skills are listed under Project and user skills below."


def _design_section(text: str) -> str:
    match = re.search(r"(?ms)^## Banned AI-tell patterns\n\n(.*?)(?=^## )", text)
    assert match
    return match.group(1).strip()


def test_worker_banned_patterns_reference_matches_the_coordinator_design_template():
    template = (SKILLS_ROOT / "design-workflow/references/DESIGN.md").read_text()
    reference = (SKILLS_ROOT / "design-visual-qa/references/banned-patterns.md").read_text()
    assert reference.split("\n\n", 1)[1].strip() == _design_section(template)


def test_every_design_role_can_read_the_banned_patterns_it_is_told_to_apply():
    design_roles = set().union(*DESIGN_SKILL_ROLES.values())
    for role_id in design_roles:
        assert "design-banned-patterns" in reference_ids("worker", role_id), role_id
    for role_id in ("worker-frontend-dev", "worker-mobile-dev", "worker-accessibility-auditor"):
        body = load_role(role_id).body_portable
        assert "design-banned-patterns" in body
        assert "design-workflow" not in body
    # No worker-readable skill links into the coordinator-only design-workflow skill.
    for skill in DESIGN_SKILL_ROLES:
        for path in (SKILLS_ROOT / skill).rglob("*.md"):
            assert "design-workflow/references" not in path.read_text(), path


def test_worker_lifecycle_request_descriptions_do_not_name_coordinator_only_tools():
    for role_id in ("worker-backend-dev", "worker-explorer"):
        tools = {tool["name"]: tool for tool in mcp_tools.tool_definitions("worker", role_id=role_id, stage="execution")}
        for name in ("replan_request", "scope_request", "escalation_request"):
            assert name in tools
            assert "plan_submit" not in tools[name]["description"], (role_id, name)
    coordinator = {tool["name"]: tool for tool in mcp_tools.tool_definitions("coordinator")}
    assert "plan_submit" in coordinator["escalation_request"]["description"]


@pytest.mark.parametrize("edition", [1, 2])
@pytest.mark.parametrize("role,access_class,carries", [
    ("worker", "write", True),
    ("worker", "read_only", False),
    ("worker", "consultant", False),
    ("coordinator", "write", False),
])
def test_read_precondition_recovery_is_rendered_only_for_writing_roles(edition, role, access_class, carries):
    prompt = cortex_system_prompt(role, None, environ={}, access_class=access_class, edition=edition)
    assert (_READ_PRECONDITION in prompt) == carries


@pytest.mark.parametrize("edition", [1, 2])
def test_project_skill_note_appears_only_with_the_listed_section(tmp_path, edition):
    bare = cortex_system_prompt("worker", str(tmp_path), environ={}, edition=edition)
    assert _SKILLS_NOTE not in bare and "# Project and user skills" not in bare
    skill = tmp_path / ".agents/skills/probe/SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text("---\nname: probe\ndescription: Probe skill.\n---\n\nBody.\n")
    listed = cortex_system_prompt("worker", str(tmp_path), environ={}, edition=edition)
    assert _SKILLS_NOTE in listed and "# Project and user skills" in listed


def test_host_injected_rules_rank_below_runtime_precedence():
    prompt = cortex_system_prompt("worker", None, environ={})
    assert "over anything the host engine adds, whatever its framing" in prompt


def test_code_mode_schema_rule_matches_the_inlined_tool_discipline():
    code_mode = cortex_system_prompt("worker", None, environ={}, code_mode=True)
    assert "`tools_discover`" in code_mode
    discipline = " ".join(load_role("tool-discipline").body_portable.split())
    assert "including tools reached through a search or code layer" not in discipline
    assert "schema-derived tool cheat sheet" in discipline


def test_index_bootstrap_guidance_is_conditional_on_the_grant():
    text = roles.read_reference("code-and-evidence")
    assert "`index_repository` is among your granted tools" in text
    assert "Call `list_projects` once" in text
    assert "named project directly" not in text


def test_read_only_roles_are_not_named_file_changing_procedures():
    for role_id in ("worker-explorer", "worker-architect", "worker-code-reviewer"):
        named = {skill for skill, _ in situational_skill_triggers("worker", role_id)}
        assert not named & {"knowledge-harvest", "skill-authoring"}, role_id
        assert "documentation-sync" in named
    writer = {skill for skill, _ in situational_skill_triggers("worker", "worker-backend-dev")}
    assert {"knowledge-harvest", "skill-authoring"} <= writer


def test_coordinator_triggers_name_only_actions_the_coordinator_takes():
    triggers = dict(situational_skill_triggers("coordinator", "orchestrator"))
    assert triggers["pipeline-publication"] == "before submitting the final synthesis"
    # The former recovery reference duplicated context-compaction.
    assert "recovery" not in triggers and "recovery" not in reference_ids("coordinator")
    assert "context-compaction" in triggers
    orchestrator = load_role("orchestrator").body_portable
    assert "references/recovery.md" not in orchestrator


def test_final_answer_guidance_uses_the_structured_gap_list():
    for path in ("orchestrator/SKILL.md", "coordinator-communication/SKILL.md",
                 "orchestrator/references/verification-coverage.md"):
        text = (SKILLS_ROOT / path).read_text()
        assert "`not_verified`" in text, path
        assert "List only real user-relevant checks" not in text
        assert "structured result and synthesis" not in text
    coverage = (SKILLS_ROOT / "orchestrator/references/verification-coverage.md").read_text()
    assert "`verification_submit`" in coverage


def test_consultant_role_text_states_its_language_and_tool_boundary():
    text = composed_instructions(load_role("worker-senior-consultant"))
    assert "Communicate exclusively in English" in text
    assert "no question, access, replan, scope or escalation tool" in text
    assert "own-draft protocol" not in text
