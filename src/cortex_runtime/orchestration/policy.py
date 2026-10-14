"""Durable steering queue, boundary application, and explicit interruption."""
import json
import re

from ..contracts import StoreError
from .state import event, transition, now


def denied_paths(db, task_id):
    rows = db.execute(
        "SELECT payload_json FROM runtime_events e WHERE task_id=? AND type='USER_STEERING' "
        "AND EXISTS (SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id "
        "AND a.type='STEERING_APPLIED' AND a.actor=e.actor) ORDER BY id",
        (task_id,),
    )
    return sorted({path for row in rows for path in json.loads(row[0]).get('deny_write', [])})


_DENY_WRITE=re.compile(r'deny-write\s*:\s*(?:"([^"]+)"|\'([^\']+)\'|`([^`]+)`|(\S+))',re.I)
_WILDCARD=re.compile(r'[*?\[\]]')


def parse_denials(text):
    """Recognize only an explicit 'deny-write: <path>' directive (owner
    decision D7): no natural-language phrasing, and no wildcard or masked
    path is ever accepted as a denial. The path may be quoted."""
    from .gateway import _parts
    values=[]
    for match in _DENY_WRITE.finditer(text):
        value=next(g for g in match.groups() if g is not None).strip().rstrip('.,;')
        if not value or _WILDCARD.search(value):
            continue
        # Store the same normalized path parts the gateway's write-effect
        # check uses, so './secrets/key.txt' and 'secrets/key.txt' denote
        # one denial (C-10).
        parts=_parts(value)
        if parts:
            values.append('/'.join(parts))
    return sorted(set(values))


def overlaps(left,right):
    """Conservative overlap: wildcard prefixes cannot justify concurrency
    or override a denial. Exact disjoint directory roots remain parallel.
    """
    def prefix(value):
        pieces=re.split(r'[*?\[]',value,1)
        value=pieces[0].rstrip('/')
        return ('' if value=='.' else value),len(pieces)>1
    (a,aw),(b,bw)=prefix(left),prefix(right)
    return (not a or not b or a==b or a.startswith(b+'/') or b.startswith(a+'/')
            or aw and b.startswith(a) or bw and a.startswith(b))


def validate_grants(db,task_id,scopes):
    if any(overlaps(scope,denied) for scope in scopes for denied in denied_paths(db,task_id)):
        raise StoreError('scope_conflicts_with_user_policy')


def steer(store, task_id, text, identity):
    """Durably queue steering without changing grants or active assignments."""
    denied = parse_denials(text)
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if not task or task['state'] == 'CANCELLED':
            db.commit()
            return False
        existing = db.execute(
            "SELECT 1 FROM runtime_events WHERE task_id=? AND type='USER_STEERING' AND actor=?",
            (task_id, identity),
        ).fetchone()
        if existing:
            db.commit()
            return False
        event(db, task_id, 'USER_STEERING', dict(text=text, deny_write=denied), identity)
        db.commit()
    return True


def has_pending_steering(store, task_id):
    with store.connection() as db:
        return db.execute(
            "SELECT 1 FROM runtime_events e WHERE e.task_id=? AND e.type='USER_STEERING' "
            "AND NOT EXISTS (SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id "
            "AND a.type='STEERING_APPLIED' AND a.actor=e.actor) LIMIT 1",
            (task_id,),
        ).fetchone() is not None


def apply_pending_steering(store, task_id):
    """Apply queued requirements only after active work reaches a safe boundary.

    The controller calls this at planning/reconciliation boundaries. All current
    revision workers, review and verification must finish before a new plan.
    """
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task = db.execute('SELECT state,revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if not task or task['state'] not in {
            'PLANNING', 'PLAN_READY', 'ANSWER_READY', 'RECONCILING', 'COMPLETED', 'FAILED',
        }:
            db.commit()
            return False
        pending = db.execute(
            "SELECT e.actor FROM runtime_events e WHERE e.task_id=? AND e.type='USER_STEERING' "
            "AND NOT EXISTS (SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id "
            "AND a.type='STEERING_APPLIED' AND a.actor=e.actor) ORDER BY e.id",
            (task_id,),
        ).fetchall()
        if not pending:
            db.commit()
            return False
        if db.execute(
            "SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND status='running'",
            (task_id, task['revision']),
        ).fetchone():
            db.commit()
            return False
        if task['state'] == 'PLAN_READY':
            db.execute(
                "UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND revision=? AND status='pending'",
                (task_id, task['revision']),
            )
        if task['state'] in {'COMPLETED', 'FAILED'}:
            db.execute(
                "UPDATE runtime_tasks SET state='RECOVERING',synthesis=NULL,blocker=NULL,updated_at=? "
                "WHERE task_id=?", (now(), task_id),
            )
            event(db, task_id, 'TASK_REOPENED', dict(previous=task['state'], revision=task['revision']))
        else:
            transition(db, task_id, 'RECOVERING')
            db.execute("UPDATE runtime_tasks SET synthesis=NULL WHERE task_id=?", (task_id,))
        for row in pending:
            event(db, task_id, 'STEERING_APPLIED', {}, row['actor'])
        transition(db, task_id, 'PLANNING')
        db.commit()
    return True


def interrupt(store, task_id):
    """Explicit turn cancellation fences unfinished work without creating steering."""
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if task and task['state'] not in {'COMPLETED', 'FAILED', 'CANCELLED'}:
            if task['state'] != 'RECOVERING':
                transition(db, task_id, 'RECOVERING')
            # The cancelled workers' open questions end with them; otherwise
            # the next user message is swallowed as a late answer to one.
            from .worker_waits import cancel_tx
            cancel_tx(db, task_id, 'turn_interrupted')
            db.execute(
                "UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",
                (task_id,),
            )
            db.execute(
                "UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
                (now(), task_id),
            )
            event(db, task_id, 'TURN_INTERRUPTED', {})
        db.commit()


def resume_planning(store,task_id):
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task=db.execute('SELECT state FROM runtime_tasks WHERE task_id=?',(task_id,)).fetchone()
        if task and task['state']=='RECOVERING':
            transition(db,task_id,'PLANNING')
        db.commit()


REPLAN_LIMIT=3


def replans_since_user_input(db,task_id):
    """Accepted replans since the user last spoke (a message or an answer).

    The limit bounds autonomous replanning; any later user input is the
    user's decision and starts a fresh budget."""
    spoke=db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type IN ('USER_STEERING','USER_ANSWER_RECEIVED')",
                     (task_id,)).fetchone()[0] or 0
    return db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED' AND id>?",
                      (task_id,spoke)).fetchone()[0]


def propose_replan(store,channel,operation,args):
    import hashlib
    from .state import encoded,validate_scope
    from .gateway import _authority
    digest=hashlib.sha256(encoded(args).encode()).hexdigest()
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        previous=db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                            (channel.task_id,channel.channel_id,args['request_key'])).fetchone()
        if previous:
            if previous['operation']!=operation or previous['digest']!=digest: raise StoreError('request_key_conflict')
            return json.loads(previous['result_json'])
        _authority(db,channel)
        if operation=='scope_request':
            validate_scope(args['scope']);validate_grants(db,channel.task_id,args['scope'])
        if replans_since_user_input(db,channel.task_id)>=REPLAN_LIMIT: raise StoreError('replan_limit_requires_user_input')
        transition(db,channel.task_id,'RECOVERING')
        from .worker_waits import cancel_tx
        cancel_tx(db, channel.task_id, 'replanning')
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",(channel.task_id,))
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",(now(),channel.task_id))
        revision=db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?',(channel.task_id,)).fetchone()[0]
        receipt=dict(accepted=True,state='RECOVERING',decision='senior_consultation' if operation=='escalation_request' else 'replan',scope_granted=False,
                     next='End this turn. The runtime will stop workers and request the new plan.')
        event(db,channel.task_id,'REPLAN_REQUEST_ACCEPTED',dict(operation=operation,proposal=args,decision=receipt,revision=revision),channel.channel_id)
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id,args['request_key'],channel.channel_id,operation,digest,encoded(receipt)))
        db.commit()
    return receipt
