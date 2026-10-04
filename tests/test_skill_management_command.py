"""Tests for the explicit skill-management command and portable project layout."""
from __future__ import annotations

import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp.agent import Agent
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.mcp_tools import _enumerate_skills, _shared_read_skill
from cortex_runtime.orchestration import skill_links
from cortex_runtime.portable.roles import NAMED_SKILLS, load_role


def test_command_is_registered_and_only_recognized_by_its_explicit_prefix():
    entry = next(item for item in acp_commands.AVAILABLE_COMMANDS if item["name"] == "skill-management")
    assert entry["input"]["hint"] == "<request>"
    assert "explicitly invokes" in entry["description"]

    assert acp_commands.recognize([{"type": "text", "text": "/skill-management"}]) == (
        "skill-management", "")
    assert acp_commands.recognize([{
        "type": "text", "text": "/skill-management create a skill for release notes",
    }]) == ("skill-management", "create a skill for release notes")
    assert acp_commands.recognize([{
        "type": "text", "text": "please create a project skill for release notes",
    }]) is None
    assert acp_commands.recognize([{
        "type": "text", "text": "/skill-management-extra create a skill",
    }]) is None
    assert acp_commands.recognize([
        {"type": "text", "text": "/skill-management create a skill"},
        {"type": "text", "text": "another block"},
    ]) is None


def test_registered_command_routes_to_the_management_workflow(tmp_path):
    agent = object.__new__(Agent)
    agent._store_for = lambda _project: object()
    calls = []
    agent._run_skill_management = lambda *args: calls.append(args)
    session = {"project": tmp_path, "task_id": None}
    cancel = threading.Event()

    Agent._run_command_body(
        agent, "request-1", "session-1", session,
        ("skill-management", "create release-notes"), cancel,
    )

    assert len(calls) == 1
    assert calls[0][:5] == ("request-1", "session-1", session, "create release-notes", cancel)
    assert callable(calls[0][5])


@pytest.mark.parametrize("task_id", [None, "task-1"])
def test_workflow_passes_the_user_request_and_loads_coordinator_guidance(task_id):
    agent = object.__new__(Agent)
    prompts = []
    agent._run_prompt_body = lambda *args: prompts.append(args)
    session = {"project": "/project", "task_id": task_id}
    cancel = threading.Event()
    request = "Import the reviewed skill from https://example.test/skill"
    replies = []

    Agent._run_skill_management(
        agent, "request-2", "session-2", session, request, cancel, replies.append,
    )

    assert replies == []
    assert len(prompts) == 1
    prompt = prompts[0][3]
    assert prompt.startswith(request + "\n\n")
    assert "skill-management using read_skill" in prompt
    assert "ordinary prose does not activate it" in prompt
    assert prompts[0][4] is cancel


def test_empty_command_reports_usage_without_starting_a_task():
    agent = object.__new__(Agent)
    prompts = []
    replies = []
    agent._run_prompt_body = lambda *args: prompts.append(args)

    Agent._run_skill_management(
        agent, "request-3", "session-3", {"project": "/project", "task_id": None},
        "  ", threading.Event(), replies.append,
    )

    assert prompts == []
    assert replies == ["Usage: /skill-management <request>"]


def test_skill_management_guidance_is_coordinator_only_and_covers_import_review():
    assert "skill-management" in NAMED_SKILLS["coordinator"]
    assert "skill-management" not in NAMED_SKILLS["worker"]

    coordinator_schema = _enumerate_skills(
        {"properties": {"skill": {"type": "string"}}}, "coordinator", None)
    worker_schema = _enumerate_skills(
        {"properties": {"skill": {"type": "string"}}}, "worker", "worker-general")
    named = coordinator_schema["properties"]["skill"]["anyOf"][0]
    assert "skill-management" in named["enum"]
    worker_named = worker_schema["properties"]["skill"]["anyOf"][0]
    assert "skill-management" not in worker_named["enum"]

    body = _shared_read_skill(
        None, SimpleNamespace(role="coordinator"), {"skill": "skill-management"})
    assert "CORTEX-SOURCE.md" in body["body"]
    assert "immutable revision" in body["body"]
    assert "SHA-256" in body["body"]
    assert "untrusted source data" in body["body"]
    assert "project_skill_link" in body["body"]
    assert "Do not create it automatically" in body["body"]
    assert "no particular declared scope is required" in body["body"]
    assert "agreed work area coordinating writers" in body["body"]
    assert "permission request accepted by the user" in body["body"]
    assert "whole-project write scope" not in body["body"]
    assert "user_question_request carrying skill_proposal" in body["body"]
    assert "Follow the live schema" in body["body"]
    assert "[Create the skill, Not now]" not in body["body"]
    assert "[Update the skill, Not now]" not in body["body"]
    assert "ask the coordinator" not in body["body"]
    assert "An arbitrary first option never approves" in " ".join(body["body"].split())
    assert "merely because skill files or directories exist" in body["body"]
    with pytest.raises(StoreError, match="unknown_skill"):
        _shared_read_skill(None, SimpleNamespace(role="worker", attempt_id=None), {"skill": "skill-management"})


def test_codex_source_and_claude_link_expose_the_same_skill_tree(tmp_path):
    source = tmp_path / ".agents" / "skills" / "release-notes"
    source.mkdir(parents=True)
    skill_text = (
        "---\n"
        "name: release-notes\n"
        "description: Create release notes from verified changes.\n"
        "---\n"
        "# Release notes\n"
    )
    (source / "SKILL.md").write_text(skill_text, encoding="utf-8")
    (source / "CORTEX-SOURCE.md").write_text(
        "Source URL: https://example.test/skill\nRevision: sha256:fixture\n",
        encoding="utf-8",
    )
    references = source / "references"
    references.mkdir()
    (references / "format.md").write_text("# Format\n", encoding="utf-8")

    receipt = skill_links.apply(
        tmp_path, {"action": "link", "name": "release-notes"}, {"possible": False},
    )

    claude_path = tmp_path / ".claude" / "skills" / "release-notes"
    assert receipt["changed"] is True
    assert claude_path.is_symlink()
    assert claude_path.readlink().as_posix() == "../../.agents/skills/release-notes"
    assert (source / "SKILL.md").read_text(encoding="utf-8") == skill_text
    assert (claude_path / "SKILL.md").read_text(encoding="utf-8") == skill_text
    assert (claude_path / "CORTEX-SOURCE.md").read_text(encoding="utf-8").endswith(
        "Revision: sha256:fixture\n")
    assert (claude_path / "references" / "format.md").read_text(encoding="utf-8") == "# Format\n"


def test_cortex_skill_has_valid_basic_frontmatter():
    skill_path = Path(__file__).resolve().parents[1] / "skills" / "skill-management" / "SKILL.md"
    text = skill_path.read_text(encoding="utf-8")
    assert text.startswith("---\n")
    header = text.split("\n---\n", 1)[0]
    assert "name: skill-management" in header.splitlines()
    description = next(line for line in header.splitlines() if line.startswith("description: "))
    assert description.removeprefix("description: ").strip()


def test_skill_creator_provenance_is_exact_and_reviewable():
    source_path = Path(__file__).resolve().parents[1] / "skills" / "skill-management" / "CORTEX-SOURCE.md"
    source = source_path.read_text(encoding="utf-8")

    assert "https://github.com/anthropics/skills/tree/main/skills/skill-creator" in source
    assert "33375500bcea98d610eb30ce10ac4e59b89c390d" in source
    assert "Apache-2.0" in source
    assert "No upstream source files, scripts, templates, schemas, or assets are copied" in source
    assert "Retrieved and checked: 2026-09-27" in source
