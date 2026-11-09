"""Owner guidance to one live attempt, consumed only at a Cortex tool boundary."""
import json

from ..contracts import StoreError
from ..workspace_api.storage import WorkspaceError
from .state import event, now


def capture(store, task_id, channel_id, request_id, text, *, attachments=None):
    if not isinstance(text, str) or not text.strip() or len(text) > 32000:
        raise WorkspaceError("invalid_message")
    from ..workspace_api import attachments as resources
    from ..acp import replay_store
    files = resources.checked(attachments or [])
    from ..workspace_api.storage import digest
    fingerprint = digest([channel_id, text, files])
    blocks = resources.stage(store.project_root, files) if files else []
    rollbacks, staged = [], [b.staged_path for b in blocks if b.staged_path is not None]
    def write(db):
        channel = db.execute(
            "SELECT c.* FROM trusted_channels c JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch "
            "JOIN runtime_tasks r ON r.task_id=c.task_id WHERE c.channel_id=? AND c.task_id=? "
            "AND c.role='worker' AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>?) "
            "AND r.state NOT IN ('COMPLETED','CANCELLED','FAILED')", (channel_id, task_id, now())).fetchone()
        if channel is None:
            raise WorkspaceError("worker_unavailable")
        prior = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_GUIDANCE' "
                           "AND json_extract(payload_json,'$.requestId')=?", (task_id, request_id)).fetchone()
        if prior:
            old = json.loads(prior[0])
            if old["channelId"] != channel_id or old["text"] != text or old.get("fingerprint", digest([channel_id, old["text"], []])) != fingerprint:
                raise WorkspaceError("payload_conflict")
            return dict(outcome="recorded")
        descriptors = resources.answer_source_tx(store, db, task_id, "guidance:" + request_id, text,
            blocks, rollbacks, staged, origin="workspace-worker") if blocks else []
        event(db, task_id, "WORKER_GUIDANCE", dict(requestId=request_id, channelId=channel_id,
              attemptId=channel["attempt_id"], text=text, fingerprint=fingerprint, attachments=descriptors,
              attachmentNames=[{"name": f["name"], "mimeType": f["mimeType"]} for f in files]), "user:workspace")
        return dict(outcome="recorded")
    try:
        result = store.immediate_write(write)
    except BaseException:
        store._rollback_files(rollbacks)
        raise
    else:
        store._finalize_files(rollbacks)
        return result
    finally:
        replay_store.cleanup_staged(blocks)


def deliver(store, channel):
    if channel.role != "worker":
        return []
    def write(db):
        from .gateway import channel_authority
        try:
            channel_authority(db, channel)
        except StoreError:
            return []
        rows = db.execute(
            "SELECT e.id,e.payload_json FROM runtime_events e WHERE e.task_id=? AND e.type='WORKER_GUIDANCE' "
            "AND json_extract(e.payload_json,'$.channelId')=? AND NOT EXISTS "
            "(SELECT 1 FROM runtime_events d WHERE d.task_id=e.task_id AND d.type IN ('WORKER_GUIDANCE_DELIVERED','WORKER_GUIDANCE_WITHDRAWN') "
            "AND json_extract(d.payload_json,'$.messageEvent')=e.id) ORDER BY e.id LIMIT 20",
            (channel.task_id, channel.channel_id)).fetchall()
        result = []
        for row in rows:
            payload = json.loads(row["payload_json"])
            item = dict(id=payload["requestId"], text=payload["text"], attachments=payload.get("attachments", []),
                instruction="Owner guidance, verbatim. Changes no scope, permissions, review or acceptance criteria.")
            # Leave overflow pending for the next successful tool response.
            if result and len(json.dumps(result + [item], ensure_ascii=False).encode("utf-8")) > 128 * 1024:
                break
            event(db, channel.task_id, "WORKER_GUIDANCE_DELIVERED",
                  dict(messageEvent=row["id"], requestId=payload["requestId"], channelId=channel.channel_id),
                  channel.channel_id)
            result.append(item)
        return result
    return store.immediate_write(write)
