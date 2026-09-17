"""Adapter-specific ``InstructionDelivery`` mechanisms and engine+version registry.

Deliberately *not* under ``portable/`` (see
``tests/test_portable_no_native_tokens.py``: the portable package must
never name a specific engine/vendor). This module is the one place that
maps a concrete engine name/version to the
``portable.instruction_delivery.InstructionDelivery`` mechanism that
actually applies to it.
"""
from __future__ import annotations

from dataclasses import dataclass

from ..portable.instruction_delivery import DeliveryPlan, DeliveryQualification, InstructionDelivery, UnsupportedDelivery
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.roles import RoleInstructionSet


@dataclass(frozen=True)
class CodexAcp1_12Delivery:
    """The pinned codex-acp 1.12.0 mechanism: a leading prompt text block.

    Wraps ``engine.codex_adapter_acp.build_first_prompt_blocks`` (T013).
    Always UNQUALIFIED: nothing in the pinned release's static surface
    (no distinct "system"/"instructions" role on the wire) confirms the
    engine treats a leading block differently from any other user text.
    """

    engine: str = "codex-acp"
    version: str = "1.12.0"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        from .codex_adapter_acp import build_first_prompt_blocks

        blocks = build_first_prompt_blocks(role.body_portable, user_blocks)
        return DeliveryPlan(
            qualification=DeliveryQualification.UNQUALIFIED,
            engine=self.engine,
            version=self.version,
            channel="session/prompt leading text block",
            blocks=tuple(blocks),
        )


@dataclass(frozen=True)
class ClaudeAgentAcpDelivery:
    """claude-agent-acp: no static evidence of any delivery mechanism yet.

    Always UNSUPPORTED until a version-gated class replaces this one with
    live evidence (T035 scope); ``plan`` never launches anything.
    """

    engine: str = "claude-agent-acp"
    version: str = "*"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        return DeliveryPlan(
            qualification=DeliveryQualification.UNSUPPORTED,
            engine=self.engine,
            version=self.version,
            channel="none",
            blocks=(),
        )


# Registry keyed by (engine, version). An unknown engine or an unknown
# version of a known engine resolves to UnsupportedDelivery -- never a
# guessed/interpolated qualification.
_REGISTRY: dict[tuple[str, str], InstructionDelivery] = {
    ("codex-acp", "1.12.0"): CodexAcp1_12Delivery(),
    ("claude-agent-acp", "*"): ClaudeAgentAcpDelivery(),
}


def delivery_for(engine: str, version: str) -> InstructionDelivery:
    """Look up the delivery mechanism for an exact engine+version.

    Falls back, in order: exact (engine, version); (engine, "*") wildcard
    registration (used by ``claude-agent-acp`` today, which has no
    version-specific evidence); otherwise :class:`UnsupportedDelivery`.
    """
    if not isinstance(engine, str) or not engine:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation="'engine' must be a non-empty string.",
            retry_eligible=False,
            field="engine",
        )
    if not isinstance(version, str) or not version:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation="'version' must be a non-empty string.",
            retry_eligible=False,
            field="version",
        )
    exact = _REGISTRY.get((engine, version))
    if exact is not None:
        return exact
    wildcard = _REGISTRY.get((engine, "*"))
    if wildcard is not None:
        return wildcard
    return UnsupportedDelivery(engine=engine, version=version)
