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
* the tree digest (stored in the ``dirty_digest`` column for schema
  compatibility with the pre-existing ``attempt_revisions`` table) -- a
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


def _hash_file(path: str, budget: dict, deadline: float) -> str:
    h = hashlib.sha256()
    size = 0
    try:
        fd = _open_regular_nofollow(path)
    except OSError as error:
        raise _Unavailable(f"unreadable_entry:{path}:{error}") from error
    try:
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
    finally:
        os.close(fd)
    return h.hexdigest()


def _walk(root: str, budget: dict, deadline: float, records: list) -> None:
    try:
        entries = list(os.scandir(root))
    except OSError as error:
        raise _Unavailable(f"unreadable_entry:{root}:{error}") from error
    for entry in entries:
        if time.monotonic() > deadline:
            raise _Unavailable("capture_timed_out")
        relpath = os.path.relpath(entry.path, budget["root"]).replace(os.sep, "/")
        if os.sep != "/" and "\\" in relpath:  # pragma: no cover - posix only in practice
            relpath = relpath.replace("\\", "/")
        top = relpath.split("/", 1)[0]
        if root == budget["root"] and top in _EXCLUDED_TOP_LEVEL:
            continue
        kind = _classify(entry)
        budget["files"] += 1
        if budget["files"] > _MAX_FILES:
            raise _Unavailable("workspace_too_large")
        if kind == "f":
            digest = _hash_file(entry.path, budget, deadline)
            try:
                size = entry.stat(follow_symlinks=False).st_size
            except OSError as error:
                raise _Unavailable(f"unreadable_entry:{entry.path}:{error}") from error
            records.append(f"f\t{relpath}\t{size}\t{digest}")
        elif kind == "l":
            try:
                target = os.readlink(entry.path)
            except OSError as error:
                raise _Unavailable(f"unreadable_entry:{entry.path}:{error}") from error
            records.append(f"l\t{relpath}\t-\t{target}")
        elif kind == "d":
            records.append(f"d\t{relpath}\t-\t-")
            _walk(entry.path, budget, deadline, records)
        else:
            records.append(f"{kind}\t{relpath}\t-\t-")


def _tree_digest(root) -> tuple[str | None, str | None]:
    """Returns ``(digest_hex, None)`` on success or ``(None, reason)``."""
    root = str(root)
    if not os.path.isdir(root):
        return None, "workspace root is not a directory"
    budget = dict(files=0, total_bytes=0, root=root)
    deadline = time.monotonic() + _TIME_BUDGET_SECONDS
    records: list[str] = []
    try:
        _walk(root, budget, deadline, records)
    except _Unavailable as error:
        return None, error.reason
    records.sort()
    h = hashlib.sha256()
    for line in records:
        h.update(line.encode("utf-8", "surrogateescape"))
        h.update(b"\n")
    return h.hexdigest(), None


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
    digest, digest_reason = _tree_digest(workspace_root)
    if digest is None:
        return dict(kind="unavailable", head=None, dirty_digest=None, detail=digest_reason)
    head, _head_detail = _read_head(workspace_root)
    return dict(kind="observed", head=head, dirty_digest=digest, detail=None)


def record_attempt_revision(store, attempt_id: str, phase: str, workspace_root) -> dict:
    """Capture and durably append one ``attempt_revisions`` row. Returns the
    captured ``dict`` (see :func:`capture_revision`)."""
    captured = capture_revision(workspace_root)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO attempt_revisions VALUES (?,?,?,?,?,?,?)",
                (attempt_id, _now(), phase, captured["kind"], captured["head"], captured["dirty_digest"], captured["detail"]),
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
            "SELECT kind, head, dirty_digest, detail FROM attempt_revisions "
            "WHERE attempt_id=? AND phase LIKE ? ORDER BY captured_at DESC LIMIT 1",
            (attempt_id, phase_prefix + "%"),
        ).fetchone()
    return dict(row) if row is not None else None


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
