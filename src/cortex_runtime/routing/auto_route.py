"""Choose a worker's connection, model and effort from what is actually
available -- replaces the hand-maintained per-role ``route-policy.json``.

1. Each role has a tier (:data:`ROLE_TIERS`, ``standard`` when unlisted).
2. Each model's tier: a Claude family name decides it (:data:`FAMILY_TIERS`),
   a known Codex model its entry in :data:`KNOWN_MODEL_TIERS`; only a model
   neither names is placed by its position in its own engine's list
   (strongest first), split into thirds, so a new model still lands in a
   tier without configuration. Every tier thus has a candidate from each
   engine that offers a model for it.
3. Among the models of the role's tier (nearest other tier when there are
   none), candidates are ranked, first difference wins (:func:`rank`):
   the coordinator's per-task ``provider`` preference when it can be
   honoured (see :func:`rank`); the plan left on the account
   (``connections.quota``): ``deep``/``standard`` work goes to accounts
   below :data:`HOT_PERCENT` first, ``light`` work to accounts *above* it
   first, spending what is left of a nearly exhausted plan on cheap work;
   which engine suits the role's kind of work (:data:`ROLE_WORK_KINDS`,
   :data:`WORK_KIND_ENGINES`); then the lower used percentage; then the
   engine's own order. An exhausted window (100%) is never chosen, and
   neither is a route ``routing.availability`` recorded as unavailable (an
   engine that refused it for quota, rate limit, an unavailable model, or
   a dispatch failure) until that record expires. With routes configured
   but none usable the delegation is refused as ``no_available_route``.
   What decided it is returned as :attr:`Choice.decision` and recorded in
   the attempt's route audit.
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
   more step up (``escalate``) -- the strongest models run on hard work or
   where a weaker one already fell short. The model never names a model.

Only engine-confirmed catalogs are used -- a connection whose models were
never observed (seed catalog) is not routed to automatically; the worker
then stays on the coordinator's route, as before.

Optional overrides, first existing file wins (never merged):
``<project>/.cortex/routing.json``, ``$CORTEX_ROUTING``,
``$XDG_CONFIG_HOME/cortex/routing.json`` (``~/.config/...``)::

    {
      "pin":       {"security-auditor": "claude-agent-acp/opus:high", "*": "codex-acp/gpt-5.6-luna"},
      "deny":      ["gpt-5.5", "claude-agent-acp/haiku"],
      "tiers":     {"gpt-5.6-luna": "light"},
      "providers": ["claude-agent-acp", "codex-acp"]
    }

``pin`` is strict: a pinned route that is not available (not offered, or
recorded unavailable) refuses the delegation rather than substituting
another one. ``deny`` also applies to
explicitly selected routes and to the model selector.

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
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

from ..catalog.evidence import ProvenanceSource
from ..portable.errors import PortableError, PortableErrorCategory

ENV_VAR = "CORTEX_ROUTING"
HOT_PERCENT = 85.0
TIERS = ("deep", "standard", "light")
_FALLBACK_ORDER = {
    "deep": ("deep", "standard", "light"),
    "standard": ("standard", "deep", "light"),
    "light": ("light", "standard", "deep"),
}
EFFORT_BY_TIER = {"deep": "high", "standard": "medium", "light": "low"}
# Claude lists current models first and older generations after them, so
# its list order is not strength order; its family names are.
FAMILY_TIERS = {"opus": "deep", "fable": "deep", "sonnet": "standard", "haiku": "light"}
# Codex lists its newest generation first (gpt-6-luna before gpt-5.6-sol),
# so its list order is not strength order either. Astra > Sol > Terra >
# Luna within a generation; gpt-5.5 is the oldest and cheapest.
KNOWN_MODEL_TIERS = {
    "gpt-6-astra": "deep", "gpt-6-sol": "deep",
    "gpt-5.6-sol": "standard", "gpt-5.6-terra": "standard",
    "gpt-6-luna": "light", "gpt-5.6-luna": "light", "gpt-5.5": "light",
}
EFFORT_ORDER = ("low", "medium", "high", "xhigh", "max", "ultra")
_NEVER_EFFORTS = frozenset({"ultra"})
_LIGHT_ONLY_EFFORTS = frozenset({"max"})
# The strongest models are already strong enough at high; above it they
# only cost more. Named because no engine reports "flagship" itself.
FLAGSHIP_FAMILIES = ("astra", "fable")
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


def choose(role: str, project_root=None, environ: Mapping[str, str] | None = None, *,
           escalate: bool = False, complexity: str = "normal", unavailable: Sequence[Mapping] = (),
           preference: str | None = None) -> Choice | None:
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
            candidates=[_candidate(e, usage, role, False) for e in ranked],
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        return Choice(best.connection_id, model,
                      nearest(effort or EFFORT_BY_TIER[role_tier], permitted_efforts(best, tiers[best.route.id])),
                      "auto:pin", decision)
    providers = overrides.get("providers")
    status = None
    if preference is not None and providers is not None and preference not in providers:
        status = "ignored:provider_not_allowed"
    elif preference is not None and not any(e.route.engine == preference for e in entries):
        status = "ignored:provider_not_configured"
    picked, decision = rank(role_tier, allowed, usage, overrides.get("tiers") or {}, excluded=frozenset(blocked),
                            role=role, preference=None if status else preference)
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
                         + "; ".join(_unavailable_reasons(allowed, blocked, usage))
                         + ". Wait for a limit to reset, connect another account, or change routing.json."),
            retry_eligible=False,
        )
    effort = nearest(EFFORT_BY_TIER[role_tier], permitted_efforts(picked, tiers[picked.route.id]))
    return Choice(picked.connection_id, picked.route.model, effort, "auto", decision)


_COMPLEXITY_SHIFT = {"low": 1, "normal": 0, "high": -1}


def tier_for(role: str, escalate: bool = False, complexity: str = "normal") -> str:
    """The role's tier, one step cheaper for ``low`` work, one step
    stronger for ``high`` work, and one more step stronger when escalated."""
    index = TIERS.index(ROLE_TIERS.get(role, "standard")) + _COMPLEXITY_SHIFT.get(complexity, 0) - bool(escalate)
    return TIERS[min(len(TIERS) - 1, max(0, index))]


def pick(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None):
    """The best entry for ``role_tier`` (see :func:`rank`), or ``None``."""
    return rank(role_tier, entries, usage, tier_overrides, excluded, role=role, preference=preference)[0]


def rank(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None,
         excluded: frozenset = frozenset(), *, role: str | None = None, preference: str | None = None):
    """``(best entry, decision)`` for ``role_tier``, or ``(None, None)``.

    ``entries`` are in engine order; ``excluded`` route ids are skipped
    without moving any model's tier. The tier is the first of
    :data:`_FALLBACK_ORDER` with a usable candidate; within it candidates
    are ordered by, first difference wins:

    1. ``preference`` (an engine id) when honoured: the engine has a usable
       candidate in this tier and, for ``deep``/``standard`` work, is not
       above :data:`HOT_PERCENT` while another candidate is below it.
       Otherwise it is ignored and the reason recorded.
    2. the quota band: below :data:`HOT_PERCENT` first, or above it first
       for ``light`` work;
    3. :func:`suitability` of the engine for ``role``;
    4. the lower used percentage (unknown counts as 0);
    5. the engine's own order.

    ``decision`` records the tier, every candidate of it in rank order with
    what ranked it, the preference and whether it was honoured."""
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
        ranked = sorted(candidates, key=lambda e: (
            e.route.engine != honoured, _order_key(role_tier, usage(e))[0],
            -suitability(role, e.route.engine), usage(e) or 0.0, order[e.route.id]))
        best = ranked[0]
        decision = dict(
            role=role, role_tier=role_tier, tier=tier, basis="rank",
            preference=preference, preference_status=status,
            candidates=[_candidate(e, usage, role, e.route.engine == honoured) for e in ranked],
            unavailable=sorted(_route_label(e) for e in entries
                               if tiers[e.route.id] == tier and e.route.id not in {c.route.id for c in candidates}),
            chosen=_route_label(best), chosen_connection_id=best.connection_id,
        )
        return best, decision
    return None, None


def _hot(used: float | None) -> bool:
    return (used or 0.0) >= HOT_PERCENT


def _route_label(entry) -> str:
    return f"{entry.route.engine}/{entry.route.model}"


def _candidate(entry, usage, role, preferred: bool) -> dict:
    """One ranked candidate as the route audit records it."""
    return dict(route=_route_label(entry), connection_id=entry.connection_id, used_percent=usage(entry),
                hot=_hot(usage(entry)), suitability=suitability(role, entry.route.engine), preferred=preferred)


def model_tiers(entries: Sequence, tier_overrides: Mapping[str, str]) -> dict[str, str]:
    tiers = {}
    by_connection: dict[str, list] = {}
    for entry in entries:
        model = entry.route.model
        family = (next((tier for name, tier in FAMILY_TIERS.items() if name in model.casefold()), None)
                  or KNOWN_MODEL_TIERS.get(model.casefold()))
        if tier_overrides.get(model) or family:
            tiers[entry.route.id] = tier_overrides.get(model) or family
        else:
            by_connection.setdefault(entry.connection_id, []).append(entry)
    for group in by_connection.values():
        for index, entry in enumerate(group):
            tiers[entry.route.id] = TIERS[min(2, index * 3 // len(group))]
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


def _denied(overrides: Mapping, engine: str, model: str) -> bool:
    deny = overrides.get("deny") or ()
    return model in deny or f"{engine}/{model}" in deny


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
    if not isinstance(pins, dict) or not all(isinstance(v, str) and "/" in v for v in pins.values()):
        raise ValueError("'pin' must map a role to \"engine/model\" or \"engine/model:effort\"")
    if not isinstance(deny, list) or not all(isinstance(v, str) and v for v in deny):
        raise ValueError("'deny' must be a list of \"model\" or \"engine/model\"")
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
