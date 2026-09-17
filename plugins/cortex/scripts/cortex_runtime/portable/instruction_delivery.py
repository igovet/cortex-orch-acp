"""Instruction-delivery qualification: how confidently a role's instructions
(and its critical rules) actually reach a given engine/adapter/version.

Engine-agnostic on purpose (see ``tests/test_portable_no_native_tokens.py``):
this module defines the closed qualification vocabulary, the plan shape, and
the refusal policy, but never names a specific engine/vendor. Concrete
adapter-specific ``InstructionDelivery`` implementations and the
engine+version registry that resolves one for a given adapter live outside
``portable/`` -- see ``engine.instruction_delivery_registry``.

No ACP release exposes a universal "system prompt"/"skills" field (see
``docs/acp/roles-and-delegation.md`` and each adapter's own evidence-status
documentation: selected / configuration_applied / execution_verified are
distinct facts, and no pinned adapter release has been shown, with live
evidence, to treat delivered instructions as anything other than ordinary
user text). This module names that uncertainty explicitly instead of
assuming delivery "just works", and refuses to admit a role that carries
``critical_rules`` to a channel that is not confirmed QUALIFIED.
"""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Protocol

from .errors import PortableError, PortableErrorCategory
from .roles import RoleInstructionSet


class DeliveryQualification(str, Enum):
    """How confidently an adapter/version delivers instructions to the engine.

    qualified
        Live evidence (see T035-T037) confirms the engine actually treats
        the delivered content as instructions distinct from ordinary user
        text -- for example a confirmed system-role channel or a documented,
        version-pinned engine behavior.
    unqualified
        A concrete delivery mechanism exists and is exercised (for example
        a leading prompt block), but nothing in available evidence confirms
        the engine gives it any different standing than normal user text.
    unsupported
        No delivery mechanism is implemented/evidenced for this
        adapter+version at all.
    """

    QUALIFIED = "qualified"
    UNQUALIFIED = "unqualified"
    UNSUPPORTED = "unsupported"


@dataclass(frozen=True)
class DeliveryPlan:
    """The concrete blocks/arguments an adapter would send, plus its qualification."""

    qualification: DeliveryQualification
    engine: str
    version: str
    channel: str
    blocks: tuple[dict, ...] = ()


class InstructionDelivery(Protocol):
    """One adapter+version's instruction-delivery mechanism."""

    engine: str
    version: str

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        ...


@dataclass(frozen=True)
class UnsupportedDelivery:
    """Generic fallback for any engine/version with no registered mechanism.

    Reusable by any adapter-specific registry (see
    ``engine.instruction_delivery_registry.delivery_for``) as the answer for
    an unknown engine or an unknown version of a known engine -- never a
    guessed/interpolated qualification.
    """

    engine: str
    version: str

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        return DeliveryPlan(
            qualification=DeliveryQualification.UNSUPPORTED,
            engine=self.engine,
            version=self.version,
            channel="none",
            blocks=(),
        )


def qualify_delivery(
    role: RoleInstructionSet,
    mechanism: InstructionDelivery,
    user_blocks: list,
    *,
    allow_unqualified_delivery: bool = False,
) -> DeliveryPlan:
    """Build a delivery plan, refusing an under-qualified channel for a
    role that carries non-negotiable ``critical_rules``.

    ``mechanism`` is an already-resolved :class:`InstructionDelivery` (see
    ``engine.instruction_delivery_registry.delivery_for(engine, version)``
    for how callers typically obtain one) -- this function never resolves
    an engine name itself, keeping it engine-agnostic.

    A role with no ``critical_rules`` may be delivered at any qualification
    (there is nothing non-negotiable to risk losing). A role with
    ``critical_rules`` requires QUALIFIED delivery unless the caller passes
    ``allow_unqualified_delivery=True`` -- an explicit constructor-level
    escape hatch on the provisioning service, never a per-call tool
    argument (see ``delegation.provisioning.ProvisioningService``).
    """
    plan = mechanism.plan(role, user_blocks)
    if role.critical_rules and plan.qualification != DeliveryQualification.QUALIFIED:
        if not (allow_unqualified_delivery and plan.qualification == DeliveryQualification.UNQUALIFIED):
            raise PortableError(
                category=PortableErrorCategory.AUTHORIZATION,
                code="instruction_delivery_unqualified",
                remediation=(
                    f"{plan.engine} {plan.version} delivers role '{role.role_id}' instructions "
                    f"via '{plan.channel}' at qualification '{plan.qualification.value}', "
                    "but this role carries non-negotiable critical_rules. Qualify the "
                    "channel via T035 live evidence, choose a QUALIFIED engine, or "
                    "(coordinator-owned, non-tool-argument) construct the provisioning "
                    "service with allow_unqualified_delivery=True to proceed at reduced "
                    "confidence."
                ),
                retry_eligible=False,
                field="engine",
                details=f"engine={plan.engine} version={plan.version} channel={plan.channel} qualification={plan.qualification.value}",
            )
    return plan
