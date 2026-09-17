"""T027: pre-dispatch route/capability/budget enforcement and authorized
fallback decisions.

Every function here either returns normally (authorized) or raises a
:class:`~..portable.errors.PortableError` with a concrete ``code`` --
never a silent downgrade, never a partial authorization. Nothing here ever
starts an engine process or reserves capacity itself: callers
(``configuration.service.ConfigurationService.resolve_worker`` and
``delegation.live_adapters.EngineSessionManagerAdapter``) call these
*before* allocation/dispatch and act on the raised error (or its absence)
themselves.
"""
from __future__ import annotations

import hashlib
import json
from typing import Mapping

from . import store as routing_store
from ..configuration.policy import (
    FALLBACK_HARD_DENYLIST,
    KNOWN_FALLBACK_ELIGIBLE_CLASSES,
    PolicyCeiling,
    RouteProfile,
)
from ..configuration.resolution import validate_dependent_settings
from ..portable.errors import PortableError, PortableErrorCategory


def _fail(code: str, remediation: str, *, field: str | None = None, retry_eligible: bool = False) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION if not code.startswith("fallback")
        else PortableErrorCategory.AUTHORIZATION,
        code=code, remediation=remediation, retry_eligible=retry_eligible, field=field,
    )


def fingerprint_params(parameters: Mapping[str, object]) -> str:
    """A stable, order-independent fingerprint of an attempt's effort/mode/
    model-config/summarizer/other parameters -- used only for audit
    (never for equality/dedup of any durable identity), so two attempts
    with byte-identical parameters always audit the identical fingerprint
    and any difference (including a value type change) always audits a
    different one."""
    encoded = json.dumps(dict(parameters), sort_keys=True, ensure_ascii=False, separators=(",", ":"), default=str)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def allowed_routes_for_role(
    route_profile: RouteProfile, ceiling: PolicyCeiling, role: str, coordinator_route_id: str,
) -> frozenset:
    """The role's authorized route ids: the profile's own set narrowed by
    ``ceiling.allowed_routes`` (never the reverse -- the profile can only
    ever narrow within routes the ceiling already permits, mirroring
    ``PolicyCeiling``'s own narrowing-only semantics elsewhere)."""
    from_profile = route_profile.allowed_routes_for(role, coordinator_route_id)
    if ceiling.allowed_routes is None:
        return from_profile
    return from_profile & ceiling.allowed_routes


def authorize_dispatch(
    store, *, task_id: str, role: str, route_profile: RouteProfile, coordinator_route_id: str,
    candidate_route_id: str, ceiling: PolicyCeiling,
    catalog_entry=None, parameters: Mapping[str, object] | None = None,
) -> None:
    """Raise before any allocation/dispatch when ``role`` is not authorized
    for ``candidate_route_id``, the route does not support the requested
    parameters (only checked when ``catalog_entry`` is supplied -- a
    caller that already validated this bundle once at configuration-
    propose time, see ``configuration.service.ConfigurationService
    .propose``, may omit it here rather than reconstruct a full
    ``CatalogEntry`` from a bare route id), or this task has already
    exhausted its ``PolicyCeiling.limits['attempts']`` budget (when
    tracked). Never checks per-connection concurrency itself -- that
    remains ``delegation.capacity.reserve``'s own atomic job, called by
    the caller strictly *after* this returns without raising.
    """
    allowed = allowed_routes_for_role(route_profile, ceiling, role, coordinator_route_id)
    if candidate_route_id not in allowed:
        raise _fail(
            "role_route_not_authorized",
            f"role '{role}' is not authorized for route {candidate_route_id!r} under this task's "
            "route profile/ceiling; only {" + ", ".join(sorted(allowed)) + "} may be dispatched.",
            field="route_id",
        )
    if catalog_entry is not None:
        validate_dependent_settings(catalog_entry, parameters or {})
    limit = ceiling.limits.get("attempts")
    if limit is not None:
        used = routing_store.count_task_attempts(store, task_id)
        if used >= limit:
            raise _fail(
                "attempt_budget_exhausted",
                f"This task has already used {used} of its {limit}-attempt policy ceiling; "
                "no further attempt may be dispatched under the current configuration.",
                field="attempts",
            )


def authorize_fallback(
    store, *, task_id: str, ceiling: PolicyCeiling, failure_class: str, role: str,
    route_profile: RouteProfile, coordinator_route_id: str,
    current_connection, target_connection, target_route_id: str,
) -> None:
    """Raise unless a durable fallback policy explicitly names
    ``failure_class`` as eligible, the target route is within both the
    fallback policy's own allowed targets and this role's route
    authorization, and -- when ``target_connection.provider`` differs from
    ``current_connection.provider`` -- a durable recipient-consent record
    already exists for the target connection/provider. Never mutates
    anything; the caller reconciles the old attempt and dispatches the new
    one only after this returns without raising, then records the
    resulting attempt with :func:`~.store.record_attempt_audit`
    (``decision='fallback'``).
    """
    if failure_class in FALLBACK_HARD_DENYLIST:
        raise _fail(
            "fallback_not_authorized",
            f"'{failure_class}' can never trigger fallback (refusal/cancel/authorization-loss/"
            "provider-limit are permanently excluded from fallback, regardless of policy).",
            field="failure_class",
        )
    if failure_class not in KNOWN_FALLBACK_ELIGIBLE_CLASSES:
        raise _fail(
            "fallback_not_authorized",
            f"'{failure_class}' is not one of the durable fallback-eligible failure classes "
            f"({sorted(KNOWN_FALLBACK_ELIGIBLE_CLASSES)}); no fallback attempt is made.",
            field="failure_class",
        )
    fallback_policy = ceiling.fallback_policy
    if not fallback_policy.permits(failure_class):
        raise _fail(
            "fallback_not_authorized",
            "No durable fallback_policy on this task's effective ceiling names "
            f"'{failure_class}' with recipient consent; stop and report rather than attempt fallback.",
            field="failure_class",
        )
    if fallback_policy.allowed_target_routes is not None and target_route_id not in fallback_policy.allowed_target_routes:
        raise _fail(
            "fallback_route_not_allowed",
            f"route {target_route_id!r} is not in this task's fallback_policy.allowed_target_routes.",
            field="target_route_id",
        )
    role_allowed = allowed_routes_for_role(route_profile, ceiling, role, coordinator_route_id)
    if target_route_id not in role_allowed:
        raise _fail(
            "fallback_role_route_not_authorized",
            f"role '{role}' is not authorized for fallback target route {target_route_id!r} "
            "under this task's route profile/ceiling.",
            field="target_route_id",
        )
    if target_connection.provider != current_connection.provider:
        if not fallback_policy.data_recipient_consent:
            raise _fail(
                "fallback_consent_required",
                "Cross-provider fallback requires fallback_policy.data_recipient_consent=True "
                "in addition to a durable per-recipient consent record; neither transfer proceeds "
                "without both.",
                field="data_recipient_consent",
            )
        if not routing_store.has_active_consent(store, task_id, target_connection.id):
            raise _fail(
                "fallback_consent_required",
                f"No durable, unrevoked consent record exists for target connection "
                f"{target_connection.id!r} (provider {target_connection.provider!r}); record one via "
                "routing.store.record_consent before this fallback may transfer any data.",
                field="target_connection_id",
            )
    if fallback_policy.budget is not None:
        used = routing_store.count_fallback_attempts(store, task_id)
        if used >= fallback_policy.budget:
            raise _fail(
                "fallback_budget_exhausted",
                f"This task has already used {used} of its {fallback_policy.budget}-attempt "
                "fallback budget; no further fallback attempt is authorized.",
                field="budget",
            )
    # A fallback attempt is still an attempt: the overall
    # PolicyCeiling.limits['attempts'] ceiling (when tracked) bounds every
    # attempt this task ever dispatches, fallback included -- never a
    # separate, uncounted budget.
    attempt_limit = ceiling.limits.get("attempts")
    if attempt_limit is not None:
        used_attempts = routing_store.count_task_attempts(store, task_id)
        if used_attempts >= attempt_limit:
            raise _fail(
                "attempt_budget_exhausted",
                f"This task has already used {used_attempts} of its {attempt_limit}-attempt "
                "policy ceiling; no further attempt (including a fallback) may be dispatched.",
                field="attempts",
            )
