"""Owner configuration management: one safe, versioned API for every
currently supported Cortex configuration surface.

A settings UI (for example a Paseo plugin page) edits Cortex configuration
through this module instead of hand-editing JSON. It is deliberately not a
generic JSON or file editor: every surface has a closed set of editable
settings, is validated by the same loaders the runtime itself uses, and is
written only through the storage path that surface already has.

Surfaces (:data:`SURFACES`):

- ``user_defaults`` -- ``$XDG_CONFIG_HOME/cortex/defaults.json``: the saved
  route default (``route_id``/``route_connection_id``/``route_model``), its
  ``parameters``, the ``language`` default and the user
  :class:`~.policy.PolicyCeiling`. Same encoding as
  ``configuration.store.write_user_defaults``; written under the connection
  registry lock that the ACP agent's own default writes hold.
- ``project_ceiling`` -- the project's ``project_configuration_defaults`` row
  in ``<project>/.cortex/cortex.sqlite3``. It may only narrow the user ceiling
  (``policy.require_narrowing``, as ``ConfigurationService.propose``
  enforces); the row's project defaults are preserved.
- ``routing`` -- the precedence of ``routing.auto_route.load_overrides``
  (project ``.cortex/routing.json``, then ``$CORTEX_ROUTING``, then the user
  ``routing.json``). Reads report every source, the effective one and its
  rules; writes replace the eight supported keys of the user or project
  file, validated by ``auto_route._validate``. ``$CORTEX_ROUTING`` is
  read-only. ``models`` holds the owner's per-engine automatic-routing
  selection, model tiers and model policy (``routing.model_policy``).
- ``routing_profiles`` -- owner profiles (``routing-profiles.json``, format
  2): snapshots of every user-level setting -- user ``routing.json``, the
  ``defaults.json`` route default/parameters/language/owner ceiling, each
  MCP server's usage policy (never its ``command``/``args``/``env``) and
  each connection's session limit. Saving into a profile is explicit
  (``save``, ``create``); activation materializes every section into its
  backing file under the lock, journaled and rolled back on failure. It
  adds no runtime source and never holds a project setting.
- ``mcp_servers`` -- ``$XDG_CONFIG_HOME/cortex/mcp-servers.json``. Every
  result redacts ``env`` values; a write may keep an existing value without
  ever reading it back. The whole written registry must pass
  ``mcp_servers._normalize_entries``.
- ``connections`` -- ``connections.json`` metadata (never credential values
  or credential paths): rename, session limit, validate, remove, and the
  explicit opt-in onboarding of the optional OpenCode v2 engine. OpenCode is
  never onboarded, and never let into automatic routing, as a side effect.
- ``route_catalog`` (read-only) -- every supported engine and every catalog
  route Cortex knows of, with its effective state under the effective
  routing rules and the reason: disabled (a ``deny`` token, ``<engine>/*``
  for a whole engine), shown in the model selector, and eligible for
  automatic routing or not (``providers``, a ``models`` selection list that
  leaves it out, the built-in Haiku/OpenCode exclusions lifted by
  ``auto_allow``, superseded versions, unobserved seed catalogs, connection
  state) -- plus its tier and where it comes from, its effective policy,
  and which ``deny``/``auto_allow`` tokens of each routing file cover it.
  Reading never refreshes a catalog or revalidates a connection, so it writes
  nothing; the separate ``refresh`` action re-reads every connection's model
  list from its engine without a prompt (``catalog.refresh``) and adds a new
  catalog snapshot. Each route reports ``enabled`` (routing ``enabled`` or
  its built-in default: OpenCode models stay off until listed). Enabling or
  disabling an engine or model is a ``routing`` update.

Versioning: every read returns a ``revision`` -- ``"absent"`` or
``"sha256:<hex>"`` of the authoritative bytes (for the project row, of its
stored columns). Every mutation requires ``expected_revision`` and is refused
with ``revision_conflict`` when the current revision differs, so a concurrent
external edit is reported, never overwritten. The comparison runs under the
registry lock (for the project row, inside the same SQLite ``BEGIN
IMMEDIATE``) immediately before an atomic replace. A writer that takes no lock
at all (a hand edit, ``routing.defaults.ensure_default_routing``) can still
race that last instant; ``remove``/``validate``/OpenCode onboarding check the
revision under the lock and then run the existing ``connections.onboarding``
operation in its own registry transaction, which applies to the registry as
it then is and never overwrites another change.

Errors are :class:`~..portable.errors.PortableError`;
:class:`ConfigValidationError` lists every invalid setting in ``errors``.
:func:`execute` is the JSON request/response form of the API, and
``python3 -B -m cortex_runtime.configuration.management`` answers one JSON
request on stdin.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import replace
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
from typing import Iterator, Mapping

from ..connections import registry
from ..connections.model import (
    MAX_CONCURRENT_SESSIONS_LIMIT,
    OPTIONAL_ENGINES,
    SUPPORT_MATRIX,
    SUPPORTED_ENGINES,
    EngineNativeDir,
    OsKeyring,
    SecretFile,
)
from ..portable.errors import PortableError, PortableErrorCategory
from ..routing import auto_route, model_policy
from . import mcp_servers as mcp_registry
from . import store as configuration_store
from .policy import FALLBACK_HARD_DENYLIST, FallbackPolicy, PermissionLevel, PolicyCeiling, require_narrowing

API_VERSION = 1
SURFACES = ("user_defaults", "project_ceiling", "routing", "routing_profiles", "mcp_servers", "connections")
ROUTE_CATALOG = "route_catalog"
READ_ONLY_SURFACES = (ROUTE_CATALOG,)
ABSENT = "absent"
OPENCODE = "opencode"

USER_DEFAULT_KEYS = ("route_id", "route_connection_id", "route_model", "parameters", "language")
ROUTE_DEFAULT_KEYS = ("route_id", "route_connection_id", "route_model")
LANGUAGES = ("en", "ru")
CEILING_KEYS = (
    "allowed_routes", "allowed_connections", "allowed_data_recipients", "permission_level",
    "limits", "spend_limit", "spend_limit_enforceable", "fallback_policy",
)
FALLBACK_KEYS = ("allowed_target_routes", "allowed_failure_classes", "budget", "data_recipient_consent")
LIMIT_KEYS = ("attempts", "calls", "tokens", "concurrency")
ROUTING_KEYS = ("pin", "deny", "enabled", "auto_allow", "tiers", "providers", "engine_affinity", "models")
ROUTING_TARGETS = ("project", "user")
MCP_SERVER_KEYS = (
    "command", "args", "env", "enabled", "allowed_tools", "read_only_tools", "coordinator_tools",
    "always_load_tools", "read_paths", "write_paths", "network", "runtime_dir_env", "roles", "engines",
)
CONNECTION_OPERATIONS = ("rename", "set_max_concurrent_sessions", "validate", "remove")
#: ``routing_profiles`` mutations and their request fields (besides
#: ``api_version``/``action``/``surface``).
PROFILE_ACTION_FIELDS = {
    "create": ("expected_revision", "expected_revisions", "name", "description", "project"),
    "save": ("expected_revision", "expected_revisions", "name", "project"),
    "update": ("expected_revision", "name", "rename_to", "description", "project"),
    "delete": ("expected_revision", "name", "project"),
    "activate": ("expected_revision", "expected_revisions", "name", "discard_modifications", "project"),
    "deactivate": ("expected_revision", "project"),
}

_KEEP = object()
_REVISION = re.compile(r"sha256:[0-9a-f]{64}")
_ROUTE_ID = re.compile(r"xr_[0-9a-f]{12,64}")
_CONNECTION_ID = re.compile(r"cn_[0-9a-f]{12}")
_PARAMETER_NAME = re.compile(r"[a-z][a-z0-9_]{0,63}")
_CONTROL = re.compile(r"[\x00-\x1f\x7f]")
_TOKEN = re.compile(r"[^\s\x00-\x1f\x7f]{1,256}")
_ROLE_KEY = re.compile(r"\*|[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
_SERVER_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
_ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,127}")


# --- errors --------------------------------------------------------------

def _error(code: str, message: str, *, field: str | None = None,
           category: PortableErrorCategory = PortableErrorCategory.VALIDATION) -> PortableError:
    return PortableError(category=category, code=code, remediation=message, retry_eligible=False, field=field)


class ConfigValidationError(PortableError):
    """Every invalid setting of one request, as ``errors`` of
    ``{"field", "code", "message"}``; nothing was written."""

    def __init__(self, errors) -> None:
        items = tuple(dict(field=item["field"], code=item["code"], message=item["message"]) for item in errors)
        shown = "; ".join(f"{item['field']}: {item['message']}" for item in items[:5])
        more = f" (and {len(items) - 5} more)" if len(items) > 5 else ""
        super().__init__(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_configuration",
            remediation=f"Correct the invalid setting(s); nothing was written. {shown}{more}",
            retry_eligible=False,
            field=items[0]["field"] if items else None,
        )
        self.errors = items


class RevisionConflict(PortableError):
    """The surface changed after it was read; nothing was written."""

    def __init__(self, surface: str, expected: str, current: str) -> None:
        super().__init__(
            category=PortableErrorCategory.VALIDATION,
            code="revision_conflict",
            remediation=(
                f"The {surface} configuration changed after it was read (expected revision {expected}, "
                f"current {current}); nothing was written. Re-read it and apply the change again."
            ),
            retry_eligible=False,
            field="expected_revision",
        )
        self.surface = surface
        self.current_revision = current


class _Errors:
    def __init__(self) -> None:
        self.items: list[dict[str, str]] = []

    def add(self, field: str, code: str, message: str) -> None:
        self.items.append(dict(field=field, code=code, message=message))

    def absorb(self, error: ConfigValidationError) -> None:
        self.items.extend(dict(item) for item in error.errors)

    def raise_if_any(self) -> None:
        if self.items:
            raise ConfigValidationError(self.items)


def error_payload(error: PortableError) -> dict[str, object]:
    """The JSON form of a refusal; never carries a configuration value."""
    payload: dict[str, object] = dict(
        category=error.category.value, code=error.code, message=error.remediation,
        field=error.field, retry_eligible=error.retry_eligible,
    )
    if isinstance(error, ConfigValidationError):
        payload["errors"] = [dict(item) for item in error.errors]
    if isinstance(error, RevisionConflict):
        payload["current_revision"] = error.current_revision
    return payload


# --- shared helpers --------------------------------------------------------

def _env(environ: Mapping[str, str] | None) -> dict[str, str] | None:
    return None if environ is None else dict(environ)


def _revision(data: bytes | None) -> str:
    return ABSENT if data is None else "sha256:" + hashlib.sha256(data).hexdigest()


def _expected(value) -> str:
    if not isinstance(value, str) or not (value == ABSENT or _REVISION.fullmatch(value)):
        raise _error(
            "invalid_expected_revision",
            "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>').",
            field="expected_revision",
        )
    return value


def _check(surface: str, expected: str, current: str) -> None:
    if expected != current:
        raise RevisionConflict(surface, expected, current)


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _clean_text(value, *, limit: int = 256) -> bool:
    return isinstance(value, str) and bool(value.strip()) and len(value) <= limit and not _CONTROL.search(value)


def _unique_strings(value) -> bool:
    return (isinstance(value, list) and all(_clean_text(item) for item in value)
            and len(set(value)) == len(value))


def _non_negative_int(value) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value >= 0


def _json_object(data: bytes, what: str, code: str) -> dict:
    try:
        value = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise _error(code, f"{what} is not valid UTF-8 JSON; repair it by hand. Nothing was changed.") from None
    if not isinstance(value, dict):
        raise _error(code, f"{what} must contain a JSON object; repair it by hand. Nothing was changed.")
    return value


@contextmanager
def _owner_config_lock(environ: dict[str, str] | None) -> Iterator[None]:
    """The connection registry's cross-process lock (``connections.lock``),
    without reading or rewriting ``connections.json`` -- the same lock
    ``registry.transaction`` and the ACP agent's user-default writes take."""
    root = registry.config_root(environ)
    registry.ensure_private_dir(root, root=root, environ=environ)
    lock_path = registry.registry_lock_path(environ)
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    try:
        descriptor = os.open(lock_path, flags, 0o600)
    except OSError:
        raise _error("unsafe_storage", f"{lock_path} is an unsafe link or cannot be opened safely.") from None
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise _error("unsafe_storage", "connections.lock is an unsafe link/hardlink/mode file.")
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def _read_plain(path: Path) -> bytes | None:
    """``path``'s bytes, or ``None`` when absent. Never follows a symlink."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    try:
        descriptor = os.open(path, flags)
    except FileNotFoundError:
        return None
    except OSError:
        raise _error("unsafe_storage", f"{path} is a symlink or cannot be opened safely; Cortex configuration "
                                       "files must be real files.") from None
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise _error("unsafe_storage", f"{path} is not a regular file.")
        with os.fdopen(descriptor, "rb") as handle:
            descriptor = -1
            return handle.read()
    finally:
        if descriptor != -1:
            os.close(descriptor)


def _replace_file(path: Path, data: bytes) -> None:
    """Atomically replace ``path`` (owner-only result). An existing target
    must be a single-link regular file the owner owns."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        info = None
    if info is not None and (stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode)
                             or info.st_nlink != 1 or info.st_uid != os.getuid()):
        raise _error("unsafe_storage", f"{path} is a link, hardlink or foreign file; refusing to replace it.")
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def _pretty(document: Mapping) -> bytes:
    return (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


# --- policy ceilings -------------------------------------------------------

def ceiling_to_dict(ceiling: PolicyCeiling) -> dict[str, object]:
    """The exact JSON shape ``defaults.json`` and the project row store."""
    return json.loads(configuration_store.serialize_ceiling(ceiling))


def parse_ceiling(value, *, field: str = "ceiling") -> PolicyCeiling:
    """A :class:`PolicyCeiling` from its JSON shape, or every problem at once."""
    errors = _Errors()
    if not isinstance(value, Mapping):
        errors.add(field, "invalid_type", "must be an object")
        errors.raise_if_any()
    for key in sorted(set(value) - set(CEILING_KEYS)):
        errors.add(f"{field}.{key}", "unsupported_setting", f"is not a ceiling setting; use {', '.join(CEILING_KEYS)}")

    def string_set(raw, name):
        if raw is None:
            return None
        if not _unique_strings(raw):
            errors.add(name, "invalid_type", "must be null (unrestricted) or an array of unique non-empty strings")
            return None
        return frozenset(raw)

    routes = string_set(value.get("allowed_routes"), f"{field}.allowed_routes")
    connections = string_set(value.get("allowed_connections"), f"{field}.allowed_connections")
    recipients = string_set(value.get("allowed_data_recipients"), f"{field}.allowed_data_recipients")
    level = None
    permission = value.get("permission_level")
    if permission is not None:
        if isinstance(permission, str) and permission in PermissionLevel.__members__:
            level = PermissionLevel[permission]
        else:
            errors.add(f"{field}.permission_level", "invalid_value",
                       f"must be null or one of {', '.join(PermissionLevel.__members__)}")
    limits: dict[str, int] = {}
    raw_limits = value.get("limits", {})
    if not isinstance(raw_limits, Mapping):
        errors.add(f"{field}.limits", "invalid_type", "must be an object of named non-negative integers")
    else:
        for key, amount in raw_limits.items():
            if key not in LIMIT_KEYS:
                errors.add(f"{field}.limits.{key}", "unsupported_setting", f"use one of {', '.join(LIMIT_KEYS)}")
            elif not _non_negative_int(amount):
                errors.add(f"{field}.limits.{key}", "invalid_value", "must be a non-negative integer")
            else:
                limits[key] = amount
    spend_limit = value.get("spend_limit")
    if spend_limit is not None and not _non_negative_int(spend_limit):
        errors.add(f"{field}.spend_limit", "invalid_value", "must be null or a non-negative integer")
    enforceable = value.get("spend_limit_enforceable", False)
    if type(enforceable) is not bool:
        errors.add(f"{field}.spend_limit_enforceable", "invalid_type", "must be true or false")
    elif spend_limit is not None and not enforceable:
        errors.add(f"{field}.spend_limit", "unenforceable_spend_limit",
                   "may only be set with spend_limit_enforceable: true (unknown cost is never treated as zero)")
    fallback = FallbackPolicy()
    raw_fallback = value.get("fallback_policy", {})
    if not isinstance(raw_fallback, Mapping):
        errors.add(f"{field}.fallback_policy", "invalid_type", "must be an object")
    else:
        for key in sorted(set(raw_fallback) - set(FALLBACK_KEYS)):
            errors.add(f"{field}.fallback_policy.{key}", "unsupported_setting",
                       f"use one of {', '.join(FALLBACK_KEYS)}")
        targets = string_set(raw_fallback.get("allowed_target_routes"), f"{field}.fallback_policy.allowed_target_routes")
        classes = string_set(raw_fallback.get("allowed_failure_classes"), f"{field}.fallback_policy.allowed_failure_classes")
        if classes is not None and classes & FALLBACK_HARD_DENYLIST:
            errors.add(f"{field}.fallback_policy.allowed_failure_classes", "fallback_denylisted_failure_class",
                       f"may never name {', '.join(sorted(classes & FALLBACK_HARD_DENYLIST))}")
            classes = None
        budget = raw_fallback.get("budget")
        if budget is not None and not _non_negative_int(budget):
            errors.add(f"{field}.fallback_policy.budget", "invalid_value", "must be null or a non-negative integer")
            budget = None
        consent = raw_fallback.get("data_recipient_consent", False)
        if type(consent) is not bool:
            errors.add(f"{field}.fallback_policy.data_recipient_consent", "invalid_type", "must be true or false")
            consent = False
        if not errors.items:
            fallback = FallbackPolicy(allowed_target_routes=targets, allowed_failure_classes=classes,
                                      budget=budget, data_recipient_consent=consent)
    errors.raise_if_any()
    try:
        return PolicyCeiling(
            allowed_routes=routes, allowed_connections=connections, allowed_data_recipients=recipients,
            permission_level=level, limits=limits, spend_limit=spend_limit,
            spend_limit_enforceable=enforceable, fallback_policy=fallback,
        )
    except PortableError as error:
        raise ConfigValidationError([dict(field=f"{field}.{error.field}" if error.field else field,
                                          code=error.code, message=error.remediation)]) from None


def _narrowing_error(error: PortableError, field: str = "ceiling") -> ConfigValidationError:
    return ConfigValidationError([dict(field=f"{field}.{error.field}" if error.field else field,
                                       code=error.code, message=error.remediation)])


# --- user defaults ---------------------------------------------------------

def _user_defaults_path(environ) -> Path:
    return registry.config_root(environ) / "defaults.json"


def _parse_user_defaults(data: bytes | None) -> tuple[dict, PolicyCeiling, dict]:
    """``(defaults, ceiling, whole document)`` exactly as
    ``configuration.store.read_user_defaults`` reads them, plus the check
    that ``defaults`` is an object."""
    if data is None:
        return {}, PolicyCeiling(), {}
    document = _json_object(data, "defaults.json", "invalid_user_defaults")
    defaults = document.get("defaults", {})
    if not isinstance(defaults, dict):
        raise _error("invalid_user_defaults", "defaults.json 'defaults' must be an object; repair it by hand.")
    ceiling = (configuration_store.deserialize_ceiling(json.dumps(document["ceiling"]))
               if "ceiling" in document else PolicyCeiling())
    return defaults, ceiling, document


def _user_defaults_view(path: Path, data: bytes | None) -> dict[str, object]:
    defaults, ceiling, document = _parse_user_defaults(data)
    return dict(
        surface="user_defaults", path=str(path), revision=_revision(data), exists=data is not None,
        defaults=defaults, editable_keys=list(USER_DEFAULT_KEYS),
        preserved_keys=sorted(key for key in defaults if key not in USER_DEFAULT_KEYS),
        languages=list(LANGUAGES), ceiling=ceiling_to_dict(ceiling), ceiling_set="ceiling" in document,
    )


def read_user_defaults(environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """``defaults.json`` as the runtime reads it; creates nothing."""
    env = _env(environ)
    path = _user_defaults_path(env)
    return _user_defaults_view(path, registry.read_owner_file(path))


def _validate_default_patch(patch, errors: _Errors) -> dict:
    if patch is None:
        return {}
    if not isinstance(patch, Mapping):
        errors.add("defaults", "invalid_type", "must be an object of setting -> value (null removes it)")
        return {}
    for key, value in patch.items():
        field = f"defaults.{key}"
        if key not in USER_DEFAULT_KEYS:
            errors.add(field, "unsupported_setting", f"is not an editable default; use {', '.join(USER_DEFAULT_KEYS)}")
        elif value is None:
            continue
        elif key == "route_id" and not (isinstance(value, str) and _ROUTE_ID.fullmatch(value)):
            errors.add(field, "invalid_value", "must be an observed route id (xr_<hex>)")
        elif key == "route_connection_id" and not (isinstance(value, str) and _CONNECTION_ID.fullmatch(value)):
            errors.add(field, "invalid_value", "must be a connection id (cn_<12 hex>)")
        elif key == "route_model" and not _clean_text(value):
            errors.add(field, "invalid_value", "must be the route's exact model id")
        elif key == "language" and value not in LANGUAGES:
            errors.add(field, "invalid_value", f"must be null (auto) or one of {', '.join(LANGUAGES)}")
        elif key == "parameters":
            if not isinstance(value, Mapping):
                errors.add(field, "invalid_type", "must be an object of parameter name -> value")
                continue
            for name, item in value.items():
                if not (isinstance(name, str) and _PARAMETER_NAME.fullmatch(name)) or not _clean_text(item, limit=128):
                    errors.add(f"{field}.{name}", "invalid_value",
                               "parameter names are lowercase identifiers and values non-empty strings")
    return dict(patch)


def update_user_defaults(expected_revision, *, defaults=None, ceiling=_KEEP,
                         environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Patch ``defaults.json``: ``defaults`` maps an editable key to its new
    value (``None`` removes it); ``ceiling`` replaces the user ceiling (a
    ceiling object) or clears it (``None``). Other keys are preserved.

    A route default is set or cleared as the whole
    ``route_id``/``route_connection_id``/``route_model`` group and must name
    a registered connection; whether the route is still offered is checked
    again when a session uses it (``configuration.service``).
    """
    expected = _expected(expected_revision)
    env = _env(environ)
    errors = _Errors()
    patch = _validate_default_patch(defaults, errors)
    new_ceiling = None
    if ceiling is not _KEEP and ceiling is not None:
        try:
            new_ceiling = parse_ceiling(ceiling)
        except ConfigValidationError as error:
            errors.absorb(error)
    errors.raise_if_any()
    path = _user_defaults_path(env)
    with _owner_config_lock(env):
        data = registry.read_owner_file(path)
        _check("user_defaults", expected, _revision(data))
        current, current_ceiling, document = _parse_user_defaults(data)
        merged = dict(current)
        for key, value in patch.items():
            if value is None:
                merged.pop(key, None)
            else:
                merged[key] = value
        if any(key in patch for key in ROUTE_DEFAULT_KEYS):
            present = [key for key in ROUTE_DEFAULT_KEYS if merged.get(key) is not None]
            if present and len(present) != len(ROUTE_DEFAULT_KEYS):
                errors.add("defaults", "route_incomplete",
                           "route_id, route_connection_id and route_model are set or removed together")
            elif present and merged["route_connection_id"] not in registry.inspect_registry(env).connections:
                errors.add("defaults.route_connection_id", "unknown_connection", "names no registered connection")
            errors.raise_if_any()
        target = current_ceiling if ceiling is _KEEP else (new_ceiling if new_ceiling is not None else PolicyCeiling())
        payload = {key: value for key, value in document.items() if key not in ("defaults", "ceiling")}
        payload.update(defaults=merged, ceiling=ceiling_to_dict(target))
        body = configuration_store._encode(payload).encode("utf-8")
        registry.ensure_private_dir(path.parent, root=registry.config_root(env), environ=env)
        registry.atomic_write_private(path, body, directory=path.parent)
    return dict(_user_defaults_view(path, body), changed=body != data)


# --- project ceiling --------------------------------------------------------

def _project_row_tx(db, project_root: str):
    return db.execute("SELECT * FROM project_configuration_defaults WHERE project_root=?", (project_root,)).fetchone()


def _project_revision(row) -> str:
    if row is None:
        return ABSENT
    material = json.dumps([row["defaults"], row["ceiling"], row["updated_at"]], ensure_ascii=False,
                          separators=(",", ":")).encode("utf-8")
    return _revision(material)


def _project_view(row, project_root: str, environ) -> dict[str, object]:
    user_ceiling = configuration_store.inspect_user_defaults(environ)["ceiling"]
    view: dict[str, object] = dict(
        surface="project_ceiling", project_root=project_root, revision=_project_revision(row),
        exists=row is not None, user_ceiling=ceiling_to_dict(user_ceiling),
    )
    if row is None:
        view.update(ceiling=None, project_defaults={}, effective_ceiling=ceiling_to_dict(user_ceiling),
                    narrows_user_ceiling=True, violation=None, updated_at=None)
        return view
    ceiling = configuration_store.deserialize_ceiling(row["ceiling"])
    violation = None
    try:
        require_narrowing(ceiling, user_ceiling)
    except PortableError as error:
        # The user ceiling tightened after this row was written; the service
        # refuses such a revision at apply time (ceiling_tightened).
        violation = dict(code=error.code, field=error.field, message=error.remediation)
    view.update(
        ceiling=ceiling_to_dict(ceiling), project_defaults=json.loads(row["defaults"]),
        effective_ceiling=ceiling_to_dict(user_ceiling.intersect(ceiling)),
        narrows_user_ceiling=violation is None, violation=violation, updated_at=row["updated_at"],
    )
    return view


def read_project_ceiling(store, project_root: str, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """The project's ceiling, the user ceiling and their intersection."""
    with store.connection() as db:
        row = _project_row_tx(db, project_root)
    return _project_view(row, project_root, _env(environ))


def update_project_ceiling(store, project_root: str, expected_revision, ceiling,
                           environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Replace the project ceiling (``ceiling`` object) or remove it
    (``None``). It must narrow the current user ceiling; the project's own
    defaults are preserved, so removing is refused while they exist."""
    expected = _expected(expected_revision)
    if not isinstance(project_root, str) or not project_root.startswith("/"):
        raise _error("invalid_project", "project_root must be the absolute project root.", field="project_root")
    env = _env(environ)
    candidate = None if ceiling is None else parse_ceiling(ceiling)
    if candidate is not None:
        user_ceiling = configuration_store.inspect_user_defaults(env)["ceiling"]
        try:
            require_narrowing(candidate, user_ceiling)
        except PortableError as error:
            raise _narrowing_error(error) from None
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = _project_row_tx(db, project_root)
            _check("project_ceiling", expected, _project_revision(row))
            if candidate is None:
                if row is not None:
                    if json.loads(row["defaults"]):
                        raise _error("project_defaults_present",
                                     "This project row also holds project defaults; replace the ceiling instead of "
                                     "removing it.", field="ceiling")
                    db.execute("DELETE FROM project_configuration_defaults WHERE project_root=?", (project_root,))
            else:
                db.execute(
                    "INSERT INTO project_configuration_defaults VALUES (?,?,?,?) "
                    "ON CONFLICT(project_root) DO UPDATE SET ceiling=excluded.ceiling,updated_at=excluded.updated_at",
                    (project_root, row["defaults"] if row is not None else configuration_store._encode({}),
                     configuration_store.serialize_ceiling(candidate), configuration_store._now()),
                )
            written = _project_row_tx(db, project_root)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(_project_view(written, project_root, env), changed=True)


# --- routing ------------------------------------------------------------------

def _routing_sources(project_root, environ) -> list[tuple[str, Path]]:
    """``auto_route.load_overrides``'s candidate files, labelled, in its order."""
    env = os.environ if environ is None else environ
    labels = (["project"] if project_root else []) + (["env"] if env.get(auto_route.ENV_VAR) else []) + ["user"]
    paths = auto_route._candidate_paths(project_root, environ)
    if len(paths) != len(labels):
        raise _error("routing_precedence_changed", "The routing loader's file precedence is not the one this API knows.")
    return list(zip(labels, paths))


def _routing_file_view(source: str, path: Path) -> dict[str, object]:
    view: dict[str, object] = dict(source=source, path=str(path), writable=source in ROUTING_TARGETS)
    try:
        data = _read_plain(path)
    except PortableError as error:
        view.update(exists=True, revision=None, valid=False, error=error_payload(error))
        return view
    view.update(exists=data is not None, revision=_revision(data))
    if data is None:
        return view
    try:
        document = json.loads(data.decode("utf-8"))
        auto_route._validate(document)
    except (UnicodeDecodeError, ValueError) as error:
        view.update(valid=False, error=dict(code="invalid_routing_overrides", message=str(error)))
        return view
    view.update(valid=True, rules={key: document[key] for key in ROUTING_KEYS if key in document},
                ignored_keys=sorted(key for key in document if key not in ROUTING_KEYS))
    return view


def _routing_builtin() -> dict[str, object]:
    from ..routing.defaults import DEFAULT_DENY

    return dict(
        supported_engines=list(SUPPORTED_ENGINES),
        optional_engines=list(OPTIONAL_ENGINES),
        default_auto_deny_engines=sorted(getattr(auto_route, "DEFAULT_AUTO_DENY_ENGINES", ())),
        default_auto_deny_families=sorted(auto_route.DEFAULT_AUTO_DENY_FAMILIES),
        tiers=list(auto_route.TIERS), efforts=list(auto_route.EFFORT_ORDER),
        work_kinds=list(model_policy.WORK_KINDS), work_kind_labels=dict(model_policy.WORK_KIND_LABELS),
        work_kind_roles={kind: sorted(role for role, role_kind in auto_route.ROLE_WORK_KINDS.items()
                                      if role_kind == kind) for kind in model_policy.WORK_KINDS},
        builtin_affinity=dict(auto_route.WORK_KIND_ENGINES), default_user_deny=list(DEFAULT_DENY),
        selection_modes=list(model_policy.SELECTION_MODES), tier_sources=list(model_policy.TIER_SOURCES),
        policy_attributes=list(model_policy.POLICY_ATTRIBUTES), policy_efforts=list(model_policy.POLICY_EFFORTS),
        max_request_names=model_policy.MAX_REQUEST_NAMES,
    )


def read_routing(project_root: str | None = None, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Every routing source, the effective one (the first existing file, as
    the loader picks it; files never merge) and its rules."""
    env = _env(environ)
    pairs = _routing_sources(project_root, env)
    views = [_routing_file_view(source, path) for source, path in pairs]
    effective = next((view for (_, path), view in zip(pairs, views) if path.is_file()), None)
    return dict(
        surface="routing", project_root=project_root, precedence=[view["source"] for view in views],
        sources=views, effective_source=effective["source"] if effective is not None else "builtin",
        effective_valid=effective is None or bool(effective.get("valid")),
        effective_rules={} if effective is None else effective.get("rules"),
        builtin=_routing_builtin(),
    )


def _opencode_model(model: str) -> bool:
    provider, slash, name = model.partition("/")
    return bool(slash and provider and name)


def validate_routing_rules(rules) -> dict[str, object]:
    """The eight supported keys, checked strictly (every problem at once) and
    then by the loader itself (``auto_route._validate``).

    Beyond the loader: a pin names a supported engine and an effort from
    ``auto_route.EFFORT_ORDER``; a ``deny``/``auto_allow`` token containing
    ``/`` starts with an engine id; and an OpenCode pin or ``auto_allow``
    names the exact ``opencode/<provider>/<model>`` route -- OpenCode model
    ids contain a ``/``, so a bare id would be read as a foreign engine
    prefix, and a family token would lift OpenCode's automatic-routing
    exclusion for more than the route the owner chose.

    ``models`` (``routing.model_policy.validate_models``) maps a supported
    engine to ``auto`` (its automatic-routing selection), ``tiers`` and
    ``policy``, each keyed by an exact model id; an OpenCode id is the exact
    ``<provider>/<model>``, and a ``request_names`` entry is never an
    ordinary request, work or negation word.
    """
    errors = _Errors()
    if not isinstance(rules, Mapping):
        errors.add("rules", "invalid_type", f"must be an object with keys from {', '.join(ROUTING_KEYS)}")
        errors.raise_if_any()
    engines = tuple(SUPPORTED_ENGINES)
    for key in sorted(set(rules) - set(ROUTING_KEYS)):
        errors.add(f"rules.{key}", "unsupported_setting", f"use one of {', '.join(ROUTING_KEYS)}")
    pins = rules.get("pin")
    if pins is not None:
        if not isinstance(pins, Mapping):
            errors.add("rules.pin", "invalid_type", "must map a role name or * to engine/model[:effort]")
        else:
            for role, value in pins.items():
                field = f"rules.pin.{role}"
                if not isinstance(role, str) or not _ROLE_KEY.fullmatch(role):
                    errors.add(field, "invalid_role", "must be a worker role name or *")
                if not isinstance(value, str) or not _TOKEN.fullmatch(value):
                    errors.add(field, "invalid_pin", "must be engine/model or engine/model:effort")
                    continue
                route, colon, effort = value.partition(":")
                engine, slash, model = route.partition("/")
                if not slash or engine not in engines or not model:
                    errors.add(field, "invalid_pin", f"must be engine/model[:effort] with engine one of {', '.join(engines)}")
                elif engine == OPENCODE and not _opencode_model(model):
                    errors.add(field, "opencode_model_not_exact",
                               "an OpenCode pin names the exact provider/model id, e.g. opencode/anthropic/claude-sonnet-4-5")
                if colon and effort not in auto_route.EFFORT_ORDER:
                    errors.add(field, "invalid_effort", f"effort must be one of {', '.join(auto_route.EFFORT_ORDER)}")
    for key in ("deny", "auto_allow"):
        tokens = rules.get(key)
        if tokens is None:
            continue
        if not isinstance(tokens, list) or not all(isinstance(t, str) and _TOKEN.fullmatch(t) for t in tokens) \
                or len(set(tokens)) != len(tokens):
            errors.add(f"rules.{key}", "invalid_type", "must be an array of unique model tokens without spaces")
            continue
        for token in tokens:
            prefix, slash, rest = token.partition("/")
            if "*" in token:
                if key == "deny" and slash and prefix in engines and rest == auto_route.ENGINE_WILDCARD:
                    continue
                errors.add(f"rules.{key}", "unsupported_wildcard",
                           f"{token!r}: the only wildcard is <engine>/* in deny, which disables that whole engine"
                           if key == "deny" else
                           f"{token!r}: auto_allow names an exact model or family, never a wildcard")
                continue
            if not slash:
                continue
            if prefix not in engines or not rest:
                errors.add(f"rules.{key}", "ambiguous_engine_prefix",
                           f"{token!r}: the part before '/' must be an engine id ({', '.join(engines)}); write an "
                           f"OpenCode model with its engine, e.g. opencode/{token}")
            elif prefix == OPENCODE and key == "auto_allow" and not _opencode_model(rest):
                errors.add(f"rules.{key}", "opencode_model_not_exact",
                           f"{token!r}: auto_allow lifts the OpenCode exclusion only for an exact "
                           "opencode/<provider>/<model> route")
    tiers = rules.get("tiers")
    if tiers is not None:
        if not isinstance(tiers, Mapping):
            errors.add("rules.tiers", "invalid_type", "must map a model id to a tier")
        else:
            for model, tier in tiers.items():
                if not (isinstance(model, str) and _TOKEN.fullmatch(model)) or tier not in auto_route.TIERS:
                    errors.add(f"rules.tiers.{model}", "invalid_value", f"tier must be one of {', '.join(auto_route.TIERS)}")
    if "providers" in rules:
        providers = rules["providers"]
        if (not isinstance(providers, list) or not providers or len(set(map(str, providers))) != len(providers)
                or not all(isinstance(item, str) and item in engines for item in providers)):
            errors.add("rules.providers", "invalid_value",
                       f"must be a non-empty array of unique engines from {', '.join(engines)}")
    if "engine_affinity" in rules:
        affinity = rules["engine_affinity"]
        kinds = (*auto_route.WORK_KIND_ENGINES, "*")
        if not isinstance(affinity, Mapping):
            errors.add("rules.engine_affinity", "invalid_type", "must map a work kind or * to an engine or null")
        else:
            for kind, engine in affinity.items():
                if kind not in kinds:
                    errors.add(f"rules.engine_affinity.{kind}", "invalid_value", f"work kind must be one of {', '.join(kinds)}")
                elif engine is not None and engine not in engines:
                    errors.add(f"rules.engine_affinity.{kind}", "invalid_value",
                               f"must be null or one of {', '.join(engines)}")
    if "enabled" in rules:
        enabled = rules["enabled"]
        if not isinstance(enabled, Mapping):
            errors.add("rules.enabled", "invalid_type", "must map an engine id to an array of exact model ids")
        else:
            for engine, models in enabled.items():
                field = f"rules.enabled.{engine}"
                if engine not in engines:
                    errors.add(field, "invalid_value", f"engine must be one of {', '.join(engines)}")
                elif (not isinstance(models, list) or len(set(map(str, models))) != len(models)
                      or not all(isinstance(m, str) and _TOKEN.fullmatch(m) for m in models)):
                    errors.add(field, "invalid_type", "must be an array of unique exact model ids without spaces")
                elif engine == OPENCODE and not all(_opencode_model(m) for m in models):
                    errors.add(field, "opencode_model_not_exact",
                               "an OpenCode model is its exact provider/model id, e.g. anthropic/claude-sonnet-4-5")
    if "models" in rules:
        for where, code, message in model_policy.validate_models(
                rules["models"], engines=engines, work_kinds=tuple(auto_route.WORK_KIND_ENGINES),
                reserved_word=auto_route._reserved_request_word,
                exact_model=lambda engine, model: engine != OPENCODE or _opencode_model(model),
                prefix="rules.models"):
            errors.add(where, code, message)
    errors.raise_if_any()
    document = {key: rules[key] for key in ROUTING_KEYS if key in rules}
    try:
        auto_route._validate(document)
    except ValueError as error:
        raise ConfigValidationError([dict(field="rules", code="invalid_routing_overrides", message=str(error))]) from None
    return document


def _project_routing_path(project_root) -> Path:
    if not isinstance(project_root, str) or not project_root.startswith("/"):
        raise _error("invalid_project", "The project target needs the absolute project root.", field="project_root")
    root = Path(project_root)
    try:
        if str(root.resolve(strict=True)) != project_root:
            raise ValueError
        info = (root / ".cortex").lstat()
    except (OSError, RuntimeError, ValueError):
        raise _error("project_store_missing", f"{project_root} has no Cortex project store (.cortex); open a task "
                                              "there first.", field="project_root") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise _error("unsafe_storage", f"{root / '.cortex'} is not the owner's real directory.", field="project_root")
    return root / ".cortex" / "routing.json"


def update_routing(target: str, expected_revision, rules, *, project_root: str | None = None,
                   environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Replace the eight supported keys of the ``user`` or ``project`` routing
    file with ``rules`` (an omitted key is removed); other keys in the file
    are kept. The result's ``shadowed_by`` names the source that still wins
    when the written file is not the effective one."""
    if target == "env":
        raise _error("routing_source_read_only", "$CORTEX_ROUTING names a file Cortex never modifies; edit it where it "
                                                 "is kept, or unset the variable.", field="target")
    if target not in ROUTING_TARGETS:
        raise _error("invalid_target", f"target must be one of {', '.join(ROUTING_TARGETS)}.", field="target")
    expected = _expected(expected_revision)
    document = validate_routing_rules(rules)
    env = _env(environ)
    path = _project_routing_path(project_root) if target == "project" else registry.config_root(env) / "routing.json"
    with _owner_config_lock(env):
        data = _read_plain(path)
        _check(f"routing ({target})", expected, _revision(data))
        preserved = {}
        if data is not None:
            existing = _json_object(data, str(path), "invalid_routing_overrides")
            preserved = {key: value for key, value in existing.items() if key not in ROUTING_KEYS}
        body = _pretty({**document, **preserved})
        _replace_file(path, body)
    # Profiles are saved explicitly and are not a routing source: a user save
    # only reports whether the selected profile now differs, and a damaged or
    # newer profiles file never prevents it.
    profile_result = None
    if target == "user":
        from . import routing_profiles
        profile_result = routing_profiles.selection_status(env)
    view = read_routing(project_root, env)
    return dict(view, changed=body != data, target=target, written_revision=_revision(body),
                shadowed_by=None if view["effective_source"] == target else view["effective_source"],
                profile=profile_result)


# --- route catalog (read-only) -------------------------------------------------

def _catalog_entries(record, environ) -> tuple[tuple, str | None]:
    """``record``'s routes as the model selector presents them -- its newest
    genuine catalog snapshot, else its engine's seed catalog -- and when that
    snapshot was observed (``None`` for the seed), without the selector's
    live connection revalidation, which may rewrite the registry."""
    from ..acp.config_options import presented_entries
    from ..catalog import discovery as catalog_discovery
    from ..catalog import store as catalog_store

    view = catalog_store.latest_connection_snapshot(record.id, environ)
    if view is not None:
        return presented_entries(view.entries), view.created_at
    observation = catalog_discovery.seed_observation(record.engine)
    if observation is None:
        return (), None
    entries = catalog_discovery.build_catalog(
        {record.id: record}, {record.id: observation}, seeded_connection_ids=frozenset({record.id})).entries
    return presented_entries(entries), None


#: Per-connection bound of one prompt-free catalog refresh; connections are
#: refreshed in parallel so the whole action stays inside the plugin's
#: write timeout.
REFRESH_CATALOG_TIMEOUT_SECONDS = 20.0


def refresh_route_catalog(project_root: str | None = None, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Re-read every registered connection's model list from its engine
    (``catalog.refresh.perform_refresh``: ``initialize`` + ``session/new``,
    never a prompt, so no model is called) and return the fresh
    :func:`read_route_catalog` with one ``refresh`` outcome per connection.
    A failed refresh keeps that connection's previous snapshot. The engines
    start in a private empty directory, never in the project."""
    from concurrent.futures import ThreadPoolExecutor

    from ..catalog import refresh as catalog_refresh

    env = _env(environ)
    records = sorted(registry._parse_registry_bytes(_registry_bytes(env) or b"").connections.values(),
                     key=lambda record: (record.engine, record.name, record.id))
    with tempfile.TemporaryDirectory(prefix="cortex-catalog-refresh-") as workspace:
        def run(record):
            return catalog_refresh.perform_refresh(record, workspace, environ=env, timeout=REFRESH_CATALOG_TIMEOUT_SECONDS)
        with ThreadPoolExecutor(max_workers=max(1, len(records))) as pool:
            outcomes = list(pool.map(run, records))
    view = read_route_catalog(project_root, env)
    view["refresh"] = [dict(connection_id=outcome.connection_id, success=outcome.success, reason=outcome.reason)
                       for outcome in outcomes]
    return view


def _token_matches(tokens, engine: str, model: str, *, wildcard: bool) -> list[str]:
    """The tokens of ``tokens`` that name ``engine``/``model`` exactly as the
    loader matches them (``auto_route._listed``, plus ``<engine>/*`` for
    ``deny``)."""
    return [token for token in tokens or () if isinstance(token, str) and (
        (wildcard and token == auto_route.engine_wildcard(engine)) or auto_route._listed([token], engine, model))]


def read_route_catalog(project_root: str | None = None, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Every supported engine and every known route, with its effective state
    under the effective routing rules (:func:`read_routing`) and the reason.

    ``automatic`` is the first reason automatic worker routing would skip the
    route (``routing_invalid``, ``connection_not_ready``, ``no_launch_command``,
    ``disabled``, ``not_observed``, ``engine_excluded_from_automatic_routing``,
    ``not_selected``, ``excluded_by_default``, ``superseded``) or
    ``eligible``; what the runtime decides per dispatch (recorded
    unavailability, quota) is not shown. Each route also carries its tier
    and ``tier_source`` (``configured``, ``legacy_tiers``, ``builtin``,
    ``position``; ``null`` when it is not an automatic-routing candidate),
    its engine's ``selection`` mode and ``auto_selected`` membership, the
    ``configured_tier`` and the effective ``policy`` with each attribute's
    source; each engine its ``selection``, ``selected_models`` and the
    ``unmatched_selected`` ids no catalog of it offers. Connection state is
    the stored one: nothing is revalidated, refreshed or written.
    """
    env = _env(environ)
    routing = read_routing(project_root, env)
    valid = bool(routing["effective_valid"])
    rules = (routing["effective_rules"] or {}) if valid else {}
    source_rules = {view["source"]: view.get("rules") or {} for view in routing["sources"] if view.get("valid")}
    records = sorted(registry._parse_registry_bytes(_registry_bytes(env) or b"").connections.values(),
                     key=lambda record: (record.engine, record.name, record.id))
    pairs, problems, observed_at = [], [], {}
    for record in records:
        try:
            entries, observed_at[record.id] = _catalog_entries(record, env)
        except PortableError as error:
            problems.append(dict(connection_id=record.id, code=error.code, message=error.remediation))
            continue
        pairs.extend((entry, record) for entry in entries)
    providers = rules.get("providers")
    deny = rules.get("deny") or ()

    def usable(entry, record) -> bool:
        return entry.is_selectable(record.state, record.auth_mode.value) and record.launch_command is not None

    offered = [entry for entry, record in pairs if usable(entry, record) and not auto_route._is_seed(entry)
               and not auto_route._denied(rules, entry.route.engine, entry.route.model)]
    candidates = auto_route._allowed(rules, offered)
    policies = model_policy.resolve_policies(candidates, rules)
    superseded = auto_route.superseded_routes(candidates)
    applied = auto_route._applied_superseded(rules, candidates, superseded)
    routes = []
    for entry, record in pairs:
        engine, model = entry.route.engine, entry.route.model
        selectable = entry.is_selectable(record.state, record.auth_mode.value)
        deny_tokens = _token_matches(deny, engine, model, wildcard=True)
        enabled = auto_route.model_enabled(rules, engine, model)
        allow_tokens = _token_matches(rules.get("auto_allow"), engine, model, wildcard=False)
        builtin_excluded = auto_route._default_auto_denied({}, entry)
        exclusion = auto_route._auto_exclusion(rules, entry)
        candidate_policy = policies.get(entry.route.id)
        policy = candidate_policy or model_policy.policy_for(engine, model, rules)
        if not valid:
            automatic = "routing_invalid"
        elif not selectable:
            automatic = "connection_not_ready"
        elif record.launch_command is None:
            automatic = "no_launch_command"
        elif deny_tokens or not enabled:
            automatic = "disabled"
        elif auto_route._is_seed(entry):
            automatic = "not_observed"
        elif providers is not None and engine not in providers:
            automatic = "engine_excluded_from_automatic_routing"
        elif exclusion is not None:
            automatic = exclusion
        elif entry.route.id in applied:
            automatic = "superseded"
        else:
            automatic = "eligible"
        routes.append(dict(
            route_id=entry.route.id, engine=engine, model=model, model_name=entry.model_name or model,
            connection_id=record.id, connection_name=record.name, connection_state=record.state.value,
            qualified=record.qualified, observed=not auto_route._is_seed(entry),
            tier=candidate_policy.tier if candidate_policy else None,
            tiers=list(candidate_policy.tiers) if candidate_policy else [],
            tier_source=candidate_policy.tier_source if candidate_policy else None,
            configured_tier=(model_policy.engine_config(rules, engine).get("tiers") or {}).get(model),
            selection=policy.selection, auto_selected=policy.selected, policy=policy.attributes(),
            preferred_for=sorted(policy.preferred_for),
            preferred_inactive=bool(policy.preferred_for) and automatic != "eligible",
            selectable=selectable, in_selector=selectable and not deny_tokens and enabled,
            disabled=bool(deny_tokens) or not enabled, deny_tokens=deny_tokens, enabled=enabled,
            engine_disabled=auto_route.engine_wildcard(engine) in deny,
            default_auto_excluded=(None if not builtin_excluded else
                                   "optional_engine" if engine in auto_route.DEFAULT_AUTO_DENY_ENGINES else "family"),
            auto_allow_tokens=allow_tokens, automatic=automatic, superseded_by=superseded.get(entry.route.id),
            matches={source: dict(deny=_token_matches(values.get("deny"), engine, model, wildcard=True),
                                  auto_allow=_token_matches(values.get("auto_allow"), engine, model, wildcard=False))
                     for source, values in source_rules.items()},
        ))
    engines = []
    for engine in SUPPORTED_ENGINES:
        token = auto_route.engine_wildcard(engine)
        config = model_policy.engine_config(rules, engine)
        selected = list(config.get("auto") or ()) if "auto" in config else None
        offered_models = {entry.route.model for entry, record in pairs if record.engine == engine}
        enabled_models = auto_route.enabled_models(rules, engine)
        engines.append(dict(
            engine=engine, optional=engine in OPTIONAL_ENGINES,
            selection="builtin" if selected is None else "listed", selected_models=selected,
            unmatched_selected=[model for model in selected or () if model not in offered_models],
            enabled_models=None if enabled_models is None else list(enabled_models),
            enabled_default=engine not in auto_route.DEFAULT_DISABLED_ENGINES,
            unmatched_enabled=[model for model in enabled_models or () if model not in offered_models],
            connections=[dict(id=record.id, name=record.name, state=record.state.value, qualified=record.qualified,
                              catalog_observed_at=observed_at.get(record.id))
                         for record in records if record.engine == engine],
            disabled=token in deny, automatic=providers is None or engine in providers,
            default_auto_excluded=engine in auto_route.DEFAULT_AUTO_DENY_ENGINES,
            route_count=sum(1 for _, record in pairs if record.engine == engine),
            matches={source: dict(disabled=token in (values.get("deny") or ()),
                                  automatic=values.get("providers") is None or engine in values["providers"])
                     for source, values in source_rules.items()},
        ))
    return dict(
        surface=ROUTE_CATALOG, project_root=project_root, precedence=routing["precedence"],
        effective_source=routing["effective_source"], effective_valid=valid, providers=providers,
        engines=engines, routes=routes, catalog_problems=problems,
        not_shown=["recorded_unavailability", "quota"],
    )


# --- MCP servers --------------------------------------------------------------

def _mcp_path(environ) -> Path:
    return registry.config_root(environ) / "mcp-servers.json"


def _mcp_document(data: bytes | None) -> tuple[dict, str]:
    if data is None:
        return {"version": 1, "servers": {}}, "servers"
    document = _json_object(data, "mcp-servers.json", "invalid_mcp_registry")
    key = "servers" if "servers" in document else ("mcpServers" if "mcpServers" in document else None)
    if key is None or not isinstance(document[key], dict):
        raise _error("invalid_mcp_registry", "mcp-servers.json must hold a 'servers' object; repair it by hand.")
    return document, key


def _redacted_env(value) -> list[dict[str, object]] | None:
    if isinstance(value, Mapping):
        return [dict(name=name, value_set=isinstance(item, str) and item != "") for name, item in value.items()]
    if isinstance(value, list):
        return [dict(name=item.get("name"), value_set=isinstance(item.get("value"), str) and item.get("value") != "")
                for item in value if isinstance(item, Mapping)]
    return None


def _server_problem(name, spec, path: Path) -> dict[str, str] | None:
    """The loader's own verdict on one entry, as if it were enabled."""
    if not isinstance(name, str) or name == "cortex":
        return dict(code="invalid_mcp_registry", message="'cortex' is reserved for Cortex's own server")
    if not isinstance(spec, Mapping):
        return dict(code="invalid_mcp_registry", message="a server entry must be an object")
    try:
        mcp_registry._normalize_entries({"servers": {name: dict(spec, enabled=True)}}, path)
    except PortableError as error:
        return dict(code=error.code, message=error.remediation)
    return None


def _mcp_view(path: Path, data: bytes | None) -> dict[str, object]:
    document, key = _mcp_document(data)
    servers = []
    for name, spec in document[key].items():
        problem = _server_problem(name, spec, path)
        config = None
        if isinstance(spec, Mapping):
            config = {item: (_redacted_env(value) if item == "env" else value)
                      for item, value in spec.items() if item in MCP_SERVER_KEYS}
        servers.append(dict(
            name=name, enabled=(spec.get("enabled", True) is not False) if isinstance(spec, Mapping) else None,
            valid=problem is None, problem=problem, config=config,
            unsupported_keys=sorted(item for item in spec if item not in MCP_SERVER_KEYS) if isinstance(spec, Mapping) else [],
        ))
    return dict(surface="mcp_servers", path=str(path), revision=_revision(data), exists=data is not None,
                root_key=key, version=document.get("version"), servers=servers,
                editable_keys=list(MCP_SERVER_KEYS), engines=list(SUPPORTED_ENGINES))


def read_mcp_servers(environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """The owner-managed MCP registry; ``env`` values are never returned,
    only whether each is set."""
    env = _env(environ)
    path = _mcp_path(env)
    return _mcp_view(path, registry.read_owner_file(path))


def _old_env(value) -> dict[str, str]:
    if isinstance(value, Mapping):
        return {name: item for name, item in value.items() if isinstance(name, str) and isinstance(item, str)}
    if isinstance(value, list):
        return {item["name"]: item["value"] for item in value
                if isinstance(item, Mapping) and isinstance(item.get("name"), str) and isinstance(item.get("value"), str)}
    return {}


def _resolve_env(new, old, field: str, errors: _Errors) -> dict[str, str] | None:
    """``env`` for the written entry. A value is a new string, or
    ``{"keep": true}`` for the value already stored (never read back)."""
    if new is None:
        return None
    previous = _old_env(old)
    if isinstance(new, Mapping):
        pairs = list(new.items())
    elif isinstance(new, list) and all(isinstance(item, Mapping) for item in new):
        pairs = [(item.get("name"), {"keep": True} if item.get("keep") is True else item.get("value")) for item in new]
    else:
        errors.add(field, "invalid_type", "must map NAME -> value or {\"keep\": true}")
        return None
    result: dict[str, str] = {}
    for name, value in pairs:
        if not isinstance(name, str) or not _ENV_NAME.fullmatch(name) or name in result:
            errors.add(field, "invalid_env_name", "variable names are unique identifiers")
        elif isinstance(value, Mapping) and value.get("keep") is True and set(value) == {"keep"}:
            if name not in previous:
                errors.add(f"{field}.{name}", "unknown_env_value", "has no stored value to keep")
            else:
                result[name] = previous[name]
        elif isinstance(value, str) and "\x00" not in value:
            result[name] = value
        else:
            errors.add(f"{field}.{name}", "invalid_value", "must be a string or {\"keep\": true}")
    return result


def update_mcp_servers(expected_revision, operations, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Apply ``operations`` -- ``{"op": "upsert", "name", "server"}`` (the
    whole entry; an ``env`` value omitted from it is dropped),
    ``{"op": "remove", "name"}``, ``{"op": "set_enabled", "name", "enabled"}``
    -- all or nothing. The resulting registry must pass the runtime loader."""
    expected = _expected(expected_revision)
    if not isinstance(operations, list) or not operations or not all(isinstance(op, Mapping) for op in operations):
        raise _error("invalid_operations", "operations must be a non-empty array of operation objects.", field="operations")
    env = _env(environ)
    path = _mcp_path(env)
    with _owner_config_lock(env):
        data = registry.read_owner_file(path)
        _check("mcp_servers", expected, _revision(data))
        document, key = _mcp_document(data)
        if document.get("version", 1) != 1:
            raise _error("unsupported_mcp_registry_version", "mcp-servers.json 'version' must be 1.", field="version")
        servers = dict(document[key])
        errors = _Errors()
        for index, operation in enumerate(operations):
            field = f"operations[{index}]"
            kind, name = operation.get("op"), operation.get("name")
            if kind == "upsert":
                spec = operation.get("server")
                if not isinstance(name, str) or not _SERVER_NAME.fullmatch(name) or name == "cortex":
                    errors.add(f"{field}.name", "invalid_server_name",
                               "must be a short name of letters, digits, '.', '_' or '-' other than 'cortex'")
                    continue
                if not isinstance(spec, Mapping):
                    errors.add(f"{field}.server", "invalid_type", "must be an object")
                    continue
                for item in sorted(set(spec) - set(MCP_SERVER_KEYS)):
                    errors.add(f"{field}.server.{item}", "unsupported_setting", f"use one of {', '.join(MCP_SERVER_KEYS)}")
                entry = {item: spec[item] for item in MCP_SERVER_KEYS if item in spec and item != "env"}
                previous = servers.get(name) if isinstance(servers.get(name), Mapping) else {}
                resolved = _resolve_env(spec.get("env"), previous.get("env"), f"{field}.server.env", errors)
                if resolved:
                    entry["env"] = resolved
                if "enabled" in entry and type(entry["enabled"]) is not bool:
                    errors.add(f"{field}.server.enabled", "invalid_type", "must be true or false")
                    continue
                problem = _server_problem(name, entry, path)
                if problem is not None:
                    errors.add(f"{field}.server", problem["code"], problem["message"])
                    continue
                servers[name] = entry
            elif kind == "remove":
                if not isinstance(name, str) or name not in servers:
                    errors.add(f"{field}.name", "unknown_server", "names no configured server")
                else:
                    del servers[name]
            elif kind == "set_enabled":
                enabled = operation.get("enabled")
                if not isinstance(name, str) or name not in servers or not isinstance(servers[name], Mapping):
                    errors.add(f"{field}.name", "unknown_server", "names no configured server")
                elif type(enabled) is not bool:
                    errors.add(f"{field}.enabled", "invalid_type", "must be true or false")
                else:
                    servers[name] = dict(servers[name], enabled=enabled)
            else:
                errors.add(f"{field}.op", "unsupported_operation", "must be upsert, remove or set_enabled")
        errors.raise_if_any()
        document = dict(document)
        document.setdefault("version", 1)
        document[key] = servers
        try:
            # The runtime loader's verdict on the whole registry: an enabled
            # entry it would reject (from this request or an older edit)
            # blocks the write, exactly as it would fail every session.
            mcp_registry._normalize_entries(document, path)
        except PortableError as error:
            raise ConfigValidationError([dict(field="servers", code=error.code, message=error.remediation)]) from None
        body = _pretty(document)
        registry.ensure_private_dir(path.parent, root=registry.config_root(env), environ=env)
        registry.atomic_write_private(path, body, directory=path.parent)
    return dict(_mcp_view(path, body), changed=body != data)


# --- connections --------------------------------------------------------------

def _credential_summary(ref) -> dict[str, object]:
    """The credential's kind only -- never its value, and never a path."""
    if isinstance(ref, EngineNativeDir):
        return dict(kind="engine_native_dir", owned=ref.owned)
    if isinstance(ref, SecretFile):
        return dict(kind="secret_file")
    if isinstance(ref, OsKeyring):
        return dict(kind="os_keyring")
    return dict(kind="unknown")


def _connection_view(record) -> dict[str, object]:
    return dict(
        id=record.id, name=record.name, engine=record.engine, provider=record.provider,
        auth_mode=record.auth_mode.value, key_kind=record.key_kind, endpoint=record.endpoint,
        launch_command=list(record.launch_command) if record.launch_command is not None else None,
        credential=_credential_summary(record.credential_ref), state=record.state.value,
        qualified=record.qualified, max_concurrent_sessions=record.max_concurrent_sessions,
        optional_engine=record.engine in OPTIONAL_ENGINES, created_at=record.created_at, updated_at=record.updated_at,
    )


def _qualified_versions(engine: str):
    from ..connections.discovery import QUALIFIED_ADAPTER_VERSIONS

    versions = QUALIFIED_ADAPTER_VERSIONS.get(engine, ())
    return "any" if versions is None else list(versions)


def _registry_bytes(environ) -> bytes | None:
    return registry.read_owner_file(registry.registry_path(environ))


def _connections_view(data: bytes | None) -> dict[str, object]:
    state = registry._parse_registry_bytes(data or b"")
    records = sorted(state.connections.values(), key=lambda record: (record.engine, record.name, record.id))
    engines = []
    for engine in SUPPORTED_ENGINES:
        engines.append(dict(
            engine=engine, optional=engine in OPTIONAL_ENGINES,
            onboarding="explicit_opt_in" if engine in OPTIONAL_ENGINES else "discovery_or_setup",
            auth_modes={mode.value: support.qualified for mode, support in SUPPORT_MATRIX.get(engine, {}).items()},
            qualified_versions=_qualified_versions(engine),
            connection_ids=[record.id for record in records if record.engine == engine],
        ))
    return dict(surface="connections", revision=_revision(data), exists=data is not None,
                connections=[_connection_view(record) for record in records], engines=engines,
                operations=list(CONNECTION_OPERATIONS), max_concurrent_sessions_limit=MAX_CONCURRENT_SESSIONS_LIMIT)


def read_connections(environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Connection metadata; credential values and paths are never included."""
    return _connections_view(_registry_bytes(_env(environ)))


def update_connection(expected_revision, connection_id, operation, *, name=None, max_concurrent_sessions=None,
                      environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """``rename``/``set_max_concurrent_sessions`` edit one record inside a
    registry transaction; ``validate``/``remove`` run the existing
    ``connections.onboarding`` operation once the revision matched."""
    from ..connections import onboarding

    expected = _expected(expected_revision)
    if not isinstance(connection_id, str) or not _CONNECTION_ID.fullmatch(connection_id):
        raise _error("invalid_connection_id", "connection_id must be cn_<12 hex>.", field="connection_id")
    if operation not in CONNECTION_OPERATIONS:
        raise _error("unsupported_operation", f"operation must be one of {', '.join(CONNECTION_OPERATIONS)}.",
                     field="operation")
    if operation == "rename" and not _clean_text(name, limit=120):
        raise ConfigValidationError([dict(field="name", code="invalid_value", message="must be 1-120 printable characters")])
    if operation == "set_max_concurrent_sessions" and not (
            _non_negative_int(max_concurrent_sessions) and 1 <= max_concurrent_sessions <= MAX_CONCURRENT_SESSIONS_LIMIT):
        raise ConfigValidationError([dict(field="max_concurrent_sessions", code="invalid_value",
                                          message=f"must be an integer from 1 through {MAX_CONCURRENT_SESSIONS_LIMIT}")])
    env = _env(environ)
    notes: list[str] = []
    if operation in ("rename", "set_max_concurrent_sessions"):
        with registry.transaction(env) as state:
            _check("connections", expected, _revision(_registry_bytes(env)))
            record = state.connections.get(connection_id)
            if record is None:
                raise _error("unknown_connection", "connection_id names no registered connection.", field="connection_id")
            changes = dict(name=name) if operation == "rename" else dict(max_concurrent_sessions=max_concurrent_sessions)
            state.connections[connection_id] = replace(record, updated_at=_now_iso(), **changes)
    else:
        with _owner_config_lock(env):
            data = _registry_bytes(env)
            _check("connections", expected, _revision(data))
            if connection_id not in registry._parse_registry_bytes(data or b"").connections:
                raise _error("unknown_connection", "connection_id names no registered connection.", field="connection_id")
        if operation == "validate":
            onboarding.validate(connection_id, environ=env)
        else:
            onboarding.remove(connection_id, environ=env)
            try:
                defaults = configuration_store.inspect_user_defaults(env)["defaults"]
            except PortableError:
                defaults = {}
            if isinstance(defaults, dict) and defaults.get("route_connection_id") == connection_id:
                notes.append("user_default_route_uses_removed_connection")
    return dict(read_connections(env), changed=True, notes=notes)


def discover_opencode(*, binary: str | None = None, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Look for an OpenCode binary (``connections.discovery.discover_opencode``:
    its ``--version`` runs in a network-less sandbox). Writes nothing."""
    from ..connections import discovery

    env = _env(environ)
    if binary is not None and not (isinstance(binary, str) and binary.startswith("/")):
        raise _error("invalid_binary", "binary must be an absolute path.", field="binary")
    found = discovery.discover_opencode(binary=binary, environ=env)
    onboarded = [item["id"] for item in read_connections(env)["connections"] if item["engine"] == OPENCODE]
    if found is None:
        return dict(found=False, onboarded_connection_ids=onboarded, qualified_versions=_qualified_versions(OPENCODE))
    return dict(found=True, binary=found.launch_command[0], version=found.version,
                qualified=discovery.is_qualified(OPENCODE, found.version),
                qualified_versions=_qualified_versions(OPENCODE), onboarded_connection_ids=onboarded)


_OPENCODE_NEXT_ACTION = {
    "configured": "refresh_catalog",
    "authorization_required": "opencode_auth_login",
    "invalid": "make_credential_dir_private",
}


def onboard_opencode(expected_revision, *, explicit_opt_in, binary: str | None = None, name: str = "OpenCode",
                     environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Register the optional OpenCode v2 engine -- only on an explicit
    opt-in, only from a trusted binary of a qualified version, referencing
    the owner's own ``~/.local/share/opencode`` login (never copied). The
    same steps as ``scripts/cortex_acp_setup.py add-opencode-connection``.
    Routing is not touched: OpenCode stays out of automatic routing until
    ``routing.json`` opts a route in."""
    from ..connections import discovery, onboarding
    from ..engine import opencode_install

    if explicit_opt_in is not True:
        raise _error("explicit_opt_in_required",
                     "OpenCode is optional and never onboarded implicitly; send explicit_opt_in: true.",
                     field="explicit_opt_in")
    expected = _expected(expected_revision)
    if not _clean_text(name, limit=120):
        raise ConfigValidationError([dict(field="name", code="invalid_value", message="must be 1-120 printable characters")])
    if binary is not None and not (isinstance(binary, str) and binary.startswith("/")):
        raise _error("invalid_binary", "binary must be an absolute path.", field="binary")
    env = _env(environ)
    with _owner_config_lock(env):
        _check("connections", expected, _revision(_registry_bytes(env)))
    found = discovery.discover_opencode(binary=binary, environ=env)
    if found is None:
        raise _error("opencode_not_found",
                     "No trusted OpenCode binary reporting a version was found (looked at the given binary, "
                     "$CORTEX_OPENCODE_BINARY, ~/.opencode/bin/opencode and PATH).", field="binary")
    install = opencode_install.verify_install(
        opencode_install.OpenCodeInstall(binary=found.launch_command[0], version=found.version), env)
    record = onboarding.create_referenced_connection(
        name=name, engine=OPENCODE, provider=OPENCODE,
        launch_command=opencode_install.launch_command_for(install), reuse=True, environ=env,
    )
    record = onboarding.validate(record.id, environ=env)
    return dict(
        read_connections(env), changed=True, connection=_connection_view(record),
        opencode=dict(version=install.version, next_action=_OPENCODE_NEXT_ACTION.get(record.state.value),
                      automatic_routing="excluded_until_routing_opts_in"),
    )


# --- JSON request/response ------------------------------------------------------

def _fields(request: Mapping, allowed: tuple[str, ...]) -> None:
    unknown = sorted(set(request) - {"api_version", "action", "surface", *allowed})
    if unknown:
        raise _error("unsupported_request_field", f"Unknown request field(s) {', '.join(unknown)} for this action.",
                     field=unknown[0])


def _open_project_store(project, *, write: bool):
    from ..contracts import StoreError
    from ..project_storage import canonical_project, project_store_directory
    from ..store import Store

    try:
        root = canonical_project(project)
    except StoreError:
        raise _error("invalid_project", "project must be an existing absolute project root without symlinks.",
                     field="project") from None
    directory = project_store_directory(root)
    if not (directory / "cortex.sqlite3").is_file():
        raise _error("project_store_missing", f"{root} has no Cortex project store yet; open a task there first.",
                     field="project")
    return root, Store(directory, initialize=write, project_root=root)


def read_routing_profiles(project_root=None, environ=None):
    from . import routing_profiles
    return routing_profiles.read_profiles(project_root, environ)


def read_all(project: str | None = None, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Every surface at once; a surface that cannot be read reports its
    error instead of hiding the others."""
    env = _env(environ)
    readers = dict(
        user_defaults=lambda: read_user_defaults(env),
        routing=lambda: read_routing(project, env),
        routing_profiles=lambda: read_routing_profiles(project, env),
        mcp_servers=lambda: read_mcp_servers(env),
        connections=lambda: read_connections(env),
    )
    if project is not None:
        def project_reader():
            root, store = _open_project_store(project, write=False)
            return read_project_ceiling(store, root, env)
        readers["project_ceiling"] = project_reader
    result: dict[str, object] = {}
    for surface, reader in readers.items():
        try:
            result[surface] = dict(ok=True, result=reader())
        except PortableError as error:
            result[surface] = dict(ok=False, error=error_payload(error))
    return result


def _dispatch(request, environ) -> dict[str, object]:
    if not isinstance(request, Mapping):
        raise _error("invalid_request", "The request must be a JSON object.")
    if request.get("api_version", API_VERSION) != API_VERSION:
        raise _error("unsupported_api_version", f"This Cortex answers configuration API version {API_VERSION}.",
                     field="api_version")
    action, surface = request.get("action"), request.get("surface")
    if action == "read" and surface == "all":
        _fields(request, ("project",))
        return read_all(request.get("project"), environ)
    if surface == ROUTE_CATALOG:
        if action not in ("read", "refresh"):
            raise _error("unsupported_action", f"{ROUTE_CATALOG} is read-only (or 'refresh' to re-read the engines' "
                                               "model lists); enable or disable an engine or model with a routing "
                                               "update.", field="action")
        _fields(request, ("project",))
        if action == "refresh":
            return refresh_route_catalog(request.get("project"), environ)
        return read_route_catalog(request.get("project"), environ)
    if surface not in SURFACES:
        raise _error("unknown_surface", f"surface must be one of {', '.join(SURFACES)} (or 'all' or "
                                        f"'{ROUTE_CATALOG}' for read).", field="surface")
    if surface == "user_defaults" and action == "read":
        _fields(request, ())
        return read_user_defaults(environ)
    if surface == "user_defaults" and action == "update":
        _fields(request, ("expected_revision", "defaults", "ceiling"))
        return update_user_defaults(request.get("expected_revision"), defaults=request.get("defaults"),
                                    ceiling=request["ceiling"] if "ceiling" in request else _KEEP, environ=environ)
    if surface == "project_ceiling" and action in ("read", "update"):
        _fields(request, ("project",) if action == "read" else ("project", "expected_revision", "ceiling"))
        if action == "update" and "ceiling" not in request:
            raise _error("missing_field", "Send 'ceiling' (an object, or null to remove it).", field="ceiling")
        root, store = _open_project_store(request.get("project"), write=action == "update")
        if action == "read":
            return read_project_ceiling(store, root, environ)
        return update_project_ceiling(store, root, request.get("expected_revision"), request["ceiling"], environ)
    if surface == "routing" and action == "read":
        _fields(request, ("project",))
        return read_routing(request.get("project"), environ)
    if surface == "routing" and action == "update":
        _fields(request, ("target", "expected_revision", "rules", "project"))
        return update_routing(request.get("target"), request.get("expected_revision"), request.get("rules"),
                              project_root=request.get("project"), environ=environ)
    if surface == "routing_profiles":
        from . import routing_profiles
        if action == "read":
            _fields(request, ("project",))
            return routing_profiles.read_profiles(request.get("project"), environ)
        if action in PROFILE_ACTION_FIELDS:
            _fields(request, PROFILE_ACTION_FIELDS[action])
            return routing_profiles.mutate(
                action, request.get("expected_revision"), name=request.get("name"),
                description=request.get("description"), rename_to=request.get("rename_to"),
                expected_revisions=request.get("expected_revisions"),
                discard_modifications=request.get("discard_modifications", False),
                project_root=request.get("project"), environ=environ)
    if surface == "mcp_servers" and action == "read":
        _fields(request, ())
        return read_mcp_servers(environ)
    if surface == "mcp_servers" and action == "update":
        _fields(request, ("expected_revision", "operations"))
        return update_mcp_servers(request.get("expected_revision"), request.get("operations"), environ)
    if surface == "connections" and action == "read":
        _fields(request, ())
        return read_connections(environ)
    if surface == "connections" and action == "update":
        _fields(request, ("expected_revision", "connection_id", "operation", "name", "max_concurrent_sessions"))
        return update_connection(request.get("expected_revision"), request.get("connection_id"),
                                 request.get("operation"), name=request.get("name"),
                                 max_concurrent_sessions=request.get("max_concurrent_sessions"), environ=environ)
    if surface == "connections" and action == "discover_opencode":
        _fields(request, ("binary",))
        return discover_opencode(binary=request.get("binary"), environ=environ)
    if surface == "connections" and action == "onboard_opencode":
        _fields(request, ("expected_revision", "explicit_opt_in", "binary", "name"))
        return onboard_opencode(request.get("expected_revision"), explicit_opt_in=request.get("explicit_opt_in"),
                                binary=request.get("binary"), name=request.get("name", "OpenCode"), environ=environ)
    raise _error("unsupported_action", f"Action {action!r} is not supported for surface {surface!r}.", field="action")


def execute(request, *, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """One JSON request -> ``{"api_version", "ok", "result" | "error"}``."""
    from ..contracts import StoreError

    try:
        result = _dispatch(request, _env(environ))
    except PortableError as error:
        return dict(api_version=API_VERSION, ok=False, error=error_payload(error))
    except StoreError as error:
        code = error.args[0] if error.args and isinstance(error.args[0], str) else "storage_error"
        return dict(api_version=API_VERSION, ok=False, error=dict(
            category="validation", code=code, message="The project store refused the request.",
            field=getattr(error, "field", None), retry_eligible=False))
    return dict(api_version=API_VERSION, ok=True, result=result)


def main(stdin=None, stdout=None) -> int:
    """Answer one JSON request read from stdin; exit 0 on success, 1 on refusal."""
    stdin = sys.stdin if stdin is None else stdin
    stdout = sys.stdout if stdout is None else stdout
    try:
        request = json.loads(stdin.read())
    except ValueError:
        response = dict(api_version=API_VERSION, ok=False, error=dict(
            category="validation", code="invalid_request", message="stdin must hold one JSON request object.",
            field=None, retry_eligible=False))
    else:
        response = execute(request)
    stdout.write(json.dumps(response, ensure_ascii=False, sort_keys=True) + "\n")
    return 0 if response["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
