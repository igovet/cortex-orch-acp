"""Durable one-shot native-auth replacement fences; never route fallback."""
import json

from ..contracts import StoreError
from ..engine.auth_recovery import login_blocker
from ..delegation.events import append_event_tx
from ..portable.identity import new_id


def claim_coordinator(store, task_id):
    """Spend the allowance before teardown, including across runtime restarts.

    Opening a session is not recovery: only a completed prompt ends an episode.
    """
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        previous = db.execute(
            "SELECT json_extract(payload_json,'$.kind') AS type FROM task_events WHERE task_id=? "
            "AND event_type='progress' AND json_extract(payload_json,'$.kind') IN "
            "('COORDINATOR_AUTH_RETRY','COORDINATOR_AUTH_BLOCKED','COORDINATOR_AUTH_RECOVERED') ORDER BY id DESC LIMIT 1",
            (task_id,)).fetchone()
        if previous and previous['type'] != 'COORDINATOR_AUTH_RECOVERED':
            append_event_tx(db, task_id, 'progress', new_id('auth_'),
                            dict(kind='COORDINATOR_AUTH_BLOCKED'), 'runtime')
            db.commit()
            raise StoreError(login_blocker())
        append_event_tx(db, task_id, 'progress', new_id('auth_'),
                        dict(kind='COORDINATOR_AUTH_RETRY', reason='authorization_loss'), 'runtime')
        db.commit()


def coordinator_recovered(store, task_id):
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        previous = db.execute(
            "SELECT json_extract(payload_json,'$.kind') AS type FROM task_events WHERE task_id=? "
            "AND event_type='progress' AND json_extract(payload_json,'$.kind') IN "
            "('COORDINATOR_AUTH_RETRY','COORDINATOR_AUTH_BLOCKED','COORDINATOR_AUTH_RECOVERED') ORDER BY id DESC LIMIT 1",
            (task_id,)).fetchone()
        if previous and previous['type'] != 'COORDINATOR_AUTH_RECOVERED':
            append_event_tx(db, task_id, 'progress', new_id('auth_'),
                            dict(kind='COORDINATOR_AUTH_RECOVERED'), 'runtime')
        db.commit()


def check_coordinator(store, task_id, probe_source=None):
    """A retained terminal auth blocker cannot launch again during recovery.

    An explicitly identified new user input may probe once after manual login.
    It does not replenish the retry allowance; only a successful prompt does.
    """
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        previous = db.execute(
            "SELECT json_extract(payload_json,'$.kind') AS type FROM task_events WHERE task_id=? "
            "AND event_type='progress' AND json_extract(payload_json,'$.kind') IN "
            "('COORDINATOR_AUTH_RETRY','COORDINATOR_AUTH_BLOCKED','COORDINATOR_AUTH_RECOVERED') "
            "ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
        if previous and previous['type'] == 'COORDINATOR_AUTH_BLOCKED':
            used = probe_source is None or db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND json_extract(payload_json,'$.kind')='COORDINATOR_AUTH_PROBE' "
                "AND json_extract(payload_json,'$.source_origin')=? "
                "AND json_extract(payload_json,'$.source_event_id')=?",
                (task_id, *probe_source)).fetchone()
            if used:
                raise StoreError(login_blocker())
            append_event_tx(db, task_id, 'progress', new_id('auth_'),
                            dict(kind='COORDINATOR_AUTH_PROBE', source_origin=probe_source[0],
                                 source_event_id=probe_source[1]), 'runtime')
        db.commit()


def worker_route(store, task_id, attempt_id):
    """The previous audited identity for this node's one auth retry, or None."""
    with store.connection() as db:
        node = db.execute(
            "SELECT n.node_id,n.revision FROM delegation_provisioning p JOIN runtime_nodes n "
            "ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
            "WHERE p.task_id=? AND p.attempt_id=?", (task_id, attempt_id)).fetchone()
        if node is None:
            return None
        retry = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED' "
            "AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=? "
            "ORDER BY rowid DESC LIMIT 1", (task_id, node['node_id'], node['revision'])).fetchone()
        payload = json.loads(retry[0]) if retry else {}
        if not payload.get('auth_retry'):
            return None
        audit = db.execute(
            "SELECT a.* FROM attempt_route_audit a JOIN delegation_provisioning p "
            "ON p.task_id=a.task_id AND p.attempt_id=a.attempt_id "
            "WHERE p.task_id=? AND p.idempotency_key=? ORDER BY a.rowid DESC LIMIT 1",
            (task_id, payload['previous_assignment'])).fetchone()
        if audit is None:
            raise StoreError(login_blocker() + ' Previous route identity is unavailable.')
        return dict(audit)
