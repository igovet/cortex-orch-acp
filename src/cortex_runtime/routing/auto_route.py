"""Choose a worker's connection, model and effort from what is actually
available -- replaces the hand-maintained per-role ``route-policy.json``.

1. Each role has a tier (:data:`ROLE_TIERS`, ``standard`` when unlisted).
2. Each model's tier: a Claude family name decides it (:data:`FAMILY_TIERS`);
   any other model is placed by its position in its own engine's list
   (codex lists strongest first, by its catalog ``priority``), split into
   thirds. A new model therefore lands in a tier without configuration.
3. Among the models of the role's tier (nearest other tier when there are
   none), accounts are ordered by how much of their plan is left
   (``connections.quota``): ``deep``/``standard`` work goes to accounts
   below :data:`HOT_PERCENT` first; ``light`` work goes to accounts
   *above* it first, spending what is left of a nearly exhausted plan on
   cheap work. An exhausted window (100%) is never chosen.
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
      "pin":   {"security-auditor": "claude-agent-acp/opus:high", "*": "codex-acp/gpt-5.6-luna"},
      "deny":  ["gpt-5.5", "claude-agent-acp/haiku"],
      "tiers": {"gpt-5.6-luna": "light"}
    }

``pin`` is strict: a pinned route that is not available refuses the
delegation rather than substituting another one. ``deny`` also applies to
explicitly selected routes and to the model selector.
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
_EFFORT_ORDER = ("low", "medium", "high", "xhigh", "max", "ultra")
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


@dataclass(frozen=True)
class Choice:
    connection_id: str
    model: str
    effort: str | None
    basis: str  # "auto" | "auto:pin"


def choose(role: str, project_root=None, environ: Mapping[str, str] | None = None, *,
           escalate: bool = False, complexity: str = "normal") -> Choice | None:
    """The route this worker role should open on, or ``None`` when nothing
    engine-confirmed is available (keep the coordinator's route).
    See :func:`tier_for` for ``escalate`` and ``complexity``."""
    from ..connections import quota
    from ..connections import registry as connection_registry

    overrides = load_overrides(project_root, environ)
    connections = connection_registry.read_registry(environ).connections
    entries = [entry for entry in _entries(overrides, connections, environ) if not _is_seed(entry)]
    tiers = model_tiers(entries, overrides.get("tiers") or {})
    usage_cache: dict = {}

    def usage(entry):
        key = (entry.connection_id, entry.route.model)
        if key not in usage_cache:
            usage_cache[key] = quota.used_percent(connections[entry.connection_id], entry.route.model, environ)
        return usage_cache[key]

    role_tier = tier_for(role, escalate, complexity)
    pin = _pin_for(overrides, role)
    if pin is not None:
        engine, model, effort = pin
        matches = [e for e in entries if e.route.engine == engine and e.route.model == model]
        if not matches:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="pinned_route_unavailable",
                remediation=(f"routing.json pins role {role!r} to {engine}/{model}, which no configured "
                             "connection currently offers; connect it or change the pin."),
                retry_eligible=False, field="pin",
            )
        best = sorted(matches, key=lambda e: _order_key("deep", usage(e)))[0]
        return Choice(best.connection_id, model,
                      nearest(effort or EFFORT_BY_TIER[role_tier], permitted_efforts(best, tiers[best.route.id])),
                      "auto:pin")
    picked = pick(role_tier, entries, usage, overrides.get("tiers") or {})
    if picked is None:
        return None
    effort = nearest(EFFORT_BY_TIER[role_tier], permitted_efforts(picked, tiers[picked.route.id]))
    return Choice(picked.connection_id, picked.route.model, effort, "auto")


_COMPLEXITY_SHIFT = {"low": 1, "normal": 0, "high": -1}


def tier_for(role: str, escalate: bool = False, complexity: str = "normal") -> str:
    """The role's tier, one step cheaper for ``low`` work, one step
    stronger for ``high`` work, and one more step stronger when escalated."""
    index = TIERS.index(ROLE_TIERS.get(role, "standard")) + _COMPLEXITY_SHIFT.get(complexity, 0) - bool(escalate)
    return TIERS[min(len(TIERS) - 1, max(0, index))]


def pick(role_tier: str, entries: Sequence, usage, tier_overrides: Mapping[str, str] | None = None):
    """The best entry for ``role_tier``; ``entries`` are in engine order."""
    tiers = model_tiers(entries, tier_overrides or {})
    usable = [e for e in entries if (usage(e) or 0.0) < 100.0]
    for tier in _FALLBACK_ORDER[role_tier]:
        candidates = [e for e in usable if tiers[e.route.id] == tier]
        if candidates:
            # sorted() is stable: equal keys keep the engine's strongest-first order.
            return sorted(candidates, key=lambda e: _order_key(role_tier, usage(e)))[0]
    return None


def model_tiers(entries: Sequence, tier_overrides: Mapping[str, str]) -> dict[str, str]:
    tiers = {}
    by_connection: dict[str, list] = {}
    for entry in entries:
        model = entry.route.model
        family = next((tier for name, tier in FAMILY_TIERS.items() if name in model.casefold()), None)
        if tier_overrides.get(model) or family:
            tiers[entry.route.id] = tier_overrides.get(model) or family
        else:
            by_connection.setdefault(entry.connection_id, []).append(entry)
    for group in by_connection.values():
        for index, entry in enumerate(group):
            tiers[entry.route.id] = TIERS[min(2, index * 3 // len(group))]
    return tiers


def _order_key(role_tier: str, used: float | None):
    used = used or 0.0
    hot = used >= HOT_PERCENT
    return (not hot, used) if role_tier == "light" else (hot, used)


def permitted_efforts(entry, tier: str) -> tuple[str, ...]:
    """What ``entry``'s model offers, weakest first, minus ``ultra``,
    ``max`` unless the model is ``light``, and anything above ``high`` on a
    flagship model."""
    offered = {value for name, value in entry.route.supported_parameters if name == "thought_level"}
    flagship = any(family in entry.route.model.casefold() for family in FLAGSHIP_FAMILIES)
    ceiling = _EFFORT_ORDER.index(_FLAGSHIP_CEILING) if flagship else len(_EFFORT_ORDER)
    return tuple(
        value for index, value in enumerate(_EFFORT_ORDER)
        if value in offered and value not in _NEVER_EFFORTS and index <= ceiling
        and (tier == "light" or value not in _LIGHT_ONLY_EFFORTS)
    )


def nearest(effort: str, permitted: Sequence[str]) -> str | None:
    if effort in permitted:
        return effort
    ranked = [value for value in _EFFORT_ORDER if value in permitted]
    if not ranked or effort not in _EFFORT_ORDER:
        return ranked[-1] if ranked else None
    target = _EFFORT_ORDER.index(effort)
    return min(ranked, key=lambda value: abs(_EFFORT_ORDER.index(value) - target))


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
    """Launchable, non-denied catalog entries, in each engine's own order."""
    from ..acp.config_options import presented_entries, selectable_entries_from_snapshots

    return [
        entry for entry in presented_entries(selectable_entries_from_snapshots(connections, environ))
        if getattr(connections.get(entry.connection_id), "launch_command", None) is not None
        and not _denied(overrides, entry.route.engine, entry.route.model)
    ]


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
    if not isinstance(data, dict):
        raise ValueError("must contain a JSON object")
    pins, deny, tiers = data.get("pin", {}), data.get("deny", []), data.get("tiers", {})
    if not isinstance(pins, dict) or not all(isinstance(v, str) and "/" in v for v in pins.values()):
        raise ValueError("'pin' must map a role to \"engine/model\" or \"engine/model:effort\"")
    if not isinstance(deny, list) or not all(isinstance(v, str) and v for v in deny):
        raise ValueError("'deny' must be a list of \"model\" or \"engine/model\"")
    if not isinstance(tiers, dict) or not all(v in TIERS for v in tiers.values()):
        raise ValueError(f"'tiers' must map a model to one of {', '.join(TIERS)}")


def _candidate_paths(project_root, environ) -> list[Path]:
    environ = os.environ if environ is None else environ
    home = Path(environ.get("HOME") or str(Path.home()))
    paths = [Path(project_root) / ".cortex" / "routing.json"] if project_root else []
    if environ.get(ENV_VAR):
        paths.append(Path(environ[ENV_VAR]))
    xdg = environ.get("XDG_CONFIG_HOME") or ""
    paths.append((Path(xdg) if xdg.startswith("/") else home / ".config") / "cortex" / "routing.json")
    return paths
