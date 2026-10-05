"""Explicit install-only public model defaults and separate legacy cleanup.

Existing owner files are never patched, merged or replaced by installation.
Runtime routing does not import or read the installation asset.
"""
from __future__ import annotations

import json
import os
import tempfile
import time
from pathlib import Path
from typing import Mapping

from . import auto_route

TEMPLATE_PATH = Path(__file__).resolve().parents[3] / "policy" / "model-settings-default.json"


def load_model_settings_template(path: Path | None = None) -> dict:
    """Validate public schema_version=1 {routing, coordinator} install data.

    Returns a fresh document. Coordinator is portable engine/model/parameters,
    never a connection or route fingerprint. This API is install/migration only.
    """
    from ..connections.model import SUPPORTED_ENGINES

    document = json.loads((TEMPLATE_PATH if path is None else path).read_text(encoding="utf-8"))
    if (not isinstance(document, dict) or (not {"schema_version", "routing", "coordinator"} <= set(document)
                                         or set(document) - {"schema_version", "routing", "coordinator", "language"})
            or type(document["schema_version"]) is not int or document["schema_version"] != 1):
        raise ValueError("invalid model-settings template envelope")
    if "language" in document and (not isinstance(document["language"], str)
                                   or document["language"] not in {"en", "ru"}):
        raise ValueError("invalid install language")
    routing = document["routing"]
    if not isinstance(routing, dict) or set(routing) - {
            "model_policy_version", "pin", "deny", "enabled", "auto_allow", "providers", "engine_affinity", "models", "quota_windows"}:
        raise ValueError("template routing contains unsupported/private fields")
    auto_route.validate_routing(routing)
    coordinator = document["coordinator"]
    if (not isinstance(coordinator, dict) or set(coordinator) != {"engine", "model", "parameters"}
            or coordinator["engine"] not in SUPPORTED_ENGINES
            or not auto_route.model_policy._model_id(coordinator["model"])
            or not auto_route.model_enabled(routing, coordinator["engine"], coordinator["model"])
            or auto_route._denied(routing, coordinator["engine"], coordinator["model"])
            or not isinstance(coordinator["parameters"], dict)
            or set(coordinator["parameters"]) - {"thought_level", "mode"}
            or any(not isinstance(v, str) for v in coordinator["parameters"].values())
            or coordinator["parameters"].get("thought_level") not in auto_route.model_policy.POLICY_EFFORTS
            or coordinator["parameters"].get("mode") not in {None, "agent"}):
        raise ValueError("invalid portable coordinator preference")
    return document


def default_routing() -> dict:
    """Fresh validated routing data for explicit absent-file installation."""
    return load_model_settings_template()["routing"]


def default_coordinator() -> dict:
    """Fresh portable preference; dependent installer owns local binding."""
    return load_model_settings_template()["coordinator"]


def _create_private(path: Path, body: str) -> bool:
    """Publish a complete private file without replacing a racing owner file."""
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            fd = None
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError:
            return False
        return True
    finally:
        if fd is not None:
            os.close(fd)
        os.unlink(temporary)


def _config_root(environ: Mapping[str, str] | None) -> Path:
    from ..connections import registry

    return registry.config_root(None if environ is None else dict(environ))


def _write_private(path: Path, body: str) -> None:
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            fd = None
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if fd is not None:
            os.close(fd)
        if temporary is not None:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def _regular_file(path: Path) -> bool:
    if path.is_symlink():
        raise ValueError(f"refusing symlinked configuration file {path}")
    return path.is_file()


def ensure_model_settings(environ=None, *, routing_only=False):
    """Validate the entire asset first; install absent owner files without replacement.

    Existing owner files (including malformed and legacy data) are diagnostic-only.
    The owner lock and single-link private publication use current storage primitives.
    """
    from ..connections import registry
    from ..configuration import management as m
    from ..configuration.resolution import validate_portable_coordinator
    from ..portable.errors import PortableError

    root = _config_root(environ)
    try:
        template = load_model_settings_template()
        registry.ensure_private_dir(root, root=root, environ=environ)
        with m._owner_config_lock(environ):
            result = {}
            names = ("routing",) if routing_only else ("routing", "defaults")
            for name in names:
                path = root / (name + ".json")
                try:
                    data = registry.read_owner_file(path)
                    if data is None:
                        payload = (template["routing"] if name == "routing" else
                                   {"defaults": dict(coordinator=template["coordinator"],
                                                     **({"language": template["language"]} if "language" in template else {}))})
                        created = _create_private(path, json.dumps(payload, indent=2, ensure_ascii=False) + "\n")
                        result[name] = dict(path=str(path), status="created" if created else "existing", changed=created)
                        continue
                    if name == "routing":
                        auto_route.validate_routing(json.loads(data))
                    else:
                        values, _, _ = m._parse_user_defaults(data)
                        errors = m._Errors()
                        m._validate_default_patch({k: v for k, v in values.items() if k in m.USER_DEFAULT_KEYS}, errors)
                        errors.raise_if_any()
                        if values.get("coordinator") is not None:
                            validate_portable_coordinator(values["coordinator"])
                        elif not all(values.get(k) for k in m.ROUTE_DEFAULT_KEYS):
                            result[name] = dict(path=str(path), status="coordinator_default_missing", changed=False)
                            continue
                    result[name] = dict(path=str(path), status="current", changed=False)
                except PortableError as error:
                    result[name] = dict(path=str(path), status=error.code, changed=False)
                except (OSError, ValueError) as error:
                    result[name] = dict(path=str(path), status="invalid", changed=False, error=str(error))
            return result
    except PortableError as error:
        status = error.code
    except (OSError, ValueError):
        status = "invalid"
    return {name: dict(path=str(root / (name + ".json")), status=status, changed=False)
            for name in (("routing",) if routing_only else ("routing", "defaults"))}


def ensure_default_routing(environ: Mapping[str, str] | None = None) -> dict:
    """Compatibility installer entry point: only absent routing is written."""
    root = _config_root(environ)
    if not root.exists() and not root.is_symlink():
        return dict(path=str(root / "routing.json"), status="config_root_missing", changed=False)
    return ensure_model_settings(environ, routing_only=True)["routing"]


def clean_route_policy(environ: Mapping[str, str] | None = None) -> dict:
    """Drop globally denied models from a leftover ``route-policy.json``."""
    from ..portable.errors import PortableError

    root = _config_root(environ)
    path = root / "route-policy.json"
    try:
        if not _regular_file(path):
            return dict(path=str(path), status="absent", changed=False)
        routing = root / "routing.json"
        deny = []
        if _regular_file(routing):
            loaded = json.loads(routing.read_text(encoding="utf-8"))
            auto_route._validate(loaded)
            deny = list(loaded.get("deny") or [])
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            raise ValueError("must contain a JSON object")
    except PortableError as error:
        return dict(path=str(path), status=error.code, changed=False)
    except (OSError, ValueError) as error:
        return dict(path=str(path), status="invalid", changed=False, error=str(error))
    removed = []

    def keep(route) -> bool:
        if not isinstance(route, dict):
            return True
        engine, model = route.get("engine"), route.get("model")
        if not isinstance(engine, str) or not isinstance(model, str) or model == "*":
            return True
        if auto_route._listed(deny, engine, model):
            removed.append(f"{engine}/{model}")
            return False
        return True

    cleaned = dict(data)
    roles = data.get("roles")
    emptied = []
    if isinstance(roles, dict):
        cleaned_roles = {}
        for role, routes in roles.items():
            if not isinstance(routes, list):
                cleaned_roles[role] = routes
                continue
            kept = [route for route in routes if keep(route)]
            if kept or not routes:
                cleaned_roles[role] = kept
            else:
                emptied.append(role)
        cleaned["roles"] = cleaned_roles
    if isinstance(data.get("default"), list):
        cleaned["default"] = [route for route in data["default"] if keep(route)]
    if not removed:
        return dict(path=str(path), status="current", changed=False, removed=[])
    backup = path.with_name(f"{path.name}.cortex-backup-{time.time_ns()}")
    try:
        _write_private(backup, path.read_text(encoding="utf-8"))
        _write_private(path, json.dumps(cleaned, indent=2, ensure_ascii=False) + "\n")
    except OSError as error:
        return dict(path=str(path), status="write_failed", changed=False, error=str(error))
    return dict(path=str(path), status="cleaned", changed=True, removed=sorted(set(removed)),
                removed_roles=sorted(emptied), backup=str(backup))


def apply(environ: Mapping[str, str] | None = None) -> dict:
    """Install only; destructive legacy cleanup requires a separate explicit call."""
    return dict(ensure_model_settings(environ), route_policy=dict(status="not_requested", changed=False))
