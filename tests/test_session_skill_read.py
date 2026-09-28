"""Session-bound project/user skill reads never become arbitrary file reads."""
from __future__ import annotations

import sqlite3
import os
from types import SimpleNamespace

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts, mcp_tools
from cortex_runtime.delegation.session_skills import MAX_SKILL_BYTES, read_session_skill, record_session_skills
from cortex_runtime.delegation.live_adapters import _session_new_meta_for
from cortex_runtime.engine.unified_system_prompt import project_skills_section


class _Store:
    def __init__(self):
        self.db = sqlite3.connect(":memory:")
        self.db.row_factory = sqlite3.Row
        self.db.execute("CREATE TABLE context_generations (id TEXT, engine_session_ref TEXT, task_id TEXT)")
        self.db.execute("INSERT INTO context_generations VALUES ('cg_one', 'session-one', 'task-one')")
        self.db.execute("INSERT INTO context_generations VALUES ('cg_two', 'session-two', 'task-one')")

    def connection(self):
        return self.db


def _skill(root, rel, body="---\nname: demo\ndescription: Test.\n---\n\nBody.\n"):
    path = root / rel / "SKILL.md"
    path.parent.mkdir(parents=True)
    path.write_text(body, encoding="utf-8")
    return path


def _setup(tmp_path, role):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    home.mkdir()
    project_skill = _skill(project, ".agents/skills/demo")
    user_skill = _skill(home, ".codex/skills/personal", "---\nname: personal\n---\n\nPrivate body.\n")
    environ = {"HOME": str(home)}
    section = project_skills_section(str(project), environ)
    store = _Store()
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-one",
                          project_root=str(project), environ=environ,
                          sent_meta={"systemPrompt": "# Cortex session\n\n" + section})
    channel = SimpleNamespace(role=role, task_id="task-one", attempt_id="attempt-one",
                              context_generation_id="cg_one")
    return store, channel, project_skill, user_skill, project, home


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_only_exact_session_listed_skill_paths_are_readable(tmp_path, role):
    store, channel, project_skill, user_skill, project, home = _setup(tmp_path, role)
    listed_project = ".agents/skills/demo/SKILL.md"
    assert mcp_tools.dispatch(store, channel, "read_skill", {"skill": listed_project})["body"] == project_skill.read_text()
    assert mcp_tools.dispatch(store, channel, "read_skill", {"skill": str(user_skill)})["body"] == user_skill.read_text()
    sibling = project_skill.parent / "notes.md"
    sibling.write_text("not a skill")
    late = _skill(project, ".agents/skills/late")
    denied = [str(sibling), ".agents/skills/demo/notes.md", ".agents/skills/demo/../demo/SKILL.md",
              "../home/.codex/skills/personal/SKILL.md", str(late),
              str(tmp_path / "unlisted/SKILL.md"), str(home / ".codex/skills/other/SKILL.md")]
    for path in denied:
        with pytest.raises(StoreError, match="unknown_skill"):
            mcp_tools.dispatch(store, channel, "read_skill", {"skill": path})
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, SimpleNamespace(**{**vars(channel), "context_generation_id": "cg_two"}), listed_project)


def test_symlink_escape_and_retargeting_are_refused(tmp_path):
    store, channel, project_skill, _user_skill, project, _home = _setup(tmp_path, "worker")
    outside = _skill(tmp_path, "outside/secret", "secret")
    escape = project / ".agents/skills/escape"
    escape.symlink_to(outside.parent, target_is_directory=True)
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, ".agents/skills/escape/SKILL.md")
    project_skill.unlink()
    project_skill.symlink_to(outside)
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")


def test_catalog_skips_symlink_to_other_file_inside_project(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    other = _skill(project, "private/secret", "do not list")
    link = project / ".agents/skills/leak"
    link.parent.mkdir(parents=True)
    link.symlink_to(other.parent, target_is_directory=True)
    assert "leak" not in project_skills_section(str(project), environ={})


def test_catalog_skips_hardlinked_skill_body(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    outside = _skill(tmp_path, "outside/secret", "private")
    link = project / ".agents/skills/leak/SKILL.md"
    link.parent.mkdir(parents=True)
    os.link(outside, link)
    assert "leak" not in project_skills_section(str(project), environ={})


def test_skill_size_cap_and_unverified_prompt_fail_closed(tmp_path):
    store, channel, project_skill, _user_skill, project, home = _setup(tmp_path, "coordinator")
    project_skill.write_bytes(b"x" * (MAX_SKILL_BYTES + 1))
    with pytest.raises(StoreError, match="skill_unavailable"):
        read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-two",
                          project_root=str(project), environ={"HOME": str(home)}, sent_meta={"systemPrompt": "no skills"})
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, SimpleNamespace(**{**vars(channel), "context_generation_id": "cg_two"}),
                           ".agents/skills/demo/SKILL.md")
    assert "`demo`" not in project_skills_section(str(project), {"HOME": str(home)})


def test_path_input_is_advertised_and_schema_bounded():
    for role in ("coordinator", "worker"):
        entry = next(tool for tool in mcp_tools.tool_definitions(role) if tool["name"] == "read_skill")
        assert "exact path shown" in entry["description"]
        schema = entry["inputSchema"]["properties"]["skill"]
        assert schema["maxLength"] >= 4096
        contracts.validate(role, "read_skill", {"skill": ".agents/skills/demo/SKILL.md"})
        with pytest.raises(StoreError, match="invalid_arguments"):
            contracts.validate(role, "read_skill", {"skill": "x" * (schema["maxLength"] + 1)})


@pytest.mark.parametrize("engine,version", [("claude-agent-acp", "0.78.0"), ("codex-acp", "1.13.0")])
def test_actual_engine_session_metadata_binds_the_skill_list(tmp_path, engine, version):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    home.mkdir()
    skill = _skill(project, ".agents/skills/demo")
    environ = {"HOME": str(home)}
    meta = _session_new_meta_for(engine, version, "Role text.", role="worker",
                                 project_root=str(project), environ=environ)
    store = _Store()
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-one",
                          project_root=str(project), environ=environ, sent_meta=meta)
    channel = SimpleNamespace(role="worker", task_id="task-one", attempt_id="attempt-one",
                              context_generation_id="cg_one")
    assert read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")["body"] == skill.read_text()
