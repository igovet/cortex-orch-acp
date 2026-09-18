"""Derive and verify a pinned engine install straight from a connection's
own recorded ``launch_command`` -- the single place both real production
callers (``catalog.refresh``'s per-model effort enrichment, and
``delegation.live_adapters``'s real engine launch) go through instead of
each hand-building/receiving a caller-supplied install out of band (which,
before this module, nothing in production code ever did -- see T035's
follow-up report: codex per-model efforts were unreachable because
``refresh-catalog`` never built an ``install``, and a claude-agent-acp
launch's ``launch_command`` was never re-verified against its package at
all).

``ConnectionRecord.launch_command`` for a real, operator-pinned engine
install is always exactly ``(node_binary, adapter_entry)`` -- see
``codex_install``/``claude_install``'s own module docstrings, and every
real connection in this project's registry (``launch_command`` is set once,
at onboarding, from ``codex_install.launch_command_for``/
``claude_install.launch_command_for`` of a verified install). A
``launch_command`` of any other shape (for example this repository's own
test doubles, a multi-argument ``python3 -B fake_engine.py scenario.json``)
is never a real npm-packaged install and is reported as
:class:`InstallUnresolvable` rather than treated as a verification failure
-- callers distinguish "this connection's launch command isn't even
install-shaped" (never fatal, on either call path) from "it is install-shaped
and verification of it failed" (:class:`~.portable.errors.PortableError`,
fatal on the launch path, non-fatal-but-recorded on the refresh path).

The codex-side bundled ``codex`` binary is located by replicating the
platform-selection logic the installed ``@openai/codex`` package's own
``bin/codex.js`` performs (see that file's ``PLATFORM_PACKAGE_BY_TARGET``
table, mirrored in ``_CODEX_TARGET_TRIPLE_BY_PLATFORM``/
``_CODEX_PLATFORM_PACKAGE`` below): ``<package
root>/node_modules/@openai/codex-<platform>/vendor/<target triple>/bin/codex``.
"""
from __future__ import annotations

import hashlib
import json
import platform
from pathlib import Path

from ..portable.errors import PortableError, PortableErrorCategory
from . import claude_install, codex_install

_MAX_PACKAGE_JSON_SEARCH_DEPTH = 6


class InstallUnresolvable(Exception):
    """``record.launch_command`` is not shaped like a real, operator-pinned
    npm install (wrong arity, no ``package.json`` found above the adapter
    entry, no bundled ``@openai/codex`` for codex-acp, ...). Never itself a
    verification *failure* -- a caller sees this for a connection that was
    simply never a real npm-packaged install to begin with (this
    repository's own fake-engine test doubles included) and treats it the
    same as "no install available", never as a drift/tamper finding."""


def _validation_error(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=False, field=field,
    )


def _hash_file(path: Path) -> str:
    hasher = hashlib.sha256()
    try:
        hasher.update(path.read_bytes())
    except OSError as error:
        raise InstallUnresolvable(f"could not read {path}: {error}") from error
    return hasher.hexdigest()


def _read_package_json(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise InstallUnresolvable(f"could not parse {path} as JSON: {error}") from error
    if not isinstance(data, dict):
        raise InstallUnresolvable(f"{path} is not a JSON object.")
    return data


def _find_package_json(adapter_entry: Path) -> Path:
    """Walk upward (bounded) from ``adapter_entry``'s own directory looking
    for the nearest ``package.json`` -- the package root a real install's
    ``dist/index.js`` (or similar) sits under."""
    directory = adapter_entry.parent
    for _ in range(_MAX_PACKAGE_JSON_SEARCH_DEPTH):
        candidate = directory / "package.json"
        if candidate.is_file():
            return candidate
        parent = directory.parent
        if parent == directory:
            break
        directory = parent
    raise InstallUnresolvable(f"no package.json found above adapter entry {adapter_entry}.")


# Mirrors the installed @openai/codex package's own bin/codex.js platform
# selection table exactly (see this module's own docstring) -- the only way
# to know which vendored platform package holds the real spawned codex
# binary for this machine.
_CODEX_TARGET_TRIPLE_BY_PLATFORM = {
    ("Linux", "x86_64"): "x86_64-unknown-linux-musl",
    ("Linux", "aarch64"): "aarch64-unknown-linux-musl",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Windows", "AMD64"): "x86_64-pc-windows-msvc",
    ("Windows", "ARM64"): "aarch64-pc-windows-msvc",
}

_CODEX_PLATFORM_PACKAGE = {
    "x86_64-unknown-linux-musl": "@openai/codex-linux-x64",
    "aarch64-unknown-linux-musl": "@openai/codex-linux-arm64",
    "x86_64-apple-darwin": "@openai/codex-darwin-x64",
    "aarch64-apple-darwin": "@openai/codex-darwin-arm64",
    "x86_64-pc-windows-msvc": "@openai/codex-win32-x64",
    "aarch64-pc-windows-msvc": "@openai/codex-win32-arm64",
}


def _resolve_node_package_dir(package_dir: Path, name: str, probe: str) -> Path | None:
    """Find a dependency directory the way Node resolves it, identified by
    a file ``probe`` it must actually contain.

    Nested first, then each enclosing ``node_modules``. Only looking
    nested is only correct for a bundled install: npm hoists shared
    dependencies to the top of the tree by default, which is exactly how
    the client installs these engines -- `@openai/codex` sits beside the
    adapter, not under it -- so a nested-only lookup refused to launch the
    engine the user had configured (D-T035-02).

    Walks upward rather than guessing a depth, and returns ``None`` rather
    than a path that does not exist.
    """
    nested = package_dir / "node_modules" / Path(name)
    if (nested / probe).is_file():
        return nested
    for parent in package_dir.parents:
        if parent.name != "node_modules":
            continue
        candidate = parent / Path(name)
        if (candidate / probe).is_file():
            return candidate
    return None


def _build_codex_install(node_binary: Path, adapter_entry: Path) -> codex_install.CodexAcpInstall:
    package_json = _find_package_json(adapter_entry)
    package_data = _read_package_json(package_json)
    package_dir = package_json.parent

    codex_package_dir = _resolve_node_package_dir(package_dir, "@openai/codex", "package.json")
    codex_package_json = codex_package_dir / "package.json" if codex_package_dir is not None else None
    if codex_package_json is None:
        raise InstallUnresolvable(
            f"no @openai/codex package resolvable from {package_dir} "
            "(looked inside the adapter and in each enclosing node_modules)."
        )
    codex_package_data = _read_package_json(codex_package_json)
    bundled_codex_version = codex_package_data.get("version")
    if not isinstance(bundled_codex_version, str) or not bundled_codex_version:
        raise InstallUnresolvable(f"{codex_package_json} has no usable \"version\" field.")

    triple_key = (platform.system(), platform.machine())
    target_triple = _CODEX_TARGET_TRIPLE_BY_PLATFORM.get(triple_key)
    if target_triple is None:
        raise InstallUnresolvable(f"unsupported platform {triple_key!r} for bundled codex binary discovery.")
    platform_package_name = _CODEX_PLATFORM_PACKAGE[target_triple]
    binary_name = "codex.exe" if platform.system() == "Windows" else "codex"
    # Resolved the same way as the package above: the platform binary is
    # hoisted alongside it in a normal npm install, not nested under the
    # adapter.
    vendor_relative = f"vendor/{target_triple}/bin/{binary_name}"
    platform_package_dir = _resolve_node_package_dir(package_dir, platform_package_name, vendor_relative)
    bundled_codex_binary = platform_package_dir / vendor_relative if platform_package_dir is not None else None
    if bundled_codex_binary is None:
        raise InstallUnresolvable(
            f"no {platform_package_name} codex binary resolvable from {package_dir} "
            f"(expected vendor/{target_triple}/bin/{binary_name})."
        )

    return codex_install.CodexAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version=str(package_data.get("version")),
        package_json_sha256=_hash_file(package_json),
        adapter_entry_sha256=_hash_file(adapter_entry),
        bundled_codex_version=bundled_codex_version,
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_hash_file(bundled_codex_binary),
    )


def _build_claude_install(node_binary: Path, adapter_entry: Path) -> claude_install.ClaudeAcpInstall:
    package_json = _find_package_json(adapter_entry)
    package_data = _read_package_json(package_json)
    return claude_install.ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version=str(package_data.get("version")),
        package_json_sha256=_hash_file(package_json),
        adapter_entry_sha256=_hash_file(adapter_entry),
    )


_BUILDERS = {
    "codex-acp": _build_codex_install,
    "claude-agent-acp": _build_claude_install,
}
_VERIFIERS = {
    "codex-acp": codex_install.verify_install,
    "claude-agent-acp": claude_install.verify_install,
}


def resolve_verified_install(record, environ: dict[str, str] | None = None):
    """Build and re-verify the pinned install for ``record``'s own engine,
    derived entirely from ``record.launch_command`` -- never from a
    caller-supplied install.

    Raises :class:`InstallUnresolvable` when ``record.launch_command`` is
    not install-shaped at all (wrong arity, unknown engine, no
    ``package.json``/bundled binary found -- always non-fatal to both
    callers). Raises :class:`~.portable.errors.PortableError` (from
    ``codex_install``/``claude_install``'s own ``verify_install``) when a
    real candidate install *was* built but failed integrity/trust
    verification (hash drift, wrong entry point, untrusted location) --
    fatal to a caller launching a real engine; a refresh caller still
    treats this as optional-enrichment-skipped, never as its own failure.
    """
    if record.launch_command is None or len(record.launch_command) != 2:
        raise InstallUnresolvable(
            "record.launch_command is not the exact (node_binary, adapter_entry) shape a real "
            "pinned install's own launch_command_for() always produces."
        )
    builder = _BUILDERS.get(record.engine)
    verifier = _VERIFIERS.get(record.engine)
    if builder is None or verifier is None:
        raise InstallUnresolvable(f"no install resolution is defined for engine {record.engine!r}.")
    node_binary_text, adapter_entry_text = record.launch_command
    if not isinstance(node_binary_text, str) or not isinstance(adapter_entry_text, str):
        raise InstallUnresolvable("record.launch_command elements must be strings.")
    install = builder(Path(node_binary_text), Path(adapter_entry_text))
    return verifier(install, environ)
