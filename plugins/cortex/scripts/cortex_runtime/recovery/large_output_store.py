"""T024: durable, owner-private persistence for oversized tool/terminal
output, plus a scoped chunked-read API.

Both ``engine.terminal_bridge.LocalTerminal`` (T014) and any future
engine-tool-result capture keep their *raw* output only in memory, bounded
to a fixed byte window (``_truncate_from_start``) -- once a terminal or
tool call is released, that in-memory buffer is gone. This module is the
durable side: :func:`persist_output` writes the *exact* bytes a caller
already has (never re-fetches or re-derives them) to one owner-private
file under the task's storage directory (``store._task_directory``'s
private, 0700 directory; the file itself is created 0600, matching every
other body file this codebase writes -- see ``store.py``'s report-body
writes), and records one ``large_outputs`` metadata row carrying the exact
same ``exit_code``/``signal``/``failure``/``truncated``/``byte_count``
fields the in-memory bridge already tracked -- persisting to a file must
never lose any of them (task card line 45 / lead decision 5).

:func:`read_output` is the scoped read side: a caller must supply the
*same* ``owner_scope`` (``"coordinator"`` or a specific ``assignment_ref``)
the record was persisted under; a mismatched scope is refused as unknown
(never resolved), mirroring ``engine.terminal_bridge.TerminalBridge``'s own
"a terminal created by a different session ... is refused as unknown"
discipline. Reads are chunked (``offset``/``chunk_size``) so a huge
persisted output is never forced back into memory whole by a caller that
only wants a slice of it; every chunk's response still carries the
record's ``exit_status``, ``failure``, and ``truncated`` flags intact
(never only on the first/last chunk), plus the total ``byte_count``.
"""
from __future__ import annotations

import os

from ..portable.identity import new_id
from ..store import private_directory, regular, sync_directory

DEFAULT_CHUNK_SIZE = 64 * 1024
_LARGE_OUTPUTS_DIRNAME = "large_outputs"


class LargeOutputError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _large_outputs_dir(store, db, task_id: str):
    directory = store._task_directory(db, task_id) / _LARGE_OUTPUTS_DIRNAME
    return private_directory(directory)


def persist_output(
    store, *, task_id: str, owner_scope: str, source_kind: str, data: bytes,
    source_ref: str | None = None, exit_code: int | None = None, signal: str | None = None,
    failure: str | None = None, truncated: bool = False,
) -> dict:
    """Persist ``data`` (the exact bytes already captured by the caller) to
    one new owner-private file and record its metadata row. Returns the
    metadata dict (``id``, ``byte_count``, ``truncated``, ``exit_code``,
    ``signal``, ``failure``, ``filename``).

    ``source_kind`` is ``"terminal"`` or ``"tool_result"``. ``owner_scope``
    is ``"coordinator"`` or a specific ``assignment_ref`` string -- never
    both/ambiguous (see module docstring).
    """
    if source_kind not in ("terminal", "tool_result"):
        raise LargeOutputError("invalid_source_kind")
    if not owner_scope:
        raise LargeOutputError("invalid_owner_scope")
    record_id = new_id("lo_")
    filename = record_id + ".bin"
    with store.connection() as db:
        directory = _large_outputs_dir(store, db, task_id)
        path = directory / filename
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
        except BaseException:
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            raise
        sync_directory(directory)
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO large_outputs "
                "(id, task_id, owner_scope, source_kind, source_ref, filename, byte_count, "
                " truncated, exit_code, signal, failure, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                (record_id, task_id, owner_scope, source_kind, source_ref, filename, len(data),
                 1 if truncated else 0, exit_code, signal, failure, store._now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(
        id=record_id, task_id=task_id, owner_scope=owner_scope, source_kind=source_kind,
        source_ref=source_ref, filename=filename, byte_count=len(data), truncated=truncated,
        exit_code=exit_code, signal=signal, failure=failure,
    )


def _row(store, db, task_id: str, owner_scope: str, record_id: str):
    row = db.execute(
        "SELECT * FROM large_outputs WHERE id=? AND task_id=? AND owner_scope=?",
        (record_id, task_id, owner_scope),
    ).fetchone()
    if row is None:
        raise LargeOutputError("unknown_large_output_for_scope")
    return row


def read_output(
    store, *, task_id: str, owner_scope: str, record_id: str,
    offset: int = 0, chunk_size: int = DEFAULT_CHUNK_SIZE,
) -> dict:
    """Read one chunk of a persisted output, scoped to ``owner_scope``
    (refused as unknown for any other scope, even a real record id under a
    different scope -- see module docstring). Every response carries the
    full record's ``exit_status``, ``failure``, and ``truncated`` flags and
    the total ``byte_count``, regardless of which chunk this is."""
    if offset < 0 or chunk_size <= 0:
        raise LargeOutputError("invalid_read_range")
    with store.connection() as db:
        row = _row(store, db, task_id, owner_scope, record_id)
        directory = _large_outputs_dir(store, db, task_id)
    path = directory / row["filename"]
    regular(path)
    with open(path, "rb") as stream:
        stream.seek(offset)
        chunk = stream.read(chunk_size)
    next_offset = offset + len(chunk)
    return dict(
        id=record_id,
        chunk=chunk,
        offset=offset,
        next_offset=next_offset if next_offset < row["byte_count"] else None,
        byte_count=row["byte_count"],
        truncated=bool(row["truncated"]),
        exit_status=dict(exit_code=row["exit_code"], signal=row["signal"]),
        failure=row["failure"],
        source_kind=row["source_kind"],
        source_ref=row["source_ref"],
    )
