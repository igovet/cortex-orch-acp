"""Shared skills actually reach the models.

The role text has always said things like "load/apply
`documentation-sync`" -- an instruction from when a Codex plugin host
could load a skill by name. In an ACP session nothing can: `load_role` reads
exactly one `SKILL.md`, and no tool read a skill. So every shared skill --
tool discipline, content safety, how to talk to the user -- was unreachable,
and the instruction to load one could only be disobeyed.

Two halves now: the skills that always apply are delivered with the role, and
the situational ones are readable by name.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "src"))

from cortex_runtime.contracts import StoreError  # noqa: E402
from cortex_runtime.delegation.mcp_tools import _enumerate_skills, _shared_read_skill  # noqa: E402
from cortex_runtime.portable.roles import (  # noqa: E402
    COORDINATOR_ONLY_SHARED_ROLE_IDS, NAMED_SKILLS, SHARED_ROLE_IDS, composed_instructions,
    load_role, read_reference,
)
from cortex_runtime.portable.errors import PortableError  # noqa: E402


def _read(skill_id, kind):
    from types import SimpleNamespace
    channel = SimpleNamespace(role="coordinator") if kind == "coordinator" else None
    return _shared_read_skill(None, channel, {"skill": skill_id})["body"]


def test_a_coordinator_is_told_which_skills_exist_and_when_to_read_one():
    """Named with their occasion, not pasted.

    Handing every relevant skill over with the role made the delivered text
    half again as long before the work started, and most of it did not
    apply to the assignment in hand. "Load these skills" is a ritual; a
    reason to read one is not.
    """
    role = load_role("orchestrator")
    composed = composed_instructions(role)
    assert composed.startswith(role.body_portable.strip()[:80]), "the role's own body comes first"
    for skill_id in NAMED_SKILLS["coordinator"]:
        assert f"`{skill_id}`" in composed, f"{skill_id} was never named"
        # Named, and never pasted: the body stays one read away.
        assert _read(skill_id, "coordinator").strip()[:200] not in composed, skill_id
    assert 'read_skill' in composed
    assert "`read_skill` tool on the `cortex` MCP server" in composed
    assert "engine-native skill loader" in composed
    assert "live tool descriptions and schemas" in role.body_portable
    # A commentary-then-final-answer host (e.g. Codex) otherwise treats a
    # detailed pre-call message and the final answer as two separate,
    # never-collapsed turns and repeats the content in both (investigation
    # r_af50d7468665, session c2016029): tell the model explicitly not to.
    assert "never follow it with a shorter recap or summary in the same turn" in " ".join(role.body_portable.split())


def test_a_worker_is_told_which_skills_exist_and_when_to_read_one():
    role = load_role("worker-backend-dev")
    composed = composed_instructions(role)
    for skill_id in NAMED_SKILLS["worker"]:
        assert f"`{skill_id}`" in composed, skill_id
        assert _read(skill_id, "worker").strip()[:200] not in composed, skill_id
    # Each one comes with the case that calls for it, so a worker with no
    # such case spends nothing on it.
    assert "a tool refused you" in composed
    # Not the coordinator's own communication rules: a worker never talks
    # to the user.
    assert "coordinator-communication" not in NAMED_SKILLS["worker"]
    assert "`read_skill` tool on the `cortex` MCP server" in composed


def test_the_delivered_text_stays_close_to_the_role_itself():
    """The whole point of naming rather than pasting."""
    role = load_role("worker-backend-dev")
    composed = composed_instructions(role)
    assert len(composed) < len(role.body_portable) * 1.2, len(composed)


def test_the_situational_skills_are_not_carried_on_every_turn():
    """`knowledge-harvest` alone is 18KB. Delivering every shared skill
    unconditionally would charge every worker turn for all of it."""
    composed = composed_instructions(load_role("worker-backend-dev"))
    for skill_id in ("knowledge-harvest", "documentation-sync"):
        assert load_role(skill_id).body_portable.strip()[:200] not in composed, skill_id


def test_a_situational_skill_can_be_read_by_name():
    result = _shared_read_skill(None, None, {"skill": "documentation-sync"})
    assert result["skill"] == "documentation-sync"
    assert result["body"] == load_role("documentation-sync").body_portable
    # Codex-plugin-era namespace aliases are not part of the ACP contract.
    with pytest.raises(StoreError) as error:
        _shared_read_skill(None, None, {"skill": "cortex:knowledge-harvest"})
    assert "unknown_skill" in str(error.value)


def test_only_shared_skills_are_readable():
    """A role's own instructions are handed to the engine that runs it,
    never fetched -- and never by another role."""
    for denied in ("orchestrator", "worker-backend-dev", "../../etc/passwd", "", None):
        with pytest.raises(StoreError):
            _shared_read_skill(None, None, {"skill": denied})


def test_every_named_skill_is_readable_by_its_role():
    """A role is told these ids; each must resolve through the reader for it."""
    for kind, skills in NAMED_SKILLS.items():
        for skill_id in skills:
            assert _read(skill_id, kind).strip(), (kind, skill_id)
    # The coordinator's retention command is not a worker's to run.
    assert "cortex-control" not in NAMED_SKILLS["worker"]


def test_coordinator_only_shared_skills_are_hidden_from_a_worker_and_the_consultant():
    """`cortex-control` and `coordinator-communication` are coordinator-only:
    a worker or the consultant (also a "worker"-kind channel) may never see
    them advertised, nor read them, even though the coordinator still can."""
    from types import SimpleNamespace

    assert COORDINATOR_ONLY_SHARED_ROLE_IDS <= SHARED_ROLE_IDS
    for skill_id in COORDINATOR_ONLY_SHARED_ROLE_IDS:
        assert skill_id not in NAMED_SKILLS["worker"]
        with pytest.raises(StoreError) as error:
            _shared_read_skill(None, None, {"skill": skill_id})
        assert "unknown_skill" in str(error.value)
        # The coordinator channel still reads it fine.
        coordinator_channel = SimpleNamespace(role="coordinator")
        result = _shared_read_skill(None, coordinator_channel, {"skill": skill_id})
        assert result["skill"] == skill_id

    schema = {"properties": {"skill": {"type": "string"}}}
    worker_schema = _enumerate_skills(schema, "worker", "worker-backend-dev")
    consultant_schema = _enumerate_skills(schema, "worker", "worker-senior-consultant")
    coordinator_schema = _enumerate_skills(schema, "coordinator", None)
    for skill_id in COORDINATOR_ONLY_SHARED_ROLE_IDS:
        assert skill_id not in worker_schema["properties"]["skill"]["enum"]
        assert skill_id not in consultant_schema["properties"]["skill"]["enum"]
        assert skill_id in coordinator_schema["properties"]["skill"]["enum"]


def test_reference_inventory_is_bounded_and_role_scoped():
    assert "# Assignment planning" in read_reference("worker-routing", role_kind="coordinator")
    assert "# Report publication" in read_reference("report-publication", role_kind="worker")
    assert "# " in read_reference("report-example", role_kind="worker")
    for reference_id, role_kind in (
        ("worker-routing", "worker"),
        ("report-publication", "coordinator"),
        ("../orchestrator/SKILL", "coordinator"),
    ):
        with pytest.raises(PortableError):
            read_reference(reference_id, role_kind=role_kind)


def test_every_read_skill_target_in_shipped_instructions_is_advertised():
    """Every explicit read_skill target uses an id the reader advertises."""
    import re

    role_enums = {
        "coordinator": set(NAMED_SKILLS["coordinator"]),
        "worker": set(NAMED_SKILLS["worker"]),
    }
    paths_by_role = {
        "coordinator": [REPO_ROOT / "skills/orchestrator/SKILL.md"],
        "worker": [
            *REPO_ROOT.glob("skills/**/SKILL.md"),
            *REPO_ROOT.glob("skills/**/references/*.md"),
            *REPO_ROOT.glob("agent-sources/**/*.md"),
        ],
    }
    target_pattern = re.compile(
        r"(?<!not )(?:load|read) (?:skill )?`([a-z][a-z0-9-]*)`[^\n]{0,80}`read_skill`"
    )

    for role, paths in paths_by_role.items():
        for path in paths:
            for skill_id in target_pattern.findall(path.read_text()):
                target_role = (
                    "coordinator" if skill_id == "coordinator-communication" else role
                )
                assert skill_id in role_enums[target_role], (target_role, path, skill_id)
