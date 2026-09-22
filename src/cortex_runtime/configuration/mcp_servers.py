"""Cortex-owned MCP server configuration.

ACP clients are allowed to send an ``mcpServers`` list, but not every client
does (Paseo currently sends an empty list).  The engine sessions therefore
also load an explicit Cortex registry.  Values are read only into memory and
are never copied into the project store, task reports, or diagnostics.

Configuration is intentionally small and stdio-only for the current engine
bridge::

    {"version": 1, "servers": {"codebase-memory-mcp": {
        "command": "/absolute/path/to/server", "args": [],
        "env": {"TOKEN": "secret"}, "enabled": true,
        "allowed_tools": ["inspect"], "read_only_tools": ["inspect"],
        "coordinator_tools": ["inspect"],
        "read_paths": ["/absolute/path/to/server"],
        "write_paths": ["/absolute/path/to/its/cache"], "network": false
    }}}

External servers run with the owner's real ``HOME``; ``write_paths`` is the
only writable state they get beyond the task scope (e.g. an index cache).

Every launch also gets its own private runtime directory, exported as
``XDG_RUNTIME_DIR`` and under each name in ``runtime_dir_env``. A server that
coordinates through a per-user daemon (codebase-memory-mcp's
``CBM_RUNTIME_DIR``) otherwise finds a daemon started outside the sandbox and
cannot verify it across the sandbox's process namespaces: it then answers
``initialize`` with an id-less error and never starts.

Only the user-global registry (``$XDG_CONFIG_HOME/cortex/mcp-servers.json``)
is trusted. A project-local registry is deliberately unsupported because the
project can be modified by a worker and must not gain a persistent executable
MCP capability for later sessions. Client entries remain an explicit
session-scoped fallback. Files are accepted only when they are owner-only
regular files, matching the connection registry's secret handling.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Mapping

from ..connections import registry
from ..portable.errors import PortableError, PortableErrorCategory

_FILENAME = "mcp-servers.json"
_ENV_NAME = __import__("re").compile(r"[A-Z_][A-Z0-9_]*")
# Set by the sandbox itself; a registry entry may not redirect them.
_RESERVED_ENV = frozenset({"PATH", "HOME", "TMPDIR", "XDG_RUNTIME_DIR"})


def _error(code: str, remediation: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
    )


def _global_path(environ: Mapping[str, str] | None) -> Path:
    # Use the same guarded XDG root as connection storage.  Do not resolve a
    # caller-provided relative path against the current working directory.
    return registry.config_root(dict(environ or os.environ)) / _FILENAME


def _read_json(path: Path) -> object | None:
    raw = registry.read_owner_file(path)
    if raw is None:
        return None
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise _error("invalid_mcp_registry", f"Cannot parse owner-only MCP registry {path}: {exc}") from None


def _as_env(value, *, path: Path, name: str) -> list[dict[str, str]]:
    if value is None:
        return []
    if isinstance(value, Mapping):
        pairs = value.items()
    elif isinstance(value, list):
        pairs = []
        for item in value:
            if not isinstance(item, Mapping) or not isinstance(item.get("name"), str) or not isinstance(item.get("value"), str):
                raise _error("invalid_mcp_registry", f"{path}: {name}.env must contain name/value strings.")
            pairs.append((item["name"], item["value"]))
    else:
        raise _error("invalid_mcp_registry", f"{path}: {name}.env must be an object or array.")
    result = []
    for key, item in pairs:
        if not isinstance(key, str) or not key or "\x00" in key or not isinstance(item, str) or "\x00" in item:
            raise _error("invalid_mcp_registry", f"{path}: {name}.env contains an invalid variable.")
        result.append({"name": key, "value": item})
    return result


def _normalize_entries(raw: object, path: Path) -> list[dict[str, object]]:
    if not isinstance(raw, Mapping):
        raise _error("invalid_mcp_registry", f"{path} must contain an object.")
    entries = raw.get("servers", raw.get("mcpServers"))
    if entries is None:
        raise _error("invalid_mcp_registry", f"{path} must contain a 'servers' object.")
    if not isinstance(entries, Mapping):
        raise _error("invalid_mcp_registry", f"{path}.servers must be an object keyed by server name.")
    result: list[dict[str, object]] = []
    for name, spec in entries.items():
        if not isinstance(name, str) or not name or name == "cortex" or not isinstance(spec, Mapping):
            raise _error("invalid_mcp_registry", f"{path} contains an invalid server entry.")
        if spec.get("enabled", True) is False:
            continue
        command = spec.get("command")
        if not isinstance(command, str) or not command or "\x00" in command:
            raise _error("invalid_mcp_registry", f"{path}: {name}.command must be a non-empty string.")
        args = spec.get("args", [])
        if not isinstance(args, list) or any(not isinstance(arg, str) or "\x00" in arg for arg in args):
            raise _error("invalid_mcp_registry", f"{path}: {name}.args must be a string array.")
        roles = spec.get("roles")
        if roles is not None and (
            not isinstance(roles, list) or not roles
            or any(role not in ("coordinator", "worker") for role in roles)
            or len(set(roles)) != len(roles)
        ):
            raise _error("invalid_mcp_registry", f"{path}: {name}.roles must be a non-empty array of unique coordinator/worker roles.")
        engines = spec.get("engines")
        if engines is not None and (
            not isinstance(engines, list) or not engines
            or any(engine not in ("codex-acp", "claude-agent-acp") for engine in engines)
            or len(set(engines)) != len(engines)
        ):
            raise _error("invalid_mcp_registry", f"{path}: {name}.engines must be a non-empty array of unique codex-acp/claude-agent-acp engines.")
        entry = dict(
            name=name,
            command=command,
            args=list(args),
            env=_as_env(spec.get("env"), path=path, name=name),
        )
        allowed_tools=spec.get('allowed_tools')
        if not isinstance(allowed_tools,list) or not allowed_tools or any(not isinstance(tool,str) or not tool for tool in allowed_tools):
            raise _error('external_mcp_explicit_tool_grants_required',f'{path}: {name}.allowed_tools must name the exact permitted tools.')
        grants={}
        for key in ('read_paths','write_paths'):
            grants[key]=spec.get(key,[])
            if not isinstance(grants[key],list) or any(not isinstance(value,str) or not Path(value).is_absolute() for value in grants[key]):
                raise _error('invalid_mcp_registry',f'{path}: {name}.{key} must contain absolute paths.')
        if type(spec.get('network',False)) is not bool:
            raise _error('invalid_mcp_registry',f'{path}: {name}.network must be boolean.')
        entry.update(allowed_tools=list(allowed_tools),read_paths=list(grants['read_paths']),
                     write_paths=list(grants['write_paths']),network=spec.get('network',False))
        readonly=spec.get('read_only_tools',[])
        if not isinstance(readonly,list) or any(tool not in allowed_tools for tool in readonly):
            raise _error('invalid_mcp_registry',f'{path}: {name}.read_only_tools must be a subset of allowed_tools.')
        entry['read_only_tools']=list(readonly)
        runtime_env=spec.get('runtime_dir_env',[])
        if (not isinstance(runtime_env,list) or len(set(runtime_env))!=len(runtime_env)
                or any(not isinstance(item,str) or not _ENV_NAME.fullmatch(item) or item in _RESERVED_ENV
                       for item in runtime_env)):
            raise _error('invalid_mcp_registry',f'{path}: {name}.runtime_dir_env must be unique environment variable names.')
        if runtime_env:
            entry['runtime_dir_env']=list(runtime_env)
        if 'coordinator_tools' in spec:
            lookup=spec['coordinator_tools']
            if not isinstance(lookup,list) or any(tool not in readonly for tool in lookup):
                raise _error('invalid_mcp_registry',f'{path}: {name}.coordinator_tools must be a subset of read_only_tools.')
            entry['coordinator_tools']=list(lookup)
        if roles is not None:
            # Private Cortex routing metadata; never forward as an ACP field.
            entry["roles"] = list(roles)
        if engines is not None:
            entry["engines"] = list(engines)
        result.append(entry)
    return result


def load_mcp_servers(project_root: str | os.PathLike[str], *, client_servers=(), environ: Mapping[str, str] | None = None) -> list[dict[str, object]]:
    """Return the effective stdio MCP list for one project.

    Missing registries are normal.  Invalid existing registries fail closed;
    silently dropping a configured tool would make the model's capability set
    untrustworthy.  The returned entries are copies safe for per-session use.
    """
    # Never load a project-local registry: workers and repository contents are
    # not an authority to install persistent executable capabilities.
    sources = ((_global_path(environ), True),)
    owned: dict[str, dict[str, object]] = {}
    for path, _ in sources:
        raw = _read_json(path)
        if raw is None:
            continue
        for entry in _normalize_entries(raw, path):
            owned.setdefault(entry["name"], entry)
    result: list[dict[str, object]] = []
    seen: set[str] = set()
    # Cortex-owned entries win over client-supplied duplicates.  The client
    # list is still forwarded for names Cortex does not own.
    for entry in owned.values():
        result.append(dict(entry))
        seen.add(str(entry["name"]))
    for entry in client_servers or ():
        if not isinstance(entry, Mapping) or not isinstance(entry.get("name"), str):
            continue
        if entry["name"] in seen or entry["name"] == "cortex":
            continue
        result.append(dict(entry))
        seen.add(entry["name"])
    return result
