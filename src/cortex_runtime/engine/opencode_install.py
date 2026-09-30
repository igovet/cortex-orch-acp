"""Verified OpenCode v2 install: one standalone ``opencode`` binary, checked before launch.

OpenCode is not an npm package: its installer writes a single executable
(by default ``~/.opencode/bin/opencode``) that serves ACP through
``opencode acp``. So an install here is that binary and the version it
reports, never a ``package.json``.

:func:`verify_install` is the only way a :class:`OpenCodeInstall` becomes
something ``engine.process.EngineProcess`` may launch. It requires:

- an absolute, already-canonical path (no symlink component) to a regular,
  executable file, outside the untrusted roots ``claude_install`` refuses;
- a file owned by the current user or root and writable by nobody else, so
  another account cannot swap what Cortex runs;
- a version Cortex qualified (``connections.discovery
  .QUALIFIED_ADAPTER_VERSIONS["opencode"]``).

The version comes from :func:`probe_version`, which runs ``<binary>
--version`` inside a network-less, read-only sandbox with a throwaway HOME
(``orchestration.sandbox.verification_command``) -- never in the owner's own
environment, where a run could write OpenCode state or check for updates.
The engine's own ``initialize`` ``agentInfo.version`` is checked again at
launch (``engine.session_manager.EngineSessionManager.negotiate``), so a
binary replaced between the two is still refused before any session.
"""
from __future__ import annotations

import os
import re
import shutil
import stat
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from ..portable.errors import PortableError, PortableErrorCategory

ENGINE = "opencode"
#: The ACP entry point of the binary: ``opencode acp``.
ACP_SUBCOMMAND = "acp"
PROBE_TIMEOUT_SECONDS = 20.0
_VERSION = re.compile(r"\bv?(\d+\.\d+\.\d+)\b")


def _validation_error(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=False, field=field,
    )


def qualified_versions() -> tuple[str, ...]:
    from ..connections.discovery import QUALIFIED_ADAPTER_VERSIONS

    versions = QUALIFIED_ADAPTER_VERSIONS.get(ENGINE)
    return tuple(versions) if isinstance(versions, tuple) else ()


@dataclass(frozen=True)
class OpenCodeInstall:
    """One OpenCode binary and the version it reported."""

    binary: str
    version: str

    def __post_init__(self) -> None:
        if not isinstance(self.binary, str) or not self.binary.startswith("/"):
            raise _validation_error("invalid_field", "'binary' must be an absolute path string.", field="binary")
        if not isinstance(self.version, str) or not _VERSION.fullmatch(self.version):
            raise _validation_error("invalid_field", "'version' must be an x.y.z version string.", field="version")


def trusted_binary(binary: str, environ: dict[str, str] | None = None) -> Path:
    """``binary`` as a trusted path, or ``engine_install_untrusted_location``/
    ``engine_install_drifted``. Never follows or accepts a symlink."""
    from .claude_install import _untrusted_roots

    path = Path(binary)
    if not path.is_absolute():
        raise _validation_error("engine_install_untrusted_location", "The OpenCode binary path must be absolute.",
                                field="binary")
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        raise _validation_error("engine_install_drifted", f"The OpenCode binary {path} does not exist.",
                                field="binary") from None
    if resolved != path:
        raise _validation_error(
            "engine_install_untrusted_location",
            f"{path} contains a symlinked or relative component; record the real binary path "
            f"({resolved}) instead.", field="binary",
        )
    for root in _untrusted_roots(environ):
        if root == resolved or root in resolved.parents:
            raise _validation_error(
                "engine_install_untrusted_location", f"{path} is under {root}, which Cortex never launches from.",
                field="binary",
            )
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or not os.access(path, os.X_OK):
        raise _validation_error("engine_install_drifted", f"{path} is not an executable regular file.", field="binary")
    if info.st_uid not in (os.getuid(), 0) or info.st_mode & 0o022:
        raise _validation_error(
            "engine_install_untrusted_location",
            f"{path} must be owned by you or root and writable by nobody else "
            "(another account could replace what Cortex runs).", field="binary",
        )
    return path


def probe_version(binary: str, *, timeout: float = PROBE_TIMEOUT_SECONDS) -> str | None:
    """The x.y.z version ``<binary> --version`` prints inside a network-less,
    read-only sandbox with a throwaway HOME, or ``None`` when it cannot be
    established (no sandbox, a failure, a timeout, no version in the output)."""
    from ..contracts import StoreError
    from ..orchestration.sandbox import verification_command

    if shutil.which("bwrap") is None:
        return None
    with tempfile.TemporaryDirectory(prefix="cortex-opencode-version-") as directory:
        root = Path(directory) / "root"
        scratch = Path(directory) / "scratch"
        root.mkdir()
        scratch.mkdir()
        try:
            command = verification_command(root, scratch, [binary, "--version"], system_root=True)
            done = subprocess.run(command, capture_output=True, timeout=timeout, env={"PATH": "/usr/bin:/bin"})
        except (OSError, subprocess.SubprocessError, StoreError):
            return None
    if done.returncode != 0:
        return None
    match = _VERSION.search(done.stdout.decode("utf-8", "replace"))
    return match.group(1) if match else None


def build_install(binary: str, environ: dict[str, str] | None = None) -> OpenCodeInstall:
    """Check ``binary`` and read its version; the result still needs
    :func:`verify_install` before launch."""
    path = trusted_binary(binary, environ)
    version = probe_version(str(path))
    if version is None:
        raise _validation_error(
            "engine_install_drifted",
            f"Could not read the version of {path} (`--version` in a sandbox); OpenCode is not launched "
            "without a verified version.", field="version",
        )
    return OpenCodeInstall(binary=str(path), version=version)


def verify_install(install: OpenCodeInstall, environ: dict[str, str] | None = None) -> OpenCodeInstall:
    """Re-check the binary's trust and that its version is qualified; return
    ``install`` unchanged, or raise."""
    if not isinstance(install, OpenCodeInstall):
        raise _validation_error("invalid_field", "install must be an OpenCodeInstall instance.", field="install")
    trusted_binary(install.binary, environ)
    from ..connections.discovery import is_qualified

    if not is_qualified(ENGINE, install.version):
        raise _validation_error(
            "opencode_version_unqualified",
            f"OpenCode {install.version} is not a version Cortex qualified "
            f"({', '.join(qualified_versions()) or 'none'}); install a qualified version or wait for "
            "Cortex to qualify this one.", field="version",
        )
    return install


def launch_command_for(install: OpenCodeInstall) -> tuple[str, str]:
    """``(binary, "acp")`` for a *verified* install."""
    if not isinstance(install, OpenCodeInstall):
        raise _validation_error("invalid_field", "install must be an OpenCodeInstall instance.", field="install")
    return (install.binary, ACP_SUBCOMMAND)
