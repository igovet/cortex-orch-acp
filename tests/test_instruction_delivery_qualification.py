"""T015/T026/T035: portable.instruction_delivery -- delivery qualification and refusal.

Updated by T026 (explicit lead re-direction, 2026-09-17): the T015-era
belief that claude-agent-acp 0.78.0 had no usable instructions channel was
based on incomplete evidence, not the actual dist. A rigorous, fresh
re-verification of the installed dist (see
``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``'s own
docstring for the full file:line evidence chain: adapter ->
``@anthropic-ai/claude-agent-sdk`` -> spawned CLI control protocol) found a
real, code-confirmed system-prompt channel (``session/new``'s own
``params._meta.systemPrompt``), distinct from ordinary user content at
every layer and honored on resume/load, not merely session/new. This file
now asserts QUALIFIED for exactly that pinned (engine, version) -- the
tests below that previously asserted UNSUPPORTED for
``("claude-agent-acp", "0.78.0")`` are updated in place, not merely
appended to; any *other*/unrecognized claude-agent-acp version, or any
other unknown engine, still correctly resolves to UNSUPPORTED (never a
guessed/interpolated upgrade for evidence read against a different
version).

Further updated by T035 (2026-09-18): a live paid run against
``gpt-5.6-luna`` behind codex-acp 1.12.0 (see
``.cortex/acp-migration/evidence/T035/
RUN-03-codex-luna-tagged-instructions.md`` and
``engine.instruction_delivery_registry.CodexAcpDelivery``'s own
docstring for the full evidence) promoted codex-acp 1.12.0's tagged
leading-block channel from UNQUALIFIED to QUALIFIED. Tests below that
previously asserted UNQUALIFIED for ``("codex-acp", "1.12.0")`` are
updated in place to assert QUALIFIED; the refusal/escape-hatch tests that
used to rely on codex-acp as their UNQUALIFIED stand-in now use a
locally-defined fake mechanism instead (``_FakeUnqualifiedDelivery``
below), since no currently-registered engine is UNQUALIFIED any more --
this keeps those tests asserting the actual UNQUALIFIED refusal/escape
behavior of ``qualify_delivery`` rather than being weakened or dropped.
"""
from __future__ import annotations

from dataclasses import dataclass

import pytest

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable import roles
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.instruction_delivery import (
    DeliveryPlan,
    DeliveryQualification,
    UnsupportedDelivery,
    qualify_delivery,
)


@dataclass(frozen=True)
class _FakeUnqualifiedDelivery:
    """A minimal ``InstructionDelivery`` stand-in reporting UNQUALIFIED,
    for tests that exercise ``qualify_delivery``'s UNQUALIFIED-specific
    refusal/escape-hatch behavior without depending on any real
    registered engine remaining UNQUALIFIED (see module docstring: codex-acp
    1.12.0, the only previous UNQUALIFIED example, was promoted to
    QUALIFIED by T035 live evidence)."""

    engine: str = "fake-unqualified-engine"
    version: str = "0.0.0"

    def plan(self, role, user_blocks: list) -> DeliveryPlan:
        return DeliveryPlan(
            qualification=DeliveryQualification.UNQUALIFIED,
            engine=self.engine,
            version=self.version,
            channel="fake leading text block",
            blocks=({"type": "text", "text": "fake"},),
        )


def test_delivery_for_codex_acp_pinned_version():
    mechanism = delivery_for("codex-acp", "1.12.0")
    assert mechanism.engine == "codex-acp"
    assert mechanism.version == "1.12.0"


def test_delivery_for_claude_agent_acp_pinned_version():
    mechanism = delivery_for("claude-agent-acp", "0.78.0")
    assert mechanism.engine == "claude-agent-acp"
    assert mechanism.version == "0.78.0"


def test_delivery_for_claude_agent_acp_unrecognized_version_is_wildcard_unsupported():
    mechanism = delivery_for("claude-agent-acp", "0.99.0")
    assert mechanism.engine == "claude-agent-acp"
    assert mechanism.version == "*"
    assert mechanism.plan(None, []).qualification == DeliveryQualification.UNSUPPORTED


def test_delivery_for_unknown_engine_is_unsupported():
    mechanism = delivery_for("some-future-engine", "9.9.9")
    assert isinstance(mechanism, UnsupportedDelivery)


def test_delivery_for_unknown_codex_acp_version_is_unsupported_not_guessed():
    mechanism = delivery_for("codex-acp", "2.0.0")
    assert isinstance(mechanism, UnsupportedDelivery)


def test_codex_acp_plan_is_qualified():
    """The verified private adapter sends role text through developerInstructions."""
    role = roles.load_role("worker-backend-dev")
    plan = delivery_for("codex-acp", "1.12.0").plan(role, [])
    assert plan.qualification == DeliveryQualification.QUALIFIED
    assert plan.channel == "session/new _meta.systemPrompt to thread/start.developerInstructions"
    assert plan.blocks == ()
    assert plan.session_new_meta == {"systemPrompt": role.body_portable}


def test_claude_agent_acp_pinned_plan_is_qualified_via_session_new_meta():
    role = roles.load_role("worker-backend-dev")
    plan = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
    assert plan.qualification == DeliveryQualification.QUALIFIED
    assert plan.channel == "session/new _meta.systemPrompt"
    # Never delivered via a leading prompt block for this pinned version --
    # a distinct, session/new-level channel instead.
    assert plan.blocks == ()
    assert plan.session_new_meta['systemPrompt']=={'append':role.body_portable}
    assert 'tools' not in plan.session_new_meta['claudeCode']['options']
    assert plan.session_new_meta['claudeCode']['options']['strictMcpConfig'] is True


def test_claude_agent_acp_unrecognized_version_plan_is_unsupported():
    role = roles.load_role("worker-backend-dev")
    plan = delivery_for("claude-agent-acp", "0.11.0").plan(role, [])
    assert plan.qualification == DeliveryQualification.UNSUPPORTED
    assert plan.blocks == ()
    assert plan.session_new_meta is None


def test_qualify_delivery_refuses_unqualified_for_critical_role_by_default():
    role = roles.load_role("worker-backend-dev")
    assert role.critical_rules
    mechanism = _FakeUnqualifiedDelivery()
    with pytest.raises(PortableError) as excinfo:
        qualify_delivery(role, mechanism, [])
    error = excinfo.value
    assert error.code == "instruction_delivery_unqualified"
    assert error.retry_eligible is False
    assert mechanism.engine in error.remediation
    assert mechanism.version in error.remediation


def test_qualify_delivery_admits_qualified_codex_acp_for_critical_role():
    """A codex-acp critical-rules role is no longer refused by default at
    the pinned 1.12.0 version -- T035 promoted this channel to QUALIFIED,
    so it needs no escape hatch at all any more (see
    ``CodexAcpDelivery``'s own docstring for the live evidence)."""
    role = roles.load_role("worker-backend-dev")
    assert role.critical_rules
    plan = qualify_delivery(role, delivery_for("codex-acp", "1.12.0"), [])
    assert plan.qualification == DeliveryQualification.QUALIFIED


def test_qualify_delivery_admits_qualified_claude_agent_acp_for_critical_role():
    """A claude-agent-acp critical-rules role is no longer refused by
    default at the pinned 0.78.0 version -- the QUALIFIED channel needs no
    escape hatch at all (unlike the fake UNQUALIFIED stand-in above)."""
    role = roles.load_role("worker-backend-dev")
    assert role.critical_rules
    plan = qualify_delivery(role, delivery_for("claude-agent-acp", "0.78.0"), [])
    assert plan.qualification == DeliveryQualification.QUALIFIED


def test_qualify_delivery_refuses_unsupported_claude_agent_acp_for_critical_role():
    """An *unrecognized* claude-agent-acp version is still refused for a
    critical-rules role -- the QUALIFIED upgrade above is scoped to
    exactly the pinned 0.78.0 evidence, never any version."""
    role = roles.load_role("worker-backend-dev")
    with pytest.raises(PortableError) as excinfo:
        qualify_delivery(role, delivery_for("claude-agent-acp", "0.11.0"), [])
    assert excinfo.value.code == "instruction_delivery_unqualified"


def test_qualify_delivery_allows_unqualified_when_escape_hatch_set():
    role = roles.load_role("worker-backend-dev")
    plan = qualify_delivery(role, _FakeUnqualifiedDelivery(), [], allow_unqualified_delivery=True)
    assert plan.qualification == DeliveryQualification.UNQUALIFIED


def test_qualify_delivery_escape_hatch_does_not_admit_unsupported():
    """The escape hatch only widens UNQUALIFIED, never UNSUPPORTED."""
    role = roles.load_role("worker-backend-dev")
    with pytest.raises(PortableError):
        qualify_delivery(role, delivery_for("claude-agent-acp", "0.11.0"), [], allow_unqualified_delivery=True)


def test_qualify_delivery_allows_any_qualification_for_role_without_critical_rules():
    role = roles.load_role("content-safety")
    assert role.critical_rules == ()
    plan = qualify_delivery(role, delivery_for("claude-agent-acp", "0.11.0"), [])
    assert plan.qualification == DeliveryQualification.UNSUPPORTED


def test_delivery_for_rejects_empty_engine_or_version():
    with pytest.raises(PortableError):
        delivery_for("", "1.0.0")
    with pytest.raises(PortableError):
        delivery_for("codex-acp", "")
