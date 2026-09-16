"""User-scoped connection registry: storage, locking, and safety checks.

The registry lives under ``$XDG_CONFIG_HOME/cortex`` (or ``~/.config/cortex``
when unset), never under any project or its ``.cortex`` store -- this keeps
credential *references* (never values) out of project state, Git, and
reports, matching the packet-wide invariant that ``.cortex`` is the only
project runtime store and credentials never enter it.

Per-connection engine-native state (``CODEX_HOME``/``CLAUDE_CONFIG_DIR``
equivalents) lives under ``$XDG_STATE_HOME/cortex/connections/<id>/``; Cortex-
managed secret files live under
``$XDG_CONFIG_HOME/cortex/secrets/<id>``. Every directory this module creates
is ``0700`` and every file it writes is ``0600``; every read rejects
symlinks, hardlinks, wrong ownership, and group/other-readable modes before
trusting the content.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import asdict
import fcntl
import json
import os
from pathlib import Path
import stat
import tempfile
from typing import Iterator

from ..portable.errors import PortableError, PortableErrorCategory
from .model import (
    AuthMode,
    ConnectionRecord,
    ConnectionState,
    EngineNativeDir,
    OsKeyring,
    SecretFile,
    deserialize_credential_ref,
    serialize_credential_ref,
)

SCHEMA_VERSION = 1
_ROOT_KEYS = {"schema_version", "connections", "override_history"}
_RECORD_KEYS = {
    "id",
    "name",
    "engine",
    "provider",
    "auth_mode",
    "key_kind",
    "endpoint",
    "launch_command",
    "credential_ref",
    "approved_fingerprint",
    "state",
    "qualified",
    "created_at",
    "updated_at",
}
_HISTORY_ENTRY_KEYS = {
    "fingerprint",
    "previous_fingerprint",
    "engine",
    "provider",
    "endpoint",
    "launch_command",
    "approved_at",
    "approved_by_user",
}


def _fail_closed(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
        field=field,
    )


_PROJECT_MARKERS = (".git", ".cortex")


def _assert_no_project_component(path: Path) -> None:
    """Reject any registry/state/secret path that passes through ``.cortex``.

    The connection registry, per-connection engine-native state, and
    Cortex-managed secret files are user-scoped and must never live inside a
    project's ``.cortex`` store (the only supported project runtime store).
    """
    if ".cortex" in path.parts:
        raise _fail_closed(
            "connection_storage_under_project",
            "Connection storage must be user-scoped under XDG_CONFIG_HOME/"
            "XDG_STATE_HOME, never inside a project's .cortex store.",
        )


def _assert_not_nested_in_project_workspace(path: Path) -> None:
    """Reject storage nested anywhere under a directory that is itself a
    project workspace (has a ``.git`` or ``.cortex`` entry directly inside
    it), even when no path component is literally named ``.cortex``.

    This defends against ``XDG_CONFIG_HOME``/``XDG_STATE_HOME`` being
    pointed at an arbitrary subdirectory of a project checkout/worktree.
    Every ancestor directory of ``path`` (excluding ``path`` itself) is
    checked; a user whose own ``$HOME`` is a dotfiles Git repository should
    set ``XDG_CONFIG_HOME``/``XDG_STATE_HOME`` outside that repository.
    """
    current = Path(path.anchor)
    for part in path.parts[1:-1]:
        current = current / part
        for marker in _PROJECT_MARKERS:
            marker_path = current / marker
            try:
                marker_info = marker_path.lstat()
            except OSError:
                continue
            raise _fail_closed(
                "connection_storage_under_project",
                f"{path} is nested under {current}, which contains a {marker!r} "
                "project marker; connection storage must be user-scoped, never "
                "inside any project workspace.",
            )


def config_root(environ: dict[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    raw = env.get("XDG_CONFIG_HOME")
    base = Path(raw).expanduser() if raw else Path(env.get("HOME", str(Path.home()))).expanduser() / ".config"
    root = base / "cortex"
    _assert_no_project_component(root)
    _assert_not_nested_in_project_workspace(root)
    return root


def state_root(environ: dict[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    raw = env.get("XDG_STATE_HOME")
    base = Path(raw).expanduser() if raw else Path(env.get("HOME", str(Path.home()))).expanduser() / ".local" / "state"
    root = base / "cortex"
    _assert_no_project_component(root)
    _assert_not_nested_in_project_workspace(root)
    return root


def registry_path(environ: dict[str, str] | None = None) -> Path:
    return config_root(environ) / "connections.json"


def registry_lock_path(environ: dict[str, str] | None = None) -> Path:
    return config_root(environ) / "connections.lock"


def secrets_dir(environ: dict[str, str] | None = None) -> Path:
    return config_root(environ) / "secrets"


def secret_path(connection_id: str, environ: dict[str, str] | None = None) -> Path:
    return secrets_dir(environ) / connection_id


_ENGINE_STATE_SUBDIR = {
    "codex-acp": "codex-home",
    "claude-agent-acp": "claude-config",
}


def connection_state_dir(connection_id: str, engine: str, environ: dict[str, str] | None = None) -> Path:
    """Per-connection engine-native state root (CODEX_HOME/CLAUDE_CONFIG_DIR)."""
    subdir = _ENGINE_STATE_SUBDIR.get(engine)
    if subdir is None:
        raise _fail_closed("unsupported_auth_mode", f"Unsupported engine {engine!r} has no isolated state directory convention.")
    return state_root(environ) / "connections" / connection_id / subdir


def canonical_credential_ref(
    connection_id: str,
    engine: str,
    ref_kind: str,
    *,
    keyring_service: str | None = None,
    keyring_account: str | None = None,
    environ: dict[str, str] | None = None,
):
    """The one id-derived reference a connection's ``credential_ref`` must
    equal for its ``kind`` -- never accepted from a caller for filesystem
    kinds, only recomputed here from ``connection_id``/``engine``.
    """
    if ref_kind == "secret_file":
        return SecretFile(path=str(secret_path(connection_id, environ)))
    if ref_kind == "engine_native_dir":
        return EngineNativeDir(path=str(connection_state_dir(connection_id, engine, environ)))
    if ref_kind == "os_keyring":
        return OsKeyring(service=keyring_service or "cortex", account=keyring_account or connection_id)
    raise _fail_closed("invalid_credential_ref", f"Unknown credential_ref kind {ref_kind!r}.")


def credential_ref_matches_canonical(record: ConnectionRecord, environ: dict[str, str] | None = None) -> bool:
    """True iff ``record.credential_ref`` is exactly the id-derived reference.

    For ``os_keyring`` this only confirms the stored service/account are
    themselves well-formed identifiers (already enforced by
    :class:`model.OsKeyring`); there is no id-derived filesystem path to
    compare against for a keyring entry.
    """
    ref = record.credential_ref
    if isinstance(ref, OsKeyring):
        return True
    try:
        expected = canonical_credential_ref(record.id, record.engine, ref.kind, environ=environ)
    except PortableError:
        return False
    if not isinstance(ref, type(expected)):
        return False
    if ref.path != expected.path:
        return False
    allowed_root = config_root(environ) if isinstance(ref, SecretFile) else state_root(environ)
    return _is_root_or_descendant(Path(ref.path), allowed_root)


def duplicate_credential_paths(connections: dict[str, ConnectionRecord]) -> dict[str, str]:
    """Map ``connection_id -> colliding path`` for every filesystem
    ``credential_ref`` whose path is shared with a different connection id.

    A correctly generated connection can never collide (paths are id-
    derived from a unique ``cn_`` id); a non-empty result here only occurs
    for a hand-tampered ``connections.json``.
    """
    seen: dict[str, str] = {}
    collisions: dict[str, str] = {}
    for connection_id, record in connections.items():
        ref = record.credential_ref
        if not isinstance(ref, (SecretFile, EngineNativeDir)):
            continue
        holder = seen.get(ref.path)
        if holder is not None:
            collisions[connection_id] = ref.path
            collisions[holder] = ref.path
        else:
            seen[ref.path] = connection_id
    return collisions


def assert_canonical_and_unique(
    record: ConnectionRecord,
    all_connections: dict[str, ConnectionRecord],
    environ: dict[str, str] | None = None,
) -> None:
    """Raise ``connection_invalid`` before any read/write touches storage.

    Called immediately before every secret read, secret write, or native
    login instruction: a tampered ``credential_ref`` (wrong path, or a path
    shared with another connection) must never be trusted for I/O, even if
    it happens to look like a safe, owner-only file or directory.
    """
    if not credential_ref_matches_canonical(record, environ):
        raise _fail_closed(
            "connection_invalid",
            f"Connection {record.id} credential_ref does not match its id-derived "
            "canonical reference; it will never be used for reads/writes.",
        )
    if record.id in duplicate_credential_paths(all_connections):
        raise _fail_closed(
            "connection_invalid",
            f"Connection {record.id} credential_ref path collides with another "
            "connection's; it will never be used for reads/writes.",
        )


def _is_root_or_descendant(path: Path, root: Path) -> bool:
    """Exact-prefix check on the literal path strings (no resolve heuristics).

    ``path`` must equal ``root`` or have ``root`` as one of its
    ``Path.parents`` entries. Both are expected to already be the clean,
    absolute, programmatically constructed paths this module builds (never
    a caller-controlled string), so a literal-component prefix check is
    sufficient and avoids resolving a possibly-not-yet-existing path.
    """
    return path == root or root in path.parents


def ensure_private_dir(path: Path, *, root: Path) -> None:
    """Create/validate an owner-only (0700) directory, rejecting symlinks.

    ``path`` must be exactly ``root`` or a descendant of it (an explicit,
    resolved-path prefix check -- never a component-name heuristic like
    matching a directory literally named ``cortex``). Only ``root`` and
    everything at/below it is created and locked down to ``0700``;
    pre-existing ancestors above ``root`` (``$HOME``, ``$XDG_CONFIG_HOME``,
    ...) belong to the user/OS and are never chmod'd, only checked for an
    unsafe symlink component, mirroring
    ``runtime.state.RuntimePaths.ensure`` which likewise only locks down
    ``CODEX_HOME`` and below, never its ancestors.
    """
    if not path.is_absolute() or not root.is_absolute():
        raise _fail_closed("unsafe_storage", "Connection storage paths must be absolute.")
    if not _is_root_or_descendant(path, root):
        raise _fail_closed("unsafe_storage", f"{path} is not under the Cortex-managed root {root}.")
    _assert_not_nested_in_project_workspace(path)
    parts = path.parts
    root_len = len(root.parts)
    current = Path(parts[0])
    for index, part in enumerate(parts[1:], start=1):
        current /= part
        managed = (index + 1) >= root_len
        try:
            info = current.lstat()
        except FileNotFoundError:
            current.mkdir(mode=0o700 if managed else 0o755)
            info = current.lstat()
        if stat.S_ISLNK(info.st_mode):
            raise _fail_closed("unsafe_storage", f"{current} is an unsafe symlink; connection storage must be a real directory tree.")
        if not stat.S_ISDIR(info.st_mode):
            raise _fail_closed("unsafe_storage", f"{current} must be a directory.")
        if managed:
            os.chmod(current, 0o700)
            info = current.lstat()
            if info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise _fail_closed("unsafe_storage", f"{current} must be owner-only and owned by the current user.")


def _safe_existing_file(path: Path) -> os.stat_result | None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if (
        stat.S_ISLNK(info.st_mode)
        or not stat.S_ISREG(info.st_mode)
        or info.st_nlink != 1
        or info.st_uid != os.getuid()
        or info.st_mode & 0o077
    ):
        raise _fail_closed("unsafe_storage", f"{path} is an unsafe link/hardlink/mode file.")
    return info


def read_owner_file(path: Path) -> bytes | None:
    """O_NOFOLLOW read of one owner-only regular single-link file, or None."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    try:
        descriptor = os.open(path, flags)
    except FileNotFoundError:
        return None
    except OSError:
        raise _fail_closed("unsafe_storage", f"{path} is an unsafe link or cannot be opened safely.")
    try:
        info = os.fstat(descriptor)
        if (
            stat.S_ISLNK(info.st_mode)
            or not stat.S_ISREG(info.st_mode)
            or info.st_nlink != 1
            or info.st_uid != os.getuid()
            or info.st_mode & 0o077
        ):
            raise _fail_closed("unsafe_storage", f"{path} is an unsafe link/hardlink/mode file.")
        with os.fdopen(descriptor, "rb") as handle:
            descriptor = -1
            return handle.read()
    finally:
        if descriptor != -1:
            os.close(descriptor)


def atomic_write_private(path: Path, data: bytes, *, directory: Path) -> None:
    _safe_existing_file(path)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=directory)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _record_to_dict(record: ConnectionRecord) -> dict[str, object]:
    payload = asdict(record)
    payload["auth_mode"] = record.auth_mode.value
    payload["state"] = record.state.value
    payload["credential_ref"] = serialize_credential_ref(record.credential_ref)
    payload["launch_command"] = list(record.launch_command) if record.launch_command is not None else None
    return payload


def _record_from_dict(data: object) -> ConnectionRecord:
    if not isinstance(data, dict) or set(data) != _RECORD_KEYS:
        raise _fail_closed("invalid_connection_record", "A connection record has an unexpected or missing field set.")
    try:
        auth_mode = AuthMode(data["auth_mode"])
        state = ConnectionState(data["state"])
    except ValueError:
        raise _fail_closed("invalid_connection_record", "auth_mode/state must be known enum values.") from None
    launch_command = data["launch_command"]
    if launch_command is not None:
        if not isinstance(launch_command, list) or not all(isinstance(part, str) for part in launch_command):
            raise _fail_closed("invalid_connection_record", "launch_command must be a list of strings or null.")
        launch_command = tuple(launch_command)
    return ConnectionRecord(
        id=data["id"],
        name=data["name"],
        engine=data["engine"],
        provider=data["provider"],
        auth_mode=auth_mode,
        key_kind=data["key_kind"],
        endpoint=data["endpoint"],
        launch_command=launch_command,
        credential_ref=deserialize_credential_ref(data["credential_ref"]),
        approved_fingerprint=data["approved_fingerprint"],
        state=state,
        qualified=data["qualified"],
        created_at=data["created_at"],
        updated_at=data["updated_at"],
    )


def _validate_history_entry(entry: object) -> dict[str, object]:
    if not isinstance(entry, dict) or set(entry) != _HISTORY_ENTRY_KEYS:
        raise _fail_closed("invalid_override_history", "An override-history entry has an unexpected or missing field set.")
    return entry


class RegistryState:
    """In-memory registry contents: connections plus override history."""

    __slots__ = ("connections", "override_history")

    def __init__(self, connections: dict[str, ConnectionRecord], override_history: dict[str, list[dict[str, object]]]) -> None:
        self.connections = connections
        self.override_history = override_history

    def to_json_bytes(self) -> bytes:
        payload = {
            "schema_version": SCHEMA_VERSION,
            "connections": {cid: _record_to_dict(record) for cid, record in sorted(self.connections.items())},
            "override_history": {cid: entries for cid, entries in sorted(self.override_history.items())},
        }
        return json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def _empty_state() -> RegistryState:
    return RegistryState(connections={}, override_history={})


def _parse_registry_bytes(data: bytes) -> RegistryState:
    if not data:
        return _empty_state()
    try:
        raw = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise _fail_closed("invalid_registry", "connections.json is not valid UTF-8 JSON.") from None
    if not isinstance(raw, dict) or set(raw) - _ROOT_KEYS:
        raise _fail_closed("invalid_registry", f"connections.json must only contain keys from {sorted(_ROOT_KEYS)}.")
    if raw.get("schema_version") != SCHEMA_VERSION:
        raise _fail_closed("invalid_registry", f"connections.json schema_version must be {SCHEMA_VERSION}.")
    raw_connections = raw.get("connections", {})
    if not isinstance(raw_connections, dict):
        raise _fail_closed("invalid_registry", "connections.json 'connections' must be an object.")
    connections = {cid: _record_from_dict(value) for cid, value in raw_connections.items()}
    for cid, record in connections.items():
        if record.id != cid:
            raise _fail_closed("invalid_registry", "connection record id must match its registry key.")
    raw_history = raw.get("override_history", {})
    if not isinstance(raw_history, dict):
        raise _fail_closed("invalid_registry", "connections.json 'override_history' must be an object.")
    override_history: dict[str, list[dict[str, object]]] = {}
    for cid, entries in raw_history.items():
        if not isinstance(entries, list):
            raise _fail_closed("invalid_registry", "override_history entries must be a list.")
        override_history[cid] = [_validate_history_entry(entry) for entry in entries]
    return RegistryState(connections=connections, override_history=override_history)


def read_registry(environ: dict[str, str] | None = None) -> RegistryState:
    """Read and validate the registry; a missing file is an empty registry."""
    path = registry_path(environ)
    ensure_private_dir(path.parent, root=config_root(environ))
    data = read_owner_file(path)
    if data is None:
        return _empty_state()
    return _parse_registry_bytes(data)


def write_registry(state: RegistryState, environ: dict[str, str] | None = None) -> None:
    path = registry_path(environ)
    ensure_private_dir(path.parent, root=config_root(environ))
    atomic_write_private(path, state.to_json_bytes(), directory=path.parent)


@contextmanager
def transaction(environ: dict[str, str] | None = None) -> Iterator[RegistryState]:
    """Lock, read, yield a mutable :class:`RegistryState`, then write back.

    Writing back only happens if the ``with`` block exits without raising,
    so a failed onboarding/validation step never partially commits.
    """
    lock_path = registry_lock_path(environ)
    ensure_private_dir(lock_path.parent, root=config_root(environ))
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(lock_path, flags, 0o600)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise _fail_closed("unsafe_storage", "connections.lock is an unsafe link/hardlink/mode file.")
        os.fchmod(descriptor, 0o600)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        try:
            state = read_registry(environ)
            yield state
            write_registry(state, environ)
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)
