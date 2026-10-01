"""Layer merge, dependent-settings bundle validation, and route re-derivation.

Layers, most general first: user defaults, project defaults, task layer
(the task's own prior revision), assignment override (the call-time input).
Merge is *per field*: the most-specific non-empty layer wins that field
outright; fields are never deep-merged within a single dict value. A route
change always re-derives dependent settings from scratch -- a changed
route's parameters are never inherited from a previous route's revision.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Mapping, Sequence

from ..catalog.model import CatalogEntry
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import ExecutionRoute
from .policy import PolicyCeiling

# Dependent-settings names this module knows how to validate as one bundle
# against a CatalogEntry's observed supported_parameters. Anything else in
# an inputs.parameters mapping is passed through untouched (engine-specific,
# but still validated as part of the same atomic bundle by membership).
_DEPENDENT_SETTING_NAMES = frozenset(
    {"effort", "thought_level", "mode", "model_config", "summarizer"}
)


@dataclass(frozen=True)
class ConfigurationInputs:
    """One caller's proposed configuration change.

    Deliberately has no cost/spend field anywhere: cost is never an input to
    configuration resolution, only ever an output of usage/cost telemetry
    checked against a ceiling's ``spend_limit`` at a later, separate step.
    """

    route: ExecutionRoute
    parameters: Mapping[str, Any] = field(default_factory=dict)
    ceiling_override: PolicyCeiling | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.route, ExecutionRoute):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'route' must be an ExecutionRoute instance.",
                retry_eligible=False,
                field="route",
            )
        object.__setattr__(self, "parameters", dict(self.parameters))


def merge_layers(*layers: Mapping[str, Any] | None) -> dict[str, Any]:
    """Per-field most-specific-wins merge, most general layer first.

    A later (more specific) layer's field, if present, always replaces an
    earlier layer's same-named field wholesale; ``None``/empty layers are
    skipped.
    """
    merged: dict[str, Any] = {}
    for layer in layers:
        if not layer:
            continue
        merged.update(layer)
    return merged


def rederive_on_route_change(
    previous_route_id: str | None, new_route: ExecutionRoute, parameters: Mapping[str, Any]
) -> dict[str, Any]:
    """Drop dependent settings tied to a previous route when the route changes.

    Never carries an old route's effort/thought_level/mode/model_config/
    summarizer forward across a route change; every other (engine-agnostic)
    parameter is preserved.
    """
    if previous_route_id is None or previous_route_id == new_route.id:
        return dict(parameters)
    return {key: value for key, value in parameters.items() if key not in _DEPENDENT_SETTING_NAMES}


@dataclass(frozen=True)
class SavedRouteResolution:
    """The outcome of :func:`resolve_saved_route`.

    Exactly one of ``entry``/``withdrawn_model``/``unresolved`` is
    meaningful: ``entry`` on success, ``withdrawn_model`` when the saved
    (connection, model) identity was recovered but that model is no longer
    offered on that connection (fail closed, naming it), ``unresolved``
    (always ``True`` then) when there was not even enough saved identity to
    look anything up.
    """

    entry: CatalogEntry | None
    withdrawn_model: str | None = None
    unresolved: bool = False


def resolve_saved_route(
    entries: Sequence[CatalogEntry], connection_id: str | None, model: str | None,
) -> SavedRouteResolution:
    """Resolve a saved default by its stable ``(connection_id, model)``
    identity against ``entries`` -- the *current* catalog's selectable
    entries -- instead of by its old fingerprint ``route_id``.

    This is what makes a saved default survive a catalog refresh that
    mints new route ids for an unchanged (connection, model) pair (see
    ``catalog.discovery.route_fingerprint`` -- a route id also folds in
    ``supported_parameters``, so a refresh that only changes *those* for
    the same model still changes the id every time): the caller always
    re-derives ``entries`` from the live catalog first, then this function
    matches on identity alone and returns whatever id that entry's route
    presently has.

    By construction this can never resolve to a different model or a
    different connection than ``connection_id``/``model`` name: every
    candidate is filtered on both fields *before* any entry is returned
    (see ``tests/test_catalog_refresh.py``'s near-miss assertions).
    """
    if not connection_id or not model:
        return SavedRouteResolution(entry=None, unresolved=True)
    matches = [e for e in entries if e.connection_id == connection_id and e.route.model == model]
    if len(matches) > 1:
        raise _portable_error("saved_route_ambiguous", "The saved identity has multiple current routes; select an exact route.")
    if matches:
        return SavedRouteResolution(entry=matches[0])
    return SavedRouteResolution(entry=None, withdrawn_model=model)


def _portable_error(code, message):
    return PortableError(category=PortableErrorCategory.VALIDATION, code=code,
                         remediation=message, retry_eligible=False, field="defaults.coordinator")


def validate_portable_coordinator(value):
    from ..connections.model import SUPPORTED_ENGINES
    if (not isinstance(value, Mapping) or set(value) != {"engine", "model", "parameters"}
            or value["engine"] not in SUPPORTED_ENGINES
            or not isinstance(value["model"], str) or not value["model"].strip()
            or not isinstance(value["parameters"], Mapping)
            or not all(isinstance(k, str) and k and isinstance(v, str) and v
                       for k, v in value["parameters"].items())):
        raise _portable_error("invalid_portable_coordinator", "Use exact engine/model and a parameter string mapping.")
    return dict(value, parameters=dict(value["parameters"]))



def portable_from_defaults(values):
    value = validate_portable_coordinator(values["coordinator"])
    if "parameters" in values:
        value["parameters"] = values["parameters"]
        value = validate_portable_coordinator(value)
    return value


def resolve_portable_coordinator(entries, value):
    """Bind only a unique exact, observed current identity; never mint a route."""
    from ..catalog.evidence import Fact, ProvenanceSource
    value = validate_portable_coordinator(value)
    matches = [entry for entry in entries if entry.route.engine == value["engine"]
               and entry.route.model == value["model"]
               and entry.evidence.get(Fact.CATALOG_OBTAINED) is not None
               and entry.evidence.get(Fact.CATALOG_OBTAINED).source == ProvenanceSource.DISCOVERY]
    if not matches:
        raise _portable_error("portable_coordinator_unavailable", "The configured coordinator is not currently observed/selectable.")
    if len(matches) != 1:
        raise _portable_error("portable_coordinator_ambiguous", "Multiple current connections/routes match; explicitly select one.")
    entry = matches[0]
    supported = set(entry.route.supported_parameters)
    if any((key, val) not in supported for key, val in value["parameters"].items()):
        raise _portable_error("unsupported_dependent_setting", "The configured coordinator parameters are not all observed on this route.")
    return entry


def resolve_owner_coordinator(value, environ=None):
    """Read-only local binding, without login, engine or network discovery.

    Deliberately does NOT apply the ``providers`` restriction: ``providers``
    scopes automatic worker routing, failover and pins (see
    ``routing.auto_route.choose``/``rank`` and the dispatch-time
    ``route_provider_excluded`` check in ``delegation.live_adapters``), not
    manual/portable coordinator selection. A uniquely observed, enabled,
    non-denied coordinator outside ``providers`` must still bind here so
    management diagnostics and picker metadata agree with the first-prompt
    ``resolve_portable_coordinator`` binding for the same identity.
    """
    from ..connections import registry
    from ..catalog import store as catalog_store
    from ..routing import auto_route
    records = registry.inspect_registry(environ).connections
    entries = []
    for record in records.values():
        view = catalog_store.latest_connection_snapshot(record.id, environ)
        if view is not None and record.launch_command is not None:
            entries.extend(entry for entry in view.entries
                           if entry.is_selectable(record.state, record.auth_mode.value))
    rules = auto_route.load_overrides(environ=environ)
    entries = [e for e in entries if auto_route.model_enabled(rules, e.route.engine, e.route.model)
               and not auto_route._denied(rules, e.route.engine, e.route.model)]
    return resolve_portable_coordinator(entries, value)


def validate_dependent_settings(entry: CatalogEntry, parameters: Mapping[str, Any]) -> None:
    """Validate every dependent setting in ``parameters`` as ONE bundle.

    Checked against ``entry.route.supported_parameters`` (the CatalogEntry's
    observed ``(name, value)`` pairs). No partial apply: if any single
    dependent setting is unsupported, the whole bundle is rejected with
    ``unsupported_dependent_setting`` and nothing about the proposal is
    accepted.
    """
    supported: dict[str, set[str]] = {}
    for name, value in entry.route.supported_parameters:
        supported.setdefault(name, set()).add(value)
    rejected = []
    for name, value in parameters.items():
        if name not in _DEPENDENT_SETTING_NAMES:
            continue
        allowed = supported.get(name)
        if allowed is None or str(value) not in allowed:
            rejected.append(name)
    if rejected:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="unsupported_dependent_setting",
            remediation=(
                f"The selected route does not support {sorted(rejected)} with the requested "
                "value(s); the whole configuration bundle is rejected atomically -- correct "
                "every listed field and resubmit the complete bundle."
            ),
            retry_eligible=False,
            field="parameters",
        )
