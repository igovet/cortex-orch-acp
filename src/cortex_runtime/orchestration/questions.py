"""Atomic question identities and deterministic resume after a real answer."""
import hashlib
import json

from ..contracts import StoreError
from ..portable.identity import new_id
from .state import encoded, event, now, transition


def request(store, channel, args):
    digest = hashlib.sha256(encoded(args).encode()).hexdigest()
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                              (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
        if previous:
            if previous['operation'] != 'user_question_request' or previous['digest'] != digest:
                raise StoreError('request_key_conflict')
            return json.loads(previous['result_json'])
        task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
        allowed={'PLANNING','RECONCILING'} if channel.role=='coordinator' else {'EXECUTING','REVIEWING'}
        if not task or task['state'] not in allowed:
            raise StoreError('question_not_expected')
        resume_state=task['state']
        if channel.role=='worker':
            from .gateway import _authority
            _authority(db,channel)
            resume_state='PLANNING'
            db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",(channel.task_id,))
            db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",(now(),channel.task_id))
        question_id = new_id('pq_')
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) VALUES(?,?,?,?,?,'open',?)",
                   (question_id, channel.task_id, f'runtime:{channel.channel_id}:{args["request_key"]}',
                    args['question'], encoded(args.get('options')) if args.get('options') else None, now()))
        event(db, channel.task_id, 'USER_QUESTION_REQUESTED',
              dict(question_id=question_id, question=args['question'], options=args.get('options'), resume_state=resume_state), channel.channel_id)
        transition(db, channel.task_id, 'WAITING_USER')
        receipt = dict(question_id=question_id, state='WAITING_USER')
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id, args['request_key'], channel.channel_id, 'user_question_request', digest, encoded(receipt)))
        db.commit()
    return receipt


def pending(store, task_id):
    with store.connection() as db:
        row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
    if not row:
        raise StoreError('runtime_question_missing')
    return json.loads(row['payload_json'])


def resume_answered(store, task_id):
    """No elapsed-time, ordinary message or absent observation counts as an answer."""
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if not task or task['state'] != 'WAITING_USER':
            return False
        row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
        if not row:
            raise StoreError('runtime_question_missing')
        saved = json.loads(row['payload_json'])
        answer = db.execute('SELECT status,answer_text FROM pending_questions WHERE task_id=? AND id=?', (task_id, saved['question_id'])).fetchone()
        if not answer or answer['status'] != 'answered':
            return False
        event(db, task_id, 'USER_ANSWER_RECEIVED', dict(question_id=saved['question_id'], answer=answer['answer_text']))
        transition(db, task_id, saved['resume_state'])
        db.commit()
    return True
