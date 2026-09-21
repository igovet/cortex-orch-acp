"""T019 design point 4: observed (never claimed) artifact-revision binding.

``capture_revision`` observes a workspace root with pure, read-only Python
filesystem calls -- it never shells out to ``git`` or any other
repo-configurable program. An earlier revision of this module ran
``git status``/``git rev-parse`` against the shared workspace; a repo-local
``.git/config`` (``core.fsmonitor``, ``core.hooksPath``) or
``.gitattributes`` (clean/smudge filters) can make plain git plumbing
execute an arbitrary program chosen by whoever last wrote into that
workspace -- including a delegated worker. Because a revision capture must
be safe to run against a workspace a worker just finished writing into,
this module now never invokes git, never opens a file through a symlink
(``O_NOFOLLOW``), and never follows a symlinked directory when walking the
tree. The result is *stricter* than ``git status`` (git-ignored build
output changes now count as a workspace change); that is intentional and
documented below, not a bug.

A capture has two independent parts:

* ``head`` -- best-effort HEAD resolution by parsing ``<root>/.git``
  (a directory, or a ``gitdir:`` pointer file for a worktree), the ``HEAD``
  file, a loose ref, or ``packed-refs``, all with strict format validation
  and small size caps. Missing or malformed input simply leaves
  ``head=None`` (with ``head_detail`` explaining why) -- it never makes the
  whole capture ``unavailable``, since HEAD is optional context, not the
  thing that makes a capture trustworthy.
* the tree digest (stored in the ``dirty_digest`` column) and its bounded
  sorted manifest (stored in ``manifest_json``) -- a
  sha256 over a sorted, bounded manifest of every file/dir/symlink/other
  entry under the workspace root (excluding the top-level ``.git`` and
  ``.cortex`` directories). Any unreadable entry, any cap exceeded (file
  count, total bytes hashed, per-file size, or wall-clock time budget)
  makes the *whole* capture ``kind="unavailable"`` -- never a partial or
  best-effort digest, since a silently-incomplete digest could hide a
  change.

``record_attempt_revision`` durably stores one capture (see the additive
``attempt_revisions`` table in ``store.SCHEMA``), tagged with the
capturing ``phase`` (for example ``"worker_result:<event id>"`` or
``"gate_acceptance"``) -- multiple captures accumulate; nothing is ever
overwritten, so the full observation history for one attempt is auditable.

Because every delegated attempt in this runtime shares the task's one
project root (see ``delegation.live_adapters.SharedProjectRootWorkspaceProvisioner``
-- T025 owns real per-attempt isolation), a revision captured for one
attempt and a revision captured later for the completion gate are
comparisons of the *same* workspace root at two different instants: any
concurrent change by a different worker (or the coordinator, or the user)
invalidates every other attempt's already-recorded observed checks just as
much as a change by the attempt's own worker would. This is a known,
documented limitation of the shared-root design, not a bug in this module.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import stat
import time
from datetime import datetime, timezone

_MAX_FILES = 50_000
_MAX_TOTAL_HASH_BYTES = 512 * 1024 * 1024
_MAX_FILE_HASH_BYTES = 64 * 1024 * 1024
_TIME_BUDGET_SECONDS = 20.0
_READ_CHUNK = 1024 * 1024

_HEAD_FILE_CAP = 4096
_GITDIR_FILE_CAP = 4096
_REF_FILE_CAP = 4096
_PACKED_REFS_CAP = 8 * 1024 * 1024

_SHA_RE = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")  # sha-1 (40) or sha-256 (64)
_REF_NAME_RE = re.compile(r"^[A-Za-z0-9._/-]+$")

_EXCLUDED_TOP_LEVEL = {".git", ".cortex"}

# Claude's ACP runtime creates this short-lived lock while it services
# scheduled-task bookkeeping.  It is engine-owned housekeeping, not a
# project artifact: the file may appear between worker publication and the
# acceptance recheck, or disappear before the recheck.  Keep the exemption
# deliberately path-exact.  The rest of ``.claude`` remains part of the
# observed project tree, so a project-owned settings file or any other
# Claude configuration still invalidates a revision when it changes.
_ENGINE_VOLATILE_PATHS = frozenset({".claude/scheduled_tasks.lock"})


def _is_engine_volatile_path(relpath: str) -> bool:
    """Return whether *relpath* is one verified engine housekeeping path.

    This is intentionally an explicit allowlist rather than a ``.claude``
    directory rule.  Broadly ignoring engine-looking directories would hide
    legitimate project content and weaken unknown-drift detection.
    """
    return relpath in _ENGINE_VOLATILE_PATHS


def _same_node_state(before, after) -> bool:
    """Return whether a node remained the same observable filesystem object."""
    return (
        (before.st_dev, before.st_ino, before.st_mode, before.st_size,
         getattr(before, "st_mtime_ns", 0), getattr(before, "st_ctime_ns", 0))
        ==
        (after.st_dev, after.st_ino, after.st_mode, after.st_size,
         getattr(after, "st_mtime_ns", 0), getattr(after, "st_ctime_ns", 0))
    )


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


class _Unavailable(Exception):
    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


def _open_regular_nofollow(path: str) -> int:
    """Open ``path`` for reading, refusing to follow a final symlink
    component and refusing to ever block on it. A file classified as a
    regular file by ``os.scandir``/``lstat`` can be swapped in place (TOCTOU)
    for a FIFO/device before this open runs; opening with ``O_NONBLOCK``
    makes an open against a FIFO with no writer return immediately instead
    of hanging (which would otherwise silently defeat the wall-clock
    budget and hang the whole capture, and transitively the completion
    gate). The immediate ``fstat``+``S_ISREG`` check below then refuses
    outright rather than ever attempting to read a non-regular file that
    slipped through. ``O_CLOEXEC`` keeps the fd out of any child process."""
    flags = os.O_RDONLY | os.O_NONBLOCK
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    fd = os.open(path, flags)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise OSError(f"{path} is not a regular file (changed after classification)")
        # Regular files ignore O_NONBLOCK for I/O purposes per POSIX, but
        # clear it anyway now that S_ISREG is confirmed, for a normal
        # blocking read loop below.
        try:
            current_flags = fcntl.fcntl(fd, fcntl.F_GETFL)
            fcntl.fcntl(fd, fcntl.F_SETFL, current_flags & ~os.O_NONBLOCK)
        except OSError:
            pass
    except BaseException:
        os.close(fd)
        raise
    return fd


def _open_directory_nofollow(path: str, expected=None) -> int:
    """Open a directory without following its final component.

    The ``lstat``/``scandir(path)`` pair is otherwise vulnerable to a worker
    replacing a directory with a symlink between those calls.  When an
    identity was observed from a parent directory entry, require the opened
    directory to have the same device/inode before scanning it.
    """
    flags = os.O_RDONLY
    if hasattr(os, "O_DIRECTORY"):
        flags |= os.O_DIRECTORY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    fd = os.open(path, flags)
    try:
        actual = os.fstat(fd)
        if not stat.S_ISDIR(actual.st_mode):
            raise OSError(f"{path} is not a directory")
        if expected is not None and (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino):
            raise OSError(f"{path} changed while opening")
    except BaseException:
        os.close(fd)
        raise
    return fd


def _open_directory_at(parent_fd: int, name: str, display_path: str, expected=None) -> int:
    flags = os.O_RDONLY
    if hasattr(os, "O_DIRECTORY"):
        flags |= os.O_DIRECTORY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    fd = os.open(name, flags, dir_fd=parent_fd)
    try:
        actual = os.fstat(fd)
        if not stat.S_ISDIR(actual.st_mode):
            raise OSError(f"{display_path} is not a directory")
        if expected is not None and (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino):
            raise OSError(f"{display_path} changed while opening")
    except BaseException:
        os.close(fd)
        raise
    return fd


def _open_regular_nofollow_at(parent_fd: int, name: str, display_path: str, expected=None) -> int:
    flags = os.O_RDONLY | os.O_NONBLOCK
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    fd = os.open(name, flags, dir_fd=parent_fd)
    try:
        actual = os.fstat(fd)
        if not stat.S_ISREG(actual.st_mode):
            raise OSError(f"{display_path} is not a regular file")
        if expected is not None and (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino):
            raise OSError(f"{display_path} changed while opening")
        try:
            current_flags = fcntl.fcntl(fd, fcntl.F_GETFL)
            fcntl.fcntl(fd, fcntl.F_SETFL, current_flags & ~os.O_NONBLOCK)
        except OSError:
            pass
    except BaseException:
        os.close(fd)
        raise
    return fd


def _open_nofollow(path: str, cap: int) -> bytes:
    """Open ``path`` refusing to follow a final symlink component or to
    ever be a non-regular file (see :func:`_open_regular_nofollow`), and
    read at most ``cap`` bytes (a short read never silently succeeds as if
    it were the whole file -- callers treat a file at/over the cap as
    invalid/too large for the small metadata files this is used for)."""
    fd = _open_regular_nofollow(path)
    try:
        data = os.read(fd, cap + 1)
    finally:
        os.close(fd)
    if len(data) > cap:
        raise ValueError("file exceeds size cap")
    return data


def _resolve_gitdir(root) -> str | None:
    """Return the path of the ``.git`` directory for ``root``, or ``None``
    if it cannot be determined safely. Handles both a plain ``.git``
    directory and a worktree ``gitdir:`` pointer file."""
    dot_git = os.path.join(str(root), ".git")
    try:
        st = os.lstat(dot_git)
    except OSError:
        return None
    if stat.S_ISDIR(st.st_mode):
        return dot_git
    if stat.S_ISREG(st.st_mode):
        try:
            data = _open_nofollow(dot_git, _GITDIR_FILE_CAP)
        except (OSError, ValueError):
            return None
        text = data.decode("utf-8", "replace").strip()
        if not text.startswith("gitdir:"):
            return None
        target = text[len("gitdir:"):].strip()
        if not target:
            return None
        resolved = target if os.path.isabs(target) else os.path.normpath(os.path.join(str(root), target))
        try:
            target_st = os.lstat(resolved)
        except OSError:
            return None
        if not stat.S_ISDIR(target_st.st_mode):
            return None
        return resolved
    return None


def _resolve_common_dir(gitdir: str) -> str:
    """Branches (unlike a per-worktree HEAD) live in the *common* git dir,
    shared across all of a repo's worktrees; a linked worktree's own gitdir
    names it via a ``commondir`` file. Falls back to ``gitdir`` itself for
    a normal (non-worktree) repository."""
    commondir_path = os.path.join(gitdir, "commondir")
    try:
        data = _open_nofollow(commondir_path, _GITDIR_FILE_CAP)
    except (OSError, ValueError):
        return gitdir
    target = data.decode("utf-8", "replace").strip()
    if not target:
        return gitdir
    resolved = target if os.path.isabs(target) else os.path.normpath(os.path.join(gitdir, target))
    try:
        st = os.lstat(resolved)
    except OSError:
        return gitdir
    if not stat.S_ISDIR(st.st_mode):
        return gitdir
    return resolved


def _read_ref_target(gitdir: str, ref_name: str) -> str | None:
    """A loose ref file, falling back to ``packed-refs``."""
    if not _REF_NAME_RE.match(ref_name) or ".." in ref_name.split("/"):
        return None
    loose = os.path.join(gitdir, *ref_name.split("/"))
    try:
        data = _open_nofollow(loose, _REF_FILE_CAP)
    except FileNotFoundError:
        data = None
    except (OSError, ValueError):
        data = None
    if data is not None:
        candidate = data.decode("utf-8", "replace").strip()
        if _SHA_RE.match(candidate):
            return candidate
    packed = os.path.join(gitdir, "packed-refs")
    try:
        data = _open_nofollow(packed, _PACKED_REFS_CAP)
    except (OSError, ValueError):
        return None
    for line in data.decode("utf-8", "replace").splitlines():
        if not line or line.startswith("#") or line.startswith("^"):
            continue
        parts = line.split(" ", 1)
        if len(parts) != 2:
            continue
        sha, name = parts[0], parts[1].strip()
        if name == ref_name and _SHA_RE.match(sha):
            return sha
    return None


def _read_head(root) -> tuple[str | None, str | None]:
    """Best-effort HEAD resolution. Returns ``(sha_or_none, detail)``;
    ``detail`` is only set when ``sha`` is ``None``."""
    gitdir = _resolve_gitdir(root)
    if gitdir is None:
        return None, "no .git found"
    head_path = os.path.join(gitdir, "HEAD")
    try:
        data = _open_nofollow(head_path, _HEAD_FILE_CAP)
    except (OSError, ValueError) as error:
        return None, f"HEAD unreadable: {error}"
    text = data.decode("utf-8", "replace").strip()
    if text.startswith("ref:"):
        ref_name = text[len("ref:"):].strip()
        common_dir = _resolve_common_dir(gitdir)
        sha = _read_ref_target(common_dir, ref_name)
        if sha is None:
            return None, f"ref {ref_name!r} not resolvable"
        return sha, None
    if _SHA_RE.match(text):
        return text, None
    return None, "HEAD content is not a valid ref/sha"


def _classify(entry: os.DirEntry) -> str:
    if entry.is_symlink():
        return "l"
    try:
        st = entry.stat(follow_symlinks=False)
    except OSError as error:
        raise _Unavailable(f"unreadable_entry:{entry.path}:{error}") from error
    mode = st.st_mode
    if stat.S_ISREG(mode):
        return "f"
    if stat.S_ISDIR(mode):
        return "d"
    if stat.S_ISFIFO(mode):
        return "p"
    if stat.S_ISSOCK(mode):
        return "s"
    if stat.S_ISBLK(mode) or stat.S_ISCHR(mode):
        return "dev"
    return "o"


def _hash_file(path: str, budget: dict, deadline: float, *, parent_fd=None, name=None, expected=None) -> str:
    h = hashlib.sha256()
    size = 0
    try:
        fd = (_open_regular_nofollow_at(parent_fd, name, path, expected)
              if parent_fd is not None else _open_regular_nofollow(path))
    except OSError as error:
        raise _Unavailable(f"unreadable_entry:{path}:{error}") from error
    try:
        try:
            opened_st = os.fstat(fd)
        except OSError as error:
            raise _Unavailable(f"unreadable_entry:{path}:{error}") from error
        while True:
            if time.monotonic() > deadline:
                raise _Unavailable("capture_timed_out")
            try:
                chunk = os.read(fd, _READ_CHUNK)
            except OSError as error:
                raise _Unavailable(f"unreadable_entry:{path}:{error}") from error
            if not chunk:
                break
            size += len(chunk)
            if size > _MAX_FILE_HASH_BYTES:
                raise _Unavailable("workspace_too_large")
            budget["total_bytes"] += len(chunk)
            if budget["total_bytes"] > _MAX_TOTAL_HASH_BYTES:
                raise _Unavailable("workspace_too_large")
            h.update(chunk)
        try:
            final_st = os.fstat(fd)
        except OSError as error:
            raise _Unavailable(f"unreadable_entry:{path}:{error}") from error
        if not _same_node_state(opened_st, final_st):
            raise _Unavailable(f"changed_while_reading:{path}")
    finally:
        os.close(fd)
    return h.hexdigest()


def _walk(root: str, budget: dict, deadline: float, records: list, *, parent_fd=None, relprefix="", expected=None) -> None:
    try:
        fd = (_open_directory_at(parent_fd, os.path.basename(root), root, expected)
              if parent_fd is not None else _open_directory_nofollow(root, expected))
    except OSError as error:
        raise _Unavailable(f"unreadable_entry:{root}:{error}") from error
    try:
        try:
            scan_st = os.fstat(fd)
        except OSError as error:
            raise _Unavailable(f"unreadable_entry:{root}:{error}") from error
        with os.scandir(fd) as entries:
            for entry in entries:
                if time.monotonic() > deadline:
                    raise _Unavailable("capture_timed_out")
                relpath = f"{relprefix}/{entry.name}" if relprefix else entry.name
                top = relpath.split("/", 1)[0]
                if not relprefix and top in _EXCLUDED_TOP_LEVEL:
                    continue
                kind = _classify(entry)
                budget["files"] += 1
                if budget["files"] > _MAX_FILES:
                    raise _Unavailable("workspace_too_large")
                display_path = os.path.join(budget["root"], *relpath.split("/"))
                if _is_engine_volatile_path(relpath) and kind == "f":
                    # Exempt only the verified regular lock file.  Open it
                    # relative to the already-open parent and require the
                    # same inode, so a directory/symlink/device at this path
                    # is observed (or a replacement race fails closed) rather
                    # than being silently hidden as engine housekeeping.
                    try:
                        entry_st = entry.stat(follow_symlinks=False)
                        volatile_fd = _open_regular_nofollow_at(
                            fd, entry.name, display_path, expected=entry_st
                        )
                        os.close(volatile_fd)
                    except OSError as error:
                        raise _Unavailable(f"unreadable_entry:{display_path}:{error}") from error
                    continue
                if kind == "f":
                    try:
                        entry_st = entry.stat(follow_symlinks=False)
                    except OSError as error:
                        raise _Unavailable(f"unreadable_entry:{display_path}:{error}") from error
                    digest = _hash_file(display_path, budget, deadline, parent_fd=fd, name=entry.name, expected=entry_st)
                    records.append(f"f\t{relpath}\t{entry_st.st_size}\t{digest}")
                elif kind == "l":
                    try:
                        target = os.readlink(entry.name, dir_fd=fd)
                    except OSError as error:
                        raise _Unavailable(f"unreadable_entry:{display_path}:{error}") from error
                    records.append(f"l\t{relpath}\t-\t{target}")
                elif kind == "d":
                    try:
                        entry_st = entry.stat(follow_symlinks=False)
                    except OSError as error:
                        raise _Unavailable(f"unreadable_entry:{display_path}:{error}") from error
                    records.append(f"d\t{relpath}\t-\t-")
                    _walk(display_path, budget, deadline, records, parent_fd=fd, relprefix=relpath, expected=entry_st)
                else:
                    records.append(f"{kind}\t{relpath}\t-\t-")
        try:
            final_st = os.fstat(fd)
        except OSError as error:
            raise _Unavailable(f"unreadable_entry:{root}:{error}") from error
        if not _same_node_state(scan_st, final_st):
            raise _Unavailable(f"changed_while_scanning:{root}")
    finally:
        os.close(fd)


def _tree_digest(root) -> tuple[str | None, str | None, list[str] | None]:
    """Returns ``(digest_hex, None, manifest)`` or ``(None, reason, None)``."""
    root = str(root)
    budget = dict(files=0, total_bytes=0, root=root)
    deadline = time.monotonic() + _TIME_BUDGET_SECONDS
    records: list[str] = []
    try:
        _walk(root, budget, deadline, records)
    except _Unavailable as error:
        return None, error.reason, None
    records.sort()
    h = hashlib.sha256()
    for line in records:
        h.update(line.encode("utf-8", "surrogateescape"))
        h.update(b"\n")
    return h.hexdigest(), None, records


def capture_revision(workspace_root) -> dict:
    """Read-only, pure-Python observation of ``workspace_root`` -- never
    executes git or any other repo-configurable program. Returns
    ``dict(kind, head, dirty_digest, detail)``: ``kind`` is ``"observed"``
    (``dirty_digest`` a sha256 tree-manifest digest; ``head`` is a
    best-effort sha, possibly ``None``) or ``"unavailable"`` (``head``/
    ``dirty_digest`` are ``None``, ``detail`` a short concrete reason) --
    a capture is only ever ``"unavailable"`` because the tree digest itself
    could not be computed safely/completely; a missing or unresolvable HEAD
    never by itself makes the capture unavailable."""
    digest, digest_reason, manifest = _tree_digest(workspace_root)
    if digest is None:
        return dict(kind="unavailable", head=None, dirty_digest=None, detail=digest_reason, manifest=None)
    head, _head_detail = _read_head(workspace_root)
    return dict(kind="observed", head=head, dirty_digest=digest, detail=None, manifest=manifest)


def record_attempt_revision(store, attempt_id: str, phase: str, workspace_root) -> dict:
    """Capture and durably append one ``attempt_revisions`` row. Returns the
    captured ``dict`` (see :func:`capture_revision`)."""
    captured = capture_revision(workspace_root)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO attempt_revisions "
                "(attempt_id,captured_at,phase,kind,head,dirty_digest,detail,manifest_json) "
                "VALUES (?,?,?,?,?,?,?,?)",
                (attempt_id, _now(), phase, captured["kind"], captured["head"], captured["dirty_digest"],
                 captured["detail"], json.dumps(captured.get("manifest"), ensure_ascii=False, separators=(",", ":"))),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return captured


def latest_revision(store, attempt_id: str, phase_prefix: str) -> dict | None:
    """The most recently captured row for ``attempt_id`` whose ``phase``
    starts with ``phase_prefix`` (for example ``"worker_result:"``), or
    ``None`` if nothing was ever captured for it."""
    with store.connection() as db:
        row = db.execute(
            "SELECT kind, head, dirty_digest, detail, manifest_json FROM attempt_revisions "
            "WHERE attempt_id=? AND phase LIKE ? ORDER BY captured_at DESC LIMIT 1",
            (attempt_id, phase_prefix + "%"),
        ).fetchone()
    return dict(row) if row is not None else None


_BENIGN_GENERATED_PARTS = {"__pycache__", ".pytest_cache"}
_BENIGN_GENERATED_SUFFIXES = (".pyc", ".pyo")


def _manifest_map(manifest) -> dict[str, str] | None:
    if not isinstance(manifest, list) or any(not isinstance(item, str) for item in manifest):
        return None
    result = {}
    for item in manifest:
        fields = item.split("\t", 3)
        if len(fields) != 4 or not fields[1] or fields[1] in result:
            return None
        result[fields[1]] = item
    return result


def _is_benign_generated(path: str) -> bool:
    parts = path.split("/")
    return any(part in _BENIGN_GENERATED_PARTS for part in parts) or path.endswith(_BENIGN_GENERATED_SUFFIXES)


def publication_attribution(publication: dict, current: dict) -> str:
    """Classify a same-turn direct-worker publication against current state.

    Returns ``exact``, ``benign_generated_only`` or ``unknown``. A manifest is
    required to explain a changed digest; missing or malformed evidence never
    authorizes a coordinator turn. Only Python bytecode cache entries are
    ignored, and every other path-level delta remains attributable to nobody.
    """
    if publication.get("kind") != "observed" or current.get("kind") != "observed":
        return "unknown"
    if publication.get("dirty_digest") == current.get("dirty_digest"):
        return "exact"
    before = _manifest_map(publication.get("manifest"))
    after = _manifest_map(current.get("manifest"))
    if before is None or after is None:
        return "unknown"
    changed = {path for path in set(before) | set(after) if before.get(path) != after.get(path)}
    return "benign_generated_only" if changed and all(_is_benign_generated(path) for path in changed) else "unknown"


def _manifest_delta(before: dict, after: dict) -> set[str] | None:
    """Return changed paths for two complete observed captures.

    ``None`` means the durable observation is not sufficient to make an
    ownership decision.  This is deliberately kept separate from the digest
    comparison: a matching current tree still does not say which same-user
    worker wrote it.
    """
    if before.get("kind") != "observed" or after.get("kind") != "observed":
        return None
    left, right = _manifest_map(before.get("manifest")), _manifest_map(after.get("manifest"))
    if left is None or right is None:
        return None
    return {path for path in set(left) | set(right) if left.get(path) != right.get(path)}


def overlapping_manifest_deltas(base_a: dict, publication_a: dict,
                                base_b: dict, publication_b: dict) -> set[str] | None:
    """Return paths edited by both shared-root attempts, or ``None`` if
    either attempt lacks complete durable manifest evidence."""
    left = _manifest_delta(base_a, publication_a)
    right = _manifest_delta(base_b, publication_b)
    if left is None or right is None:
        return None
    return left & right


def shared_publication_attribution(store, task_id: str, attempt_id: str,
                                   publication: dict, current: dict) -> tuple[bool, str | None]:
    """Attribute a direct shared-root publication using durable ownership evidence.

    A tree match is only admissible when the attempt has a durable pre-turn
    manifest and no other shared-root attempt has an overlapping changed-path
    interval.  Unknown baselines, unfinished concurrent mutators, and
    overlapping deltas fail closed.  This is process-observable evidence; it
    makes no OS-level claim about which same-user process performed a write.
    """
    basic = publication_attribution(publication, current)
    if publication.get("kind") != "observed" or current.get("kind") != "observed":
        return False, "shared_root_revision_drift_unattributed"
    # Keep the path delta available: a later, independently-owned shared-root
    # publication may explain it.  Returning immediately on ``unknown`` here
    # incorrectly rejected a planner publication when a disjoint writer had
    # subsequently published docs into the same root.
    own_to_current = _manifest_delta(publication, current)
    if own_to_current is None:
        return False, "shared_root_revision_drift_unattributed"
    if basic == "exact":
        own_to_current = set()
    uncovered = {path for path in own_to_current if not _is_benign_generated(path)}
    with store.connection() as db:
        start_count = db.execute(
            "SELECT COUNT(*) AS n FROM attempt_revisions WHERE attempt_id=? AND phase=?",
            (attempt_id, f"worker_start:{attempt_id}"),
        ).fetchone()["n"]
        if start_count > 1:
            return False, "shared_root_baseline_ambiguous"
        own = db.execute(
            "SELECT kind, dirty_digest, manifest_json FROM attempt_revisions "
            "WHERE attempt_id=? AND phase=? ORDER BY rowid DESC LIMIT 1",
            (attempt_id, f"worker_start:{attempt_id}"),
        ).fetchone()
        if own is None:
            return False, "shared_root_baseline_unavailable"
        try:
            own_base = dict(kind=own["kind"], dirty_digest=own["dirty_digest"],
                            manifest=json.loads(own["manifest_json"]) if own["manifest_json"] else None)
        except (TypeError, ValueError):
            return False, "shared_root_baseline_malformed"
        own_delta = _manifest_delta(own_base, publication)
        if own_delta is None:
            return False, "shared_root_delta_unavailable"
        rows = db.execute(
            "SELECT p.attempt_id, p.status, p.workspace_binding_id, e.payload_json "
            "FROM delegation_provisioning p LEFT JOIN task_events e "
            "ON e.task_id=p.task_id AND e.assignment_ref=p.attempt_id "
            "AND e.event_type='worker_result' AND e.superseded=0 "
            "WHERE p.task_id=? AND p.workspace_binding_id IN "
            "('shared_project_root','serialized_shared_root') AND p.attempt_id IS NOT NULL",
            (task_id,),
        ).fetchall()

        def superseded_cancellation(other_attempt: str, *, allow_unpublished_followup: bool = False) -> bool:
            """Return whether a canceled attempt is explicitly historical.

            A cancellation alone is not enough to discard a shared-root
            mutation: a user-canceled worker may have left files behind.
            Only a durable coordinator cancellation whose reason explicitly
            describes replacement/correction is eligible for the historical
            path below.  The changed paths are still checked against the
            retained publications after this predicate succeeds.
            """
            identity = db.execute(
                "SELECT idempotency_key, assignment_id FROM delegation_provisioning "
                "WHERE task_id=? AND attempt_id=?",
                (task_id, other_attempt),
            ).fetchone()
            refs = [other_attempt]
            if identity is not None:
                refs.extend([identity["idempotency_key"], identity["assignment_id"]])
            placeholders = ",".join("?" for _ in refs)
            canceled = db.execute(
                "SELECT origin, payload_json FROM task_events WHERE task_id=? "
                "AND event_type='assignment_cancelled' AND superseded=0 "
                f"AND assignment_ref IN ({placeholders}) ORDER BY id DESC LIMIT 1",
                (task_id, *refs),
            ).fetchone()
            if canceled is None:
                return False
            if canceled["origin"] not in {"coordinator", "runtime"}:
                return False
            try:
                payload = json.loads(canceled["payload_json"])
                if not isinstance(payload, dict):
                    return False
                reason = str(payload.get("reason") or "").lower()
            except (AttributeError, TypeError, ValueError):
                return False
            replacement = any(marker in reason for marker in (
                "supersed", "replaced", "corrective", "corrected",
            ))
            historical = any(marker in reason for marker in (
                "historical", "same-file", "same file", "no project edit",
                "final active work graph", "replacement",
            ))
            if not (replacement and historical):
                return False
            # Require a durable replacement edge in the task graph.  Freeform
            # cancellation prose alone must never authorize discarding a
            # shared-root publication.
            for successor in db.execute(
                "SELECT assignment_ref, payload_json FROM task_events WHERE task_id=? "
                "AND event_type='delegation_requested' AND superseded=0",
                (task_id,),
            ).fetchall():
                try:
                    successor_payload = json.loads(successor["payload_json"])
                    target = successor_payload.get("supersedes_assignment_ref") if isinstance(successor_payload, dict) else None
                    exact_target = target in refs
                    if allow_unpublished_followup and not exact_target and successor["assignment_ref"] in refs and target:
                        # A follow-up can be canceled under its own
                        # assignment ref while its durable request points at
                        # the predecessor it supersedes.  Accept this only
                        # when the successor's own request carries exactly
                        # that target and the target is a real provisioning
                        # identity; arbitrary valid task assignments are not
                        # interchangeable.
                        own_request = db.execute(
                            "SELECT payload_json FROM task_events WHERE task_id=? "
                            "AND event_type='delegation_requested' AND superseded=0 "
                            "AND assignment_ref=? ORDER BY id DESC LIMIT 1",
                            (task_id, successor["assignment_ref"]),
                        ).fetchone()
                        try:
                            own_payload = json.loads(own_request["payload_json"]) if own_request else None
                        except (AttributeError, TypeError, ValueError):
                            own_payload = None
                        exact_target = isinstance(own_payload, dict) and own_payload.get("supersedes_assignment_ref") == target and db.execute(
                            "SELECT 1 FROM delegation_provisioning WHERE task_id=? "
                            "AND (idempotency_key=? OR assignment_id=?) LIMIT 1",
                            (task_id, target, target),
                        ).fetchone() is not None
                    if exact_target:
                        return True
                except (AttributeError, TypeError, ValueError):
                    continue
            return False

        # Explicitly superseded historical attempts can overlap a later
        # corrective publication.  Gather them first so that they can be
        # admitted only when every path they durably changed is covered by a
        # retained publication.  An unpublished canceled attempt is allowed
        # only when its baseline is exactly the current tree; otherwise its
        # unknown edits remain a hard failure.
        retained_rows = []
        superseded_rows = []
        for row in rows:
            other = row["attempt_id"]
            if other == attempt_id:
                continue
            base_row = db.execute(
                "SELECT kind, dirty_digest, manifest_json FROM attempt_revisions "
                "WHERE attempt_id=? AND phase=? ORDER BY rowid DESC LIMIT 1",
                (other, f"worker_start:{other}"),
            ).fetchone()
            base_count = db.execute(
                "SELECT COUNT(*) AS n FROM attempt_revisions WHERE attempt_id=? AND phase=?",
                (other, f"worker_start:{other}"),
            ).fetchone()["n"]
            if base_count > 1:
                return False, f"shared_root_concurrent_baseline_ambiguous:{other}"
            if base_row is None:
                return False, f"shared_root_concurrent_baseline_unavailable:{other}"
            result = row["payload_json"]
            if result is None:
                # A cancelled, unpublished attempt need not be a replacement
                # to have left this tree unchanged. Require its complete
                # retained baseline to match the current tree; cancellation
                # by itself never authorizes an unknown edit.
                cancelled = db.execute(
                    "SELECT 1 FROM task_events e WHERE e.task_id=? "
                    "AND e.event_type='assignment_cancelled' AND e.superseded=0 "
                    "AND (e.assignment_ref=? OR e.assignment_ref IN "
                    "(SELECT p.idempotency_key FROM delegation_provisioning p "
                    "WHERE p.task_id=? AND p.attempt_id=?)) LIMIT 1",
                    (task_id, other, task_id, other),
                ).fetchone()
                if cancelled is not None:
                    try:
                        cancelled_base = dict(
                            kind=base_row["kind"], dirty_digest=base_row["dirty_digest"],
                            manifest=json.loads(base_row["manifest_json"]) if base_row["manifest_json"] else None,
                        )
                    except (TypeError, ValueError):
                        return False, f"shared_root_concurrent_evidence_malformed:{other}"
                    if _manifest_delta(cancelled_base, current) == set():
                        continue
                if superseded_cancellation(other, allow_unpublished_followup=True):
                    try:
                        historical_base = dict(
                            kind=base_row["kind"], dirty_digest=base_row["dirty_digest"],
                            manifest=json.loads(base_row["manifest_json"]) if base_row["manifest_json"] else None,
                        )
                    except (TypeError, ValueError):
                        return False, f"shared_root_concurrent_evidence_malformed:{other}"
                    unpublished_delta = _manifest_delta(historical_base, current)
                    if unpublished_delta is None:
                        return False, f"shared_root_superseded_unpublished:{other}"
                    if {path for path in unpublished_delta if not _is_benign_generated(path)}:
                        return False, f"shared_root_superseded_unpublished:{other}"
                    continue
                retained_rows.append((row, base_row, None))
                continue
            try:
                payload = json.loads(result)
                if not isinstance(payload, dict):
                    return False, f"shared_root_concurrent_evidence_malformed:{other}"
                report_ref = payload.get("report_ref")
                pub_rows = db.execute(
                    "SELECT kind, dirty_digest, manifest_json, captured_at FROM attempt_revisions "
                    "WHERE attempt_id=? AND phase=? ORDER BY rowid ASC",
                    (other, f"worker_publish:{report_ref}"),
                ).fetchall()
                if len(pub_rows) > 1:
                    return False, f"shared_root_concurrent_publication_ambiguous:{other}"
                pub_row = pub_rows[0] if pub_rows else None
                other_base = dict(kind=base_row["kind"], dirty_digest=base_row["dirty_digest"],
                                  manifest=json.loads(base_row["manifest_json"]) if base_row["manifest_json"] else None)
                other_pub = None if pub_row is None else dict(
                    kind=pub_row["kind"], dirty_digest=pub_row["dirty_digest"],
                    manifest=json.loads(pub_row["manifest_json"]) if pub_row["manifest_json"] else None)
            except (TypeError, ValueError, KeyError):
                return False, f"shared_root_concurrent_evidence_malformed:{other}"
            if superseded_cancellation(other):
                delta = _manifest_delta(other_base, other_pub)
                if delta is None:
                    return False, f"shared_root_superseded_revision_unavailable:{other}"
                superseded_rows.append((other, delta, pub_row["captured_at"] if pub_row is not None else None))
            else:
                retained_rows.append((row, base_row, (other_base, other_pub, pub_row["captured_at"] if pub_row is not None else None)))

        for row, _base_row, pair in retained_rows:
            if pair is not None:
                delta = _manifest_delta(pair[0], pair[1])
                if delta is None:
                    return False, f"shared_root_concurrent_revision_unavailable:{row['attempt_id']}"
        own_delta = _manifest_delta(own_base, publication)
        if own_delta is None:
            return False, "shared_root_delta_unavailable"
        own_pub_row = db.execute(
            "SELECT captured_at FROM attempt_revisions WHERE attempt_id=? "
            "AND phase LIKE 'worker_publish:%' AND dirty_digest=? ORDER BY rowid DESC LIMIT 1",
            (attempt_id, publication.get("dirty_digest")),
        ).fetchone()
        own_pub_time = own_pub_row["captured_at"] if own_pub_row is not None else None
        retained_publications = []
        own_map = _manifest_map(publication.get("manifest"))
        if own_map is None:
            return False, "shared_root_delta_unavailable"
        retained_publications.append((own_pub_time, own_map, own_delta))
        for _row, _base_row, pair in retained_rows:
            if pair is not None and pair[1] is not None:
                retained_publications.append((pair[2], _manifest_map(pair[1].get("manifest")),
                                              _manifest_delta(pair[0], pair[1])))
        current_map = _manifest_map(current.get("manifest"))
        if current_map is None:
            return False, "shared_root_delta_unavailable"
        for other, delta, superseded_time in superseded_rows:
                latest = []
                for path in delta:
                    path_publications = []
                    for pub_time, pub_map, pub_delta in retained_publications:
                        if pub_map is None or pub_delta is None or path not in pub_delta:
                            continue
                        if superseded_time is None or pub_time is None or pub_time > superseded_time:
                            path_publications.append((pub_time, pub_map))
                    if path_publications:
                        path_publications.sort(key=lambda item: item[0] or "")
                        latest.append((path, path_publications[-1][1]))
                if not latest:
                    return False, f"shared_root_superseded_uncovered:{other}"
                latest_maps = dict(latest)
                for path in delta:
                    if _is_benign_generated(path):
                        continue
                    latest_map = latest_maps.get(path)
                    if latest_map is None or latest_map.get(path) != current_map.get(path):
                        return False, f"shared_root_superseded_uncovered:{other}"

        covered: set[str] = set()
        for row, base_row, prepared in retained_rows:
            other = row["attempt_id"]
            result = row["payload_json"]
            if result is None:
                return False, f"shared_root_concurrent_mutator_unpublished:{other}"
            try:
                other_base, other_pub, _pub_time = prepared
            except (TypeError, ValueError):
                return False, f"shared_root_concurrent_evidence_malformed:{other}"
            other_delta = _manifest_delta(other_base, other_pub) if other_pub else None
            if other_delta is None:
                return False, f"shared_root_concurrent_revision_unavailable:{other}"
            overlap = sorted(overlapping_manifest_deltas(own_base, publication, other_base, other_pub) or ())
            if overlap:
                return False, f"shared_root_overlapping_edits:{other}:" + ",".join(overlap[:20])
            # The other publication may explain only the paths it durably
            # owns.  Anything else in the current delta remains external
            # drift and is refused below.
            covered.update(other_delta)
        # Path ownership is not enough: an external writer can replace the
        # same path after a retained publication.  For every path attributed
        # to a retained publication, bind the current manifest entry to the
        # latest retained publication that changed that path.
        for path in uncovered & covered:
            candidates = []
            for pub_time, pub_map, pub_delta in retained_publications:
                if pub_map is None or pub_delta is None or path not in pub_delta:
                    continue
                candidates.append((pub_time or "", pub_map.get(path)))
            if not candidates:
                return False, "shared_root_revision_drift_unattributed"
            expected = sorted(candidates, key=lambda item: item[0])[-1][1]
            if current_map.get(path) != expected:
                return False, "shared_root_revision_drift_unattributed"
        if uncovered - covered:
            return False, "shared_root_revision_drift_unattributed"
    return True, None


def workspace_changed(before: dict, after: dict) -> bool:
    """T035 FINDING-12 closure, engine-independent backstop: True unless
    ``before``/``after`` (two :func:`capture_revision` results, taken
    immediately before and immediately after one coordinator engine turn)
    can positively prove the workspace's tree digest did not move.

    What this compares: only ``dirty_digest`` -- the sha256 tree-manifest
    digest capture_revision computes over every file/dir/symlink/other
    entry under the workspace root (excluding ``.git``/``.cortex``; see
    that function's own docstring). ``head`` is never part of this
    comparison (a HEAD move with an identical tree, e.g. an empty commit or
    a branch switch onto identical content, is not itself a workspace
    mutation this backstop cares about).

    Fail-closed, not fail-open: either capture being ``"unavailable"``
    (the tree could not be safely/completely digested -- see
    ``capture_revision``'s own docstring for when that happens) means this
    returns ``True`` -- "cannot prove unchanged" is treated the same as
    "changed", never silently treated as "probably fine". This mirrors
    ``capture_revision`` itself, which already refuses a partial/best-effort
    digest rather than risk hiding a change.

    Cost: this is exactly one extra full :func:`capture_revision` walk of
    the shared project workspace on *every* coordinator turn (before *and*
    after -- two walks per turn), the same bounded, pure-Python,
    no-subprocess walk ``record_attempt_revision`` already performs for
    every ``worker_result`` -- capped by the same ``_MAX_FILES``/
    ``_MAX_TOTAL_HASH_BYTES``/``_TIME_BUDGET_SECONDS`` constants (so, worst
    case, up to ~20 extra wall-clock seconds and a full re-read of every
    tracked file's bytes, per coordinator turn) -- see module docstring for
    why this is pure Python rather than a faster ``git status`` shortcut.
    """
    return (
        before.get("kind") != "observed"
        or after.get("kind") != "observed"
        or before.get("dirty_digest") != after.get("dirty_digest")
    )


def label(captured: dict | None) -> str:
    """A short, human-readable continuation-text label for one capture (or
    ``None``, meaning nothing was ever captured)."""
    if captured is None:
        return "revision: not captured"
    if captured["kind"] != "observed":
        return "revision: unknown (unavailable)"
    head = captured["head"]
    head_label = head[:12] if head else "no-head"
    return f"revision: {head_label} tree={captured['dirty_digest'][:8]}"
