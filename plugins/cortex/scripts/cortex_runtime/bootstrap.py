"""Durable, idempotent task bootstrap outside model-originated tool calls.

Persists the initial user input and task binding *before* any coordinator
delivery, in one ``BEGIN IMMEDIATE`` transaction, deduplicated by source
event identity (``origin``, ``event_id``) rather than text equality: two
identical texts under distinct event ids remain distinct tasks, and a
replayed event never creates a second task.  No model call is made here.
"""
from __future__ import annotations

from dataclasses import dataclass
import uuid

from .contracts import StoreError
from .store import unique_identifier, project_draft_directory
from .ownership import acquire_task_owner_tx
from .trusted_channel import issue_channel_tx

_MAX_IDENTITY_LENGTH = 512


@dataclass(frozen=True)
class BootstrapResult:
    task_id: str
    channel_id: str | None
    secret: str | None
    replayed: bool


def _bounded_text(value, field_name):
    if not isinstance(value, str) or not value or len(value) > _MAX_IDENTITY_LENGTH:
        raise StoreError("invalid_source_event", field_name, value, "non-empty string of at most 512 characters")
    return value


def bootstrap_task(store, source_event, user_input, owner_ref):
    """Create (or recover) exactly one task for ``source_event``.

    ``store`` must already be constructed against the target project root.
    On replay (an ``(origin, event_id)`` already recorded), returns the
    existing ``task_id`` with ``channel_id``/``secret`` both ``None`` and
    ``replayed=True``: the caller must issue a fresh coordinator channel via
    ``trusted_channel.issue_channel`` if it lost the original secret. A crash
    before commit leaves nothing persisted; a crash after commit is safely
    recoverable by bootstrapping the identical event again.
    """
    origin = _bounded_text(source_event.origin, "origin")
    event_id = _bounded_text(source_event.event_id, "event_id")
    if not isinstance(user_input, str) or not user_input:
        raise StoreError("invalid_source_event", "user_input", user_input, "non-empty string")
    owner_ref = _bounded_text(owner_ref, "owner_ref")
    if store.project_root is None:
        raise StoreError("project_context_unavailable")
    project = store.project_root
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT task_id FROM source_events WHERE origin=? AND event_id=?",
                (origin, event_id)).fetchone()
            if existing is not None:
                db.commit()
                return BootstrapResult(task_id=existing["task_id"], channel_id=None, secret=None, replayed=True)
            project_draft_directory(project, "report")
            project_draft_directory(project, "pipeline")
            task = unique_identifier(db, "t_")
            db.execute("INSERT INTO tasks VALUES (?,?,?)", (task, project, store._now()))
            principal = str(uuid.uuid4())
            db.execute("INSERT INTO thread_bindings VALUES (?,NULL,?)", (principal, task))
            rollbacks, staged = [], []
            try:
                store.bootstrap_source(db, task, principal, origin+":"+event_id, user_input, rollbacks, staged)
            except BaseException:
                store._rollback_files(rollbacks)
                for path in staged: path.unlink(missing_ok=True)
                raise
            db.execute("INSERT INTO source_events VALUES (?,?,?,?)", (origin, event_id, task, store._now()))
            epoch = acquire_task_owner_tx(db, task, owner_ref, None)
            channel_id, secret = issue_channel_tx(
                db, task, "coordinator", epoch.epoch, principal_thread_id=principal)
            db.commit()
        except BaseException:
            db.rollback(); raise
        store._finalize_files(rollbacks)
    return BootstrapResult(task_id=task, channel_id=channel_id, secret=secret, replayed=False)
