"""Lossless offload of oversized, already-redacted worker activity envelopes."""
from __future__ import annotations

import hashlib

from ..acp.wire_budget import bound_value
from .large_output_store import persist_output

INLINE_MAX_BYTES = 64 * 1024
PREVIEW_FIELD_BYTES = 512


def offload_activity(store, task_id: str, event_ref: str, payload: dict, data: bytes) -> dict:
    """Keep small events inline; retain full large JSON under coordinator scope.

    Worker-panel history is runtime-owned diagnostic evidence, not worker tool
    output. Its reference is readable by the coordinator's existing scoped
    read_large_output API; no cross-worker read permission is introduced.
    """
    if len(data) <= INLINE_MAX_BYTES:
        return payload
    # Replay must reuse the original reference, not create another body for the
    # same event. The full-envelope digest is already part of event_ref.
    with store.connection() as db:
        row = db.execute(
            "SELECT id FROM large_outputs WHERE task_id=? AND owner_scope='coordinator' "
            "AND source_kind='tool_result' AND source_ref=? ORDER BY created_at LIMIT 1",
            (task_id, event_ref),
        ).fetchone()
    output_id = row["id"] if row else persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="tool_result",
        source_ref=event_ref, data=data,
    )["id"]
    preview = {key: bound_value(value, PREVIEW_FIELD_BYTES) for key, value in payload.items()}
    preview.update(
        payload_offloaded=True, large_output_id=output_id,
        sha256=hashlib.sha256(data).hexdigest(), byte_count=len(data),
    )
    # Existing plain-text cards/diagnostics also expose the retained evidence,
    # even when their projection does not know the structured offload fields.
    preview["text"] += f" [preview; full redacted activity: {output_id}, {len(data)} bytes]"
    return preview
