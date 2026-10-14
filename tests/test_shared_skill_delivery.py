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
    COORDINATOR_ONLY_SHARED_ROLE_IDS, INLINED_SKILLS, NAMED_SKILLS, SHARED_ROLE_IDS, composed_instructions,
    load_role, read_reference, situational_skill_triggers,
)
from cortex_runtime.portable.errors import PortableError  # noqa: E402


def _read(skill_id, kind):
    from types import SimpleNamespace
    channel = SimpleNamespace(role="coordinator") if kind == "coordinator" else None
    return _shared_read_skill(None, channel, {"skill": skill_id})["body"]


def _check_split(role_id, kind):
    """The skills of almost every session arrive in full, exactly as
    `read_skill` returns them; the situational ones are named with their
    occasion and stay one read away."""
    role = load_role(role_id)
    composed = composed_instructions(role)
    assert composed.startswith(role.body_portable.strip()[:80]), "the role's own body comes first"
    for skill_id in INLINED_SKILLS[kind]:
        assert f"### Skill `{skill_id}`\n\n" + _read(skill_id, kind).strip() in composed, skill_id
    for skill_id, _ in situational_skill_triggers(kind, role_id):
        if skill_id in INLINED_SKILLS[kind]:
            continue
        assert f"`{skill_id}` --" in composed, f"{skill_id} was never named"
        assert _read(skill_id, kind).strip()[:200] not in composed, skill_id
    assert "do not read them again with `read_skill`" in composed
    assert "`read_skill` tool on the `cortex` MCP server" in composed
    assert "engine-native skill loader" in composed
    return role, composed


def test_a_coordinator_gets_its_everyday_skills_and_is_told_when_to_read_the_rest():
    role, composed = _check_split("orchestrator", "coordinator")
    assert "live tool descriptions and schemas" in role.body_portable
    # A commentary-then-final-answer host otherwise treats a detailed
    # pre-call message and the final answer as two separate, never-collapsed
    # turns and repeats the content in both (observed live): tell the model
    # explicitly not to. The rule was later strengthened from banning only a
    # *shorter* recap to banning any restatement, of any length
    # (docs/project/decisions.md "the runtime withholds coordinator text
    # after its own question").
    assert "never follow it with a recap, summary or rewritten version of any length in the same turn" in " ".join(role.body_portable.split())


def test_a_worker_gets_its_everyday_skills_and_is_told_when_to_read_the_rest():
    _, composed = _check_split("worker-backend-dev", "worker")
    # Not the coordinator's own communication rules: a worker never talks
    # to the user.
    assert "coordinator-communication" not in NAMED_SKILLS["worker"]
    assert "coordinator-communication" not in INLINED_SKILLS["worker"]
    assert "before starting or using a browser, device, port or server session" in composed
    assert "call Cortex MCP `read_skill` with that id before the action" in composed


@pytest.mark.parametrize("role_id,kind", [("orchestrator", "coordinator"),
                                            ("worker-backend-dev", "worker"),
                                            ("worker-senior-consultant", "worker")])
def test_prompt_and_tool_share_one_situational_catalogue(role_id, kind):
    role = load_role(role_id)
    composed = composed_instructions(role)
    schema = _enumerate_skills({"properties": {"skill": {"type": "string", "description": "Select a skill."}}},
                               kind, role_id)
    advertised = schema["properties"]["skill"]
    for skill_id, trigger in situational_skill_triggers(kind, role_id):
        assert f"`{skill_id}` -- {trigger}" in composed
        assert f"{skill_id}: {trigger}" in advertised["description"]
        assert skill_id in advertised["description"]
    if role_id == "worker-senior-consultant":
        assert "`documentation-sync` --" not in composed


def test_the_delivered_text_is_the_role_plus_its_everyday_skills():
    """Only the everyday skills are added; nothing else grows the text."""
    role = load_role("worker-backend-dev")
    composed = composed_instructions(role)
    inlined = sum(len(_read(skill_id, "worker")) for skill_id in INLINED_SKILLS["worker"])
    assert len(composed) < len(role.body_portable) + inlined + 2500, len(composed)


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
        from cortex_runtime.portable.roles import shared_skill_ids, DESIGN_SKILL_ROLES
        for skill_id in skills:
            if skill_id in DESIGN_SKILL_ROLES:
                # Stage-specific skills are not readable without an assigned role.
                assert skill_id not in shared_skill_ids(kind)
                continue
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
        assert skill_id not in worker_schema["properties"]["skill"]["description"]
        assert skill_id not in consultant_schema["properties"]["skill"]["description"]
        assert skill_id in coordinator_schema["properties"]["skill"]["description"]


def test_reference_inventory_is_bounded_and_role_scoped():
    assert "# Assignment planning" in read_reference("worker-routing", role_kind="coordinator")
    from types import SimpleNamespace
    assert _shared_read_skill(None, SimpleNamespace(role="coordinator"),
                              {"skill": "references/worker-routing.md"})["skill"] == "worker-routing"
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
                assert skill_id in role_enums[role], (role, path, skill_id)


def test_worker_protocol_does_not_reload_inlined_skills_and_respects_role_access():
    for role_id in ("worker-backend-dev", "worker-explorer",
                    "worker-database-architect", "worker-performance-engineer"):
        body = " ".join(load_role(role_id).body_portable.split())
        assert "through the advertised skill reader" not in body
        assert "If your role writes, read relevant project instructions" in body
        composed = " ".join(composed_instructions(load_role(role_id)).split())
        if role_id == "worker-backend-dev":
            assert "Verify every behavior your assignment changes" in body
            assert "Run the checks that exercise" in body
        else:
            assert "Change nothing; run only authorized non-mutating checks" in body
            assert "only authorized non-mutating checks" in body
        assert "reserve them through `access_request`" in composed
        assert "only when the plan itself must change" in composed
        assert "advertised `ask_question` route to the user" in body
        assert "only when the assignment needs that check and project rules do not already decide it" in body
    for role_id in ("worker-database-architect", "worker-performance-engineer"):
        assert "role is read-only" in load_role(role_id).body_portable


def test_every_coordinator_reference_link_resolves_through_the_advertised_reader():
    import re
    from types import SimpleNamespace

    body = load_role("orchestrator").body_portable
    references = re.findall(r"\]\(references/([a-z-]+)\.md\)", body)
    assert "verification-coverage" in references
    channel = SimpleNamespace(role="coordinator")
    schema = _enumerate_skills({"properties": {"skill": {"type": "string"}}},
                               "coordinator", None)
    advertised = schema["properties"]["skill"]["anyOf"][0]["enum"]
    for reference_id in references:
        assert reference_id in advertised
        assert _shared_read_skill(None, channel, {"skill": reference_id})["body"].strip()


def test_portable_verification_guidance_uses_target_policy_not_cortex_development():
    for reference_id in ("verification-coverage", "live-qualification"):
        text = read_reference(reference_id, role_kind="coordinator")
        assert "TARGET project" in text
        assert "explicit owner request" in text
        assert "paid live check not requested by the owner" in text
        for residue in ("Paseo", "~/.paseo", "integrations/paseo",
                        "--i-authorize-paid-call", "--approval-ref", "deep coordinator"):
            assert residue not in text
    local = (REPO_ROOT / "docs/project/verification.md").read_text()
    for rule in ("~/.paseo-dev", "~/.config/Paseo-dev", "Heavy models",
                 "--i-authorize-paid-call", "--approval-ref", "owner-selected route",
                 "npm --prefix integrations/paseo", "paid live check not requested by the owner"):
        assert rule in local


def test_recovery_routes_and_shared_access_wording_do_not_drift():
    recovery = load_role("context-compaction").body_portable
    coordinator, worker = recovery.split("## Coordinator", 1)[1].split("## Worker", 1)
    assert coordinator.index("task_get_context") < coordinator.index("read_report") < coordinator.index("list_sources")
    assert "read_capsule" in worker
    assert "may read bounded project files" in coordinator
    assert "runtime's per-turn language directive" in coordinator
    for residue in ("advisory governance", "worker handles", "session_id", "text(result)",
                    "ask the coordinator", "code-mode"):
        assert residue not in recovery
    discipline = load_role("tool-discipline").body_portable
    worker_discipline = load_role("tool-discipline-worker").body_portable
    assert "parallel writers" in worker_discipline
    assert "advertised permission route" in worker_discipline
    assert "command_run" not in discipline
    assert "project is read-only except your assignment scope" not in discipline
    assert "Use this skill only during selected Cortex work" not in discipline
    assert "Code-mode" not in discipline
