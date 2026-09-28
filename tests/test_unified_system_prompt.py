"""The unified Cortex system prompt reaches both engines as the same text."""
from __future__ import annotations

import datetime

import pytest

from cortex_runtime.delegation.live_adapters import _session_new_meta_for
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt


def test_prompt_is_role_specific_engine_neutral_and_carries_the_environment(tmp_path):
    (tmp_path / ".git").mkdir()
    day = datetime.date(2026, 9, 28)
    worker = cortex_system_prompt("worker", str(tmp_path), today=day, environ={})
    coordinator = cortex_system_prompt("coordinator", str(tmp_path), today=day, environ={})
    assert worker.startswith("# Cortex session\n\nYou are a Cortex worker")
    assert coordinator.startswith("# Cortex session\n\nYou are the Cortex coordinator")
    for text in (worker, coordinator):
        assert f"- Working directory: {tmp_path}" in text and "- Git repository: yes" in text
        assert "- Date (UTC): 2026-09-28" in text
        assert "`read_skill`" in text and "code index" in text
        assert "Tools have three sources" in text
        assert "`cortex` MCP server" in text and "`codebase-memory-mcp`" in text
        assert "native shell, file and search tools" in text
        # Project rule file names are named; engines and products are not.
        lowered = text.lower().replace("claude.md", "")
        assert "codex" not in lowered and "claude" not in lowered
    with pytest.raises(ValueError):
        cortex_system_prompt("reviewer", None)


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_both_engines_receive_the_same_prompt_before_the_same_role_text(tmp_path, role):
    claude = _session_new_meta_for("claude-agent-acp", "0.78.0", "Role text.", role=role, project_root=str(tmp_path))
    codex = _session_new_meta_for("codex-acp", "1.13.0", "Role text.", role=role, project_root=str(tmp_path))
    assert codex["systemPrompt"] == "Role text." and codex["cortexNativeContext"] is False
    assert claude["systemPrompt"] == codex["cortexBaseInstructions"].rstrip() + "\n\nRole text."
    assert codex["cortexBaseInstructions"] == cortex_system_prompt(role, str(tmp_path))


def _skill(base, rel, name, description=None):
    directory = base / rel / name
    directory.mkdir(parents=True)
    (directory / "SKILL.md").write_text(
        f'---\nname: {name}\ndescription: "{description or "About " + name}."\n---\n\nBody.\n')
    return directory


def test_skills_from_every_engine_directory_are_listed_once_for_every_engine(tmp_path):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    for rel, name in ((".agents/skills", "p-agents"), (".claude/skills", "p-claude"), (".codex/skills", "p-codex"),
                      ("skills", "p-bare")):
        _skill(project, rel, name)
    for rel, name in ((".agents/skills", "h-agents"), (".claude/skills", "h-claude"), (".codex/skills", "h-codex"),
                      (".codex/skills", ".system")):
        _skill(home, rel, name)
    # Claude's project link to a shared skill, a link escaping the project, and a directory without SKILL.md.
    (project / ".claude/skills/p-agents").symlink_to("../../.agents/skills/p-agents", target_is_directory=True)
    outside = _skill(tmp_path, "outside", "leak")
    (project / ".agents/skills/leak").symlink_to(outside, target_is_directory=True)
    (project / ".agents/skills/empty").mkdir()
    many = [_skill(project, ".agents/skills", f"bulk-{index:03d}") for index in range(60)]
    text = cortex_system_prompt("worker", str(project), environ={"HOME": str(home)})
    assert "# Project and user skills" in text
    assert "- `p-agents` (.agents/skills/p-agents/SKILL.md): About p-agents." in text
    assert text.count("`p-agents`") == 1
    for name in ("p-claude", "p-codex", "h-agents", "h-claude", "h-codex"):
        assert f"`{name}`" in text
    assert f"({home}/.claude/skills/h-claude/SKILL.md)" in text
    assert all(f"`{path.name}`" in text for path in many)
    for absent in ("p-bare", ".system", "leak", "`empty`"):
        assert absent not in text
    codex_home = tmp_path / "codex-home"
    _skill(codex_home, "skills", "cx-custom")
    assert "`cx-custom`" in cortex_system_prompt("worker", str(project), environ={"HOME": str(home), "CODEX_HOME": str(codex_home)})
    assert "skills" not in cortex_system_prompt("worker", None, environ={}).split("# Environment")[1]


def test_long_descriptions_are_cut_at_the_agent_skills_limit(tmp_path):
    _skill(tmp_path, ".agents/skills", "long", "x" * 3000)
    line = next(l for l in cortex_system_prompt("worker", str(tmp_path), environ={}).splitlines() if "`long`" in l)
    assert line.endswith(": " + "x" * 1024)


def test_only_workers_are_told_to_work_without_narration():
    worker = cortex_system_prompt("worker", None, environ={})
    coordinator = cortex_system_prompt("coordinator", None, environ={})
    assert "Work without narration" in worker and "write one final message" in worker
    assert "including before the first tool and after a tool result" in worker
    assert "native shell output alone is not a verified check receipt" in worker
    assert "Work without narration" not in coordinator and "@@" not in worker + coordinator


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_conditional_skill_instruction_is_imperative_but_not_eager(role):
    prompt = cortex_system_prompt(role, None, environ={})
    assert "call the `read_skill` tool from the `cortex` MCP server with that exact skill id" in prompt
    assert "before assessing documentation impact for a plan or updating affected docs" in prompt
    assert "Do not read a conditional skill when its trigger has not arisen" in prompt
    assert "do not re-read them" in prompt
