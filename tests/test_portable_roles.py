"""T015: portable.roles -- role resolution and RoleInstructionSet loading."""
from __future__ import annotations

import pytest

from cortex_runtime.portable import roles
from cortex_runtime.portable.errors import PortableError, PortableErrorCategory


def test_reviewer_defaults_to_inline_report_without_weakening_shared_protocol():
    reviewer = roles.composed_instructions(roles.load_role("worker-code-reviewer"))
    normalized = " ".join(reviewer.split())
    assert "Publish the complete review report inline with `result_submit` by default" in normalized
    assert "conclusion, observed evidence, checks, contradictions, limits and next action" in normalized
    assert "Runtime facts are context, not a substitute for your independent analysis" in normalized
    assert "ordinary `general` report template" in normalized
    writer = roles.composed_instructions(roles.load_role("worker-backend-dev"))
    assert "owned draft" in writer
    assert "ordinary `implementation` report template" in writer


def test_known_role_ids_counts():
    ids = roles.known_role_ids()
    assert len(ids) == 42
    assert "orchestrator" in ids
    assert "worker-backend-dev" in ids
    assert "cortex-control" in ids


def test_resolve_role_exact_id():
    assert roles.resolve_role("worker-backend-dev") == "worker-backend-dev"
    assert roles.resolve_role("orchestrator") == "orchestrator"


def test_resolve_role_alias():
    assert roles.resolve_role("backend-dev") == "worker-backend-dev"
    assert roles.resolve_role("senior-consultant") == "worker-senior-consultant"


def test_resolve_role_unknown_lists_valid_ids():
    with pytest.raises(PortableError) as excinfo:
        roles.resolve_role("nonexistent-role")
    error = excinfo.value
    assert error.code == "unknown_role"
    assert error.retry_eligible is False
    assert "worker-backend-dev" in error.remediation
    assert "orchestrator" in error.remediation


def test_resolve_role_normalizes_separators_and_case():
    """FINDING 11: backend_dev / backend-dev / Backend_Dev / worker-backend-dev
    all resolve to the same role -- the tool schema advertises the hyphenated
    spelling, but an underscore is the spelling any model reaches for."""
    for hint in ("backend_dev", "backend-dev", "Backend_Dev", "worker-backend-dev", "WORKER_BACKEND_DEV"):
        assert roles.resolve_role(hint) == "worker-backend-dev"


def test_resolve_role_normalization_does_not_invent_matches():
    """Normalization is separator/case folding only, never fuzzy matching --
    a name that is genuinely not a known id or alias still raises."""
    with pytest.raises(PortableError):
        roles.resolve_role("backend_developer")
    with pytest.raises(PortableError):
        roles.resolve_role("totally_unknown_role")


def test_resolve_role_no_fuzzy_matching():
    """A near-miss (prefix/substring) never silently resolves."""
    with pytest.raises(PortableError):
        roles.resolve_role("backend")
    with pytest.raises(PortableError):
        roles.resolve_role("worker-backend")


@pytest.mark.parametrize("bad", [None, "", 42, [], {}])
def test_resolve_role_rejects_non_string_hints(bad):
    with pytest.raises(PortableError):
        roles.resolve_role(bad)


def test_load_role_worker_has_critical_rules_and_no_native_leak():
    role = roles.load_role("worker-backend-dev")
    assert role.kind == "worker"
    assert role.critical_rules
    assert "codex_app" not in role.body_portable
    assert "apply_patch" not in role.body_portable
    assert role.title
    assert len(role.sha256) == 64


def test_load_role_coordinator_describes_runtime_owned_policy():
    role=roles.load_role('orchestrator')
    assert role.kind=='coordinator'
    assert 'runtime owns the task lifecycle' in role.body_portable


def test_load_role_shared_has_no_critical_rules_by_default():
    role = roles.load_role("content-safety")
    assert role.kind == "shared"
    # No non-negotiable block was added to shared skills; empty is expected,
    # not an error.
    assert role.critical_rules == ()


@pytest.mark.parametrize(("role_id", "skill_id", "reader"), [
    ("worker-backend-dev", "content-safety", "load_role"),
    ("worker-backend-dev", "code-and-evidence", "read_reference"),
    ("orchestrator", "coordinator-communication", "load_role"),
    ("worker-senior-consultant", "report-publication", "read_reference"),
])
@pytest.mark.parametrize("builder", ["composition", "bundle"])
def test_required_skill_load_failure_blocks_composition(monkeypatch, role_id, skill_id, reader, builder):
    role = roles.load_role(role_id)
    original = getattr(roles, reader)
    cause = PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="test_skill_unreadable",
        remediation="Restore the skill source.",
        retry_eligible=False,
        details="test source failure",
    )

    def fail_selected(selected, *args, **kwargs):
        if selected == skill_id:
            raise cause
        return original(selected, *args, **kwargs)

    monkeypatch.setattr(roles, reader, fail_selected)
    with pytest.raises(PortableError) as excinfo:
        if builder == "bundle":
            from cortex_runtime.portable.instruction_bundle import build_instruction_bundle

            build_instruction_bundle(
                task_id="task", assignment_ref="assignment", attempt_id="attempt",
                role_set=role, engine="codex-acp", engine_version="2.1.1",
            )
        else:
            roles.composed_instructions(role)
    error = excinfo.value
    assert error.code == "required_skill_unavailable"
    assert error.retry_eligible is False
    assert role_id in error.remediation and skill_id in error.remediation
    assert cause.code in error.remediation and cause.remediation in error.remediation
    assert error.details == cause.details
    assert error.__cause__ is cause


def test_empty_required_skill_blocks_composition(monkeypatch):
    role = roles.load_role("worker-backend-dev")
    original = roles._inlined_skill_text
    monkeypatch.setattr(
        roles, "_inlined_skill_text",
        lambda skill_id, kind: " \n" if skill_id == "content-safety" else original(skill_id, kind),
    )
    with pytest.raises(PortableError) as excinfo:
        roles.composed_instructions(role)
    assert excinfo.value.code == "required_skill_unavailable"
    assert "required_skill_empty" in excinfo.value.remediation
    assert "content-safety" in excinfo.value.remediation


def test_missing_situational_skill_is_not_eagerly_loaded(monkeypatch):
    role = roles.load_role("worker-backend-dev")
    original = roles.load_role
    called = []

    def fail_situational(skill_id):
        called.append(skill_id)
        if skill_id == "documentation-sync":
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="test_skill_unreadable",
                remediation="Restore the situational skill.",
                retry_eligible=False,
            )
        return original(skill_id)

    monkeypatch.setattr(roles, "load_role", fail_situational)
    text = roles.composed_instructions(role)
    assert "documentation-sync" not in called
    assert "`documentation-sync` --" in text
    assert "### Skill `content-safety`" in text


def test_load_all_roles_is_deterministic_and_matches_known_ids():
    first = roles.load_all_roles()
    second = roles.load_all_roles()
    assert set(first) == set(second) == set(roles.known_role_ids())
    for role_id in first:
        assert first[role_id].sha256 == second[role_id].sha256


def test_load_role_source_unavailable_for_missing_file(tmp_path, monkeypatch):
    monkeypatch.setattr(roles, "SKILLS_ROOT", tmp_path)
    with pytest.raises(PortableError) as excinfo:
        roles.load_role("orchestrator")
    assert excinfo.value.code == "role_source_unavailable"


def test_situational_catalogue_excludes_inlined_and_consultant_mutation_skills():
    worker = dict(roles.situational_skill_triggers("worker", "worker-backend-dev"))
    consultant = dict(roles.situational_skill_triggers("worker", "worker-senior-consultant"))
    assert "documentation-sync" in worker and "knowledge-harvest" in worker
    assert "report-publication" not in worker and "code-and-evidence" not in worker
    assert set(consultant) == {"index", "context-compaction"}
    assert "skill-management" in dict(roles.situational_skill_triggers("coordinator"))


def test_known_location_handoff_guidance_is_delivered_to_workers_and_coordinator():
    worker = " ".join(roles.load_role("worker-backend-dev").body_portable.split())
    coordinator = " ".join(roles.load_role("orchestrator").body_portable.split())
    assert "When its brief includes `Known locations`, begin with those locations" in worker
    assert "read the current source directly before editing; the hints can be stale" in worker
    assert "Read a relevant documentation index and do broad discovery only when" in worker
    assert "For each task objective, include a short `Known locations` block only when you" in coordinator
    assert "do not investigate further merely to produce hints" in coordinator


def test_consultant_receives_reporting_but_no_source_discovery_skill():
    delivered = roles.composed_instructions(roles.load_role("worker-senior-consultant"))
    assert "### Skill `report-publication`" in delivered
    assert "### Skill `content-safety`" in delivered
    for skill_id in roles.CONSULTANT_EXCLUDED_NAMED_SKILLS:
        assert f"### Skill `{skill_id}`" not in delivered
        assert f"`{skill_id}` --" not in delivered
    worker = roles.composed_instructions(roles.load_role("worker-backend-dev"))
    assert "### Skill `code-and-evidence`" in worker


DESIGN_ROLE_MATRIX = {
    "design-reference-analysis": {"worker-ux-designer"},
    "design-ui-ux-pro-max": {"worker-ux-designer", "worker-mobile-dev"},
    "design-frontend-design": {"worker-frontend-dev", "worker-mobile-dev"},
    "design-impeccable": {"worker-ux-designer", "worker-accessibility-auditor", "worker-frontend-dev"},
    "design-visual-qa": {"worker-ux-designer", "worker-accessibility-auditor",
                         "worker-frontend-dev", "worker-mobile-dev", "worker-qa-engineer"},
}


@pytest.mark.parametrize("role_id", tuple(roles.WORKER_ALIASES.values()) + (None,))
def test_design_skills_are_role_scoped_in_delivery_and_reader(monkeypatch, role_id):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.contracts import StoreError

    monkeypatch.setattr(mcp_tools, "_own_role_id", lambda store, channel: role_id)
    allowed = roles.shared_skill_ids("worker", role_id)
    triggers = dict(roles.situational_skill_triggers("worker", role_id))
    schema = mcp_tools._enumerate_skills(
        {"properties": {"skill": {"type": "string"}}}, "worker", role_id)
    advertised = schema["properties"]["skill"]["description"]
    for reference, (parent_skill, relative_path) in roles.DESIGN_REFERENCE_PATHS.items():
        expected = role_id in DESIGN_ROLE_MATRIX[parent_skill]
        assert (reference in roles.reference_ids("worker", role_id)) == expected
        assert (reference in advertised) == expected
        if expected:
            body = mcp_tools._shared_read_skill(None, None, {"skill": reference})["body"]
            assert body == (roles.SKILLS_ROOT / parent_skill / relative_path).read_text()
        else:
            with pytest.raises(StoreError, match="unknown_skill"):
                mcp_tools._shared_read_skill(None, None, {"skill": reference})
            with pytest.raises(PortableError, match="unknown_reference"):
                roles.read_reference(reference, role_kind="worker", role_id=role_id)
    for skill_id, owners in DESIGN_ROLE_MATRIX.items():
        assert (skill_id in allowed) == (role_id in owners)
        assert (skill_id in triggers) == (role_id in owners)
        assert (skill_id in advertised) == (role_id in owners)
        if role_id in owners:
            body = mcp_tools._shared_read_skill(None, None, {"skill": skill_id})["body"]
            assert body == roles.load_role(skill_id).body_portable
        else:
            with pytest.raises(StoreError, match="unknown_skill"):
                mcp_tools._shared_read_skill(None, None, {"skill": skill_id})


def test_coordinator_design_intake_is_situational_and_not_worker_readable():
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.contracts import StoreError
    from types import SimpleNamespace

    coordinator = roles.composed_instructions(roles.load_role("orchestrator"))
    triggers = dict(roles.situational_skill_triggers("coordinator"))
    for skill_id in ("requirements-intake", "design-workflow"):
        assert skill_id in roles.COORDINATOR_ONLY_SHARED_ROLE_IDS
        assert skill_id in triggers
        assert f"`{skill_id}` --" in coordinator
        assert roles.load_role(skill_id).body_portable[:200] not in coordinator
        assert mcp_tools._shared_read_skill(
            None, SimpleNamespace(role="coordinator"), {"skill": skill_id})["body"]
        with pytest.raises(StoreError, match="unknown_skill"):
            mcp_tools._shared_read_skill(None, None, {"skill": skill_id})
    assert not (set(DESIGN_ROLE_MATRIX) & roles.shared_skill_ids("coordinator"))
    assert not (set(DESIGN_ROLE_MATRIX) & set(triggers))
    for reference in roles.DESIGN_REFERENCE_PATHS:
        with pytest.raises(StoreError, match="unknown_skill"):
            mcp_tools._shared_read_skill(
                None, SimpleNamespace(role="coordinator"), {"skill": reference})


@pytest.mark.parametrize("reference_id", (
    "question-banks", "agreed-spec", "design-intake",
    "PRODUCT", "DESIGN", "SCREENS", "visual-review",
))
def test_coordinator_design_resources_resolve_through_existing_reader(reference_id):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.contracts import StoreError
    from types import SimpleNamespace

    result = mcp_tools._shared_read_skill(
        None, SimpleNamespace(role="coordinator"), {"skill": f"references/{reference_id}.md"})
    assert result["body"] == roles.read_reference(reference_id, role_kind="coordinator")
    assert result["body"].startswith("# ")
    with pytest.raises(StoreError, match="unknown_skill"):
        mcp_tools._shared_read_skill(None, None, {"skill": reference_id})
