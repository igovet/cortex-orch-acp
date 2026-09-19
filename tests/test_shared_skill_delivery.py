"""Shared skills actually reach the models.

The role text has always said things like "load/apply
`cortex:documentation-sync`" -- an instruction from when a Codex plugin host
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
from cortex_runtime.delegation.mcp_tools import _shared_read_skill  # noqa: E402
from cortex_runtime.portable.roles import (  # noqa: E402
    ALWAYS_ON_SKILLS, SHARED_ROLE_IDS, composed_instructions, load_role,
)


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
    for skill_id in ALWAYS_ON_SKILLS["coordinator"]:
        assert f"`{skill_id}`" in composed, f"{skill_id} was never named"
        # Named, and never pasted: the body stays one read away.
        assert load_role(skill_id).body_portable.strip()[:200] not in composed, skill_id
    assert 'read_skill' in composed


def test_a_worker_is_told_which_skills_exist_and_when_to_read_one():
    role = load_role("worker-backend-dev")
    composed = composed_instructions(role)
    for skill_id in ALWAYS_ON_SKILLS["worker"]:
        assert f"`{skill_id}`" in composed, skill_id
        assert load_role(skill_id).body_portable.strip()[:200] not in composed, skill_id
    # Each one comes with the case that calls for it, so a worker with no
    # such case spends nothing on it.
    assert "a tool refused you" in composed
    # Not the coordinator's own communication rules: a worker never talks
    # to the user.
    assert "coordinator-communication" not in ALWAYS_ON_SKILLS["worker"]


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
    # The `cortex:` prefix the role text uses is accepted.
    assert _shared_read_skill(None, None, {"skill": "cortex:knowledge-harvest"})["skill"] == "knowledge-harvest"


def test_only_shared_skills_are_readable():
    """A role's own instructions are handed to the engine that runs it,
    never fetched -- and never by another role."""
    for denied in ("orchestrator", "worker-backend-dev", "../../etc/passwd", "", None):
        with pytest.raises(StoreError):
            _shared_read_skill(None, None, {"skill": denied})


def test_every_always_on_skill_is_a_real_shared_skill():
    for kind, skills in ALWAYS_ON_SKILLS.items():
        for skill_id in skills:
            assert skill_id in SHARED_ROLE_IDS, (kind, skill_id)
            assert load_role(skill_id).body_portable.strip()
