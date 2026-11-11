"""Bounded owner-supplied bytes, never client-supplied filesystem paths."""
import base64
import hashlib
import re
from dataclasses import replace
from urllib.parse import quote

from ..acp import replay_store
from ..workspace_contract import LIMITS, validate
from .storage import WorkspaceError

MAX_BYTES = LIMITS["attachmentBytes"]
MAX_FILES = LIMITS["attachmentFiles"]


def checked(items):
    validate("Attachments", items)
    total = 0
    for item in items:
        if (re.search(r"[\\/\x00-\x1f\x7f]", item["name"])
                or not re.fullmatch(r"[a-zA-Z0-9.+_-]+/[a-zA-Z0-9.+_-]+", item["mimeType"])):
            raise WorkspaceError("invalid_attachment")
        try:
            body = base64.b64decode(item["data"], validate=True)
        except (ValueError, TypeError):
            raise WorkspaceError("invalid_attachment") from None
        if base64.b64encode(body).decode("ascii") != item["data"]:
            raise WorkspaceError("invalid_attachment")
        total += len(body)
    if total > MAX_BYTES:
        raise WorkspaceError("attachments_too_large")
    return items


def content(items):
    """Inline ACP resources snapshot bytes; no URI is fetched or opened."""
    result = []
    for item in checked(items):
        uri = "attachment:///" + hashlib.sha256(base64.b64decode(item["data"])).hexdigest() + "/" + quote(item["name"], safe="")
        if item["mimeType"].startswith("image/"):
            result.append(dict(type="image", mimeType=item["mimeType"], data=item["data"]))
        else:
            result.append(dict(type="resource", resource=dict(uri=uri, mimeType=item["mimeType"], blob=item["data"])))
    return result


def stage(project_root, items):
    blocks = replay_store.resolve_prompt_blocks(content(items), project_root)
    blocks = [replace(block, name=item["name"]) for block, item in zip(blocks, items)]
    staged = replay_store.stage_blocks(project_root, blocks)
    if any(b.capture.availability != "stored" for b in staged):
        replay_store.cleanup_staged(staged)
        raise WorkspaceError("attachment_capture_failed")
    return staged


def answer_source_tx(store, db, task_id, key, text, blocks, rollbacks, staged, *, origin="workspace-answer"):
    """Answer and its resource source commit together on the caller's transaction."""
    principal = db.execute("SELECT thread_id FROM thread_bindings WHERE task_id=? AND parent_thread_id IS NULL LIMIT 1",
                           (task_id,)).fetchone()
    if principal is None:
        raise WorkspaceError("task_not_bound")
    report, _, _ = store.bootstrap_source(db, task_id, principal[0], origin + ":" + key, text, rollbacks, staged)
    replay_store.record_blocks_tx(store, db, task_id, report, blocks, rollbacks, staged)
    db.execute("INSERT INTO source_events VALUES (?,?,?,?,?,NULL)", (origin, key, task_id, store._now(), None))
    from ..acp.steering import describe_attachments
    from ..delegation.events import append_event_tx
    descriptors = [dict(source_ref=f"{report}:{b.capture.block_index}", name=b.capture.name,
                        kind=b.capture.kind, availability="stored") for b in blocks]
    append_event_tx(db, task_id, "user_message", origin + "-resources:" + key,
                    dict(content=describe_attachments(descriptors), attachments=descriptors), origin=origin)
    return descriptors
