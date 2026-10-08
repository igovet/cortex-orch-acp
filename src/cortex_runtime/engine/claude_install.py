"""Pinned claude-agent-acp install record: identity, integrity, and trust checks.

Mirrors ``engine.codex_install`` (T013) for the second, non-Codex engine
(T026). A :class:`ClaudeAcpInstall` is a frozen description of exactly one
on-disk claude-agent-acp install (Node entry point and the
``@agentclientprotocol/claude-agent-acp`` adapter bundle it names) *as
discovered and recorded by a human operator following the manual recipe in
``docs/acp/claude-agent-acp-adapter.md``* -- this module never discovers or
installs anything itself; connections.engine_update owns installation.

The current fallback release is 0.87.0 (upstream Claude Agent SDK 0.3.287,
ACP SDK 1.7.0). Cortex's side-by-side SDK 0.3.293 refresh exposes Haiku 5.5;
the original install is not overwritten. package.json still declares
dist/index.js as its bin.
The adapter imports the SDK in Node; the SDK in turn starts Claude Code
through its own native transport. This record pins the adapter launch
entry, not a separately selected Claude Code executable. There is no
bundled_codex_binary-equivalent field, and build_claude_launch does not
inject CODEX_PATH. The native account uses CLAUDE_CONFIG_DIR, set by
connections.environment for this engine. Installation is performed by
connections.engine_update, not by this integrity-record module.

:func:`verify_install` is the only place that turns a caller-held
``ClaudeAcpInstall`` back into something ``engine.process.EngineProcess``
may actually launch: it re-hashes every pinned file fresh (never trusts a
previously computed digest without re-checking it), re-reads
``package.json``'s own ``"version"`` field, and rejects any path that is not
a plain, absolute, non-symlinked file, or that resolves under a known
untrusted install location (a Zed/codeg *registry* copy, never pinned by
this project, or either app's own private data directory) -- the identical
untrusted-root list ``codex_install`` uses, since both engines are observed
installed under the same Zed registry layout. A caller must never construct
a launch command from an unverified install.
"""
from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path

from ..portable.errors import PortableError, PortableErrorCategory

_HASH_CHUNK_SIZE = 1024 * 1024


def _validation_error(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=False, field=field,
    )


@dataclass(frozen=True)
class ClaudeAcpInstall:
    """One operator-recorded, pinned claude-agent-acp install.

    Every path field is required to be an absolute string; format/shape is
    checked here at construction, but *trust* (integrity + location) is only
    ever established by :func:`verify_install`, never by constructing this
    dataclass.
    """

    node_binary: str
    adapter_entry: str
    package_json: str
    package_version: str
    package_json_sha256: str
    adapter_entry_sha256: str

    def __post_init__(self) -> None:
        for name in ("node_binary", "adapter_entry", "package_json"):
            value = getattr(self, name)
            if not isinstance(value, str) or not value.startswith("/"):
                raise _validation_error(
                    "invalid_field", f"'{name}' must be an absolute path string.", field=name,
                )
        for name in ("package_version", "package_json_sha256", "adapter_entry_sha256"):
            value = getattr(self, name)
            if not isinstance(value, str) or not value:
                raise _validation_error(
                    "invalid_field", f"'{name}' must be a non-empty string.", field=name,
                )
        for name in ("package_json_sha256", "adapter_entry_sha256"):
            value = getattr(self, name)
            if len(value) != 64 or any(ch not in "0123456789abcdef" for ch in value.lower()):
                raise _validation_error(
                    "invalid_field", f"'{name}' must be a 64-character lowercase hex sha256 digest.", field=name,
                )


# Identical to codex_install's own list (see that module's comment for why
# the client's own agent registry is no longer among them: D-T035-02 makes
# the engines the user configured in their client the engines Cortex runs,
# and those live exactly there). Relative to ``$HOME`` (the explicit
# ``environ``'s ``HOME``, never a hardcoded one).
_UNTRUSTED_HOME_RELATIVE_ROOTS = (
    (".local", "share", "app.codeg"),
    (".config", "app.codeg"),
)


def _resolve_home(environ: dict[str, str] | None) -> Path:
    mapping = os.environ if environ is None else environ
    home = mapping.get("HOME")
    if not home:
        raise _validation_error(
            "engine_install_untrusted_location",
            "HOME is not set in the explicit environ used to verify this install; an "
            "untrusted-location check cannot be performed without it.",
        )
    home_path = Path(home)
    if not home_path.is_absolute():
        raise _validation_error(
            "engine_install_untrusted_location",
            "HOME in the explicit environ used to verify this install must be an absolute path.",
        )
    # Same fix as codex_install._resolve_home (F2, T013 review): resolve
    # HOME itself (strict) before building the untrusted-root list, so a
    # symlinked HOME cannot silently escape the comparison.
    return home_path.resolve()


def _untrusted_roots(environ: dict[str, str] | None) -> tuple[Path, ...]:
    home = _resolve_home(environ)
    return tuple(home.joinpath(*parts) for parts in _UNTRUSTED_HOME_RELATIVE_ROOTS)


def _reject_untrusted_or_impure_path(path_text: str, field: str, environ: dict[str, str] | None) -> Path:
    path = Path(path_text)
    if not path.is_absolute():
        raise _validation_error(
            "engine_install_untrusted_location",
            f"'{field}' must be an absolute path.", field=field,
        )
    resolved = path.resolve()
    if resolved != path:
        raise _validation_error(
            "engine_install_untrusted_location",
            f"'{field}' contains a relative or symlinked path component; only a plain, "
            "already-canonical absolute path is trusted.", field=field,
        )
    for root in _untrusted_roots(environ):
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        raise _validation_error(
            "engine_install_untrusted_location",
            f"'{field}' resolves under {root}, a location this project never pins an "
            "install from (a client-managed registry checkout or another app's private "
            "data directory); record a copy under an operator-controlled path instead.",
            field=field,
        )
    return resolved


def _hash_file(path: Path) -> str:
    hasher = hashlib.sha256()
    try:
        with open(path, "rb") as handle:
            while True:
                chunk = handle.read(_HASH_CHUNK_SIZE)
                if not chunk:
                    break
                hasher.update(chunk)
    except OSError as error:
        raise _validation_error(
            "engine_install_drifted", f"Could not read {path} to verify its pinned hash: {error}.",
        ) from error
    return hasher.hexdigest()


def verify_install(install: ClaudeAcpInstall, environ: dict[str, str] | None = None) -> ClaudeAcpInstall:
    """Re-verify every pinned file fresh; return ``install`` unchanged on success.

    Raises ``PortableError(code="engine_install_untrusted_location")`` for any
    path failing the trust check above, or
    ``PortableError(code="engine_install_drifted")`` for any hash/version
    mismatch or unreadable file. Never partially trusts an install: the
    first failing check raises immediately.
    """
    if not isinstance(install, ClaudeAcpInstall):
        raise _validation_error("invalid_field", "install must be a ClaudeAcpInstall instance.", field="install")

    node_binary = _reject_untrusted_or_impure_path(install.node_binary, "node_binary", environ)
    adapter_entry = _reject_untrusted_or_impure_path(install.adapter_entry, "adapter_entry", environ)
    package_json = _reject_untrusted_or_impure_path(install.package_json, "package_json", environ)

    if not node_binary.is_file():
        raise _validation_error("engine_install_drifted", f"node_binary {node_binary} is not a regular file.", field="node_binary")
    if not os.access(node_binary, os.X_OK):
        raise _validation_error("engine_install_drifted", f"node_binary {node_binary} is not executable.", field="node_binary")

    actual_adapter_hash = _hash_file(adapter_entry)
    if actual_adapter_hash != install.adapter_entry_sha256:
        raise _validation_error(
            "engine_install_drifted",
            f"adapter_entry {adapter_entry} sha256 {actual_adapter_hash} does not match the pinned "
            f"{install.adapter_entry_sha256}; the installed adapter has changed since it was pinned.",
            field="adapter_entry_sha256",
        )

    actual_package_json_hash = _hash_file(package_json)
    if actual_package_json_hash != install.package_json_sha256:
        raise _validation_error(
            "engine_install_drifted",
            f"package_json {package_json} sha256 {actual_package_json_hash} does not match the pinned "
            f"{install.package_json_sha256}; package.json has changed since it was pinned.",
            field="package_json_sha256",
        )

    try:
        package_data = json.loads(package_json.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise _validation_error(
            "engine_install_drifted", f"package_json {package_json} could not be parsed as JSON: {error}.",
            field="package_json",
        ) from error
    observed_version = package_data.get("version") if isinstance(package_data, dict) else None
    if observed_version != install.package_version:
        raise _validation_error(
            "engine_install_drifted",
            f"package.json version {observed_version!r} does not match the pinned "
            f"{install.package_version!r}; the installed claude-agent-acp release has changed since it "
            "was pinned.",
            field="package_version",
        )

    # `adapter_entry` must be the exact file package.json's own "bin" field
    # names as the launchable entry point (`dist/index.js`, per this
    # module's docstring) -- never merely *some* file under the package
    # (e.g. `dist/acp-agent.js`, a library module `dist/index.js` imports
    # that never calls its own `runAcp()` and so, launched directly, exits
    # immediately without ever reading stdin: node prints nothing, binds no
    # ACP handlers, and the very first `initialize` call sees its transport
    # closed). Checked against every declared bin entry (not just one key)
    # since a caller is never required to know the package's exact bin
    # key up front.
    bin_field = package_data.get("bin") if isinstance(package_data, dict) else None
    bin_targets = bin_field.values() if isinstance(bin_field, dict) else ()
    package_dir = package_json.parent
    resolved_bin_paths = {package_dir.joinpath(target).resolve() for target in bin_targets if isinstance(target, str)}
    if resolved_bin_paths and adapter_entry not in resolved_bin_paths:
        raise _validation_error(
            "engine_install_drifted",
            f"adapter_entry {adapter_entry} is not the launchable entry point package.json's own "
            f"\"bin\" field names ({sorted(str(p) for p in resolved_bin_paths)}); launching any other "
            "file under this package is never trusted even if its own pinned hash matches.",
            field="adapter_entry",
        )

    return install


def launch_command_for(install: ClaudeAcpInstall) -> tuple[str, str]:
    """The exact ``(node_binary, adapter_entry)`` command for a *verified* install.

    Never called on an unverified install -- callers must pass the return
    value of :func:`verify_install` here, never a caller-constructed
    ``ClaudeAcpInstall`` directly.
    """
    if not isinstance(install, ClaudeAcpInstall):
        raise _validation_error("invalid_field", "install must be a ClaudeAcpInstall instance.", field="install")
    return (install.node_binary, install.adapter_entry)
