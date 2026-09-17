"""T015: portable.instruction_delivery -- delivery qualification and refusal."""
from __future__ import annotations

import pytest

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable import roles
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.instruction_delivery import (
    DeliveryQualification,
    UnsupportedDelivery,
    qualify_delivery,
)


def test_delivery_for_codex_acp_pinned_version():
    mechanism = delivery_for("codex-acp", "1.12.0")
    assert mechanism.engine == "codex-acp"
    assert mechanism.version == "1.12.0"


def test_delivery_for_claude_agent_acp_any_version_is_wildcard():
    mechanism = delivery_for("claude-agent-acp", "0.78.0")
    assert mechanism.engine == "claude-agent-acp"


def test_delivery_for_unknown_engine_is_unsupported():
    mechanism = delivery_for("some-future-engine", "9.9.9")
    assert isinstance(mechanism, UnsupportedDelivery)


def test_delivery_for_unknown_codex_acp_version_is_unsupported_not_guessed():
    mechanism = delivery_for("codex-acp", "2.0.0")
    assert isinstance(mechanism, UnsupportedDelivery)


def test_codex_acp_plan_is_unqualified():
    role = roles.load_role("worker-backend-dev")
    plan = delivery_for("codex-acp", "1.12.0").plan(role, [])
    assert plan.qualification == DeliveryQualification.UNQUALIFIED
    assert plan.blocks
    assert plan.blocks[0]["type"] == "text"
    assert role.body_portable in plan.blocks[0]["text"]


def test_claude_agent_acp_plan_is_unsupported():
    role = roles.load_role("worker-backend-dev")
    plan = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
    assert plan.qualification == DeliveryQualification.UNSUPPORTED
    assert plan.blocks == ()


def test_qualify_delivery_refuses_unqualified_for_critical_role_by_default():
    role = roles.load_role("worker-backend-dev")
    assert role.critical_rules
    with pytest.raises(PortableError) as excinfo:
        qualify_delivery(role, delivery_for("codex-acp", "1.12.0"), [])
    error = excinfo.value
    assert error.code == "instruction_delivery_unqualified"
    assert error.retry_eligible is False
    assert "codex-acp" in error.remediation
    assert "1.12.0" in error.remediation


def test_qualify_delivery_refuses_unsupported_for_critical_role():
    role = roles.load_role("worker-backend-dev")
    with pytest.raises(PortableError) as excinfo:
        qualify_delivery(role, delivery_for("claude-agent-acp", "0.78.0"), [])
    assert excinfo.value.code == "instruction_delivery_unqualified"


def test_qualify_delivery_allows_unqualified_when_escape_hatch_set():
    role = roles.load_role("worker-backend-dev")
    plan = qualify_delivery(role, delivery_for("codex-acp", "1.12.0"), [], allow_unqualified_delivery=True)
    assert plan.qualification == DeliveryQualification.UNQUALIFIED


def test_qualify_delivery_escape_hatch_does_not_admit_unsupported():
    """The escape hatch only widens UNQUALIFIED, never UNSUPPORTED."""
    role = roles.load_role("worker-backend-dev")
    with pytest.raises(PortableError):
        qualify_delivery(role, delivery_for("claude-agent-acp", "0.78.0"), [], allow_unqualified_delivery=True)


def test_qualify_delivery_allows_any_qualification_for_role_without_critical_rules():
    role = roles.load_role("content-safety")
    assert role.critical_rules == ()
    plan = qualify_delivery(role, delivery_for("claude-agent-acp", "0.78.0"), [])
    assert plan.qualification == DeliveryQualification.UNSUPPORTED


def test_delivery_for_rejects_empty_engine_or_version():
    with pytest.raises(PortableError):
        delivery_for("", "1.0.0")
    with pytest.raises(PortableError):
        delivery_for("codex-acp", "")
