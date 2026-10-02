"""Read-only grants for the project/user SKILL.md paths actually sent to a session."""
from __future__ import annotations

import os
import sqlite3
import stat

from ..contracts import StoreError
from ..engine.unified_system_prompt import MAX_SKILL_BYTES, project_skill_entries, project_skills_section, _within


def record_session_skills(store, *, task_id: str, attempt_id: str, session_ref: str,
                          project_root: str | None, environ, sent_meta: dict | None) -> None:
    """Snapshot only paths in the exact skills section of the sent prompt.

    A changed directory between prompt construction and this snapshot fails
    closed: no grant is recorded. The tool never accepts a model-supplied glob.
    """
    entries = project_skill_entries(project_root, environ)
    section = project_skills_section(project_root, environ, entries=entries)
    prompt = ((sent_meta or {}).get("cortexBaseInstructions") or (sent_meta or {}).get("systemPrompt")
              or (sent_meta or {}).get("opencodeAgentPrompt"))
    if (not isinstance(prompt, str) or not prompt.startswith("# Cortex session\n\n")
            or not section or section not in prompt):
        entries = []
    with store.connection() as db:
        db.execute("""CREATE TABLE IF NOT EXISTS session_skill_paths (
            task_id TEXT NOT NULL, attempt_id TEXT NOT NULL, session_ref TEXT NOT NULL,
            listed_path TEXT NOT NULL, real_path TEXT NOT NULL, base_path TEXT NOT NULL,
            title TEXT NOT NULL, PRIMARY KEY(session_ref, listed_path))""")
        db.executemany(
            "INSERT INTO session_skill_paths VALUES (?,?,?,?,?,?,?)",
            ((task_id, attempt_id, session_ref, shown, real, base, title)
             for shown, real, base, title, _description in entries),
        )


def bind_session_skill_generation(store, *, channel_id: str, task_id: str,
                                  attempt_id: str, generation_id: str, session_ref: str) -> None:
    """Bind only the launch's own live channel to its observed session.

    Issuance precedes session/new, so the generation cannot be supplied at
    issue time. Replacements use fresh channels; compaction may advance the
    generation of the same session, but never move a channel across sessions.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            changed = db.execute(
                "UPDATE trusted_channels SET context_generation_id=? "
                "WHERE channel_id=? AND task_id=? AND COALESCE(attempt_id,'')=? "
                "AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?) "
                "AND (context_generation_id IS NULL OR context_generation_id IN "
                "(SELECT id FROM context_generations WHERE engine_session_ref=?)) "
                "AND EXISTS (SELECT 1 FROM context_generations g WHERE g.id=? "
                "AND g.task_id=trusted_channels.task_id AND g.recipient=trusted_channels.role "
                "AND g.engine_session_ref=? AND NOT EXISTS (SELECT 1 FROM context_generations newer "
                "WHERE newer.engine_session_ref=g.engine_session_ref AND newer.sequence>g.sequence))",
                (generation_id, channel_id, task_id, attempt_id, store._now(), session_ref,
                 generation_id, session_ref),
            ).rowcount
            if changed != 1:
                raise StoreError("session_skill_binding_invalid")
            db.commit()
        except BaseException:
            db.rollback()
            raise


def revoke_session_skills(store, session_ref: str) -> None:
    """Remove reads before termination, even when process exit is uncertain."""
    try:
        with store.connection() as db:
            db.execute("DELETE FROM session_skill_paths WHERE session_ref=?", (session_ref,))
    except sqlite3.OperationalError as error:
        if "no such table: session_skill_paths" not in str(error):
            raise


def _read_regular_nofollow(base: str, real: str) -> str:
    if not _within(real, base) or not real.endswith(os.sep + "SKILL.md"):
        raise ValueError("skill path escaped its base")
    parts = os.path.relpath(real, base).split(os.sep)
    directory = os.open(os.sep, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in base.split(os.sep):
            if not part:
                continue
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
            os.close(directory)
            directory = child
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
            os.close(directory)
            directory = child
        file_fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory)
        try:
            metadata = os.fstat(file_fd)
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1
                    or metadata.st_size > MAX_SKILL_BYTES):
                raise ValueError("skill is not a bounded regular file")
            chunks, total = [], 0
            while total <= MAX_SKILL_BYTES:
                chunk = os.read(file_fd, min(8192, MAX_SKILL_BYTES + 1 - total))
                if not chunk:
                    break
                chunks.append(chunk)
                total += len(chunk)
            data = b"".join(chunks)
            if len(data) > MAX_SKILL_BYTES:
                raise ValueError("skill exceeds size limit")
            return data.decode("utf-8")
        finally:
            os.close(file_fd)
    finally:
        os.close(directory)


def read_session_skill(store, channel, listed_path: str) -> dict:
    """Resolve the live persisted binding, not an MCP server's cached context.

    tools/list can resolve the channel before session/new finishes. Its
    cached context then has no generation, and also outlives compactions.
    Only the current trusted row may select the session catalog.
    """
    if (store is None or channel is None
            or not getattr(channel, "task_id", None)
            or not getattr(channel, "attempt_id", None)
            or not getattr(channel, "channel_id", None)
            or not getattr(channel, "role", None)):
        raise StoreError("unknown_skill", "skill", "unbound session", "a bound session channel")
    try:
        with store.connection() as db:
            row = db.execute(
                "SELECT s.real_path, s.base_path, s.title FROM trusted_channels c "
                "JOIN context_generations g ON g.id=c.context_generation_id "
                "JOIN session_skill_paths s ON s.session_ref=g.engine_session_ref "
                "AND s.task_id=g.task_id AND s.attempt_id=? "
                "WHERE c.channel_id=? AND c.task_id=? AND c.role=? AND g.recipient=c.role "
                "AND COALESCE(c.attempt_id,'')=s.attempt_id AND g.task_id=c.task_id "
                "AND s.listed_path=? AND c.revoked_at IS NULL "
                "AND (c.expires_at IS NULL OR c.expires_at>?) "
                "AND NOT EXISTS (SELECT 1 FROM context_generations newer "
                "WHERE newer.engine_session_ref=g.engine_session_ref AND newer.sequence>g.sequence)",
                (channel.attempt_id or "", channel.channel_id, channel.task_id, channel.role,
                 listed_path, store._now()),
            ).fetchone()
    except sqlite3.OperationalError as error:
        if "no such table: session_skill_paths" not in str(error):
            raise
        row = None
    if row is None:
        raise StoreError("unknown_skill", "skill", "path not listed", "an exact SKILL.md path listed in this session")
    real, base = row["real_path"], row["base_path"]
    # A listed symlink may be retargeted after session creation. Never follow
    # it to a different file, even another file inside the same skill folder.
    source = listed_path if os.path.isabs(listed_path) else os.path.join(base, listed_path)
    if os.path.realpath(source) != real:
        raise StoreError("unknown_skill", "skill", "skill target changed", "the originally listed SKILL.md")
    try:
        body = _read_regular_nofollow(base, real)
    except (OSError, UnicodeError, ValueError) as error:
        raise StoreError("skill_unavailable", "skill", type(error).__name__,
                         f"a UTF-8 regular SKILL.md no larger than {MAX_SKILL_BYTES} bytes") from error
    return dict(skill=listed_path, title=row["title"], body=body)
