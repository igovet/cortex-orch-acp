"""Which models automatic worker routing may use, their tiers and their
per-model policy -- owner configuration first, built-in defaults second.

routing.json ``models`` (see :mod:`routing.auto_route` for the file's
precedence) configures each engine separately, keyed by the exact model id
the engine reports (OpenCode ids may contain ``/`` and ``,``)::

    "models": {
      "<engine id>": {
        "auto":   ["<model id>", ...],
        "tiers":  {"<model id>": "light|standard|deep|flagship" or ["standard", "deep"]},
        "policy": {"<model id>": {"effort_floor": "high", "max_effort": null,
                                  "effort_ladder": ["high", "xhigh", "max"],
                                  "also_standard_for": ["testing"],
                                  "preferred_for": ["design", "review"],
                                  "request_names": ["астр"]}}
      }
    }

- ``auto`` present: LISTED mode -- exactly the listed models of that engine
  are eligible for automatic routing (``[]``: none); the built-in exclusions
  and supersede no longer apply to that engine. ``auto`` absent: BUILTIN
  mode -- the built-in rules decide (:data:`BUILTIN_AUTO_DENY_ENGINES`,
  :data:`BUILTIN_AUTO_DENY_FAMILIES`, ``auto_allow``, supersede).
- A model's tier is, first that applies: ``models[engine].tiers`` (source
  ``configured``), the legacy engine-agnostic top-level ``tiers``
  (``legacy_tiers``), the built-in tables (``builtin``), else its position
  in its own connection's list (``position``). Only ``models[engine].tiers``
  may give a model several tiers (a list): it is then a candidate of each of
  them, and its efforts follow the tier it was chosen for.
  :attr:`RoutePolicy.tier` is the strongest of them, :attr:`RoutePolicy.tiers`
  all of them.
- A model's policy is the built-in attributes (:func:`builtin_attributes`)
  overlaid key by key with ``models[engine].policy[model]``; a configured
  ``null`` removes the built-in value.

Policy attributes (:data:`POLICY_ATTRIBUTES`):

- ``effort_floor``: never run below this effort (also for pins and requested
  efforts) when the model offers it.
- ``max_effort``: never run above this effort. Without it a model of the
  effective ``flagship`` tier stops at ``high``.
- ``effort_ladder``: 1-3 ascending efforts a light-tier model runs at for
  light work, standard work, and standard work that fell short once; a
  ladder that holds ``max`` also permits ``max`` outside the light tier.
- ``also_standard_for``: work kinds for which a light-tier model is also a
  standard-tier candidate (never for high complexity, deep or flagship work,
  or deep roles).
- ``preferred_for``: work kinds this model is the owner's preferred choice
  for; it ranks ahead of its tier's other candidates for that kind of work
  after plan usage and retry diversification, before engine affinity. It
  never moves a tier.
- ``request_names``: extra spellings (Cyrillic stems take Russian endings)
  that count as the user naming this model's family in an explicit request.

Nothing here names a model to a coordinator or worker: prompts and schemas
refer to ``list_route_options``, which reports the effective values.
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
TIER_SOURCES = ("configured", "legacy_tiers", "builtin", "position")
SELECTION_MODES = ("builtin", "listed")
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

# --- built-in defaults ---------------------------------------------------------
#
# Used only where routing.json ``models`` says nothing about a model.

# Claude lists current models first and older generations after them, so
# its list order is not strength order; its family names are (substring).
BUILTIN_FAMILY_TIERS = {"opus": "deep", "fable": "flagship", "sonnet": "standard", "haiku": "light"}
# Codex lists its newest generation first, so its list order is not strength
# order either; these ids, then the line table below, place its models.
BUILTIN_MODEL_TIERS = {
    "gpt-6-astra": "flagship", "gpt-6-sol": "deep",
    "gpt-5.6-sol": "standard", "gpt-5.6-terra": "standard",
    "gpt-6-luna": "light", "gpt-5.6-luna": "light", "gpt-5.5": "light",
}
# A Codex line keeps its tier across generations, so a newer release no
# table names still lands in its line's tier rather than by list position.
BUILTIN_LINE_TIERS = {"astra": "flagship", "sol": "deep", "terra": "standard", "luna": "light"}
# The strongest models are already strong enough at high (substring match).
BUILTIN_FLAGSHIP_FAMILIES = ("astra", "fable")
# A cheap but strong light line: never below high, its effort steps with the
# work, and it also takes standard-tier testing work.
BUILTIN_STRETCH_LINES = frozenset({"luna"})
BUILTIN_STRETCH_POLICY = dict(effort_floor="high", effort_ladder=("high", "xhigh", "max"),
                              also_standard_for=frozenset({"testing"}))
# Russian spellings of family names the engines list in Latin.
BUILTIN_REQUEST_NAMES = {
    "astra": ("астр",),
    "fable": ("фейбл", "фэйбл"),
    "mythos": ("мифос", "майтос", "митос"),
}
# Claude Haiku repeatedly submitted arguments that contradicted advertised
# Cortex tool schemas; it stays available for a pin but is not routed
# automatically on Claude unless selected or ``auto_allow``-ed.
BUILTIN_AUTO_DENY_FAMILIES = frozenset({"haiku"})
BUILTIN_AUTO_DENY_FAMILY_ENGINE = "claude-agent-acp"
#: Optional engines are never routed automatically unless selected.
BUILTIN_AUTO_DENY_ENGINES = frozenset({"opencode"})

# --- model versions ------------------------------------------------------------

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


# --- resolution ----------------------------------------------------------------

@dataclass(frozen=True)
class RoutePolicy:
    """One route's effective tier, selection and policy attributes."""

    tier: str | None  # the strongest of ``tiers``
    tier_source: str | None  # configured | legacy_tiers | builtin | position | None (unplaced)
    selection: str  # builtin | listed
    selected: bool | None  # listed-mode membership; None in builtin mode
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
        """A light-tier model with its own effort ladder or standard work
        kinds (the rule built in for the Codex Luna line)."""
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
    return "listed" if "auto" in engine_config(overrides, engine) else "builtin"


def is_selected(overrides: Mapping, engine: str, model: str) -> bool | None:
    """LISTED-mode membership of ``engine``/``model``; ``None`` in BUILTIN mode."""
    config = engine_config(overrides, engine)
    if "auto" not in config:
        return None
    return model in (config.get("auto") or ())


def builtin_tier(model: str) -> str | None:
    """The built-in tier of ``model``, or ``None`` when no table names it."""
    lowered = model.casefold()
    return (next((tier for name, tier in BUILTIN_FAMILY_TIERS.items() if name in lowered), None)
            or BUILTIN_MODEL_TIERS.get(lowered)
            or BUILTIN_LINE_TIERS.get(model_line(model)[0]))


def builtin_attributes(model: str) -> dict[str, object]:
    """The built-in policy attributes of ``model`` (only those it has)."""
    attributes: dict[str, object] = {}
    lowered = model.casefold()
    family = model_line(model)[0]
    if any(name in lowered for name in BUILTIN_FLAGSHIP_FAMILIES):
        attributes["max_effort"] = "high"
    if family in BUILTIN_STRETCH_LINES:
        attributes.update(BUILTIN_STRETCH_POLICY)
    if family in BUILTIN_REQUEST_NAMES:
        attributes["request_names"] = BUILTIN_REQUEST_NAMES[family]
    return attributes


def tier_list(value) -> tuple[str, ...]:
    """A configured tier value (one tier or a list of tiers) as the known
    tiers it names, strongest first."""
    named = {value} if isinstance(value, str) else set(value or ())
    return tuple(tier for tier in TIERS if tier in named)


def _configured_tier(overrides: Mapping, engine: str, model: str) -> tuple[tuple[str, ...], str | None]:
    configured = engine_config(overrides, engine).get("tiers") or {}
    if tier_list(configured.get(model)):
        return tier_list(configured[model]), "configured"
    legacy = overrides.get("tiers") or {}
    if legacy.get(model):
        return (legacy[model],), "legacy_tiers"
    tier = builtin_tier(model)
    return ((tier,), "builtin") if tier else ((), None)


def _policy(engine: str, model: str, overrides: Mapping, tiers: tuple[str, ...], tier_source: str | None) -> RoutePolicy:
    attributes = builtin_attributes(model)
    sources = {name: "builtin" for name in attributes}
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
        selection="builtin" if selected is None else "listed", selected=selected,
        effort_floor=attributes.get("effort_floor"), max_effort=attributes.get("max_effort"),
        effort_ladder=tuple(attributes.get("effort_ladder") or ()),
        also_standard_for=frozenset(attributes.get("also_standard_for") or ()),
        preferred_for=frozenset(attributes.get("preferred_for") or ()),
        request_names=tuple(attributes.get("request_names") or ()),
        attribute_sources={name: sources[name] for name in POLICY_ATTRIBUTES if name in sources},
    )


def policy_for(engine: str, model: str, overrides: Mapping | None = None) -> RoutePolicy:
    """One model's policy without its connection's list: the tier is
    ``None`` when only its list position would place it."""
    overrides = overrides or {}
    tiers, source = _configured_tier(overrides, engine, model)
    return _policy(engine, model, overrides, tiers, source)


def resolve_policies(entries: Sequence, overrides: Mapping | None = None) -> dict[str, RoutePolicy]:
    """Route id -> :class:`RoutePolicy` for catalog ``entries`` (in each
    engine's own order). A model no configuration or table places is put
    by its position in its connection's list, strongest first, in thirds."""
    overrides = overrides or {}
    result: dict[str, RoutePolicy] = {}
    unplaced: dict[str, list] = {}
    for entry in entries:
        engine, model = entry.route.engine, entry.route.model
        tiers, source = _configured_tier(overrides, engine, model)
        if not tiers:
            unplaced.setdefault(entry.connection_id, []).append(entry)
            continue
        result[entry.route.id] = _policy(engine, model, overrides, tiers, source)
    for group in unplaced.values():
        for index, entry in enumerate(group):
            tier = ("deep", "standard", "light")[min(2, index * 3 // len(group))]
            result[entry.route.id] = _policy(entry.route.engine, entry.route.model, overrides, (tier,), "position")
    return result


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
