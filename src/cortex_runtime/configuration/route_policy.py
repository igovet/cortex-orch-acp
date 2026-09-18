"""D-T035-03: per-role model/effort route policy, hand-edited by the user.

The gap this closes: ``routing.authorization``'s route profile/ceiling
already constrain which *route* (engine+connection) a role may dispatch on,
but never which *parameter values* (effort/reasoning level) within that
route -- so "implementation may use codex/luna at high/xhigh/max, and
codex/terra at medium/high/xhigh" could not be expressed. This module is
the one place that reads the user's own policy file and answers "is this
role/engine/model/effort combination allowed, and if no effort was
selected, what is this route's default" -- checked strictly *after* the
existing route/ceiling authorization (see
``delegation.live_adapters.EngineSessionManagerAdapter._authorize_dispatch``,
the one caller), so this can only ever narrow further, never grant a route
the ceiling/profile has already refused.

File shape (JSON)::

    {
      "_comment": "role -> list of {engine, model, efforts, default_effort}. "
                  "model/efforts may be \"*\" (any). A role absent from "
                  "'roles' falls back to 'default'. Absent from both means "
                  "no restriction for that role.",
      "roles": {
        "implementation": [
          {"engine": "codex-acp", "model": "luna", "efforts": ["high", "xhigh", "max"], "default_effort": "high"},
          {"engine": "codex-acp", "model": "terra", "efforts": ["medium", "high", "xhigh"], "default_effort": "medium"},
          {"engine": "claude-agent-acp", "model": "sonnet-5", "efforts": ["medium", "high", "xhigh"], "default_effort": "medium"}
        ]
      },
      "default": [
        {"engine": "codex-acp", "model": "*", "efforts": ["*"], "default_effort": null}
      ]
    }

Location, read in precedence order (first one that exists on disk wins;
never merged across locations): ``<project>/.cortex/route-policy.json``,
the path named by the ``CORTEX_ROUTE_POLICY`` environment variable,
``~/.cortex/route-policy.json``, ``~/.config/cortex/route-policy.json``.
Absent everywhere means "no constraint beyond the permission ceiling" --
never a failure, and never means every role is denied.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

from ..portable.errors import PortableError, PortableErrorCategory

ENV_VAR = "CORTEX_ROUTE_POLICY"
WILDCARD = "*"


@dataclass(frozen=True)
class RoutePolicyEntry:
    engine: str
    model: str = WILDCARD
    efforts: frozenset = frozenset({WILDCARD})
    default_effort: str | None = None

    def matches(self, engine: str, model: str | None) -> bool:
        if self.engine != engine:
            return False
        return self.model == WILDCARD or model is None or self.model == model

    def permits_effort(self, effort: str) -> bool:
        return WILDCARD in self.efforts or effort in self.efforts


@dataclass(frozen=True)
class RoutePolicy:
    """``roles[role]`` is the closed list of entries that role may use;
    ``default`` (``None`` when the file never named one) is used only for a
    role absent from ``roles`` -- mirroring
    ``configuration.policy.RouteProfile``'s own "named role missing from an
    explicit mapping is denied, only the generic default still falls
    through" shape, one level down (parameter values, not routes)."""

    roles: Mapping[str, tuple[RoutePolicyEntry, ...]]
    default: tuple[RoutePolicyEntry, ...] | None

    def resolve_effort(self, role: str, engine: str, model: str | None, requested_effort: str | None) -> str | None:
        """The effort this dispatch is authorized to actually use.

        Raises ``PortableError(code="route_policy_not_authorized")`` --
        never silently substitutes a different engine/model -- when
        ``role`` names entries (its own, or the ``default`` bucket) but
        none matches ``engine``/``model``, or a ``requested_effort`` was
        given and no matching entry permits it. When ``requested_effort``
        is ``None`` (nothing else selected one) and a matching entry names
        a ``default_effort``, that default is returned instead.
        """
        entries = self.roles.get(role)
        if entries is None:
            entries = self.default
        if entries is None:
            return requested_effort
        candidates = [entry for entry in entries if entry.matches(engine, model)]
        if not candidates:
            raise _refuse(role, engine, model, requested_effort, entries)
        if requested_effort is None:
            for entry in candidates:
                if entry.default_effort:
                    return entry.default_effort
            return None
        for entry in candidates:
            if entry.permits_effort(requested_effort):
                return requested_effort
        raise _refuse(role, engine, model, requested_effort, candidates)


def _refuse(role: str, engine: str, model: str | None, requested_effort: str | None, entries) -> PortableError:
    allowed = sorted(
        f"{entry.engine}/{entry.model}:{{{','.join(sorted(entry.efforts))}}}" for entry in entries
    ) or ["nothing"]
    return PortableError(
        category=PortableErrorCategory.AUTHORIZATION,
        code="route_policy_not_authorized",
        remediation=(
            f"role '{role}' is not authorized by this project's route policy for "
            f"engine {engine!r} model {model!r} effort {requested_effort!r}; "
            f"this role's policy allows: {allowed}."
        ),
        retry_eligible=False,
        field="effort",
    )


def _candidate_paths(project_root: str | os.PathLike | None, environ: Mapping[str, str] | None) -> list[Path]:
    environ = environ if environ is not None else os.environ
    # Mirrors ``connections.registry._effective_home``: an explicit
    # environ's own "HOME" always wins (what makes this hermetically
    # testable), falling back to the real interpreter home only when the
    # caller never overrides it -- never a second, independent notion of
    # "home" from this module.
    home = Path(environ.get("HOME") or str(Path.home()))
    paths = []
    if project_root:
        paths.append(Path(project_root) / ".cortex" / "route-policy.json")
    env_path = environ.get(ENV_VAR)
    if env_path:
        paths.append(Path(env_path))
    paths.append(home / ".cortex" / "route-policy.json")
    paths.append(home / ".config" / "cortex" / "route-policy.json")
    return paths


def load_route_policy(
    project_root: str | os.PathLike | None = None, environ: Mapping[str, str] | None = None,
) -> tuple[RoutePolicy | None, dict | None]:
    """``(policy, malformed_detail)`` -- exactly one is non-``None``, except
    both are ``None`` when no policy file exists anywhere in precedence
    order (no constraint). The first existing path in precedence order
    always wins outright; a malformed file at that path is never skipped
    in favor of a lower-precedence one -- the caller must fix (or remove)
    the file the precedence order actually selected.
    """
    for path in _candidate_paths(project_root, environ):
        try:
            if not path.is_file():
                continue
        except OSError:
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            return _parse_policy(data), None
        except Exception as error:  # noqa: BLE001 - any parse/shape failure is "malformed", recorded verbatim
            return None, dict(path=str(path), reason=str(error), error_type=type(error).__name__)
    return None, None


def _parse_policy(data) -> RoutePolicy:
    if not isinstance(data, Mapping):
        raise ValueError("route policy file must contain a JSON object")
    roles_raw = data.get("roles") or {}
    if not isinstance(roles_raw, Mapping):
        raise ValueError("'roles' must be an object mapping role id/alias to a list of entries")
    roles = {str(role): _parse_entries(role, entries) for role, entries in roles_raw.items()}
    default_raw = data.get("default")
    default = _parse_entries("default", default_raw) if default_raw is not None else None
    return RoutePolicy(roles=roles, default=default)


def _parse_entries(role, entries) -> tuple[RoutePolicyEntry, ...]:
    if not isinstance(entries, Sequence) or isinstance(entries, (str, bytes)):
        raise ValueError(f"role {role!r} entries must be a list")
    parsed = []
    for entry in entries:
        if not isinstance(entry, Mapping):
            raise ValueError(f"role {role!r} has a non-object entry")
        engine = entry.get("engine")
        if not isinstance(engine, str) or not engine:
            raise ValueError(f"role {role!r} entry is missing a string 'engine'")
        model = entry.get("model", WILDCARD)
        if not isinstance(model, str) or not model:
            raise ValueError(f"role {role!r} entry has an invalid 'model'")
        efforts_raw = entry.get("efforts", [WILDCARD])
        if not isinstance(efforts_raw, Sequence) or isinstance(efforts_raw, (str, bytes)) or not efforts_raw:
            raise ValueError(f"role {role!r} entry 'efforts' must be a non-empty list")
        efforts = frozenset(str(value) for value in efforts_raw)
        default_effort = entry.get("default_effort")
        if default_effort is not None and not isinstance(default_effort, str):
            raise ValueError(f"role {role!r} entry 'default_effort' must be a string or null")
        parsed.append(RoutePolicyEntry(engine=engine, model=model, efforts=efforts, default_effort=default_effort))
    return tuple(parsed)
