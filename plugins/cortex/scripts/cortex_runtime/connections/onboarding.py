"""Connection onboarding: create, authorize, validate, remove, resolve.

Every mutation goes through :func:`registry.transaction` so a failed step
never partially commits.  Secret bytes only ever pass through
:func:`set_api_key`'s ``reader`` callback (never an argv/string parameter)
and are written straight to a private file; nothing here logs, prints, or
stores a secret value anywhere else, including the registry itself.

``credential_ref`` is never caller-supplied for filesystem-backed kinds:
:func:`create_connection` always derives it from the newly generated
connection id via ``registry.canonical_credential_ref`` (``SecretFile`` ->
``registry.secret_path(id)``; ``EngineNativeDir`` (native_account/gateway)
-> ``registry.connection_state_dir(id, engine)``), so a caller can never
point a connection at an arbitrary path (e.g. inside a project's
``.cortex``, or at another connection's file). Every subsequent read
(``validate``), write (``set_api_key``), instruction
(``mark_native_login_pending``), and deletion (``remove``) re-checks that
the stored ``credential_ref`` still equals that id-derived canonical value
and does not collide with any other connection's path before touching
storage; on mismatch the connection is treated as ``invalid`` and no I/O is
performed against the tampered value.
"""
from __future__ import annotations

from datetime import datetime, timezone
import os
import stat
from pathlib import Path
from typing import Callable

from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from . import registry
from . import trust
from .model import (
    AuthMode,
    ConnectionRecord,
    ConnectionState,
    CredentialRef,
    EngineNativeDir,
    OsKeyring,
    SecretFile,
    validate_auth_selection,
)

# T002 evidence basis: presence of these marker filenames is a documented
# convention read from the adapters' auth handling, never a verified content
# check (we deliberately never read/parse marker file bytes here).
_NATIVE_LOGIN_MARKER = {
    "codex-acp": "auth.json",
    "claude-agent-acp": ".credentials.json",
}

_ISOLATION_ENV_VAR = {
    "codex-acp": "CODEX_HOME",
    "claude-agent-acp": "CLAUDE_CONFIG_DIR",
}

_MAX_ID_ATTEMPTS = 32


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _authorization_error(code: str, remediation: str, *, retry_eligible: bool = False) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.AUTHORIZATION,
        code=code,
        remediation=remediation,
        retry_eligible=retry_eligible,
    )


def _validation_error(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
        field=field,
    )


def _dir_is_safe(path: Path) -> bool:
    try:
        current = Path(path.anchor)
        for part in path.parts[1:]:
            current /= part
            info = current.lstat()
            if stat.S_ISLNK(info.st_mode):
                return False
        info = path.lstat()
        return bool(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not (info.st_mode & 0o077))
    except OSError:
        return False


def _derive_state(record: ConnectionRecord, all_connections: dict[str, ConnectionRecord], environ: dict[str, str] | None) -> ConnectionState:
    """Compute connection state from a credential reference's *shape* only.

    First re-checks that ``record.credential_ref`` is still the id-derived
    canonical reference and collides with no other connection's path (see
    :func:`registry.assert_canonical_and_unique`); a mismatch is always
    ``invalid``, before any filesystem/keyring lookup is attempted. Never
    reads secret file contents or native marker file bytes -- only
    existence, type, ownership, and mode of paths, and (for os_keyring)
    whether an entry is present, never its value.
    """
    if not registry.credential_ref_matches_canonical(record, environ) or record.id in registry.duplicate_credential_paths(all_connections):
        return ConnectionState.INVALID

    ref = record.credential_ref
    engine = record.engine

    if isinstance(ref, EngineNativeDir):
        path = Path(ref.path)
        try:
            path.lstat()
        except FileNotFoundError:
            return ConnectionState.AUTHORIZATION_REQUIRED
        if not _dir_is_safe(path):
            return ConnectionState.INVALID
        marker = _NATIVE_LOGIN_MARKER.get(engine)
        if marker is None:
            return ConnectionState.UNAVAILABLE
        marker_path = path / marker
        try:
            marker_info = marker_path.lstat()
        except FileNotFoundError:
            return ConnectionState.AUTHORIZATION_REQUIRED
        if stat.S_ISLNK(marker_info.st_mode) or not stat.S_ISREG(marker_info.st_mode):
            return ConnectionState.INVALID
        return ConnectionState.CONFIGURED

    if isinstance(ref, SecretFile):
        path = Path(ref.path)
        if not _dir_is_safe(path.parent):
            return ConnectionState.INVALID
        try:
            info = path.lstat()
        except FileNotFoundError:
            return ConnectionState.AUTHORIZATION_REQUIRED
        if (
            stat.S_ISLNK(info.st_mode)
            or not stat.S_ISREG(info.st_mode)
            or info.st_nlink != 1
            or info.st_uid != os.getuid()
            or info.st_mode & 0o077
        ):
            return ConnectionState.INVALID
        if info.st_size == 0:
            return ConnectionState.AUTHORIZATION_REQUIRED
        return ConnectionState.CONFIGURED

    if isinstance(ref, OsKeyring):
        try:
            import keyring
        except ImportError:
            return ConnectionState.UNAVAILABLE
        try:
            value = keyring.get_password(ref.service, ref.account)
        except Exception:
            return ConnectionState.UNAVAILABLE
        present = value is not None
        del value
        return ConnectionState.CONFIGURED if present else ConnectionState.AUTHORIZATION_REQUIRED

    raise _validation_error("invalid_credential_ref", "credential_ref must be a known reference shape.")


def _duplicate_key(record: ConnectionRecord) -> tuple:
    return (
        record.engine,
        record.provider,
        record.auth_mode.value,
        record.key_kind,
        record.endpoint,
        record.launch_command,
    )


def _derive_credential_ref(
    connection_id: str,
    engine: str,
    auth_mode: AuthMode,
    *,
    credential_kind: str,
    keyring_service: str | None,
    keyring_account: str | None,
) -> CredentialRef:
    if auth_mode == AuthMode.API_KEY:
        if credential_kind == "os_keyring":
            return registry.canonical_credential_ref(
                connection_id, engine, "os_keyring", keyring_service=keyring_service, keyring_account=keyring_account,
            )
        if credential_kind == "secret_file":
            return registry.canonical_credential_ref(connection_id, engine, "secret_file")
        raise _validation_error("unsupported_auth_mode", "credential_kind must be 'secret_file' or 'os_keyring' for api_key connections.", field="credential_kind")
    # native_account and gateway both use engine-native state directory
    # isolation; there is no per-connection filesystem state for os_keyring
    # in those modes (they never carry a Cortex-managed secret).
    return registry.canonical_credential_ref(connection_id, engine, "engine_native_dir")


def create_connection(
    *,
    name: str,
    engine: str,
    provider: str,
    auth_mode: AuthMode,
    key_kind: str | None = None,
    endpoint: str | None = None,
    launch_command: tuple[str, ...] | None = None,
    credential_kind: str = "secret_file",
    keyring_service: str | None = None,
    keyring_account: str | None = None,
    reuse: bool = False,
    environ: dict[str, str] | None = None,
) -> ConnectionRecord:
    """Register a new connection, or return the existing one iff reuse=True.

    ``name``/``provider``/``endpoint``/``launch_command`` as supplied here
    seed ``approved_fingerprint`` (see trust.fingerprint); a project cannot
    silently change them afterward without an explicit
    :func:`trust.approve_override`.

    There is no ``credential_ref``/path parameter: the credential reference
    is always derived from the newly generated connection id (see module
    docstring). ``credential_kind``/``keyring_service``/``keyring_account``
    only apply to ``auth_mode=api_key`` and select between a Cortex-managed
    secret file (default) and an OS keyring entry (``keyring_service``/
    ``keyring_account`` are plain identifiers, never paths -- enforced by
    :class:`model.OsKeyring`).
    """
    support = validate_auth_selection(engine, auth_mode, key_kind)
    fingerprint = trust.fingerprint(engine, provider, endpoint, launch_command)
    now = _now_iso()
    with registry.transaction(environ) as state:
        candidate = None
        for _attempt in range(_MAX_ID_ATTEMPTS):
            candidate_id = new_id("cn_")
            if candidate_id not in state.connections:
                candidate = candidate_id
                break
        if candidate is None:
            raise _validation_error("identifier_unavailable", "Could not allocate a unique connection id; retry.")

        credential_ref = _derive_credential_ref(
            candidate, engine, auth_mode,
            credential_kind=credential_kind, keyring_service=keyring_service, keyring_account=keyring_account,
        )
        if isinstance(credential_ref, EngineNativeDir):
            registry.ensure_private_dir(Path(credential_ref.path), root=registry.state_root(environ))
        elif isinstance(credential_ref, SecretFile):
            registry.ensure_private_dir(Path(credential_ref.path).parent, root=registry.config_root(environ))

        record = ConnectionRecord(
            id=candidate,
            name=name,
            engine=engine,
            provider=provider,
            auth_mode=auth_mode,
            key_kind=key_kind,
            endpoint=endpoint,
            launch_command=launch_command,
            credential_ref=credential_ref,
            approved_fingerprint=fingerprint,
            state=ConnectionState.AUTHORIZATION_REQUIRED,
            qualified=support.qualified,
            created_at=now,
            updated_at=now,
        )
        key = _duplicate_key(record)
        for existing in state.connections.values():
            if _duplicate_key(existing) == key:
                if not reuse:
                    raise _validation_error(
                        "connection_already_exists",
                        "A connection with the same engine/provider/auth_mode/key_kind/"
                        "endpoint/launch_command already exists; pass reuse=True to reuse it "
                        "instead of registering a materially identical duplicate.",
                    )
                return existing
        record = ConnectionRecord(
            id=record.id, name=record.name, engine=record.engine, provider=record.provider,
            auth_mode=record.auth_mode, key_kind=record.key_kind, endpoint=record.endpoint,
            launch_command=record.launch_command, credential_ref=record.credential_ref,
            approved_fingerprint=record.approved_fingerprint,
            state=_derive_state(record, {**state.connections, record.id: record}, environ),
            qualified=record.qualified, created_at=record.created_at, updated_at=record.updated_at,
        )
        state.connections[record.id] = record
    return record


def set_api_key(connection_id: str, reader: Callable[[], bytes], *, environ: dict[str, str] | None = None) -> ConnectionRecord:
    """Write an API key secret from ``reader()`` bytes; never an argv/string path.

    ``reader`` is called exactly once and its return value is written
    directly to the connection's private secret file; the value is never
    logged, printed, or stored anywhere else (including the registry).
    Refuses to write if ``credential_ref`` is not this connection's
    id-derived canonical secret file (see :func:`registry.assert_canonical_and_unique`).
    """
    with registry.transaction(environ) as state:
        record = state.connections.get(connection_id)
        if record is None:
            raise _authorization_error("connection_unavailable", "Unknown connection id; register the connection before setting its key.")
        if record.auth_mode != AuthMode.API_KEY or record.key_kind is None:
            raise _validation_error("unsupported_auth_mode", "set_api_key only applies to api_key connections with a key_kind.")
        if not isinstance(record.credential_ref, SecretFile):
            raise _validation_error("unsupported_auth_mode", "set_api_key requires a secret_file credential_ref.")
        registry.assert_canonical_and_unique(record, state.connections, environ)
        path = registry.secret_path(record.id, environ)
        secret = reader()
        if not isinstance(secret, (bytes, bytearray)) or len(secret) == 0:
            raise _validation_error("empty_secret", "The supplied API key must be non-empty bytes.", field="secret")
        registry.ensure_private_dir(path.parent, root=registry.config_root(environ))
        registry.atomic_write_private(path, bytes(secret), directory=path.parent)
        del secret
        updated = ConnectionRecord(
            id=record.id,
            name=record.name,
            engine=record.engine,
            provider=record.provider,
            auth_mode=record.auth_mode,
            key_kind=record.key_kind,
            endpoint=record.endpoint,
            launch_command=record.launch_command,
            credential_ref=record.credential_ref,
            approved_fingerprint=record.approved_fingerprint,
            state=_derive_state(record, state.connections, environ),
            qualified=record.qualified,
            created_at=record.created_at,
            updated_at=_now_iso(),
        )
        state.connections[connection_id] = updated
    return updated


def mark_native_login_pending(connection_id: str, *, environ: dict[str, str] | None = None) -> str:
    """Return non-secret instructions naming the isolation dir/env var.

    Never touches or reports any credential content; this only tells the
    user which engine-specific environment variable and directory to use
    for their own native login flow. Refuses (raises) if ``credential_ref``
    is not this connection's id-derived canonical native directory.
    """
    with registry.transaction(environ) as state:
        record = state.connections.get(connection_id)
        if record is None:
            raise _authorization_error("connection_unavailable", "Unknown connection id.")
        if record.auth_mode != AuthMode.NATIVE_ACCOUNT or not isinstance(record.credential_ref, EngineNativeDir):
            raise _validation_error("unsupported_auth_mode", "mark_native_login_pending only applies to native_account connections with an engine_native_dir.")
        registry.assert_canonical_and_unique(record, state.connections, environ)
        env_var = _ISOLATION_ENV_VAR.get(record.engine)
        if env_var is None:
            raise _validation_error("unsupported_auth_mode", f"No isolation env var is defined for engine {record.engine!r}.")
        directory = str(registry.connection_state_dir(record.id, record.engine, environ))
        updated = ConnectionRecord(
            id=record.id, name=record.name, engine=record.engine, provider=record.provider,
            auth_mode=record.auth_mode, key_kind=record.key_kind, endpoint=record.endpoint,
            launch_command=record.launch_command, credential_ref=record.credential_ref,
            approved_fingerprint=record.approved_fingerprint, state=ConnectionState.AUTHORIZATION_REQUIRED,
            qualified=record.qualified, created_at=record.created_at, updated_at=_now_iso(),
        )
        state.connections[connection_id] = updated
    return (
        f"Run the {record.engine} native account login with {env_var}={directory} set in its "
        "process environment, using only this connection's isolated directory; Cortex does not "
        "collect, store, or forward this credential itself."
    )


def validate(connection_id: str, *, environ: dict[str, str] | None = None) -> ConnectionRecord:
    """Recompute and persist this connection's state from its current ref."""
    with registry.transaction(environ) as state:
        record = state.connections.get(connection_id)
        if record is None:
            raise _authorization_error("connection_unavailable", "Unknown connection id.")
        new_state = _derive_state(record, state.connections, environ)
        if new_state == record.state:
            return record
        updated = ConnectionRecord(
            id=record.id, name=record.name, engine=record.engine, provider=record.provider,
            auth_mode=record.auth_mode, key_kind=record.key_kind, endpoint=record.endpoint,
            launch_command=record.launch_command, credential_ref=record.credential_ref,
            approved_fingerprint=record.approved_fingerprint, state=new_state,
            qualified=record.qualified, created_at=record.created_at, updated_at=_now_iso(),
        )
        state.connections[connection_id] = updated
    return updated


def remove(connection_id: str, *, environ: dict[str, str] | None = None) -> None:
    """Delete the registry entry and any Cortex-created secret file only.

    Never deletes engine-native directory contents (``EngineNativeDir``):
    that directory and everything in it belongs to the engine, not Cortex.
    Only ever unlinks a secret file that is this connection's own
    id-derived canonical path (never a caller/attacker-supplied path from a
    tampered registry entry).
    """
    with registry.transaction(environ) as state:
        record = state.connections.pop(connection_id, None)
        if record is None:
            raise _authorization_error("connection_unavailable", "Unknown connection id.")
        state.override_history.pop(connection_id, None)
        if isinstance(record.credential_ref, SecretFile) and registry.credential_ref_matches_canonical(record, environ):
            path = registry.secret_path(record.id, environ)
            try:
                info = path.lstat()
            except FileNotFoundError:
                info = None
            if info is not None and stat.S_ISREG(info.st_mode) and not stat.S_ISLNK(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1:
                path.unlink()


_STATE_ERROR_CODE = {
    ConnectionState.AUTHORIZATION_REQUIRED: "connection_requires_reauthorization",
    ConnectionState.UNAVAILABLE: "connection_unavailable",
    ConnectionState.INVALID: "connection_invalid",
}


def resolve_connection_for_route(connection_id: str, *, environ: dict[str, str] | None = None) -> ConnectionRecord:
    """Return the connection iff configured; never substitutes another one."""
    try:
        record = validate(connection_id, environ=environ)
    except PortableError:
        raise
    if record.state == ConnectionState.CONFIGURED:
        return record
    code = _STATE_ERROR_CODE[record.state]
    raise _authorization_error(
        code,
        f"Connection {connection_id} is {record.state.value}; complete its onboarding/"
        "re-authorization action instead of silently switching to a different connection.",
    )
