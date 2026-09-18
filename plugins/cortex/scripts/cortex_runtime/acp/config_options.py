"""Build truthful ACP ``SessionConfigOption`` entries for one ACP session.

Three independent axes exist (T035; previously ``model`` alone fused
provider+engine+model into one opaque route select -- that fusion is
undone here): ``provider`` (category ``provider``), ``model`` (category
``model``, scoped to the chosen provider), and ``thought_level`` (category
``thought_level``, scoped to the chosen model). All three are built from
the *actual* connection registry + cached catalog snapshots
(``catalog.store``, falling back to a static, honestly-labelled
``catalog.model_seed`` seed when a connection has never yet been observed
live -- see ``selectable_entries_from_snapshots``) and the task's own
configuration history (``configuration.store`` / ``ConfigurationService``)
-- never a fabricated placeholder value.

- ``provider``: values are distinct ``CatalogEntry.connection_id`` among
  every entry currently selectable for a new execution; names are safe
  ``<short engine name> · account`` labels (``catalog.labels
  .build_provider_label``, using ``catalog.labels.engine_short_name`` (e.g.
  ``claude``/``codex``) and the connection's own resolved account identity
  (``connections.account_identity.resolve_account_identity``, read-only
  from that connection's own isolated credential directory) -- never the
  raw adapter package id and never the operator-chosen connection
  nickname; falls back to the engine name alone when no account can be
  determined).
- ``model``: values are the chosen provider's own ``ExecutionRoute.model``
  ids; names are safe ``Engine · Connection name · Model`` labels,
  re-disambiguated across the *combined* offered set. If there is nothing
  selectable, the option is omitted entirely -- never an empty select,
  never a fake value.
- ``thought_level``: values are exactly the ``(name, value)`` pairs recorded
  on the *currently selected* route's ``ExecutionRoute.supported_parameters``
  whose name is ``"thought_level"`` (the same dependent-setting name used by
  ``configuration.resolution``/``catalog.discovery``). Omitted when the
  route reports no such parameter.

Choosing provider+model always resolves to one concrete
``ExecutionRoute``/configuration revision through the existing T008 flow
(``configuration.resolution``/``configuration.service.ConfigurationService``
-- see ``acp.agent._apply_config_selection``); an unsupported value is
refused, never silently accepted.

None of these are ever emitted for team/permission/spend/summarizer
settings; those are out of scope for this task (T010).
"""
from __future__ import annotations

from typing import Iterable, Mapping, Sequence

from ..catalog import discovery as catalog_discovery
from ..catalog import store as catalog_store
from ..catalog.evidence import Fact, ProvenanceSource
from ..catalog.labels import build_model_label, build_provider_label, disambiguate_labels, engine_short_name
from ..catalog.model import CatalogEntry
from ..connections import account_identity as connection_account_identity
from ..connections import registry as connection_registry
from ..connections.model import ConnectionRecord

# T035: three independent axes -- provider (which configured connection),
# model (within that provider), and thought_level (reasoning effort) --
# never a single fused "model" select whose values were whole routes
# (provider+engine+model). See build_provider_option/build_model_option/
# build_thought_level_option and build_config_options, which resolves them
# in that order, each depending only on the one before it.
PROVIDER_OPTION_ID = "provider"
MODEL_OPTION_ID = "model"
THOUGHT_LEVEL_OPTION_ID = "thought_level"
# T020: a third, independent configId -- never tied to model/route
# selectability (unlike model/thought_level, always offered once a caller
# opts in via build_config_options(..., include_language=True), including
# before any connection/model exists). See acp.steering.get_language_override/
# set_language_override (per-task) and acp.commands.CORTEX_LANGUAGE (the
# slash-command equivalent for a host without SessionConfigOption support).
LANGUAGE_OPTION_ID = "language"
_LANGUAGE_VALUES = ("auto", "en", "ru")
_LANGUAGE_NAMES = {"auto": "Auto (detect from your messages)", "en": "English", "ru": "Russian"}

# _meta._cortex.state values meaning "the saved/selected route is no longer
# selectable and currentValue is a substitute, never the saved choice" --
# see build_model_option. Callers (acp.agent) must never auto-propose in
# this state.
UNAVAILABLE_STATES = frozenset({"default_unavailable", "selection_unavailable"})

# Configuration revision states counted as "the most recent selection" for
# currentValue purposes (see module docstring); 'rejected'/'failed' never
# count -- a rejected proposal must not silently become the shown value.
_LIVE_SELECTION_STATES = frozenset({"selected", "pending_next_use", "active"})

_CONFIRMED_FACTS = (Fact.CONFIGURATION_APPLIED, Fact.EXECUTION_VERIFIED)


def _evidence_note(entry: CatalogEntry) -> str:
    if any(entry.evidence.has(fact) for fact in _CONFIRMED_FACTS):
        return "confirmed by an engine"
    # T035: a statically-seeded model (catalog.model_seed, never an
    # engine's own advertisement) is labelled "seeded", distinctly from
    # "unverified" (a real discovery observation that just hasn't been
    # executed yet) -- never reported as observed/confirmed.
    obtained = entry.evidence.get(Fact.CATALOG_OBTAINED)
    if obtained is not None and obtained.source == ProvenanceSource.SEED:
        return "seeded — not yet confirmed by an engine"
    return "unverified — not yet confirmed by an engine"


def _state_suffix(state: str | None) -> str | None:
    if state in ("selected", "pending_next_use"):
        return "(pending — applies to the next run)"
    if state == "active":
        return "(active)"
    # state == "default" (a pre-task session showing an inherited, unchanged
    # persisted user default -- see acp.agent._current_selection_context) or
    # None (no selection at all yet) both get no suffix: neither is pending.
    return None


def _cortex_meta(state: str | None, revision: int | None) -> dict | None:
    if state is None:
        return None
    return {"_cortex": {"state": state, "revision": revision}}


def _is_cross_connection_placeholder(view: catalog_store.ConnectionSnapshotView) -> bool:
    """True when ``view`` is not a genuine observation of its own
    connection at all, but the entry-less placeholder ``catalog.store
    .save_snapshot`` writes into *every other* connection's directory that
    a ``build_catalog`` call's ``connections`` mapping happened to include
    without ever actually attempting to observe it (T035 FINDING-07 --
    e.g. a pre-fix ``pin_route`` call declaring one connection while
    passing the *whole* registry).

    ``catalog.discovery.build_catalog`` only ever sets
    ``unavailable[connection_id]`` (-> this file's ``unavailable_reason``)
    when that connection's own ``observations`` entry was ``None`` --
    "this call made no observation attempt for you", never "I observed
    you and you have nothing". A genuine zero-model observation (an
    engine whose payload legitimately advertised no ``model`` option) has
    ``unavailable_reason is None`` and remains fully honoured -- see the
    module docstring's "distinguishable" requirement -- only the noise
    case (empty entries *and* a reason) is skipped here.
    """
    return not view.entries and view.unavailable_reason is not None


def selectable_entries_from_snapshots(
    connections: Mapping[str, ConnectionRecord], environ: Mapping[str, str] | None = None
) -> tuple[CatalogEntry, ...]:
    """Every catalog entry selectable for a new execution, across every
    connection's most-recently-cached *genuine* snapshot file.

    A connection with no cached snapshot (never discovered/refreshed) never
    contributes any entry -- it is simply absent, not a fabricated one.

    Picks each connection's newest file that actually describes that
    connection (see ``_is_cross_connection_placeholder``), not merely its
    newest file by mtime: a placeholder written into this connection's
    directory by an unrelated ``build_catalog`` call for a sibling
    connection is skipped in favour of an older, real snapshot underneath
    it (T035 FINDING-07). If every cached file for a connection turns out
    to be such a placeholder, that connection is treated exactly like one
    with no cached snapshot at all -- falling back to its seed catalog
    below, never silently dropped.
    """
    selectable: list[CatalogEntry] = []
    catalog_dir = connection_registry.state_root(environ) / "catalog"
    for connection_id, record in connections.items():
        directory = catalog_dir / connection_id
        files = []
        if directory.is_dir():
            files = sorted(
                (path for path in directory.glob("cs_*.json") if path.is_file()),
                key=lambda path: path.stat().st_mtime,
                reverse=True,
            )
        view = None
        for path in files:
            candidate = catalog_store.load_connection_snapshot(connection_id, path.stem, environ)
            if candidate is None:
                continue
            if _is_cross_connection_placeholder(candidate):
                continue
            view = candidate
            break
        if view is None:
            # T035: no real snapshot has ever been cached for this
            # connection yet -- fall back to its engine's static seed
            # catalog (never persisted to disk; recomputed every call, and
            # unconditionally superseded the moment a real snapshot file
            # exists -- see catalog.discovery.seed_observation).
            observation = catalog_discovery.seed_observation(record.engine)
            if observation is None:
                continue
            snapshot = catalog_discovery.build_catalog(
                {connection_id: record}, {connection_id: observation},
                seeded_connection_ids=frozenset({connection_id}))
            for entry in snapshot.entries:
                if entry.is_selectable(record.state, record.auth_mode.value):
                    selectable.append(entry)
            continue
        for entry in view.entries:
            if entry.is_selectable(record.state, record.auth_mode.value):
                selectable.append(entry)
    return tuple(selectable)


_UNAVAILABLE_DESCRIPTION = "Saved route is unavailable — choose a route"
_UNAVAILABLE_THOUGHT_LEVEL_DESCRIPTION = "Depends on an unavailable route — choose a route"


def build_provider_option(
    selectable_entries: Sequence[CatalogEntry],
    connections: Mapping[str, ConnectionRecord],
    current_connection_id: str | None,
    current_state: str | None,
    current_revision: int | None,
    environ: Mapping[str, str] | None = None,
) -> dict | None:
    """The ``provider`` SessionConfigOption -- which configured connection --
    or ``None`` when nothing is selectable (T035; account identity added
    for the "which account" follow-up).

    One option per distinct ``CatalogEntry.connection_id`` among
    ``selectable_entries``; ``name`` is the safe ``<short engine name> ·
    account`` label (``catalog.labels.build_provider_label``, account
    resolved read-only from *this connection's own* isolated credential
    directory by ``connections.account_identity.resolve_account_identity``
    -- never the operator-chosen connection nickname, and never an
    endpoint/credential/raw adapter package id). When the account cannot be
    determined the label falls back to the engine name alone -- never a
    fabricated value and never the nickname. When ``current_connection_id`` is set but not among
    the selectable connections (its connection was removed/deauthorized, or
    it is the raw, never-a-real-connection-id saved ``route_id`` passed
    straight through by ``build_config_options`` when the saved route itself
    couldn't be resolved at all) the *first* selectable connection is shown
    as ``currentValue`` -- never silently: same
    ``"default_unavailable"``/``"selection_unavailable"`` marker convention
    as the (former, T010) fused ``model`` option used.
    """
    if not selectable_entries:
        return None
    first_entry_by_connection: dict[str, CatalogEntry] = {}
    for entry in selectable_entries:
        first_entry_by_connection.setdefault(entry.connection_id, entry)
    pairs = []
    for connection_id, entry in first_entry_by_connection.items():
        record = connections.get(connection_id)
        if record is not None:
            account = connection_account_identity.resolve_account_identity(record, environ)
            label = build_provider_label(entry.route.engine, account)
        else:
            label = engine_short_name(entry.route.engine)
        pairs.append((connection_id, label))
    labels = disambiguate_labels(pairs)
    options = [dict(value=connection_id, name=labels[connection_id]) for connection_id, _label in pairs]
    value_ids = set(first_entry_by_connection)
    unavailable = current_connection_id is not None and current_connection_id not in value_ids
    if unavailable:
        current_value = pairs[0][0]
        effective_state = "default_unavailable" if current_state == "default" else "selection_unavailable"
        description = _UNAVAILABLE_DESCRIPTION
    else:
        current_value = current_connection_id if current_connection_id is not None else pairs[0][0]
        effective_state = current_state
        description = _state_suffix(current_state)
    option: dict = dict(id=PROVIDER_OPTION_ID, name="Provider", type="select", category="provider",
                         currentValue=current_value, options=options)
    if description:
        option["description"] = description
    meta = _cortex_meta(effective_state, current_revision)
    if meta:
        option["_meta"] = meta
    return option


def build_model_option(
    provider_entries: Sequence[CatalogEntry],
    current_model: str | None,
    current_state: str | None,
    current_revision: int | None,
) -> dict | None:
    """The ``model`` SessionConfigOption for the currently chosen provider --
    values are plain ``ExecutionRoute.model`` ids (T035; previously whole
    fused route ids), or ``None`` when the provider has no selectable model.
    ``name`` is the bare, sanitized model id (``catalog.labels
    .build_model_label``) -- never the full ``Engine · Connection · Model``
    route label (``CatalogEntry.label``), which the ``provider`` option
    already conveys via its own ``Engine · Connection name`` label; repeating
    that identical prefix on every model entry made them render identically
    once a host truncated the shared prefix (FINDING-04).

    ``current_state`` must be the *provider* option's own effective state
    (see ``build_config_options``, mirroring how ``build_thought_level_option``
    already receives the model option's effective state): when the provider
    is itself unavailable (``current_state`` in ``UNAVAILABLE_STATES``) this
    option cascades the identical marker rather than presenting a stale
    saved model as if selected. Independently self-detects an unavailable
    ``current_model`` that simply isn't among this provider's own models
    (e.g. a persisted default from a provider that has since dropped that
    model) the same way.
    """
    if not provider_entries:
        return None
    first_entry_by_model: dict[str, CatalogEntry] = {}
    for entry in provider_entries:
        first_entry_by_model.setdefault(entry.route.model, entry)
    pairs = [(model, build_model_label(entry.route.model)) for model, entry in first_entry_by_model.items()]
    labels = disambiguate_labels(pairs)
    options = [
        dict(value=model, name=labels[model], description=_evidence_note(entry))
        for model, entry in first_entry_by_model.items()
    ]
    value_ids = set(first_entry_by_model)
    unavailable = current_state in UNAVAILABLE_STATES or (
        current_model is not None and current_model not in value_ids)
    if unavailable:
        current_value = pairs[0][0]
        effective_state = current_state if current_state in UNAVAILABLE_STATES else (
            "default_unavailable" if current_state == "default" else "selection_unavailable")
        description = _UNAVAILABLE_DESCRIPTION
    else:
        current_value = current_model if current_model is not None else pairs[0][0]
        effective_state = current_state
        description = _state_suffix(current_state)
    option: dict = dict(id=MODEL_OPTION_ID, name="Model", type="select", category="model",
                         currentValue=current_value, options=options)
    if description:
        option["description"] = description
    meta = _cortex_meta(effective_state, current_revision)
    if meta:
        option["_meta"] = meta
    return option


def build_thought_level_option(
    current_entry: CatalogEntry | None,
    current_parameters: Mapping[str, object],
    current_state: str | None,
    current_revision: int | None,
) -> dict | None:
    """The ``thought_level`` SessionConfigOption, or ``None`` when the
    currently selected route reports no ``thought_level`` parameter.

    ``current_state`` must be the *model* option's effective state (see
    ``build_config_options``, which passes the model option's own
    ``_meta._cortex.state`` here, never the raw ``current["state"]``): when
    the model route is unavailable (``current_state`` in
    ``UNAVAILABLE_STATES``), ``current_entry`` is already the *substituted*
    route (see ``build_config_options``), and this option must never present
    the stale saved ``current_parameters["thought_level"]`` value as if it
    were actually selected -- ``currentValue`` is always the substituted
    route's first supported value, and the same unavailable marker/
    description is carried here too.
    """
    if current_entry is None:
        return None
    values = sorted({
        value for name, value in current_entry.route.supported_parameters if name == THOUGHT_LEVEL_OPTION_ID
    })
    if not values:
        return None
    options = [dict(value=value, name=value) for value in values]
    unavailable = current_state in UNAVAILABLE_STATES
    if unavailable:
        current = values[0]
        description = _UNAVAILABLE_THOUGHT_LEVEL_DESCRIPTION
    else:
        current = current_parameters.get(THOUGHT_LEVEL_OPTION_ID) if current_parameters else None
        if current not in values:
            current = values[0]
        description = _state_suffix(current_state)
    option: dict = dict(id=THOUGHT_LEVEL_OPTION_ID, name="Thought level", type="select", category="thought_level",
                         currentValue=current, options=options)
    if description:
        option["description"] = description
    meta = _cortex_meta(current_state, current_revision)
    if meta:
        option["_meta"] = meta
    return option


def build_language_option(current_value: str | None) -> dict:
    """The ``language`` SessionConfigOption -- ``auto``/``en``/``ru``.
    ``current_value`` is the stored override (``None`` means "auto", the
    default); any other/unknown stored value also falls back to shown as
    ``"auto"`` rather than fabricating an unlisted value."""
    current = current_value if current_value in _LANGUAGE_VALUES else "auto"
    return dict(
        id=LANGUAGE_OPTION_ID, name="Language", type="select", category="other",
        currentValue=current,
        description="Language for Cortex's own status/question/stop lines; auto detects from your messages.",
        options=[dict(value=value, name=_LANGUAGE_NAMES[value]) for value in _LANGUAGE_VALUES],
    )


def build_config_options(
    selectable_entries: Sequence[CatalogEntry],
    connections: Mapping[str, ConnectionRecord],
    current: Mapping[str, object] | None = None,
    *, include_language: bool = False, language_current: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> list[dict]:
    """The full ordered ``configOptions`` list -- ``provider``, then
    ``model`` (within that provider), then ``thought_level`` (within that
    model), then -- only when ``include_language`` is explicitly requested
    (see ``acp.agent._config_options_for_session``, which always sets it) --
    ``language`` (T035: three independent axes, never one fused route
    select; see module docstring and ``build_provider_option``/
    ``build_model_option``).

    ``current`` is ``None`` or ``{"route_id", "state", "revision", "parameters"}``
    describing the most recent live selection (see
    ``_LIVE_SELECTION_STATES``) -- still keyed by the one concrete
    ``ExecutionRoute.id`` a provider+model pair resolves to (see
    ``configuration.resolution``/T008), not by separate provider/model
    fields; this function is what derives the provider/model axes back out
    of it. A task-less session (no selection yet) is represented as
    ``current=None`` and simply shows each axis's first selectable value as
    the (unstated, non-pending) default.

    ``include_language``/``language_current`` are keyword-only and default
    to omitting the language option entirely -- every pre-T020 direct call
    (and every existing test of this exact function) keeps returning ``[]``
    for "nothing selectable, no language requested", unchanged. Unlike
    provider/model/thought_level, the language option is appended even when
    ``selectable_entries`` is empty (no model is selectable yet): language
    is never gated on route/connection availability.
    """
    current = current or {}
    options: list[dict] = []
    current_route_id = current.get("route_id")
    current_state = current.get("state")
    current_revision = current.get("revision")
    current_entry = None
    if current_route_id is not None:
        current_entry = next((entry for entry in selectable_entries if entry.route.id == current_route_id), None)
    # When the saved route couldn't be resolved at all, pass the raw saved
    # route_id straight through as the "current" connection/model value: it
    # can never equal a real connection_id ("cn_...") or model id, so
    # build_provider_option/build_model_option's own self-detection reliably
    # (and consistently with the pre-T035 build_model_option's identical
    # trick) treats it as unavailable rather than "no selection at all".
    current_connection_id = current_entry.connection_id if current_entry is not None else current_route_id

    provider_option = build_provider_option(
        selectable_entries, connections, current_connection_id, current_state, current_revision, environ)
    if provider_option is None:
        if include_language:
            options.append(build_language_option(language_current))
        return options
    options.append(provider_option)
    provider_effective_state = provider_option.get("_meta", {}).get("_cortex", {}).get("state", current_state)
    chosen_connection_id = provider_option["currentValue"]
    provider_entries = [entry for entry in selectable_entries if entry.connection_id == chosen_connection_id]
    current_model = current_entry.route.model if current_entry is not None else None

    model_option = build_model_option(provider_entries, current_model, provider_effective_state, current_revision)
    if model_option is not None:
        options.append(model_option)
        chosen_model = model_option["currentValue"]
        chosen_entry = next((entry for entry in provider_entries if entry.route.model == chosen_model), None)
        # Always the model option's own *effective* state (never the raw
        # current["state"]): when the provider/route is unavailable, the
        # model option's state is already "default_unavailable"/
        # "selection_unavailable" (see build_model_option), and
        # build_thought_level_option must see that same effective state to
        # carry the identical marker instead of presenting a stale saved
        # value.
        model_effective_state = model_option.get("_meta", {}).get("_cortex", {}).get("state", provider_effective_state)
        thought_option = build_thought_level_option(
            chosen_entry, current.get("parameters") or {}, model_effective_state, current_revision)
        if thought_option is not None:
            options.append(thought_option)
    if include_language:
        options.append(build_language_option(language_current))
    return options


def most_recent_live_selection(history: Iterable[Mapping[str, object]]) -> dict | None:
    """The highest-revision entry in ``history`` whose state counts as a
    "live" selection (see ``_LIVE_SELECTION_STATES``), or ``None``."""
    best = None
    for row in history:
        if row.get("state") not in _LIVE_SELECTION_STATES:
            continue
        if best is None or row["revision"] > best["revision"]:
            best = row
    return best


# --- boolean-as-select rendering (unit-tested helper; no boolean setting is
#     emitted by this task -- see module docstring) --------------------------

def render_boolean_as_select(
    option_id: str,
    name: str,
    current_value: bool,
    *,
    category: str | None = None,
    description: str | None = None,
    client_capabilities: Mapping[str, object] | None = None,
    true_label: str = "On",
    false_label: str = "Off",
) -> dict:
    """Render a boolean-typed setting.

    Emits a real ``type: "boolean"`` option only when the client's
    ``clientCapabilities.session.configOptions.boolean`` is present
    (``{}`` or any object counts as support, per the pinned schema: "Omitted
    or null both mean the client does not advertise support"); otherwise
    renders the identical setting as an on/off ``select`` with two options
    so every host, including one with no boolean selector support, gets a
    valid, working configuration surface.
    """
    capabilities = client_capabilities or {}
    session_caps = capabilities.get("session") if isinstance(capabilities, Mapping) else None
    config_caps = session_caps.get("configOptions") if isinstance(session_caps, Mapping) else None
    supports_boolean = isinstance(config_caps, Mapping) and config_caps.get("boolean") is not None

    option: dict = dict(id=option_id, name=name)
    if category:
        option["category"] = category
    if description:
        option["description"] = description
    if supports_boolean:
        option["type"] = "boolean"
        option["currentValue"] = bool(current_value)
    else:
        option["type"] = "select"
        option["currentValue"] = "true" if current_value else "false"
        option["options"] = [dict(value="true", name=true_label), dict(value="false", name=false_label)]
    return option
