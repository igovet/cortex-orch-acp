"""Durable constraints and revocation before delivering user steering."""
import json
import re

from ..contracts import StoreError
from .state import event, transition, now


def denied_paths(db,task_id):
    rows=db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_STEERING' ORDER BY id",(task_id,))
    return sorted({path for row in rows for path in json.loads(row[0]).get('deny_write',[])})


def parse_denials(text):
    """Recognize explicit path prohibitions. Unstructured steering still
    revokes all old assignments before replanning; it cannot preserve grants.
    """
    from .gateway import _parts
    values=[]
    pattern=r'(?:deny-write\s*:|do\s+not\s+(?:change|modify|edit)|не\s+(?:меняй|менять|изменяй|изменять|трогай|трогать))\s+[`\"\']?([\w./*-]+)'
    for match in re.finditer(pattern,text,re.I):
        value=match.group(1).rstrip('.')
        if value:
            _parts(value)
            values.append(value.rstrip('/'))
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


def steer(store,task_id,text,identity):
    denied=parse_denials(text)
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task=db.execute('SELECT state FROM runtime_tasks WHERE task_id=?',(task_id,)).fetchone()
        if not task: return False
        existing=db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='USER_STEERING' AND actor=?",(task_id,identity)).fetchone()
        if existing: return False
        if task['state'] in {'CANCELLED','FAILED'}: return False
        # A new user request can extend completed work; it creates a new plan
        # revision and invalidates the prior completion, never edits its report.
        if task['state']=='COMPLETED':
            db.execute("UPDATE runtime_tasks SET state='RECOVERING',synthesis=NULL,blocker=NULL WHERE task_id=?",(task_id,))
            event(db,task_id,'TASK_REOPENED',{},identity)
        elif task['state']!='RECOVERING':
            transition(db,task_id,'RECOVERING')
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",(task_id,))
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
                   (now(),task_id))
        event(db,task_id,'USER_STEERING',dict(text=text,deny_write=denied),identity)
        db.commit()
    return True


def resume_planning(store,task_id):
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task=db.execute('SELECT state FROM runtime_tasks WHERE task_id=?',(task_id,)).fetchone()
        if task and task['state']=='RECOVERING':
            transition(db,task_id,'PLANNING')
        db.commit()


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
        count=db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED'",(channel.task_id,)).fetchone()[0]
        if count>=3: raise StoreError('replan_limit_requires_user_input')
        transition(db,channel.task_id,'RECOVERING')
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",(channel.task_id,))
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",(now(),channel.task_id))
        revision=db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?',(channel.task_id,)).fetchone()[0]
        receipt=dict(accepted=True,state='RECOVERING',decision='senior_consultation' if operation=='escalation_request' else 'replan',scope_granted=False)
        event(db,channel.task_id,'REPLAN_REQUEST_ACCEPTED',dict(operation=operation,proposal=args,decision=receipt,revision=revision),channel.channel_id)
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id,args['request_key'],channel.channel_id,operation,digest,encoded(receipt)))
        db.commit()
    return receipt
