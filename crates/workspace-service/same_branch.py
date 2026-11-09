"""Task-owned content evidence for a shared branch; unknown never means landed."""
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import time


def recorded_content(h, root, task):
    """Use successful channel-bound file receipts, never scope names or model prose."""
    if not task:
        return None
    db = h.store_reader(root)
    if db is None:
        return None
    try:
        execution = db.execute(
            "SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e "
            "ON e.execution_id=t.execution_id WHERE t.task_id=?", (task,)).fetchone()
        if execution is None:
            return None
        content = {}
        rows = db.execute(
                "SELECT e.payload_json,e.created_at FROM runtime_events e JOIN trusted_channels c "
                "ON c.channel_id=e.actor AND c.task_id=e.task_id "
                "WHERE e.task_id=? AND e.type='TOOL_COMPLETED' ORDER BY e.id LIMIT 1001",
                (execution["runtime_task_id"],)).fetchall()
        if len(rows) > 1000:
            return None
        for row in rows:
            value = json.loads(row[0])
            if value.get("operation") not in ("project_write", "project_edit", "project_delete"):
                continue
            name = h.relative(value["path"])
            result = value.get("result") or {}
            sha = result.get("full_content_sha256")
            # Deletes, links and incomplete receipts cannot authorize a file commit.
            content[name] = (sha if isinstance(sha, str) and re.fullmatch("[0-9a-f]{64}", sha) else None, row["created_at"])
        return content if content and len(content) <= 100 else None
    except (sqlite3.Error, ValueError, KeyError, TypeError, h.Refused):
        return None
    finally:
        db.close()


def blob_hash(h, path, ref, name):
    spec = ref + ":" + name
    size = h.out(path, "cat-file", "-s", spec, check=False).strip()
    if not size.isdecimal() or int(size) > 2_000_000:
        return None
    return hashlib.sha256(h.git(path, "cat-file", "blob", spec).stdout).hexdigest()


def facts(h, root, path, record, task, target):
    result = dict(sameBranch=True, alreadyInTarget=False, commitAvailable=False,
                  pendingPaths=[], otherUncommitted=False,
                  reason="Task-owned file evidence is unavailable. Close as is, or use Done outside the app after finishing in Git.")
    content = recorded_content(h, root, task)
    if not content or any(value[0] is None for value in content.values()):
        return result
    dirty = set(h.out(path, "diff", "--name-only", "-z", "HEAD").split("\0"))
    dirty.update(h.out(path, "ls-files", "--others", "--exclude-standard", "-z").split("\0"))
    dirty = {p for p in dirty if p and not __import__("task_git").helper_path(p)}
    result["otherUncommitted"] = bool(dirty - content.keys())
    pending, unknown = [], []
    deadline = time.monotonic() + 4
    for name, (wanted, written_at) in sorted(content.items()):
        if time.monotonic() >= deadline:
            return result
        # Matching any landed version survives later edits to the same file.
        # A known baseline bounds history so an earlier version cannot stand in for a reversal.
        if blob_hash(h, path, target, name) == wanted:
            continue
        base = record.get("baseRevision")
        history = base + ".." + target if base else target
        try:
            since = datetime.fromisoformat(written_at.replace("Z", "+00:00")).timestamp()
        except (TypeError, ValueError, AttributeError):
            since = None
        landed = False
        for line in h.out(path, "log", "-n", "128", "--format=%H %ct", history, "--", ":(literal)" + name).splitlines():
            if time.monotonic() >= deadline:
                return result
            ref, at = line.split()
            # With no Git baseline, an older historical copy is not proof this work landed.
            if not base and (since is None or int(at) < since):
                continue
            if blob_hash(h, path, ref, name) == wanted:
                landed = True
                break
        if landed:
            continue
        file = Path(path) / name
        staged = h.out(path, "diff", "--cached", "--name-only", "--", ":(literal)" + name).strip()
        if (any(p.is_symlink() for p in [file, *file.parents] if p != Path(path)) or not file.is_file() or file.stat().st_size > 2_000_000
                or hashlib.sha256(file.read_bytes()).hexdigest() != wanted
                or (staged and blob_hash(h, path, "", name) != wanted)
                or "mode change" in h.out(path, "diff", "--summary", "HEAD", "--", ":(literal)" + name)):
            unknown.append(name)
        else:
            pending.append(name)
    # A landed path that is dirty again was changed by someone else; own pending and unknown paths were not.
    result["otherUncommitted"] = bool(dirty - set(pending) - set(unknown))
    result["pendingPaths"] = pending
    result["alreadyInTarget"] = not pending and not unknown
    result["commitAvailable"] = bool(pending) and not unknown
    result["reason"] = ("Task files changed again or their recorded content cannot be located. Finish in Git and use Done outside the app, or close as is."
                        if unknown else None)
    return result


def commit(h, path, paths, message):
    # --only does not sweep already-staged unrelated paths into this commit.
    # The caller rechecks content and uses the existing index rollback transaction.
    if any(__import__("task_git").helper_path(p) for p in h.out(path, "diff", "--cached", "--name-only").splitlines()):
        raise h.Refused("private_index", "Unstage Workspace helper data before committing.")
    h.git(path, "add", "-A", "--", *[":(literal)" + p for p in paths])
    h.git(path, "commit", "--only", "-m", message, "--", *[":(literal)" + p for p in paths])
