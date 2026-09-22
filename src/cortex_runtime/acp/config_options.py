"""Build truthful ACP ``SessionConfigOption`` entries for one ACP session.

Zed receives three independent axes: ``provider`` (category ``provider``),
``model`` (category ``model``, scoped to the chosen provider), and
``thought_level`` (category ``thought_level``, scoped to the chosen model).
    Paseo renders only the ``model`` category in its picker, so its model option
    lists every selectable route with exact route IDs and unambiguous labels;
    selecting one also changes the provider. Its provider axis is intentionally
    omitted because the selected route already identifies the account. All
    options are built from
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
- ``model``: Zed values are the chosen provider's own
  ``ExecutionRoute.model`` ids. Paseo values are exact route IDs across all
  providers, with the model name first and a short provider name beneath it.
  If there is nothing
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
from ..catalog.labels import build_model_label, build_provider_label, disambiguate_labels, engine_short_name
from ..catalog.model import CatalogEntry
from ..connections import account_identity as connection_account_identity
from ..connections import onboarding as connection_onboarding
from ..connections import registry as connection_registry
from ..connections.model import ConnectionRecord
from ..portable.errors import PortableError


def filter_denied_entries(
    entries: Sequence[CatalogEntry], project_root: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> tuple[CatalogEntry, ...]:
    """Drop routes the user listed under ``deny`` in ``routing.json`` (see
    ``routing.auto_route``), so the selector never offers what dispatch
    would refuse. A malformed file is left to dispatch to report."""
    from ..routing import auto_route

    return tuple(
        entry for entry in entries
        if not auto_route.is_denied(entry.route.engine, entry.route.model, project_root, environ)
    )

# T035: three independent axes -- provider (which configured connection),
# model (within that provider), and thought_level (reasoning effort) --
# never a single fused "model" select whose values were whole routes
# (provider+engine+model). See build_provider_option/build_model_option/
# build_thought_level_option and build_config_options, which resolves them
# in that order, each depending only on the one before it.
PROVIDER_OPTION_ID = "provider"
MODEL_OPTION_ID = "model"
THOUGHT_LEVEL_OPTION_ID = "thought_level"
_EFFORT_STRENGTH = {name: index for index, name in enumerate(
    ("low", "medium", "high", "xhigh", "max", "ultra")
)}
_ENGINE_DEFAULT_VALUE = "default"
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

    Each connection's selectability is decided from its **live** state (T035
    FINDING-10), never the stored/cached ``ConnectionRecord.state`` a caller
    happened to pass in: this function re-derives it here via
    ``connections.onboarding.validate`` -- the identical call the launch
    path (``onboarding.resolve_connection_for_route``) already makes -- so a
    connection whose credential was just restored becomes selectable again
    without any separate revalidation step, and one whose credential just
    disappeared stops being offered even though the registry's stored
    record still says ``configured``. ``validate`` only does bounded local
    filesystem stats (never a network call) and only writes back when the
    derived state actually changed, so this costs one cheap local
    read-modify-maybe-write per connection -- run at most once per
    connection per ``session/new``/``session/load``/config rebuild, the same
    frequency the pre-fix, non-self-healing stored-state read already ran
    at. A connection absent from the registry entirely (e.g. a
    caller-constructed ``ConnectionRecord`` never persisted, as in a few
    direct-call unit tests) falls back to the record's own given state
    rather than failing the whole selection surface.

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
        try:
            record = connection_onboarding.validate(connection_id, environ=environ)
        except PortableError:
            # Not (or no longer) in the registry -- use the given record's
            # own state rather than failing the whole selection surface
            # (covers direct-call sites that pass an ad-hoc, unpersisted
            # ConnectionRecord).
            pass
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


def _model_description(
    entry: CatalogEntry,
    connections: Mapping[str, ConnectionRecord] | None,
    environ: Mapping[str, str] | None,
) -> str:
    """Paseo's grey line identifies the provider, without repeating the model."""
    return engine_short_name(entry.route.engine)


def build_model_option(
    provider_entries: Sequence[CatalogEntry],
    current_model: str | None,
    current_state: str | None,
    current_revision: int | None,
    *,
    connections: Mapping[str, ConnectionRecord] | None = None,
    environ: Mapping[str, str] | None = None,
) -> dict | None:
    """The ``model`` SessionConfigOption for the currently chosen provider --
    values are plain ``ExecutionRoute.model`` ids (T035; previously whole
    fused route ids), or ``None`` when the provider has no selectable model.
    ``name`` is the engine-provided friendly model name when available, with
    the stable model id as a backwards-compatible fallback. Engine/account
    provider name is placed in the secondary description.

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
    provider_entries = tuple(
        entry for entry in provider_entries
        if entry.route.model.casefold() != _ENGINE_DEFAULT_VALUE
    )
    if not provider_entries:
        return None
    first_entry_by_model: dict[str, CatalogEntry] = {}
    for entry in provider_entries:
        first_entry_by_model.setdefault(entry.route.model, entry)
    pairs = [
        (model, build_model_label(entry.model_name or entry.route.model))
        for model, entry in first_entry_by_model.items()
    ]
    labels = disambiguate_labels(pairs)
    options = [
        dict(value=model, name=labels[model], description=_model_description(entry, connections, environ))
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


def build_combined_model_option(
    selectable_entries: Sequence[CatalogEntry],
    connections: Mapping[str, ConnectionRecord],
    current_route_id: str | None,
    fallback_connection_id: str,
    current_state: str | None,
    current_revision: int | None,
    environ: Mapping[str, str] | None = None,
) -> dict | None:
    """Offer every selectable route in clients that expose only a model picker.

    A model ID is the value when it names exactly one selectable route. Paseo's
    CLI supplies that ID for ``--model`` and validates it against these values
    before issuing ``session/set_config_option``. Duplicate model IDs retain
    exact route IDs so a choice can never silently select another account.
    The provider option is intentionally omitted by ``build_config_options``
    when this combined representation is selected; the route value already
    identifies the provider/account and Paseo must not render a duplicate
    provider selector.
    """
    selectable_entries = tuple(
        entry for entry in selectable_entries
        if entry.route.model.casefold() != _ENGINE_DEFAULT_VALUE
    )
    if not selectable_entries:
        return None
    values = combined_model_values(selectable_entries)
    pairs = [(entry.route.id, build_model_label(entry.model_name or entry.route.model))
             for entry in selectable_entries]
    labels = disambiguate_labels(pairs)
    options = [
        dict(value=values[entry.route.id], name=labels[entry.route.id],
             description=_model_description(entry, connections, environ))
        for entry in selectable_entries
    ]
    ids = {entry.route.id for entry in selectable_entries}
    unavailable = current_route_id is not None and current_route_id not in ids
    if unavailable:
        current_value = next(
            (values[entry.route.id] for entry in selectable_entries
             if entry.connection_id == fallback_connection_id),
            values[selectable_entries[0].route.id],
        )
        effective_state = "default_unavailable" if current_state == "default" else "selection_unavailable"
        description = _UNAVAILABLE_DESCRIPTION
    else:
        current_value = values[current_route_id] if current_route_id else next(
            (values[entry.route.id] for entry in selectable_entries
             if entry.connection_id == fallback_connection_id),
            values[selectable_entries[0].route.id],
        )
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


def combined_model_values(selectable_entries: Sequence[CatalogEntry]) -> dict[str, str]:
    """Map each route to a unique value accepted by Paseo's model selector."""
    counts: dict[str, int] = {}
    route_ids = {entry.route.id for entry in selectable_entries}
    for entry in selectable_entries:
        counts[entry.route.model] = counts.get(entry.route.model, 0) + 1
    return {
        entry.route.id: (
            entry.route.model
            if counts[entry.route.model] == 1 and entry.route.model not in route_ids
            else entry.route.id
        )
        for entry in selectable_entries
    }


def _allowed_efforts(entry, selectable_entries, project_root, environ) -> tuple[str, ...] | None:
    """The same effort rule automatic routing applies (``routing.auto_route
    .permitted_efforts``), so the selector never offers what dispatch would
    lower anyway."""
    if entry is None:
        return None
    from ..routing import auto_route

    try:
        tier_overrides = auto_route.load_overrides(project_root, environ).get("tiers") or {}
    except PortableError:
        tier_overrides = {}
    tiers = auto_route.model_tiers(selectable_entries, tier_overrides)
    return auto_route.permitted_efforts(entry, tiers[entry.route.id])


def build_thought_level_option(
    current_entry: CatalogEntry | None,
    current_parameters: Mapping[str, object],
    current_state: str | None,
    current_revision: int | None,
    *, allowed_efforts: Sequence[str] | None = None,
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
        value for name, value in current_entry.route.supported_parameters
        if name == THOUGHT_LEVEL_OPTION_ID and value.casefold() != _ENGINE_DEFAULT_VALUE
    }, key=lambda value: (_EFFORT_STRENGTH.get(value.casefold(), len(_EFFORT_STRENGTH)), value.casefold()))
    if allowed_efforts is not None:
        values = [value for value in values if value in allowed_efforts]
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


def build_language_option(current_value: str | None, *, as_mode: bool = False) -> dict:
    """The ``language`` SessionConfigOption -- ``auto``/``en``/``ru``.
    ``current_value`` is the stored override (``None`` means "auto", the
    default); any other/unknown stored value also falls back to shown as
    ``"auto"`` rather than fabricating an unlisted value."""
    current = current_value if current_value in _LANGUAGE_VALUES else "auto"
    return dict(
        id=LANGUAGE_OPTION_ID, name="Coordinator language" if as_mode else "Language", type="select",
        category="mode" if as_mode else "other",
        currentValue=current,
        description="Language for Cortex's own status/question/stop lines; auto detects from your messages.",
        options=[dict(value=value, name=_LANGUAGE_NAMES[value]) for value in _LANGUAGE_VALUES],
    )


def build_config_options(
    selectable_entries: Sequence[CatalogEntry],
    connections: Mapping[str, ConnectionRecord],
    current: Mapping[str, object] | None = None,
    *, include_language: bool = False, language_current: str | None = None,
    language_as_mode: bool = False,
    environ: Mapping[str, str] | None = None,
    project_root: str | None = None,
    combined_model_picker: bool = False,
) -> list[dict]:
    """The full ordered ``configOptions`` list: provider, model, thought
    level, and optionally language. By default model values are scoped to
    the provider. With ``combined_model_picker`` the model option contains
    exact routes across providers for a client that renders only models.

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
    # Some engines advertise a literal ``default`` model/effort as an
    # implementation alias.  It is not a concrete choice for a person and
    # must not leak into either client's model picker.  Keep the underlying
    # catalog evidence intact; only the public selection surface is filtered.
    selectable_entries = tuple(
        entry for entry in selectable_entries
        if entry.route.model.casefold() != _ENGINE_DEFAULT_VALUE
    )
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

    # Paseo's composer has one native model picker and resolves the account
    # from that model choice. Exposing our independent provider axis here
    # makes Paseo render a second account/provider selector (for example
    # "Codex · user@example.com" and "Claude · user@example.com"), which is
    # an invalid duplicate control for this client. Keep the provider axis
    # for Zed and other ACP clients, while Paseo receives only the combined
    # route model picker plus effort and coordinator language.
    if not selectable_entries:
        if include_language:
            options.append(build_language_option(language_current, as_mode=language_as_mode))
        return options
    if combined_model_picker:
        provider_effective_state = current_state
        chosen_connection_id = (
            current_entry.connection_id
            if current_entry is not None
            else selectable_entries[0].connection_id
        )
    else:
        provider_option = build_provider_option(
            selectable_entries, connections, current_connection_id, current_state, current_revision, environ)
        if provider_option is None:
            if include_language:
                options.append(build_language_option(language_current, as_mode=language_as_mode))
            return options
        options.append(provider_option)
        provider_effective_state = provider_option.get("_meta", {}).get("_cortex", {}).get("state", current_state)
        chosen_connection_id = provider_option["currentValue"]
    provider_entries = [entry for entry in selectable_entries if entry.connection_id == chosen_connection_id]
    current_model = current_entry.route.model if current_entry is not None else None

    if combined_model_picker:
        model_option = build_combined_model_option(
            selectable_entries, connections,
            current_route_id, chosen_connection_id, provider_effective_state,
            current_revision, environ,
        )
    else:
        model_option = build_model_option(
            provider_entries, current_model, provider_effective_state, current_revision,
            connections=connections, environ=environ,
        )
    if model_option is not None:
        options.append(model_option)
        chosen_model = model_option["currentValue"]
        combined_values = combined_model_values(selectable_entries) if combined_model_picker else {}
        chosen_entry = next(
            (entry for entry in selectable_entries
             if combined_values[entry.route.id] == chosen_model), None
        ) if combined_model_picker else next(
            (entry for entry in provider_entries if entry.route.model == chosen_model), None
        )
        # Always the model option's own *effective* state (never the raw
        # current["state"]): when the provider/route is unavailable, the
        # model option's state is already "default_unavailable"/
        # "selection_unavailable" (see build_model_option), and
        # build_thought_level_option must see that same effective state to
        # carry the identical marker instead of presenting a stale saved
        # value.
        model_effective_state = model_option.get("_meta", {}).get("_cortex", {}).get("state", provider_effective_state)
        thought_option = build_thought_level_option(
            chosen_entry, current.get("parameters") or {}, model_effective_state, current_revision,
            allowed_efforts=_allowed_efforts(chosen_entry, selectable_entries, project_root, environ),
        )
        if thought_option is not None:
            options.append(thought_option)
    if include_language:
        options.append(build_language_option(language_current, as_mode=language_as_mode))
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
