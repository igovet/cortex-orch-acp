"""Exact owner-authored model policy (routing.json model_policy_version=2).

Automatic identities and nonempty tiers come only from models.<engine>.
Omitted/null optional attributes are neutral. No catalog order, display name,
family or version supplies membership, tiers, effort or request labels.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Callable, Iterable, Mapping, Sequence

TIERS = ("flagship", "deep", "standard", "light")
EFFORT_ORDER = ("low", "medium", "high", "xhigh", "max", "ultra")
#: Efforts a policy may name: ``ultra`` is never used by automatic routing.
POLICY_EFFORTS = EFFORT_ORDER[:-1]
POLICY_ATTRIBUTES = ("effort_floor", "max_effort", "effort_ladder", "also_standard_for", "preferred_for",
                     "request_names")
ENGINE_KEYS = ("auto", "tiers", "policy")
TIER_SOURCES = ("configured",)
SELECTION_MODES = ("listed",)
MAX_SELECTED = 1000
MAX_REQUEST_NAMES = 16
WORK_KINDS = ("implementation", "operations", "debugging", "testing", "investigation",
              "review", "design", "writing", "planning", "analysis")
WORK_KIND_LABELS = {
    "implementation": "Implementation", "operations": "Operations", "debugging": "Debugging",
    "testing": "Testing", "investigation": "Investigation", "review": "Review",
    "design": "Design and architecture", "writing": "Documentation and writing",
    "planning": "Planning", "analysis": "Analysis",
}

# --- resolution ----------------------------------------------------------------

@dataclass(frozen=True)
class RoutePolicy:
    """One route's effective tier, selection and policy attributes."""

    tier: str | None  # the strongest of ``tiers``
    tier_source: str | None  # configured | None (unplaced)
    selection: str  # listed
    selected: bool  # exact automatic membership
    effort_floor: str | None = None
    max_effort: str | None = None
    effort_ladder: tuple[str, ...] = ()
    also_standard_for: frozenset = frozenset()
    preferred_for: frozenset = frozenset()
    request_names: tuple[str, ...] = ()
    attribute_sources: Mapping[str, str] = field(default_factory=dict)
    #: Every tier the model is a candidate of, strongest first (``(tier,)``
    #: unless ``models[engine].tiers`` lists several).
    tiers: tuple[str, ...] = ()

    def __post_init__(self):
        if not self.tiers and self.tier is not None:
            object.__setattr__(self, "tiers", (self.tier,))

    @property
    def stretch(self) -> bool:
        """A light-tier model with configured effort/standard-work allowances."""
        return "light" in self.tiers and bool(self.effort_ladder or self.also_standard_for)

    def attributes(self) -> dict[str, object]:
        """The effective attributes as JSON, with each one's source."""
        return dict(effort_floor=self.effort_floor, max_effort=self.max_effort,
                    effort_ladder=list(self.effort_ladder), also_standard_for=sorted(self.also_standard_for),
                    preferred_for=sorted(self.preferred_for), request_names=list(self.request_names),
                    attribute_sources=dict(self.attribute_sources))


def engine_config(overrides: Mapping, engine: str) -> Mapping:
    """``overrides["models"][engine]``, or ``{}``."""
    models = overrides.get("models") or {}
    value = models.get(engine) if isinstance(models, Mapping) else None
    return value if isinstance(value, Mapping) else {}


def selection_mode(overrides: Mapping, engine: str) -> str:
    return "listed"


def is_selected(overrides: Mapping, engine: str, model: str) -> bool:
    return model in (engine_config(overrides, engine).get("auto") or ())


def tier_list(value) -> tuple[str, ...]:
    """A configured tier value (one tier or a list of tiers) as the known
    tiers it names, strongest first."""
    named = {value} if isinstance(value, str) else set(value or ())
    return tuple(tier for tier in TIERS if tier in named)


def _configured_tier(overrides: Mapping, engine: str, model: str) -> tuple[tuple[str, ...], str | None]:
    configured = engine_config(overrides, engine).get("tiers") or {}
    if tier_list(configured.get(model)):
        return tier_list(configured[model]), "configured"
    return (), None


def _policy(engine: str, model: str, overrides: Mapping, tiers: tuple[str, ...], tier_source: str | None) -> RoutePolicy:
    attributes = {}
    sources = {}
    configured = (engine_config(overrides, engine).get("policy") or {}).get(model) or {}
    for name in POLICY_ATTRIBUTES:
        if name in configured:
            attributes[name] = configured[name]
            sources[name] = "configured"
    for name, value in list(attributes.items()):
        if value is None:
            del attributes[name]
            sources.pop(name, None)
    selected = is_selected(overrides, engine, model)
    return RoutePolicy(
        tier=tiers[0] if tiers else None, tiers=tuple(tiers), tier_source=tier_source,
        selection="listed", selected=selected,
        effort_floor=attributes.get("effort_floor"), max_effort=attributes.get("max_effort"),
        effort_ladder=tuple(attributes.get("effort_ladder") or ()),
        also_standard_for=frozenset(attributes.get("also_standard_for") or ()),
        preferred_for=frozenset(attributes.get("preferred_for") or ()),
        request_names=tuple(attributes.get("request_names") or ()),
        attribute_sources={name: sources[name] for name in POLICY_ATTRIBUTES if name in sources},
    )


def policy_for(engine: str, model: str, overrides: Mapping | None = None) -> RoutePolicy:
    """One exact model policy; missing placement remains ``None``."""
    overrides = overrides or {}
    tiers, source = _configured_tier(overrides, engine, model)
    return _policy(engine, model, overrides, tiers, source)


def resolve_policies(entries: Sequence, overrides: Mapping | None = None) -> dict[str, RoutePolicy]:
    """Resolve each exact catalog identity; unconfigured identities remain unplaced."""
    return {entry.route.id: policy_for(entry.route.engine, entry.route.model, overrides)
            for entry in entries}


def stretch_effort(ladder: Sequence[str], role_tier: str, escalate: bool = False) -> str | None:
    """A stretch model's target effort: one ladder step per tier above
    light, and one more for a retry or a fix after requested changes."""
    if not ladder:
        return None
    step = (0 if role_tier == "light" else 1) + int(escalate)
    return ladder[min(step, len(ladder) - 1)]


# --- validation ----------------------------------------------------------------

_MODEL_ID = re.compile(r"[^\s\x00-\x1f\x7f]{1,256}")
_REQUEST_NAME = re.compile(r"[\w-]{2,32}", re.UNICODE)


def _model_id(value) -> bool:
    return isinstance(value, str) and bool(_MODEL_ID.fullmatch(value))


def _efforts_ok(value) -> bool:
    return isinstance(value, str) and value in POLICY_EFFORTS


def validate_models(value, *, engines: Sequence[str], work_kinds: Sequence[str],
                    reserved_word: Callable[[str], bool] | None = None,
                    exact_model: Callable[[str, str], bool] | None = None,
                    prefix: str = "models") -> list[tuple[str, str, str]]:
    """Every problem of a routing.json ``models`` value, as
    ``(field, code, message)``; ``[]`` when it is valid.

    ``reserved_word`` refuses a ``request_names`` entry that is itself an
    ordinary request, work, means or negation word; ``exact_model(engine,
    model)`` refuses a model id that cannot be one of ``engine``'s exact ids."""
    problems: list[tuple[str, str, str]] = []

    def add(where: str, code: str, message: str) -> None:
        problems.append((where, code, message))

    if not isinstance(value, Mapping):
        add(prefix, "invalid_type", "must map an engine id to {auto, tiers, policy}")
        return problems
    kinds = tuple(work_kinds)
    for engine, config in value.items():
        base = f"{prefix}.{engine}"
        if engine not in engines:
            add(base, "unknown_engine", f"engine must be one of {', '.join(engines)}")
            continue
        if not isinstance(config, Mapping):
            add(base, "invalid_type", "must be an object with optional auto, tiers and policy")
            continue
        for key in config:
            if key not in ENGINE_KEYS:
                add(f"{base}.{key}", "unsupported_setting", f"use one of {', '.join(ENGINE_KEYS)}")

        def check_model(where: str, model) -> bool:
            if not _model_id(model):
                add(where, "invalid_model", "must be an exact model id without whitespace (at most 256 characters)")
                return False
            if exact_model is not None and not exact_model(engine, model):
                add(where, "model_not_exact", f"{model!r} is not an exact {engine} model id")
                return False
            return True

        if "auto" in config:
            auto = config["auto"]
            if not isinstance(auto, list) or len(auto) > MAX_SELECTED:
                add(f"{base}.auto", "invalid_type", f"must be an array of at most {MAX_SELECTED} exact model ids")
            else:
                seen = set()
                for index, model in enumerate(auto):
                    if check_model(f"{base}.auto[{index}]", model):
                        if model in seen:
                            add(f"{base}.auto[{index}]", "duplicate_model", f"{model!r} is listed twice")
                        seen.add(model)
        if "tiers" in config:
            tiers = config["tiers"]
            if not isinstance(tiers, Mapping):
                add(f"{base}.tiers", "invalid_type", "must map an exact model id to a tier or a list of tiers")
            else:
                for model, tier in tiers.items():
                    where = f"{base}.tiers.{model}"
                    if not check_model(where, model):
                        continue
                    if isinstance(tier, list):
                        if (not tier or len(set(map(str, tier))) != len(tier)
                                or not all(isinstance(item, str) and item in TIERS for item in tier)):
                            add(where, "invalid_value",
                                f"a list of tiers must be non-empty and unique, drawn from {', '.join(TIERS)}")
                    elif tier not in TIERS:
                        add(where, "invalid_value", f"tier must be one of {', '.join(TIERS)} or a list of them")
        if "policy" in config:
            policies = config["policy"]
            if not isinstance(policies, Mapping):
                add(f"{base}.policy", "invalid_type", "must map an exact model id to its policy attributes")
                continue
            for model, policy in policies.items():
                where = f"{base}.policy.{model}"
                if not check_model(where, model):
                    continue
                if not isinstance(policy, Mapping):
                    add(where, "invalid_type", f"must be an object of {', '.join(POLICY_ATTRIBUTES)}")
                    continue
                problems.extend(_policy_problems(where, policy, kinds, reserved_word))
                preferred = policy.get("preferred_for")
                if ("auto" in config and isinstance(config["auto"], list)
                        and isinstance(preferred, list) and preferred and model not in config["auto"]):
                    add(f"{where}.preferred_for", "preferred_not_selected",
                        "a preferred model must be selected in this engine's auto list")
    return problems


def _policy_problems(where: str, policy: Mapping, kinds: Sequence[str],
                     reserved_word: Callable[[str], bool] | None) -> Iterable[tuple[str, str, str]]:
    for key in policy:
        if key not in POLICY_ATTRIBUTES:
            yield f"{where}.{key}", "unsupported_setting", f"use one of {', '.join(POLICY_ATTRIBUTES)}"
    for key in ("effort_floor", "max_effort"):
        value = policy.get(key)
        if value is not None and not _efforts_ok(value):
            yield f"{where}.{key}", "invalid_effort", f"must be null or one of {', '.join(POLICY_EFFORTS)}"
    floor, ceiling = policy.get("effort_floor"), policy.get("max_effort")
    if _efforts_ok(floor) and _efforts_ok(ceiling) and EFFORT_ORDER.index(floor) > EFFORT_ORDER.index(ceiling):
        yield f"{where}.effort_floor", "invalid_value", "effort_floor must not be above max_effort"
    ladder = policy.get("effort_ladder")
    if ladder is not None:
        if (not isinstance(ladder, list) or not 1 <= len(ladder) <= 3 or not all(_efforts_ok(v) for v in ladder)
                or any(EFFORT_ORDER.index(a) >= EFFORT_ORDER.index(b) for a, b in zip(ladder, ladder[1:]))):
            yield (f"{where}.effort_ladder", "invalid_value",
                   f"must be null or 1-3 strictly ascending efforts from {', '.join(POLICY_EFFORTS)}")
    for key in ("also_standard_for", "preferred_for"):
        value = policy.get(key)
        if value is None:
            continue
        if (not isinstance(value, list) or len(set(map(str, value))) != len(value)
                or not all(isinstance(kind, str) and kind in kinds for kind in value)):
            yield f"{where}.{key}", "invalid_value", f"must be null or unique work kinds from {', '.join(kinds)}"
    names = policy.get("request_names")
    if names is not None:
        if not isinstance(names, list) or not 1 <= len(names) <= MAX_REQUEST_NAMES or len(set(map(str, names))) != len(names):
            yield (f"{where}.request_names", "invalid_value",
                   f"must be null or 1-{MAX_REQUEST_NAMES} unique names")
        else:
            for index, name in enumerate(names):
                if not (isinstance(name, str) and _REQUEST_NAME.fullmatch(name) and name == name.casefold()):
                    yield (f"{where}.request_names[{index}]", "invalid_value",
                           "must be a lowercase word or stem of 2-32 letters, digits, '_' or '-'")
                elif reserved_word is not None and reserved_word(name):
                    yield (f"{where}.request_names[{index}]", "reserved_word",
                           f"{name!r} is an ordinary request, work, means or negation word and cannot name a model")
