"""Pinned codex-acp install record: identity, integrity, and trust checks.

A :class:`CodexAcpInstall` is a frozen description of exactly one on-disk
codex-acp install (Node entry point, ``@openai/codex`` adapter bundle, and
the platform ``codex`` binary it vendors) *as discovered and recorded by a
human operator following the manual recipe in
``docs/acp/codex-acp-adapter.md``* -- this module never discovers or
installs anything itself (see T031 for any future installer).

:func:`verify_install` is the only place that turns a caller-held
``CodexAcpInstall`` back into something ``engine.process.EngineProcess`` may
actually launch: it re-hashes every pinned file fresh (never trusts a
previously computed digest without re-checking it), re-reads
``package.json``'s own ``"version"`` field, and rejects any path that is not
a plain, absolute, non-symlinked file, or that resolves under a known
untrusted install location (a Zed/codeg *registry* copy, never pinned by
this project, or either app's own private data directory). A caller must
never construct a launch command from an unverified install.
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
class CodexAcpInstall:
    """One operator-recorded, pinned codex-acp install.

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
    bundled_codex_version: str
    bundled_codex_binary: str
    bundled_codex_binary_sha256: str

    def __post_init__(self) -> None:
        for name in ("node_binary", "adapter_entry", "package_json", "bundled_codex_binary"):
            value = getattr(self, name)
            if not isinstance(value, str) or not value.startswith("/"):
                raise _validation_error(
                    "invalid_field", f"'{name}' must be an absolute path string.", field=name,
                )
        for name in (
            "package_version", "package_json_sha256", "adapter_entry_sha256",
            "bundled_codex_version", "bundled_codex_binary_sha256",
        ):
            value = getattr(self, name)
            if not isinstance(value, str) or not value:
                raise _validation_error(
                    "invalid_field", f"'{name}' must be a non-empty string.", field=name,
                )
        for name in ("package_json_sha256", "adapter_entry_sha256", "bundled_codex_binary_sha256"):
            value = getattr(self, name)
            if len(value) != 64 or any(ch not in "0123456789abcdef" for ch in value.lower()):
                raise _validation_error(
                    "invalid_field", f"'{name}' must be a 64-character lowercase hex sha256 digest.", field=name,
                )


# Locations that are never trusted as the source of a pinned install file,
# even if the recorded hash happens to match a file found there: a Zed
# registry checkout is refreshed/replaced by Zed itself outside this
# project's control, and an app's own private data directory is not a
# location an operator deliberately pinned. Every entry is relative to
# ``$HOME`` (the explicit ``environ``'s ``HOME``, never a hardcoded one).
_UNTRUSTED_HOME_RELATIVE_ROOTS = (
    (".local", "share", "zed", "external_agents"),
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
    # F2 (T013 review): resolve HOME itself (strict -- following every
    # symlink component) before building the untrusted-root list below, so
    # an untrusted-location comparison is always made in resolved-path
    # space on both sides. Comparing a resolved install path against roots
    # built from a *symlinked* HOME would never match (the two path forms
    # disagree on the same real directory), silently letting an install
    # under e.g. HOME/.local/share/zed/external_agents through when HOME
    # itself is a symlink.
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
        # A different resolved form means a symlink, "..", or non-canonical
        # component was involved somewhere in the path -- never trusted,
        # regardless of what the resolved target actually contains.
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


def verify_install(install: CodexAcpInstall, environ: dict[str, str] | None = None) -> CodexAcpInstall:
    """Re-verify every pinned file fresh; return ``install`` unchanged on success.

    Raises ``PortableError(code="engine_install_untrusted_location")`` for any
    path failing the trust check above, or
    ``PortableError(code="engine_install_drifted")`` for any hash/version
    mismatch or unreadable file. Never partially trusts an install: the
    first failing check raises immediately.
    """
    if not isinstance(install, CodexAcpInstall):
        raise _validation_error("invalid_field", "install must be a CodexAcpInstall instance.", field="install")

    node_binary = _reject_untrusted_or_impure_path(install.node_binary, "node_binary", environ)
    adapter_entry = _reject_untrusted_or_impure_path(install.adapter_entry, "adapter_entry", environ)
    package_json = _reject_untrusted_or_impure_path(install.package_json, "package_json", environ)
    bundled_codex_binary = _reject_untrusted_or_impure_path(
        install.bundled_codex_binary, "bundled_codex_binary", environ,
    )

    if not node_binary.is_file():
        raise _validation_error("engine_install_drifted", f"node_binary {node_binary} is not a regular file.", field="node_binary")
    if not os.access(node_binary, os.X_OK):
        raise _validation_error("engine_install_drifted", f"node_binary {node_binary} is not executable.", field="node_binary")
    if not bundled_codex_binary.is_file():
        raise _validation_error(
            "engine_install_drifted", f"bundled_codex_binary {bundled_codex_binary} is not a regular file.",
            field="bundled_codex_binary",
        )
    if not os.access(bundled_codex_binary, os.X_OK):
        raise _validation_error(
            "engine_install_drifted", f"bundled_codex_binary {bundled_codex_binary} is not executable.",
            field="bundled_codex_binary",
        )

    actual_adapter_hash = _hash_file(adapter_entry)
    if actual_adapter_hash != install.adapter_entry_sha256:
        raise _validation_error(
            "engine_install_drifted",
            f"adapter_entry {adapter_entry} sha256 {actual_adapter_hash} does not match the pinned "
            f"{install.adapter_entry_sha256}; the installed adapter has changed since it was pinned.",
            field="adapter_entry_sha256",
        )

    actual_binary_hash = _hash_file(bundled_codex_binary)
    if actual_binary_hash != install.bundled_codex_binary_sha256:
        raise _validation_error(
            "engine_install_drifted",
            f"bundled_codex_binary {bundled_codex_binary} sha256 {actual_binary_hash} does not match the "
            f"pinned {install.bundled_codex_binary_sha256}; the bundled codex binary has changed since it "
            "was pinned.",
            field="bundled_codex_binary_sha256",
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
            f"{install.package_version!r}; the installed codex-acp release has changed since it was pinned.",
            field="package_version",
        )

    return install


def launch_command_for(install: CodexAcpInstall) -> tuple[str, str]:
    """The exact ``(node_binary, adapter_entry)`` command for a *verified* install.

    Never called on an unverified install -- callers must pass the return
    value of :func:`verify_install` here, never a caller-constructed
    ``CodexAcpInstall`` directly.
    """
    if not isinstance(install, CodexAcpInstall):
        raise _validation_error("invalid_field", "install must be a CodexAcpInstall instance.", field="install")
    return (install.node_binary, install.adapter_entry)
