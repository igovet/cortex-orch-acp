"""Build truthful ACP ``SessionConfigOption`` entries for one ACP session.

Only two options exist today: ``model`` (category ``model``) and
``thought_level`` (category ``thought_level``). Both are built from the
*actual* connection registry + cached catalog snapshots (``catalog.store``)
and the task's own configuration history (``configuration.store`` /
``ConfigurationService``) -- never a fabricated placeholder value.

- ``model``: values are ``CatalogEntry.route.id`` for every entry currently
  selectable for a new execution (``CatalogSnapshot.selectable_for_new_execution``,
  recomputed here per-connection from the most recently cached snapshot file);
  names are safe ``Engine / Connection / Model`` labels, re-disambiguated
  across the *combined* offered set (not just one connection's own
  snapshot). If there is nothing selectable, the option is omitted entirely
  -- never an empty select, never a fake value.
- ``thought_level``: values are exactly the ``(name, value)`` pairs recorded
  on the *currently selected* route's ``ExecutionRoute.supported_parameters``
  whose name is ``"thought_level"`` (the same dependent-setting name used by
  ``configuration.resolution``/``catalog.discovery``). Omitted when the
  route reports no such parameter.

Neither option is ever emitted for team/permission/spend/summarizer
settings; those are out of scope for this task (T010).
"""
from __future__ import annotations

from typing import Iterable, Mapping, Sequence

from ..catalog import store as catalog_store
from ..catalog.evidence import Fact
from ..catalog.labels import disambiguate_labels
from ..catalog.model import CatalogEntry
from ..connections import registry as connection_registry
from ..connections.model import ConnectionRecord

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


def selectable_entries_from_snapshots(
    connections: Mapping[str, ConnectionRecord], environ: Mapping[str, str] | None = None
) -> tuple[CatalogEntry, ...]:
    """Every catalog entry selectable for a new execution, across every
    connection's most-recently-cached snapshot file.

    A connection with no cached snapshot (never discovered/refreshed) never
    contributes any entry -- it is simply absent, not a fabricated one.
    """
    selectable: list[CatalogEntry] = []
    catalog_dir = connection_registry.state_root(environ) / "catalog"
    for connection_id, record in connections.items():
        directory = catalog_dir / connection_id
        if not directory.is_dir():
            continue
        files = sorted(
            (path for path in directory.glob("cs_*.json") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
        if not files:
            continue
        snapshot_id = files[0].stem
        view = catalog_store.load_connection_snapshot(connection_id, snapshot_id, environ)
        if view is None:
            continue
        for entry in view.entries:
            if entry.is_selectable(record.state, record.auth_mode.value):
                selectable.append(entry)
    return tuple(selectable)


_UNAVAILABLE_DESCRIPTION = "Saved route is unavailable — choose a route"
_UNAVAILABLE_THOUGHT_LEVEL_DESCRIPTION = "Depends on an unavailable route — choose a route"


def build_model_option(
    selectable_entries: Sequence[CatalogEntry],
    current_route_id: str | None,
    current_state: str | None,
    current_revision: int | None,
) -> dict | None:
    """The ``model`` SessionConfigOption, or ``None`` when nothing is selectable.

    When ``current_route_id`` is set (a persisted default, or a task/session
    selection) but its route is no longer among ``selectable_entries`` --
    its connection was removed/deauthorized, or its catalog entry is
    otherwise no longer selectable -- the *first* selectable entry is shown
    as ``currentValue`` only because the schema requires a non-empty value;
    this substitution is never silent: the option's ``description`` says so
    and ``_meta._cortex.state`` becomes ``"default_unavailable"`` (the saved
    route was a pre-task default) or ``"selection_unavailable"`` (it was an
    actual task/session selection), never the original state. Callers (see
    ``acp.agent``) must never auto-propose this substituted value as if it
    were the saved choice.
    """
    if not selectable_entries:
        return None
    pairs = [(entry.route.id, entry.label) for entry in selectable_entries]
    labels = disambiguate_labels(pairs)
    options = [
        dict(value=entry.route.id, name=labels[entry.route.id], description=_evidence_note(entry))
        for entry in selectable_entries
    ]
    value_ids = {entry.route.id for entry in selectable_entries}
    unavailable = current_route_id is not None and current_route_id not in value_ids
    if unavailable:
        current_value = selectable_entries[0].route.id
        effective_state = "default_unavailable" if current_state == "default" else "selection_unavailable"
        description = _UNAVAILABLE_DESCRIPTION
    else:
        current_value = current_route_id if current_route_id is not None else selectable_entries[0].route.id
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
    current: Mapping[str, object] | None = None,
    *, include_language: bool = False, language_current: str | None = None,
) -> list[dict]:
    """The full ordered ``configOptions`` list (model, then thought_level,
    then -- only when ``include_language`` is explicitly requested (see
    ``acp.agent._config_options_for_session``, which always sets it) --
    ``language``).

    ``current`` is ``None`` or ``{"route_id", "state", "revision", "parameters"}``
    describing the most recent live selection (see
    ``_LIVE_SELECTION_STATES``); a task-less session (no selection yet) is
    represented as ``current=None`` and simply shows the first selectable
    entry as the (unstated, non-pending) default.

    ``include_language``/``language_current`` are keyword-only and default
    to omitting the language option entirely -- every pre-T020 direct call
    (and every existing test of this exact function) keeps returning ``[]``
    for "nothing selectable, no language requested", unchanged. Unlike
    model/thought_level, the language option is appended even when
    ``selectable_entries`` is empty (no model is selectable yet): language
    is never gated on route/connection availability.
    """
    current = current or {}
    options: list[dict] = []
    model_option = build_model_option(
        selectable_entries, current.get("route_id"), current.get("state"), current.get("revision"))
    if model_option is not None:
        options.append(model_option)
        chosen_route_id = model_option["currentValue"]
        chosen_entry = next((entry for entry in selectable_entries if entry.route.id == chosen_route_id), None)
        # Always the model option's own *effective* state (never the raw
        # current["state"]): when the saved/selected route is unavailable,
        # the model option's state is already "default_unavailable"/
        # "selection_unavailable" (see build_model_option), and
        # build_thought_level_option must see that same effective state to
        # carry the identical marker instead of presenting a stale saved
        # value.
        model_effective_state = model_option.get("_meta", {}).get("_cortex", {}).get("state", current.get("state"))
        thought_option = build_thought_level_option(
            chosen_entry, current.get("parameters") or {}, model_effective_state, current.get("revision"))
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
