"""Durable question groups over the existing question/event ledger.

Membership is immutable. Answers are source data, never an access decision.
No new storage format or permission grant is introduced.
"""
import hashlib
import json

from ..contracts import StoreError
from ..portable.identity import new_id
from .state import encoded, event, now


_PLACEHOLDERS = {'other', 'other...', 'other…', 'type your answer',
                 'reply in chat or type an answer under other…'}


def forms(args):
    if 'questions' in args:
        if any(k in args for k in ('question', 'options', 'skill_proposal')):
            raise StoreError('question_forms_conflict')
        questions = args['questions']
        if not isinstance(questions, list) or not 2 <= len(questions) <= 8:
            raise StoreError('question_batch_invalid')
    else:
        questions = [dict(question=args.get('question'), options=args.get('options'))]
    for question in questions:
        if (not isinstance(question, dict) or not {'question', 'options'} <= set(question)
                or not set(question) <= {'question', 'options', 'topic'}
                or not isinstance(question['question'], str) or not question['question'].strip()
                or len(question['question']) > 4000):
            raise StoreError('question_invalid')
        options = question['options']
        if (not isinstance(options, list) or not 2 <= len(options) <= 8
                or any(not isinstance(o, str) or not o.strip() or len(o) > 1000 for o in options)):
            raise StoreError('question_options_required', 'options', None,
                             '2 to 8 meaningful selectable answers; custom text is always separate')
        labels = [o.strip().casefold() for o in options]
        if len(set(labels)) != len(labels) or any(o in _PLACEHOLDERS for o in labels):
            raise StoreError('question_options_invalid')
        # The tab/topic label a batch UI shows instead of the raw question
        # id (T-topic). Optional: a client without it derives its own
        # fallback from the question text, never the id.
        if 'topic' in question:
            topic = question['topic']
            if not isinstance(topic, str) or not topic.strip() or len(topic) > 24:
                raise StoreError('question_topic_invalid', 'topic', None,
                                 '1 to 24 characters naming this question\'s subject in one or two words')
    return questions


def create_tx(db, task_id, channel_id, request_key, questions, *, assignment_ref=None, owner_epoch=None):
    members = []
    for index, form in enumerate(questions):
        question_id = new_id('pq_')
        db.execute("INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,options_json,status,created_at) "
                   "VALUES(?,?,?,?,?,?,'open',?)",
                   (question_id, task_id, assignment_ref, f'runtime:{channel_id}:{request_key}:{index}',
                    form['question'], encoded(form['options']), now()))
        members.append(dict(question_id=question_id, **form))
    result = dict(question_id=members[0]['question_id'], question=members[0]['question'],
                  options=members[0]['options'])
    if len(members) > 1:
        revision = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()[0]
        result.update(batch_id=new_id('qb_'), questions=members, question_ids=[q['question_id'] for q in members])
        event(db, task_id, 'QUESTION_BATCH_CREATED', dict(
            **result, task_id=task_id, channel_id=channel_id, assignment_ref=assignment_ref,
            revision=revision, owner_epoch=owner_epoch), channel_id)
    return result


def find_tx(db, task_id, *, batch_id=None, question_id=None):
    if batch_id is None and question_id is None:
        return None
    clause = ("json_extract(payload_json,'$.batch_id')=?" if batch_id is not None else
              "EXISTS (SELECT 1 FROM json_each(payload_json,'$.questions') q "
              "WHERE json_extract(q.value,'$.question_id')=?)")
    row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='QUESTION_BATCH_CREATED' AND "
                     + clause + ' ORDER BY id DESC LIMIT 1', (task_id, batch_id or question_id)).fetchone()
    return json.loads(row[0]) if row else None


def completed_tx(db, batch):
    return all(db.execute("SELECT 1 FROM pending_questions WHERE task_id=? AND id=? AND status='answered' "
                          "AND answer_source IN ('text','elicitation') AND length(trim(answer_text))>0",
                          (batch['task_id'], q['question_id'])).fetchone() for q in batch['questions'])


def answer(store, task_id, batch_id, answers, request_key, *, session_id, owner_epoch):
    """All-or-none exact submission; identical acknowledged deliveries replay."""
    from .worker_waits import answer_tx
    from ..ownership import current_owner_epoch
    if not isinstance(request_key, str) or not request_key.strip() or len(request_key) > 128:
        raise StoreError('question_answer_key_invalid')
    if (not isinstance(answers, list) or not 2 <= len(answers) <= 8
            or any(not isinstance(a, dict) or set(a) != {'questionId', 'answer'}
                   or not isinstance(a['questionId'], str) or not isinstance(a['answer'], str)
                   or not a['answer'].strip() or len(a['answer']) > 32000 for a in answers)):
        raise StoreError('question_batch_answers_invalid')
    digest = hashlib.sha256(encoded(dict(batch_id=batch_id, answers=answers)).encode()).hexdigest()
    actor = 'question-session:' + session_id
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        session = db.execute('SELECT task_id,closed_at FROM acp_host_sessions WHERE session_id=?', (session_id,)).fetchone()
        if not session or session['task_id'] != task_id or session['closed_at']:
            raise StoreError('question_answer_session_mismatch')
        if current_owner_epoch(db, task_id) != owner_epoch:
            raise StoreError('question_batch_stale')
        previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                              (task_id, actor, request_key)).fetchone()
        if previous:
            if previous['operation'] != 'question_batch_answer' or previous['digest'] != digest:
                raise StoreError('request_key_conflict')
            return dict(json.loads(previous['result_json']), replayed=True)
        batch = find_tx(db, task_id, batch_id=batch_id)
        if not batch:
            raise StoreError('question_batch_unknown')
        expected = {q['question_id'] for q in batch['questions']}
        if len({a['questionId'] for a in answers}) != len(answers) or {a['questionId'] for a in answers} != expected:
            raise StoreError('question_batch_members_mismatch')
        task = db.execute('SELECT state,revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        channel = db.execute('SELECT revoked_at,expires_at FROM trusted_channels WHERE channel_id=?',
                             (batch['channel_id'],)).fetchone()
        if (not task or task['revision'] != batch['revision'] or task['state'] not in {'WAITING_USER','EXECUTING','REVIEWING'}
                or db.execute('SELECT 1 FROM task_stopped WHERE task_id=?', (task_id,)).fetchone()):
            raise StoreError('question_batch_stale')
        if batch['assignment_ref']:
            if (batch['owner_epoch'] != owner_epoch or not channel or channel['revoked_at']
                    or (channel['expires_at'] and channel['expires_at'] <= now())):
                raise StoreError('question_batch_stale')
        else:
            # A recovered coordinator has a new channel/owner epoch. The
            # session's current owner may answer only the exact still-current
            # persisted WAITING_USER batch, not revive a superseded one.
            latest = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED' "
                                "ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
            if task['state'] != 'WAITING_USER' or not latest or json.loads(latest[0]).get('batch_id') != batch_id:
                raise StoreError('question_batch_stale')
        if completed_tx(db, batch):
            raise StoreError('question_batch_already_answered')
        for item in answers:
            row = db.execute('SELECT status,answer_text FROM pending_questions WHERE task_id=? AND id=?',
                             (task_id, item['questionId'])).fetchone()
            if not row or row['status'] not in {'open','answered'}:
                raise StoreError('question_batch_stale')
            if row['status'] == 'answered':
                if row['answer_text'] != item['answer']:
                    raise StoreError('question_answer_conflict')
            elif not answer_tx(db, item['questionId'], item['answer'], 'elicitation', session_id=session_id):
                raise StoreError('question_batch_stale')
        receipt = dict(batch_id=batch_id, question_ids=[q['question_id'] for q in batch['questions']], complete=True)
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (task_id, request_key, actor, 'question_batch_answer', digest, encoded(receipt)))
        event(db, task_id, 'QUESTION_BATCH_ANSWERED', dict(**receipt, session_id=session_id))
        db.commit()
    return dict(receipt, replayed=False)


def answer_single(store, task_id, question_id, text, request_key, *, session_id, owner_epoch):
    """Single-card compatibility with a durable exact answer receipt."""
    from .worker_waits import answer_tx
    from ..ownership import current_owner_epoch
    if (not isinstance(text, str) or not text.strip() or len(text) > 32000
            or not isinstance(request_key, str) or not request_key.strip() or len(request_key) > 128):
        raise StoreError('question_answer_invalid')
    digest = hashlib.sha256(encoded(dict(question_id=question_id, answer=text)).encode()).hexdigest()
    actor = 'question-session:' + session_id
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        session = db.execute('SELECT task_id,closed_at FROM acp_host_sessions WHERE session_id=?', (session_id,)).fetchone()
        if not session or session['task_id'] != task_id or session['closed_at']:
            raise StoreError('question_answer_session_mismatch')
        if current_owner_epoch(db, task_id) != owner_epoch:
            raise StoreError('question_answer_stale')
        previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                              (task_id, actor, request_key)).fetchone()
        if previous:
            if previous['operation'] != 'question_card_answer' or previous['digest'] != digest:
                raise StoreError('request_key_conflict')
            return dict(json.loads(previous['result_json']), replayed=True)
        row = db.execute('SELECT task_id FROM pending_questions WHERE id=?', (question_id,)).fetchone()
        if not row or row['task_id'] != task_id or find_tx(db, task_id, question_id=question_id):
            raise StoreError('question_answer_mismatch')
        if db.execute('SELECT 1 FROM task_stopped WHERE task_id=?', (task_id,)).fetchone():
            raise StoreError('question_answer_stale')
        if not answer_tx(db, question_id, text, 'elicitation', session_id=session_id):
            raise StoreError('question_answer_stale')
        receipt = dict(question_id=question_id, complete=True)
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (task_id, request_key, actor, 'question_card_answer', digest, encoded(receipt)))
        db.commit()
    return dict(receipt, replayed=False)
