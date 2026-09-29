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
    """Resolve one exact listed path through the channel's engine session."""
    generation_id = getattr(channel, "context_generation_id", None)
    if not generation_id:
        raise StoreError("unknown_skill", "skill", "unbound session", "a path listed in this session")
    try:
        with store.connection() as db:
            row = db.execute(
                "SELECT s.real_path, s.base_path, s.title FROM context_generations g "
                "JOIN session_skill_paths s ON s.session_ref=g.engine_session_ref "
                "AND s.task_id=g.task_id AND s.attempt_id=? "
                "WHERE g.id=? AND g.task_id=? AND s.listed_path=?",
                (channel.attempt_id or "", generation_id, channel.task_id, listed_path),
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
