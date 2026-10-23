"""Settings-only automatic routing from exact observed catalog identities.

First-existing-file precedence: project .cortex/routing.json, CORTEX_ROUTING,
then user routing.json. model_policy_version=2 is required; installation
assets are never runtime fallbacks. Generic tier, economy, quota, affinity,
retry, availability and authorization rules do not infer model capabilities.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, Mapping, Sequence

from ..catalog.evidence import ProvenanceSource
from ..portable.errors import PortableError, PortableErrorCategory
from . import model_policy

ENV_VAR = "CORTEX_ROUTING"
# A known plan usage at or above this is "hot": such a candidate ranks
# behind every non-hot one (the quota band, see :func:`rank`).
HOT_PERCENT = 95.0
TIERS = model_policy.TIERS
_FALLBACK_ORDER = {
    "flagship": ("flagship", "deep", "standard", "light"),
    "deep": ("deep", "standard", "light"),
    "standard": ("standard", "deep", "light"),
    "light": ("light", "standard", "deep"),
}
EFFORT_BY_TIER = {"flagship": "high", "deep": "high", "standard": "medium", "light": "low"}
EFFORT_ORDER = model_policy.EFFORT_ORDER
_NEVER_EFFORTS = frozenset({"ultra"})
_LIGHT_ONLY_EFFORTS = frozenset({"max"})
_FLAGSHIP_CEILING = "high"

# What a mistake costs: design/security get the strongest models,
# exploration/planning/writing the cheapest. Debugging and performance work
# start at standard and escalate when that falls short (see ``escalate``).
ROLE_TIERS = {
    "worker-architect": "deep",
    "worker-database-architect": "deep",
    "worker-security-auditor": "deep",
    "worker-senior-consultant": "deep",
    "worker-explorer": "light",
    "worker-general": "light",
    "worker-planner": "light",
    "worker-technical-writer": "light",
}

# Which engine is the better fit for a kind of work (engine affinity), used
# among candidates of one tier after quota and plan usage. A judgement, not a
# benchmark: Codex models are tuned for agentic terminal coding (write, run,
# fix), Claude models for reading, judging and writing prose. Debugging and
# testing are both, so they are neutral (``None``: no engine bonus, plan
# usage and engine order decide); investigation is reading. Unlisted roles are
# "implementation_backend". routing.json ``engine_affinity`` overrides this per kind.
WORK_KIND_ENGINES = {
    "implementation_backend": "codex-acp",
    "implementation_frontend": "codex-acp",
    "implementation_frontend_design": "claude-agent-acp",
    "operations": "codex-acp",
    "debugging": None,
    "testing": None,
    "investigation": "claude-agent-acp",
    "review": "claude-agent-acp",
    "design": "claude-agent-acp",
    "writing": "claude-agent-acp",
    "planning": "claude-agent-acp",
    "analysis": "claude-agent-acp",
}
assert tuple(WORK_KIND_ENGINES) == model_policy.WORK_KINDS

# These work kinds require their affinity engine, not merely a ranking bonus.
# Model identities and eligible tiers still come exclusively from owner policy.
AFFINITY_BOUND_WORK_KINDS = frozenset({"implementation_frontend_design"})
ROLE_WORK_KINDS = {
    "worker-accessibility-auditor": "review",
    "worker-accessibility-fixer": "implementation_frontend",
    "worker-architect": "design",
    "worker-backend-dev": "implementation_backend",
    "worker-build-verification": "testing",
    "worker-code-reviewer": "review",
    "worker-data-engineer": "implementation_backend",
    "worker-database-architect": "design",
    "worker-debugger": "debugging",
    "worker-devops-engineer": "operations",
    "worker-explorer": "investigation",
    "worker-frontend-dev": "implementation_frontend",
    "worker-fullstack-dev": "implementation_backend",
    "worker-general": "implementation_backend",
    "worker-mobile-dev": "implementation_frontend",
    "worker-performance-engineer": "debugging",
    "worker-planner": "planning",
    "worker-qa-engineer": "testing",
    "worker-refactorer": "implementation_backend",
    "worker-security-auditor": "review",
    "worker-senior-consultant": "design",
    "worker-technical-writer": "writing",
    "worker-ux-designer": "design",
}


@dataclass(frozen=True)
class Choice:
    connection_id: str
    model: str
    effort: str | None
    basis: str  # "auto" | "auto:pin" | "auto:user_request"
    # What decided it (tier, ranked candidates, the provider preference and
    # whether it was honoured) -- recorded in the attempt's route audit.
    decision: Mapping | None = None
    route_id: str | None = None


def work_kind(role: str | None, override: str | None = None) -> str | None:
    if override is not None:
        if override not in model_policy.WORK_KINDS:
            raise ValueError(f"unknown work kind {override!r}")
        return override
    return None if role is None else ROLE_WORK_KINDS.get(role, "implementation_backend")


def stretch_status(role: str | None, role_tier: str, complexity: str = "normal", kinds=None, *, work_kind_override: str | None = None) -> str:
    """Whether configured light stretch policy may serve this work."""
    if role_tier == "light":
        return "light_tier"
    if complexity == "high":
        return "excluded:complexity_high"
    if role_tier != "standard":
        return f"excluded:{role_tier}_work"
    if ROLE_TIERS.get(role or "", "standard") in ("deep", "flagship"):
        return "excluded:deep_role"
    kind = work_kind(role, work_kind_override)
    if kind not in model_policy.expand_work_kinds(kinds or ()):
        return f"not_candidate:work_kind_{kind}"
    return "standard_candidate"


def hint_serves_tier(model: str, model_tier: str | None, role: str | None, role_tier: str,
                     complexity: str = "normal", *, also_standard_for=None, work_kind_override: str | None = None) -> bool:
    """Whether explicit tiers or configured standard allowances serve this work."""
    kinds = frozenset(model_policy.expand_work_kinds(also_standard_for or ()))
    return model_tier == role_tier or (
        model_tier == "light" and bool(kinds)
        and stretch_status(role, role_tier, complexity, kinds, work_kind_override=work_kind_override) == "standard_candidate")


def stretch_effort(role_tier: str, escalate: bool = False, ladder: Sequence[str] | None = None) -> str | None:
    """Target a configured stretch effort ladder step."""
    return model_policy.stretch_effort(ladder or (), role_tier, escalate)


def _stretch_summary(statuses: Mapping[str, str]) -> str:
    """One historical stretch audit status for every offered stretch model: a
    ``standard_candidate`` when any is one, else their common status."""
    values = set(statuses.values())
    if not values:
        return "not_offered"
    return "standard_candidate" if "standard_candidate" in values else sorted(values)[0]


def kind_engine(kind: str, affinity: Mapping[str, str | None] | None = None) -> str | None:
    """The engine ``kind`` of work leans to, or ``None`` (neutral):
    routing.json ``engine_affinity`` names it, then its ``"*"``, then
    :data:`WORK_KIND_ENGINES`."""
    affinity = affinity or {}
    if kind in affinity:
        return affinity[kind]
    if kind in model_policy.IMPLEMENTATION_KINDS and "implementation" in affinity:
        return affinity["implementation"]
    if "*" in affinity:
        return affinity["*"]
    return WORK_KIND_ENGINES[kind]


def affinity_engine(role: str | None, affinity: Mapping[str, str | None] | None = None, *,
                    work_kind_override: str | None = None) -> str | None:
    """The engine ``role``'s kind of work leans to, or ``None`` (neutral).
    ``affinity`` is routing.json ``engine_affinity`` (see :func:`kind_engine`)."""
    kind = work_kind(role, work_kind_override)
    return None if kind is None else kind_engine(kind, affinity)


def suitability(role: str | None, engine: str, affinity: Mapping[str, str | None] | None = None, *,
                work_kind_override: str | None = None) -> int:
    """1 when ``engine`` is the better fit for ``role``'s kind of work."""
    preferred = affinity_engine(role, affinity, work_kind_override=work_kind_override)
    return int(preferred is not None and preferred == engine)


# A coordinator's model hint is passed over for quota only when its known
# usage is at or above this percentage while the tier has a candidate below
# it (known or unknown usage): a hot hinted route ranks behind non-hot routes,
# exactly like the quota band of :func:`rank`, and the replacement is recorded
# as ``ignored:near_quota_limit``. A lower or merely higher usage never moves
# an eligible hinted route (rank version 15). Excluded, unavailable and
# exhausted (100%) routes are never honoured at all.
HINT_TOLERANCE = HOT_PERCENT
# The owner's own choices (a provider preference, a preferred model for the
# work kind) are passed over for quota only when hot *and* this many points
# worse than an available alternative's known usage (:func:`_meaningfully_worse`).
# Plan usage no longer ranks candidates in steps of this margin: it is only the
# last tie-breaker between otherwise equal candidates (rank version 15).
USAGE_MARGIN = 10.0


def route_menu(project_root=None, environ=None, unavailable=()):
    """Current policy and engine-confirmed identities; dispatch revalidates."""
    from ..connections import quota
    from ..connections import registry as connection_registry
    from . import availability
    from ..portable.roles import WORKER_ALIASES
    overrides = load_overrides(project_root, environ)
    connections = connection_registry.read_registry(environ).connections
    entries = [e for e in _allowed(overrides, _entries(overrides, connections, environ))
               if not _is_seed(e) and not _denied(overrides, e.route.engine, e.route.model)]
    policies = model_policy.resolve_policies(entries, overrides)
    superseded = superseded_routes(entries)
    applied = _applied_superseded(overrides, entries, superseded)
    affinity = overrides.get("engine_affinity") or {}
    roles = sorted(role for role in set(WORKER_ALIASES.values()) if role.startswith("worker-")
                   and role not in {"worker-code-reviewer", "worker-build-verification"})
    result = {tier: [] for tier in TIERS}
    flagship, stretch, preferred, conditional = [], [], {}, {}
    eligible_engines = set()
    for entry in entries:
        engine, model = entry.route.engine, entry.route.model
        policy = policies[entry.route.id]
        label = _route_label(entry)
        used = quota.used_percent(connections[entry.connection_id], model, environ, routing=overrides)
        blocked = availability.blocking(unavailable, entry.connection_id, model)
        reason = _auto_exclusion(overrides, entry) or ("superseded" if entry.route.id in applied else None)
        # A model the owner placed in several tiers is listed under each.
        for tier in policy.tiers:
            result[tier].append(dict(
                route=label, engine=engine, model=model,
                connection_id=entry.connection_id,
                efforts=list(permitted_efforts(entry, tier, policy)),
                state=(f"unavailable_until:{blocked['expires_at']}" if blocked else
                       "exhausted" if used is not None and used >= 100 else "available"),
                auto_eligible=reason is None, auto_reason=reason or "eligible",
                superseded_by=superseded.get(entry.route.id),
                tier_source=policy.tier_source, tiers=list(policy.tiers), selection=policy.selection,
                policy=policy.attributes(),
                quota=dict(band="unknown" if used is None else "hot" if used >= HOT_PERCENT else "cool",
                           used_percent=used)))
        if "flagship" in policy.tiers:
            flagship.append((label, policy))
        if policy.stretch:
            ladder = policy.effort_ladder
            stretch.append(dict(
                route=label, auto_eligible=reason is None, effort_floor=policy.effort_floor,
                efforts=(dict(light=stretch_effort("light", ladder=ladder),
                              standard=stretch_effort("standard", ladder=ladder),
                              standard_escalated=stretch_effort("standard", True, ladder)) if ladder else None),
                standard_work_kinds=sorted(policy.also_standard_for),
                standard_candidate_roles=[role for role in roles if stretch_status(
                    role, "standard", kinds=policy.also_standard_for) == "standard_candidate"],
                never="complexity high, deep or flagship work, deep roles"))
        if reason is None:
            eligible_engines.add(engine)
            if policy.preferred_when is not None:
                conditional[label] = dict(when=policy.preferred_when, work_kinds=sorted(policy.preferred_for))
            else:
                for kind in sorted(policy.preferred_for):
                    preferred.setdefault(kind, []).append(label)
    return dict(tiers=result, quota_window_policy=dict(unmapped="conservative_shared",
                                                      rules=overrides.get("quota_windows", {})),
                roles={role: {complexity: tier_for(role, complexity=complexity)
                                          for complexity in ("low", "normal", "high")}
                                     for role in roles},
                role_work_kinds={role: work_kind(role) for role in roles},
                # Only an engine with a route automatic selection may use is
                # worth a hint: a denied, disabled or unselected engine (or
                # one outside ``providers``) is withheld and named below.
                affinity={kind: (kind_engine(kind, affinity) if kind_engine(kind, affinity) in eligible_engines
                                 else None) for kind in WORK_KIND_ENGINES},
                affinity_not_eligible={kind: kind_engine(kind, affinity) for kind in WORK_KIND_ENGINES
                                       if kind_engine(kind, affinity) is not None
                                       and kind_engine(kind, affinity) not in eligible_engines},
                affinity_source={kind: "routing.json" if kind in affinity or "*" in affinity else "builtin"
                                 for kind in WORK_KIND_ENGINES},
                preferred=preferred,
                # Owner-written conditions only the coordinator can judge: a
                # node that meets one carries that exact route as its hint.
                preferred_when=conditional,
                flagship=dict(routes=[label for label, _policy in flagship],
                              request_names={label: list(policy.request_names) for label, policy in flagship
                                             if policy.request_names},
                              rule="escalation, or an exact route the user explicitly asked for in this task"),
                stretch=stretch)


def _effective_profile_label(project_root, environ):
    """Audit a user profile only when its materialized file actually wins."""
    paths = _candidate_paths(project_root, environ)
    if any(path.is_file() for path in paths[:-1]):
        return None
    try:
        from ..configuration import routing_profiles
        from ..configuration.management import _revision, _read_plain
        return routing_profiles.effective_label(environ, _revision(_read_plain(paths[-1])))
    except PortableError:
        return None


def choose(role: str, project_root=None, environ: Mapping[str, str] | None = None, *,
           escalate: bool = False, complexity: str = "normal", unavailable: Sequence[Mapping] = (),
           preference: str | None = None, model_hint: str = "auto",
           model_reason: str | None = None, previous_engine: str | None = None,
           user_request: Mapping | None = None,
           eligible_route_ids: frozenset[str] | None = None,
           user_revocation: Mapping | None = None, work_kind_override: str | None = None) -> Choice | None:
    """The route this worker role should open on, or ``None`` when nothing
    engine-confirmed is configured (keep the coordinator's route).
    See :func:`tier_for` for ``escalate`` and ``complexity``, and
    :func:`rank` for the coordinator's per-task ``preference`` (an engine
    id); a preference is never honoured over a ``pin`` or outside
    ``providers``, and never moves the tier.

    ``unavailable`` are the unexpired ``routing.availability`` records;
    a route they name is skipped. When routes are configured but none is
    usable, the delegation is refused (``no_available_route``) with every
    unavailable route named -- never the coordinator's route instead.

    User requests are established by user_model_request from retained user
    text. For covered exact hints they precede pins, ranking, automatic
    membership and role tiers, including on retries. Hard owner authorization,
    observed identity, availability and exhausted quota still refuse visibly;
    a requested route is never silently substituted. ``user_revocation``
    (:func:`user_model_revocation`) is never consent; it is recorded in the
    decision as ``user_model_request.status='revoked_by_user'``.

    Tiers, automatic eligibility and per-model policy come from routing.json
    ``models`` only (:mod:`routing.model_policy`). ``eligible_route_ids`` is
    the task profile/ceiling intersection, applied before ranking. An empty
    set grants nothing; a pin outside it is refused, never substituted.
    """
    from ..connections import quota
    from ..connections import registry as connection_registry
    from . import availability

    overrides = load_overrides(project_root, environ)
    work_kind(role, work_kind_override)  # Validate before pins or explicit user routes.
    profile_label = _effective_profile_label(project_root, environ)
    connections = connection_registry.read_registry(environ).connections
    entries = [entry for entry in _entries(overrides, connections, environ) if not _is_seed(entry)]
    allowed = [e for e in _allowed(overrides, entries)
               if not _denied(overrides, e.route.engine, e.route.model)]
    settings_allowed = allowed
    if eligible_route_ids is not None:
        allowed = [e for e in allowed if e.route.id in eligible_route_ids]
    policies = model_policy.resolve_policies(allowed, overrides)
    tiers = {route_id: policy.tier for route_id, policy in policies.items()}
    tier_sets = {route_id: policy.tiers for route_id, policy in policies.items()}
    models = overrides.get("models") or {}
    usage_cache: dict = {}

    def usage(entry):
        key = (entry.connection_id, entry.route.model)
        if key not in usage_cache:
            usage_cache[key] = quota.used_percent(connections[entry.connection_id], entry.route.model, environ, routing=overrides)
        return usage_cache[key]

    blocked = {}
    for entry in entries:
        record = availability.blocking(unavailable, entry.connection_id, entry.route.model)
        if record is not None:
            blocked[entry.route.id] = record
    base_role_tier = tier_for(role, False, complexity)
    role_tier = tier_for(role, escalate, complexity)
    # A retry of standard work of a configured stretch work kind is
    # the intermediate stretch-max band. Decide from the original work
    # before generic escalation labels it deep. Explicit high complexity and
    # deep roles still fail stretch_status here.
    hold_kinds = frozenset().union(
        *(policy.also_standard_for for policy in policies.values() if policy.stretch and policy.selected))
    if escalate and stretch_status(role, base_role_tier, complexity, hold_kinds, work_kind_override=work_kind_override) == "standard_candidate":
        role_tier = base_role_tier
    affinity = overrides.get("engine_affinity") or {}
    # Light-tier stretch models follow their own rule (see stretch_status); a
    # tier override that moves one elsewhere routes it like any model.
    stretch_states = {route_id: stretch_status(role, role_tier, complexity, policy.also_standard_for, work_kind_override=work_kind_override)
                      for route_id, policy in policies.items() if policy.stretch}
    stretch_ids = frozenset(stretch_states)
    stretch_summary = _stretch_summary(stretch_states)
    if user_request is not None and "/" in model_hint:
        requested = _requested_routes(model_hint, overrides, entries, eligible_route_ids, blocked, usage)
        picked = sorted(requested, key=lambda e: _order_key(usage(e)))[0]
        policy = model_policy.policy_for(picked.route.engine, picked.route.model, overrides)
        if not policy.tiers:
            raise _policy_error("user_requested_route_unavailable",
                                f"{model_hint}: no configured tier/effort policy; no substitution permitted.")
        requested_tier = policy.tier
        explicit_effort = user_request.get("effort")
        permitted = efforts_in_any_tier(picked, policy)
        if explicit_effort is not None and explicit_effort not in permitted:
            raise _policy_error("user_requested_effort_unavailable",
                                f"{model_hint}:{explicit_effort} is not supported within current effort policy; "
                                "no substitution permitted. Select a permitted effort or ask the owner to revise policy.")
        effort = (explicit_effort if explicit_effort is not None else
                  nearest(EFFORT_BY_TIER[requested_tier], permitted_efforts(picked, requested_tier, policy)))
        decision = dict(
            role=role, base_role_tier=base_role_tier, role_tier=requested_tier, tier=requested_tier,
            basis="user_request", decided_by="user_request", rank_version=RANK_VERSION,
            escalated=escalate, preference=preference,
            preference_status="ignored:user_request" if preference else "none",
            model_hint=model_hint, model_hint_status="honoured", model_hint_reason=model_reason,
            effort_basis="user_request:explicit_effort" if explicit_effort is not None else f"user_request:{requested_tier}",
            user_model_request=dict(user_request, status="honoured"),
            candidates=[_candidate(e, usage, role, False, affinity=affinity, work_kind_override=work_kind_override) for e in requested],
            chosen=model_hint, chosen_connection_id=picked.connection_id)
        if profile_label is not None:
            decision["routing_profile"] = profile_label
        return Choice(picked.connection_id, picked.route.model, effort, "auto:user_request", decision, picked.route.id)
    kind = work_kind(role, work_kind_override)
    bound_engine = kind_engine(kind, affinity) if kind in AFFINITY_BOUND_WORK_KINDS else None
    if kind in AFFINITY_BOUND_WORK_KINDS and bound_engine is None:
        raise _policy_error("no_available_route", f"{kind} requires a non-neutral engine affinity; no substitution permitted.")
    pin = _pin_for(overrides, role)
    if pin is not None:
        engine, model, effort = pin
        if bound_engine is not None and engine != bound_engine:
            raise _policy_error("pinned_route_unavailable",
                                f"{kind} requires affinity engine {bound_engine}; the pin names {engine}. "
                                "Change the pin or confirm an explicit user model request; no substitution permitted.")
        providers = overrides.get("providers")
        if providers is not None and engine not in providers:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="pinned_route_unavailable",
                remediation=(f"routing.json pins role {role!r} to {engine}/{model}, but 'providers' in "
                             f"routing.json restricts routing to {sorted(providers)}; add {engine!r} to "
                             "'providers' or change or remove the pin."),
                retry_eligible=False, field="pin",
            )
        matches = [e for e in settings_allowed if e.route.engine == engine and e.route.model == model]
        if not matches:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="pinned_route_unavailable",
                remediation=(f"routing.json pins role {role!r} to {engine}/{model}, which no configured "
                             "connection currently offers; connect it or change the pin."),
                retry_eligible=False, field="pin",
            )
        if eligible_route_ids is not None:
            matches = [e for e in matches if e.route.id in eligible_route_ids]
            if not matches:
                raise _policy_error("role_route_not_authorized", "The pinned route is outside this task's profile/ceiling; no substitution is permitted.")
        open_matches = [e for e in matches if e.route.id not in blocked]
        if not open_matches:
            # Strict: a pin is never substituted, even by an available route.
            reasons = "; ".join(sorted({availability.describe(blocked[e.route.id]) for e in matches}))
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="pinned_route_unavailable",
                remediation=(f"routing.json pins role {role!r} to {engine}/{model}, which is unavailable "
                             f"({reasons}); wait until then, or change or remove the pin."),
                retry_eligible=False, field="pin",
            )
        ranked = sorted(open_matches, key=lambda e: _order_key(usage(e)))
        best = ranked[0]
        best_policy = policies[best.route.id]
        if not best_policy.tiers:
            raise _policy_error("policy_missing", "A pinned automatic route needs explicit per-engine tiers.")
        decision = dict(
            role=role, base_role_tier=base_role_tier, role_tier=role_tier,
            tier=tiers.get(best.route.id), basis="pin",
            preference=preference, preference_status="ignored:pinned" if preference else "none",
            rank_version=RANK_VERSION, effort_basis="pin", decided_by="pin",
            candidates=[_candidate(e, usage, role, False, affinity=affinity, work_kind_override=work_kind_override) for e in ranked],
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        decision.update(model_hint=model_hint, model_hint_status=(
            "auto" if model_hint == "auto" else "ignored:escalated" if escalate else "ignored:pinned"),
            model_hint_reason=model_reason, diversify_from=previous_engine if escalate else None,
            stretch=dict(status="pinned" if best.route.id in stretch_ids else stretch_summary, chosen=best.route.id in stretch_ids))
        if user_request is not None:
            decision["user_model_request"] = dict(user_request, status="ignored:pinned")
        elif user_revocation is not None:
            decision["user_model_request"] = dict(user_revocation, status="revoked_by_user")
        if profile_label is not None:
            decision["routing_profile"] = profile_label
        default_effort = (stretch_effort(role_tier, escalate, best_policy.effort_ladder)
                          if best.route.id in stretch_ids and best_policy.effort_ladder
                          else EFFORT_BY_TIER[tier_for(role, escalate, complexity)])
        permitted = efforts_in_any_tier(best, best_policy)
        if effort is None and best_policy.effort_ladder and default_effort not in permitted:
            raise _policy_error("model_effort_policy_unsupported", "Pinned effort_ladder step is not permitted.")
        pinned_effort = nearest(effort or default_effort, permitted)
        if effort is not None and pinned_effort != effort:
            raise _policy_error("pinned_effort_unavailable",
                                f"The owner pin {_route_label(best)}:{effort} conflicts with observed support or effort policy; "
                                "no substitution permitted. Ask the owner to revise the pin or policy.")
        if effort is not None:
            decision["effort_basis"] = "pin:explicit_effort"
        return Choice(best.connection_id, model, pinned_effort,
                      "auto:pin", decision, best.route.id)
    default_denied = {entry.route.id for entry in allowed if _default_auto_denied(overrides, entry)}
    # A model configured with several native tiers (e.g. both
    # "light" and "standard") keeps its own non-stretch tiers even when its
    # *stretch* promotion to standard is excluded for this work; only a
    # model with no other native tier (light-only) is denied outright, so
    # it is not offered as a bare fallback the owner never configured it
    # for either.
    stretch_denied = frozenset(route_id for route_id, status in stretch_states.items()
                               if status.startswith("excluded:") and policies[route_id].tiers == ("light",))
    stretch_standard = frozenset(route_id for route_id, status in stretch_states.items()
                              if status == "standard_candidate")
    superseded = _applied_superseded(overrides, allowed, superseded_routes(allowed))
    providers = overrides.get("providers")
    request_status = ("not_requested_by_user" if model_hint != "auto"
                      and any(_route_label(e) == model_hint and "flagship" in tier_sets[e.route.id]
                              for e in allowed) else None)
    status = None
    if preference is not None and providers is not None and preference not in providers:
        status = "ignored:provider_not_allowed"
    elif preference is not None and not any(e.route.engine == preference for e in entries):
        status = "ignored:provider_not_configured"
    picked, decision = rank(role_tier, allowed, usage, overrides.get("tiers") or {},
                            excluded=frozenset(blocked) | default_denied | frozenset(superseded) | stretch_denied,
                            policy_excluded=frozenset(default_denied) | frozenset(superseded) | stretch_denied,
                            superseded=superseded,
                            stretch_standard=stretch_standard,
                            stretch_status=stretch_summary,
                            role=role, preference=None if status else preference,
                            model_hint=model_hint, model_reason=model_reason,
                            escalated=escalate, previous_engine=previous_engine,
                            all_entries=entries, providers=providers, affinity=affinity, models=models,
                            work_kind_override=work_kind_override)
    if decision is not None and status:
        decision.update(preference=preference, preference_status=status)
    if decision is not None and user_request is None and user_revocation is not None and model_hint != "auto":
        decision["user_model_request"] = dict(user_revocation, status="revoked_by_user")
    elif decision is not None and request_status is not None:
        decision["user_model_request"] = dict(user_request or {}, status=request_status)
    if decision is not None and profile_label is not None:
        decision["routing_profile"] = profile_label
    if picked is None:
        if bound_engine is not None:
            raise _policy_error("no_available_route",
                                f"No authorized, available, non-exhausted route on affinity engine {bound_engine} "
                                f"or any other configured engine can serve {kind} in an eligible tier.")
        if not entries:
            # Nothing engine-confirmed at all -- unrelated to 'providers'.
            return None
        if not allowed:
            if settings_allowed and eligible_route_ids is not None:
                raise _policy_error("role_route_not_authorized", "No settings-eligible route is within this task's profile/ceiling.")
            # 'providers' excluded every engine-confirmed route: fail closed
            # rather than silently falling back to the coordinator's route,
            # which could be on an excluded provider.
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="no_available_route",
                remediation=(f"'providers' in routing.json restricts routing to {sorted(overrides.get('providers') or ())}, "
                             "but no confirmed route is policy-eligible (deny/enabled/providers). Enable an exact identity, "
                             "or widen 'providers' in routing.json."),
                retry_eligible=False,
            )
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="no_available_route",
            remediation=("No configured route can take this work now: "
                         + "; ".join(_unavailable_reasons(allowed, blocked, usage)
                                     + ([", ".join(sorted(_route_label(e) for e in allowed
                                                          if e.route.id in default_denied))
                                         + " excluded from automatic routing (not "
                                         "selected under 'models' in routing.json); pin one or select it "
                                         "there to use it"] if default_denied else [])
                                     + (["light stretch models (" + ", ".join(sorted(
                                         _route_label(e) for e in allowed if e.route.id in stretch_denied))
                                         + f") are not used for this work ({stretch_summary.partition(':')[2]})"]
                                        if stretch_denied else []))
                         + ". Wait for a limit to reset, connect another account, or change routing.json."),
            retry_eligible=False,
        )
    policy = policies[picked.route.id]
    effort_target = EFFORT_BY_TIER[role_tier]
    effort_basis = f"role_tier:{role_tier}"
    if picked.route.id in stretch_ids and policy.effort_ladder:
        # Record the generic policy, independent of the configured model name.
        effort_target = stretch_effort(role_tier, escalate, policy.effort_ladder)
        effort_basis = f"stretch:{role_tier}" + (":escalated" if escalate else "")
    elif escalate and tier_for(role, False, complexity) == role_tier:
        effort_target = EFFORT_ORDER[min(EFFORT_ORDER.index(effort_target) + 1,
                                         EFFORT_ORDER.index("high"))]
        effort_basis = "escalation_step"
    # Efforts follow the tier the model was chosen for (one of its tiers; a
    # light stretch model taking standard work keeps its light-tier efforts).
    served = decision.get("tier") if decision.get("tier") in policy.tiers else policy.tier
    permitted = permitted_efforts(picked, served, policy)
    if policy.effort_ladder and picked.route.id in stretch_ids and effort_target not in permitted:
        raise _policy_error("model_effort_policy_unsupported", "Required effort_ladder step is not permitted.")
    effort = nearest(effort_target, permitted)
    decision["base_role_tier"] = base_role_tier
    decision["effort_basis"] = effort_basis
    return Choice(picked.connection_id, picked.route.model, effort, "auto", decision, picked.route.id)


_COMPLEXITY_SHIFT = {"low": 1, "normal": 0, "high": -1}


def tier_for(role: str, escalate: bool = False, complexity: str = "normal") -> str:
    """Return an economical role tier.

    Complexity moves ordinary work only between light, standard and deep.
    ``flagship`` is reserved for an explicit escalation after an earlier
    attempt falls short; explorer stays light even then. :func:`choose`
    also runs an exact flagship hint the user explicitly asked for in the
    task at the flagship tier (:func:`user_model_request`).
    """
    if role == "worker-explorer":
        return "light"
    index = TIERS.index(ROLE_TIERS.get(role, "standard")) + _COMPLEXITY_SHIFT.get(complexity, 0)
    if not escalate:
        index = max(TIERS.index("deep"), index)
    else:
        index -= 1
    return TIERS[min(len(TIERS) - 1, max(0, index))]


def pick(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None,
         affinity: Mapping[str, str | None] | None = None, models: Mapping | None = None):
    """The best entry for ``role_tier`` (see :func:`rank`), or ``None``."""
    return rank(role_tier, entries, usage, tier_overrides, excluded, role=role, preference=preference,
                affinity=affinity, models=models)[0]


# Generic rank ordering; historical decision keys remain readable by clients.
RANK_VERSION = 15
# The rank key's parts in order, named for the decision's ``decided_by``.
# ``quota`` moved from right after ``hint`` to right before ``used_percent``
# in rank version 6 (a late tie breaker, behind the usage-gap signal,
# retry diversification and affinity), then back to right after ``hint``
# in rank version 11: a candidate whose *known* usage reaches
# :data:`HOT_PERCENT` must rank behind every non-hot candidate -- known or
# unknown usage alike -- never merely a late, easily-ties-with-unknown tie
# breaker affinity and diversification could still outrank.
# Rank version 13 ranks by plan usage whenever every candidate's usage is
# known again, not only once one reaches HOT_PERCENT (version 10's gate).
# Rank version 14 confines plan usage to the owner's preferred models for the
# work kind while they are honoured, and applies none once they are used up.
# Rank version 15 (owner decision 2026-10-23) removes the usage-gap step
# (``usage``, 10-point bands): plan usage is only the last tie-breaker
# (``used_percent``) between candidates equal on every other key, and a
# coordinator-named eligible route is passed over for quota only when hot.
RANK_KEYS = ("preference", "purpose", "hint", "quota", "diversify", "affinity", "used_percent",
             "engine_order")


def rank(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None,
         policy_excluded: frozenset = frozenset(), superseded: Mapping[str, str] | None = None,
         model_hint: str = "auto", model_reason: str | None = None,
         escalated: bool = False, previous_engine: str | None = None,
         all_entries: Sequence | None = None, providers: Sequence[str] | None = None,
         affinity: Mapping[str, str | None] | None = None,
         stretch_standard: frozenset = frozenset(), stretch_status: str | None = None,
         models: Mapping | None = None, work_kind_override: str | None = None):
    """``(best entry, decision)`` for ``role_tier``, or ``(None, None)``.

    ``entries`` are in engine order; ``excluded`` route ids are skipped
    without moving any model's tier. The tier is the first of
    :data:`_FALLBACK_ORDER` with a usable candidate; a tier whose usable
    candidates are all on one engine is not widened with another engine's
    stronger models. Within the tier candidates are ordered by, first
    difference wins (:data:`RANK_KEYS`):

    1. ``preference`` (an engine id) when honoured: the engine has a usable
       candidate in this tier, and its usage is not both at or above
       :data:`HOT_PERCENT` and meaningfully worse (by more than
       :data:`USAGE_MARGIN`) than an available alternative's. Otherwise it
       is ignored and the reason recorded.
    2. ``purpose``: a model whose routing.json ``models`` policy lists the
       role's work kind under ``preferred_for`` (the owner's choice of model
       for that kind of work) ranks ahead of the tier's other candidates,
       unless its plan is hot and meaningfully worse on the same basis
       (``decision["purpose"]["status"]``). A model whose policy also has
       ``preferred_when`` (owner-written text the runtime cannot judge) is
       preferred only for a node whose model hint names that exact route,
       the coordinator's judgement that the condition holds; its
       ``preferred_for`` then narrows the kinds of work, and on escalation
       the condition no longer applies. Affinity-bound work instead uses the
       structured ``preferred_for`` on every attempt, independently of that
       text or hint, and keeps preferred routes in eligible fallback tiers;
       it never grants an unconfigured tier or bypasses exclusions;
    3. an honoured model hint, passed over for quota only when its known
       usage is hot while the tier has a non-hot candidate (see
       :data:`HINT_TOLERANCE`); plan usage never otherwise outranks it. A
       hint naming no preferred route loses to ``purpose`` and records
       ``ignored:owner_preferred``.
       Affinity-bound work still prefers the configured engine's preferred
       route even over an honoured cross-engine hint while that engine has
       an eligible candidate at all;
    4. the quota band (unknown is not hot): a candidate whose *known*
       usage is at or above :data:`HOT_PERCENT` ranks behind every
       candidate below that band, known or unknown usage alike --
       this outranks diversification and affinity below, so a hot known
       candidate never wins merely by being the preferred engine or more
       suitable for the work;
    5. on escalation, an engine other than the one that fell short;
    6. engine affinity (:func:`suitability`), the task's fit;
    7. lower exact used percent, whenever every compared usage is known:
       plan usage is only this last tie-breaker between candidates equal on
       every key above (rank version 15 removed the 10-point usage bands);
    8. engine order.

    Step 7 compares plan usage within ``decision["usage"]["scope"]``:
    only the ``purpose`` models while that preference is honoured (usage
    picks among the owner's chosen models, never against other models);
    ``none`` when the tier has such models but none is honoured (all hot
    and meaningfully worse, exhausted or excluded), so the replacement is
    chosen by the other criteria rather than by usage; ``all`` otherwise.

    Affinity-bound work is first restricted to its configured/default engine.
    If a usable preferred route serves an eligible fallback tier and is not
    hot and meaningfully worse, only preferred routes compete. Otherwise
    fallback remains within that engine. Only when that engine has no usable
    route serving any eligible fallback tier at all does ranking widen to the
    normal cross-engine candidate pool (still subject to every other rule
    below); while the affinity engine has one eligible candidate, no other
    engine is ever chosen. A neutral affinity yields no choice.

    How many attempts each engine received before never enters the rank:
    it says nothing about which engine fits the work.

    ``decision`` records the tier, every candidate of it in rank order with
    what ranked it, the preference and whether it was honoured, and
    ``decided_by``: the first key that separated the chosen candidate from
    the runner-up (``only_candidate`` when there was none).
    ``superseded`` maps an older route id to the newer route that replaces
    it (see :func:`superseded_routes`); those ids should also be in
    ``excluded`` and ``policy_excluded``. ``stretch_standard`` route ids (light
    stretch models, see :func:`stretch_status`) are also candidates of the
    standard tier; ``decision["stretch"]`` records ``stretch_status`` and, when a
    stretch model was ranked but not chosen, the key that ranked it behind.
    ``models`` is routing.json ``models`` (tiers and policy per engine)."""
    superseded = superseded or {}
    policies = model_policy.resolve_policies(entries, dict(tiers=tier_overrides or {}, models=models or {}))
    kind = work_kind(role, work_kind_override)
    affinity_bound = kind in AFFINITY_BOUND_WORK_KINDS
    bound_engine = kind_engine(kind, affinity) if affinity_bound else None
    if affinity_bound and bound_engine is None:
        return None, None

    def purpose(e) -> bool:
        policy = policies[e.route.id]
        if policy.preferred_when is None or affinity_bound:
            return kind is not None and kind in policy.preferred_for
        # A conditional preference: the coordinator judged this node against
        # the owner's ``preferred_when`` text and named this exact route.
        # Ordinary retries/fixes ignore hints and their free-text conditions.
        # Affinity-bound work above uses structured preferences instead.
        return (not escalated and model_hint == _route_label(e)
                and (not policy.preferred_for or kind in policy.preferred_for))

    def _preference_configured_for_tier(serving_tier: str) -> bool:
        """Whether routing.json's ``models`` policy names a model preferred
        for ``kind`` whose configured tier(s) include ``serving_tier`` --
        derived from the owner's settings alone, never from ``entries``, so
        a preferred identity missing from the live catalog, disconnected,
        or removed by eligibility filtering still counts as configured
        (mirrors :func:`purpose`'s own-preference semantics without
        requiring a catalog entry)."""
        if kind is None:
            return False
        configured_overrides = {"models": models or {}}
        for engine, config in (models or {}).items():
            if not isinstance(config, Mapping):
                continue
            names = set(config.get("tiers") or {}) | set(config.get("policy") or {})
            for name in names:
                model_policy_entry = model_policy.policy_for(engine, name, configured_overrides)
                if serving_tier not in model_policy_entry.tiers:
                    continue
                if model_policy_entry.preferred_when is None or affinity_bound:
                    if kind in model_policy_entry.preferred_for:
                        return True
                elif (not escalated and model_hint == f"{engine}/{name}"
                      and (not model_policy_entry.preferred_for or kind in model_policy_entry.preferred_for)):
                    return True
        return False

    usable_all = [e for e in entries if e.route.id not in excluded and (usage(e) or 0.0) < 100.0]
    order = {e.route.id: index for index, e in enumerate(entries)}

    def serves(e, tier):
        return tier in policies[e.route.id].tiers or (tier == "standard" and e.route.id in stretch_standard)

    cross_engine_fallback = False
    if affinity_bound:
        usable_bound = [e for e in usable_all if e.route.engine == bound_engine]
        bound_eligible = any(serves(e, tier) for e in usable_bound for tier in _FALLBACK_ORDER[role_tier])
        if bound_eligible:
            usable = usable_bound
        else:
            # No open, authorized, non-exhausted route on the affinity engine
            # serves any eligible fallback tier at all: widen to the normal
            # cross-engine candidate pool instead of refusing. While the
            # affinity engine has one eligible candidate this branch is never
            # reached, so another engine is never chosen over it.
            usable = usable_all
            cross_engine_fallback = True
        # Keep structured owner preferences across escalation and tier fallback.
        # Do not let a higher tier or retry diversification replace a preferred
        # route still eligible in the fallback order. Quota pressure may still
        # select another route, within the affinity engine while it has one
        # eligible, else within the cross-engine fallback pool above.
        eligible = [e for e in usable if any(serves(e, tier) for tier in _FALLBACK_ORDER[role_tier])]
        preferred = [e for e in eligible if purpose(e)]
        if preferred and not _meaningfully_worse(preferred, eligible, usage):
            usable = preferred
    else:
        usable = usable_all

    for tier in _FALLBACK_ORDER[role_tier]:
        candidates = [e for e in usable if serves(e, tier)]
        if not candidates:
            continue
        status = "none"
        if preference is not None:
            preferred = [e for e in candidates if e.route.engine == preference]
            if not preferred:
                offered = any(e.route.engine == preference and serves(e, tier) for e in entries)
                status = "ignored:provider_unavailable" if offered else "ignored:no_candidate_in_tier"
            elif _meaningfully_worse(preferred, candidates, usage):
                status = "ignored:provider_near_quota_limit"
            else:
                status = "honoured"
        honoured = preference if status == "honoured" else None
        hint_status = "auto"
        hinted = []
        if model_hint != "auto":
            hint_engine, slash, hint_model = model_hint.partition("/")
            hinted = [e for e in candidates if e.route.engine == hint_engine
                      and (not slash or e.route.model == hint_model)]
            offered = [e for e in (all_entries if all_entries is not None else entries)
                       if e.route.engine == hint_engine and (not slash or e.route.model == hint_model)]
            allowed_offered = [e for e in entries if e.route.engine == hint_engine
                               and (not slash or e.route.model == hint_model)]
            if escalated:
                hint_status = "ignored:escalated"
            elif providers is not None and hint_engine not in providers:
                hint_status = "ignored:provider_not_allowed"
            elif not offered or not allowed_offered:
                hint_status = "ignored:not_offered"
            elif slash and not any(serves(e, tier) for e in allowed_offered):
                hint_status = "ignored:tier_mismatch"
            elif slash and all(e.route.id in superseded for e in allowed_offered):
                hint_status = "ignored:superseded"
            elif slash and all(e.route.id in policy_excluded for e in allowed_offered):
                hint_status = "ignored:auto_excluded"
            elif not hinted:
                hint_status = ("ignored:no_candidate_in_tier" if not slash and
                               not any(e.route.engine == hint_engine and serves(e, tier)
                                       for e in entries) else "ignored:unavailable")
            elif _hot_with_cooler_alternative(hinted, candidates, usage):
                hint_status = "ignored:near_quota_limit"
            else:
                hint_status = "honoured"

        # The owner's preferred model for this kind of work (routing.json
        # ``models`` policy ``preferred_for``) ranks right after the runtime's
        # own provider preference: a coordinator hint, plan usage and retry
        # diversification never outrank the owner's explicit choice. Like a
        # hint it is passed over only when its plan is both hot and
        # meaningfully worse than an available alternative's.
        purposed = [e for e in candidates if purpose(e)]
        if not purposed:
            purpose_status = "none"
        elif _meaningfully_worse(purposed, candidates, usage):
            purpose_status = "ignored:near_quota_limit"
        else:
            purpose_status = "honoured"

        # Plan usage never reaches past the owner's choice of model for this
        # kind of work: while that choice is honoured it compares only the
        # chosen models with each other, and once they are used up (hot and
        # meaningfully worse, exhausted or unavailable) the replacement is
        # chosen by the remaining criteria, not by usage. Whether a
        # preference is configured at all comes from the owner's settings,
        # never from which candidates happen to survive the live catalog or
        # eligibility filtering -- a preferred model absent from the
        # connected catalog, disconnected, or outside this task's
        # authorization ceiling must still suppress usage ranking (``none``),
        # exactly as an exhausted-but-present one does; only the genuine
        # absence of any configured preference opens usage ranking to every
        # candidate (``all``).
        purpose_configured = _preference_configured_for_tier(tier)
        if purpose_status == "honoured":
            usage_scope, scope_name = purposed, "preferred"
        elif purpose_configured:
            usage_scope, scope_name = [], "none"
        else:
            usage_scope, scope_name = candidates, "all"
        scope_ids = {e.route.id for e in usage_scope}
        used_percent_compared = bool(usage_scope) and all(usage(e) is not None for e in usage_scope)
        usage_active = used_percent_compared
        least_used = min(usage(e) for e in usage_scope) if usage_active else None

        def used_percent(e):
            return usage(e) if usage_active and e.route.id in scope_ids else 0.0

        def key(e):
            # A candidate's own hot/known-quota flag ranks right after
            # preference/purpose/hint -- before diversification and
            # affinity -- so a known >=95% candidate always loses to any
            # non-hot one (known or unknown usage). Otherwise plan usage
            # only breaks the last tie (``used_percent``).
            return (e.route.engine != honoured,
                    not (purpose_status == "honoured" and purpose(e)),
                    not (hint_status == "honoured" and e in hinted),
                    _hot(usage(e)),
                    not (escalated and previous_engine and e.route.engine != previous_engine),
                    -suitability(role, e.route.engine, affinity, work_kind_override=work_kind_override),
                    used_percent(e), order[e.route.id])

        ranked = sorted(candidates, key=key)
        best = ranked[0]
        if len(ranked) == 1:
            decided_by = "only_candidate"
        else:
            first, second = key(best), key(ranked[1])
            decided_by = next(name for name, a, b in zip(RANK_KEYS, first, second) if a != b)
        best_stretch = next((e for e in ranked if policies[e.route.id].stretch), None)
        affinity_kind = kind
        reported_hint_status = hint_status
        if (hint_status == "honoured" and purpose_status == "honoured" and purpose(best)
                and not any(purpose(e) for e in hinted)):
            # The hint named only routes the owner does not prefer for this
            # kind of work; the owner's preference outranked it.
            reported_hint_status = "ignored:owner_preferred"
        decision = dict(
            role=role, role_tier=role_tier, tier=tier, basis="rank", rank_version=RANK_VERSION,
            preference=preference, preference_status=status,
            model_hint=model_hint, model_hint_status=reported_hint_status,
            model_hint_reason=model_reason, diversify_from=previous_engine if escalated else None,
            usage=dict(compared=used_percent_compared, active=usage_active,
                       status=("active" if usage_active else
                               "not_applied:owner_preferred_unavailable" if scope_name == "none" else "unavailable"),
                       scope=scope_name, threshold=HOT_PERCENT, role="tie_breaker", least_used_percent=least_used),
            affinity=dict(work_kind=affinity_kind, engine=affinity_engine(role, affinity, work_kind_override=work_kind_override),
                          source=("routing.json" if affinity_kind is not None and affinity
                                  and (affinity_kind in affinity or "*" in affinity
                                       or (affinity_kind in model_policy.IMPLEMENTATION_KINDS
                                           and "implementation" in affinity)) else "default")),
            used_percent_compared=used_percent_compared,
            purpose=dict(work_kind=kind, preferred=[_route_label(e) for e in ranked if purpose(e)],
                         status=purpose_status),
            candidates=[_candidate(e, usage, role, e.route.engine == honoured,
                                   affinity=affinity, work_kind_override=work_kind_override) for e in ranked],
            unavailable=sorted(_route_label(e) for e in entries
                               if serves(e, tier) and e.route.id not in policy_excluded
                               and (e.route.id in excluded or (usage(e) or 0.0) >= 100.0)),
            excluded_by_policy=sorted(_route_label(e) for e in entries
                                      if tier in policies[e.route.id].tiers and e.route.id in policy_excluded
                                      and e.route.id not in superseded),
            superseded={_route_label(e): superseded[e.route.id] for e in entries if e.route.id in superseded},
            decided_by=decided_by,
            stretch=dict(status=stretch_status, candidate=best_stretch is not None, chosen=best_stretch is best,
                      outranked_by=(None if best_stretch is None or best_stretch is best else
                                    next(name for name, a, b in zip(RANK_KEYS, key(best), key(best_stretch))
                                         if a != b))),
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        if affinity_bound:
            decision["affinity"]["required"] = True
            decision["affinity"]["cross_engine_fallback"] = cross_engine_fallback
        return best, decision
    return None, None


def _requested_routes(model_hint, overrides, entries, eligible_route_ids, blocked, usage):
    """Exact user choices bypass ranking, never hard authorization or availability."""
    from . import availability
    engine, _, model = model_hint.partition("/")

    def refuse(reason):
        raise _policy_error("user_requested_route_unavailable",
                            f"{model_hint}: {reason}; no substitution permitted. "
                            "Change the request or repair the configured route.")

    if _denied(overrides, engine, model):
        refuse("denied or disabled by owner policy")
    providers = overrides.get("providers")
    if providers is not None and engine not in providers:
        refuse("provider not allowed by owner policy")
    offered = [e for e in entries if _route_label(e) == model_hint]
    if not offered:
        refuse("not offered by a configured engine connection")
    if eligible_route_ids is not None:
        offered = [e for e in offered if e.route.id in eligible_route_ids]
        if not offered:
            refuse("outside the task's route/profile authorization ceiling")
    opened = [e for e in offered if e.route.id not in blocked]
    if not opened:
        refuse("unavailable: " + "; ".join(availability.describe(blocked[e.route.id]) for e in offered))
    usable = [e for e in opened if usage(e) is None or usage(e) < 100.0]
    if not usable:
        refuse("quota exhausted (100%)")
    return usable


def _hot(used: float | None) -> bool:
    return (used or 0.0) >= HOT_PERCENT


def _hot_with_cooler_alternative(group: Sequence, candidates: Sequence, usage) -> bool:
    """Whether a coordinator-named ``group`` (a hinted subset of
    ``candidates``) should be passed over for quota: its least-used member's
    usage is known and hot while some other candidate is not hot (known
    below :data:`HOT_PERCENT`, or unknown, which is never inferred hot) --
    the quota band's own rule. Usage below the hot band, or a gap of any
    size, never moves an eligible hinted route."""
    group_ids = {e.route.id for e in group}
    group_usages = [usage(e) for e in group]
    return (all(u is not None for u in group_usages) and _hot(min(group_usages))
            and any(not _hot(usage(e)) for e in candidates if e.route.id not in group_ids))


def _meaningfully_worse(group: Sequence, candidates: Sequence, usage) -> bool:
    """Whether ``group`` (a preferred/hinted subset of ``candidates``)
    should be passed over for quota: its least-used member's usage is known,
    at or above :data:`HOT_PERCENT`, and more than :data:`USAGE_MARGIN`
    points above the least-used usage of an available alternative (a
    candidate outside ``group``, also known). Unknown usage on either side,
    or no alternative, keeps quota neutral -- crossing the hot band alone
    is never enough."""
    group_ids = {e.route.id for e in group}
    group_usages = [usage(e) for e in group]
    alternative_usages = [usage(e) for e in candidates if e.route.id not in group_ids]
    return (bool(alternative_usages) and all(u is not None for u in group_usages)
            and all(u is not None for u in alternative_usages)
            and _hot(min(group_usages))
            and min(group_usages) - min(alternative_usages) >= USAGE_MARGIN)


def _route_label(entry) -> str:
    return f"{entry.route.engine}/{entry.route.model}"


def _candidate(entry, usage, role, preferred: bool, *,
               affinity: Mapping[str, str | None] | None = None, work_kind_override: str | None = None) -> dict:
    """One ranked candidate as the route audit records it. Rank version 15
    dropped ``usage_band``; older audits keep it."""
    return dict(route=_route_label(entry), connection_id=entry.connection_id, used_percent=usage(entry),
                hot=_hot(usage(entry)), quota_state="unknown" if usage(entry) is None else "known",
                suitability=suitability(role, entry.route.engine, affinity, work_kind_override=work_kind_override), preferred=preferred)


def model_tiers(entries: Sequence, tier_overrides: Mapping[str, str], models: Mapping | None = None) -> dict[str, str]:
    """Exact per-engine configured tiers; missing identities remain unplaced."""
    policies = model_policy.resolve_policies(entries, dict(tiers=tier_overrides or {}, models=models or {}))
    return {route_id: policy.tier for route_id, policy in policies.items()}


def _unavailable_reasons(entries: Sequence, blocked: Mapping, usage) -> list[str]:
    """Why each configured route is out, one line per record or account."""
    from . import availability

    reasons = set()
    for entry in entries:
        if entry.route.id in blocked:
            reasons.add(availability.describe(blocked[entry.route.id]))
        elif (usage(entry) or 0.0) >= 100.0:
            reasons.add(f"{entry.route.engine} on {entry.connection_id}: plan window at 100% (engine-reported)")
    return sorted(reasons)


def _order_key(used: float | None):
    """Quota-band order: an account at or above :data:`HOT_PERCENT` ranks
    last, so work goes to the plan with more room. Used to choose among a
    pin's open connections, where no task/model fit or usage-gap signal
    applies; the general rank (:func:`rank`) instead treats the band as a
    late tie breaker behind plan usage and affinity (see ``RANK_VERSION`` 6).
    Light work used to spend a hot account first (7b7aca22); that assumed
    the window resets soon, but ``used_percent`` is the most-used open
    window (often a weekly one) and the rank never sees its reset time, so
    it could push cheap work into an exhausting plan and fail it mid-run.
    Unknown quota is never hot; 100% never gets here."""
    return (_hot(used), used if _hot(used) else 0.0)


def permitted_efforts(entry, tier: str, policy: model_policy.RoutePolicy | None = None) -> tuple[str, ...]:
    """Observed efforts narrowed by generic economy and explicit policy; unsupported requirements refuse."""
    policy = policy or model_policy.policy_for(entry.route.engine, entry.route.model)
    floor, most, ladder = policy.effort_floor, policy.max_effort, policy.effort_ladder
    offered = {value for name, value in entry.route.supported_parameters if name == "thought_level"}
    if floor is not None and floor not in offered:
        raise _policy_error("model_effort_policy_unsupported", f"{_route_label(entry)} does not offer effort_floor {floor!r}.")
    if any(value not in offered for value in ladder):
        raise _policy_error("model_effort_policy_unsupported", f"{_route_label(entry)} does not offer the required effort_ladder.")
    if most is not None:
        ceiling = EFFORT_ORDER.index(most)
    elif tier == "flagship":
        ceiling = EFFORT_ORDER.index(_FLAGSHIP_CEILING)
    else:
        ceiling = len(EFFORT_ORDER)
    permitted = tuple(
        value for index, value in enumerate(EFFORT_ORDER)
        if value in offered and value not in _NEVER_EFFORTS and index <= ceiling
        and (tier == "light" or value in ladder or value not in _LIGHT_ONLY_EFFORTS)
    )
    if floor is not None:
        lowest = EFFORT_ORDER.index(floor)
        permitted = tuple(value for value in permitted if EFFORT_ORDER.index(value) >= lowest)
    if not permitted and (floor is not None or most is not None or ladder):
        raise _policy_error("model_effort_policy_unsupported", f"{_route_label(entry)} has no permitted effort under its policy.")
    return permitted


def efforts_in_any_tier(entry, policy: model_policy.RoutePolicy) -> tuple[str, ...]:
    """:func:`permitted_efforts` of every tier ``policy`` places the model
    in, weakest first (a pin or an explicit effort is not tied to a tier)."""
    permitted = set()
    for tier in policy.tiers or (policy.tier,):
        permitted.update(permitted_efforts(entry, tier, policy))
    return tuple(value for value in EFFORT_ORDER if value in permitted)


def nearest(effort: str, permitted: Sequence[str]) -> str | None:
    if effort in permitted:
        return effort
    ranked = [value for value in EFFORT_ORDER if value in permitted]
    if not ranked or effort not in EFFORT_ORDER:
        return ranked[-1] if ranked else None
    target = EFFORT_ORDER.index(effort)
    return min(ranked, key=lambda value: abs(EFFORT_ORDER.index(value) - target))


def allowed_efforts(engine: str, model: str | None, project_root=None, environ=None) -> tuple[str, ...] | None:
    """The efforts ``engine``/``model`` may run at, or ``None`` when the
    model is not in any catalog (nothing to judge by)."""
    from ..connections import registry as connection_registry

    overrides = load_overrides(project_root, environ)
    entries = _entries(overrides, connection_registry.read_registry(environ).connections, environ)
    policies = model_policy.resolve_policies(entries, overrides)
    match = next((e for e in entries if e.route.engine == engine and e.route.model == model), None)
    if match is None:
        return None
    return efforts_in_any_tier(match, policies[match.route.id])


def _entries(overrides, connections, environ) -> list:
    """Launchable observed entries before deny/enabled/provider filtering.

    Retain the raw confirmed catalogue so an empty policy-eligible set can
    never be mistaken for no observed catalogue and inherit the coordinator.
    """
    from ..acp.config_options import presented_entries, selectable_entries_from_snapshots

    return [
        entry for entry in presented_entries(selectable_entries_from_snapshots(connections, environ))
        if getattr(connections.get(entry.connection_id), "launch_command", None) is not None
    ]


def _allowed(overrides: Mapping, entries: Sequence) -> list:
    """``entries`` restricted to ``overrides["providers"]`` when set."""
    providers = overrides.get("providers")
    return entries if providers is None else [e for e in entries if e.route.engine in providers]


def _is_seed(entry) -> bool:
    return any(p.source == ProvenanceSource.SEED for p in entry.evidence.facts.values())


# --- overrides ---------------------------------------------------------------

def refuse_if_denied(engine: str, model: str | None, project_root=None, environ=None) -> None:
    if model and _denied(load_overrides(project_root, environ), engine, model):
        raise PortableError(
            category=PortableErrorCategory.AUTHORIZATION, code="route_denied",
            remediation=(f"{engine}/{model} is listed under 'deny' in routing.json."
                         if model_enabled(load_overrides(project_root, environ), engine, model) else
                         f"{engine}/{model} is not enabled: switch it on under Settings > Models "
                         f"(routing.json 'enabled')."),
            retry_eligible=False, field="model",
        )


def is_denied(engine: str, model: str, project_root=None, environ=None) -> bool:
    try:
        return _denied(load_overrides(project_root, environ), engine, model)
    except PortableError:
        return True


def _listed(values: Sequence[str], engine: str, model: str) -> bool:
    """Exact id, exact engine/id, or engine/*; no family/version matching."""
    return model in values or f"{engine}/{model}" in values or engine_wildcard(engine) in values


def _auto_exclusion(overrides: Mapping, entry) -> str | None:
    """Exact automatic membership and placement diagnostic."""
    engine, model = entry.route.engine, entry.route.model
    if not model_policy.is_selected(overrides, engine, model):
        return "not_selected"
    if not model_policy.policy_for(engine, model, overrides).tiers:
        return "policy_missing"
    return None


def _default_auto_denied(overrides: Mapping, entry) -> bool:
    """Whether automatic selection skips ``entry`` by policy (see
    :func:`_auto_exclusion`); ``deny`` still wins over any selection."""
    return _auto_exclusion(overrides, entry) is not None


def _applied_superseded(overrides: Mapping, entries: Sequence, superseded: Mapping[str, str]) -> dict[str, str]:
    """Historical audit compatibility; exact owner lists have no supersession."""
    return {}


def superseded_routes(entries: Sequence) -> dict[str, str]:
    """Historical audit compatibility; no replacement is inferred."""
    return {}


#: The ``deny`` token suffix that names every model of one engine
#: (``codex-acp/*``). Deny-only: ``auto_allow`` never takes a wildcard.
ENGINE_WILDCARD = "*"


def engine_wildcard(engine: str) -> str:
    """The ``deny`` token that disables every route of ``engine``."""
    return f"{engine}/{ENGINE_WILDCARD}"


def enabled_models(overrides: Mapping, engine: str) -> tuple[str, ...]:
    """Exact enabled identities; an omitted engine is disabled."""
    enabled = overrides.get("enabled") or {}
    return tuple(enabled.get(engine) or ())


def model_enabled(overrides: Mapping, engine: str, model: str) -> bool:
    return model in enabled_models(overrides, engine)


def _denied(overrides: Mapping, engine: str, model: str) -> bool:
    deny = overrides.get("deny") or ()
    return (engine_wildcard(engine) in deny or _listed(deny, engine, model)
            or not model_enabled(overrides, engine, model))


def _pin_for(overrides: Mapping, role: str):
    from ..portable.roles import WORKER_ALIASES

    pins = overrides.get("pin") or {}
    aliases = {short for short, full in WORKER_ALIASES.items() if full == role}
    raw = next((pins[key] for key in (role, *aliases, "*") if key in pins), None)
    if raw is None:
        return None
    route, _, effort = raw.partition(":")
    engine, _, model = route.partition("/")
    return engine, model, effort or None


def _policy_error(code: str, remediation: str) -> PortableError:
    return PortableError(category=PortableErrorCategory.VALIDATION, code=code,
                         remediation=remediation, retry_eligible=False, field="model_policy_version")


def require_model_policy(data: Mapping) -> None:
    version = data.get("model_policy_version")
    if version is None:
        raise _policy_error("model_policy_migration_required",
                            "Explicitly migrate routing.json to model_policy_version=2 with exact enabled/auto identities and per-engine tiers.")
    if type(version) is not int or version != 2:
        raise _policy_error("model_policy_version_unsupported", "Only model_policy_version=2 is supported.")


def load_overrides(project_root=None, environ: Mapping[str, str] | None = None) -> dict:
    """First existing validated owner policy; absence/legacy/malformed fails closed."""
    for path in _candidate_paths(project_root, environ):
        if not path.is_file():
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            _validate(data, require_policy=False)
            require_model_policy(data)
        except (OSError, ValueError) as error:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_routing_overrides",
                remediation=f"Repair {path}: {error}", retry_eligible=False,
            ) from error
        return model_policy.normalize_work_kinds(data)
    require_model_policy({})
    return {}


def _validate(data, *, require_policy=True) -> None:
    from ..connections.model import SUPPORTED_ENGINES

    if not isinstance(data, dict):
        raise ValueError("must contain a JSON object")
    if require_policy:
        require_model_policy(data)
    pins, deny, tiers = data.get("pin", {}), data.get("deny", []), data.get("tiers", {})
    auto_allow = data.get("auto_allow", [])
    if not isinstance(pins, dict) or not all(isinstance(v, str) and "/" in v for v in pins.values()):
        raise ValueError("'pin' must map a role to \"engine/model\" or \"engine/model:effort\"")
    if not isinstance(deny, list) or not all(isinstance(v, str) and v for v in deny):
        raise ValueError("'deny' must be a list of \"model\" or \"engine/model\"")
    if not isinstance(auto_allow, list) or not all(isinstance(v, str) and v for v in auto_allow):
        raise ValueError("'auto_allow' must be a list of \"model\" or \"engine/model\"")
    if not isinstance(tiers, dict) or not all(v in TIERS for v in tiers.values()):
        raise ValueError(f"'tiers' must map a model to one of {', '.join(TIERS)}")
    if "providers" in data:
        providers = data["providers"]
        if (not isinstance(providers, list) or not providers
                or not all(isinstance(v, str) and v in SUPPORTED_ENGINES for v in providers)):
            raise ValueError(
                f"'providers' must be a nonempty list drawn from {', '.join(SUPPORTED_ENGINES)}")
    if "engine_affinity" in data:
        affinity = data["engine_affinity"]
        if not isinstance(affinity, dict):
            raise ValueError("'engine_affinity' must map a work kind (or \"*\") to an engine id or null")
        kinds = (*WORK_KIND_ENGINES, "implementation", "*")
        for kind, engine in affinity.items():
            if kind not in kinds:
                raise ValueError(f"'engine_affinity' names unknown work kind {kind!r}; "
                                 f"use one of {', '.join(kinds)}")
            if engine is not None and engine not in SUPPORTED_ENGINES:
                raise ValueError(f"'engine_affinity' maps {kind!r} to unknown engine {engine!r}; "
                                 f"use one of {', '.join(SUPPORTED_ENGINES)} or null")
    if data.get("model_policy_version") == 2 and data.get("tiers"):
        raise ValueError("legacy top-level tiers require explicit per-engine migration")
    if "quota_windows" in data:
        from ..connections import quota
        quota.validate_quota_windows(data["quota_windows"])
    if "enabled" in data:
        enabled = data["enabled"]
        if not isinstance(enabled, dict) or not all(
                engine in SUPPORTED_ENGINES and isinstance(models, list)
                and len(models) == len(set(map(str, models)))
                and all(model_policy._model_id(v) for v in models) for engine, models in enabled.items()):
            raise ValueError("'enabled' must map an engine id to a list of exact model ids")
    if "models" in data:
        problems = model_policy.validate_models(
            data["models"], engines=SUPPORTED_ENGINES, work_kinds=tuple(WORK_KIND_ENGINES),
            reserved_word=_reserved_request_word)
        if problems:
            raise ValueError("; ".join(f"'{where}' {message}" for where, _code, message in problems[:5]))

    if data.get("model_policy_version") == 2:
        labels = {}
        for engine, config in (data.get("models") or {}).items():
            tiers = config.get("tiers") or {}
            for model in config.get("auto") or ():
                if not model_policy.tier_list(tiers.get(model)):
                    raise _policy_error("policy_missing", f"models.{engine}.tiers.{model} needs nonempty explicit tiers.")
            for model, policy in (config.get("policy") or {}).items():
                for label in policy.get("request_names") or ():
                    identity = f"{engine}/{model}"
                    if label in labels and labels[label] != identity:
                        raise _policy_error("model_request_label_ambiguous", f"Request label {label!r} names more than one exact identity.")
                    labels[label] = identity


def validate_routing(data: Mapping) -> None:
    """Validate v2 owner settings without consulting catalogs or template data.

    Named PortableErrors: model_policy_migration_required,
    model_policy_version_unsupported, policy_missing,
    model_request_label_ambiguous. Structural errors remain ValueError.
    Enabled engines and automatic lists may be empty/omitted (disabled).
    Every automatic model needs nonempty per-engine tiers, even when dormant.
    """
    _validate(data)


def _candidate_paths(project_root, environ) -> list[Path]:
    environ = os.environ if environ is None else environ
    home = Path(environ.get("HOME") or str(Path.home()))
    paths = [Path(project_root) / ".cortex" / "routing.json"] if project_root else []
    if environ.get(ENV_VAR):
        paths.append(Path(environ[ENV_VAR]))
    xdg = environ.get("XDG_CONFIG_HOME") or ""
    paths.append((Path(xdg) if xdg.startswith("/") else home / ".config") / "cortex" / "routing.json")
    return paths


# --- source-bound explicit model requests ------------------------------------

_RU_ENDING = r"(?:а|ы|е|у|ой|ою|и|ом|ам|ами|ах|ов)?"
# Words that request the family by themselves: verbs whose object is the model
# and nouns of requirement. Deliberately narrow: prepositions ("on", "via",
# "through", "на", "через"; see WORK_VERBS), "with"/"с", "by", "about", "let"
# and wishing or modal verbs ("want", "should", "хочу", "нужно") also
# introduce remarks, comparisons, authorship and questions, so a request
# phrased with them alone ("I want the named model") is not recognised -- the node then
# stays on its ordinary tier and the receipt says ``not_requested_by_user``.
# Russian verbs are listed only in the forms that ask (imperative, infinitive,
# first person plural), never a past tense, participle or verbal noun.
REQUEST_WORDS = re.compile(
    r"use|run|execute|launch|route|delegate|assign|switch|prefer|pick|choose|select"
    r"|requires?|requirement|request"
    r"|использ(?:уй|уйте|овать|уем)|задейств(?:уй|уйте|овать|уем)"
    r"|запуст(?:и|ите|ить|им)|запуска(?:й|йте|ть|ем)|прогон(?:и|ите|яй|яйте)|прогнать"
    r"|выбер(?:и|ите|ем)|выбрать|выбира(?:й|йте|ть)|назнач(?:ь|ьте|ить|им|ай|айте)"
    r"|отда(?:й|йте|ть|дим|вай|вайте)|переключ(?:и|ите|ись|итесь|ить|иться|им|имся|ай|айся)"
    r"|постав(?:ь|ьте|ить|им)|возьм(?:и|ите|ём|ем)|взять|бери|берите|берём|берем|пусть"
    r"|переда(?:й|йте|ть|дим|вай|вайте)|поруч(?:и|ите|ить|им|ай|айте)"
    r"|треб(?:ование|ования|ованию|ованием|овании|ую|уем|ует|уется|уются|уй|уйте|овать)",
    re.IGNORECASE,
)
# Verbs of the work itself. They request the family only through a
# preposition of means (MEANS_WORDS) directly before it ("implement it on
# the named model", "сделай на модели"): with the family as their object they name what
# the work is about ("review the named model's output", "проверь модель"), and a
# preposition with no such verb is a remark ("it failed on the named model", "посмотри
# на модель"). Russian forms as in REQUEST_WORDS.
WORK_VERBS = re.compile(
    r"implement|build|fix|write|test|review|audit|perform|handle|redo|rerun|retry|debug|refactor"
    r"|investigate|analy[sz]e|do"
    r"|реализ(?:уй|уйте|овать|уем)|(?:с|пере|до)?дела(?:й|йте|ть)|(?:с|пере|до)делаем"
    r"|выполн(?:и|ите|ить|им|яй|яйте|ять)|напиш(?:и|ите|ем)|написать"
    r"|провер(?:ь|ьте|ить|им|яй|яйте|ять)|исправ(?:ь|ьте|ить|им|ляй|ляйте|лять)"
    r"|почин(?:и|ите|ить|им)|(?:про)?тестир(?:уй|уйте|овать|уем)"
    r"|собер(?:и|ите|ём|ем)|собрать|провед(?:и|ите|ём|ем)|провести|отревью(?:й|йте|ить)",
    re.IGNORECASE,
)
# Past tenses of the verbs above. After "чтобы"/"чтоб"/"что бы" in the
# same clause ("хочу, чтобы ты сделал аудит через <model>") a past tense is the
# Russian subjunctive, which asks; anywhere else it reports ("сделал на <model>").
_RU_PAST_SUFFIX = r"(?:л|ла|ло|ли|лся|лась|лось|лись)"
_RU_SUBJUNCTIVE_REQUEST = re.compile(
    r"(?:использова|задействова|запусти|запуска|прогна|прогоня|выбра|выбира|назначи|назнача"
    r"|отда|отдава|переключи|переключа|постави|взя|бра|переда|передава|поручи|поруча)" + _RU_PAST_SUFFIX,
    re.IGNORECASE,
)
_RU_SUBJUNCTIVE_WORK = re.compile(
    r"(?:(?:с|пере|до)?дела|реализова|выполни|выполня|написа|провери|проверя|исправи|исправля"
    r"|почини|(?:про)?тестирова|собра|провё|прове|отревьюи)" + _RU_PAST_SUFFIX,
    re.IGNORECASE,
)
_RU_SUBJUNCTIVE_MARKERS = frozenset({"чтобы", "чтоб"})
MEANS_WORDS = re.compile(r"on|via|through|using|на|через|посредством|силами|помощью", re.IGNORECASE)
# At most one of these may stand between the preposition and the family
# ("on the the named model route", "на модели модель").
MEANS_FILLERS = frozenset({"the", "a", "an", "model", "модели", "моделью"})
# An English request or work verb is in a request form when it opens its
# clause or directly follows one of these words (or an "'ll"/"'d"
# contraction). Anything else before it -- an article, a possessive, a noun,
# an auxiliary -- makes it a noun or a report: "the last run on the named model", "the
# tests run on the named model", "was run on the named model", "I use the named model daily".
_REQUEST_LEAD = frozenset({
    "please", "pls", "plz", "kindly", "just", "simply", "only", "always", "also", "then",
    "and", "now", "so", "ok", "okay", "yes", "no", "to", "can", "could", "would", "will",
    "should", "must", "may", "let's", "lets", "better",
})
# "could you run it on the named model": "you" leads only after one of these.
_ASKING_MODALS = frozenset({"can", "could", "would", "will", "please"})
# "I prefer the named model": a first-person subject leads only a verb of choice.
_CHOOSERS = frozenset({"i", "we"})
_CHOICE_WORDS = frozenset({"prefer", "pick", "choose", "select", "request", "require"})
# "to" after one of these reports an attempt or a habit, not a request
# ("we tried to run it on the named model").
_REPORTED_BEFORE_TO = frozenset({
    "tried", "wanted", "used", "failed", "managed", "attempted", "meant", "supposed",
    "was", "were", "had", "did", "went", "planned", "started", "began", "continued", "seemed",
})
# Nouns and statements of requirement need no lead ("the task requires the named model").
_UNGUARDED = frozenset({"requirement", "requires"})
_CYRILLIC = re.compile(r"[а-яё]")
# A Russian past tense ("пытались", "хотел") right before a verb makes it a report.
_RU_PAST = re.compile(r"[а-яё]+(?:л|ла|ло|ли|лся|лась|лось|лись)")
# One of these within NEGATION_WINDOW words before the family (same sentence)
# makes the mention a refusal, a comparison, a substitution or a report rather
# than a request. Deliberately broad, and read from a wider window than
# REQUEST_WORDS: a missed refusal would run a flagship model, a missed request
# only leaves the node on its ordinary tier. "no"/"нет" are not here: "No,
# use the named model" is a request, and a bare "нет модели" has no request word anyway.
NEGATION_WORDS = re.compile(
    r"not|never|without|avoid|except|cannot|unable"
    r"|(?:do|does|did|is|are|was|were|has|have|had|ca|could|would|should|must|wo|shall|need)n'?t"
    r"|instead|rather|than|versus|vs|over|compar\w*|based|depend\w*"
    r"|не|без|никогда|кроме|нельзя|запрещ\w*|вместо|чем|сравн\w*|против|основан\w*|исходя",
    re.IGNORECASE,
)
_WORD = re.compile(r"[\w']+", re.UNICODE)
_SENTENCE_END = re.compile(r"[.!?;\n]")
# A request verb's lead is read within its own clause ("No, use the named model").
_CLAUSE_BREAK = re.compile(r"[,:()\"«»“”„—–]")
# An opening quote attached to a request label is not a preceding word.
_ATTACHED = re.compile(r"\S*\Z")
# A closing quote attached to a mention is not a clause break after it.
_ATTACHED_AFTER = re.compile(r"\A[\"'`»”]+")
REQUEST_WINDOW = 4
NEGATION_WINDOW = 5
# Refusal, unavailability and cost words. One anywhere in a mention's own
# clause segment, before or after it, refuses that mention ("X must never be
# used", "X is too expensive", "X слишком дорогая", "Opus не нужен"): a
# missed refusal would run a model the user refused. Quota and limit words are
# deliberately absent: "Сделай аудит на Opus в пределах лимита" still asks for
# Opus, and a chosen option that only reports a quota ("Opus исчерпал квоту")
# is rejected by _option_selects as a mention, without revoking anything.
_REFUSAL_WORDS = re.compile(
    r"not|never|no|without|avoid|cannot|unable|unavailable|expensive|costly|overpriced|pricey"
    r"|forbidden|banned|prohibited|disallowed"
    r"|(?:do|does|did|is|are|was|were|has|have|had|ca|could|would|should|must|wo|shall|need)n'?t"
    r"|не|нет|нельзя|никогда|без|запрещ\w*|недоступ\w*|дорог\w*",
    re.IGNORECASE,
)
# Substitution and stopping words refuse a mention they precede in its clause
# segment ("use Y instead of X", "stop using X", "замени X"). After a mention
# they name what it replaces ("use X instead of Y"), so they do not refuse it.
_SUBSTITUTION_WORDS = re.compile(
    r"instead|rather|replac\w*|substitut\w*|stop|quit|except"
    r"|вместо|замен\w*|хватит|прекрат\w*|перестан\w*|отказ\w*|кроме",
    re.IGNORECASE,
)
# Words that always open a new clause segment: contrast and subordination
# ("use X because Y is too expensive", "используй X, а Y не трогай").
_SEGMENT_BREAKS = frozenset({
    "but", "then", "however", "yet", "because", "since", "if", "when", "while", "although",
    "though", "unless", "until", "so",
    "а", "но", "затем", "потом", "однако", "зато", "потому", "поскольку", "если", "когда",
    "пока", "хотя", "чтобы", "чтоб",
})
# Coordinators open a new segment only before a new predicate ("... и
# используй X", "use X and never Y"); "X and Y are too expensive" stays one.
_COORDINATORS = frozenset({"and", "or", "plus", "и", "или"})
_REQUEST_CLAUSE_BREAK = re.compile(r"[,()\"«»“”„—–]")
_PREDICATE_LEADS = _REQUEST_LEAD | {"просто", "только", "также", "тоже", "ещё", "еще"}


def family_of(model_hint: str) -> str | None:
    """Historical API name: now the full exact engine/model identity."""
    engine, slash, model = model_hint.partition("/")
    return model_hint if engine and slash and model else None


def family_pattern(family: str, names: Sequence[str] | None = None) -> re.Pattern:
    """Match an exact identity or owner-configured labels, never guessed words."""
    identity = rf"(?-i:{re.escape(family)})"
    # IDs are opaque: punctuation, including a terminal dot, can be ID data.
    # Unquoted identities must occupy the whole whitespace-delimited token.
    patterns = [rf"(?<!\S){identity}(?!\S)"]
    # Explicit paired quotes disambiguate surrounding prose punctuation. Both
    # ends of the quoted token must be isolated; quoted prefixes/variants do
    # not authorize the shorter identity. Keep quotes out of the retained match.
    # Paired parentheses are the same: "Claude Opus (claude-agent-acp/opus[1m])"
    # names the identity; "(engine/unit.v2)" still never names "engine/unit".
    for opening, closing in (("\"", "\""), ("'", "'"), ("`", "`"), ("«", "»"), ("“", "”"), ("(", ")")):
        left, right = re.escape(opening), re.escape(closing)
        patterns.append(rf"(?<={left})(?<!\S{left}){identity}(?={right}[.,!?;:]?(?!\S))")
    for name in names or ():
        ending = _RU_ENDING if _CYRILLIC.search(name) else "s?"
        patterns.append(rf"(?<!\w){re.escape(name)}{ending}(?!\w)")
    return re.compile("|".join(patterns), re.IGNORECASE)


def _clause_words(sentence: str) -> list[tuple[str, tuple[str, ...]]]:
    """Each casefolded word of ``sentence`` with the words before it in its
    own clause."""
    result = []
    for clause in _CLAUSE_BREAK.split(sentence):
        words = [w for w in (token.casefold().strip("'") for token in _WORD.findall(clause)) if w]
        result.extend((word, tuple(words[:index])) for index, word in enumerate(words))
    return result


def _subjunctive_lead(lead: tuple[str, ...]) -> bool:
    """Whether ``lead`` opens a Russian subjunctive ("чтобы ты", "что бы вы")
    with no negation after the marker."""
    for index in range(len(lead) - 1, -1, -1):
        word = lead[index]
        if word in _RU_SUBJUNCTIVE_MARKERS or (word == "бы" and index and lead[index - 1] == "что"):
            return not any(NEGATION_WORDS.fullmatch(after) for after in lead[index + 1:])
    return False


def _request_form(pattern: re.Pattern, word: str, lead: tuple[str, ...],
                  subjunctive: bool = False) -> bool:
    """Whether ``word``, after ``lead`` in its clause, is a ``pattern`` word
    used as a request -- not a noun ("the last run"), a statement ("the tests
    run", "I use") or a report ("tried to run", "пытались запустить").
    With ``subjunctive``, a Russian past tense of such a verb after
    "чтобы"/"что бы" in its clause also asks ("чтобы ты сделал")."""
    if subjunctive:
        past = _RU_SUBJUNCTIVE_REQUEST if pattern is REQUEST_WORDS else _RU_SUBJUNCTIVE_WORK
        if past.fullmatch(word) and _subjunctive_lead(lead):
            return True
    if not pattern.fullmatch(word):
        return False
    previous = lead[-1] if lead else None
    if _CYRILLIC.search(word):
        return previous is None or not _RU_PAST.fullmatch(previous)
    if word in _UNGUARDED or previous is None:
        return True
    if previous == "to":
        return len(lead) < 2 or lead[-2] not in _REPORTED_BEFORE_TO
    if previous in _REQUEST_LEAD or previous.endswith(("'ll", "'d")):
        return True
    if previous in _CHOOSERS:
        return word in _CHOICE_WORDS
    if previous == "you":
        return len(lead) >= 2 and lead[-2] in _ASKING_MODALS
    return False


def _governed_means(window: list[tuple[str, tuple[str, ...]]], subjunctive: bool = False) -> bool:
    """A :data:`MEANS_WORDS` preposition directly before the family (at most
    one :data:`MEANS_FILLERS` word between) governed by a :data:`WORK_VERBS`
    verb in request form earlier in ``window``."""
    position = len(window) - 1
    if position >= 0 and window[position][0] in MEANS_FILLERS:
        position -= 1
    if position < 0 or not MEANS_WORDS.fullmatch(window[position][0]):
        return False
    return any(_request_form(WORK_VERBS, word, lead, subjunctive) for word, lead in window[:position])


def _plain_words(text: str) -> list[str]:
    return [w for w in (token.casefold().strip("'") for token in _WORD.findall(text)) if w]


def _segment_break(words: Sequence[str], index: int) -> bool:
    """Whether ``words[index]`` opens a new clause segment."""
    word = words[index]
    if word in _SEGMENT_BREAKS:
        return True
    if word not in _COORDINATORS or index + 1 >= len(words):
        return False
    following = words[index + 1]
    return (following in _PREDICATE_LEADS or bool(_REFUSAL_WORDS.fullmatch(following))
            or any(p.fullmatch(following) for p in (REQUEST_WORDS, WORK_VERBS, NEGATION_WORDS)))


def _mention_context(text: str, match: re.Match) -> dict:
    """The words around one mention: ``sentence`` (every word before it in
    its sentence, with clause leads), ``clause`` (those of its own clause),
    ``before``/``after`` (its clause segment on each side) and ``alone``
    (nothing but the mention in its clause, e.g. a listing item)."""
    before_text = _ATTACHED.sub("", text[: match.start()]).replace("’", "'")
    before_sentence = _SENTENCE_END.split(before_text)[-1]
    before_clause = _CLAUSE_BREAK.split(before_sentence)[-1]
    after_text = _ATTACHED_AFTER.sub("", text[match.end():].replace("’", "'"))
    after_clause = _CLAUSE_BREAK.split(_SENTENCE_END.split(after_text)[0])[0]
    lead, follow = _plain_words(before_clause), _plain_words(after_clause)
    start = max((i + 1 for i in range(len(lead)) if _segment_break(lead, i)), default=0)
    end = next((i for i in range(len(follow)) if _segment_break(follow, i)), len(follow))
    sentence = _clause_words(before_sentence)
    # A colon introduces what its verb governs ("Use: <identity>"), so the
    # request clause spans it; every other clause break ends it.
    governed = len(_plain_words(_REQUEST_CLAUSE_BREAK.split(before_sentence)[-1]))
    return dict(sentence=sentence, clause=sentence[len(sentence) - governed:] if governed else [],
                before=lead[start:], after=follow[:end], alone=not lead and not follow,
                sentence_words=[word for word, _lead in sentence])


def _refused_mention(context: Mapping, *, strict: bool = False) -> bool:
    """Whether a mention is refused, substituted or compared away.

    Clause-wide: a :data:`_REFUSAL_WORDS` word anywhere in its clause segment
    on either side, or a :data:`_SUBSTITUTION_WORDS` word before it in that
    segment; a negation before "чтобы"/"что бы" in its sentence ("не хочу,
    чтобы ..."). Windowed: a refusal or substitution word (any
    :data:`NEGATION_WORDS` word unless ``strict``) within
    :data:`NEGATION_WINDOW` words before it in its sentence. ``strict``
    leaves comparisons and remarks ("compare X with Y", "based on X") out:
    they decline a grant but do not revoke an earlier one."""
    if any(_REFUSAL_WORDS.fullmatch(word) or _SUBSTITUTION_WORDS.fullmatch(word) for word in context["before"]):
        return True
    if any(_REFUSAL_WORDS.fullmatch(word) for word in context["after"]):
        return True
    words = context["sentence_words"]
    markers = [i for i, word in enumerate(words) if word in _RU_SUBJUNCTIVE_MARKERS
               or (word == "бы" and i and words[i - 1] == "что")]
    if markers and any(NEGATION_WORDS.fullmatch(word) for word in words[:markers[-1]]):
        return True
    window = words[-NEGATION_WINDOW:]
    if strict:
        return any(_REFUSAL_WORDS.fullmatch(word) or _SUBSTITUTION_WORDS.fullmatch(word) for word in window)
    return any(NEGATION_WORDS.fullmatch(word) for word in window)


def refuses_family(text: str, family: str, names: Sequence[str] | None = None,
                   *, pattern: re.Pattern | None = None) -> str | None:
    """The first mention of ``family`` in ``text`` that refuses it (see
    :func:`_refused_mention` with ``strict``), or ``None``. A newer user
    message with such a mention revokes an earlier grant."""
    for match in (pattern if pattern is not None else family_pattern(family, names)).finditer(text):
        if _refused_mention(_mention_context(text, match), strict=True):
            return match.group(0)
    return None


# "Responsible for" a role or stage, and those roles and stages. A past
# form ("отвечал") is a request only after "чтобы"/"пусть"; alone it reports.
_RESPONSIBILITY_SUBJUNCTIVE = frozenset({"чтобы", "пусть", "should", "must", "will", "let"})
_ROLE_OR_STAGE = re.compile(
    r"(?:audit|аудит|review|ревью|code|код|design|дизайн|frontend|фронт|backend|бэк|test|тест|qa|plan|план|"
    r"docs?|документ|security|безопасност|accessibility|доступност|architect|архитект|debug|отладк|"
    r"verification|проверк|implementation|реализац|refactor|рефактор|research|исследован)\w*", re.I)


def assigns_family(text: str, pattern: re.Pattern) -> str | None:
    """A model explicitly made responsible for a role object, not a remark.

    Match the construction next to this mention, never responsibility assigned
    to a different model elsewhere in the clause. Past forms require a request
    mood ("чтобы за audit отвечал opus"); generic "does/handles" assign nothing.
    """
    role = _ROLE_OR_STAGE.pattern
    object_role = rf"(?:\w+\s+){{0,2}}{role}\b"
    label_lead = r"(?:claude\s*)?"
    historical = {"was", "were", "previously", "last", "раньше", "ранее", "вчера", "прошлый"}
    for match in pattern.finditer(text):
        context = _mention_context(text, match)
        if _refused_mention(context):
            continue
        before = _plain_words(_REQUEST_CLAUSE_BREAK.split(
            _SENTENCE_END.split(text[:match.start()])[-1])[-1])
        after = _plain_words(_REQUEST_CLAUSE_BREAK.split(
            _SENTENCE_END.split(text[match.end():])[0])[0])
        if any(word in historical for word in [*before, *after]):
            continue
        lead, follow = " ".join(before), " ".join(after)
        if (re.search(rf"\bза\s+{object_role}\s+отвеча(?:ет|ют|ть|й|йте)\s*{label_lead}$", lead)
                or re.match(rf"отвеча(?:ет|ют|ть|й|йте)\s+за\s+{object_role}", follow)
                or re.match(rf"(?:(?:is|be)\s+)?(?:responsible\s+for|in\s+charge\s+of)\s+{object_role}", follow)):
            return match.group(0)
        # "чтобы" is also written "что бы".
        subjunctive = (any(word in _RESPONSIBILITY_SUBJUNCTIVE for word in before)
                       or any(a == "что" and b == "бы" for a, b in zip(before, before[1:])))
        if subjunctive and (
                re.search(rf"\bза\s+{object_role}\s+отвечал[аи]?\s*{label_lead}$", lead)
                or re.match(rf"отвечал[аи]?\s+за\s+{object_role}", follow)):
            return match.group(0)
        if ((re.search(rf"\blet\s*{label_lead}$", lead)
             and re.match(rf"(?:handle|perform)\s+{object_role}", follow))
                or re.match(rf"(?:should|must)\s+(?:handle|perform)\s+{object_role}", follow)):
            return match.group(0)
    return None


# A restrictive selection: "только"/"only" right before the mention
# ("Выполнить аудит только Claude Opus", "do the review only on Opus", "только
# Opus"), or English "only" right after it ("audit with Opus only").
_EXCLUSIVE_WORDS = frozenset({"only", "exclusively", "solely", "только", "исключительно", "лишь"})
_POSTPOSITIVE_EXCLUSIVE = frozenset({"only", "exclusively", "solely"})
# The means words of MEANS_WORDS plus "with"/"с": after a restrictive marker
# governed by a request or work verb they no longer read as a comparison.
_EXCLUSIVE_MEANS = re.compile(r"on|via|through|using|with|на|через|посредством|силами|помощью|с|со",
                             re.IGNORECASE)
# Vendor words people put before a model name ("only Claude Opus").
_VENDOR_WORDS = frozenset({"claude", "клод", "anthropic", "openai", "codex", "gpt"})
_PURPOSE_PREPOSITIONS = frozenset({"for", "для"})
_EXCLUSIVE_TAIL = frozenset({"please", "pls", "пожалуйста"})
_EXCLUSIVE_LEAD_WINDOW = 6
# A Russian verb form that a role stem can take: a past tense (_RU_PAST), a
# personal present or future, an infinitive, an imperative, a verbal adverb or
# a participle ("тестировал", "тестирую", "кодит", "рефакторю", "ревьюить",
# "планируй", "тестируя", "рефакторя", "протестировав", "тестирующий",
# "тестировавший", "тестированный", "тестируемый", and the short passive
# "тестирован", "тестировано"). The role nouns that end like one ("аудит",
# "аудите", "тестированием", "реализация", "безопасность", "план", "дизайн")
# are not verbs; a genitive plural in -ов ("тестов") is not a verbal adverb.
_RU_VERB_FORM = re.compile(
    r"[а-яё]+(?:(?:ую|юю|[рл]ю|ешь|ёшь|ишь|ет|ёт|ит|ем|ём|им|ете|ёте|ите|ут|ют|ат|ят"
    r"|ть|уй|уйте|юй|юйте|уя|юя|[бвгджзклмнпрстфхцчшщ]я"
    r"|(?:ущ|ющ|ащ|ящ|вш|нн|ем|им)(?:ий|ый|ой|ая|яя|ое|ее|ие|ые|ого|его|ому|ему|ым|им|ом|ем"
    r"|ую|юю|ых|их|ыми|ими|ей|ою|ею))(?:ся|сь)?"
    r"|[аяеи]в(?:ши|шись)?|(?:ован|ен)[аоы]?)"
)
_RU_VERB_LOOKALIKE = re.compile(r"аудит|аудите|[а-яё]+(?:ием|ия|ость|есть)")


def _ru_verb_form(word: str) -> bool:
    word = word.casefold()
    return bool(_RU_PAST.fullmatch(word)
                or (_RU_VERB_FORM.fullmatch(word) and not _RU_VERB_LOOKALIKE.fullmatch(word)))


def _role_noun(word: str) -> bool:
    """A role or stage word that is not a Russian verb form of one ("аудит",
    "тесты", "ревью", "review"; not "тестируй", "тестирован", "проверь")."""
    return bool(_ROLE_OR_STAGE.fullmatch(word)) and not (
        _CYRILLIC.search(word) and (WORK_VERBS.fullmatch(word) or REQUEST_WORDS.fullmatch(word)
                                    or _ru_verb_form(word)))


def _apposition_word(word: str) -> bool:
    """A plain Latin word that may name the model before its parenthesized
    exact identity ("Claude Opus (claude-agent-acp/opus[1m])")."""
    return not (_CYRILLIC.search(word) or _reserved_request_word(word) or word in _EXCLUSIVE_WORDS
                or _ROLE_OR_STAGE.fullmatch(word) or _REFUSAL_WORDS.fullmatch(word))


def _exclusive_selection(context: Mapping, pattern: re.Pattern, parenthesized: bool = False) -> bool:
    """Whether a not-refused mention is restrictively selected: "только"/
    "only" (optionally followed by a means preposition, a vendor word such
    as "Claude" or another spelling of the same model) stands right before
    it, or English "only" right after it, and

    - nothing follows the mention in its clause segment except a purpose
      phrase ("for the audit", "для ревью") or "please" -- "only Opus found
      the bug" is a remark; and
    - before the marker in its clause there is nothing ("Только Opus"), only
      role or stage nouns without a means preposition ("Аудит только Opus"),
      or a request-form request verb, or a request-form work verb with its
      own object or a means preposition ("Выполнить аудит только Opus",
      "review the code only on Opus") -- "review only Opus" names what is
      reviewed, "аудит делал только Opus" reports.

    A ``parenthesized`` exact identity is an apposition of the name before
    it ("только Claude Opus (claude-agent-acp/opus[1m])"): up to two plain
    name words may stand between it and the marker."""
    clause = context["clause"]
    words = [word for word, _lead in clause]

    def name_word(word):
        if word in _VENDOR_WORDS or word in MEANS_FILLERS or pattern.fullmatch(word):
            return True
        # Latin name words only ("Claude Opus"); Russian spellings are labels.
        return parenthesized and _apposition_word(word)

    position = len(words)
    for _ in range(2):
        if position and name_word(words[position - 1]):
            position -= 1
    means = bool(position) and bool(_EXCLUSIVE_MEANS.fullmatch(words[position - 1]))
    if means:
        position -= 1
    after = list(context["after"])
    if position and words[position - 1] in _EXCLUSIVE_WORDS:
        lead = clause[:position - 1]
    elif after and after[0] in _POSTPOSITIVE_EXCLUSIVE:
        lead, after = clause[:position], after[1:]
    else:
        return False
    if after and after[0] not in _PURPOSE_PREPOSITIONS and not all(word in _EXCLUSIVE_TAIL for word in after):
        return False
    lead_words = [word for word, _lead in lead]
    if not lead_words:
        return True
    # A role noun alone, never a verb: English "Review only Opus" names what is
    # reviewed, so an English role word needs "for" or an article before it,
    # and a Russian verb in any form ("Тестируй только Opus", "Тестировал
    # только Opus", "Тестируя только Opus", "Тестирующий только Opus")
    # is never a role noun; "для" only looks like a verbal adverb.
    if not means and len(lead_words) <= 3 and (
            _CYRILLIC.search(lead_words[0]) or lead_words[0] in _PURPOSE_PREPOSITIONS
            or lead_words[0] in {"the", "a", "an"}) and all(
            word in MEANS_FILLERS or word in _PURPOSE_PREPOSITIONS or _role_noun(word) for word in lead_words):
        return True
    recent = lead[-_EXCLUSIVE_LEAD_WINDOW:]
    for index, (word, word_lead) in enumerate(recent):
        if _request_form(REQUEST_WORDS, word, word_lead):
            return True
        if _request_form(WORK_VERBS, word, word_lead) and (means or index < len(recent) - 1):
            return True
    return False


def requests_family(text: str, family: str, names: Sequence[str] | None = None,
                    *, pattern: re.Pattern | None = None,
                    exact_pattern: re.Pattern | None = None) -> str | None:
    """The first mention of ``family`` in ``text`` that is a request -- within
    :data:`REQUEST_WINDOW` words before it, in its own clause, a
    :data:`REQUEST_WORDS` word in request form or a :data:`WORK_VERBS` verb
    governing a :data:`MEANS_WORDS` preposition right before it, and the
    mention is not refused (:func:`_refused_mention`: refusal words anywhere
    in its clause segment on both sides, substitution words before it, any
    :data:`NEGATION_WORDS` word within :data:`NEGATION_WINDOW` before it) --
    or ``None`` when every mention is bare, a remark or refused. An
    ``exact_pattern`` for enabled identities additionally accepts a direct
    на/on/via/with, or a listing item (the identity alone in its clause, as in
    "Part 2: <identity>") governed by a non-negated request-form verb
    elsewhere in the same message. Labels retain local grammar. A
    restrictive selection (:func:`_exclusive_selection`: "только Opus",
    "do the audit only on Opus") is a request too. ``names`` are extra
    spellings (see :func:`family_pattern`)."""
    # Only enabled exact identities get message-wide request governance and
    # a direct means-preposition grant. Owner-configured labels keep the
    # conservative local grammar; model-authored text never reaches this call.
    exact_spans = {match.span() for match in exact_pattern.finditer(text)} if exact_pattern is not None else set()
    message_request = False
    for sentence in _SENTENCE_END.split(text.replace("’", "'")):
        for word, lead in _clause_words(sentence):
            if (_request_form(REQUEST_WORDS, word, lead)
                    and not any(NEGATION_WORDS.fullmatch(prior) for prior in lead[-NEGATION_WINDOW:])):
                message_request = True
                break
    pattern = pattern if pattern is not None else family_pattern(family, names)
    for match in pattern.finditer(text):
        context = _mention_context(text, match)
        if _refused_mention(context):
            continue
        if _exclusive_selection(context, pattern, parenthesized=(
                match.span() in exact_spans and text[match.start() - 1:match.start()] == "("
                and text[match.end():match.end() + 1] == ")")):
            return match.group(0)
        # The governing verb must stand in the mention's own clause.
        window = context["clause"][-REQUEST_WINDOW:]
        # A refusal before the subjunctive's clause ("не хочу, чтобы ...")
        # may stand outside the negation window; it refuses the whole sentence.
        subjunctive = not any(NEGATION_WORDS.fullmatch(word) for word in context["sentence_words"])
        if (any(_request_form(REQUEST_WORDS, word, lead, subjunctive) for word, lead in window)
                or _governed_means(window, subjunctive)
                or (match.span() in exact_spans and
                    ((message_request and context["alone"]) or (window and
                     re.fullmatch(r"на|on|via|with", window[-1][0]))))):
            return match.group(0)
    return None


def _timestamp_key(value) -> float:
    """Sort key of a stored ISO timestamp; unknown values sort oldest."""
    from datetime import datetime, timezone

    try:
        parsed = datetime.fromisoformat(str(value))
    except (TypeError, ValueError):
        return float("-inf")
    return (parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)).timestamp()


def _user_texts(store, db, task_id: str) -> Iterator[tuple[str, str | None]]:
    """``(source_ref, text)`` of :func:`_user_sources`."""
    for source_ref, text, _selection in _user_sources(store, db, task_id):
        yield source_ref, text


def _choice(value) -> str:
    return " ".join(str(value).split()).casefold().rstrip(".!")


def _user_sources(store, db, task_id: str) -> Iterator[tuple[str, str | None, dict | None]]:
    """``(source_ref, text, selection)`` of every user-authored source of the
    task, newest first: source revisions (by publication time) and answered
    questions' answers (by answer time), interleaved so the latest user
    message comes first. ``None`` text is an attribution barrier after current
    user text surrounding an ambiguous imported summary; older grants cannot
    survive it unless a newer direct user request reauthorizes them.

    ``selection`` is ``{"question": <question text>}`` when a real user
    answer is exactly one of its question's recorded options (a typed option
    number is stored as that option), else ``None``. The user's choice of an
    option is the user's own words; the question text and the options a
    model wrote are never consent by themselves."""
    from ..acp import questions_store

    rows = db.execute(
        "SELECT sr.report_id, r.filename, r.created_at FROM source_revisions sr JOIN reports r ON r.id=sr.report_id "
        "WHERE sr.task_id=? ORDER BY sr.revision DESC", (task_id,),
    ).fetchall()
    texts = []
    for row in rows:
        try:
            text = (store._task_directory(db, task_id) / row["filename"]).read_bytes().decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        texts.append((_timestamp_key(row["created_at"]), f"{row['report_id']}:-1", text, None))
    for answer in reversed(questions_store.answered_questions(db, task_id)):
        if isinstance(answer.get("answer_text"), str):
            options = answer.get("options") if isinstance(answer.get("options"), list) else []
            chosen = _choice(answer["answer_text"])
            selection = (dict(question=str(answer.get("question_text") or ""))
                         if chosen and any(isinstance(option, str) and _choice(option) == chosen for option in options)
                         else None)
            texts.append((_timestamp_key(answer.get("answered_at")), answer["source_ref"], answer["answer_text"],
                          selection))
    # Stable: equal times keep sources before answers, each newest first.
    for _time, source_ref, text, selection in sorted(texts, key=lambda item: item[0], reverse=True):
        yield source_ref, _without_substitution_emphasis(_user_authored(text)), selection
        if _HISTORY.search(text):
            yield source_ref, None, None


_HISTORY = re.compile(r"<chat-history-summary>.*?</chat-history-summary>", re.S)


def _user_authored(text: str) -> str:
    """Exclude terminated imported history from model authorization.

    Paseo joins unescaped multiline entries with a newline, so a literal
    ``[User]`` in assistant/tool output is indistinguishable from a real turn.
    No text in that lossy summary can safely grant or revoke consent. The
    owner must restate requests outside it. An unterminated literal tag is
    ordinary current user text and must not erase a subsequent refusal.
    """
    return _HISTORY.sub("", text)


# "без замены другой моделью" / "without substitution" insists on the model;
# its substitution word must not read as a refusal of it.
_SUBSTITUTION_EMPHASIS = re.compile(
    r"\bбез\s+(?:какой-?(?:либо|нибудь)\s+)?(?:замен\w*|подмен\w*)(?:\s+(?:на\s+)?(?:друг\w*|ин\w*)\s+модел\w*)?"
    r"|\bwithout\s+(?:any\s+)?(?:substitut\w*|replac\w*)(?:\s+(?:by|with)\s+(?:another|other|a\s+different)\s+model)?"
    r"|\bno\s+(?:substitut\w*|replacement)s?\b", re.IGNORECASE)


def _without_substitution_emphasis(text: str) -> str:
    return _SUBSTITUTION_EMPHASIS.sub("", text)


def _reserved_request_word(word: str) -> bool:
    """Whether ``word`` is an ordinary request, work, means, filler or
    negation word, which a ``request_names`` policy may never name: it would
    let everyday words count as asking for a model."""
    return word in MEANS_FILLERS or any(pattern.fullmatch(word) for pattern in (
        REQUEST_WORDS, WORK_VERBS, MEANS_WORDS, NEGATION_WORDS))


def request_names(model_hint: str, project_root=None, environ: Mapping[str, str] | None = None) -> tuple[str, ...]:
    """Only owner-configured request labels for an exact identity."""
    engine, _slash, model = (model_hint or "").partition("/")
    overrides = load_overrides(project_root, environ)
    return model_policy.policy_for(engine, model, overrides).request_names


# Words a model id ends with that name no particular model.
_GENERIC_MODEL_WORDS = frozenset({"default", "auto", "latest", "flash", "pro", "mini", "plan", "preview", "chat"})


def derived_request_names(model_hint: str, identities: Sequence[str], overrides: Mapping) -> tuple[str, ...]:
    """For an enabled model the owner gave no request names, the last word
    of its id ("claude-sonnet-5" -> "sonnet", "gpt-6.1-sol" -> "sol") when
    no other enabled model's id or configured names share it, else ``()``:
    "sol" or "luna" in an owner's message could never name those models."""
    def word(identity):
        model = identity.partition("/")[2].split("[", 1)[0]
        parts = [part for part in re.split(r"[-_./]", model) if part.isalpha()]
        return parts[-1].casefold() if parts else None
    own = word(model_hint)
    # A one-word id ("haiku") is already matched as itself; a variant suffix
    # is not part of how people name a model, so "opus" still names
    # "opus[1m]" (a dev profile without request names ran an owner-assigned
    # Opus audit on Sonnet, 2026-10-22).
    if (model_hint not in identities or not own or own in _GENERIC_MODEL_WORDS or len(own) < 3
            or own == model_hint.partition("/")[2].casefold()):
        return ()
    for identity in identities:
        if identity == model_hint:
            continue
        engine, _, model = identity.partition("/")
        names = model_policy.policy_for(engine, model, overrides).request_names or ()
        if word(identity) == own or own in {name.casefold() for name in names}:
            return ()
    return (own,)


def _enabled_model_identities(overrides: Mapping) -> tuple[str, ...]:
    return tuple(f"{engine}/{model}" for engine, models in (overrides.get("enabled") or {}).items()
                 for model in models)


def _request_token_pattern(token: str, identities: Sequence[str]) -> str:
    """Exact spelling, or one trailing prose mark unless it is a configured ID."""
    spellings = {*identities, *(identity.partition("/")[2] for identity in identities)}
    longer = [spelling for spelling in sorted(spellings) if spelling != token and spelling.startswith(token)]
    excluded = "|".join(re.escape(spelling) + r"(?!\S)" for spelling in longer)
    guard = rf"(?!{excluded})" if excluded else ""
    # Paired quotes and whole tokens remain exact; only unquoted terminal
    # punctuation gains a fallback, never suffixes/provider prefixes.
    punctuated = rf"(?<!\S){guard}(?-i:{re.escape(token)})(?=[.,!?;:](?!\S))"
    return family_pattern(token).pattern + "|" + punctuated


def _source_request_pattern(family: str, names: Sequence[str], identities: Sequence[str]) -> re.Pattern:
    model = family.partition("/")[2]
    patterns = [family_pattern(family, names).pattern, _request_token_pattern(family, identities)]
    matches = [identity for identity in identities if identity.partition("/")[2] == model]
    if matches == [family]:
        patterns.append(_request_token_pattern(model, identities))
    return re.compile("|".join(patterns), re.IGNORECASE)


def ambiguous_user_model_request(store, task_id: str, model_hint: str) -> dict | None:
    """A retained request naming a bare ID shared by enabled engines; never consent."""
    family = family_of(model_hint or "")
    if family is None:
        return None
    identities = _enabled_model_identities(load_overrides(getattr(store, "project_root", None)))
    model = family.partition("/")[2]
    matches = [identity for identity in identities if identity.partition("/")[2] == model]
    if family not in matches or len(matches) < 2:
        return None
    pattern = re.compile(_request_token_pattern(model, identities), re.IGNORECASE)
    with store.connection() as db:
        for source_ref, text in _user_texts(store, db, task_id):
            if text is None:
                return None
            matched = requests_family(text, family, pattern=pattern, exact_pattern=pattern)
            if matched is not None:
                return dict(source_ref=source_ref, matched=matched)
    return None


def _effort_source_text(text: str, identities: Sequence[str]) -> str:
    """Recognize engine/model:effort (or /effort), never a configured longer ID."""
    spellings = {*identities, *(identity.partition("/")[2] for identity in identities)}
    tokens = "|".join(re.escape(token) for token in sorted(spellings, key=len, reverse=True))
    if not tokens:
        return text
    suffix = re.compile(
        rf"(?<!\S)(?P<open>[\"'\x60«“]?)(?P<model>{tokens})"
        rf"(?P<effort>[:/](?:{'|'.join(EFFORT_ORDER)}))(?P<close>[\"'\x60»”]?)(?=[.,!?;:]?(?:\s|$))")
    def strip(match):
        opening, closing = match["open"], match["close"]
        if ((opening and {'"': '"', "'": "'", "\x60": "\x60", "«": "»", "“": "”"}.get(opening) != closing)
                or (not opening and closing) or match["model"] + match["effort"] in spellings):
            return match.group(0)
        # Keep source offsets, moving a paired closing quote next to the model.
        return opening + match["model"] + closing + " " * len(match["effort"])
    return suffix.sub(strip, text)


def _source_effort(source: str, normalized: str, pattern: re.Pattern, matched: str) -> str | None:
    """Effort directly attached to a granted mention, from that same user source.

    Deliberately bounded grammar: no inherited editor setting, report text,
    distant adjective or effort from another model/message becomes consent.
    """
    values = set()
    levels = r"(?:low|medium|high|xhigh|max|ultra)(?![\w/-])"
    suffix = re.compile(
        rf"^[\"'\x60»”]*(?:\s*[:/]\s*(?P<attached>{levels})|"
        rf"\s+(?:at|with|на|с)\s+(?P<qualified>{levels})|"
        rf"[,\s]+effort\s*[=:]\s*(?P<named>{levels})|"
        rf"[,\s]+(?P<described>{levels})(?=\s+(?:effort|throughout)\b))", re.IGNORECASE)
    for mention in pattern.finditer(normalized):
        if mention.group(0) != matched or _refused_mention(_mention_context(normalized, mention)):
            continue
        found = suffix.match(source[mention.end():])
        if found is not None:
            values.add(next(value for value in found.groupdict().values() if value is not None).lower())
    if len(values) > 1:
        raise _policy_error("user_requested_effort_ambiguous",
                            "Conflicting effort selections for one model in the same user source; "
                            "state one exact engine/model:effort before dispatch.")
    return next(iter(values), None)


def _named_identity_patterns(overrides: Mapping, identities: Sequence[str]) -> dict[str, re.Pattern]:
    """Every enabled identity's request pattern: the exact identity, a unique
    bare model ID, and its configured (else derived) request labels."""
    patterns = {}
    for identity in identities:
        engine, _, model = identity.partition("/")
        names = model_policy.policy_for(engine, model, overrides).request_names or derived_request_names(
            identity, identities, overrides)
        patterns[identity] = re.compile(_source_request_pattern(identity, names, identities).pattern + (
            "|" + "|".join(rf"(?<![^\s\"'«“(]){re.escape(n)}(?=[.,!?;:)»”\"']?(?:\s|$))" for n in names)
            if names else ""), re.IGNORECASE)
    return patterns


# The words a chosen question option may hold besides the model's names and
# still be a choice of that model (see _option_selects).
_OPTION_AFFIRMATIONS = frozenset({"да", "yes", "ok", "okay", "ок"})
_OPTION_NOTES = frozenset({"recommended", "default", "рекомендуется", "рекомендую", "рекомендуемый",
                           "рекомендуемая", "рекомендуемое"})
_OPTION_FILLERS = MEANS_FILLERS | {"модель"}
_OPTION_PRONOUNS = frozenset({"it", "this", "это", "его", "её", "ее"})
_OPTION_COORDINATORS = frozenset({"and", "or", "и", "или"})
# "Use Opus to review the code" / "Opus для аудита" name what the model is for.
_OPTION_PURPOSE = _PURPOSE_PREPOSITIONS | {"to"}
# "Sonnet вместо Opus": what a chosen model replaces, followed only by that model.
_OPTION_SUBSTITUTION = re.compile(r"instead|rather|replac\w*|substitut\w*|вместо|замен\w*", re.IGNORECASE)
_OPTION_SUBSTITUTED = frozenset({"of", "than", "на"})
# Request verbs whose bare object before the model is what the model gets
# ("Передать аудит Opus"); after any other verb it may be the model's own
# ("Использовать аудит Opus" can mean Opus's audit).
_DATIVE_REQUESTS = re.compile(r"переда\w*|поруч\w*|отда\w*|назнач\w*|assign|delegate", re.IGNORECASE)
_SELF, _OTHER = object(), object()


def _purpose_noun(word: str) -> bool:
    """A role, stage or purpose word ("аудита", "review", "ui"), never a verb."""
    return _role_noun(word) or (not _CYRILLIC.search(word)
                                and any(re.fullmatch(purpose, word, re.I) for purpose, _roles in _PURPOSE_ROLES))


def _option_tokens(option: str, pattern: re.Pattern, others: Sequence[re.Pattern]) -> list:
    """The casefolded words of ``option`` with each mention of this model as
    :data:`_SELF` and of another enabled model as :data:`_OTHER`. Up to two
    plain Latin words right before a parenthesized mention are its name
    ("Claude Fable (engine/fable-5)")."""
    spans = []
    for kind, patterns in ((_SELF, [pattern]), (_OTHER, others)):
        for candidate in patterns:
            for match in candidate.finditer(option):
                if not any(match.start() < end and start < match.end() for start, end, _kind in spans):
                    spans.append((match.start(), match.end(), kind))
    tokens, cursor = [], 0
    for start, end, kind in sorted(spans, key=lambda span: span[0]):
        words = _plain_words(option[cursor:start])
        if kind is _SELF and option[start - 1:start] == "(" and option[end:end + 1] == ")":
            for index in range(len(words) - 1, max(len(words) - 3, -1), -1):
                if not _apposition_word(words[index]) or words[index] in (
                        _OPTION_PURPOSE | _OPTION_AFFIRMATIONS | _OPTION_NOTES | _EXCLUSIVE_TAIL):
                    break
                words[index] = _SELF
        tokens.extend(words)
        tokens.append(kind)
        cursor = end
    tokens.extend(_plain_words(option[cursor:]))
    return tokens


def _option_prefix(words: Sequence, post_exclusive: bool) -> bool:
    """Whether the words before the model in a chosen option only select it:
    an optional affirmation, an optional request-form request or work verb
    (a Russian role-noun object may precede it), its object (role or stage
    nouns, "it") or a purpose phrase ("для аудита"),
    then exclusive markers, means prepositions, vendor words and fillers.
    A bare role noun right before the model may be the model's own ("аудит
    Opus"), so it needs a marker, a means preposition or a giving verb; a work
    verb needs a means preposition, or an object and a marker ("Review only
    Opus" names what is reviewed)."""
    index = 0
    while index < len(words) and (words[index] in _OPTION_AFFIRMATIONS or words[index] in _EXCLUSIVE_TAIL):
        index += 1
    # Russian may name the object first ("Ревью делай через Opus").
    verb = index
    while verb < len(words) and isinstance(words[verb], str) and _CYRILLIC.search(words[verb]) and _role_noun(
            words[verb]):
        verb += 1
    governor = None
    if verb < len(words) and isinstance(words[verb], str):
        lead = tuple(words[:verb])
        if _request_form(REQUEST_WORDS, words[verb], lead):
            governor = "dative" if _DATIVE_REQUESTS.fullmatch(words[verb]) else "request"
        elif _request_form(WORK_VERBS, words[verb], lead):
            governor = "work"
    objects = governor is not None and verb > index
    if governor is not None:
        index = verb + 1
    purpose = False
    while index < len(words) and isinstance(words[index], str):
        word = words[index]
        if word in _PURPOSE_PREPOSITIONS:
            purpose = True
        elif purpose and (_purpose_noun(word) or word in _OPTION_FILLERS or word in _OPTION_COORDINATORS):
            pass
        elif not purpose and ((_role_noun(word) and (governor or _CYRILLIC.search(word)))
                              or (governor and word in _OPTION_PRONOUNS)):
            objects = True
        elif not purpose and (word in _OPTION_FILLERS or (objects and word in _OPTION_COORDINATORS)):
            pass
        else:
            break
        index += 1
    exclusive = post_exclusive
    strict_means = means = False
    for word in words[index:]:
        if not isinstance(word, str):
            return False
        if word in _EXCLUSIVE_WORDS:
            exclusive = True
        elif _EXCLUSIVE_MEANS.fullmatch(word) or (word == "to" and governor in {"request", "dative"}):
            means = True
            strict_means = strict_means or bool(MEANS_WORDS.fullmatch(word))
        elif not (word in _VENDOR_WORDS or word in _OPTION_FILLERS):
            return False
    if governor == "work":
        return strict_means or (exclusive and (objects or means))
    return not objects or means or exclusive or governor == "dative"


def _option_suffix(words: Sequence) -> bool:
    """Whether the words after the model in a chosen option only qualify the
    choice: "only", "please", "recommended", a purpose phrase ("for the
    audit", "для ревью") or what it replaces ("вместо Opus")."""
    state = "free"
    for word in words:
        if word is _OTHER:
            if state != "substitution":
                return False
            state = "replaced"
        elif not isinstance(word, str):
            return False
        elif state == "substitution":
            if not (word in _OPTION_SUBSTITUTED or word in _VENDOR_WORDS or word in _OPTION_FILLERS):
                return False
        elif word in _POSTPOSITIVE_EXCLUSIVE or word in _EXCLUSIVE_TAIL or word in _OPTION_NOTES:
            pass
        elif state in {"free", "replaced"} and (word in _VENDOR_WORDS or word in _OPTION_FILLERS):
            pass
        elif state == "free" and _OPTION_SUBSTITUTION.fullmatch(word):
            state = "substitution"
        elif word in _OPTION_PURPOSE:
            state = "purpose"
        elif state == "purpose" and (_purpose_noun(word) or word in _OPTION_FILLERS or word in _OPTION_PRONOUNS
                                     or word in _OPTION_COORDINATORS):
            pass
        else:
            return False
    return state != "substitution"


# The words a chosen option may hold besides the model's names and still be
# only its name ("Opus", "Только Opus", "Да, Opus (рекомендуется)"). Such an
# option carries no verb or purpose of its own, so it is a choice of the model
# only under a question that asks which model to use (_question_selections).
_OPTION_BARE = (_VENDOR_WORDS | _OPTION_FILLERS | _OPTION_AFFIRMATIONS | _OPTION_NOTES | _EXCLUSIVE_WORDS
                | _EXCLUSIVE_TAIL)
# Request verbs of choosing, not of giving the work to the model ("Передать
# Opus") or running it there: "Выбрать Opus" says no more than "Opus", so
# under "Чей результат принять?" it picks a result too.
_CHOICE_REQUESTS = re.compile(
    r"use|pick|choose|select|prefer|использ\w*|задейств\w*|выб[еи]р\w*|выбрать|взять|возьм\w*"
    r"|бер(?:и|ите|ём|ем)", re.IGNORECASE)
# An interrogative determiner and how many words after it may lead to the
# role or stage noun it asks about ("Какой из двух аудитов использовать?").
_QUESTION_WHICH = re.compile(r"как(?:ой|ая|ое|ие|ую|им|ом|ого|ому|их|ими)|котор\w+|which|what", re.IGNORECASE)
_WHICH_WINDOW = 4
# A model or route noun, a work verb or a purpose word between the determiner
# and the role noun makes a verb of choice before that noun pick a model:
# "Какую выбрать модель для аудита?", "Which should we pick for the audit?",
# "Which should we use to review?".
_RU_PURPOSE_CONJUNCTIONS = frozenset({"чтобы", "чтоб"})
# Means and locative prepositions governing the role noun name where the
# chosen model goes ("Какую выбрать на аудит?", "Какую использовать в
# аудите?", "Which one should we use on the audit?"); one in an unrelated
# adverbial phrase does not ("Какой в итоге выбрать аудит?", "Which should we
# pick in the end, the audit?").
_CHOICE_PLACEMENT = frozenset({"на", "в", "во", "on", "in", "at"})
# English articles: one may stand between a placement word and the role noun
# it governs, and none counts toward the determiner's window.
_CHOICE_ARTICLES = frozenset({"the", "a", "an"})
# A question's verb of choice after "<modal> we" or "<modal> I" asks what to
# choose ("Which one should we use on the audit?"), unlike the statement "we
# use" in prose.
_QUESTION_MODALS = frozenset({"should", "shall", "can", "could", "would", "will", "must", "do"})
_SELECTION_NOUN = re.compile(r"models?|routes?|engines?|модел\w*|маршрут\w*|движ(?:ок|ка|ку|ком|ке|ки|ков)",
                             re.IGNORECASE)
# A model or route asked for as the means or the object of a choice: "Какой
# моделью выполнить аудит?", "На какой модели гонять проверку?", "Какую
# модель использовать?", "Какой модели поручить аудит?", "Which model should
# run the review?". Matched over
# the casefolded words of one sentence joined by single spaces.
_MODEL_SELECTION_FRAME = re.compile(
    r"(?<!\S)(?:(?:как|котор)(?:ой|ою) моделью|(?:как|котор)ым (?:маршрутом|движком)"
    r"|на (?:как|котор)(?:ой модели|ом (?:маршруте|движке))|(?:как|котор)ую модель|(?:как|котор)ая модель для"
    r"|(?:как|котор)ой модели (?:поруч|переда|отда|назнач)\w*"
    r"|(?:which|what) (?:model|route|engine)(?:$| (?:to|for|should|shall|will|would|must|can|could|do|does)))(?!\S)")
# Whose result, opinion or past work to accept, or which model did better,
# is a judgement, not a model to use: "Чей результат аудита принять?", "Какая
# модель лучше справилась с аудитом?", "Which model found more issues?".
_QUESTION_JUDGEMENT = re.compile(
    r"чей|чь[яеёи]|чь(?:его|ему|ей|ею|им|их|ими|ю|ём|ем)|whose"
    r"|лучш\w*|хуже|better|best|worse|worst|справ(?:ил|ля|ит|ят)\w*|found|нашл\w*|наш[её]л", re.IGNORECASE)
_NOT_POSSESSIVE = frozenset({"let's", "it's", "that's", "what's", "here's", "there's", "who's"})
_EN_PAST_WORK = frozenset({"did", "done", "ran", "wrote", "written", "built", "made", "chose", "chosen",
                           "took", "taken", "gave", "given"})
# Results to accept: with only a request verb ("Результаты какой модели
# использовать?", "Which findings should we use?") the choice is of a result.
_QUESTION_RESULTS = re.compile(
    r"результат\w*|мнени\w*|замечани\w*|вывод\w*|отч[её]т\w*|находк\w*|оценк\w*|предложени\w*|верси\w*"
    r"|ответ(?:а|ы|ов|у|ом|е|ам|ами|ах)?"
    r"|results?|findings?|opinions?|conclusions?|reports?|answers?|verdicts?|outputs?|suggestions?|proposals?"
    r"|versions?", re.IGNORECASE)


def _past_work(words: Sequence[str], index: int) -> bool:
    """Whether ``words[index]`` reports past work or a past choice ("выполнила",
    "провели", "выбрали", "reviewed", "used", "did"), not a request; a
    Russian past tense after "чтобы" is the subjunctive and still asks."""
    word = words[index]
    if _CYRILLIC.search(word):
        return bool((_RU_SUBJUNCTIVE_WORK.fullmatch(word) or _RU_SUBJUNCTIVE_REQUEST.fullmatch(word))
                    and not any(lead in _RU_SUBJUNCTIVE_MARKERS for lead in words[:index]))
    return word in _EN_PAST_WORK or (word.endswith("ed") and any(
        verbs.fullmatch(word[:-cut]) for verbs in (REQUEST_WORDS, WORK_VERBS) for cut in (1, 2, 3)))


def _product_choice(words: Sequence[str]) -> bool:
    """Whether a sentence asks which work product to choose: a verb of
    choice (:data:`_CHOICE_REQUESTS`) after a role or stage noun under an
    interrogative determiner ("Какой аудит выбрать?", "Какой из двух аудитов
    использовать?", "Which audit should we pick?") picks an audit, not a model.
    So does one between the determiner and the noun ("Какой выбрать аудит?",
    "Which should we pick, the audit?") unless a model or route noun, a work
    verb, a purpose word or a means/locative preposition governing the noun
    sits between them (:data:`_SELECTION_NOUN`, :data:`_CHOICE_PLACEMENT`:
    "Какую выбрать на аудит?"). Articles do not count toward the window."""
    for index, word in enumerate(words):
        if not _QUESTION_WHICH.fullmatch(word):
            continue
        limit, verb, blocked, position, articles = index + _WHICH_WINDOW, None, False, index, 0
        while position < min(limit, len(words) - 1):
            position += 1
            later = words[position]
            if later in _CHOICE_ARTICLES:
                articles, limit = articles + 1, limit + 1
                continue
            if _role_noun(later):
                if verb is not None and not blocked:
                    return True
                if position <= index + _WHICH_WINDOW + articles:
                    return any(_CHOICE_REQUESTS.fullmatch(after) for after in words[position + 1:])
                break
            if _CHOICE_REQUESTS.fullmatch(later):
                if verb is None:
                    verb, limit = position, position + _WHICH_WINDOW
            elif (_SELECTION_NOUN.fullmatch(later) or WORK_VERBS.fullmatch(later) or later in _OPTION_PURPOSE
                  or later in _RU_PURPOSE_CONJUNCTIONS or (later in _CHOICE_PLACEMENT
                                                           and _governs_role_noun(words, position))):
                blocked = True
    return False


def _governs_role_noun(words: Sequence[str], position: int) -> bool:
    """Whether the placement word at ``words[position]`` governs a role noun:
    the noun is the next word, optionally after one article ("на аудит",
    "on the audit"), not further on ("в итоге ... аудит")."""
    following = words[position + 1:position + 3]
    if following and following[0] in _CHOICE_ARTICLES:
        following = following[1:]
    return bool(following) and _role_noun(following[0])


def _question_sentences(question: str) -> list[tuple[str, bool]]:
    """The sentences of a model-authored question that neither judge -- a
    possessor, evaluation, finding or past-work cue (:data:`_QUESTION_JUDGEMENT`,
    "model's", "выполнила") -- nor choose among results or work products
    (:data:`_QUESTION_RESULTS`, :func:`_product_choice`), each with whether it
    asks which model to use for some work: a model/route selection frame
    (:data:`_MODEL_SELECTION_FRAME`) or a request-form request or work verb
    whose object is not a result or work product ("Чем выполнить аудит?",
    "Кому поручить аудит?"). A selection frame asks for a model whatever else
    the sentence names."""
    sentences = []
    for sentence in _SENTENCE_END.split(question.replace("’", "'")):
        words = _plain_words(sentence)
        if not words or any(
                _QUESTION_JUDGEMENT.fullmatch(word) or (word.endswith("'s") and word not in _NOT_POSSESSIVE)
                or _past_work(words, index) for index, word in enumerate(words)):
            continue
        product = _product_choice(words)
        requested = False
        for clause in _CLAUSE_BREAK.split(sentence):
            clause_words = _plain_words(clause)
            requested = requested or any(
                _request_form(verbs, word, tuple(clause_words[:index]), subjunctive=True)
                and not (product and verbs is REQUEST_WORDS and _CHOICE_REQUESTS.fullmatch(word))
                for verbs in (REQUEST_WORDS, WORK_VERBS) for index, word in enumerate(clause_words))
            requested = requested or (not product and any(
                _CHOICE_REQUESTS.fullmatch(word) and index >= 2 and clause_words[index - 1] in _CHOOSERS
                and clause_words[index - 2] in _QUESTION_MODALS for index, word in enumerate(clause_words)))
        results = any(_QUESTION_RESULTS.fullmatch(word) for word in words)
        if _MODEL_SELECTION_FRAME.search(" ".join(words)):
            sentences.append((sentence, True))
        elif requested and not results:
            sentences.append((sentence, True))
        elif not (results or product):
            sentences.append((sentence, False))
    return sentences


def _question_selections(question: str) -> list[str]:
    """The sentences of a model-authored question that ask which model to use
    for some work (see :func:`_question_sentences`). A bare-name option grants
    only under one of them, and they scope which purpose the choice covers.
    The question never grants by itself."""
    return [sentence for sentence, selects in _question_sentences(question) if selects]


def _question_scope(question: str) -> list[str]:
    """The sentences of a model-authored question that scope the purpose of a
    chosen option naming no work of its own: those asking which model to use,
    else those that neither judge nor choose a result or work product
    ("Аудит завершён. Что дальше?" -> the first sentence; never "Чей результат
    аудита принять?")."""
    sentences = _question_sentences(question)
    return [sentence for sentence, selects in sentences if selects] or [sentence for sentence, _ in sentences]


def _bare_choice(prefix: Sequence, suffix: Sequence) -> bool:
    """Whether a chosen option holds, apart from the model's names, only
    bare words (:data:`_OPTION_BARE`), at most one request-form verb of choice
    before the model (:data:`_CHOICE_REQUESTS`: "Выбрать Opus", "Use Opus";
    not a giving verb, "Передать Opus") and what the model replaces ("Sonnet
    вместо Opus"): no object, purpose or means of its own."""
    cut = next((index for index, word in enumerate(suffix)
                if isinstance(word, str) and _OPTION_SUBSTITUTION.fullmatch(word)), len(suffix))
    if not all(isinstance(word, str) and word in _OPTION_BARE for word in suffix[:cut]):
        return False
    if not all(word is _OTHER or (isinstance(word, str) and (
            _OPTION_SUBSTITUTION.fullmatch(word) or word in _OPTION_SUBSTITUTED or word in _OPTION_BARE))
            for word in suffix[cut:]):
        return False
    verbs = [index for index, word in enumerate(prefix) if word is not _SELF and word not in _OPTION_BARE]
    if not verbs:
        return True
    verb = verbs[0]
    return (len(verbs) == 1 and isinstance(prefix[verb], str) and bool(_CHOICE_REQUESTS.fullmatch(prefix[verb]))
            and _request_form(REQUEST_WORDS, prefix[verb], tuple(prefix[:verb])))


# A parenthesized plan-usage note a coordinator may add to an option now that
# it sees usage: a percentage with only usage words ("(квота 52%)", "(52% of
# quota used)", "(использовано 52% лимита)"). A note with any other word, such
# as "исчерпана", stays in the option and makes it a remark.
_USAGE_NOTE = re.compile(r"\(([^()]*\d\s*%[^()]*)\)")
_USAGE_NOTE_WORDS = re.compile(
    r"\d+|квот\w*|лимит\w*|план\w*|расход\w*|использован\w*|израсходован\w*|из"
    r"|quota|limit|plan|usage|used|of|the", re.IGNORECASE)


def _without_usage_notes(option: str) -> str:
    """``option`` with each parenthesized plan-usage note (:data:`_USAGE_NOTE`)
    replaced by a space."""
    return _USAGE_NOTE.sub(lambda note: " " if all(
        _USAGE_NOTE_WORDS.fullmatch(word) for word in _plain_words(note.group(1))) else note.group(0), option)


def _option_selects(option: str, pattern: re.Pattern, others: Sequence[re.Pattern], question: str) -> bool:
    """Whether a chosen option, read as a whole, selects the model of
    ``pattern`` as the one to use rather than mentioning it as a subject,
    possessor or past actor ("Исправить, как предложил Opus", "Opus исчерпал
    квоту", "Accept Opus's audit result"): apart from the model's names it
    holds only the words :func:`_option_prefix` and :func:`_option_suffix`
    accept ("Opus", "Claude Opus (claude-agent-acp/opus[1m])", "Использовать
    Opus", "Выполнить аудит только Claude Opus", "Opus для аудита", "Sonnet
    вместо Opus"). Punctuation does not split it: "Opus — уже проверил" is a
    remark too. An option that is only the model's name, or that name after a
    lone verb of choice (:func:`_bare_choice`), selects it only when
    ``question`` asks which model to use (:func:`_question_selections`): under
    "Чей результат аудита принять?" the options "Opus" and "Выбрать Opus"
    pick a result. A parenthesized plan-usage note is not part of the choice
    ("Opus (квота 52%)", :func:`_without_usage_notes`)."""
    tokens = _option_tokens(_without_usage_notes(option), pattern, others)
    if _SELF not in tokens:
        return False
    first = tokens.index(_SELF)
    last = len(tokens) - 1 - tokens[::-1].index(_SELF)
    if not all(token is _SELF or token in _VENDOR_WORDS or token in _OPTION_FILLERS
               for token in tokens[first + 1:last]):
        return False
    suffix = tokens[last + 1:]
    post_exclusive = bool(suffix) and suffix[0] in _POSTPOSITIVE_EXCLUSIVE
    if not (_option_prefix(tokens[:first], post_exclusive) and _option_suffix(suffix)):
        return False
    if _bare_choice(tokens[:last + 1], suffix):
        return bool(_question_selections(question))
    return True


def _selected_option_mention(option: str, pattern: re.Pattern, others: Sequence[re.Pattern],
                             question: str) -> str | None:
    """The mention of a model in a question option the user selected, when
    the option selects it -- an explicit responsibility construction
    (:func:`assigns_family`) or, read as a whole, a choice of that model
    (:func:`_option_selects`, which reads ``question`` for a bare-name
    option) -- it is not refused, compared or substituted, and no other
    enabled model is named the same way; else ``None``. An option naming two
    models is ambiguous and selects neither. An option that merely mentions
    the model (its result, quota or opinion) is the asking model's text,
    never the user's request for it."""
    mentions = [match for match in pattern.finditer(option)
                if not _refused_mention(_mention_context(option, match))]
    if not mentions:
        return None
    spans = [match.span() for match in mentions]
    for other in others:
        for match in other.finditer(option):
            overlaps = any(match.start() < end and start < match.end() for start, end in spans)
            if not overlaps and not _refused_mention(_mention_context(option, match)):
                return None
    if assigns_family(option, pattern) is not None or _option_selects(option, pattern, others, question):
        return mentions[0].group(0)
    return None


def _user_model_consent(store, task_id: str, model_hint: str,
                        names: Sequence[str] | None = None) -> tuple[dict | None, dict | None]:
    """``(grant, revocation)`` from the newest user message that either
    refuses or requests ``model_hint``: the latest message wins. A refusal
    newer than a grant revokes it (``reason='revoked_by_user'``); a refusal
    with no older grant is neither."""
    family = family_of(model_hint or "")
    if family is None:
        return None, None
    overrides = load_overrides(getattr(store, "project_root", None))
    engine, _, model = model_hint.partition("/")
    configured_names = model_policy.policy_for(engine, model, overrides).request_names
    if names is not None and tuple(names) != configured_names:
        raise _policy_error("model_request_labels_changed", "Request labels must match current owner settings; reread routing.json.")
    identities = _enabled_model_identities(overrides)
    names = configured_names
    pattern = _source_request_pattern(family, names, identities)
    derived = () if configured_names else derived_request_names(model_hint, identities, overrides)
    if derived:
        # A derived label is a whole standalone word only: "gpt-6-luna.v2",
        # "vendor/gpt-6-luna" or "GPT-6-LUNA" stay other identities.
        pattern = re.compile(pattern.pattern + "|" + "|".join(
            rf"(?<![^\s\"'«“(]){re.escape(label)}(?=[.,!?;:)»”\"']?(?:\s|$))" for label in derived), re.IGNORECASE)
    exact_pattern = _source_request_pattern(family, (), identities) if family in identities else None
    refusal = None
    others = None
    with store.connection() as db:
        for source_ref, text, selection in _user_sources(store, db, task_id):
            if text is None:
                if refusal is None:
                    return None, None
                continue  # retain a newer direct refusal against any older grant
            # Strip only known effort suffixes from exact configured identities,
            # keeping offsets intact for source-bound effort extraction below.
            source_text = text
            text = _effort_source_text(text, identities)
            # Within one message a refusal outweighs a grant of the same identity.
            refused = refuses_family(text, family, names, pattern=pattern)
            if refused is not None:
                refusal = refusal or dict(source_ref=source_ref, family=family, matched=refused)
                continue
            if selection is not None:
                # The user chose a question option. The asking model wrote its
                # words, so it grants only when it selects this model, never
                # when it merely mentions it ("Принять результат аудита Opus").
                if others is None:
                    others = [other for identity, other in _named_identity_patterns(overrides, identities).items()
                              if identity != family]
                matched = _selected_option_mention(text, pattern, others, selection["question"])
            else:
                matched = requests_family(text, family, names, pattern=pattern, exact_pattern=exact_pattern)
            if matched is None and selection is None:
                # A model made responsible for a role or stage is a request
                # too: "добавь требование, чтобы за audit отвечал opus" named
                # Opus for the audit, yet failed the request grammar and the
                # audit ran on another model (observed 2026-10-22).
                matched = assigns_family(text, pattern)
            if matched is None:
                continue
            if refusal is not None:
                return None, dict(refusal, reason="revoked_by_user", revoked_source_ref=source_ref)
            for engine, config in (overrides.get("models") or {}).items():
                for model, policy in (config.get("policy") or {}).items():
                    identity = f"{engine}/{model}"
                    if identity != model_hint and family_pattern(identity, policy.get("request_names")).search(matched):
                        raise _policy_error("model_request_label_ambiguous", "The requested label matches multiple configured identities.")
            grant = dict(source_ref=source_ref, family=family, matched=matched)
            effort = _source_effort(source_text, text, pattern, matched)
            if effort is not None:
                grant["effort"] = effort
            return grant, None
    return None, None


# What a role is for, in the owner's words. A node with no model hint (the
# runtime's own review node, or a plan task the coordinator left on auto)
# still gets the model the owner asked for that purpose.
_PURPOSE_ROLES = (
    (r"review\w*|ревью|code-review|аудит\w*|audit\w*", {"worker-code-reviewer", "worker-security-auditor",
                                                        "worker-accessibility-auditor"}),
    (r"design\w*|дизайн\w*|ui|ux|ui/ux|верстк\w*", {"worker-ux-designer", "worker-frontend-dev"}),
    (r"test\w*|тест\w*|qa", {"worker-qa-engineer", "worker-build-verification"}),
    (r"docs?|documentation|документац\w*", {"worker-technical-writer"}),
    (r"security|безопасност\w*", {"worker-security-auditor"}),
    (r"accessibility|a11y|доступност\w*", {"worker-accessibility-auditor", "worker-accessibility-fixer"}),
    (r"backend|бэкенд\w*|бэк", {"worker-backend-dev"}),
    (r"frontend|фронтенд\w*|фронт", {"worker-frontend-dev"}),
    (r"devops|deploy\w*|деплой\w*|ci", {"worker-devops-engineer"}),
    (r"debug\w*|отладк\w*", {"worker-debugger"}),
    (r"architect\w*|архитектур\w*", {"worker-architect", "worker-database-architect"}),
    (r"refactor\w*|рефактор\w*", {"worker-refactorer"}),
    (r"plan\w*|планир\w*", {"worker-planner"}),
)


# A qualified audit names only its specialist: "security-аудит" or "аудит
# безопасности" also sent the code review to Opus (live, 2026-10-22).
_QUALIFIED_AUDIT = re.compile(
    r"(?P<q>security|accessibility|a11y)[\s-]+(?:audit\w*|аудит\w*)|(?:audit\w*|аудит\w*)\s+"
    r"(?:of\s+)?(?P<p>security|accessibility|безопасност\w*|доступност\w*)", re.I)


def _purposes_of(role_id: str) -> list[re.Pattern]:
    return [re.compile(pattern, re.I) for pattern, roles in _PURPOSE_ROLES if role_id in roles]


# Work verbs that do not say what the work is ("Выполнить через Opus"): an
# option holding only one of them takes its purpose from its question.
_GENERIC_WORK = re.compile(
    r"perform|handle|do|redo|rerun|retry|(?:с|пере|до)?дела(?:й|йте|ть|ем)|выполн\w*|провер\w*"
    r"|провед\w*|провести", re.IGNORECASE)


def _option_names_work(option: str, patterns) -> bool:
    """Whether a chosen option names its own work, apart from any model's
    names: a role, stage or purpose word ("для аудита", "review") or a work
    verb that says what the work is ("Исправить", "fix"; not "Выполнить")."""
    for pattern in patterns:
        option = pattern.sub(" ", option)
    return any(_purpose_noun(word) or any(re.fullmatch(purpose, word, re.I) for purpose, _roles in _PURPOSE_ROLES)
               or (WORK_VERBS.fullmatch(word) and not _GENERIC_WORK.fullmatch(word))
               for word in _plain_words(option))


def user_purpose_model_request(store, task_id: str, role_id: str | None) -> str | None:
    """The exact enabled identity the owner asked for this role's purpose
    ("используй sonnet для ревью", "за аудит отвечает Opus", "UI/UX передавай
    Фейблу"), from the newest user sentence naming both a model request and
    that purpose; ``None`` when there is none, it is refused or names two
    models. Only the request grammar of :func:`user_model_request` counts, so
    the returned identity also grants through it. A selected question option
    naming one model (see :func:`_user_sources`) covers the purpose the
    option names, else the purpose its question asks the model for
    ("Какой моделью выполнить аудит?" -> "Opus"; see :func:`_question_scope`,
    which never reads a judging sentence): the question only scopes
    the user's choice, never grants. An option that names its own work
    ("Исправить через Opus") is scoped by that work alone."""
    purposes = _purposes_of(role_id or "")
    if not purposes:
        return None
    overrides = load_overrides(getattr(store, "project_root", None))
    identities = _enabled_model_identities(overrides)
    patterns = _named_identity_patterns(overrides, identities)

    def has_purpose(text):
        words = _plain_words(_QUALIFIED_AUDIT.sub(r"\g<q>\g<p>", text))
        return any(p.fullmatch(word) for p in purposes for word in words)

    with store.connection() as db:
        for _source_ref, text, selection in _user_sources(store, db, task_id):
            if text is None:
                return None
            text = _effort_source_text(text, identities)
            if selection is not None:
                # A chosen option grants by the option rule alone (see
                # _user_model_consent), so the identity returned here grants there.
                question = selection["question"]
                scope = text if _option_names_work(text, patterns.values()) else "\n".join(
                    _question_scope(question)) + "\n" + text
                if not has_purpose(scope):
                    continue
                named = {identity for identity, pattern in patterns.items()
                         if _selected_option_mention(text, pattern, [other for name, other in patterns.items()
                                                                     if name != identity], question) is not None}
                if len(named) == 1:
                    return named.pop()
                if named or any(refuses_family(text, identity, (), pattern=pattern) is not None
                                for identity, pattern in patterns.items()):
                    return None
                continue
            for sentence in reversed(_SENTENCE_END.split(text)):
                if not has_purpose(sentence):
                    continue
                named = set()
                refused = False
                for identity, pattern in patterns.items():
                    if not pattern.search(sentence):
                        continue
                    if refuses_family(sentence, identity, (), pattern=pattern) is not None:
                        refused = True
                        continue
                    if (requests_family(sentence, identity, (), pattern=pattern) is not None
                            or assigns_family(sentence, pattern) is not None):
                        named.add(identity)
                if refused and not named:
                    return None
                if len(named) == 1:
                    return named.pop()
                if named:
                    return None
    return None


def user_model_request(store, task_id: str, model_hint: str, *, names: Sequence[str] | None = None) -> dict | None:
    """Match only retained user source text, never coordinator/report text.
    The newest user message that mentions the identity decides: a newer
    refusal revokes an older request (see :func:`user_model_revocation`)."""
    return _user_model_consent(store, task_id, model_hint, names)[0]


def user_model_revocation(store, task_id: str, model_hint: str) -> dict | None:
    """The newer user refusal that revoked an earlier request for
    ``model_hint`` (``source_ref``, ``matched``, ``reason='revoked_by_user'``
    and the revoked ``revoked_source_ref``), or ``None``. Never consent."""
    return _user_model_consent(store, task_id, model_hint)[1]
