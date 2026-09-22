"""T018 durable source/content preservation, bounded reads, and host replay.

A **tenth**, independent, all-or-nothing optional schema group on top of
``SCHEMA+IDENTITY_SCHEMA+CATALOG_SCHEMA+CONFIGURATION_SCHEMA+
ACP_SESSION_SCHEMA+DELEGATION_SCHEMA+TASK_EVENTS_SCHEMA+CAPACITY_SCHEMA+
EVIDENCE_SCHEMA`` (see ``split.py``'s comments on the existing groups and
``store.py``'s ``Store.__init__``): a pre-T018 v11 store has none of these
three tables; a store opened/created by the current ``store.py`` always has
all three. No ``user_version`` bump is needed -- this reuses the exact same
additive pattern every prior ACP-migration task has used.

Three tables:

- ``source_blocks`` -- one row per *non-text* prompt content block (an
  ``EmbeddedResource`` or ``resource_link``) captured on an ACP
  ``session/prompt`` turn, keyed by the ``reports`` row that turn's exact
  user text was published into (see ``store.bootstrap_source`` /
  ``bootstrap.bootstrap_task``/``append_source_event``). Metadata only --
  never a technical channel/attempt/session id. ``trust_label`` is always
  ``'user_direct'`` here (every block on an inbound user prompt is direct
  user input; see ``docs/acp/input-history-and-replay.md``). A stored
  binary/text body (when ``availability='stored'``) lives in a
  private, owner-only file under the task's storage directory
  (``body_ref``, a bare filename resolved against
  ``store.project_task_directory(project_root)/task_id/.source_blocks/``),
  never inline in SQLite -- preserving the existing Markdown-body/
  SQLite-metadata split for binary/large text bodies too.
- ``source_block_reads`` -- append-only read-time availability
  observations (see :func:`read_source`): a stored body file that has since
  vanished, or a ``reference_only`` link that no longer resolves, is
  recorded here as an *additional* row rather than rewriting the
  capture-time ``source_blocks`` row -- the original capture-time
  truth (what was actually captured/rejected/deferred at prompt time) is
  never silently overwritten by a later observation.
- ``host_transcript`` -- every host-visible chunk this agent has ever sent
  for one ACP session: the exact stored user text (plus attachment
  descriptors) on capture, and every ``agent_message_chunk`` text this
  agent actually emitted to the host. This is the sole source
  ``session/load`` (see :func:`replay_session`) reads from; it is *never*
  read by, or fed back into, ``OrchestratingRuntime``/engine context --
  display history and engine context are deliberately different data (see
  the module docstring boundary in ``acp.orchestrating_runtime`` and this
  task's "Out of scope" list).
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import errno
import hashlib
import json
import os
from pathlib import Path
import stat
import time
from typing import Any, Mapping, Sequence
import uuid

from ..contracts import StoreError

REPLAY_SCHEMA = '''
CREATE TABLE IF NOT EXISTS source_blocks (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 report_id TEXT NOT NULL REFERENCES reports(id),
 block_index INTEGER NOT NULL,
 kind TEXT NOT NULL,
 mime TEXT,
 name TEXT,
 uri TEXT,
 sha256 TEXT,
 size_bytes INTEGER,
 availability TEXT NOT NULL,
 reason TEXT,
 trust_label TEXT NOT NULL,
 body_ref TEXT,
 fetched_at TEXT,
 created_at TEXT NOT NULL,
 PRIMARY KEY(report_id, block_index)
);
CREATE INDEX IF NOT EXISTS source_blocks_by_task ON source_blocks(task_id);
CREATE TABLE IF NOT EXISTS source_block_reads (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 report_id TEXT NOT NULL,
 block_index INTEGER NOT NULL,
 observed_at TEXT NOT NULL,
 availability TEXT NOT NULL,
 reason TEXT
);
CREATE INDEX IF NOT EXISTS source_block_reads_by_block ON source_block_reads(report_id, block_index, id);
CREATE TABLE IF NOT EXISTS host_transcript (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id TEXT NOT NULL REFERENCES acp_host_sessions(session_id),
 seq INTEGER NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('user_message','agent_message')),
 text TEXT NOT NULL,
 attachments_json TEXT,
 message_id TEXT,
 created_at TEXT NOT NULL,
 UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS host_transcript_by_session ON host_transcript(session_id, seq);
-- ACP session/load must be able to restore structured host-visible updates
-- (tool cards, plans, and thoughts), not just text chunks.  This separate
-- table keeps the original transcript schema/check constraints intact while
-- sharing its monotonically ordered sequence namespace.
CREATE TABLE IF NOT EXISTS host_updates (
 session_id TEXT NOT NULL REFERENCES acp_host_sessions(session_id),
 seq INTEGER NOT NULL,
 update_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS host_updates_by_session ON host_updates(session_id, seq);
'''

REPLAY_TABLES = frozenset({"source_blocks", "source_block_reads", "host_transcript", "host_updates"})

TRUST_LABELS = frozenset({"user_direct", "file_content", "worker_report", "derived_summary"})

MAX_BLOCK_BYTES = 1 * 1024 * 1024
MAX_PROMPT_ATTACHMENT_BYTES = 4 * 1024 * 1024
READ_CHUNK_LIMIT = 64 * 1024
MAX_META_BYTES = 16 * 1024


def encode_meta(meta: Any) -> str | None:
    """Size-capped, forensic-only JSON encoding of one ``session/prompt``
    request's raw ``_meta`` field (never used as identity -- see
    ``source_events.origin``/``event_id``, minted independently of it).

    ``None``/absent stays ``None`` (no column write). An encoding that
    would exceed :data:`MAX_META_BYTES` (16 KiB) is replaced by a small
    truncation marker recording only the original encoded size -- the
    oversize payload itself is never retained, and this never raises (an
    unencodable ``_meta`` value -- non-JSON-serializable -- is treated the
    same as oversize, not dropped silently and not propagated as a prompt
    failure).
    """
    if meta is None:
        return None
    try:
        encoded = json.dumps(meta, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    except (TypeError, ValueError):
        return json.dumps(dict(truncated=True, reason="unencodable"), sort_keys=True)
    if len(encoded.encode("utf-8")) > MAX_META_BYTES:
        return json.dumps(
            dict(truncated=True, original_size_bytes=len(encoded.encode("utf-8"))), sort_keys=True,
        )
    return encoded


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


@dataclass(frozen=True)
class BlockCapture:
    """One resolved, pre-storage prompt content block (see
    :func:`resolve_prompt_blocks`), ready to be durably recorded by
    :func:`record_blocks` alongside its source event's published report."""

    block_index: int
    kind: str  # 'resource' | 'resource_link' | 'image'
    mime: str | None
    name: str | None
    uri: str | None
    availability: str  # 'stored' | 'reference_only' | 'inaccessible' | 'too_large'
    reason: str | None
    body: bytes | None  # only set when availability == 'stored'
    body_is_text: bool = False


def _blocks_dir(store, db, task_id: str):
    from ..store import private_directory

    task_dir = store._task_directory(db, task_id)
    return private_directory(task_dir / ".source_blocks")


def _real_path_of_fd(fd: int) -> str:
    """The fully symlink-resolved real path an already-open file descriptor
    refers to -- read directly from the kernel via ``/proc/self/fd``, never
    re-derived from the original request path (TOCTOU-safe; mirrors
    ``engine.fs_bridge._real_path_of_fd``). Fails closed."""
    try:
        return os.readlink(f"/proc/self/fd/{fd}")
    except OSError as exc:
        raise OSError("cannot verify the opened file's real path") from exc


def _open_resource_link(raw_path: str):
    """Open ``raw_path`` TOCTOU-safely and return ``(fd, reason_if_failed)``.

    ``O_NOFOLLOW`` on the final path component turns a same-size symlink
    swap made *after* any earlier path-string validation into an ``ELOOP``
    open failure instead of silently following it (the bug this fixes: the
    previous implementation validated a resolved ``Path`` object and then
    separately called ``.stat()``/``.read_bytes()`` on the *path string*,
    which re-walks and re-resolves the filesystem a second time -- a
    classic check-then-use race). ``O_NONBLOCK`` means an attacker-planted
    FIFO with no writer returns immediately instead of blocking this call
    forever; every subsequent check below operates on the returned *fd*
    only, never on the path string again.
    """
    try:
        fd = os.open(raw_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except OSError as exc:
        if exc.errno == errno.ELOOP:
            return None, "symlink_path"
        return None, "unreadable"
    return fd, None


def _resolve_resource_link_block(index: int, block: Mapping[str, Any], project_root: str | None, total: int) -> tuple[BlockCapture, int]:
    from urllib.parse import unquote, urlsplit

    name = block.get("name")
    uri = block.get("uri")
    mime = block.get("mimeType")
    if not isinstance(uri, str):
        return BlockCapture(index, "resource_link", mime, name, None, "inaccessible", "missing_uri", None), total
    if project_root is None:
        return BlockCapture(index, "resource_link", mime, name, uri, "reference_only", None, None), total
    parts = urlsplit(uri)
    if parts.scheme != "file":
        return BlockCapture(index, "resource_link", mime, name, uri, "reference_only", None, None), total
    if parts.netloc not in ("", "localhost"):
        return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "unsupported_file_authority", None), total
    raw_path = unquote(parts.path)
    if not raw_path.startswith("/"):
        return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "non_absolute_file_uri", None), total
    try:
        root = Path(project_root).resolve(strict=True)
    except (OSError, RuntimeError):
        return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "unresolvable_path", None), total
    # TOCTOU-safe from here on: open once (final component never followed
    # if it is a symlink), then every remaining decision -- containment,
    # regular-file/FIFO/device, ownership, hardlink count, size, body bytes
    # -- is derived from that one already-open fd (fstat/readlink/read),
    # never by re-touching the original path string (see
    # engine.fs_bridge module docstring's identical TOCTOU rationale).
    fd, open_reason = _open_resource_link(raw_path)
    if fd is None:
        return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", open_reason, None), total
    try:
        try:
            info = os.fstat(fd)
            real_path = _real_path_of_fd(fd)
        except OSError:
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "unresolvable_path", None), total
        try:
            Path(real_path).relative_to(root)
        except ValueError:
            # Genuinely outside the project root (by the fd's own kernel-
            # verified real path, not a re-walked string) -- a legitimate
            # reference, never fetched: no bytes are read below.
            return BlockCapture(index, "resource_link", mime, name, uri, "reference_only", None, None), total
        if stat.S_ISLNK(info.st_mode):
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "symlink_path", None), total
        if not stat.S_ISREG(info.st_mode):
            # FIFOs/devices/sockets/directories: never partially read.
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "not_a_regular_file", None), total
        if info.st_uid != os.getuid():
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "not_owned", None), total
        if info.st_nlink != 1:
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "multiple_hardlinks", None), total
        cap = MAX_BLOCK_BYTES
        chunks: list[bytes] = []
        read_total = 0
        try:
            while read_total <= cap:
                block_bytes = os.read(fd, min(65536, cap + 1 - read_total))
                if not block_bytes:
                    break
                chunks.append(block_bytes)
                read_total += len(block_bytes)
        except OSError:
            return BlockCapture(index, "resource_link", mime, name, uri, "inaccessible", "unreadable", None), total
        if read_total > cap:
            # Bounded read caught the file growing past the cap during (or
            # after fstat but before/while) reading -- never store a
            # partial/truncated body silently as if it were complete.
            return BlockCapture(index, "resource_link", mime, name, uri, "too_large", "block_exceeds_1mib_cap", None), total
        body = b"".join(chunks)
    finally:
        os.close(fd)
    if total + len(body) > MAX_PROMPT_ATTACHMENT_BYTES:
        return BlockCapture(index, "resource_link", mime, name, uri, "too_large", "prompt_attachment_total_exceeds_4mib_cap", None), total
    is_text = True
    try:
        body.decode("utf-8")
    except UnicodeDecodeError:
        is_text = False
    return BlockCapture(index, "resource_link", mime, name, uri, "stored", None, body, is_text), total + len(body)


def _resolve_resource_block(index: int, block: Mapping[str, Any], total: int) -> tuple[BlockCapture, int]:
    resource = block.get("resource") or {}
    uri = resource.get("uri") if isinstance(resource, dict) else None
    mime = resource.get("mimeType") if isinstance(resource, dict) else None
    text = resource.get("text") if isinstance(resource, dict) else None
    blob_b64 = resource.get("blob") if isinstance(resource, dict) else None
    if isinstance(text, str):
        body, is_text = text.encode("utf-8"), True
    elif isinstance(blob_b64, str):
        import base64

        try:
            body = base64.b64decode(blob_b64, validate=True)
        except Exception:
            return BlockCapture(index, "resource", mime, None, uri, "inaccessible", "invalid_blob_encoding", None), total
        is_text = False
    else:
        return BlockCapture(index, "resource", mime, None, uri, "inaccessible", "no_embedded_content", None), total
    if len(body) > MAX_BLOCK_BYTES:
        return BlockCapture(index, "resource", mime, None, uri, "too_large", "block_exceeds_1mib_cap", None), total
    if total + len(body) > MAX_PROMPT_ATTACHMENT_BYTES:
        return BlockCapture(index, "resource", mime, None, uri, "too_large", "prompt_attachment_total_exceeds_4mib_cap", None), total
    return BlockCapture(index, "resource", mime, None, uri, "stored", None, body, is_text), total + len(body)


def resolve_prompt_blocks(blocks: Sequence[Mapping[str, Any]], project_root: str | None) -> list[BlockCapture]:
    """Resolve every non-text content block of one accepted prompt into a
    :class:`BlockCapture`, truthfully classifying availability -- never
    claiming a body is retained when it was not. Enforces the per-block
    (1 MiB) and per-prompt total (4 MiB) attachment caps; content over
    either cap is recorded with metadata only (``availability='too_large'``),
    never partially stored. Pure read/decode work over already-received
    prompt data -- no database access, so this always runs *before* any
    durable capture transaction opens (see ``bootstrap.bootstrap_task``/
    ``append_source_event`` and ``acp.agent``).

    Only called for blocks that already passed ``protocol``'s type/shape
    validation (``resource``/``resource_link``/``image``); text blocks contribute
    nothing here (their exact text is already the report body).

    Never raises: any unexpected per-block failure (anything not already
    classified by a specific ``except`` clause in the per-kind resolvers) is
    caught here and the block is still returned, as
    ``availability='inaccessible'``, ``reason='capture_error'`` -- a block
    is never silently dropped just because resolving it raised something
    not specifically anticipated.
    """
    results: list[BlockCapture] = []
    total = 0
    for index, block in enumerate(blocks):
        kind = block.get("type") if isinstance(block, dict) else None
        try:
            if kind == "image":
                import base64
                data = block.get("data")
                mime = block.get("mimeType")
                if not isinstance(data, str) or not isinstance(mime, str) or not mime.startswith("image/"):
                    capture = BlockCapture(index, "image", mime if isinstance(mime, str) else None, None, None,
                                           "inaccessible", "invalid_image", None)
                else:
                    encoded_block_cap = ((MAX_BLOCK_BYTES + 2) // 3) * 4
                    if len(data) > encoded_block_cap:
                        capture = BlockCapture(index, "image", mime, None, None, "too_large", "block_exceeds_1mib_cap", None)
                    else:
                        try:
                            body = base64.b64decode(data, validate=True)
                        except Exception:
                            capture = BlockCapture(index, "image", mime, None, None, "inaccessible", "invalid_blob_encoding", None)
                        else:
                            if len(body) > MAX_BLOCK_BYTES or total + len(body) > MAX_PROMPT_ATTACHMENT_BYTES:
                                reason = "block_exceeds_1mib_cap" if len(body) > MAX_BLOCK_BYTES else "prompt_attachment_total_exceeds_4mib_cap"
                                capture = BlockCapture(index, "image", mime, None, None, "too_large", reason, None)
                            else:
                                capture = BlockCapture(index, "image", mime, None, None, "stored", None, body, False)
                                total += len(body)
            elif kind == "resource":
                capture, total = _resolve_resource_block(index, block, total)
            elif kind == "resource_link":
                capture, total = _resolve_resource_link_block(index, block, project_root, total)
            else:
                continue
        except Exception:
            capture = BlockCapture(index, kind or "unknown", None, None, None, "inaccessible", "capture_error", None)
        results.append(capture)
    return results


@dataclass(frozen=True)
class StagedBlock:
    """One :class:`BlockCapture` plus (for ``availability='stored'``) the
    already-written owner-private temp file holding its body -- produced by
    :func:`stage_blocks` *before* any durable capture transaction opens, so
    the (potentially slow) file write itself never happens while a
    ``BEGIN IMMEDIATE`` write lock is held. ``record_blocks_tx`` (called
    *inside* that transaction) only ever does a fast rename of this
    already-on-disk file plus one INSERT -- see that function's docstring.
    """

    capture: BlockCapture
    staged_path: Path | None  # None unless capture.availability == 'stored'


def _staging_dir(project_root: str):
    """An owner-private, project-scoped holding area for not-yet-committed
    attachment bodies (``<project>/.cortex/.source_blocks_staging/``) --
    exists independently of any task (a task may not exist yet: the first
    prompt on a session creates it *inside* the capture transaction), so
    staging can run before that transaction even opens."""
    from ..store import project_task_directory, private_directory

    return private_directory(project_task_directory(project_root) / ".source_blocks_staging")


def stage_blocks(project_root: str | None, blocks: Sequence[BlockCapture]) -> list[StagedBlock]:
    """Write every ``availability='stored'`` block's body to an owner-private
    temp file, entirely before any durable capture transaction opens (no
    DB/SQLite lock held during this I/O). Never raises: a write failure for
    one block (disk full, permission, ...) demotes just that block to
    ``availability='inaccessible'``, ``reason='capture_error'`` -- it is
    still returned (as a :class:`StagedBlock` with ``staged_path=None``),
    never silently dropped. Blocks with any other availability pass through
    unchanged (no file to stage).
    """
    staged: list[StagedBlock] = []
    for block in blocks:
        if block.availability != "stored" or block.body is None:
            staged.append(StagedBlock(block, None))
            continue
        try:
            directory = _staging_dir(project_root) if project_root is not None else None
            if directory is None:
                raise StoreError("project_context_unavailable")
            suffix = ".txt" if block.body_is_text else ".bin"
            path = directory / (_identifier(".staged_") + suffix)
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, "wb") as output:
                output.write(block.body)
                output.flush()
                os.fsync(output.fileno())
            staged.append(StagedBlock(block, path))
        except Exception:
            demoted = BlockCapture(
                block.block_index, block.kind, block.mime, block.name, block.uri,
                "inaccessible", "capture_error", None,
            )
            staged.append(StagedBlock(demoted, None))
    return staged


def _identifier(prefix: str) -> str:
    return prefix + uuid.uuid4().hex[:16]


def cleanup_staged(blocks: Sequence[StagedBlock]) -> None:
    """Unlink every still-staged (never renamed into place) temp file --
    called by ``bootstrap.bootstrap_task``/``append_source_event`` on a
    *replay* (an already-recorded ``(origin, event_id)``: nothing new is
    ever written for it, so these pre-staged bodies would otherwise leak)
    and by their exception paths (bundled into the same ``staged`` list the
    rest of ``store.py``'s crash-safety machinery already unlinks on
    rollback -- see ``record_blocks_tx``)."""
    for block in blocks:
        if block.staged_path is not None:
            block.staged_path.unlink(missing_ok=True)


STAGING_ORPHAN_MAX_AGE_SECONDS = 3600


def sweep_orphaned_staging(project_root: str, *, max_age_seconds: float = STAGING_ORPHAN_MAX_AGE_SECONDS) -> int:
    """Delete every file left in the project's attachment staging area
    (``.cortex/.source_blocks_staging/``) older than ``max_age_seconds``.

    Every file ever placed there is, by construction, either (a) still
    genuinely in flight (a ``session/prompt`` currently between
    ``stage_blocks`` and its enclosing ``bootstrap_task``/
    ``append_source_event`` transaction -- always well under an hour) or
    (b) already renamed out of this directory into
    ``<task_dir>/.source_blocks/`` by ``record_blocks_tx`` (so it no longer
    exists here) or (c) orphaned -- left behind by a process that crashed
    or was killed between ``stage_blocks`` writing it and either the
    rename (success) or ``cleanup_staged`` (replay/exception). This
    directory therefore never holds a file any ``source_blocks.body_ref``
    still points at; an age-only sweep here can never touch a referenced
    body (those live exclusively under the separate
    ``.source_blocks/`` directory this function never looks at).

    Safe to call at any time, including concurrently with an in-flight
    capture (age-gated well past any realistic single-request duration);
    intended to be invoked opportunistically (see ``acp.agent._session_new``)
    rather than requiring a separate maintenance process. Best-effort:
    never raises for an individual file that already vanished or could
    not be removed (a permission error, for example) -- returns the count
    of files actually removed.
    """
    try:
        directory = _staging_dir(project_root)
    except StoreError:
        return 0
    removed = 0
    deadline = time.time() - max_age_seconds
    try:
        entries = list(directory.iterdir())
    except OSError:
        return 0
    for path in entries:
        try:
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode):
                continue
            if info.st_mtime <= deadline:
                path.unlink()
                removed += 1
        except OSError:
            continue
    return removed


def record_blocks_tx(
    store, db, task_id: str, report_id: str, staged_blocks: Sequence[StagedBlock], rollbacks: list, staged: list,
    *, trust_label: str = "user_direct",
) -> None:
    """Durably record every staged block's metadata, and rename its
    already-on-disk staged body file into its final place, **inside the
    caller's own open transaction** (``db``) -- called from inside
    ``bootstrap.bootstrap_task``/``append_source_event``'s transaction so
    every block's metadata (and, for ``availability='stored'``, its body)
    commits atomically with the ``source_events``/``source_revisions`` rows
    for the exact same source event: a crash before that transaction's
    commit persists nothing at all (including no partial block rows); a
    crash after commit has the text and every block's metadata present
    together, always.

    ``rollbacks``/``staged`` are the *same* lists the caller's surrounding
    transaction already passes to ``store._rollback_files``/
    ``store._finalize_files`` on exception/success (see ``store.py``'s
    ``_publish_blocks`` for the identical convention this mirrors): a
    staged body file is appended to ``staged`` immediately (so an exception
    anywhere after this call still cleans it up via the caller's existing
    ``for path in staged: path.unlink(missing_ok=True)``), then removed
    from ``staged`` and the *final* path appended to ``rollbacks`` as
    ``(final_path, None)`` once the rename succeeds (so a **later**
    failure in the same transaction still rolls it back -- ``backup=None``
    means ``_rollback_files`` simply unlinks it, exactly like a fresh
    ``reports``/``editions`` insert's rollback).

    Never partially records: any staged file that fails to rename is
    treated as data corruption of this transaction's own attempt (not a
    truthful ``availability`` classification -- that was already decided
    before staging) and propagates, aborting the whole transaction, exactly
    like a failed report-body rename would.
    """
    if trust_label not in TRUST_LABELS:
        raise StoreError("invalid_trust_label", "trust_label", trust_label, "/".join(sorted(TRUST_LABELS)))
    if not staged_blocks:
        return
    directory = _blocks_dir(store, db, task_id)
    for entry in staged_blocks:
        block = entry.capture
        body_ref = None
        sha256 = None
        size = None
        fetched_at = None
        if block.availability == "stored" and entry.staged_path is not None:
            sha256 = hashlib.sha256(block.body).hexdigest() if block.body is not None else None
            size = len(block.body) if block.body is not None else None
            fetched_at = _now()
            suffix = ".txt" if block.body_is_text else ".bin"
            body_ref = f"{report_id}_{block.block_index}{suffix}"
            final_path = directory / body_ref
            staged.append(entry.staged_path)
            os.replace(entry.staged_path, final_path)
            staged.remove(entry.staged_path)
            rollbacks.append((final_path, None))
        db.execute(
            "INSERT INTO source_blocks VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (task_id, report_id, block.block_index, block.kind, block.mime, block.name, block.uri,
             sha256, size, block.availability, block.reason, trust_label, body_ref, fetched_at, _now()),
        )


def list_sources(store, task_id: str, after: int = 0, limit: int = 50) -> dict:
    """List durable source references for one task, cursor-paginated by
    ``rowid``-adjacent ordering over ``(report_id, block_index)`` joined to
    the owning ``source_revisions``/``source_events`` row. Each entry's
    ``source_ref`` is the exact opaque string :func:`read_source` accepts.
    """
    with store.connection() as db:
        rows = db.execute(
            """SELECT sr.revision, sr.message_id, r.id AS report_id, e.updated_at
               FROM source_revisions sr JOIN reports r ON r.id=sr.report_id
               JOIN editions e ON e.report_id=r.id
               WHERE sr.task_id=? AND sr.revision>? ORDER BY sr.revision LIMIT ?""",
            (task_id, after, limit + 1),
        ).fetchall()
        page = rows[: limit]
        entries = []
        for row in page:
            origin, _, event_id = row["message_id"].rpartition(":") if ":" in row["message_id"] else (None, None, None)
            entries.append(dict(
                source_ref=f"{row['report_id']}:-1", revision=row["revision"], kind="text",
                origin=origin, event_id=event_id, trust_label="user_direct", created_at=row["updated_at"],
            ))
            blocks = db.execute(
                "SELECT block_index,kind,mime,name,uri,availability,reason,trust_label,sha256,size_bytes,created_at "
                "FROM source_blocks WHERE task_id=? AND report_id=? ORDER BY block_index",
                (task_id, row["report_id"]),
            ).fetchall()
            for block in blocks:
                entries.append(dict(
                    source_ref=f"{row['report_id']}:{block['block_index']}", revision=row["revision"], kind=block["kind"],
                    origin=origin, event_id=event_id, mime=block["mime"], name=block["name"], uri=block["uri"],
                    availability=block["availability"], reason=block["reason"], trust_label=block["trust_label"],
                    sha256=block["sha256"], size_bytes=block["size_bytes"], created_at=block["created_at"],
                ))
        next_cursor = page[-1]["revision"] if len(rows) > len(page) else after
    return dict(entries=entries, next_cursor=next_cursor)


def _parse_ref(source_ref: str) -> tuple[str, int]:
    report_id, _, index = source_ref.rpartition(":")
    try:
        return report_id, int(index)
    except ValueError:
        raise StoreError("invalid_source_ref", "source_ref", source_ref, "an opaque ref returned by list_sources") from None


def _record_read_observation(db, report_id: str, block_index: int, availability: str, reason: str | None) -> None:
    db.execute(
        "INSERT INTO source_block_reads(report_id,block_index,observed_at,availability,reason) VALUES (?,?,?,?,?)",
        (report_id, block_index, _now(), availability, reason),
    )


def read_source(store, task_id: str, source_ref: str, offset: int = 0, limit: int = READ_CHUNK_LIMIT) -> dict:
    """Bounded, addressable read of one source block (main text at
    ``block_index=-1`` or an attachment block) with precise provenance.

    ``limit`` is clamped to :data:`READ_CHUNK_LIMIT` (64 KiB). Text is
    returned in ``chunk`` as a ``str``; a non-UTF-8 stored body is returned
    base64-encoded in ``chunk`` with ``encoding='base64'`` in the result. A
    stored body file that has vanished since capture, or a ``resource_link``
    whose target no longer resolves, is reported here as ``'expired'``/
    ``'inaccessible'`` (a fresh, append-only observation -- see
    :func:`_record_read_observation`) *without* rewriting the original
    capture-time ``source_blocks`` row.
    """
    limit = max(1, min(int(limit), READ_CHUNK_LIMIT))
    offset = max(0, int(offset))
    report_id, block_index = _parse_ref(source_ref)
    with store.connection() as db:
        report = db.execute("SELECT id FROM reports WHERE id=? AND task_id=?", (report_id, task_id)).fetchone()
        if report is None:
            raise StoreError("unknown_source_ref", "source_ref", source_ref, "a source_ref for this task")
        revision = db.execute(
            "SELECT thread_id,message_id FROM source_revisions WHERE task_id=? AND report_id=?", (task_id, report_id),
        ).fetchone()
        origin, event_id = (None, None)
        if revision is not None and ":" in revision["message_id"]:
            origin, _, event_id = revision["message_id"].rpartition(":")
        if block_index == -1:
            path = store._task_directory(db, task_id) / db.execute(
                "SELECT filename FROM reports WHERE id=?", (report_id,),
            ).fetchone()["filename"]
            try:
                data = path.read_bytes()
            except OSError:
                raise StoreError("unknown_source_ref", "source_ref", source_ref, "a source_ref for this task") from None
            total = len(data)
            chunk_bytes = data[offset: offset + limit]
            chunk = chunk_bytes.decode("utf-8", errors="replace")
            next_offset = offset + len(chunk_bytes) if offset + len(chunk_bytes) < total else None
            return dict(
                source_ref=source_ref, origin=origin, event_id=event_id, block_index=-1, kind="text",
                trust_label="user_direct", availability="stored", reason=None,
                sha256=hashlib.sha256(data).hexdigest(), total_size=total, offset=offset, chunk=chunk,
                encoding="utf-8", next_offset=next_offset,
            )
        block = db.execute(
            "SELECT * FROM source_blocks WHERE task_id=? AND report_id=? AND block_index=?",
            (task_id, report_id, block_index),
        ).fetchone()
        if block is None:
            raise StoreError("unknown_source_ref", "source_ref", source_ref, "a source_ref for this task")
        availability, reason = block["availability"], block["reason"]
        chunk, encoding, total, sha256_value = "", "utf-8", block["size_bytes"] or 0, block["sha256"]
        next_offset = None
        if availability == "stored":
            directory = _blocks_dir(store, db, task_id)
            path = directory / block["body_ref"]
            try:
                data = path.read_bytes()
            except OSError:
                availability, reason = "expired", "stored_body_missing"
                db.execute("BEGIN IMMEDIATE")
                try:
                    _record_read_observation(db, report_id, block_index, availability, reason)
                    db.commit()
                except BaseException:
                    db.rollback(); raise
            else:
                if hashlib.sha256(data).hexdigest() != block["sha256"]:
                    availability, reason = "expired", "stored_body_changed"
                    db.execute("BEGIN IMMEDIATE")
                    try:
                        _record_read_observation(db, report_id, block_index, availability, reason)
                        db.commit()
                    except BaseException:
                        db.rollback(); raise
                else:
                    window = data[offset: offset + limit]
                    total = len(data)
                    try:
                        chunk = window.decode("utf-8")
                        encoding = "utf-8"
                    except UnicodeDecodeError:
                        import base64

                        chunk = base64.b64encode(window).decode("ascii")
                        encoding = "base64"
                    next_offset = offset + len(window) if offset + len(window) < total else None
        elif availability == "reference_only" and block["uri"]:
            from urllib.parse import urlsplit

            if urlsplit(block["uri"]).scheme == "file":
                proj_row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
                observed_availability, observed_reason, _resolved = _resolve_file_uri(block["uri"], proj_row["project_root"])
                if observed_availability != "reference_only":
                    db.execute("BEGIN IMMEDIATE")
                    try:
                        _record_read_observation(db, report_id, block_index, observed_availability, observed_reason)
                        db.commit()
                    except BaseException:
                        db.rollback(); raise
        return dict(
            source_ref=source_ref, origin=origin, event_id=event_id, block_index=block_index, kind=block["kind"],
            trust_label=block["trust_label"], availability=availability, reason=reason,
            sha256=sha256_value, total_size=total, offset=offset, chunk=chunk, encoding=encoding,
            next_offset=next_offset, name=block["name"], uri=block["uri"], mime=block["mime"],
        )


# -- host transcript (session/load display-only replay) --------------------


def append_host_transcript(store, session_id: str, kind: str, text: str, attachments: Sequence[Mapping[str, Any]] | None = None, message_id: str | None = None) -> int:
    """Append one host-visible chunk for ``session_id``; returns its ``seq``.

    Never called from anywhere but ``acp.agent`` (capture-time for
    ``user_message``, ``emit_update``-time for ``agent_message``) -- this is
    purely a display-history recording, never read by orchestration/engine
    context (see the module docstring).
    """
    if kind not in ("user_message", "agent_message"):
        raise ValueError(kind)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute(
                "SELECT MAX(seq) FROM (SELECT seq FROM host_transcript WHERE session_id=? "
                "UNION ALL SELECT seq FROM host_updates WHERE session_id=?)", (session_id, session_id),
            ).fetchone()
            seq = (row[0] if row[0] is not None else -1) + 1
            # Older project stores predate message_id. Upgrade this additive
            # display-only column lazily while the write lock is already held.
            columns = {row[1] for row in db.execute("PRAGMA table_info(host_transcript)")}
            if "message_id" not in columns:
                db.execute("ALTER TABLE host_transcript ADD COLUMN message_id TEXT")
            db.execute(
                "INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,message_id,created_at) VALUES (?,?,?,?,?,?,?)",
                (session_id, seq, kind, text, json.dumps(list(attachments or []), sort_keys=True, ensure_ascii=False), message_id, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback(); raise
    return seq


def append_host_update(store, session_id: str, update: Mapping[str, Any]) -> int:
    """Persist one structured ``session/update`` before transport enqueue.

    The record contains the exact runtime-produced ACP payload offered to the
    host. It does not prove transport receipt or rendering. It is display
    history, never engine input. Keeping a shared sequence with
    ``host_transcript`` lets ``session/load`` reproduce the outbound order.
    """
    if not isinstance(update, Mapping) or not isinstance(update.get("sessionUpdate"), str):
        raise ValueError("update must contain sessionUpdate")
    encoded = json.dumps(dict(update), sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute(
                "SELECT MAX(seq) FROM (SELECT seq FROM host_transcript WHERE session_id=? "
                "UNION ALL SELECT seq FROM host_updates WHERE session_id=?)", (session_id, session_id),
            ).fetchone()
            seq = (row[0] if row[0] is not None else -1) + 1
            db.execute(
                "INSERT INTO host_updates(session_id,seq,update_json,created_at) VALUES (?,?,?,?)",
                (session_id, seq, encoded, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback(); raise
    return seq


def replay_session(store, session_id: str) -> list[dict]:
    """Return the ordered, display-only transcript for ``session_id``.

    Pure read: never opens an engine session, never calls
    ``OrchestratingRuntime.start_prompt``, never mutates ``source_events``/
    ``task_events``/``reports``/``deliveries`` (see
    ``docs/acp/input-history-and-replay.md`` and this task's "session/load
    must never ..." acceptance line -- verified in tests by DB row-count
    assertions around the call).
    """
    with store.connection() as db:
        try:
            rows = db.execute(
                "SELECT seq, kind, text, attachments_json, message_id, NULL AS update_json "
                "FROM host_transcript WHERE session_id=? "
                "UNION ALL SELECT seq, 'update' AS kind, NULL AS text, '[]' AS attachments_json, NULL AS message_id, update_json "
                "FROM host_updates WHERE session_id=? ORDER BY seq", (session_id, session_id),
            ).fetchall()
        except Exception as exc:
            # A pre-messageId store can still be loaded read-only.  Its old
            # rows have no exact grouping evidence, so the caller assigns a
            # distinct fallback id per replayed row rather than merging it.
            if "no such column" not in str(exc).lower():
                raise
            rows = db.execute(
                "SELECT seq, kind, text, attachments_json, NULL AS message_id, NULL AS update_json "
                "FROM host_transcript WHERE session_id=? "
                "UNION ALL SELECT seq, 'update' AS kind, NULL AS text, '[]' AS attachments_json, NULL AS message_id, update_json "
                "FROM host_updates WHERE session_id=? ORDER BY seq", (session_id, session_id),
            ).fetchall()
    from .read_history import ReadHistoryFilter
    read_filter = ReadHistoryFilter()
    result = []
    for row in rows:
        if row["kind"] == "update":
            try:
                update = json.loads(row["update_json"])
            except (TypeError, ValueError):
                # Never fabricate a malformed host event during replay.
                continue
            if isinstance(update, dict) and isinstance(update.get("sessionUpdate"), str):
                result.append(dict(kind="update", update=read_filter.project(update)))
            continue
        try:
            attachments = json.loads(row["attachments_json"] or "[]")
        except (TypeError, ValueError):
            # A damaged legacy transcript row must never make session/load
            # fail after it has already replayed earlier entries.  Dropping
            # this one row is the only honest outcome: inventing an empty
            # attachment list would claim evidence that is not available.
            continue
        if not isinstance(attachments, list):
            continue
        result.append(dict(kind=row["kind"], text=row["text"], attachments=attachments, message_id=row["message_id"]))
    return _merge_tool_call_updates(result)


def _merge_tool_call_updates(history: list[dict]) -> list[dict]:
    """Fold each card's run of ``tool_call_update``s into its final state.

    An ACP ``tool_call_update`` replaces every field it carries (``content``
    included), so the merged update renders exactly what the whole run did.
    A worker card resends its full cumulative log on every update: replaying
    each one grew quadratically (883 updates, 17.7 MB for one card) and
    overflowed the client's history buffer, so the chat could not reopen.
    The merged update takes the run's last position; a new ``tool_call`` for
    the same id starts a new run.
    """
    run_of: dict[int, tuple[str, int]] = {}
    last_of: dict[tuple[str, int], int] = {}
    generation: dict[str, int] = {}
    for index, entry in enumerate(history):
        update = entry.get("update") if entry["kind"] == "update" else None
        card = update.get("toolCallId") if update else None
        if not isinstance(card, str):
            continue
        if update["sessionUpdate"] == "tool_call":
            generation[card] = generation.get(card, 0) + 1
        elif update["sessionUpdate"] == "tool_call_update":
            run_of[index] = (card, generation.get(card, 0))
            last_of[run_of[index]] = index
    merged: dict[tuple[str, int], dict] = {}
    result = []
    for index, entry in enumerate(history):
        run = run_of.get(index)
        if run is None:
            result.append(entry)
            continue
        merged.setdefault(run, {}).update(entry["update"])
        if last_of[run] == index:
            result.append(dict(kind="update", update=merged.pop(run)))
    return result


def terminal_worker_card_reconciliation(store, task_id: str, history: Sequence[Mapping[str, Any]]) -> list[dict]:
    """Return terminal card updates justified by durable assignment/process evidence.

    A worker result can be recorded immediately before ``/cortex-stop`` and
    before the worker card's in-flight turn emits its final update.  The
    historical host stream then contains a real worker card stuck in
    ``in_progress`` even though the durable assignment is terminal.  This
    read-only reconciliation supplies one ACP update for that existing card
    during ``session/load``.  After a host/daemon restart, a durable process
    identity whose process is now provably absent is equally strong evidence
    that the worker cannot still be ``in_progress``; that case is rendered as
    a failed card without writing the transcript or starting a replacement
    engine session.  It never creates a card absent from history and never
    infers a terminal state from task status alone.
    """
    latest: dict[str, str] = {}
    seen_cards: set[str] = set()
    for entry in history:
        if entry.get("kind") != "update":
            continue
        update = entry.get("update")
        if not isinstance(update, Mapping):
            continue
        card_id = update.get("toolCallId")
        if not isinstance(card_id, str) or not card_id.startswith("worker:"):
            continue
        seen_cards.add(card_id)
        status = update.get("status")
        if isinstance(status, str):
            latest[card_id] = status
    if not seen_cards:
        return []
    with store.connection() as db:
        attempts = {
            row["attempt_id"]: row["idempotency_key"]
            for row in db.execute(
                "SELECT attempt_id, idempotency_key FROM delegation_provisioning "
                "WHERE task_id=? AND attempt_id IS NOT NULL", (task_id,),
            ).fetchall()
        }
        # The process registry is optional on older project stores.  Keep
        # this probe read-only and bounded: session/load must remain a
        # history replay, never a recovery start or a model call.
        has_process_registry = db.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='attempt_process_registry'"
        ).fetchone() is not None
        process_rows = {}
        if has_process_registry:
            process_rows = {
                row["attempt_id"]: dict(row)
                for row in db.execute(
                    "SELECT * FROM attempt_process_registry WHERE task_id=? AND role='worker'",
                    (task_id,),
                ).fetchall()
            }
        rows = db.execute(
            "SELECT assignment_ref, event_type, payload_json FROM task_events "
            "WHERE task_id=? AND event_type IN ('worker_result','assignment_failed','assignment_cancelled') "
            "AND superseded=0 ORDER BY id", (task_id,),
        ).fetchall()
    # A task-wide cancellation can be recorded after a worker has already
    # published its result.  Group terminal evidence by assignment and let a
    # real worker_result win over cancellation/failure regardless of insert
    # order; the task's stopped state remains separate from the attempt's
    # observed result.
    terminal_by_card: dict[str, Any] = {}
    priority = {"worker_result": 2, "assignment_failed": 1, "assignment_cancelled": 1}
    for row in rows:
        assignment_ref = attempts.get(row["assignment_ref"], row["assignment_ref"])
        card_id = f"worker:{assignment_ref}" if assignment_ref else ""
        if card_id not in seen_cards:
            continue
        prior = terminal_by_card.get(card_id)
        if prior is None or priority.get(row["event_type"], 0) > priority.get(prior["event_type"], 0):
            terminal_by_card[card_id] = row
    corrections: list[dict] = []
    for card_id, row in terminal_by_card.items():
        if latest.get(card_id) in {"completed", "failed"}:
            continue
        event_type = row["event_type"]
        status = "completed" if event_type == "worker_result" else "failed"
        try:
            payload = json.loads(row["payload_json"] or "{}")
        except (TypeError, ValueError):
            payload = {}
        if not isinstance(payload, Mapping):
            payload = {}
        if event_type == "worker_result":
            summary = payload.get("summary")
            text = f"Result: {summary}" if isinstance(summary, str) and summary else None
        elif event_type == "assignment_cancelled":
            reason = payload.get("reason")
            text = f"Cancelled: {reason}" if isinstance(reason, str) and reason else None
        else:
            error = payload.get("error")
            text = f"Not delegated/failed: {error}" if isinstance(error, str) and error else None
        update = {"sessionUpdate": "tool_call_update", "toolCallId": card_id, "status": status}
        if text:
            update["content"] = [{"type": "text", "text": text[:300]}]
        corrections.append(update)
    # A process can disappear after the last ACP card update but before it
    # publishes a worker_result/assignment_failed event.  The old history
    # remains visibly ``in_progress`` forever unless load checks the durable
    # identity.  Only a positively confirmed ``absent`` result qualifies;
    # ``alive`` and ``unknown`` are intentionally left untouched so a
    # permission/procfs problem can never turn into a false failure.
    if process_rows:
        from ..recovery import registry as recovery_registry

        existing_terminal = {item["toolCallId"] for item in corrections}
        assignment_to_attempt = {assignment: attempt for attempt, assignment in attempts.items()}
        for card_id in sorted(seen_cards):
            if card_id in existing_terminal or latest.get(card_id) in {"completed", "failed"}:
                continue
            assignment_ref = card_id.removeprefix("worker:")
            attempt_id = assignment_to_attempt.get(assignment_ref)
            process = process_rows.get(attempt_id) if attempt_id else None
            if process is None:
                continue
            if recovery_registry.probe_liveness(process) != "absent":
                continue
            corrections.append({
                "sessionUpdate": "tool_call_update",
                "toolCallId": card_id,
                "status": "failed",
                "content": [{
                    "type": "text",
                    "text": "Worker process exited before reporting a result.",
                }],
            })
    return corrections


# -- crash recovery: source_events captured but never recorded as a
#    task_events user_message row (see acp.orchestrating_runtime) ----------


def record_user_message_event(
    store, task_id: str, dedup_key: str, origin: str, payload: Mapping[str, Any],
    *, ledger_origin: str | None = None, ledger_event_id: str | None = None,
):
    """Atomically (1) insert the ``user_message`` ``task_events`` row for
    ``dedup_key`` and (2) -- when ``ledger_origin``/``ledger_event_id`` are
    given (a real source event identity was available) -- mark that exact
    ``source_events`` row's delivery-ledger state ``'recorded'``, in ONE
    transaction.

    This is the structural fix for the reproduced legacy re-delivery bug: a
    crash between "task_events row inserted" and "source_events row marked
    recorded" is now impossible (both commit together, or neither does),
    so ``find_unrecorded_source_events`` can trust ``ledger_state IS NULL``
    as the single source of truth for "never yet recorded" instead of
    reconstructing/pattern-matching a ``task_events.dedup_key`` shape (the
    b19d288-era turn-count key is not reconstructible from a bare
    ``source_events`` row at all -- see
    ``trusted_channel.ensure_source_event_ledger_column``).

    Called from **every** path that records a ``user_message`` event (see
    ``acp.orchestrating_runtime._run``/``_recover_unrecorded_source_events``)
    -- including the legacy session-turn-count fallback used when no real
    source identity was threaded through (``ledger_origin``/
    ``ledger_event_id`` simply omitted there; nothing to mark, and nothing
    is silently left half-recorded).
    """
    from ..delegation import events as delegation_events

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            event = delegation_events.append_event_tx(db, task_id, "user_message", dedup_key, payload, origin=origin)
            if ledger_origin is not None and ledger_event_id is not None:
                db.execute(
                    "UPDATE source_events SET ledger_state='recorded' "
                    "WHERE origin=? AND event_id=? AND ledger_state IS NULL",
                    (ledger_origin, ledger_event_id),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return event


def record_user_message_answer_event(
    store, task_id: str, dedup_key: str, origin: str, payload: Mapping[str, Any],
    *, question_id: str, answer_text: str, answer_source: str = "text",
    ledger_origin: str | None = None, ledger_event_id: str | None = None,
):
    """Append a user message and CAS-answer its question atomically."""
    from ..delegation import events as delegation_events

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND dedup_key=?",
                (task_id, dedup_key),
            ).fetchone()
            if existing is not None:
                event = delegation_events.TaskEvent._from_row(existing)
                db.commit()
                return event, not event.superseded
            event = delegation_events.append_event_tx(db, task_id, "user_message", dedup_key, payload, origin=origin)
            if ledger_origin is not None and ledger_event_id is not None:
                db.execute(
                    "UPDATE source_events SET ledger_state='recorded' WHERE origin=? AND event_id=? AND ledger_state IS NULL",
                    (ledger_origin, ledger_event_id),
                )
            cursor = db.execute(
                "UPDATE pending_questions SET status='answered', answer_text=?, answer_source=?, answered_at=datetime('now') "
                "WHERE id=? AND task_id=? AND status='open'",
                (answer_text, answer_source, question_id, task_id),
            )
            if cursor.rowcount == 0:
                db.execute(
                    "INSERT INTO pending_question_late_answers (question_id, task_id, answer_text, answer_source, received_at) "
                    "VALUES (?,?,?,?,datetime('now'))",
                    (question_id, task_id, answer_text, answer_source),
                )
                db.execute(
                    "UPDATE task_events SET superseded=1 WHERE task_id=? AND id=? AND consumed_by IS NULL AND superseded=0",
                    (task_id, event.id),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return event, cursor.rowcount == 1


def find_unrecorded_source_events(store, task_id: str) -> list[dict]:
    """Every ``source_events`` row for ``task_id`` with ``ledger_state IS
    NULL`` (never yet marked recorded by :func:`record_user_message_event`
    -- a genuine crash between capture and recording), oldest
    ``captured_at`` first. Each entry carries the exact durably-published
    user text for that event, resolved via ``source_revisions``/``reports``
    (never a summary).

    Structural, not text-matching: a pre-T018 (b19d288-era) row is
    backfilled ``'legacy_recorded'`` by
    ``trusted_channel.ensure_source_event_ledger_column`` and therefore
    never appears here, regardless of what its original (unreconstructible)
    ``task_events.dedup_key`` shape was -- fixing the reproduced bug where
    every historical prompt was re-delivered on first use after upgrade.
    """
    with store.connection() as db:
        rows = db.execute(
            """SELECT se.origin, se.event_id, se.captured_at FROM source_events se
               WHERE se.task_id=? AND se.ledger_state IS NULL
               ORDER BY se.captured_at ASC, se.event_id ASC""",
            (task_id,),
        ).fetchall()
        principal = db.execute(
            "SELECT thread_id FROM thread_bindings WHERE task_id=? AND parent_thread_id IS NULL LIMIT 1", (task_id,),
        ).fetchone()
        results = []
        for row in rows:
            message_id = f"{row['origin']}:{row['event_id']}"
            text = None
            if principal is not None:
                revision = db.execute(
                    "SELECT report_id FROM source_revisions WHERE task_id=? AND thread_id=? AND message_id=?",
                    (task_id, principal["thread_id"], message_id),
                ).fetchone()
                if revision is not None:
                    report = db.execute("SELECT filename FROM reports WHERE id=?", (revision["report_id"],)).fetchone()
                    if report is not None:
                        path = store._task_directory(db, task_id) / report["filename"]
                        try:
                            text = path.read_bytes().decode("utf-8")
                        except OSError:
                            text = None
            results.append(dict(origin=row["origin"], event_id=row["event_id"], captured_at=row["captured_at"], text=text))
    return results
