"""Metadata-only, durable orchestration timing observations."""
import json

from .state import event


def attempt_event(store, task_id, attempt_id, kind, engine_session_id, payload=None):
    """Record each milestone once per actual downstream attempt, never content.

    ``payload`` is an optional mapping of additional metadata-only fields
    (never file/tool content) merged into the recorded event -- G5 uses it
    to record which session/update kind was first observed."""
    if kind not in {'ENGINE_READY', 'FIRST_MODEL_OUTPUT'}:
        raise ValueError('unknown attempt milestone')
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        if not db.execute('SELECT 1 FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone():
            return
        # Channel identity exists before provisioning finishes session/new.
        channel = db.execute(
            "SELECT assignment_id FROM trusted_channels WHERE task_id=? AND attempt_id=? "
            "AND role='worker' ORDER BY rowid DESC LIMIT 1", (task_id, attempt_id)).fetchone()
        if channel is None:
            return
        assignment = db.execute(
            "SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND assignment_id=?",
            (task_id, channel[0])).fetchone()
        if assignment is None:
            return
        if not db.execute(
                "SELECT 1 FROM runtime_events WHERE task_id=? AND type=? "
                "AND json_extract(payload_json,'$.attempt_id')=?",
                (task_id, kind, attempt_id)).fetchone():
            event(db, task_id, kind, dict(assignment_ref=assignment[0], attempt_id=attempt_id,
                                         engine_session_id=engine_session_id, **dict(payload or {})))
        db.commit()


def wait_signature(reason, blocked_by=None):
    """What makes a wait a new one: its reason and, for a scope wait, which
    holder and paths. A different holder of the same reason is a change the
    user should see; repeated passes over the same wait are not."""
    if not reason or not blocked_by:
        return (reason,)
    # A worker holder is its task and node; its assignment_ref only appears
    # once the node is claimed and is not a different holder.
    return (reason, blocked_by.get('kind'), blocked_by.get('task_id'), blocked_by.get('node'),
            blocked_by.get('assignment_ref') if blocked_by.get('kind') == 'access_request' else None,
            blocked_by.get('held'), blocked_by.get('limit'), blocked_by.get('retry'))


def wait_reason(db, task_id, identity, reason, **ids):
    """Persist changes only, including an explicit cleared (null) reason."""
    previous = db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SCHEDULER_WAIT_REASON' "
        "AND json_extract(payload_json,'$.identity')=? ORDER BY id DESC LIMIT 1",
        (task_id, identity)).fetchone()
    old = json.loads(previous[0]) if previous else {}
    if (wait_signature(old.get('reason'), old.get('blocked_by'))
            == wait_signature(reason, ids.get('blocked_by'))):
        return
    event(db, task_id, 'SCHEDULER_WAIT_REASON', dict(identity=identity, reason=reason, **ids))


def attempt_wait(store, task_id, attempt_id, reason, connection_id):
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        if not db.execute('SELECT 1 FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone():
            return
        wait_reason(db, task_id, attempt_id, reason, attempt_id=attempt_id, connection_id=connection_id)
        db.commit()
