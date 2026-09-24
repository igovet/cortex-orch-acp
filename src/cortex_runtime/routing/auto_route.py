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
   rank by honoured provider preference, quota band, recent same-tier engine
   balance, role suitability as a soft tiebreak, known quota percentage only
   when all candidates are known, and engine order. A project-local audit
   window and in-flight choices supply balance counts. Haiku is excluded
   by default unless routing.json opts it in with ``auto_allow``.
   Exhausted (100%) and recorded-unavailable routes never enter the rank.
   The decision records rank inputs and exclusions for later comparison.
4. Effort follows the role's tier, clamped to what that model supports.
   ``ultra`` is never used; ``max`` only on a ``light`` model (Luna-class:
   the cheapest model is the one worth running at its maximum); flagship
   models (:data:`FLAGSHIP_FAMILIES`) never above ``high``. This rule is
   for automatic worker routing only: the editor's own effort selector
   offers every effort the engine reports, and the coordinator runs at the
   effort chosen there.
5. The work moves the role's tier: the coordinator's planned
   ``complexity`` (``low``/``high``) shifts it one step cheaper/stronger,
   and a retried attempt or a fix after review requested changes runs one
   more step up (``escalate``). If a tier cannot rise (explorer), effort
   rises one step within the model's supported range.

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
      "providers": ["claude-agent-acp", "codex-acp"]
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
from typing import Mapping, Sequence

from .balance import BALANCE_WINDOW, BALANCE_TOLERANCE

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

# Which engine is the better fit for a kind of work, used among candidates
# of one tier after quota. A judgement, not a benchmark: Codex models are
# tuned for agentic terminal coding (write, run, fix), Claude models for
# reading, judging and writing prose. Unlisted roles are "implementation".
WORK_KIND_ENGINES = {
    "implementation": "codex-acp",
    "debugging": "codex-acp",
    "testing": "codex-acp",
    "operations": "codex-acp",
    "investigation": "codex-acp",
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
    basis: str  # "auto" | "auto:pin"
    # What decided it (tier, ranked candidates, the provider preference and
    # whether it was honoured) -- recorded in the attempt's route audit.
    decision: Mapping | None = None


def suitability(role: str | None, engine: str) -> int:
    """1 when ``engine`` is the better fit for ``role``'s kind of work."""
    if role is None:
        return 0
    return int(WORK_KIND_ENGINES[ROLE_WORK_KINDS.get(role, "implementation")] == engine)


HINT_TOLERANCE = 8


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
    return dict(tiers=result, roles={role: {complexity: tier_for(role, complexity=complexity)
                                          for complexity in ("low", "normal", "high")}
                                     for role in sorted(set(WORKER_ALIASES.values()))
                                     if role.startswith("worker-") and role not in
                                     {"worker-code-reviewer", "worker-build-verification"}},
                flagship_note="escalation-only")


def choose(role: str, project_root=None, environ: Mapping[str, str] | None = None, *,
           escalate: bool = False, complexity: str = "normal", unavailable: Sequence[Mapping] = (),
           preference: str | None = None, model_hint: str = "auto",
           model_reason: str | None = None, previous_engine: str | None = None,
           recent: Mapping[str, Mapping[str, int]] | None = None,
           inflight: Mapping[str, Mapping[str, int]] | None = None) -> Choice | None:
    """The route this worker role should open on, or ``None`` when nothing
    engine-confirmed is configured (keep the coordinator's route).
    See :func:`tier_for` for ``escalate`` and ``complexity``, and
    :func:`rank` for the coordinator's per-task ``preference`` (an engine
    id); a preference is never honoured over a ``pin`` or outside
    ``providers``, and never moves the tier.

    ``unavailable`` are the unexpired ``routing.availability`` records;
    a route they name is skipped. When routes are configured but none is
    usable, the delegation is refused (``no_available_route``) with every
    unavailable route named -- never the coordinator's route instead."""
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
    role_tier = tier_for(role, escalate, complexity)
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
        ranked = sorted(open_matches, key=lambda e: _order_key("deep", usage(e)))
        best = ranked[0]
        decision = dict(
            role=role, role_tier=role_tier, tier=tiers.get(best.route.id), basis="pin",
            preference=preference, preference_status="ignored:pinned" if preference else "none",
            rank_version=2, effort_basis="pin",
            candidates=[_candidate(e, usage, role, False) for e in ranked],
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        decision.update(model_hint=model_hint, model_hint_status=(
            "auto" if model_hint == "auto" else "ignored:escalated" if escalate else "ignored:pinned"),
            model_hint_reason=model_reason, diversify_from=previous_engine if escalate else None)
        return Choice(best.connection_id, model,
                      nearest(effort or EFFORT_BY_TIER[role_tier], permitted_efforts(best, tiers[best.route.id])),
                      "auto:pin", decision)
    default_denied = {entry.route.id for entry in allowed if _default_auto_denied(overrides, entry)}
    superseded = superseded_routes(allowed)
    providers = overrides.get("providers")
    status = None
    if preference is not None and providers is not None and preference not in providers:
        status = "ignored:provider_not_allowed"
    elif preference is not None and not any(e.route.engine == preference for e in entries):
        status = "ignored:provider_not_configured"
    picked, decision = rank(role_tier, allowed, usage, overrides.get("tiers") or {},
                            excluded=frozenset(blocked) | default_denied | frozenset(superseded),
                            policy_excluded=frozenset(default_denied) | frozenset(superseded),
                            superseded=superseded,
                            role=role, preference=None if status else preference,
                            model_hint=model_hint, model_reason=model_reason,
                            escalated=escalate, previous_engine=previous_engine,
                            all_entries=entries, providers=providers,
                            recent=recent, inflight=inflight)
    if decision is not None and status:
        decision.update(preference=preference, preference_status=status)
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
                                         + "); pin one in routing.json to use it"] if superseded else []))
                         + ". Wait for a limit to reset, connect another account, or change routing.json."),
            retry_eligible=False,
        )
    effort_target = EFFORT_BY_TIER[role_tier]
    effort_basis = f"role_tier:{role_tier}"
    if escalate and tier_for(role, False, complexity) == role_tier:
        effort_target = EFFORT_ORDER[min(EFFORT_ORDER.index(effort_target) + 1,
                                         EFFORT_ORDER.index("high"))]
        effort_basis = "escalation_step"
    effort = nearest(effort_target, permitted_efforts(picked, tiers[picked.route.id]))
    decision["effort_basis"] = effort_basis
    return Choice(picked.connection_id, picked.route.model, effort, "auto", decision)


_COMPLEXITY_SHIFT = {"low": 1, "normal": 0, "high": -1}


def tier_for(role: str, escalate: bool = False, complexity: str = "normal") -> str:
    """Return an economical role tier.

    Complexity moves ordinary work only between light, standard and deep.
    ``flagship`` is reserved for an explicit escalation after an earlier
    attempt falls short; explorer stays light even then.
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
         recent: Mapping[str, Mapping[str, int]] | None = None,
         inflight: Mapping[str, Mapping[str, int]] | None = None):
    """The best entry for ``role_tier`` (see :func:`rank`), or ``None``."""
    return rank(role_tier, entries, usage, tier_overrides, excluded, role=role, preference=preference,
                recent=recent, inflight=inflight)[0]


def rank(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None,
         policy_excluded: frozenset = frozenset(), superseded: Mapping[str, str] | None = None,
         model_hint: str = "auto", model_reason: str | None = None,
         escalated: bool = False, previous_engine: str | None = None,
         all_entries: Sequence | None = None, providers: Sequence[str] | None = None,
         recent: Mapping[str, Mapping[str, int]] | None = None,
         inflight: Mapping[str, Mapping[str, int]] | None = None):
    """``(best entry, decision)`` for ``role_tier``, or ``(None, None)``.

    ``entries`` are in engine order; ``excluded`` route ids are skipped
    without moving any model's tier. The tier is the first of
    :data:`_FALLBACK_ORDER` with a usable candidate; within it candidates
    are ordered by, first difference wins:

    1. ``preference`` (an engine id) when honoured: the engine has a usable
       candidate in this tier and, for ``deep``/``standard`` work, is not
       above :data:`HOT_PERCENT` while another candidate is below it.
       Otherwise it is ignored and the reason recorded.
    2. the quota band (unknown is not hot);
    3. recent same-tier balance excess beyond a four-attempt tolerance;
    4. :func:`suitability` as a soft tiebreak;
    5. lower used percent, only if every candidate has known quota;
    6. engine order.

    ``decision`` records the tier, every candidate of it in rank order with
    what ranked it, the preference and whether it was honoured.
    ``superseded`` maps an older route id to the newer route that replaces
    it (see :func:`superseded_routes`); those ids should also be in
    ``excluded`` and ``policy_excluded``."""
    superseded = superseded or {}
    tiers = model_tiers(entries, tier_overrides or {})
    usable = [e for e in entries if e.route.id not in excluded and (usage(e) or 0.0) < 100.0]
    for tier in _FALLBACK_ORDER[role_tier]:
        candidates = [e for e in usable if tiers[e.route.id] == tier]
        if not candidates:
            continue
        status = "none"
        if preference is not None:
            preferred = [e for e in candidates if e.route.engine == preference]
            if not preferred:
                offered = any(e.route.engine == preference and tiers[e.route.id] == tier for e in entries)
                status = "ignored:provider_unavailable" if offered else "ignored:no_candidate_in_tier"
            elif (role_tier != "light" and all(_hot(usage(e)) for e in preferred)
                  and not all(_hot(usage(e)) for e in candidates)):
                status = "ignored:provider_near_quota_limit"
            else:
                status = "honoured"
        honoured = preference if status == "honoured" else None
        order = {e.route.id: index for index, e in enumerate(entries)}
        engines = {e.route.engine for e in candidates}
        counts = {engine: int((recent or {}).get(tier, {}).get(engine, 0)) for engine in sorted(engines)}
        inflight_counts = {engine: int((inflight or {}).get(tier, {}).get(engine, 0)) for engine in sorted(engines)}
        minimum = min(counts.values())
        excess = {engine: max(0, count - minimum - BALANCE_TOLERANCE)
                  for engine, count in counts.items()}
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
            elif slash and all(tiers[e.route.id] != tier for e in allowed_offered):
                hint_status = "ignored:tier_mismatch"
            elif slash and all(e.route.id in superseded for e in allowed_offered):
                hint_status = "ignored:superseded"
            elif slash and all(e.route.id in policy_excluded for e in allowed_offered):
                hint_status = "ignored:auto_excluded"
            elif not hinted:
                hint_status = ("ignored:no_candidate_in_tier" if not slash and
                               not any(e.route.engine == hint_engine and tiers[e.route.id] == tier
                                       for e in entries) else "ignored:unavailable")
            elif (tier in ("deep", "standard") and all(_hot(usage(e)) for e in hinted)
                  and not all(_hot(usage(e)) for e in candidates)):
                hint_status = "ignored:near_quota_limit"
            elif counts[hint_engine] - minimum > HINT_TOLERANCE:
                hint_status = "ignored:engine_overused"
            else:
                hint_status = "honoured"
        used_percent_compared = all(usage(e) is not None for e in candidates)
        ranked = sorted(candidates, key=lambda e: (
            e.route.engine != honoured,
            not (hint_status == "honoured" and e in hinted),
            _order_key(role_tier, usage(e))[0],
            not (escalated and previous_engine and e.route.engine != previous_engine),
            excess[e.route.engine], -suitability(role, e.route.engine),
            usage(e) if used_percent_compared else 0.0, order[e.route.id]))
        best = ranked[0]
        decision = dict(
            role=role, role_tier=role_tier, tier=tier, basis="rank", rank_version=2,
            preference=preference, preference_status=status,
            model_hint=model_hint, model_hint_status=hint_status,
            model_hint_reason=model_reason, diversify_from=previous_engine if escalated else None,
            balance=dict(window=BALANCE_WINDOW, tolerance=BALANCE_TOLERANCE,
                         counts=counts, inflight=inflight_counts),
            used_percent_compared=used_percent_compared,
            candidates=[_candidate(e, usage, role, e.route.engine == honoured,
                                   excess[e.route.engine]) for e in ranked],
            unavailable=sorted(_route_label(e) for e in entries
                               if tiers[e.route.id] == tier and e.route.id not in policy_excluded
                               and (e.route.id in excluded or (usage(e) or 0.0) >= 100.0)),
            excluded_by_policy=sorted(_route_label(e) for e in entries
                                      if tiers[e.route.id] == tier and e.route.id in policy_excluded
                                      and e.route.id not in superseded),
            superseded={_route_label(e): superseded[e.route.id] for e in entries if e.route.id in superseded},
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        return best, decision
    return None, None


def _hot(used: float | None) -> bool:
    return (used or 0.0) >= HOT_PERCENT


def _route_label(entry) -> str:
    return f"{entry.route.engine}/{entry.route.model}"


def _candidate(entry, usage, role, preferred: bool, balance_excess: int = 0) -> dict:
    """One ranked candidate as the route audit records it."""
    return dict(route=_route_label(entry), connection_id=entry.connection_id, used_percent=usage(entry),
                hot=_hot(usage(entry)), quota_state="unknown" if usage(entry) is None else "known",
                balance_excess=balance_excess,
                suitability=suitability(role, entry.route.engine), preferred=preferred)


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


def _order_key(role_tier: str, used: float | None):
    hot = _hot(used)
    return (not hot, used or 0.0) if role_tier == "light" else (hot, used or 0.0)


def permitted_efforts(entry, tier: str) -> tuple[str, ...]:
    """What ``entry``'s model offers, weakest first, minus ``ultra``,
    ``max`` unless the model is ``light``, and anything above ``high`` on a
    flagship model."""
    offered = {value for name, value in entry.route.supported_parameters if name == "thought_level"}
    flagship = any(family in entry.route.model.casefold() for family in FLAGSHIP_FAMILIES)
    ceiling = EFFORT_ORDER.index(_FLAGSHIP_CEILING) if flagship else len(EFFORT_ORDER)
    return tuple(
        value for index, value in enumerate(EFFORT_ORDER)
        if value in offered and value not in _NEVER_EFFORTS and index <= ceiling
        and (tier == "light" or value not in _LIGHT_ONLY_EFFORTS)
    )


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


def _candidate_paths(project_root, environ) -> list[Path]:
    environ = os.environ if environ is None else environ
    home = Path(environ.get("HOME") or str(Path.home()))
    paths = [Path(project_root) / ".cortex" / "routing.json"] if project_root else []
    if environ.get(ENV_VAR):
        paths.append(Path(environ[ENV_VAR]))
    xdg = environ.get("XDG_CONFIG_HOME") or ""
    paths.append((Path(xdg) if xdg.startswith("/") else home / ".config") / "cortex" / "routing.json")
    return paths
