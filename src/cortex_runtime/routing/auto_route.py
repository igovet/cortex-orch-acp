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
USAGE_SWITCH_PERCENT = 95.0
HOT_PERCENT = USAGE_SWITCH_PERCENT
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
# "implementation". routing.json ``engine_affinity`` overrides this per kind.
WORK_KIND_ENGINES = {
    "implementation": "codex-acp",
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
ROLE_WORK_KINDS = {
    "worker-accessibility-auditor": "review",
    "worker-accessibility-fixer": "implementation",
    "worker-architect": "design",
    "worker-backend-dev": "implementation",
    "worker-build-verification": "testing",
    "worker-code-reviewer": "review",
    "worker-data-engineer": "implementation",
    "worker-database-architect": "design",
    "worker-debugger": "debugging",
    "worker-devops-engineer": "operations",
    "worker-explorer": "investigation",
    "worker-frontend-dev": "implementation",
    "worker-fullstack-dev": "implementation",
    "worker-general": "analysis",
    "worker-mobile-dev": "implementation",
    "worker-performance-engineer": "debugging",
    "worker-planner": "planning",
    "worker-qa-engineer": "testing",
    "worker-refactorer": "implementation",
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


def work_kind(role: str | None) -> str | None:
    return None if role is None else ROLE_WORK_KINDS.get(role, "implementation")


def stretch_status(role: str | None, role_tier: str, complexity: str = "normal", kinds=None) -> str:
    """Whether configured light stretch policy may serve this work."""
    if role_tier == "light":
        return "light_tier"
    if complexity == "high":
        return "excluded:complexity_high"
    if role_tier != "standard":
        return f"excluded:{role_tier}_work"
    if ROLE_TIERS.get(role or "", "standard") in ("deep", "flagship"):
        return "excluded:deep_role"
    kind = work_kind(role)
    if kind not in (() if kinds is None else kinds):
        return f"not_candidate:work_kind_{kind}"
    return "standard_candidate"


def hint_serves_tier(model: str, model_tier: str | None, role: str | None, role_tier: str,
                     complexity: str = "normal", *, also_standard_for=None) -> bool:
    """Whether explicit tiers or configured standard allowances serve this work."""
    kinds = frozenset(also_standard_for or ())
    return model_tier == role_tier or (
        model_tier == "light" and bool(kinds)
        and stretch_status(role, role_tier, complexity, kinds) == "standard_candidate")


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
    if "*" in affinity:
        return affinity["*"]
    return WORK_KIND_ENGINES[kind]


def affinity_engine(role: str | None, affinity: Mapping[str, str | None] | None = None) -> str | None:
    """The engine ``role``'s kind of work leans to, or ``None`` (neutral).
    ``affinity`` is routing.json ``engine_affinity`` (see :func:`kind_engine`)."""
    kind = work_kind(role)
    return None if kind is None else kind_engine(kind, affinity)


def suitability(role: str | None, engine: str, affinity: Mapping[str, str | None] | None = None) -> int:
    """1 when ``engine`` is the better fit for ``role``'s kind of work."""
    preferred = affinity_engine(role, affinity)
    return int(preferred is not None and preferred == engine)


# A model hint (or a legacy stored provider preference) is ignored for
# quota only when its usage is both at or above this percentage *and*
# meaningfully worse than an available alternative's (see USAGE_MARGIN
# below): crossing the band alone no longer disqualifies it, so a hint just
# over HOT_PERCENT with no meaningfully better alternative is still
# honoured. It is independent of how often an engine was chosen before.
# Excluded, unavailable and exhausted (100%) routes are never honoured at
# all.
HINT_TOLERANCE = HOT_PERCENT
# Once observed usage reaches USAGE_SWITCH_PERCENT, plan usage ranks
# candidates in steps of this many percentage points above
# the least-used candidate: a smaller difference is not a meaningful
# difference in remaining quota, so the task (engine affinity) decides it.
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
           user_revocation: Mapping | None = None) -> Choice | None:
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
    if escalate and stretch_status(role, base_role_tier, complexity, hold_kinds) == "standard_candidate":
        role_tier = base_role_tier
    affinity = overrides.get("engine_affinity") or {}
    # Light-tier stretch models follow their own rule (see stretch_status); a
    # tier override that moves one elsewhere routes it like any model.
    stretch_states = {route_id: stretch_status(role, role_tier, complexity, policy.also_standard_for)
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
        effort = nearest(EFFORT_BY_TIER[requested_tier],
                         permitted_efforts(picked, requested_tier, policy))
        decision = dict(
            role=role, base_role_tier=base_role_tier, role_tier=requested_tier, tier=requested_tier,
            basis="user_request", decided_by="user_request", rank_version=RANK_VERSION,
            escalated=escalate, preference=preference,
            preference_status="ignored:user_request" if preference else "none",
            model_hint=model_hint, model_hint_status="honoured", model_hint_reason=model_reason,
            effort_basis=f"user_request:{requested_tier}",
            user_model_request=dict(user_request, status="honoured"),
            candidates=[_candidate(e, usage, role, False, affinity=affinity) for e in requested],
            chosen=model_hint, chosen_connection_id=picked.connection_id)
        if profile_label is not None:
            decision["routing_profile"] = profile_label
        return Choice(picked.connection_id, picked.route.model, effort, "auto:user_request", decision, picked.route.id)
    pin = _pin_for(overrides, role)
    if pin is not None:
        engine, model, effort = pin
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
            candidates=[_candidate(e, usage, role, False, affinity=affinity) for e in ranked],
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
        if best_policy.effort_ladder and default_effort not in permitted:
            raise _policy_error("model_effort_policy_unsupported", "Pinned effort_ladder step is not permitted.")
        pinned_effort = nearest(effort or default_effort, permitted)
        if effort is not None and pinned_effort != effort:
            # C-35 L1: a configured pin effort routing.json's own policy
            # does not permit is silently lowered/raised to the nearest
            # permitted step (never refused); record that it happened.
            decision["pin_effort_clamped"] = dict(configured=effort, applied=pinned_effort)
        return Choice(best.connection_id, model, pinned_effort,
                      "auto:pin", decision, best.route.id)
    default_denied = {entry.route.id for entry in allowed if _default_auto_denied(overrides, entry)}
    # C-21: a model configured with several native tiers (e.g. both
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
                            all_entries=entries, providers=providers, affinity=affinity, models=models)
    if decision is not None and status:
        decision.update(preference=preference, preference_status=status)
    if decision is not None and user_request is None and user_revocation is not None and model_hint != "auto":
        decision["user_model_request"] = dict(user_revocation, status="revoked_by_user")
    elif decision is not None and request_status is not None:
        decision["user_model_request"] = dict(user_request or {}, status=request_status)
    if decision is not None and profile_label is not None:
        decision["routing_profile"] = profile_label
    if picked is None:
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
RANK_VERSION = 11
# The rank key's parts in order, named for the decision's ``decided_by``.
# ``quota`` moved from right after ``hint`` to right before ``used_percent``
# in rank version 6 (a late tie breaker, behind the usage-gap signal,
# retry diversification and affinity), then back to right after ``hint``
# in rank version 11 (C-19): a candidate whose *known* usage reaches
# :data:`HOT_PERCENT` must rank behind every non-hot candidate -- known or
# unknown usage alike -- never merely a late, easily-ties-with-unknown tie
# breaker affinity and diversification could still outrank.
RANK_KEYS = ("preference", "purpose", "hint", "quota", "usage", "diversify", "affinity", "used_percent",
             "engine_order")


def rank(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None,
         policy_excluded: frozenset = frozenset(), superseded: Mapping[str, str] | None = None,
         model_hint: str = "auto", model_reason: str | None = None,
         escalated: bool = False, previous_engine: str | None = None,
         all_entries: Sequence | None = None, providers: Sequence[str] | None = None,
         affinity: Mapping[str, str | None] | None = None,
         stretch_standard: frozenset = frozenset(), stretch_status: str | None = None,
         models: Mapping | None = None):
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
       the condition no longer applies. It never moves the tier;
    3. an honoured model hint (see :data:`HINT_TOLERANCE`), ignored on the
       same hot-and-meaningfully-worse basis; a hint naming no preferred
       route loses to ``purpose`` and records ``ignored:owner_preferred``;
    4. the quota band (unknown is not hot): a candidate whose *known*
       usage is at or above :data:`HOT_PERCENT` ranks behind every
       candidate below that band, known or unknown usage alike (C-19) --
       this outranks the usage-gap signal, diversification and affinity
       below, so a hot known candidate never wins merely by being the
       preferred engine or more suitable for the work;
    5. subscription plan usage, only if every remaining candidate's usage
       is known and at least one reaches :data:`USAGE_SWITCH_PERCENT`
       (95%): each full :data:`USAGE_MARGIN` points above the least-used
       candidate ranks a candidate one step behind (``usage_band``) --
       this usage-gap signal is what usually separates candidates on
       remaining quota once the quota band above has not already decided it;
    6. on escalation, an engine other than the one that fell short (a large
       usage gap above still wins first: diversification does not send a
       retry to a plan that is meaningfully more used);
    7. engine affinity (:func:`suitability`), the task's fit;
    8. lower exact used percent, only when usage switching is active;
    9. engine order.

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
    kind = work_kind(role)

    def purpose(e) -> bool:
        policy = policies[e.route.id]
        if policy.preferred_when is None:
            return kind is not None and kind in policy.preferred_for
        # A conditional preference: the coordinator judged this node against
        # the owner's ``preferred_when`` text and named this exact route.
        # Retries and fixes ignore hints, so they ignore the condition too.
        return (not escalated and model_hint == _route_label(e)
                and (not policy.preferred_for or kind in policy.preferred_for))
    usable = [e for e in entries if e.route.id not in excluded and (usage(e) or 0.0) < 100.0]
    order = {e.route.id: index for index, e in enumerate(entries)}

    def serves(e, tier):
        return tier in policies[e.route.id].tiers or (tier == "standard" and e.route.id in stretch_standard)

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
        used_percent_compared = all(usage(e) is not None for e in candidates)
        usage_active = used_percent_compared and any(_hot(usage(e)) for e in candidates)
        least_used = min(usage(e) for e in candidates) if usage_active else None

        def usage_band(e):
            return int((usage(e) - least_used) // USAGE_MARGIN) if usage_active else None

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
            elif _meaningfully_worse(hinted, candidates, usage):
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

        def key(e):
            # C-19: a candidate's own hot/known-quota flag ranks right after
            # preference/purpose/hint -- before the usage-gap band,
            # diversification and affinity -- so a known >=95% candidate
            # always loses to any non-hot one (known or unknown usage).
            return (e.route.engine != honoured,
                    not (purpose_status == "honoured" and purpose(e)),
                    not (hint_status == "honoured" and e in hinted),
                    _hot(usage(e)),
                    usage_band(e) or 0,
                    not (escalated and previous_engine and e.route.engine != previous_engine),
                    -suitability(role, e.route.engine, affinity),
                    usage(e) if usage_active else 0.0, order[e.route.id])

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
                       status="unavailable" if not used_percent_compared else "active" if usage_active else "below_threshold",
                       threshold=USAGE_SWITCH_PERCENT, margin=USAGE_MARGIN, least_used_percent=least_used),
            affinity=dict(work_kind=affinity_kind, engine=affinity_engine(role, affinity),
                          source=("routing.json" if affinity_kind is not None and affinity
                                  and (affinity_kind in affinity or "*" in affinity) else "default")),
            used_percent_compared=used_percent_compared,
            purpose=dict(work_kind=kind, preferred=[_route_label(e) for e in ranked if purpose(e)],
                         status=purpose_status),
            candidates=[_candidate(e, usage, role, e.route.engine == honoured, usage_band(e),
                                   affinity=affinity) for e in ranked],
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


def _candidate(entry, usage, role, preferred: bool, usage_band: int | None = None, *,
               affinity: Mapping[str, str | None] | None = None) -> dict:
    """One ranked candidate as the route audit records it."""
    return dict(route=_route_label(entry), connection_id=entry.connection_id, used_percent=usage(entry),
                hot=_hot(usage(entry)), quota_state="unknown" if usage(entry) is None else "known",
                usage_band=usage_band,
                suitability=suitability(role, entry.route.engine, affinity), preferred=preferred)


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
        return data
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
        kinds = (*WORK_KIND_ENGINES, "*")
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
    r"implement|build|fix|write|test|review|perform|handle|redo|rerun|retry|debug|refactor"
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
# missed refusal would run a model the user refused.
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
    for opening, closing in (("\"", "\""), ("'", "'"), ("`", "`"), ("«", "»"), ("“", "”")):
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
    elsewhere in the same message. Labels retain local grammar. ``names``
    are extra spellings (see :func:`family_pattern`)."""
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
    for match in (pattern if pattern is not None else family_pattern(family, names)).finditer(text):
        context = _mention_context(text, match)
        if _refused_mention(context):
            continue
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


def _user_texts(store, db, task_id: str) -> Iterator[tuple[str, str]]:
    """``(source_ref, text)`` of every user-authored source of the task,
    newest first: source revisions (by publication time) and answered
    questions' answers (by answer time), interleaved so the latest user
    message comes first."""
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
        texts.append((_timestamp_key(row["created_at"]), f"{row['report_id']}:-1", text))
    for answer in reversed(questions_store.answered_questions(db, task_id)):
        if isinstance(answer.get("answer_text"), str):
            texts.append((_timestamp_key(answer.get("answered_at")), answer["source_ref"], answer["answer_text"]))
    # Stable: equal times keep sources before answers, each newest first.
    for _time, source_ref, text in sorted(texts, key=lambda item: item[0], reverse=True):
        yield source_ref, text


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
            matched = requests_family(text, family, pattern=pattern, exact_pattern=pattern)
            if matched is not None:
                return dict(source_ref=source_ref, matched=matched)
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
    names = configured_names
    identities = _enabled_model_identities(overrides)
    pattern = _source_request_pattern(family, names, identities)
    exact_pattern = _source_request_pattern(family, (), identities) if family in identities else None
    refusal = None
    with store.connection() as db:
        for source_ref, text in _user_texts(store, db, task_id):
            # Within one message a refusal outweighs a grant of the same identity.
            refused = refuses_family(text, family, names, pattern=pattern)
            if refused is not None:
                refusal = refusal or dict(source_ref=source_ref, family=family, matched=refused)
                continue
            matched = requests_family(text, family, names, pattern=pattern, exact_pattern=exact_pattern)
            if matched is None:
                continue
            if refusal is not None:
                return None, dict(refusal, reason="revoked_by_user", revoked_source_ref=source_ref)
            for engine, config in (overrides.get("models") or {}).items():
                for model, policy in (config.get("policy") or {}).items():
                    identity = f"{engine}/{model}"
                    if identity != model_hint and family_pattern(identity, policy.get("request_names")).search(matched):
                        raise _policy_error("model_request_label_ambiguous", "The requested label matches multiple configured identities.")
            return dict(source_ref=source_ref, family=family, matched=matched), None
    return None, None


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
