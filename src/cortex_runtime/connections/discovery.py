"""Engine discovery: find Cortex's own engine installs (kept current by
``connections.engine_update``); a client's (e.g. Zed's) copy is never used.

D-T035-02: an engine is no longer something Cortex installs, copies, or
pins for itself -- it is whatever the user already configured in their
editor. This module only ever *reads* the client's own installed npm
package for the two engines this project implements adapters for
(``codex-acp``, ``claude-agent-acp``); it never downloads, writes, or
modifies anything. A launch command is only ever built from a package's
own declared ``package.json`` ``"bin"`` entry -- never a guessed filename
-- and an engine whose package/bin/node binary cannot actually be found on
disk is reported as absent, never as a guess.

The credential directory a discovered engine reports
(``native_credential_dir``) is always the user's own native directory
(``~/.codex``, ``~/.claude``), never a Cortex-owned copy (see
``connections.model.EngineNativeDir(owned=False)`` and
``connections.onboarding.create_referenced_connection``). Accepted cost of
that reference (D-T035-02, stated rather than hidden): the client's own
engine process and Cortex's own engine process can now both read/refresh
the same on-disk credential directory, so a token refresh can race between
the two; the Cortex-owned copy this replaces existed for exactly that
reason, and one place of truth was judged worth the trade.
"""
from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil

# Registry package name -> engine id this project has an adapter for.
_PACKAGE_NAME = {
    "codex-acp": "@agentclientprotocol/codex-acp",
    "claude-agent-acp": "@agentclientprotocol/claude-agent-acp",
}

# The user's own native credential directory per engine -- a fixed,
# well-known convention (never scanned or guessed).
_NATIVE_CREDENTIAL_SUBDIR = {
    "codex-acp": ".codex",
    "claude-agent-acp": ".claude",
}

SUPPORTED_ENGINES: tuple[str, ...] = tuple(_PACKAGE_NAME)

# The adapter versions Cortex has actually qualified for launch. codex-acp
# is byte-verified twice over -- ``engine.process
# ._prepare_codex_manual_review_adapter`` patches one exact bundle shape,
# and ``engine.instruction_delivery_registry`` qualifies one exact version
# -- so a newer install (``connections.engine_update`` installs one beside
# the current one) is not launchable until it has been qualified too.
# Preferring the newest install unconditionally took Codex offline
# entirely once 1.13.0 was installed: every codex session and every
# catalog refresh failed closed with
# ``codex_manual_review_adapter_unsupported``, and the engine's newly
# published models could never appear in the model catalog (live,
# 2026-09-22). ``claude-agent-acp`` has a version-independent delivery
# registration and no byte-level patch, so any installed version launches.
QUALIFIED_ADAPTER_VERSIONS: dict[str, tuple[str, ...] | None] = {
    "codex-acp": ("1.12.0", "1.13.0"),
    "claude-agent-acp": None,
}


def is_qualified(engine: str, version: str) -> bool:
    """True iff Cortex has qualified ``version`` of ``engine`` for launch.

    ``None`` in :data:`QUALIFIED_ADAPTER_VERSIONS` means every version; an
    engine with no entry at all is never qualified.
    """
    allowed = QUALIFIED_ADAPTER_VERSIONS.get(engine, ())
    return allowed is None or version in allowed


def is_pinned(engine: str, version: str) -> bool:
    """True iff ``version`` is one of the *named* versions an engine is
    qualified for -- an engine qualified for every version (``None``) has
    no pinned version to protect, so its installs are pruned normally."""
    allowed = QUALIFIED_ADAPTER_VERSIONS.get(engine)
    return isinstance(allowed, tuple) and version in allowed


@dataclass(frozen=True)
class EngineDiscovery:
    """One engine actually found installed for the client, ready to launch."""

    engine: str
    version: str
    launch_command: tuple[str, str]
    native_credential_dir: str


def _home(environ: dict[str, str] | None) -> Path:
    env = os.environ if environ is None else environ
    home = env.get("HOME")
    return Path(home) if home else Path.home()


def _node_binary(environ: dict[str, str] | None) -> str | None:
    """The node binary to launch a discovered engine with, canonical.

    Resolved through every symlink, because the launch trust check
    (``engine.codex_install``/``claude_install``) accepts only an
    already-canonical absolute path -- and the node on a user's PATH is
    very often a symlink (``~/.local/bin/node`` -> a version manager's
    real binary). Discovery that reported the symlink produced a
    connection that could never launch at all, refused with
    ``engine_install_untrusted_location`` naming the node binary.
    """
    env = os.environ if environ is None else environ
    override = env.get("CORTEX_NODE_BINARY")
    found = override or shutil.which("node", path=env.get("PATH"))
    if not found:
        return None
    try:
        return str(Path(found).resolve())
    except OSError:
        return found


def owned_root(environ: dict[str, str] | None = None) -> Path:
    """Cortex's own engine installs: ``<root>/<engine>/<version>/node_modules/...``,
    kept current by ``connections.engine_update``."""
    env = os.environ if environ is None else environ
    data = env.get("XDG_DATA_HOME")
    return (Path(data) if data else _home(environ) / ".local" / "share") / "cortex" / "engines"


def version_key(text: str) -> tuple[int, ...]:
    """``"0.80.0"`` -> ``(0, 80, 0)``; anything else sorts first."""
    try:
        return tuple(int(part) for part in text.split("."))
    except ValueError:
        return ()


def _declared_bin(package_json_path: Path) -> tuple[str, str] | None:
    """Return ``(version, bin_target)`` as declared by ``package.json`` itself.

    ``None`` for anything unreadable, unparsable, or without exactly one
    unambiguous ``"bin"`` target -- never a guessed filename.
    """
    try:
        data = json.loads(package_json_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    version = data.get("version")
    if not isinstance(version, str) or not version:
        return None
    bin_field = data.get("bin")
    if isinstance(bin_field, str):
        bin_target = bin_field
    elif isinstance(bin_field, dict) and len(bin_field) == 1:
        bin_target = next(iter(bin_field.values()))
    else:
        return None
    if not isinstance(bin_target, str) or not bin_target:
        return None
    return version, bin_target


def discover_engine(
    engine: str,
    *,
    registry_root: Path | None = None,
    node_binary: str | None = None,
    environ: dict[str, str] | None = None,
) -> EngineDiscovery | None:
    """Report ``engine`` iff its package is actually installed under the
    client's registry root, with a real, existing, declared ``bin`` entry
    point and a usable ``node`` binary. ``None`` otherwise -- never raises.
    """
    package_name = _PACKAGE_NAME.get(engine)
    if package_name is None:
        return None
    node = node_binary if node_binary is not None else _node_binary(environ)
    if not node:
        return None
    if registry_root is not None:
        return _discover_in(_subdirs(registry_root), engine, package_name, node, environ)
    # Only Cortex's own installs; a client's (e.g. Zed's) copy is never
    # used. A qualified version always wins over a newer unqualified one
    # (see :func:`is_qualified`); among equals, newest first. With no
    # qualified install at all the newest is still reported, so the launch
    # refusal names the real unqualified version instead of reporting the
    # engine as absent.
    owned = sorted(
        _subdirs(owned_root(environ) / engine),
        key=lambda p: (is_qualified(engine, p.name), version_key(p.name)),
        reverse=True,
    )
    return _discover_in(owned, engine, package_name, node, environ)

def _subdirs(root: Path) -> list[Path]:
    try:
        return sorted(p for p in root.iterdir() if p.is_dir() and not p.name.startswith("."))
    except OSError:
        return []


def _discover_in(entries: list[Path], engine: str, package_name: str, node: str,
                 environ: dict[str, str] | None) -> EngineDiscovery | None:
    for entry in entries:
        package_json = entry / "node_modules" / package_name / "package.json"
        try:
            if not package_json.is_file():
                continue
        except OSError:
            continue
        declared = _declared_bin(package_json)
        if declared is None:
            continue
        version, bin_target = declared
        package_dir = package_json.parent
        try:
            package_dir_resolved = package_dir.resolve()
            bin_path = (package_dir / bin_target).resolve()
        except OSError:
            continue
        try:
            bin_path.relative_to(package_dir_resolved)
        except ValueError:
            continue  # declared bin escapes its own package dir; never trust it
        try:
            if not bin_path.is_file():
                continue
        except OSError:
            continue
        native_dir = _home(environ) / _NATIVE_CREDENTIAL_SUBDIR[engine]
        return EngineDiscovery(
            engine=engine,
            version=version,
            launch_command=(node, str(bin_path)),
            native_credential_dir=str(native_dir),
        )
    return None


def discover_all(
    *,
    registry_root: Path | None = None,
    node_binary: str | None = None,
    environ: dict[str, str] | None = None,
) -> dict[str, EngineDiscovery]:
    """Discover every supported engine the client has installed.

    Never raises: a broken/unreadable registry tree, a missing package, or
    a missing ``node`` binary simply yields fewer (or zero) entries.
    """
    found: dict[str, EngineDiscovery] = {}
    for engine in SUPPORTED_ENGINES:
        try:
            result = discover_engine(engine, registry_root=registry_root, node_binary=node_binary, environ=environ)
        except OSError:
            result = None
        if result is not None:
            found[engine] = result
    return found
