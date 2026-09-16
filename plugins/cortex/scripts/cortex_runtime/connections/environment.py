"""Least-privilege, per-connection process environments.

``build_process_env`` guarantees exactly the following and nothing else:

1. The result contains only the fixed allowlisted base-env names (``PATH``,
   ``HOME``, ``LANG``, ``LC_ALL``, ``LC_CTYPE``, ``TZ``, ``TMPDIR``,
   ``USER``, ``LOGNAME``) copied verbatim from ``base_env`` when present
   (exact, case-sensitive key match against that fixed set -- no other
   ``base_env`` key is ever copied, and there is no caller-supplied
   arbitrary-key passthrough of any kind).
2. ``PATH``, if present, has every empty or non-absolute entry dropped.
3. Exactly this connection's own isolation variable
   (``CODEX_HOME``/``CLAUDE_CONFIG_DIR``) is set to its id-derived
   directory, and -- only for ``api_key`` connections, and only after
   re-verifying the credential reference is this connection's own
   id-derived, non-colliding one -- exactly one secret variable is set from
   a value read fresh from that reference at call time.
4. If ``channel_credentials=(channel_id, secret, project_root)`` is passed,
   exactly ``CORTEX_CHANNEL_ID``/``CORTEX_CHANNEL_SECRET``/
   ``CORTEX_CHANNEL_PROJECT_ROOT`` are set from it (each format-validated);
   otherwise none of those three names are set. There is no other
   credential/secret channel and no arbitrary ``extra_env`` parameter: a
   case-insensitive credential-shaped denylist is still applied to every
   candidate value as defense in depth, but the primary guarantee is the
   fixed allowlist above, not the denylist.
5. ``mcp_allowlist`` is validated but never forwarded into the process
   environment: there is no wholesale MCP server/credential forwarding
   here.
"""
from __future__ import annotations

import re
from pathlib import Path

from ..portable.errors import PortableError, PortableErrorCategory
from . import registry
from .model import (
    AuthMode,
    EngineNativeDir,
    KEY_KIND_ENV_VAR,
    OsKeyring,
    SecretFile,
)

# Minimal, documented allowlist copied from the caller's base_env. HOME is
# included because many engine/tool invocations require it to resolve
# unrelated (non-credential) user paths; this is a pragmatic, deliberately
# unqualified inclusion -- it is not claimed to be fully sandboxed, only
# reviewed and bounded.
ALLOWLISTED_BASE_ENV_KEYS = (
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "USER",
    "LOGNAME",
)

# Defense in depth only (see guarantee 4 above): applied case-insensitively
# to every value this module is about to set, including the fixed allowlist
# names and the channel-credential names, so a future edit that widens the
# allowlist cannot silently start leaking a credential-shaped name.
DENYLIST_PATTERN = re.compile(
    r"(_API_KEY|_TOKEN|_SECRET|_PASSWORD)$"
    r"|^(ANTHROPIC_|OPENAI_|CODEX_|CLAUDE_|AWS_|GOOGLE_|AZURE_|GITHUB_|GH_)",
    re.IGNORECASE,
)

_ISOLATION_ENV_VAR = {
    "codex-acp": "CODEX_HOME",
    "claude-agent-acp": "CLAUDE_CONFIG_DIR",
}

_CHANNEL_ID_PATTERN = re.compile(r"^ch_[0-9a-f]{12}$")
_CHANNEL_SECRET_PATTERN = re.compile(r"^[A-Za-z0-9_-]{16,256}$")


def _validation_error(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
        field=field,
    )


def _read_secret_value(ref) -> str:
    if isinstance(ref, SecretFile):
        data = registry.read_owner_file(Path(ref.path))
        if data is None:
            raise _validation_error("connection_requires_reauthorization", "The connection's secret file is missing.")
        return data.decode("utf-8").strip()
    if isinstance(ref, OsKeyring):
        try:
            import keyring
        except ImportError:
            raise _validation_error("connection_unavailable", "The 'keyring' module is not importable in this environment.")
        value = keyring.get_password(ref.service, ref.account)
        if value is None:
            raise _validation_error("connection_requires_reauthorization", "No keyring entry is present for this connection.")
        return value
    raise _validation_error("unsupported_auth_mode", "api_key connections require a secret_file or os_keyring credential_ref.")


def _validate_path_entries(raw_path: str) -> str:
    entries = [entry for entry in raw_path.split(":") if entry and entry.startswith("/")]
    return ":".join(entries)


def _validate_channel_credentials(channel_credentials: tuple[str, str, str]) -> dict[str, str]:
    if not isinstance(channel_credentials, tuple) or len(channel_credentials) != 3:
        raise _validation_error("invalid_field", "channel_credentials must be a (channel_id, secret, project_root) tuple.", field="channel_credentials")
    channel_id, secret, project_root = channel_credentials
    if not isinstance(channel_id, str) or not _CHANNEL_ID_PATTERN.match(channel_id):
        raise _validation_error("invalid_field", "channel_credentials[0] must be a 'ch_' + 12 hex char id.", field="channel_credentials")
    if not isinstance(secret, str) or not _CHANNEL_SECRET_PATTERN.match(secret):
        raise _validation_error("invalid_field", "channel_credentials[1] must be a URL-safe token string.", field="channel_credentials")
    if not isinstance(project_root, str) or not project_root.startswith("/") or "\x00" in project_root:
        raise _validation_error("invalid_field", "channel_credentials[2] must be an absolute path string.", field="channel_credentials")
    return {
        "CORTEX_CHANNEL_ID": channel_id,
        "CORTEX_CHANNEL_SECRET": secret,
        "CORTEX_CHANNEL_PROJECT_ROOT": project_root,
    }


def build_process_env(
    record,
    base_env: dict[str, str],
    *,
    channel_credentials: tuple[str, str, str] | None = None,
    mcp_allowlist: tuple[str, ...] = (),
) -> dict[str, str]:
    """Construct a least-privilege environment for exactly one connection.

    See the module docstring for the exact, complete list of guarantees.
    """
    if not isinstance(mcp_allowlist, tuple) or not all(isinstance(item, str) for item in mcp_allowlist):
        raise _validation_error("invalid_field", "mcp_allowlist must be a tuple of strings.", field="mcp_allowlist")
    # mcp_allowlist is intentionally not forwarded into the process
    # environment here: there is no wholesale MCP server/credential
    # forwarding service. It is accepted and validated so a caller can pass
    # it through a future per-connection MCP config path without this
    # function silently dropping an invalid value.

    env_var_name = _ISOLATION_ENV_VAR.get(record.engine)
    if env_var_name is None:
        raise _validation_error("unsupported_auth_mode", f"No isolation environment variable is defined for engine {record.engine!r}.")

    # Re-verify against the on-disk registry -- never trust a caller-held
    # `record` object's `credential_ref` for I/O without re-checking it is
    # still this connection's own id-derived, non-colliding reference.
    registry_state = registry.read_registry()
    current = registry_state.connections.get(record.id, record)
    registry.assert_canonical_and_unique(current, registry_state.connections)

    result: dict[str, str] = {}
    for key in ALLOWLISTED_BASE_ENV_KEYS:
        if key not in base_env:
            continue
        value = base_env[key]
        if key == "PATH":
            value = _validate_path_entries(value)
        if DENYLIST_PATTERN.search(key):
            continue
        result[key] = value

    if channel_credentials is not None:
        for key, value in _validate_channel_credentials(channel_credentials).items():
            result[key] = value

    # The isolation variable and (for api_key) the single secret variable
    # below are the only place a connection-scoped, previously-vetted value
    # is injected regardless of the denylist; both originate solely from
    # this connection's own freshly re-verified record, never from
    # base_env/channel_credentials.
    result[env_var_name] = _isolation_dir(current)
    if current.auth_mode == AuthMode.API_KEY:
        if current.key_kind not in KEY_KIND_ENV_VAR:
            raise _validation_error("unsupported_auth_mode", f"Unknown key_kind {current.key_kind!r}.")
        result[KEY_KIND_ENV_VAR[current.key_kind]] = _read_secret_value(current.credential_ref)

    return result


def _isolation_dir(record) -> str:
    if isinstance(record.credential_ref, EngineNativeDir):
        return record.credential_ref.path
    return str(registry.connection_state_dir(record.id, record.engine))


def describe_process_env(env: dict[str, str]) -> tuple[str, ...]:
    """Return only the variable *names* present, safe to log/print/compare."""
    return tuple(sorted(env))
