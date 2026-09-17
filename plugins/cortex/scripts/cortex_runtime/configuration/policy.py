"""Policy ceilings: dimensions, intersection, and narrowing enforcement.

A :class:`PolicyCeiling` exists only at the user and project layers (never at
task/assignment, which may only *narrow* an already-resolved ceiling, never
define a fresh one). ``None`` on any dimension means "unrestricted at this
layer"; intersecting two ceilings always produces the more restrictive value
on every dimension independently. Any candidate configuration -- a task or
assignment override, or a narrower project ceiling on top of a user ceiling
-- that would *expand* any dimension beyond the already-effective ceiling is
rejected with ``ceiling_violation`` before any row is written.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import IntEnum
from typing import Mapping

from ..portable.errors import PortableError, PortableErrorCategory

# Ordered so that a higher member is a *broader* grant; narrowing means
# "the candidate's level must be <= the ceiling's level".
class PermissionLevel(IntEnum):
    READ_ONLY = 0
    STANDARD = 1
    ELEVATED = 2


# Failure classes a fallback_policy may name. The hard denylist below can
# never be overridden by any fallback_policy, no matter how permissive.
class FailureClass(str):
    pass


CANCELLATION = "cancellation"
REFUSAL = "refusal"
AUTHORIZATION_LOSS = "authorization_loss"
PROVIDER_RESTRICTION = "provider_restriction"

# Never eligible for fallback, regardless of what any fallback_policy allows.
FALLBACK_HARD_DENYLIST = frozenset({CANCELLATION, REFUSAL, AUTHORIZATION_LOSS, PROVIDER_RESTRICTION})

_LIMIT_NAMES = frozenset({"attempts", "calls", "tokens", "concurrency"})


def _violation(dimension: str, detail: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="ceiling_violation",
        remediation=(
            f"'{dimension}' would expand beyond the already-effective ceiling ({detail}); "
            "a project ceiling may only narrow the user ceiling, and a task/assignment "
            "override may only narrow the effective ceiling further."
        ),
        retry_eligible=False,
        field=dimension,
    )


def _unenforceable_spend_limit() -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unenforceable_spend_limit",
        remediation=(
            "A spend_limit may only be set when cost telemetry is enforceable for the "
            "selected route; unknown cost is never treated as zero. Remove spend_limit or "
            "select a route whose connection reports enforceable cost telemetry."
        ),
        retry_eligible=False,
        field="spend_limit",
    )


def _subset_or_none(candidate: frozenset | None, ceiling: frozenset | None, dimension: str) -> None:
    if ceiling is None:
        return
    if candidate is None or not candidate.issubset(ceiling):
        raise _violation(dimension, f"ceiling allows only {sorted(ceiling)}")


@dataclass(frozen=True)
class FallbackPolicy:
    """Fallback rules; the hard denylist always applies on top of this."""

    allowed_target_routes: frozenset | None = None
    # None means "no failure-class restriction beyond the hard denylist"
    # (still gated by data_recipient_consent below); an explicit empty
    # frozenset means "no failure class may ever trigger fallback" -- the
    # two are deliberately distinct, mirroring allowed_target_routes' own
    # None-vs-empty-set convention.
    allowed_failure_classes: frozenset | None = None
    budget: int | None = None
    data_recipient_consent: bool = False

    def __post_init__(self) -> None:
        if self.allowed_target_routes is not None:
            object.__setattr__(self, "allowed_target_routes", frozenset(self.allowed_target_routes))
        if self.allowed_failure_classes is not None:
            object.__setattr__(self, "allowed_failure_classes", frozenset(self.allowed_failure_classes))
        if self.allowed_failure_classes is not None and self.allowed_failure_classes & FALLBACK_HARD_DENYLIST:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="fallback_denylisted_failure_class",
                remediation=(
                    "A fallback_policy may never name cancellation, refusal, "
                    "authorization_loss, or provider_restriction as an allowed failure "
                    "class; those can never trigger fallback."
                ),
                retry_eligible=False,
                field="allowed_failure_classes",
            )
        if self.budget is not None and (isinstance(self.budget, bool) or not isinstance(self.budget, int) or self.budget < 0):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'budget' must be a non-negative integer or None.",
                retry_eligible=False,
                field="budget",
            )

    def permits(self, failure_class: str) -> bool:
        """True only when ``failure_class`` may trigger fallback under this policy.

        Never true for a hard-denylisted class, and requires
        ``data_recipient_consent`` since falling over always changes the
        data recipient.
        """
        if failure_class in FALLBACK_HARD_DENYLIST:
            return False
        allowed = self.allowed_failure_classes is None or failure_class in self.allowed_failure_classes
        return allowed and self.data_recipient_consent

    def intersect(self, other: "FallbackPolicy") -> "FallbackPolicy":
        routes = _intersect_optional_set(self.allowed_target_routes, other.allowed_target_routes)
        return FallbackPolicy(
            allowed_target_routes=routes,
            allowed_failure_classes=_intersect_optional_set(self.allowed_failure_classes, other.allowed_failure_classes),
            budget=_min_optional(self.budget, other.budget),
            data_recipient_consent=self.data_recipient_consent and other.data_recipient_consent,
        )


def _intersect_optional_set(a: frozenset | None, b: frozenset | None) -> frozenset | None:
    if a is None:
        return b
    if b is None:
        return a
    return a & b


def _min_optional(a: int | None, b: int | None) -> int | None:
    if a is None:
        return b
    if b is None:
        return a
    return min(a, b)


@dataclass(frozen=True)
class PolicyCeiling:
    """A closed set of policy dimensions; only at user/project layers.

    ``spend_limit`` may only be non-``None`` when
    ``spend_limit_enforceable`` is ``True``; constructing otherwise raises
    ``unenforceable_spend_limit`` (unknown cost is never treated as zero).
    """

    allowed_routes: frozenset | None = None
    allowed_connections: frozenset | None = None
    allowed_data_recipients: frozenset | None = None
    permission_level: PermissionLevel | None = None
    limits: Mapping[str, int] = field(default_factory=dict)
    spend_limit: int | None = None
    spend_limit_enforceable: bool = False
    fallback_policy: FallbackPolicy = field(default_factory=FallbackPolicy)

    def __post_init__(self) -> None:
        for name, value in (
            ("allowed_routes", self.allowed_routes),
            ("allowed_connections", self.allowed_connections),
            ("allowed_data_recipients", self.allowed_data_recipients),
        ):
            if value is not None:
                object.__setattr__(self, name, frozenset(value))
        limits = dict(self.limits)
        unknown = set(limits) - _LIMIT_NAMES
        if unknown:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation=f"'limits' keys must be a subset of {sorted(_LIMIT_NAMES)}; got unknown key(s) {sorted(unknown)}.",
                retry_eligible=False,
                field="limits",
            )
        for key, value in limits.items():
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="invalid_field",
                    remediation=f"limits['{key}'] must be a non-negative integer.",
                    retry_eligible=False,
                    field="limits",
                )
        object.__setattr__(self, "limits", dict(limits))
        if self.spend_limit is not None and not self.spend_limit_enforceable:
            raise _unenforceable_spend_limit()
        if self.spend_limit is not None and (isinstance(self.spend_limit, bool) or not isinstance(self.spend_limit, int) or self.spend_limit < 0):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'spend_limit' must be a non-negative integer or None.",
                retry_eligible=False,
                field="spend_limit",
            )
        if not isinstance(self.fallback_policy, FallbackPolicy):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'fallback_policy' must be a FallbackPolicy instance.",
                retry_eligible=False,
                field="fallback_policy",
            )

    def intersect(self, other: "PolicyCeiling") -> "PolicyCeiling":
        """The effective ceiling: the more restrictive value on every dimension.

        Only ever used to combine the user ceiling with a project ceiling; a
        project ceiling may only narrow, never widen, the user ceiling (this
        is enforced separately by :func:`require_narrowing` at the point a
        project ceiling is written, not silently here).
        """
        permission = self.permission_level
        if other.permission_level is not None and (permission is None or other.permission_level < permission):
            permission = other.permission_level
        limits = dict(self.limits)
        for key, value in other.limits.items():
            limits[key] = value if key not in limits else min(limits[key], value)
        spend_limit = _min_optional(self.spend_limit, other.spend_limit)
        spend_enforceable = self.spend_limit_enforceable or other.spend_limit_enforceable
        if spend_limit is not None and not (self.spend_limit_enforceable and other.spend_limit_enforceable):
            # An unenforceable layer's spend_limit can never be honored;
            # intersecting drops it rather than silently treating unknown
            # cost as satisfying a numeric cap.
            if self.spend_limit_enforceable != other.spend_limit_enforceable:
                spend_limit = self.spend_limit if self.spend_limit_enforceable else other.spend_limit
                spend_enforceable = True
        return PolicyCeiling(
            allowed_routes=_intersect_optional_set(self.allowed_routes, other.allowed_routes),
            allowed_connections=_intersect_optional_set(self.allowed_connections, other.allowed_connections),
            allowed_data_recipients=_intersect_optional_set(self.allowed_data_recipients, other.allowed_data_recipients),
            permission_level=permission,
            limits=limits,
            spend_limit=spend_limit,
            spend_limit_enforceable=spend_enforceable if spend_limit is not None else False,
            fallback_policy=self.fallback_policy.intersect(other.fallback_policy),
        )


def require_narrowing(candidate: PolicyCeiling, ceiling: PolicyCeiling) -> None:
    """Reject any dimension of ``candidate`` that expands beyond ``ceiling``.

    Raises ``PortableError(code="ceiling_violation")`` before any row is
    written; never partially applies a narrower subset of dimensions.
    """
    _subset_or_none(candidate.allowed_routes, ceiling.allowed_routes, "allowed_routes")
    _subset_or_none(candidate.allowed_connections, ceiling.allowed_connections, "allowed_connections")
    _subset_or_none(candidate.allowed_data_recipients, ceiling.allowed_data_recipients, "allowed_data_recipients")
    if ceiling.permission_level is not None:
        if candidate.permission_level is None or candidate.permission_level > ceiling.permission_level:
            raise _violation("permission_level", f"ceiling allows at most {ceiling.permission_level.name}")
    for key, cap in ceiling.limits.items():
        value = candidate.limits.get(key)
        if value is None or value > cap:
            raise _violation(f"limits.{key}", f"ceiling allows at most {cap}")
    if ceiling.spend_limit is not None:
        if candidate.spend_limit is None or not candidate.spend_limit_enforceable or candidate.spend_limit > ceiling.spend_limit:
            raise _violation("spend_limit", f"ceiling allows at most {ceiling.spend_limit}")
    if ceiling.fallback_policy.allowed_target_routes is not None:
        _subset_or_none(
            candidate.fallback_policy.allowed_target_routes,
            ceiling.fallback_policy.allowed_target_routes,
            "fallback_policy.allowed_target_routes",
        )
    _subset_or_none(
        candidate.fallback_policy.allowed_failure_classes,
        ceiling.fallback_policy.allowed_failure_classes,
        "fallback_policy.allowed_failure_classes",
    )
    if ceiling.fallback_policy.budget is not None:
        if candidate.fallback_policy.budget is None or candidate.fallback_policy.budget > ceiling.fallback_policy.budget:
            raise _violation("fallback_policy.budget", f"ceiling allows at most {ceiling.fallback_policy.budget}")
    if candidate.fallback_policy.data_recipient_consent and not ceiling.fallback_policy.data_recipient_consent:
        raise _violation("fallback_policy.data_recipient_consent", "ceiling does not grant data_recipient_consent")
