"""Build a truthful :class:`CatalogSnapshot` from connections and discovery.

``build_catalog`` never synthesizes a route: a route only exists because an
``Observation`` (a validated, adapter-reported ``configOptions`` payload --
see the ACP ``SessionConfigOption`` schema) actually listed a model, or
because a caller explicitly supplied an unverified manual declaration for a
connection whose ``(engine, auth_mode)`` is a documented/qualified
combination.  A missing or failed observation for one connection never
borrows or fabricates routes for it, and never affects any other connection.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timezone
import hashlib
from types import MappingProxyType
from typing import Iterable, Mapping, Sequence

from ..connections.model import AuthMode, ConnectionRecord, ConnectionState, SUPPORT_MATRIX
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import ExecutionRoute, new_id
from .evidence import EvidenceState, Fact, Provenance, ProvenanceSource
from .labels import build_label, disambiguate_labels
from .model import CatalogEntry, CatalogSnapshot
from . import model_seed

_PARAMETER_CATEGORIES = frozenset({"mode", "model_config", "thought_level"})
_KNOWN_CATEGORIES = _PARAMETER_CATEGORIES | {"model"}


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _invalid_observation(detail: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="invalid_observation",
        remediation=(
            "A discovery observation must be a well-formed list of ACP "
            f"SessionConfigOption payloads; {detail}."
        ),
        retry_eligible=False,
        field="observation",
    )


def _require(condition: bool, detail: str) -> None:
    if not condition:
        raise _invalid_observation(detail)


def _flatten_select_options(raw_options: object) -> tuple[tuple[str, str], ...]:
    """Flatten ungrouped or grouped ``SessionConfigSelectOptions`` to ``(value, name)`` pairs."""
    _require(isinstance(raw_options, list), "'options' must be a list")
    flattened: list[tuple[str, str]] = []
    for item in raw_options:
        _require(isinstance(item, dict), "each option/group must be an object")
        if "value" in item:
            value = item.get("value")
            name = item.get("name")
            _require(isinstance(value, str) and value, "option 'value' must be a non-empty string")
            _require(isinstance(name, str) and name, "option 'name' must be a non-empty string")
            flattened.append((value, name))
        elif "group" in item:
            options = item.get("options")
            _require(isinstance(options, list), "group 'options' must be a list")
            for sub in options:
                _require(isinstance(sub, dict), "each grouped option must be an object")
                value = sub.get("value")
                name = sub.get("name")
                _require(isinstance(value, str) and value, "grouped option 'value' must be a non-empty string")
                _require(isinstance(name, str) and name, "grouped option 'name' must be a non-empty string")
                flattened.append((value, name))
        else:
            raise _invalid_observation("each entry in 'options' must have 'value' or 'group'")
    return tuple(flattened)


@dataclass(frozen=True)
class _ParsedOption:
    id: str
    category: str | None
    type: str
    current_value: object
    select_options: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class Observation:
    """A validated ``configOptions`` payload for one connection.

    ``model_thought_levels`` (T035 follow-up "per-model efforts"; user
    decision 2026-09-18, verbatim: "store it keyed by model right away,
    even if the efforts are identical"): every model's ``thought_level``
    (effort) set is stored ``{model_slug: (effort, ...)}``, always keyed by
    model -- there is no separate "flat, engine-wide" shape and no fallback
    between the two. :func:`parse_observation` itself populates this for
    *every* model it lists, copying one shared flat set to each model when
    that is all a live ACP session actually advertised (e.g.
    claude-agent-acp's one flat ``default,low,medium,high,xhigh,max`` today
    -- the duplication across models is intentional, not something to
    optimize back into a shared value); :meth:`with_model_thought_levels`
    lets a caller holding a genuinely *per-model* source -- today only
    ``catalog.codex_debug_models.fetch_effort_ceiling``'s parse of codex's
    own offline ``codex debug models`` output, e.g. ``gpt-5.5`` lacking
    ``max``/``ultra`` that its sibling models have -- overlay real per-model
    values on top (only ``catalog.refresh.perform_refresh`` does this in
    production; the live session's own parsed default is never mutated in
    place, only merged into a new ``Observation``, keeping the two
    provenances distinct: a copied-flat entry still carries the live
    session's own provenance, never claimed as codex's per-model one).
    """

    options: tuple[_ParsedOption, ...]
    model_thought_levels: Mapping[str, tuple[str, ...]] = field(default_factory=dict)

    def model_options(self) -> tuple[tuple[str, str], ...]:
        for option in self.options:
            if option.category == "model" and option.type == "select":
                return option.select_options
        return ()

    def with_model_thought_levels(self, overrides: Mapping[str, tuple[str, ...]]) -> "Observation":
        """A new :class:`Observation` with ``overrides`` merged over the
        existing ``model_thought_levels`` (a model absent from ``overrides``
        keeps whatever it already had -- never dropped); never mutates
        ``self`` (frozen dataclass). Every value is still keyed by model,
        never a fallback shape."""
        merged = dict(self.model_thought_levels)
        merged.update({model: tuple(levels) for model, levels in overrides.items() if levels})
        return Observation(options=self.options, model_thought_levels=MappingProxyType(merged))

    def parameter_pairs_for(self, model: str) -> tuple[tuple[str, str], ...]:
        """The ``(name, value)`` pairs to fold into ``model``'s own
        ``ExecutionRoute.supported_parameters``: every ``mode``/
        ``model_config`` select pair and boolean pair (identical for every
        model of this observation -- see :meth:`parameter_pairs`), plus
        ``model``'s own ``thought_level`` values looked up from
        ``model_thought_levels`` -- always keyed by model, never a
        shared/flat value read directly off ``self.options``. A model with
        no entry in ``model_thought_levels`` simply gets no
        ``thought_level`` pairs, exactly like an observation whose
        ``thought_level`` select was absent entirely.
        """
        pairs = self.parameter_pairs()
        levels = self.model_thought_levels.get(model, ())
        return pairs + tuple(("thought_level", value) for value in levels)

    def parameter_pairs(self) -> tuple[tuple[str, str], ...]:
        """The model-independent ``(name, value)`` pairs to fold into a
        route's ``ExecutionRoute.supported_parameters``: every ``mode``/
        ``model_config`` select option's own advertised values (one pair
        per value, all sharing that category name -- never only the
        ``currentValue``, since a route's ``supported_parameters`` is the
        *allowed set* a later selection is validated against; see
        ``configuration.resolution.validate_dependent_settings``) plus
        every boolean option's own single current value keyed by its own
        ``id``. ``thought_level`` is deliberately never emitted here --
        see :attr:`model_thought_levels`/:meth:`parameter_pairs_for`, which
        is always model-keyed instead."""
        pairs: list[tuple[str, str]] = []
        for option in self.options:
            if option.type == "select" and option.category in _PARAMETER_CATEGORIES and option.category != "thought_level":
                for value, _name in option.select_options:
                    pairs.append((option.category, value))
            elif option.type == "boolean":
                pairs.append((option.id, "true" if option.current_value else "false"))
        return tuple(pairs)


def parse_observation(raw: object) -> Observation:
    """Strictly validate a raw ``configOptions`` list into an :class:`Observation`."""
    _require(isinstance(raw, list), "the payload must be a JSON array")
    parsed: list[_ParsedOption] = []
    seen_ids: set[str] = set()
    for item in raw:
        _require(isinstance(item, dict), "each configOption must be an object")
        option_id = item.get("id")
        name = item.get("name")
        option_type = item.get("type")
        category = item.get("category")
        _require(isinstance(option_id, str) and option_id, "'id' must be a non-empty string")
        _require(option_id not in seen_ids, f"duplicate option id {option_id!r}")
        seen_ids.add(option_id)
        _require(isinstance(name, str) and name, "'name' must be a non-empty string")
        _require(category is None or isinstance(category, str), "'category' must be a string or null")
        if option_type == "select":
            _require("currentValue" in item, "select option missing 'currentValue'")
            current_value = item["currentValue"]
            _require(isinstance(current_value, str) and current_value, "select 'currentValue' must be a non-empty string")
            select_options = _flatten_select_options(item.get("options"))
            values = {value for value, _ in select_options}
            _require(current_value in values, "select 'currentValue' must be one of its own 'options'")
            parsed.append(_ParsedOption(option_id, category, "select", current_value, select_options))
        elif option_type == "boolean":
            _require("currentValue" in item, "boolean option missing 'currentValue'")
            current_value = item["currentValue"]
            _require(isinstance(current_value, bool), "boolean 'currentValue' must be true/false")
            parsed.append(_ParsedOption(option_id, category, "boolean", current_value, ()))
        else:
            raise _invalid_observation(f"'type' must be 'select' or 'boolean', got {option_type!r}")
    observation = Observation(options=tuple(parsed))
    # T035 follow-up (user decision 2026-09-18): a live session's own
    # thought_level select is always copied into every model's own entry
    # in model_thought_levels at parse time -- never left as one implicit
    # shared value read off self.options -- even though every model gets
    # the identical set here (a genuinely per-model source, if any, is
    # overlaid later via with_model_thought_levels; see that method and
    # catalog.refresh.perform_refresh).
    thought_values = tuple(
        value
        for option in observation.options
        if option.type == "select" and option.category == "thought_level"
        for value, _name in option.select_options
    )
    if thought_values:
        models = [model for model, _name in observation.model_options()]
        observation = observation.with_model_thought_levels({model: thought_values for model in models})
    return observation


@dataclass(frozen=True)
class ManualDeclaration:
    """An adapter-supported model declared by hand, unverified until confirmed."""

    connection_id: str
    model: str
    parameters: tuple[tuple[str, str], ...] = ()


def _registry_fingerprint(connections: Mapping[str, ConnectionRecord]) -> str:
    hashed = hashlib.sha256()
    for connection_id in sorted(connections):
        record = connections[connection_id]
        hashed.update(connection_id.encode("utf-8"))
        hashed.update(record.approved_fingerprint.encode("utf-8"))
        hashed.update(record.state.value.encode("utf-8"))
    return hashed.hexdigest()


def _connection_facts(record: ConnectionRecord, adapter_version: str) -> EvidenceState:
    state = EvidenceState.empty()
    if record.state == ConnectionState.CONFIGURED:
        state = state.promote(
            Fact.CONFIGURED,
            Provenance(source=ProvenanceSource.DISCOVERY, adapter_version=adapter_version, observed_at=_now()),
        )
    elif record.state == ConnectionState.AUTHORIZATION_REQUIRED:
        state = state.promote(
            Fact.AUTHORIZATION_REQUIRED,
            Provenance(source=ProvenanceSource.DISCOVERY, adapter_version=adapter_version, observed_at=_now()),
        )
    return state


def seed_observation(engine: str) -> "Observation | None":
    """A synthetic ``Observation`` built from :data:`model_seed.SEED_MODELS`
    for ``engine`` (T035), or ``None`` when this engine has no seed -- see
    ``model_seed``'s module docstring for sourcing. Carries only a
    ``model`` select, never a fabricated ``thought_level`` (see
    ``model_seed``'s "known ceiling" note). A caller must pass this
    connection's id in ``build_catalog``'s ``seeded_connection_ids`` so the
    resulting routes are promoted with ``ProvenanceSource.SEED``, never
    ``DISCOVERY``."""
    pairs = model_seed.SEED_MODELS.get(engine)
    if not pairs:
        return None
    raw = [dict(
        id="model", name="Model", type="select", category="model",
        currentValue=pairs[0][0],
        options=[dict(value=value, name=name) for value, name in pairs],
    )]
    return parse_observation(raw)


def build_catalog(
    connections: Mapping[str, ConnectionRecord],
    observations: Mapping[str, Observation | None],
    manual_declarations: Sequence[ManualDeclaration] = (),
    adapter_versions: Mapping[str, str] | None = None,
    seeded_connection_ids: frozenset[str] = frozenset(),
) -> CatalogSnapshot:
    """Assemble one :class:`CatalogSnapshot`; never crosses connections.

    ``seeded_connection_ids`` (T035): connection ids whose ``observations``
    entry is a synthetic :func:`seed_observation`, not a real captured
    engine payload -- their routes' ``catalog_obtained`` fact is promoted
    with :class:`~.evidence.ProvenanceSource.SEED` instead of ``DISCOVERY``,
    so ``acp.config_options`` can honestly label them "seeded" rather than
    engine-confirmed. Every other connection is unaffected (unchanged
    pre-T035 ``DISCOVERY`` provenance).
    """
    adapter_versions = adapter_versions or {}
    now = _now()
    snapshot_id = new_id("cs_")
    entries: list[CatalogEntry] = []
    unavailable: dict[str, str] = {}
    label_pairs: list[tuple[str, str]] = []
    raw_labels: dict[str, str] = {}

    for connection_id, record in connections.items():
        adapter_version = adapter_versions.get(connection_id, "unknown")
        base_facts = _connection_facts(record, adapter_version)
        observation = observations.get(connection_id)
        if observation is None:
            reason = "authorization_required" if record.state == ConnectionState.AUTHORIZATION_REQUIRED else "discovery_unavailable"
            unavailable[connection_id] = reason
            continue
        for model, _model_name in observation.model_options():
            # T035 follow-up: per-model override when the caller attached
            # one (see Observation.with_model_thought_levels); an unchanged
            # flat parameter_pairs() for every model with none (the
            # overwhelming default, identical to every pre-existing
            # observation/test).
            supported_parameters = observation.parameter_pairs_for(model)
            route = ExecutionRoute(
                engine=record.engine,
                connection_id=connection_id,
                model=model,
                supported_parameters=supported_parameters,
            )
            source = ProvenanceSource.SEED if connection_id in seeded_connection_ids else ProvenanceSource.DISCOVERY
            evidence = base_facts.promote(
                Fact.CATALOG_OBTAINED,
                Provenance(source=source, adapter_version=adapter_version, observed_at=now),
            )
            label = build_label(record.engine, record.name, model)
            raw_labels[route.id] = label
            label_pairs.append((route.id, label))
            entries.append((route, connection_id, label, evidence))

    for declaration in manual_declarations:
        record = connections.get(declaration.connection_id)
        if record is None:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="unknown_connection",
                remediation="A manual declaration must reference an existing connection id.",
                retry_eligible=False,
                field="connection_id",
            )
        support = SUPPORT_MATRIX.get(record.engine, {}).get(record.auth_mode)
        if support is None or not support.qualified:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="unsupported_manual_declaration",
                remediation=(
                    "A manual declaration is only accepted for a documented/qualified "
                    "(engine, auth_mode) combination; gateway/unqualified modes are never "
                    "manually declarable."
                ),
                retry_eligible=False,
                field="connection_id",
            )
        if declaration.parameters:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="unsupported_manual_parameters",
                remediation=(
                    "Without a discovery observation there is no confirmed allowed-parameter "
                    "set for this connection; a manual declaration must supply no parameters."
                ),
                retry_eligible=False,
                field="parameters",
            )
        adapter_version = adapter_versions.get(declaration.connection_id, "unknown")
        route = ExecutionRoute(
            engine=record.engine,
            connection_id=declaration.connection_id,
            model=declaration.model,
            supported_parameters=(),
        )
        evidence = _connection_facts(record, adapter_version)
        label = build_label(record.engine, record.name, declaration.model)
        raw_labels[route.id] = label
        label_pairs.append((route.id, label))
        entries.append((route, declaration.connection_id, label, evidence))

    disambiguated = disambiguate_labels(label_pairs)
    catalog_entries = tuple(
        CatalogEntry(
            route=route,
            connection_id=connection_id,
            label=label,
            disambiguated_label=disambiguated[route.id],
            evidence=evidence,
            catalog_snapshot_id=snapshot_id,
        )
        for route, connection_id, label, evidence in entries
    )
    return CatalogSnapshot(
        id=snapshot_id,
        created_at=now,
        connection_registry_fingerprint=_registry_fingerprint(connections),
        entries=catalog_entries,
        unavailable=unavailable,
        setup_action="add_connection" if not connections else None,
    )
