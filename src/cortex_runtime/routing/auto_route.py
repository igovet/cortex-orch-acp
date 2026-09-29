"""Choose a worker's connection, model and effort from what is actually
available -- replaces the hand-maintained per-role ``route-policy.json``.

1. Each role has a tier (:data:`ROLE_TIERS`, ``standard`` when unlisted).
2. Each model's tier: a Claude family name decides it (:data:`FAMILY_TIERS`),
   a known Codex model its entry in :data:`KNOWN_MODEL_TIERS`; only a model
   neither names is placed by its position in its own engine's list
   (strongest first), split into thirds, so a new model still lands in a
   tier without configuration. Claude Haiku remains available for an explicit
   pin, but is excluded from automatic worker selection by default.
   Only the newest version of each model family on an engine is routed
   automatically (:func:`superseded_routes`): family and version are parsed
   from the engine-confirmed model id (or, for an alias such as
   ``opus[1m]``, from the engine's own display name "Opus 5.5"), so a new
   release such as ``gpt-6-luna`` supersedes ``gpt-5.6-luna`` with no
   configuration. ``deny`` is applied first, so the newest *non-denied*
   version is the one routed. A model whose version cannot be parsed is
   never treated as older.
3. Among models of the role's tier (nearest other tier when none is usable),
   rank by honoured provider preference, honoured model hint, a different
   engine on escalation, subscription plan usage (a plan more than
   :data:`USAGE_MARGIN` points more used than the least-used candidate's
   ranks behind it; compared only when every candidate's usage is known),
   engine affinity, the quota band as a late tie breaker (an account at or
   above :data:`HOT_PERCENT` ranks behind one below it only once plan usage
   and affinity left the candidates tied -- crossing the band alone no
   longer overrides them), exact used percent, and engine order
   (:func:`rank`). A preference or model hint is ignored for quota only when
   its usage is both at or above :data:`HOT_PERCENT` and meaningfully worse
   (by more than :data:`USAGE_MARGIN`) than an available alternative's, not
   merely for crossing the band. How often an engine was chosen recently
   never counts: frequency is not evidence of fit. A model of a stronger
   tier never stands in for the role's tier because one engine lacks a model
   there: with Haiku excluded the light tier is Codex-only, and light work
   stays on it (``auto_allow`` Haiku to let Claude take light work). Only
   when no engine has a usable model in the tier does the rank fall back to
   the nearest other tier (:data:`_FALLBACK_ORDER`). Haiku is excluded by
   default unless routing.json opts it in with ``auto_allow``. Exhausted
   (100%) and recorded-unavailable routes never enter the rank. The decision records rank inputs, exclusions and
   ``decided_by`` (the first key that separated the choice from the
   runner-up) for later comparison.
   Engine affinity (:data:`WORK_KIND_ENGINES`) by default leans only
   implementation and operations to Codex; investigation, review, design,
   writing, planning and analysis to Claude; debugging and testing are
   neutral. routing.json ``engine_affinity`` overrides it per kind.
4. Effort follows the role's tier, clamped to what that model supports.
   ``ultra`` is never used; ``max`` only on a ``light`` or Luna model (the
   cheapest model is the one worth running at its maximum); flagship
   models (:data:`FLAGSHIP_FAMILIES`) never above ``high``. This rule is
   for automatic worker routing only: the editor's own effort selector
   offers every effort the engine reports, and the coordinator runs at the
   effort chosen there.
   The Codex Luna line (:data:`LUNA_LINES`) is cheap but strong, so it has
   its own rule: never below ``high`` (:data:`LUNA_EFFORT_FLOOR`, also for
   a pin or a worker's requested effort), and its effort follows how hard
   the work is (:data:`LUNA_EFFORTS`): ``high`` for light work, ``xhigh``
   for ordinary standard-tier testing, ``max`` for work above that --
   standard-tier testing that is a retry or a fix after review requested
   changes (``escalate``) -- which still does not justify a deep model.
   Besides the light tier, Luna is a candidate only for standard-tier
   testing work (:data:`LUNA_WORK_KINDS`) next to the tier's own models,
   and the rank decides between them (:func:`luna_status`). Every other
   standard-tier kind of work stays on the tier's own models. It is
   never used for ``complexity`` ``high`` work, deep or flagship work, or a
   deep role, not even as a fallback; a ``tiers`` override that moves a
   Luna model out of the light tier removes it from this rule except for
   its effort floor. Hard-coded rather than configured: routing.json names
   tiers per model, not per work kind or complexity.
5. The work moves the role's tier: the coordinator's planned
   ``complexity`` (``low``/``high``) shifts it one step cheaper/stronger,
   and a retried attempt or a fix after review requested changes runs one
   more step up (``escalate``). Standard testing instead stays in the
   standard candidate pool: Luna targets max, other models raise effort
   one step. Other standard work, explicit high complexity and deep roles
   still move up. If a
   tier cannot rise (explorer), effort rises one supported step.
6. Flagship is otherwise reached only by escalation. The one exception is
   the user's explicit request in the task: an exact flagship
   ``model_hint`` whose family the user explicitly asked for, not merely
   mentioned (:func:`user_model_request`, :func:`requests_family`)
   runs on that route at the flagship tier, still subject to pin, deny,
   providers, availability and quota (basis ``auto:user_request``).

Only engine-confirmed catalogs are used -- a connection whose models were
never observed (seed catalog) is not routed to automatically; the worker
then stays on the coordinator's route, as before.

Optional overrides, first existing file wins (never merged):
``<project>/.cortex/routing.json``, ``$CORTEX_ROUTING``,
``$XDG_CONFIG_HOME/cortex/routing.json`` (``~/.config/...``)::

    {
      "pin":       {"security-auditor": "claude-agent-acp/opus:high", "*": "codex-acp/gpt-5.6-luna"},
      "deny":      ["gpt-5.5", "haiku"],
      "auto_allow": ["claude-agent-acp/haiku"],
      "tiers":     {"gpt-5.6-luna": "light"},
      "providers": ["claude-agent-acp", "codex-acp"],
      "engine_affinity": {"implementation": "claude-agent-acp", "testing": null, "*": "codex-acp"}
    }

``pin`` is strict: a pinned route that is not available (not offered, or
recorded unavailable) refuses the delegation rather than substituting
another one; a pin may name an older, superseded version on purpose.
``deny`` also applies to explicitly selected routes and to the model
selector. A ``deny``/``auto_allow`` token is an exact model id, a versioned
id that also covers its variants (``gpt-5.5`` covers ``gpt-5.5[1m]``), or a
bare family name that covers every version and alias of that family
(``haiku`` covers ``haiku`` and ``claude-haiku-4-5-20251001``); an
``engine/`` prefix restricts it to that engine. Setup writes the default
global file (:mod:`routing.defaults`).
The built-in Haiku exclusion applies only to automatic worker selection;
a matching ``pin`` or ``auto_allow`` can opt into Haiku, while ``deny`` still wins.

``engine_affinity`` maps a work kind (a key of :data:`WORK_KIND_ENGINES`)
or ``"*"`` to an engine id, or ``null`` for no lean; a named kind wins,
then ``"*"``, then the built-in default. It only breaks ties after quota
and plan usage and never overrides a pin, deny, providers or availability.
Unknown kinds or engines are refused.

``providers`` restricts automatic routing and failover to connections whose
engine is in the list (each id must be one of
:data:`connections.model.SUPPORTED_ENGINES`); when absent, every configured
engine is still considered. A route ``deny``'d or excluded by ``providers``
is treated the same way by the editor's selector. A ``pin`` naming an engine
outside ``providers`` is refused (``pinned_route_unavailable``) rather than
silently ignored, and if no allowed provider has a usable route the existing
``no_available_route`` refusal applies, naming the excluded providers too.
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

ENV_VAR = "CORTEX_ROUTING"
HOT_PERCENT = 85.0
TIERS = ("flagship", "deep", "standard", "light")
_FALLBACK_ORDER = {
    "flagship": ("flagship", "deep", "standard", "light"),
    "deep": ("deep", "standard", "light"),
    "standard": ("standard", "deep", "light"),
    "light": ("light", "standard", "deep"),
}
EFFORT_BY_TIER = {"flagship": "high", "deep": "high", "standard": "medium", "light": "low"}
# Claude lists current models first and older generations after them, so
# its list order is not strength order; its family names are.
FAMILY_TIERS = {"opus": "deep", "fable": "flagship", "sonnet": "standard", "haiku": "light"}
# Codex lists its newest generation first (gpt-6-luna before gpt-5.6-sol),
# so its list order is not strength order either. Astra > Sol > Terra >
# Luna within a generation; gpt-5.5 is the oldest and cheapest.
KNOWN_MODEL_TIERS = {
    "gpt-6-astra": "flagship", "gpt-6-sol": "deep",
    "gpt-5.6-sol": "standard", "gpt-5.6-terra": "standard",
    "gpt-6-luna": "light", "gpt-5.6-luna": "light", "gpt-5.5": "light",
}
# A Codex line (the family name after the version) keeps its tier across
# generations, so a newer release no table names (gpt-7-luna) still lands
# in its line's tier rather than by list position.
LINE_TIERS = {"astra": "flagship", "sol": "deep", "terra": "standard", "luna": "light"}
EFFORT_ORDER = ("low", "medium", "high", "xhigh", "max", "ultra")
_NEVER_EFFORTS = frozenset({"ultra"})
_LIGHT_ONLY_EFFORTS = frozenset({"max"})
# The strongest models are already strong enough at high; above it they
# only cost more. Named because no engine reports "flagship" itself.
FLAGSHIP_FAMILIES = ("astra", "fable")
_FLAGSHIP_CEILING = "high"
# The Codex Luna line is very cheap and very effective: below high its
# answers are not worth even their small cost, and at high..max it handles
# light work and standard-tier testing. Its effort steps with the work:
# light work, standard-tier testing, standard-tier testing that fell short once.
LUNA_LINES = frozenset({"luna"})
LUNA_EFFORT_FLOOR = "high"
LUNA_EFFORTS = ("high", "xhigh", "max")
# The only standard-tier work Luna may take. Implementation, operations,
# debugging, design, review, planning, writing and analysis stay on the
# tier's own models.
LUNA_WORK_KINDS = frozenset({"testing"})
# Haiku repeatedly submitted arguments that contradicted advertised Cortex
# tool schemas in real worker sessions. Keep it available for an explicit pin,
# while excluding it from automatic worker selection.
DEFAULT_AUTO_DENY_FAMILIES = frozenset({"haiku"})

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


def work_kind(role: str | None) -> str | None:
    return None if role is None else ROLE_WORK_KINDS.get(role, "implementation")


def is_luna(model: str) -> bool:
    """Whether ``model`` is of the Codex Luna line (any generation)."""
    return model_line(model)[0] in LUNA_LINES


def luna_status(role: str | None, role_tier: str, complexity: str = "normal") -> str:
    """Whether a light-tier Luna model may take this work, and why:
    ``light_tier`` (its own tier), ``standard_candidate`` (standard-tier
    testing work, ranked with the tier's own models), or
    ``excluded:<reason>``. ``excluded`` is absolute -- not even a fallback."""
    if role_tier == "light":
        return "light_tier"
    if complexity == "high":
        return "excluded:complexity_high"
    if role_tier != "standard":
        return f"excluded:{role_tier}_work"
    if ROLE_TIERS.get(role or "", "standard") in ("deep", "flagship"):
        return "excluded:deep_role"
    kind = work_kind(role)
    if kind not in LUNA_WORK_KINDS:
        return f"not_candidate:work_kind_{kind}"
    return "standard_candidate"


def hint_serves_tier(model: str, model_tier: str | None, role: str | None, role_tier: str,
                     complexity: str = "normal") -> bool:
    """Whether a model of ``model_tier`` can take ``role_tier`` work: its own
    tier, or a light Luna model on standard-tier testing work."""
    return model_tier == role_tier or (
        model_tier == "light" and is_luna(model)
        and luna_status(role, role_tier, complexity) == "standard_candidate")


def luna_effort(role_tier: str, escalate: bool = False) -> str:
    """Luna's target effort: one :data:`LUNA_EFFORTS` step per tier above
    light, and one more for a retry or a fix after requested changes."""
    step = (0 if role_tier == "light" else 1) + int(escalate)
    return LUNA_EFFORTS[min(step, len(LUNA_EFFORTS) - 1)]


def affinity_engine(role: str | None, affinity: Mapping[str, str | None] | None = None) -> str | None:
    """The engine ``role``'s kind of work leans to, or ``None`` (neutral).
    ``affinity`` is routing.json ``engine_affinity``: a kind it names wins,
    then its ``"*"``, then :data:`WORK_KIND_ENGINES`."""
    kind = work_kind(role)
    if kind is None:
        return None
    affinity = affinity or {}
    if kind in affinity:
        return affinity[kind]
    if "*" in affinity:
        return affinity["*"]
    return WORK_KIND_ENGINES[kind]


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
# Plan usage ranks candidates in steps of this many percentage points above
# the least-used candidate: a smaller difference is not a meaningful
# difference in remaining quota, so the task (engine affinity) decides it.
USAGE_MARGIN = 10.0


def route_menu(project_root=None, environ=None, unavailable=()):
    """Current engine-confirmed routes; dispatch revalidates every entry."""
    from ..connections import quota
    from ..connections import registry as connection_registry
    from . import availability
    from ..portable.roles import WORKER_ALIASES
    overrides = load_overrides(project_root, environ)
    connections = connection_registry.read_registry(environ).connections
    entries = [e for e in _allowed(overrides, _entries(overrides, connections, environ))
               if not _is_seed(e)]
    tiers = model_tiers(entries, overrides.get("tiers") or {})
    superseded = superseded_routes(entries)
    result = {tier: [] for tier in TIERS}
    for entry in entries:
        engine, model = entry.route.engine, entry.route.model
        used = quota.used_percent(connections[entry.connection_id], model, environ)
        blocked = availability.blocking(unavailable, entry.connection_id, model)
        excluded = _default_auto_denied(overrides, entry)
        result[tiers[entry.route.id]].append(dict(
            route=_route_label(entry), engine=engine, model=model,
            connection_id=entry.connection_id,
            efforts=list(permitted_efforts(entry, tiers[entry.route.id])),
            state=(f"unavailable_until:{blocked['expires_at']}" if blocked else
                   "exhausted" if used is not None and used >= 100 else "available"),
            auto_eligible=not excluded and entry.route.id not in superseded,
            superseded_by=superseded.get(entry.route.id),
            quota=dict(band="unknown" if used is None else "hot" if used >= HOT_PERCENT else "cool",
                       used_percent=used)))
    roles = sorted(role for role in set(WORKER_ALIASES.values()) if role.startswith("worker-")
                   and role not in {"worker-code-reviewer", "worker-build-verification"})
    return dict(tiers=result, roles={role: {complexity: tier_for(role, complexity=complexity)
                                          for complexity in ("low", "normal", "high")}
                                     for role in roles},
                flagship_note="escalation, or an exact route the user explicitly requested in this task",
                luna=dict(effort_floor=LUNA_EFFORT_FLOOR,
                          efforts=dict(light=luna_effort("light"), standard=luna_effort("standard"),
                                       standard_escalated=luna_effort("standard", True)),
                          standard_candidate_roles=[role for role in roles
                                                    if luna_status(role, "standard") == "standard_candidate"],
                          never="complexity high, deep or flagship work, deep roles"))


def choose(role: str, project_root=None, environ: Mapping[str, str] | None = None, *,
           escalate: bool = False, complexity: str = "normal", unavailable: Sequence[Mapping] = (),
           preference: str | None = None, model_hint: str = "auto",
           model_reason: str | None = None, previous_engine: str | None = None,
           user_request: Mapping | None = None) -> Choice | None:
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

    ``user_request`` (:func:`user_model_request`) is the
    user's own request, in this task, for the family of an exact
    ``model_hint``. When that hinted route is a flagship model it runs at
    the flagship tier even without escalation (basis ``auto:user_request``);
    a pin still wins, and a denied, excluded, superseded, unavailable or
    exhausted requested route is refused with its reason in
    ``decision["user_model_request"]`` while the node is routed as usual.
    Without a user request a flagship hint stays escalation-only."""
    from ..connections import quota
    from ..connections import registry as connection_registry
    from . import availability

    overrides = load_overrides(project_root, environ)
    connections = connection_registry.read_registry(environ).connections
    entries = [entry for entry in _entries(overrides, connections, environ) if not _is_seed(entry)]
    allowed = _allowed(overrides, entries)
    tiers = model_tiers(allowed, overrides.get("tiers") or {})
    usage_cache: dict = {}

    def usage(entry):
        key = (entry.connection_id, entry.route.model)
        if key not in usage_cache:
            usage_cache[key] = quota.used_percent(connections[entry.connection_id], entry.route.model, environ)
        return usage_cache[key]

    blocked = {}
    for entry in entries:
        record = availability.blocking(unavailable, entry.connection_id, entry.route.model)
        if record is not None:
            blocked[entry.route.id] = record
    base_role_tier = tier_for(role, False, complexity)
    role_tier = tier_for(role, escalate, complexity)
    # A standard testing retry is the intermediate Luna-max band.
    # Decide from the original work before generic escalation labels it deep.
    # Explicit high complexity and deep roles still fail luna_status here.
    if escalate and luna_status(role, base_role_tier, complexity) == "standard_candidate":
        role_tier = base_role_tier
    affinity = overrides.get("engine_affinity") or {}
    # Light-tier Luna models follow their own rule (see luna_status); a
    # tiers override that moves one elsewhere routes it like any model.
    luna_ids = frozenset(e.route.id for e in allowed if is_luna(e.route.model) and tiers[e.route.id] == "light")
    luna = luna_status(role, role_tier, complexity) if luna_ids else "not_offered"
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
        matches = [e for e in entries if e.route.engine == engine and e.route.model == model]
        if not matches:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="pinned_route_unavailable",
                remediation=(f"routing.json pins role {role!r} to {engine}/{model}, which no configured "
                             "connection currently offers; connect it or change the pin."),
                retry_eligible=False, field="pin",
            )
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
            luna=dict(status="pinned" if best.route.id in luna_ids else luna, chosen=best.route.id in luna_ids))
        if user_request is not None:
            decision["user_model_request"] = dict(user_request, status="ignored:pinned")
        default_effort = (luna_effort(role_tier, escalate) if best.route.id in luna_ids
                          else EFFORT_BY_TIER[tier_for(role, escalate, complexity)])
        return Choice(best.connection_id, model,
                      nearest(effort or default_effort, permitted_efforts(best, tiers[best.route.id])),
                      "auto:pin", decision)
    default_denied = {entry.route.id for entry in allowed if _default_auto_denied(overrides, entry)}
    luna_denied = luna_ids if luna.startswith("excluded:") else frozenset()
    superseded = superseded_routes(allowed)
    providers = overrides.get("providers")
    # A flagship route the user asked for in this task (see user_model_request).
    requested, request_status = _user_flagship(model_hint, user_request, overrides, allowed, tiers,
                                               frozenset(blocked), superseded, usage)
    if requested:
        picked, decision = rank("flagship", requested, usage, overrides.get("tiers") or {},
                                role=role, model_hint=model_hint, model_reason=model_reason,
                                all_entries=entries, providers=providers, affinity=affinity)
        decision.update(
            base_role_tier=base_role_tier, role_tier="flagship", basis="user_request",
            decided_by="user_request", escalated=escalate, preference=preference,
            preference_status="ignored:user_request" if preference else "none",
            effort_basis="user_request:flagship",
            user_model_request=dict(user_request, status=request_status))
        effort = nearest(EFFORT_BY_TIER["flagship"], permitted_efforts(picked, tiers[picked.route.id]))
        return Choice(picked.connection_id, picked.route.model, effort, "auto:user_request", decision)
    status = None
    if preference is not None and providers is not None and preference not in providers:
        status = "ignored:provider_not_allowed"
    elif preference is not None and not any(e.route.engine == preference for e in entries):
        status = "ignored:provider_not_configured"
    picked, decision = rank(role_tier, allowed, usage, overrides.get("tiers") or {},
                            excluded=frozenset(blocked) | default_denied | frozenset(superseded) | luna_denied,
                            policy_excluded=frozenset(default_denied) | frozenset(superseded) | luna_denied,
                            superseded=superseded,
                            luna_standard=luna_ids if luna == "standard_candidate" else frozenset(),
                            luna_status=luna,
                            role=role, preference=None if status else preference,
                            model_hint=model_hint, model_reason=model_reason,
                            escalated=escalate, previous_engine=previous_engine,
                            all_entries=entries, providers=providers, affinity=affinity)
    if decision is not None and status:
        decision.update(preference=preference, preference_status=status)
    if decision is not None and request_status is not None:
        decision["user_model_request"] = dict(user_request or {}, status=request_status)
    if picked is None:
        if not entries:
            # Nothing engine-confirmed at all -- unrelated to 'providers'.
            return None
        if not allowed:
            # 'providers' excluded every engine-confirmed route: fail closed
            # rather than silently falling back to the coordinator's route,
            # which could be on an excluded provider.
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="no_available_route",
                remediation=(f"'providers' in routing.json restricts routing to {sorted(overrides.get('providers'))}, "
                             "but no configured connection offers any of them. Connect one of those providers, "
                             "or widen 'providers' in routing.json."),
                retry_eligible=False,
            )
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="no_available_route",
            remediation=("No configured route can take this work now: "
                         + "; ".join(_unavailable_reasons(allowed, blocked, usage)
                                     + (["Claude Haiku is excluded from automatic routing; pin it explicitly "
                                         "in routing.json to override this default"] if default_denied else [])
                                     + (["older versions superseded by a newer model are not routed "
                                         "automatically (" + ", ".join(sorted(
                                             f"{_route_label(e)} -> {superseded[e.route.id]}"
                                             for e in allowed if e.route.id in superseded))
                                         + "); pin one in routing.json to use it"] if superseded else [])
                                     + ([f"Luna is not used for this work ({luna.partition(':')[2]})"]
                                        if luna_denied else []))
                         + ". Wait for a limit to reset, connect another account, or change routing.json."),
            retry_eligible=False,
        )
    effort_target = EFFORT_BY_TIER[role_tier]
    effort_basis = f"role_tier:{role_tier}"
    if picked.route.id in luna_ids:
        effort_target = luna_effort(role_tier, escalate)
        effort_basis = f"luna:{role_tier}" + (":escalated" if escalate else "")
    elif escalate and tier_for(role, False, complexity) == role_tier:
        effort_target = EFFORT_ORDER[min(EFFORT_ORDER.index(effort_target) + 1,
                                         EFFORT_ORDER.index("high"))]
        effort_basis = "escalation_step"
    effort = nearest(effort_target, permitted_efforts(picked, tiers[picked.route.id]))
    decision["base_role_tier"] = base_role_tier
    decision["effort_basis"] = effort_basis
    return Choice(picked.connection_id, picked.route.model, effort, "auto", decision)


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
         affinity: Mapping[str, str | None] | None = None):
    """The best entry for ``role_tier`` (see :func:`rank`), or ``None``."""
    return rank(role_tier, entries, usage, tier_overrides, excluded, role=role, preference=preference,
                affinity=affinity)[0]


# 2: quota band, recent-count balance (tolerance 4), suitability.
# 3: plus plan usage, engine affinity and cross-tier stand-ins (tolerance 2).
# 4: no attempt counts and no cross-tier stand-ins -- the task and plan usage
# alone decide.
# 5: a light-tier Luna model also ranks among standard-tier development
# candidates (``luna_standard``); the decision records ``luna``.
# 6: the quota (hot) band is demoted from an early hard cut (right after
# hint) to a late tie breaker (right before used_percent), behind plan usage
# (the usage-gap signal, unchanged), retry diversification and engine
# affinity: crossing HOT_PERCENT alone no longer overrides task/model fit,
# no longer sends a diversified retry to a meaningfully-more-used plan, and
# no longer causes a near-boundary cliff. A preference or model hint at or
# above HOT_PERCENT is now ignored only when it is also meaningfully worse
# (more than USAGE_MARGIN points) than an available alternative's usage,
# rather than solely for crossing the band. The 100% exhaustion exclusion is
# unchanged.
# 7: Luna is a standard-tier candidate only for testing work; other
# standard-tier development (implementation, operations, debugging) no
# longer ranks Luna and, on a retry or review fix, escalates as before 5.
RANK_VERSION = 7
# The rank key's parts in order, named for the decision's ``decided_by``.
# ``quota`` moved from right after ``hint`` to right before ``used_percent``
# in rank version 6: it is now a late tie breaker rather than an early hard
# cut, behind the usage-gap signal (``usage``), retry diversification and
# affinity (see RANK_VERSION).
RANK_KEYS = ("preference", "hint", "usage", "diversify", "affinity", "quota", "used_percent", "engine_order")


def rank(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None,
         policy_excluded: frozenset = frozenset(), superseded: Mapping[str, str] | None = None,
         model_hint: str = "auto", model_reason: str | None = None,
         escalated: bool = False, previous_engine: str | None = None,
         all_entries: Sequence | None = None, providers: Sequence[str] | None = None,
         affinity: Mapping[str, str | None] | None = None,
         luna_standard: frozenset = frozenset(), luna_status: str | None = None):
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
    2. an honoured model hint (see :data:`HINT_TOLERANCE`), ignored on the
       same hot-and-meaningfully-worse basis;
    3. subscription plan usage, only if every candidate's usage is known:
       each full :data:`USAGE_MARGIN` points above the least-used candidate
       ranks a candidate one step behind (``usage_band``) -- this usage-gap
       signal, not the quota band below, is what usually separates
       candidates on remaining quota;
    4. on escalation, an engine other than the one that fell short (a large
       usage gap above still wins first: diversification does not send a
       retry to a plan that is meaningfully more used);
    5. engine affinity (:func:`suitability`), the task's fit;
    6. the quota band, as a late tie breaker only (unknown is not hot): once
       plan usage and affinity leave candidates tied, one at or above
       :data:`HOT_PERCENT` ranks behind one below it. Crossing the band
       alone no longer overrides plan usage or affinity, and no longer
       causes a cliff at the boundary;
    7. lower exact used percent, only if every candidate's usage is known;
    8. engine order.

    How many attempts each engine received before never enters the rank:
    it says nothing about which engine fits the work.

    ``decision`` records the tier, every candidate of it in rank order with
    what ranked it, the preference and whether it was honoured, and
    ``decided_by``: the first key that separated the chosen candidate from
    the runner-up (``only_candidate`` when there was none).
    ``superseded`` maps an older route id to the newer route that replaces
    it (see :func:`superseded_routes`); those ids should also be in
    ``excluded`` and ``policy_excluded``. ``luna_standard`` route ids (light
    Luna models, see :func:`luna_status`) are also candidates of the
    standard tier; ``decision["luna"]`` records ``luna_status`` and, when a
    Luna model was ranked but not chosen, the key that ranked it behind."""
    superseded = superseded or {}
    tiers = model_tiers(entries, tier_overrides or {})
    usable = [e for e in entries if e.route.id not in excluded and (usage(e) or 0.0) < 100.0]
    order = {e.route.id: index for index, e in enumerate(entries)}

    def serves(e, tier):
        return tiers[e.route.id] == tier or (tier == "standard" and e.route.id in luna_standard)

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
        least_used = min(usage(e) for e in candidates) if used_percent_compared else None

        def usage_band(e):
            return int((usage(e) - least_used) // USAGE_MARGIN) if used_percent_compared else None

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

        def key(e):
            return (e.route.engine != honoured,
                    not (hint_status == "honoured" and e in hinted),
                    usage_band(e) or 0,
                    not (escalated and previous_engine and e.route.engine != previous_engine),
                    -suitability(role, e.route.engine, affinity),
                    _hot(usage(e)),
                    usage(e) if used_percent_compared else 0.0, order[e.route.id])

        ranked = sorted(candidates, key=key)
        best = ranked[0]
        if len(ranked) == 1:
            decided_by = "only_candidate"
        else:
            first, second = key(best), key(ranked[1])
            decided_by = next(name for name, a, b in zip(RANK_KEYS, first, second) if a != b)
        best_luna = next((e for e in ranked if is_luna(e.route.model)), None)
        affinity_kind = work_kind(role)
        decision = dict(
            role=role, role_tier=role_tier, tier=tier, basis="rank", rank_version=RANK_VERSION,
            preference=preference, preference_status=status,
            model_hint=model_hint, model_hint_status=hint_status,
            model_hint_reason=model_reason, diversify_from=previous_engine if escalated else None,
            usage=dict(compared=used_percent_compared, margin=USAGE_MARGIN, least_used_percent=least_used),
            affinity=dict(work_kind=affinity_kind, engine=affinity_engine(role, affinity),
                          source=("routing.json" if affinity_kind is not None and affinity
                                  and (affinity_kind in affinity or "*" in affinity) else "default")),
            used_percent_compared=used_percent_compared,
            candidates=[_candidate(e, usage, role, e.route.engine == honoured, usage_band(e),
                                   affinity=affinity) for e in ranked],
            unavailable=sorted(_route_label(e) for e in entries
                               if serves(e, tier) and e.route.id not in policy_excluded
                               and (e.route.id in excluded or (usage(e) or 0.0) >= 100.0)),
            excluded_by_policy=sorted(_route_label(e) for e in entries
                                      if tiers[e.route.id] == tier and e.route.id in policy_excluded
                                      and e.route.id not in superseded),
            superseded={_route_label(e): superseded[e.route.id] for e in entries if e.route.id in superseded},
            decided_by=decided_by,
            luna=dict(status=luna_status, candidate=best_luna is not None, chosen=best_luna is best,
                      outranked_by=(None if best_luna is None or best_luna is best else
                                    next(name for name, a, b in zip(RANK_KEYS, key(best), key(best_luna))
                                         if a != b))),
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        return best, decision
    return None, None


def _user_flagship(model_hint: str, user_request: Mapping | None, overrides: Mapping, allowed: Sequence,
                   tiers: Mapping[str, str], blocked: frozenset, superseded: Mapping[str, str], usage):
    """``(entries, status)`` for an exact flagship ``model_hint``: the usable
    entries of the route the user asked for (status ``honoured``), or none
    and why not. ``status`` is ``None`` when the hint is not an exact route,
    or names a route of another tier the user did not ask for."""
    engine, slash, model = model_hint.partition("/")
    if not slash:
        return (), None
    offered = [e for e in allowed if e.route.engine == engine and e.route.model == model]
    flagship = bool(offered) and all(tiers[e.route.id] == "flagship" for e in offered)
    if user_request is None:
        return (), "not_requested_by_user" if flagship else None
    if _denied(overrides, engine, model):
        return (), "refused:denied"
    providers = overrides.get("providers")
    if providers is not None and engine not in providers:
        return (), "refused:provider_not_allowed"
    if not offered:
        return (), "refused:not_offered"
    if not flagship:
        return (), "not_flagship"
    current = [e for e in offered if e.route.id not in superseded]
    if not current:
        return (), "refused:superseded"
    open_routes = [e for e in current if e.route.id not in blocked]
    if not open_routes:
        return (), "refused:unavailable"
    usable = [e for e in open_routes if (usage(e) or 0.0) < 100.0]
    if not usable:
        return (), "refused:quota_exhausted"
    return usable, "honoured"


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


def model_tiers(entries: Sequence, tier_overrides: Mapping[str, str]) -> dict[str, str]:
    tiers = {}
    by_connection: dict[str, list] = {}
    for entry in entries:
        model = entry.route.model
        family = (next((tier for name, tier in FAMILY_TIERS.items() if name in model.casefold()), None)
                  or KNOWN_MODEL_TIERS.get(model.casefold())
                  or LINE_TIERS.get(model_line(model)[0]))
        if tier_overrides.get(model) or family:
            tiers[entry.route.id] = tier_overrides.get(model) or family
        else:
            by_connection.setdefault(entry.connection_id, []).append(entry)
    for group in by_connection.values():
        for index, entry in enumerate(group):
            tiers[entry.route.id] = ("deep", "standard", "light")[min(2, index * 3 // len(group))]
    return tiers


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
    return (_hot(used), used or 0.0)


def permitted_efforts(entry, tier: str) -> tuple[str, ...]:
    """What ``entry``'s model offers, weakest first, minus ``ultra``,
    ``max`` unless the model is ``light`` or Luna, anything above ``high``
    on a flagship model, and anything below :data:`LUNA_EFFORT_FLOOR` on a
    Luna model that offers the floor or more."""
    offered = {value for name, value in entry.route.supported_parameters if name == "thought_level"}
    flagship = any(family in entry.route.model.casefold() for family in FLAGSHIP_FAMILIES)
    luna = is_luna(entry.route.model)
    ceiling = EFFORT_ORDER.index(_FLAGSHIP_CEILING) if flagship else len(EFFORT_ORDER)
    permitted = tuple(
        value for index, value in enumerate(EFFORT_ORDER)
        if value in offered and value not in _NEVER_EFFORTS and index <= ceiling
        and (tier == "light" or luna or value not in _LIGHT_ONLY_EFFORTS)
    )
    if luna:
        floor = EFFORT_ORDER.index(LUNA_EFFORT_FLOOR)
        return tuple(value for value in permitted if EFFORT_ORDER.index(value) >= floor) or permitted
    return permitted


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

    try:
        overrides = load_overrides(project_root, environ)
    except PortableError:
        overrides = {}
    entries = _entries(overrides, connection_registry.read_registry(environ).connections, environ)
    tiers = model_tiers(entries, overrides.get("tiers") or {})
    match = next((e for e in entries if e.route.engine == engine and e.route.model == model), None)
    return None if match is None else permitted_efforts(match, tiers[match.route.id])


def _entries(overrides, connections, environ) -> list:
    """Launchable, non-denied catalog entries, in each engine's own order.
    ``providers`` is deliberately not applied here (see :func:`_allowed`):
    it is a stricter, fail-closed restriction, not a silent narrowing like
    ``deny``."""
    from ..acp.config_options import presented_entries, selectable_entries_from_snapshots

    return [
        entry for entry in presented_entries(selectable_entries_from_snapshots(connections, environ))
        if getattr(connections.get(entry.connection_id), "launch_command", None) is not None
        and not _denied(overrides, entry.route.engine, entry.route.model)
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
            remediation=f"{engine}/{model} is listed under 'deny' in routing.json.",
            retry_eligible=False, field="model",
        )


def is_denied(engine: str, model: str, project_root=None, environ=None) -> bool:
    try:
        return _denied(load_overrides(project_root, environ), engine, model)
    except PortableError:
        return False


def _listed(values: Sequence[str], engine: str, model: str) -> bool:
    """Whether a ``deny``/``auto_allow`` token names ``engine``/``model``:
    an exact id, a versioned id covering its variants (``gpt-5.5`` covers
    ``gpt-5.5[1m]``), or a bare family name covering every version and
    alias (``haiku`` covers ``claude-haiku-4-5-20251001``). ``engine/``
    restricts a token to that engine."""
    if model in values or f"{engine}/{model}" in values:
        return True
    family, version = model_line(model)
    for value in values:
        token_engine, slash, token = value.partition("/")
        if not slash:
            token_engine, token = None, value
        if token_engine is not None and token_engine != engine:
            continue
        token = token.casefold()
        if re.fullmatch(r"[a-z]+", token):
            if token == family:
                return True
        elif "[" not in token and version is not None and model_line(token) == (family, version):
            return True
    return False


def _default_auto_denied(overrides: Mapping, entry) -> bool:
    """The built-in Haiku exclusion from automatic selection (``auto_allow``
    lifts it; ``deny`` still wins)."""
    engine, model = entry.route.engine, entry.route.model
    return (engine == "claude-agent-acp"
            and any(family in model.casefold() for family in DEFAULT_AUTO_DENY_FAMILIES)
            and not _listed(overrides.get("auto_allow") or (), engine, model))


# --- model versions ----------------------------------------------------------

_VENDOR_PREFIXES = frozenset({"claude", "gpt"})
_VARIANT = re.compile(r"\[[^\]]*\]")
_NUMBER = re.compile(r"\d+(?:\.\d+)*")
_BUILD_DATE = re.compile(r"\d{6,}")


def _parse_tokens(text: str) -> tuple[list[str], tuple[int, ...] | None]:
    tokens = [t for t in re.split(r"[-_\s()]+", _VARIANT.sub("", text.casefold())) if t]
    words = [t for t in tokens if not _NUMBER.fullmatch(t)]
    parts = [int(p) for t in tokens if _NUMBER.fullmatch(t) and not _BUILD_DATE.fullmatch(t)
             for p in t.split(".")]
    while parts and parts[-1] == 0 and len(parts) > 1:
        parts.pop()
    return words, (tuple(parts) or None)


def model_line(model: str, name: str | None = None) -> tuple[str, tuple[int, ...] | None]:
    """``(family, version)`` parsed from an engine's model id, e.g.
    ``gpt-5.6-luna`` -> ``("luna", (5, 6))``, ``claude-opus-4-8`` ->
    ``("opus", (4, 8))``, ``claude-haiku-4-5-20251001`` -> ``("haiku",
    (4, 5))`` (a build date is not a version). A vendor prefix (``gpt``,
    ``claude``) is not a family unless nothing else is (``gpt-5.5`` ->
    ``("gpt", (5, 5))``); a context variant such as ``[1m]`` is ignored.
    An alias without a version in its id (``opus[1m]``) takes it from the
    engine's display ``name`` ("Opus 5.5") when that names the same family.
    The version is ``None`` when neither says."""
    words, version = _parse_tokens(model)
    family_words = [w for w in words if w not in _VENDOR_PREFIXES] or words
    family = "-".join(family_words)
    if version is None and name and family:
        name_words, name_version = _parse_tokens(name)
        if all(w in name_words for w in family_words):
            version = name_version
    return family, version


def superseded_routes(entries: Sequence) -> dict[str, str]:
    """Route id -> the newer route that supersedes it: among one engine's
    entries of the same family (:func:`model_line`), every version below the
    newest one. Callers pass entries with ``deny`` already applied, so the
    newest non-denied version wins; a model with no parseable version is
    never superseded and never supersedes."""
    lines: dict[tuple[str, str], list] = {}
    for entry in entries:
        family, version = model_line(entry.route.model, getattr(entry, "model_name", None))
        if family and version is not None:
            lines.setdefault((entry.route.engine, family), []).append((version, entry))
    result = {}
    for group in lines.values():
        newest = max(version for version, _ in group)
        for version, entry in group:
            if version < newest:
                successor = next((e for v, e in group if v == newest and e.connection_id == entry.connection_id),
                                 next(e for v, e in group if v == newest))
                result[entry.route.id] = _route_label(successor)
    return result


def _denied(overrides: Mapping, engine: str, model: str) -> bool:
    return _listed(overrides.get("deny") or (), engine, model)


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


def load_overrides(project_root=None, environ: Mapping[str, str] | None = None) -> dict:
    """The first existing ``routing.json``, validated, or ``{}``. A
    malformed file refuses routing -- it never silently means "none"."""
    for path in _candidate_paths(project_root, environ):
        if not path.is_file():
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            _validate(data)
        except (OSError, ValueError) as error:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_routing_overrides",
                remediation=f"Repair {path}: {error}", retry_eligible=False,
            ) from error
        return data
    return {}


def _validate(data) -> None:
    from ..connections.model import SUPPORTED_ENGINES

    if not isinstance(data, dict):
        raise ValueError("must contain a JSON object")
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


def _candidate_paths(project_root, environ) -> list[Path]:
    environ = os.environ if environ is None else environ
    home = Path(environ.get("HOME") or str(Path.home()))
    paths = [Path(project_root) / ".cortex" / "routing.json"] if project_root else []
    if environ.get(ENV_VAR):
        paths.append(Path(environ[ENV_VAR]))
    xdg = environ.get("XDG_CONFIG_HOME") or ""
    paths.append((Path(xdg) if xdg.startswith("/") else home / ".config") / "cortex" / "routing.json")
    return paths


# --- a model family the user asked for ---------------------------------------
#
# A flagship model is otherwise reserved for escalation. The coordinator may
# still route one node to an exact flagship route when the *user* explicitly
# asked for that family in the same task. The plan hint only names the route;
# the grant (``user_model_request``) comes from user-authored text the runtime
# recorded, never from coordinator prose, ``model_reason``, a question the
# model wrote, a report or worker output:
#
# * every ``source_revisions`` body of the task (the initial request and each
#   later user message or steer the host delivered), and
# * the verbatim ``answer_text`` of every question a real user answer resolved
#   (``acp.questions_store.answered_questions``) -- never the question text,
#   which the asking model wrote.
#
# A source *requests* the family (not merely mentions it) when it names the
# hinted model's family (:func:`model_line`: ``astra`` for ``gpt-6-astra``,
# ``fable`` for ``claude-fable-5-1[1m]``) as a word, case-insensitively, or
# by one of its Russian spellings (:data:`CYRILLIC_FAMILY_NAMES`), and in the
# same sentence, within :data:`REQUEST_WINDOW` words before it, either
#
# * an explicit request word stands (:data:`REQUEST_WORDS`: "use Astra",
#   "switch the review to Fable", "используй астру", "требование
#   использования астры"), or
# * a work verb (:data:`WORK_VERBS`) governs a preposition of means
#   (:data:`MEANS_WORDS`) that stands directly before the family, at most one
#   article or "model" word between (:data:`MEANS_FILLERS`): "implement it on
#   Astra", "сделай на Астре", "проверь через Фейбл".
#
# A preposition alone never requests: "it failed on Astra", "посмотри на
# Астру", "sent via Fable", "ревью через Фейбл" grant nothing. A request or
# work word counts only in a request form (:func:`_request_form`): an English
# verb must open its clause or follow a request-mood word (:data:`_REQUEST_LEAD`:
# "please", "to", "could you", "let's", ...), so a noun or a report ("the last
# run on Astra", "the tests run on Astra", "we tried to run it on Astra") is
# not a request; Russian verbs are listed only in their imperative, infinitive
# and first-person-plural forms, so past tenses, participles and verbal nouns
# ("запустили на Астре", "сделано на Астре", "использование Астры") are not
# requests either, nor is a verb right after a past tense ("пытались
# запустить на Астре"). A negation, comparison or substitution word
# (:data:`NEGATION_WORDS`) within the wider :data:`NEGATION_WINDOW` refuses the
# mention ("не используй астру", "don't use Astra", "compare Astra with
# Fable", "instead of Astra"). A bare mention -- a comparison, an availability
# remark ("Astra is unavailable; use the default route"), a quotation or
# report ("the last report said Astra ...") or a preference for the default
# route -- grants nothing. This is a deterministic word match, not an
# interpretation, and it errs towards refusing: a missed request only leaves
# the node on its ordinary tier, with the reason in the receipt. The
# coordinator must still choose to hint the exact route, and pin, deny,
# providers, availability and quota still apply (see :func:`choose`).

# Russian spellings of family names the engines list in Latin. A family not
# listed here is still recognised by its Latin name.
CYRILLIC_FAMILY_NAMES = {
    "astra": ("астр",),
    "fable": ("фейбл", "фэйбл"),
    "mythos": ("мифос", "майтос", "митос"),
}
_RU_ENDING = r"(?:а|ы|е|у|ой|ою|и|ом|ам|ами|ах|ов)?"
# Words that request the family by themselves: verbs whose object is the model
# and nouns of requirement. Deliberately narrow: prepositions ("on", "via",
# "through", "на", "через"; see WORK_VERBS), "with"/"с", "by", "about", "let"
# and wishing or modal verbs ("want", "should", "хочу", "нужно") also
# introduce remarks, comparisons, authorship and questions, so a request
# phrased with them alone ("I want Astra") is not recognised -- the node then
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
    r"|треб(?:ование|ования|ованию|ованием|овании|ую|уем|ует|уется|уются|уй|уйте|овать)",
    re.IGNORECASE,
)
# Verbs of the work itself. They request the family only through a
# preposition of means (MEANS_WORDS) directly before it ("implement it on
# Astra", "сделай на Астре"): with the family as their object they name what
# the work is about ("review Astra's output", "проверь Астру"), and a
# preposition with no such verb is a remark ("it failed on Astra", "посмотри
# на Астру"). Russian forms as in REQUEST_WORDS.
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
MEANS_WORDS = re.compile(r"on|via|through|using|на|через|посредством|силами|помощью", re.IGNORECASE)
# At most one of these may stand between the preposition and the family
# ("on the Astra route", "на модели Астра").
MEANS_FILLERS = frozenset({"the", "a", "an", "model", "модели", "моделью"})
# An English request or work verb is in a request form when it opens its
# clause or directly follows one of these words (or an "'ll"/"'d"
# contraction). Anything else before it -- an article, a possessive, a noun,
# an auxiliary -- makes it a noun or a report: "the last run on Astra", "the
# tests run on Astra", "was run on Astra", "I use Astra daily".
_REQUEST_LEAD = frozenset({
    "please", "pls", "plz", "kindly", "just", "simply", "only", "always", "also", "then",
    "and", "now", "so", "ok", "okay", "yes", "no", "to", "can", "could", "would", "will",
    "should", "must", "may", "let's", "lets", "better",
})
# "could you run it on Astra": "you" leads only after one of these.
_ASKING_MODALS = frozenset({"can", "could", "would", "will", "please"})
# "I prefer Astra": a first-person subject leads only a verb of choice.
_CHOOSERS = frozenset({"i", "we"})
_CHOICE_WORDS = frozenset({"prefer", "pick", "choose", "select", "request", "require"})
# "to" after one of these reports an attempt or a habit, not a request
# ("we tried to run it on Astra").
_REPORTED_BEFORE_TO = frozenset({
    "tried", "wanted", "used", "failed", "managed", "attempted", "meant", "supposed",
    "was", "were", "had", "did", "went", "planned", "started", "began", "continued", "seemed",
})
# Nouns and statements of requirement need no lead ("the task requires Astra").
_UNGUARDED = frozenset({"requirement", "requires"})
_CYRILLIC = re.compile(r"[а-яё]")
# A Russian past tense ("пытались", "хотел") right before a verb makes it a report.
_RU_PAST = re.compile(r"[а-яё]+(?:л|ла|ло|ли|лся|лась|лось|лись)")
# One of these within NEGATION_WINDOW words before the family (same sentence)
# makes the mention a refusal, a comparison, a substitution or a report rather
# than a request. Deliberately broad, and read from a wider window than
# REQUEST_WORDS: a missed refusal would run a flagship model, a missed request
# only leaves the node on its ordinary tier. "no"/"нет" are not here: "No,
# use Astra" is a request, and a bare "нет Астры" has no request word anyway.
NEGATION_WORDS = re.compile(
    r"not|never|without|avoid|except|cannot|unable"
    r"|(?:do|does|did|is|are|was|were|has|have|had|ca|could|would|should|must|wo|shall|need)n'?t"
    r"|instead|rather|than|versus|vs|over|compar\w*|based|depend\w*"
    r"|не|без|никогда|кроме|нельзя|запрещ\w*|вместо|чем|сравн\w*|против|основан\w*|исходя",
    re.IGNORECASE,
)
_WORD = re.compile(r"[\w']+", re.UNICODE)
_SENTENCE_END = re.compile(r"[.!?;\n]")
# A request verb's lead is read within its own clause ("No, use Astra").
_CLAUSE_BREAK = re.compile(r"[,:()\"«»“”„—–]")
# The token the family name is attached to ("codex-acp/gpt-6-" before "astra"
# in an exact route id, an opening quote) is not a word before it.
_ATTACHED = re.compile(r"\S*\Z")
REQUEST_WINDOW = 4
NEGATION_WINDOW = 5


def family_of(model_hint: str) -> str | None:
    """The model family an exact ``engine/model`` hint names, or ``None``."""
    _engine, slash, model = model_hint.partition("/")
    if not slash or not model:
        return None
    family = model_line(model)[0]
    return family or None


def family_pattern(family: str) -> re.Pattern:
    """A case-insensitive whole-word pattern for ``family``'s Latin name and
    its Russian spellings."""
    latin = r"[-\s]?".join(re.escape(part) for part in family.split("-"))
    names = [rf"(?<![a-z0-9]){latin}s?(?![a-z0-9])"]
    names += [rf"(?<![а-яё]){re.escape(stem)}{_RU_ENDING}(?![а-яё])"
              for stem in CYRILLIC_FAMILY_NAMES.get(family, ())]
    return re.compile("|".join(names), re.IGNORECASE)


def _clause_words(sentence: str) -> list[tuple[str, tuple[str, ...]]]:
    """Each casefolded word of ``sentence`` with the words before it in its
    own clause."""
    result = []
    for clause in _CLAUSE_BREAK.split(sentence):
        words = [w for w in (token.casefold().strip("'") for token in _WORD.findall(clause)) if w]
        result.extend((word, tuple(words[:index])) for index, word in enumerate(words))
    return result


def _request_form(pattern: re.Pattern, word: str, lead: tuple[str, ...]) -> bool:
    """Whether ``word``, after ``lead`` in its clause, is a ``pattern`` word
    used as a request -- not a noun ("the last run"), a statement ("the tests
    run", "I use") or a report ("tried to run", "пытались запустить")."""
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


def _governed_means(window: list[tuple[str, tuple[str, ...]]]) -> bool:
    """A :data:`MEANS_WORDS` preposition directly before the family (at most
    one :data:`MEANS_FILLERS` word between) governed by a :data:`WORK_VERBS`
    verb in request form earlier in ``window``."""
    position = len(window) - 1
    if position >= 0 and window[position][0] in MEANS_FILLERS:
        position -= 1
    if position < 0 or not MEANS_WORDS.fullmatch(window[position][0]):
        return False
    return any(_request_form(WORK_VERBS, word, lead) for word, lead in window[:position])


def requests_family(text: str, family: str) -> str | None:
    """The first mention of ``family`` in ``text`` that is a request -- within
    :data:`REQUEST_WINDOW` words before it, in the same sentence, a
    :data:`REQUEST_WORDS` word in request form or a :data:`WORK_VERBS` verb
    governing a :data:`MEANS_WORDS` preposition right before it, and no
    :data:`NEGATION_WORDS` word within :data:`NEGATION_WINDOW` -- or ``None``
    when every mention is bare, a remark or refused."""
    for match in family_pattern(family).finditer(text):
        before = _ATTACHED.sub("", text[: match.start()]).replace("’", "'")
        words = _clause_words(_SENTENCE_END.split(before)[-1])
        if any(NEGATION_WORDS.fullmatch(word) for word, _lead in words[-NEGATION_WINDOW:]):
            continue
        window = words[-REQUEST_WINDOW:]
        if (any(_request_form(REQUEST_WORDS, word, lead) for word, lead in window)
                or _governed_means(window)):
            return match.group(0)
    return None


def _user_texts(store, db, task_id: str) -> Iterator[tuple[str, str]]:
    """``(source_ref, text)`` of every user-authored source of the task,
    newest first: source revisions, then answered questions' answers."""
    from ..acp import questions_store

    rows = db.execute(
        "SELECT sr.report_id, r.filename FROM source_revisions sr JOIN reports r ON r.id=sr.report_id "
        "WHERE sr.task_id=? ORDER BY sr.revision DESC", (task_id,),
    ).fetchall()
    for row in rows:
        try:
            text = (store._task_directory(db, task_id) / row["filename"]).read_bytes().decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        yield f"{row['report_id']}:-1", text
    for answer in reversed(questions_store.answered_questions(db, task_id)):
        if isinstance(answer.get("answer_text"), str):
            yield answer["source_ref"], answer["answer_text"]


def user_model_request(store, task_id: str, model_hint: str) -> dict | None:
    """The user's request for ``model_hint``'s family in ``task_id``:
    ``{"source_ref", "family", "matched"}`` for the newest user source that
    requests it (``source_ref`` is readable with ``read_source``), or ``None``
    when no source does, including when the family is only mentioned.
    Only an exact ``engine/model`` hint can be requested. A missing or
    unreadable source grants nothing."""
    family = family_of(model_hint or "")
    if family is None:
        return None
    with store.connection() as db:
        for source_ref, text in _user_texts(store, db, task_id):
            matched = requests_family(text, family)
            if matched is not None:
                return dict(source_ref=source_ref, family=family, matched=matched)
    return None
