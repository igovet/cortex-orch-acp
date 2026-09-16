"""Legacy Codex MCP identity adapter, isolated from the portable runtime.

``thread_context`` consumes only host transport metadata (``_meta``), never
model-authored tool arguments.  It is the exact function previously defined
in ``server.py``; moved here verbatim so the portable dispatch seam (see
``server.Server.dispatch``) can treat native Codex ``x-codex-turn-metadata``
as one optional, clearly bounded legacy adapter rather than a mandatory
runtime precondition.  Behavior of the Codex path is unchanged.

Security note: this adapter itself performs no fencing. It only parses and
format-validates ``_meta``; it never authenticates a channel secret, and it
cannot by itself tell a legacy-created task from a channel-managed one. A
caller that learns (leaks, guesses, or is handed) a channel's internal
principal thread id could otherwise reuse it here as an ordinary Codex
``threadId``. The exclusion is enforced one layer down, in
``store.Store._resolve_thread``/``store.Store.call``: any task that has
acquired an owner (a ``task_owners`` row — see ``ownership``) is
channel-managed and rejects every call that did not arrive through
``server.Server._dispatch_channeled`` with a verified channel (``trusted=True``),
including reads and a legacy child thread trying to register under a leaked
principal as its parent. Do not rely on this adapter, or on ``_meta`` shape
alone, to keep the two identity paths apart.
"""
from __future__ import annotations

import uuid

from .contracts import StoreError


def thread_context(meta):
    """Consume only host transport metadata, never model-authored arguments."""
    if not isinstance(meta, dict) or not isinstance(meta.get('x-codex-turn-metadata'), dict):
        raise StoreError('thread_metadata_missing')
    thread = meta.get('threadId')
    parent = meta['x-codex-turn-metadata'].get('parent_thread_id')
    for value in [thread]+([parent] if parent is not None else []):
        try:
            if not isinstance(value, str) or str(uuid.UUID(value)) != value: raise ValueError
        except (ValueError, AttributeError): raise StoreError('thread_metadata_invalid') from None
    if thread == parent: raise StoreError('thread_metadata_invalid')
    return thread, parent


def is_legacy_meta(meta):
    """True only when ``_meta`` actually carries Codex turn metadata."""
    return isinstance(meta, dict) and isinstance(meta.get('x-codex-turn-metadata'), dict)
