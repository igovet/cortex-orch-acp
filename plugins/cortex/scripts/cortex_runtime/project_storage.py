"""Resolve one project-local store path, purely from a caller-supplied root."""
from __future__ import annotations

import fcntl
import os
from pathlib import Path
import re
import stat

from .contracts import StoreError
from .portable.identity import WorkspaceBinding, new_id


def canonical_project(value):
    try:
        project = Path(value)
        if (not project.is_absolute() or not project.is_dir()
                or str(project.resolve(strict=True)) != str(project)):
            raise ValueError
        return str(project)
    except (OSError, RuntimeError, TypeError, ValueError):
        raise StoreError("invalid_project") from None


def project_store_directory(project_root):
    """Compute the only supported location; neither create nor search directories."""
    return Path(canonical_project(project_root)) / ".cortex"


def _read_short_file(path, limit=4096):
    """Read one small owner-private regular file, never following a symlink."""
    path = Path(path)
    try:
        info = path.lstat()
    except OSError:
        raise StoreError("project_context_conflict") from None
    if stat.S_ISLNK(info.st_mode):
        raise StoreError("unsafe_storage")
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
        raise StoreError("unsafe_storage")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        data = os.read(fd, limit + 1)
    finally:
        os.close(fd)
    if len(data) > limit:
        raise StoreError("project_context_conflict")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        raise StoreError("project_context_conflict") from None


def _resolve_within_uid(base, raw_path):
    """Strictly resolve one candidate path, rejecting any non-owned component.

    ``Path.resolve(strict=True)`` performs the actual OS-level, symlink-aware
    canonicalization (matching the task's "strict resolve" requirement), so
    the returned path itself cannot contain a symlink component at the
    moment of the call. Following the same convention already used by
    ``store.private_directory`` for owner-private project paths, ancestor
    components need only be real directories; the ``lstat`` walk over them
    is defense-in-depth against a TOCTOU swap of an ancestor between the
    ``resolve()`` call and this check (for example a directory replaced by a
    symlink an instant later), not a claim that ``resolve()`` itself could
    leave a symlink behind. The resolved leaf -- the one component this call
    is actually trusting as the next step of the worktree chain -- must
    additionally be owned by the current uid. System ancestors such as
    ``/tmp`` are commonly owned by another uid and are not themselves part
    of the trusted worktree metadata.
    """
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        candidate = base / candidate
    try:
        resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError):
        raise StoreError("project_context_conflict") from None
    current = Path(resolved.anchor)
    for part in resolved.parts[1:-1]:
        current = current / part
        info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise StoreError("unsafe_storage")
    leaf_info = resolved.lstat()
    if stat.S_ISLNK(leaf_info.st_mode) or leaf_info.st_uid != os.getuid():
        raise StoreError("unsafe_storage")
    return resolved


def logical_project_root(candidate):
    """Return the canonical logical project root for one worktree-aware path.

    A plain (non-Git) directory and an ordinary main checkout are their own
    logical root. A verified linked Git worktree (created by
    ``git worktree add``) resolves to its shared main repository directory so
    both share one project store. Any ambiguous, forged, or unsafe
    Git-worktree layout is rejected without ever falling back to a
    latest/nearest guess; a submodule ``.git`` file (which has no
    ``commondir``) is likewise rejected, not treated as a worktree.
    """
    candidate = Path(canonical_project(candidate))
    marker = candidate / ".git"
    try:
        info = marker.lstat()
    except FileNotFoundError:
        return str(candidate)
    if stat.S_ISLNK(info.st_mode):
        raise StoreError("unsafe_storage")
    if stat.S_ISDIR(info.st_mode):
        if info.st_uid != os.getuid():
            raise StoreError("unsafe_storage")
        return str(candidate)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
        raise StoreError("unsafe_storage")

    lines = _read_short_file(marker).splitlines()
    if len(lines) != 1:
        raise StoreError("project_context_conflict")
    match = re.fullmatch(r"gitdir: (.+)", lines[0])
    if match is None:
        raise StoreError("project_context_conflict")
    gitdir = _resolve_within_uid(candidate, match.group(1))
    if not gitdir.is_dir():
        raise StoreError("project_context_conflict")

    common_lines = _read_short_file(gitdir / "commondir").splitlines()
    if len(common_lines) != 1:
        raise StoreError("project_context_conflict")
    common = _resolve_within_uid(gitdir, common_lines[0])
    if (not common.is_dir() or not (common / "HEAD").exists()
            or not (common / "worktrees").is_dir()):
        raise StoreError("project_context_conflict")

    # Anti-forgery: the worktree metadata directory must itself point back to
    # this exact candidate's ".git" file, and must live directly under the
    # common directory's own "worktrees/" (never an unrelated repository's).
    backlink_lines = _read_short_file(gitdir / "gitdir").splitlines()
    if len(backlink_lines) != 1:
        raise StoreError("project_context_conflict")
    backlink = _resolve_within_uid(gitdir, backlink_lines[0])
    if backlink != marker.resolve():
        raise StoreError("project_context_conflict")
    if gitdir.parent.name != "worktrees" or gitdir.parent.parent != common:
        raise StoreError("project_context_conflict")

    project = _resolve_within_uid(common, "..")
    if _resolve_within_uid(project, ".git") != common:
        raise StoreError("project_context_conflict")
    return str(canonical_project(project))


class StorageLocator:
    """Compute canonical, project-scoped storage paths for one logical project.

    ``store_dir``/``sqlite_path``/``lock_path`` only compute locations; they
    never create directories. ``draft_dir``/``task_dir`` delegate to
    ``store.project_draft_directory``/``store.project_task_directory``, which
    DO create the owner-private directories they return (imported lazily to
    avoid a module cycle with ``store.py``, matching the existing convention
    in this module).
    """

    def __init__(self, project_root):
        self.project_root = Path(canonical_project(project_root))

    def store_dir(self):
        return project_store_directory(str(self.project_root))

    def sqlite_path(self):
        return self.store_dir() / "cortex.sqlite3"

    def lock_path(self):
        return self.store_dir() / ".access.lock"

    def draft_dir(self, kind="report"):
        from .store import project_draft_directory
        return project_draft_directory(str(self.project_root), kind)

    def task_dir(self):
        from .store import project_task_directory
        return project_task_directory(str(self.project_root))

    def verify_workspace(self, candidate):
        """Return the canonical candidate path, or fail closed.

        Rejects any candidate whose logical project root differs from this
        locator's bound project, including an unrelated or forged Git
        worktree. Never falls back to the latest or nearest task.
        """
        canonical_candidate = Path(canonical_project(candidate))
        if logical_project_root(canonical_candidate) != str(self.project_root):
            raise StoreError("project_context_conflict")
        return canonical_candidate


def bind_workspace(project_root, task_id, workspace_root, known_base_revision=None):
    """Bind one execution workspace and base revision to its logical project.

    The caller supplies the task's already-bound ``project_root``; this
    function never reads native/Codex state to discover it. No persistence
    is performed here -- the caller stores the returned ``WorkspaceBinding``
    through its own channel.
    """
    verified = StorageLocator(project_root).verify_workspace(workspace_root)
    return WorkspaceBinding(id=new_id("wb_"), task_id=task_id,
                             workspace_root=str(verified),
                             known_base_revision=known_base_revision)


class StorageAccess:
    """Path-scoped classification of a project's own storage tree.

    Classification is lexical and non-following: it never resolves symlinks
    and rejects any ``..`` traversal component. It decides only which broad
    channel a path belongs to; it never itself grants access. In particular,
    "own_draft_area" only means the path has the shape of a draft file under
    this project's store -- the actual per-draft ownership check (whether
    *this* caller's thread owns *that* draft) remains the existing SQL
    ``owner_thread_id`` query in the store; this classifier must not be
    treated as a substitute for it.
    """

    _DRAFT_KINDS = ("draft-reports", "pipeline-drafts")
    _TASK_DIRECTORY = re.compile(r"t_[0-9a-f]{12}")

    def __init__(self, project_root):
        self.locator = StorageLocator(project_root)

    def classify(self, path):
        store = self.locator.store_dir()
        candidate = Path(path)
        if not candidate.is_absolute():
            return "outside_store"
        try:
            relative = candidate.relative_to(store)
        except ValueError:
            return "outside_store"
        parts = relative.parts
        if not parts or ".." in parts:
            return "outside_store"
        current = store
        for part in parts:
            current = current / part
            try:
                info = current.lstat()
            except OSError:
                break
            if stat.S_ISLNK(info.st_mode):
                return "outside_store"
        if len(parts) == 2 and parts[0] in self._DRAFT_KINDS and parts[1].endswith(".md"):
            return "own_draft_area"
        if (len(parts) == 2 and self._TASK_DIRECTORY.fullmatch(parts[0])
                and parts[1].endswith(".md")):
            return "readable_publication"
        return "runtime_metadata"

    def require_writable_draft(self, path):
        """Reject any path outside the shape of the project's own draft area."""
        if self.classify(path) != "own_draft_area":
            raise StoreError("project_context_conflict")
        return Path(path)


def _private_existing_directory(path, error="unsafe_storage"):
    """Validate one existing owner-private directory without following links."""
    path = Path(path)
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if (stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode)
            or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077):
        raise StoreError(error)
    return path


def _private_tree(path):
    """Validate an opaque published-task tree before an atomic directory move."""
    for root, directories, files in os.walk(path, topdown=True, followlinks=False):
        root_info = Path(root).lstat()
        if (stat.S_ISLNK(root_info.st_mode) or root_info.st_uid != os.getuid()
                or stat.S_IMODE(root_info.st_mode) & 0o077):
            raise StoreError("unsafe_storage")
        for name in directories + files:
            info = (Path(root) / name).lstat()
            if stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid():
                raise StoreError("unsafe_storage")
            if stat.S_ISDIR(info.st_mode):
                if stat.S_IMODE(info.st_mode) & 0o077:
                    raise StoreError("unsafe_storage")
            elif (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                  or stat.S_IMODE(info.st_mode) & 0o077):
                raise StoreError("unsafe_storage")


def migrate_legacy_project_storage(project_root):
    """Move recognized legacy project storage once; never read it as a fallback."""
    project = Path(canonical_project(project_root))
    source = project / ".codex" / "cortex"
    if _private_existing_directory(source) is None:
        return False
    from .store import private_directory, regular, sync_directory
    database_names = (
        "cortex.sqlite3", "cortex.sqlite3-wal", "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
    )
    source_database = [source / name for name in database_names
                       if (source / name).exists() or (source / name).is_symlink()]
    source_tasks = sorted(path for path in source.iterdir()
                          if re.fullmatch(r"t_[0-9a-f]{12}", path.name))
    if not source_database and not source_tasks:
        return False
    destination = private_directory(project / ".cortex")
    source_lock_path = source / ".access.lock"
    destination_lock_path = destination / ".access.lock"
    source_lock = os.open(source_lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    destination_lock = os.open(destination_lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    moved = False
    try:
        regular(source_lock_path); regular(destination_lock_path)
        try:
            fcntl.flock(source_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(destination_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (BlockingIOError, OSError):
            raise StoreError("storage_busy") from None
        destination_database = [destination / name for name in database_names
                                if (destination / name).exists() or (destination / name).is_symlink()]
        for path in source_database:
            regular(path)
        for path in destination_database:
            regular(path)
        for path in source_tasks:
            if not path.is_dir() or path.is_symlink():
                raise StoreError("unsafe_storage")
            _private_tree(path)
        conflicts = destination_database if source_database else []
        conflicts += [destination / path.name for path in source_tasks
                      if (destination / path.name).exists() or (destination / path.name).is_symlink()]
        if conflicts:
            raise StoreError("legacy_storage_conflict")
        for path in source_database:
            os.replace(path, destination / path.name)
            moved = True
        for path in source_tasks:
            os.replace(path, destination / path.name)
            moved = True
        if moved:
            sync_directory(source); sync_directory(destination)
        return moved
    finally:
        os.close(destination_lock)
        os.close(source_lock)


