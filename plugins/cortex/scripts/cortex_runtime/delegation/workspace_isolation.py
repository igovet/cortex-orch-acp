"""T025: isolated per-attempt workspaces and the serialized shared-root fallback.

Two ``WorkspaceProvisioner`` (see ``delegation.provisioning``) implementations:

* :class:`IsolatedSnapshotWorkspaceProvisioner` -- the default. ``create``
  makes an owner-private *copy* of the project tree (excluding the
  top-level ``.git``/``.cortex`` directories, matching
  ``delegation.revision``'s own exclusion) under this project's own
  ``.cortex`` state storage, one directory per attempt. The copy is a
  read-only snapshot of *whatever is actually on disk right now* --
  including a user's uncommitted/dirty files -- never a ``git reset``,
  ``git stash``, or any other operation that could rewrite the source
  tree; only plain ``os.scandir``/``open``/``read`` calls touch the real
  project root. Two on-disk copies are actually made -- a pristine
  ``base_path`` (read-only for the rest of the attempt's lifetime, never
  handed to an engine) and a ``root_path`` "work" copy the engine actually
  gets as its workspace root and freely mutates -- because
  :mod:`delegation.integration`'s later three-way merge needs the base's
  *raw bytes*, not just the sha256 the manifest alone would retain. Its
  manifest (path -> kind/sha256/mode) is recorded as the attempt's known
  base, alongside a T019 ``delegation.revision.capture_revision``
  observation of ``root_path`` (identical content -> identical tree
  digest, since that digest is computed over relative paths). Two workers
  delegated concurrently each get their own private pair of copies:
  writing into one can never be observed by, or overwrite, the other's
  copy or the main project tree -- only an explicit, later
  :mod:`delegation.integration` call touches the main tree.
* :class:`SerializedSharedRootWorkspaceProvisioner` -- the pre-T025
  behaviour (see ``delegation.live_adapters.SharedProjectRootWorkspaceProvisioner``,
  which this supersedes for a *conflicting* writer), extended with an
  advisory per-project exclusive :meth:`writer_lock` so that when a
  snapshot cannot safely be taken (size/entry caps exceeded, or an
  unreadable entry), conflicting writers are explicitly serialized --
  never run concurrently -- instead of silently sharing one root with no
  protection at all. ``IsolatedSnapshotWorkspaceProvisioner.create`` falls
  back to this mode automatically and records the fallback (``mode`` column)
  so the limitation is visible in durable state, not just a code comment.

Every filesystem check here follows the same conservative, symlink-averse
conventions as ``delegation.revision``/``project_storage`` (``O_NOFOLLOW``,
``O_NONBLOCK`` + ``S_ISREG`` re-check, small caps, no following a symlinked
directory while walking) -- this module is safe to run against a workspace
a previous worker just finished writing into.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
import shutil
import stat
import time
from datetime import datetime, timezone
from pathlib import Path

from ..contracts import StoreError
from ..project_storage import StorageLocator, canonical_project

_MAX_ENTRIES = 50_000
_MAX_TOTAL_BYTES = 512 * 1024 * 1024
_MAX_FILE_BYTES = 64 * 1024 * 1024
_TIME_BUDGET_SECONDS = 20.0
_READ_CHUNK = 1024 * 1024
_EXCLUDED_TOP_LEVEL = {".git", ".cortex"}

WORKSPACE_SCHEMA = '''
CREATE TABLE IF NOT EXISTS workspace_snapshots (
 workspace_binding_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 project_root TEXT NOT NULL,
 root_path TEXT,
 base_path TEXT,
 mode TEXT NOT NULL,
 manifest_json TEXT NOT NULL,
 base_kind TEXT NOT NULL,
 base_head TEXT,
 base_dirty_digest TEXT,
 created_at TEXT NOT NULL,
 cleaned_up_at TEXT,
 cleanup_reason TEXT
);
CREATE INDEX IF NOT EXISTS workspace_snapshots_by_attempt ON workspace_snapshots(attempt_id, created_at);
'''


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


class WorkspaceSnapshotUnavailable(Exception):
    """Raised (and always caught by ``create``) when a full, safe snapshot
    cannot be taken; never partially applied -- see module docstring."""

    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


def _open_regular_nofollow(path) -> int:
    flags = os.O_RDONLY | os.O_NONBLOCK
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    fd = os.open(path, flags)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise OSError(f"{path} is not a regular file")
        try:
            current = fcntl.fcntl(fd, fcntl.F_GETFL)
            fcntl.fcntl(fd, fcntl.F_SETFL, current & ~os.O_NONBLOCK)
        except OSError:
            pass
    except BaseException:
        os.close(fd)
        raise
    return fd


def _copy_regular_file(source_path, dest_path, budget, deadline) -> dict:
    h = hashlib.sha256()
    size = 0
    src_fd = _open_regular_nofollow(source_path)
    try:
        mode = stat.S_IMODE(os.fstat(src_fd).st_mode)
        dest_fd = os.open(dest_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            while True:
                if time.monotonic() > deadline:
                    raise WorkspaceSnapshotUnavailable("capture_timed_out")
                chunk = os.read(src_fd, _READ_CHUNK)
                if not chunk:
                    break
                size += len(chunk)
                if size > _MAX_FILE_BYTES:
                    raise WorkspaceSnapshotUnavailable("workspace_too_large")
                budget["total_bytes"] += len(chunk)
                if budget["total_bytes"] > _MAX_TOTAL_BYTES:
                    raise WorkspaceSnapshotUnavailable("workspace_too_large")
                h.update(chunk)
                os.write(dest_fd, chunk)
            os.fchmod(dest_fd, mode)
        finally:
            os.close(dest_fd)
    except OSError as error:
        raise WorkspaceSnapshotUnavailable(f"unreadable_entry:{error}") from error
    finally:
        os.close(src_fd)
    return dict(kind="f", sha256=h.hexdigest(), mode=mode, size=size)


def _walk_and_copy(src_root, dest_root, current, budget, deadline, manifest, *, top: bool) -> None:
    src_dir = os.path.join(src_root, current) if current else src_root
    dest_dir = os.path.join(dest_root, current) if current else dest_root
    try:
        entries = list(os.scandir(src_dir))
    except OSError as error:
        raise WorkspaceSnapshotUnavailable(f"unreadable_entry:{src_dir}:{error}") from error
    for entry in entries:
        if time.monotonic() > deadline:
            raise WorkspaceSnapshotUnavailable("capture_timed_out")
        relpath = f"{current}/{entry.name}" if current else entry.name
        if top and entry.name in _EXCLUDED_TOP_LEVEL:
            continue
        budget["entries"] += 1
        if budget["entries"] > _MAX_ENTRIES:
            raise WorkspaceSnapshotUnavailable("workspace_too_large")
        dest_path = os.path.join(dest_root, relpath)
        if entry.is_symlink():
            try:
                target = os.readlink(entry.path)
                mode = stat.S_IMODE(entry.stat(follow_symlinks=False).st_mode)
            except OSError as error:
                raise WorkspaceSnapshotUnavailable(f"unreadable_entry:{entry.path}:{error}") from error
            os.symlink(target, dest_path)
            manifest[relpath] = dict(kind="l", target=target, mode=mode)
            continue
        try:
            info = entry.stat(follow_symlinks=False)
        except OSError as error:
            raise WorkspaceSnapshotUnavailable(f"unreadable_entry:{entry.path}:{error}") from error
        if stat.S_ISDIR(info.st_mode):
            mode = stat.S_IMODE(info.st_mode) or 0o700
            os.mkdir(dest_path, 0o700)
            manifest[relpath] = dict(kind="d", mode=mode)
            _walk_and_copy(src_root, dest_root, relpath, budget, deadline, manifest, top=False)
        elif stat.S_ISREG(info.st_mode):
            copied = _copy_regular_file(entry.path, dest_path, budget, deadline)
            manifest[relpath] = copied
        else:
            # FIFOs/sockets/devices: never represented in a snapshot -- the
            # whole capture is unavailable (T019 precedent: a silently
            # incomplete snapshot could hide a change).
            raise WorkspaceSnapshotUnavailable(f"unsupported_entry_kind:{entry.path}")


def capture_and_copy_workspace(project_root: str, dest_root: str) -> dict:
    """Copy ``project_root`` (excluding top-level ``.git``/``.cortex``) into
    the not-yet-existing ``dest_root``, returning a manifest
    ``{relpath: {"kind": "f"|"d"|"l", ...}}``. Raises
    :class:`WorkspaceSnapshotUnavailable` (never partially leaves a usable
    ``dest_root`` -- the caller removes it) if any cap is exceeded or any
    entry cannot be safely read/represented."""
    os.mkdir(dest_root, 0o700)
    budget = dict(entries=0, total_bytes=0)
    deadline = time.monotonic() + _TIME_BUDGET_SECONDS
    manifest: dict = {}
    _walk_and_copy(project_root, dest_root, "", budget, deadline, manifest, top=True)
    return manifest


def _remove_owned_tree(root) -> None:
    path = Path(root)
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise StoreError("unsafe_storage")
    if not shutil.rmtree.avoids_symlink_attacks:
        raise StoreError("unsafe_storage")
    shutil.rmtree(path)


class IsolatedSnapshotWorkspaceProvisioner:
    """``delegation.provisioning.WorkspaceProvisioner`` (create/lookup/remove
    keyed by attempt id) backed by a private snapshot copy. See module
    docstring."""

    def __init__(self, store, project_root: str, task_id: str | None = None):
        """``task_id`` is optional: a single provisioner instance is shared
        across every task of one project (see ``acp.production_runtime``,
        which builds one ``OrchestratingRuntime``/provisioner per project
        root, not per task -- ``WorkspaceProvisioner.create`` itself is only
        ever called with an ``attempt_id``, never a ``task_id``). When not
        supplied, :meth:`create` resolves it from the already-inserted
        ``delegation_provisioning`` row for that attempt (see
        ``delegation.provisioning.ProvisioningService.delegate``, which
        creates that row -- with its ``task_id`` column already set --
        before ever reaching the ``"workspace"`` step)."""
        self._store = store
        self._project_root = canonical_project(project_root)
        self._task_id = task_id

    def _state_root(self) -> Path:
        from ..store import private_directory

        base = StorageLocator(self._project_root).store_dir() / "workspaces"
        return private_directory(base)

    def _resolve_task_id(self, attempt_id: str) -> str:
        if self._task_id is not None:
            return self._task_id
        with self._store.connection() as db:
            row = db.execute(
                "SELECT task_id FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
            ).fetchone()
        if row is None:
            raise StoreError("workspace_task_unresolved")
        return row["task_id"]

    def create(self, attempt_id: str) -> str:
        from ..portable.identity import new_id
        from . import revision as delegation_revision

        task_id = self._resolve_task_id(attempt_id)
        binding_id = new_id("wb_")
        state_root = self._state_root()
        base_dest = state_root / attempt_id / "base"
        work_dest = state_root / attempt_id / "work"
        try:
            os.mkdir(state_root / attempt_id, 0o700)
            manifest = capture_and_copy_workspace(self._project_root, str(base_dest))
            shutil.copytree(base_dest, work_dest, symlinks=True)
        except (WorkspaceSnapshotUnavailable, OSError) as error:
            shutil.rmtree(state_root / attempt_id, ignore_errors=True)
            self._record(
                binding_id, attempt_id, task_id, mode="serialized_shared_root", root_path=None, base_path=None,
                manifest={}, base=dict(kind="unavailable", head=None, dirty_digest=None,
                                        detail=str(getattr(error, "reason", error))),
            )
            return binding_id
        base = delegation_revision.capture_revision(str(work_dest))
        self._record(binding_id, attempt_id, task_id, mode="isolated_snapshot", root_path=str(work_dest),
                      base_path=str(base_dest), manifest=manifest, base=base)
        return binding_id

    def _record(self, binding_id, attempt_id, task_id, *, mode, root_path, base_path, manifest, base) -> None:
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "INSERT INTO workspace_snapshots VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    (binding_id, task_id, attempt_id, self._project_root, root_path, base_path, mode,
                     json.dumps(manifest, sort_keys=True), base.get("kind"), base.get("head"),
                     base.get("dirty_digest"), _now(), None, None),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _row_for_attempt(self, attempt_id: str):
        with self._store.connection() as db:
            return db.execute(
                "SELECT * FROM workspace_snapshots WHERE attempt_id=? AND cleaned_up_at IS NULL "
                "ORDER BY created_at DESC LIMIT 1", (attempt_id,),
            ).fetchone()

    def lookup(self, attempt_id: str) -> str:
        row = self._row_for_attempt(attempt_id)
        if row is None:
            return "absent"
        if row["mode"] == "serialized_shared_root":
            return "present"
        root = row["root_path"]
        if root is None:
            return "absent"
        try:
            info = os.lstat(root)
        except OSError:
            return "absent"
        if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode) and info.st_uid == os.getuid():
            return "present"
        return "absent"

    def remove(self, attempt_id: str) -> None:
        row = self._row_for_attempt(attempt_id)
        if row is None:
            return
        self.cleanup_binding(row["workspace_binding_id"], reason="removed")

    def row_for_binding(self, binding_id: str):
        with self._store.connection() as db:
            return db.execute(
                "SELECT * FROM workspace_snapshots WHERE workspace_binding_id=?", (binding_id,),
            ).fetchone()

    def root_for(self, binding_id: str):
        row = self.row_for_binding(binding_id)
        if row is None:
            return None
        if row["mode"] == "serialized_shared_root":
            return row["project_root"]
        return row["root_path"]

    def base_root_for(self, binding_id: str):
        row = self.row_for_binding(binding_id)
        if row is None:
            return None
        return row["base_path"]

    def manifest_for(self, binding_id: str) -> dict:
        row = self.row_for_binding(binding_id)
        if row is None:
            return {}
        return json.loads(row["manifest_json"])

    def cleanup_binding(self, binding_id: str, *, reason: str) -> bool:
        """Remove only a Cortex-owned snapshot workspace, and only after the
        caller has confirmed its lifecycle is terminal (assignment terminal
        and any integration recorded, or cancelled) -- this method itself
        performs no lifecycle check; see ``cleanup_terminal_attempt``. Never
        touches ``project_root`` or any user-owned path."""
        row = self.row_for_binding(binding_id)
        if row is None or row["cleaned_up_at"] is not None:
            return False
        state_root = self._state_root()
        for path in (row["root_path"], row["base_path"]):
            if path is not None:
                candidate = Path(path)
                try:
                    candidate.relative_to(state_root)
                except ValueError:
                    raise StoreError("unsafe_storage") from None
        for path in (row["root_path"], row["base_path"]):
            if path is not None:
                _remove_owned_tree(path)
        attempt_dir = state_root / row["attempt_id"]
        if attempt_dir.exists():
            try:
                attempt_dir.rmdir()
            except OSError:
                pass
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "UPDATE workspace_snapshots SET cleaned_up_at=?, cleanup_reason=? WHERE workspace_binding_id=?",
                    (_now(), reason, binding_id),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        return True


def resolve_attempt_root(store, task_id: str, attempt_id: str):
    """Best-effort: the isolated snapshot root actually bound to
    ``attempt_id`` under ``task_id`` right now, or ``None`` if this attempt
    never used isolation (serialized fallback, or no snapshot ever taken --
    the caller falls back to the task's shared ``project_root``)."""
    with store.connection() as db:
        row = db.execute(
            "SELECT root_path, mode FROM workspace_snapshots WHERE task_id=? AND attempt_id=? "
            "AND cleaned_up_at IS NULL ORDER BY created_at DESC LIMIT 1",
            (task_id, attempt_id),
        ).fetchone()
    if row is None or row["mode"] != "isolated_snapshot":
        return None
    return row["root_path"]


def resolve_assignment_root(store, task_id: str, assignment_ref: str):
    """Like :func:`resolve_attempt_root`, but keyed by an
    ``assignment_ref`` (``delegation_provisioning.idempotency_key`` -- what
    ``task_events.assignment_ref``/``attempt_revisions.attempt_id`` actually
    store for a worker, distinct from the provisioning row's own
    ``attempt_id`` column; see ``delegation.live_adapters``'s
    ``_open_generation`` docstring for the same distinction). Used by
    ``acp.orchestrating_runtime._capture_result_revisions`` so a
    ``worker_result``'s captured revision (T019/T025 "revision A") reflects
    the attempt's own isolated workspace when isolation was used, instead
    of always the task's shared project root."""
    with store.connection() as db:
        row = db.execute(
            "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?",
            (assignment_ref,),
        ).fetchone()
    if row is None or row["attempt_id"] is None:
        return None
    return resolve_attempt_root(store, task_id, row["attempt_id"])


class _WriterLock:
    def __init__(self, path, timeout=None):
        self._path = path
        self._timeout = timeout
        self._fd = None

    def __enter__(self):
        fd = os.open(self._path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        deadline = None if self._timeout is None else time.monotonic() + self._timeout
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if deadline is not None and time.monotonic() > deadline:
                    os.close(fd)
                    raise StoreError("workspace_writer_busy") from None
                time.sleep(0.02)
        self._fd = fd
        return self

    def __exit__(self, exc_type, exc, tb):
        fcntl.flock(self._fd, fcntl.LOCK_UN)
        os.close(self._fd)
        return False


class SerializedSharedRootWorkspaceProvisioner:
    """Non-isolated fallback ``WorkspaceProvisioner``: every attempt shares
    ``project_root``, but :meth:`writer_lock` gives callers an explicit,
    visible, cross-process mutual-exclusion primitive for conflicting
    writers -- prompt-level file scopes alone are never a lock (task card
    step 2). See module docstring."""

    def __init__(self, project_root: str):
        self._project_root = canonical_project(project_root)

    def create(self, attempt_id: str) -> str:
        return "shared_project_root"

    def lookup(self, attempt_id: str) -> str:
        return "present"

    def remove(self, attempt_id: str) -> None:
        return None

    def _lock_path(self) -> Path:
        from ..store import private_directory

        store_dir = private_directory(StorageLocator(self._project_root).store_dir())
        return store_dir / ".workspace-writer.lock"

    def writer_lock(self, *, timeout=None) -> _WriterLock:
        """A per-project exclusive advisory lock: ``with provisioner.writer_lock():``
        serializes every conflicting writer against this one shared root
        (never two at once), across threads and processes alike."""
        return _WriterLock(self._lock_path(), timeout=timeout)
