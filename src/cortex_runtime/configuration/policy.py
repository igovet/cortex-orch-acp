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


# T035 ("the coordinator must not be able to work on the project" -- user
# requirement, verbatim: "Важно что бы наш рантайм/движок не давал
# возможности координатору работать с проектом, потому что координатор это
# route"): one entry per pinned live engine (see
# ``engine.codex_adapter_acp``/``engine.claude_adapter_acp`` module
# docstrings for the full pinned "mode" vocabulary each engine actually
# offers).
#
# FINDING-12 (live evidence, see
# ``.cortex/acp-migration/evidence/T035/FINDING-12-read-only-mode-does-not-
# prevent-edits.md``): this floor previously picked codex "read-only" /
# claude "plan" -- each engine's own non-editing mode -- on the theory that
# a mode the engine itself will not edit under is sufficient enforcement.
# Live evidence proved that wrong on both counts. The mode was requested
# AND confirmed by codex-acp (the task's own ``configuration_revisions``
# row recorded ``parameters={"mode":"read-only"}`` with a real
# ``confirmation_evidence_ref``), and the coordinator still edited
# ``calc.py`` and created ``test_calc.py`` directly -- it never delegated.
# Worse, "read-only" turned out to *remove* Cortex's only interception
# point: in that mode the engine never issues ``session/request_permission``
# for its edits at all, so ``engine.permissions.PermissionBridge`` never
# even sees the attempt to refuse it. A mode chosen specifically to stop
# edits made every edit invisible instead.
#
# This floor now picks the SAME mode as ``WORKER_MODE_FLOOR`` below --
# codex "agent", claude "default" -- the narrowest mode a previous builder
# already verified, against the pinned installed builds cited in that
# floor's own docstring, still issues ``session/request_permission`` for a
# mutating tool call. The point is not to make the engine self-censor (that
# was the failed theory); it is to regain the interception point so
# ``PermissionBridge`` can actually classify and refuse a coordinator
# binding's mutating request itself (see that module's own T035 coordinator-
# refusal docstring) -- enforcement now lives in Cortex, not in a mode
# string the engine is free to ignore.
#
# ``configuration.service.ConfigurationService.propose`` applies this as a
# floor on the coordinator's own configuration revision (the only "mode"
# propose ever sets -- see that method's own docstring for the exact
# precedence and its one disclosed gap: a connection's very first proposal,
# before any live session has ever reported its own "mode" options).
COORDINATOR_MODE_FLOOR: Mapping[str, str] = {
    "codex-acp": "agent",
    "claude-agent-acp": "default",
    # OpenCode's "mode" is its agent; Cortex offers one, ``cortex``, whose
    # permission rules ask for every native tool call (see WORKER_MODE_FLOOR).
    "opencode": "cortex",
}

# T035 gap closure ("a worker's engine mode is never chosen or confirmed by
# Cortex" -- disclosed, not fixed, by the builder who landed
# ``engine.permissions.PermissionBridge``): the user's rule is "workers get
# full working capability except deletion, which must be confirmed", and
# that rule only binds while the engine actually issues
# ``session/request_permission`` for it -- ``PermissionBridge`` can only
# ever classify/relay a request that reaches it. A mode the engine will
# silently auto-approve everything under (never emitting the request at
# all) makes the "except deletion" half of the rule a no-op; a mode that
# asks about literally everything is safe but needlessly slow. This floor
# picks the narrowest mode still known -- from the pinned installed builds,
# not merely their own naming -- to always route a genuinely unsafe/
# destructive action through ``session/request_permission``:
#
# - codex-acp 1.12.0 and 1.13.0 (``dist/index.js`` ``_AgentMode`` table,
#   byte-identical in both qualified bundles): "agent"
#   ("Approve for me") has ``approvalPolicy: "on-request"`` with
#   ``approvalsReviewer: "auto_review"`` in the upstream bundle. In a real
#   Paseo run this reviewer approved an exact `rm -- <sentinel>` internally,
#   without any ACP permission request. Cortex therefore starts a private,
#   version-verified copy of the adapter that changes only this reviewer to
#   `user`; a requested approval can then reach PermissionBridge. This does
#   not guarantee that every native command requests approval. Only
#   "agent-full-access" sets
#   ``approvalPolicy: "never"`` (never asks, ever -- the mode this floor
#   must never pick); "read-only" cannot edit at all, but never issues
#   ``session/request_permission`` for the edits it silently allows either
#   (see FINDING-12 above), so it is never picked here or by
#   ``COORDINATOR_MODE_FLOOR`` -- enforcement for a worker's deletion rule,
#   and for a coordinator's own no-mutation rule, is Cortex-side
#   classification in ``engine.permissions``, never an engine mode string.
# - claude-agent-acp 0.78.0 (``dist/permissions/modes.js``,
#   ``dist/session-mode.js``, ``dist/acp-agent.js`` ``canUseTool``): "default"
#   ("Manual" -- "Always ask before making changes") is the one mode with no
#   engine-side auto-allow path at all, so every mutating tool call --
#   ordinary or destructive -- reaches ``canUseTool``/
#   ``session/request_permission``. "acceptEdits" auto-accepts file edits
#   (a delete-by-edit tool call could never reach the bridge at all);
#   "bypassPermissions" is applied by Claude Code *before* ``canUseTool`` is
#   ever invoked per that file's own comment ("a request that still reaches
#   this callback is deliberately bypass-immune") -- it never asks, ever,
#   the same disqualifying shape as codex's "agent-full-access". "auto"
#   ("Claude handles permission decisions") also delegates safe-looking
#   calls without asking; only "default" gives the unconditional guarantee
#   this floor needs.
#
# Applied by ``delegation.live_adapters.EngineSessionManagerAdapter
# ._apply_worker_mode_floor`` to every worker's own freshly-opened engine
# session, confirmed (never merely requested) over the wire via the same
# ``session/set_config_option`` confirm-or-fail path
# ``configuration.service.ConfigurationService.apply`` already uses for the
# coordinator floor (see that method's own module-docstring pointer to
# ``delegation.live_adapters._PortableRevisionConfigureAdapter``) -- see
# ``_apply_worker_mode_floor``'s own docstring for exactly why it reuses
# that adapter directly rather than a second ``ConfigurationService.apply``
# call, and what happens when the engine cannot confirm this floor.
#
# HONEST BOUNDARY (unchanged by this floor, see ``engine.permissions``'s own
# module docstring): this only ever governs a permission decision the
# engine itself routes through ``session/request_permission``. A native
# tool that deletes without ever issuing that request is invisible to, and
# not enforced by, Cortex regardless of which mode a session runs in.
WORKER_MODE_FLOOR: Mapping[str, str] = {
    "codex-acp": "agent",
    "claude-agent-acp": "default",
    # opencode 2.0.19: the ``cortex`` agent Cortex configures
    # (engine.opencode_adapter_acp) with ``"*": "ask"`` -- observed to turn
    # every native tool call into session/request_permission -- and the
    # only mode such a session offers.
    "opencode": "cortex",
}

# T035 FINDING-02 / EXT-01 #2: the one error_code that means "this engine
# has no configuration surface at all" (engine.session_manager
# .EngineSessionManager.configure: ``options`` -- this session's own
# observed configOptions -- was empty, so nothing was ever sent to the
# wire). There is no specific requested value to have silently substituted
# for it -- the session simply runs the engine's one and only mode, exactly
# as it always would have. Every *other* 'failed'/'rejected'/'indeterminate'
# error_code (an engine that DOES have configurable options but refused/
# only partially confirmed a specific requested one) is a real requested-
# and-refused-or-unconfirmed value and must still gate the turn. Shared by
# ``acp.orchestrating_runtime.OrchestratingRuntime._reject_if_configuration_failed``
# (task-level, pre-EXT-01-#2 gate) and
# ``delegation.live_adapters.EngineSessionManagerAdapter
# .reconcile_configuration`` (per-session, EXT-01 #2 gate) -- one shared
# vocabulary, never two independently-maintained copies.
BENIGN_CONFIGURATION_ERROR_CODES = frozenset({"engine_does_not_support_configuration_options"})

CANCELLATION = "cancellation"
REFUSAL = "refusal"
AUTHORIZATION_LOSS = "authorization_loss"
PROVIDER_RESTRICTION = "provider_restriction"

# Never eligible for fallback, regardless of what any fallback_policy allows.
FALLBACK_HARD_DENYLIST = frozenset({CANCELLATION, REFUSAL, AUTHORIZATION_LOSS, PROVIDER_RESTRICTION})
# T028: engine.failure_classification.RATE_LIMITED is *not* added here (a
# pre-existing test, test_configuration_ceiling_layers.py, already uses the
# literal string "rate_limited" as an arbitrary example fallback_policy
# class name unrelated to T028's own taxonomy -- adding it to this
# constructor-time-validated denylist would reject that fixture). Rate-limit
# non-eligibility is instead enforced the same way every T028 class other
# than ENGINE_FAILURE already is: KNOWN_FALLBACK_ELIGIBLE_CLASSES below only
# ever contains ENGINE_FAILURE/CAPACITY_UNAVAILABLE, and
# routing.authorization.authorize_fallback refuses any failure_class outside
# it unconditionally -- see engine.failure_classification's module docstring
# and its own test coverage (tests/test_t028_failures_cancellation_disconnects.py).

# T027: the only two failure classes a durable fallback_policy may actually
# name (see the task's own lead decision #4) -- distinct from the hard
# denylist above, which can never be overridden. A fallback_policy naming
# any *other* class is still accepted by FallbackPolicy itself (it only
# rejects the hard denylist), but ``routing.authorization.authorize_fallback``
# refuses to act on any class outside this pair, closing the "arbitrary
# failure class" gap the hard denylist alone does not cover.
ENGINE_FAILURE = "engine_failure"
CAPACITY_UNAVAILABLE = "capacity_unavailable"
KNOWN_FALLBACK_ELIGIBLE_CLASSES = frozenset({ENGINE_FAILURE, CAPACITY_UNAVAILABLE})

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


@dataclass(frozen=True)
class RouteProfile:
    """T027: the user-authorized mixed-team route assignment for one
    configuration revision.

    ``role_routes is None`` (the default) means *single-route inheritance*:
    every role, including every worker, is only ever authorized for the
    coordinator's own active route -- no mixed-team profile is in effect.
    An explicit, non-``None`` ``role_routes`` maps a role name to the
    closed set of route ids that role may be dispatched on. A *named*
    role (anything other than the generic ``"worker"``/``"coordinator"``
    buckets -- e.g. a resolved portable role id like ``"reviewer"``)
    absent from the mapping is denied by default (empty set): a mixed-team
    profile is a closed allowlist, not "unlisted roles inherit the
    coordinator route". The generic buckets keep the single-route-
    inheritance default only when no role-specific identity was available
    to the caller (a legacy/no-role-id call site) -- never used to bypass
    a profile that names the real role. An empty set for a role is a
    deliberate "this role may never be dispatched" -- distinct from the
    role being absent.

    This never encodes a universal vendor/role ranking: every id here must
    already be a route the ``PolicyCeiling.allowed_routes`` at the same
    layer permits (checked separately by
    ``routing.authorization.allowed_routes_for_role``, never by this
    dataclass, which has no ceiling of its own).
    """

    role_routes: Mapping[str, frozenset] | None = None

    # Roles with no resolved, role-specific identity (a caller that never
    # threaded a real portable role id through) -- the only roles that
    # still fall back to the coordinator's own route when absent from an
    # explicit role_routes mapping.
    _GENERIC_ROLES = frozenset({"worker", "coordinator"})

    def __post_init__(self) -> None:
        if self.role_routes is not None:
            normalized = {role: frozenset(ids) for role, ids in self.role_routes.items()}
            object.__setattr__(self, "role_routes", normalized)

    def allowed_routes_for(self, role: str, coordinator_route_id: str) -> frozenset:
        """The closed set of route ids ``role`` may be dispatched on.

        Never returns an unbounded/None set: absent a mixed-team profile
        (``role_routes is None``) the result is always exactly
        ``{coordinator_route_id}``. With an explicit profile, a *named*
        role missing from it is denied (empty set); only the generic
        ``"worker"``/``"coordinator"`` buckets still default to the
        coordinator's own route when absent.
        """
        if self.role_routes is None:
            return frozenset({coordinator_route_id})
        if role in self.role_routes:
            return self.role_routes[role]
        if role in self._GENERIC_ROLES:
            return frozenset({coordinator_route_id})
        return frozenset()

    def is_default_single_route(self) -> bool:
        return self.role_routes is None


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
