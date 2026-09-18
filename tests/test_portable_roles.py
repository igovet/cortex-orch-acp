"""T015: portable.roles -- role resolution and RoleInstructionSet loading."""
from __future__ import annotations

import pytest

from cortex_runtime.portable import roles
from cortex_runtime.portable.errors import PortableError


def test_known_role_ids_counts():
    ids = roles.known_role_ids()
    assert len(ids) == 31
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


def test_load_role_coordinator_has_critical_rules():
    role = roles.load_role("orchestrator")
    assert role.kind == "coordinator"
    assert role.critical_rules


def test_load_role_shared_has_no_critical_rules_by_default():
    role = roles.load_role("content-safety")
    assert role.kind == "shared"
    # No non-negotiable block was added to shared skills; empty is expected,
    # not an error.
    assert role.critical_rules == ()


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
