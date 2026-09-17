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
from typing import Any, Mapping

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
