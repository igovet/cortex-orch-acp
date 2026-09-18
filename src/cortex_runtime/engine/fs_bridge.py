"""FsBridge: local-worktree ``fs/read_text_file``/``fs/write_text_file``
agent-to-client handling (T014), only ever reachable when
``resource_policy.ResourcePolicy.fs_local`` is true (see
``session_manager.EngineSessionManager``'s dispatch).

Containment: every path is required to already be ACP-absolute (POSIX
``/...``; a relative path, a NUL byte, or a Windows-drive-shaped path such
as ``C:\\...`` is rejected before any filesystem call). The read/write
target is then *opened* (``O_NOFOLLOW`` on the final component) and its
real, symlink-resolved path is read back from the open file descriptor
itself (never re-derived from the original path string -- see the TOCTOU
note below) and required to be contained in the bound
``ResourceBinding.workspace_root`` via ``os.path.commonpath`` -- this is
what keeps one worker's binding from ever touching another worker's
worktree (they have different, non-overlapping ``workspace_root`` values)
and rejects any ``..`` traversal or symlink escape.

Technical storage: any path component named ``.git`` or ``.cortex`` inside
the workspace is refused unconditionally. The task card's "own-draft area"
carve-out for a binding's own ``.cortex`` storage is a documented,
deliberate simplification for this task -- see
``docs/acp/permissions-and-containment.md`` -- rather than wiring
``project_storage``'s draft classification through here, which the lead
design flagged as optional ("if wiring that is complex, deny all
``.cortex`` and document").

Every read/write records a :class:`Revision` (``source="disk"``) into a
:class:`RevisionLog`, keyed by ``workspace_binding_id`` -- distinct from
``HostFsForwarder``'s ``source="host_buffer"`` revisions, and never
compared/equated with them (T014 acceptance bullet 3).

TOCTOU (T014 review, HIGH): the check (containment/technical-storage/
regular-file) and the actual I/O are never two separate filesystem
operations against a *path* -- every check is instead performed against
the already-*opened file descriptor* (``os.O_NOFOLLOW`` on the final
path component, then ``os.readlink("/proc/self/fd/<fd>")`` for the fd's
real, fully-resolved path, then containment/technical-storage checks
against *that*), so there is no window between "checked" and "used" in
which the engine (or anything else) could swap the target. If
``/proc/self/fd`` is ever unavailable, this fails closed (refuses the
operation) rather than silently skipping the fd-real-path check. The same
principle applies to writes: the *parent directory* is opened once
(``O_DIRECTORY | O_NOFOLLOW``) and every subsequent operation (stat of the
existing target, temp-file creation, the final atomic rename) happens
*relative to that one directory file descriptor* (``dir_fd=``), so a
parent directory swapped for a symlink after this bridge "checked" it
cannot redirect the write anywhere.

Non-regular files (FIFOs, character/block devices, sockets, directories)
are refused without ever blocking: the open always uses ``O_NONBLOCK``
(harmless for a regular file; for a FIFO it means "open now, do not wait
for a writer"), and the very next step is an ``fstat`` that rejects
anything that is not ``S_ISREG`` before any read is attempted.

Known limitation (documented, not fixed by this task): a *hard* link
inside the workspace to a file whose *other* name lives outside the
workspace is not detectable from a file descriptor alone -- the fd's own
real path (via ``/proc/self/fd``) is the in-workspace name used to open
it, which legitimately passes containment; there is no portable, race-free
way to enumerate every other name (inside or outside the workspace) that
resolves to the same inode from a single open fd.
"""
from __future__ import annotations

import hashlib
import os
import stat as stat_module
import threading
import uuid
from dataclasses import dataclass

MAX_READ_BYTES = 8 * 1024 * 1024
TECHNICAL_STORAGE_NAMES = {".git", ".cortex"}
_TEMP_NAME_ATTEMPTS = 8


class FsBridgeError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


@dataclass(frozen=True)
class Revision:
    source: str  # "disk" | "host_buffer"
    sha256: str
    path: str


class RevisionLog:
    """In-memory, per-``EngineSessionManager`` revision history, keyed by
    ``workspace_binding_id``. Never persisted; a later safe-integration
    precondition check (out of this task's scope) reads it fresh."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._entries: dict[str, list[Revision]] = {}

    def record(self, workspace_binding_id: str, revision: Revision) -> None:
        with self._lock:
            self._entries.setdefault(workspace_binding_id, []).append(revision)

    def entries_for(self, workspace_binding_id: str) -> list[Revision]:
        with self._lock:
            return list(self._entries.get(workspace_binding_id, ()))


def _validate_absolute_path(raw_path) -> str:
    if not isinstance(raw_path, str) or not raw_path:
        raise FsBridgeError(-32602, "path must be a non-empty string.")
    if "\x00" in raw_path:
        raise FsBridgeError(-32602, "path must not contain a NUL byte.")
    if "\\" in raw_path:
        raise FsBridgeError(-32602, "path must be a POSIX path; backslashes are refused.")
    if len(raw_path) >= 2 and raw_path[1] == ":" and raw_path[0].isalpha():
        raise FsBridgeError(-32602, "path looks like a Windows drive path; refused.")
    if not raw_path.startswith("/"):
        raise FsBridgeError(-32602, "path must be absolute.")
    return raw_path


def _contained(resolved: str, workspace_root: str) -> bool:
    try:
        return os.path.commonpath([resolved, workspace_root]) == workspace_root
    except ValueError:
        return False


def _has_technical_component(resolved: str, workspace_root: str) -> bool:
    try:
        rel = os.path.relpath(resolved, workspace_root)
    except ValueError:
        return True
    if rel == os.curdir:
        return False
    return any(part in TECHNICAL_STORAGE_NAMES for part in rel.split(os.sep))


def _real_path_of_fd(fd: int) -> str:
    """The fully symlink-resolved real path an already-open file
    descriptor refers to -- read directly from the kernel via
    ``/proc/self/fd``, never re-derived from the original request path (see
    module docstring's TOCTOU note). Fails closed: any error reading this
    link (including ``/proc`` being unavailable) is treated as "cannot
    verify", never as "assume it is fine"."""
    try:
        return os.readlink(f"/proc/self/fd/{fd}")
    except OSError as exc:
        raise FsBridgeError(-32603, "cannot verify the opened file's real path; refusing.") from exc


def _check_contained_and_not_technical(real_path: str, workspace_root: str) -> None:
    if not _contained(real_path, workspace_root):
        raise FsBridgeError(-32602, "path is outside the bound workspace root.")
    if _has_technical_component(real_path, workspace_root):
        raise FsBridgeError(-32602, "path refers to technical storage (.git/.cortex); refused.")


class FsBridge:
    def __init__(self, *, revision_log: RevisionLog | None = None, max_bytes: int = MAX_READ_BYTES):
        self._revision_log = revision_log if revision_log is not None else RevisionLog()
        self._max_bytes = max_bytes

    def read_text_file(self, binding, params) -> dict:
        raw_path = _validate_absolute_path((params or {}).get("path"))
        workspace_root = binding.workspace_root
        # Open first (O_NOFOLLOW refuses a symlink at the final path
        # component atomically; O_NONBLOCK means a FIFO with no writer
        # returns immediately instead of blocking this call), then check
        # everything else against the *fd* -- never against the original
        # path string again (see module docstring's TOCTOU note).
        try:
            fd = os.open(raw_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        except OSError as exc:
            raise FsBridgeError(-32602, f"path is not readable: {exc}") from exc
        try:
            info = os.fstat(fd)
            if not stat_module.S_ISREG(info.st_mode):
                raise FsBridgeError(
                    -32602, "path is not a regular file (refusing FIFO/device/socket/directory)."
                )
            real_path = _real_path_of_fd(fd)
            _check_contained_and_not_technical(real_path, workspace_root)
            if info.st_size > self._max_bytes:
                raise FsBridgeError(-32602, "file exceeds the maximum readable size.")
            # O_NONBLOCK has no read-blocking effect on a regular file (only
            # relevant to FIFOs/some devices, already refused above); read
            # exactly the fstat'd size, from the fd, never by path again.
            data = os.read(fd, info.st_size)
        finally:
            os.close(fd)
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            raise FsBridgeError(-32602, "file is not valid UTF-8 text.") from None
        line = (params or {}).get("line")
        limit = (params or {}).get("limit")
        if isinstance(line, int) or isinstance(limit, int):
            lines = text.splitlines(keepends=True)
            start = max(0, line - 1) if isinstance(line, int) and line >= 1 else 0
            end = start + limit if isinstance(limit, int) and limit >= 0 else len(lines)
            text = "".join(lines[start:end])
        self._revision_log.record(
            binding.workspace_binding_id,
            Revision(source="disk", sha256=hashlib.sha256(data).hexdigest(), path=real_path),
        )
        return {"content": text}

    def write_text_file(self, binding, params) -> dict:
        raw_path = _validate_absolute_path((params or {}).get("path"))
        content = (params or {}).get("content")
        if not isinstance(content, str):
            raise FsBridgeError(-32602, "content must be a string.")
        workspace_root = binding.workspace_root
        parent = os.path.dirname(raw_path) or "/"
        basename = os.path.basename(raw_path)
        if not basename:
            raise FsBridgeError(-32602, "path must not end in a separator.")
        # Open the parent directory exactly once (O_NOFOLLOW refuses the
        # final component itself being a symlink) and perform every later
        # operation -- stat of an existing target, temp-file creation, the
        # atomic rename -- relative to *this one directory fd*
        # (``dir_fd=``), so a parent swapped for a symlink after this open
        # can never redirect anything (see module docstring's TOCTOU note).
        try:
            dir_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        except OSError as exc:
            raise FsBridgeError(-32602, f"parent directory is not accessible: {exc}") from exc
        try:
            real_parent = _real_path_of_fd(dir_fd)
            if not _contained(real_parent, workspace_root):
                raise FsBridgeError(-32602, "path is outside the bound workspace root.")
            real_target_for_checks = os.path.join(real_parent, basename)
            if not _contained(real_target_for_checks, workspace_root):
                raise FsBridgeError(-32602, "path is outside the bound workspace root.")
            if _has_technical_component(real_target_for_checks, workspace_root):
                raise FsBridgeError(-32602, "path refers to technical storage (.git/.cortex); refused.")
            mode = 0o600
            try:
                existing_stat = os.stat(basename, dir_fd=dir_fd, follow_symlinks=False)
            except FileNotFoundError:
                existing_stat = None
            except OSError as exc:
                raise FsBridgeError(-32602, f"cannot stat target: {exc}") from exc
            if existing_stat is not None:
                if stat_module.S_ISLNK(existing_stat.st_mode):
                    raise FsBridgeError(-32602, "refusing to write through a symlink target.")
                if not stat_module.S_ISREG(existing_stat.st_mode):
                    raise FsBridgeError(
                        -32602, "target is not a regular file (refusing FIFO/device/socket/directory)."
                    )
                mode = existing_stat.st_mode & 0o777
            data = content.encode("utf-8")
            if len(data) > self._max_bytes:
                raise FsBridgeError(-32602, "content exceeds the maximum writable size.")
            tmp_name = None
            tmp_fd = None
            for _attempt in range(_TEMP_NAME_ATTEMPTS):
                candidate = f".cortex-fswrite-{uuid.uuid4().hex}"
                try:
                    tmp_fd = os.open(
                        candidate, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600, dir_fd=dir_fd,
                    )
                except FileExistsError:
                    continue
                tmp_name = candidate
                break
            if tmp_name is None:
                raise FsBridgeError(-32603, "could not allocate a temp file name.")
            try:
                os.fchmod(tmp_fd, mode)
                with os.fdopen(tmp_fd, "wb") as handle:
                    handle.write(data)
                os.replace(tmp_name, basename, src_dir_fd=dir_fd, dst_dir_fd=dir_fd)
            except OSError as exc:
                try:
                    os.unlink(tmp_name, dir_fd=dir_fd)
                except OSError:
                    pass
                raise FsBridgeError(-32603, f"write failed: {exc}") from exc
        finally:
            os.close(dir_fd)
        self._revision_log.record(
            binding.workspace_binding_id,
            Revision(source="disk", sha256=hashlib.sha256(data).hexdigest(), path=real_target_for_checks),
        )
        return {}


class HostFsForwarder:
    """Forwards ``fs/read_text_file``/``fs/write_text_file`` to the
    *host's* own negotiated fs capability instead of local disk -- unsaved
    editor buffers, not worktree disk content. Only ever selected by the
    caller (``session_manager``) when the host negotiated fs capabilities
    AND ``binding.role == "coordinator"`` AND ``binding.workspace_root``
    equals the host-visible project root (T014 design decision 4); never
    used for a worker binding. Every revision it records is tagged
    ``source="host_buffer"`` and is never compared/equated with
    :class:`FsBridge`'s ``source="disk"`` revisions.
    """

    def __init__(self, *, host_fs_requester, revision_log: RevisionLog | None = None):
        """``host_fs_requester`` -- ``(method, params) -> dict`` -- forwards
        to the actual host over the incoming ACP connection (see
        ``acp.host_requests.HostRequester``)."""
        self._host_fs_requester = host_fs_requester
        self._revision_log = revision_log if revision_log is not None else RevisionLog()

    def read_text_file(self, binding, params) -> dict:
        result = self._host_fs_requester("fs/read_text_file", params) or {}
        content = result.get("content", "")
        self._revision_log.record(
            binding.workspace_binding_id,
            Revision(source="host_buffer", sha256=hashlib.sha256(content.encode("utf-8")).hexdigest(),
                      path=(params or {}).get("path", "")),
        )
        return {"content": content}

    def write_text_file(self, binding, params) -> dict:
        result = self._host_fs_requester("fs/write_text_file", params) or {}
        content = (params or {}).get("content", "")
        self._revision_log.record(
            binding.workspace_binding_id,
            Revision(source="host_buffer", sha256=hashlib.sha256(content.encode("utf-8")).hexdigest(),
                      path=(params or {}).get("path", "")),
        )
        return result
