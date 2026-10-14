"""Atomic question identities and deterministic resume after a real answer."""
import hashlib
import json

from ..contracts import StoreError
from .state import encoded, event, transition


# Only an unambiguous, complete owner answer closes unfinished work. In
# particular, requests to *do* a final check or release are not closure.
FORCE_FINISH_ANSWERS = frozenset({
    '/cortex-finish',
    'закрыть как есть', 'завершить задачу', 'заверши задачу',
    'закрыть задачу', 'закрой задачу',
    'finish the task', 'close the task', 'close as is', 'finish now',
})


# The model sees this with the receipt. A bare receipt was followed by a full
# rewrite of the message written before the call (observed live); the
# coordinator's later text in that turn is also withheld by the controller.
QUESTION_RECEIPT_NEXT = (
    'The question is shown to the user together with the message you wrote before this call. '
    'End this turn now without writing any further text: do not restate, rewrite or summarize anything. '
    'The runtime continues the task after the user answers.'
)


# The runtime's own stop question (state._ask_about_stop) as stored: the
# chat shows it rendered in the task language with the same two options.
# It is recognized by its structured runtime marker (its request names the
# blocker; its pending row's dedup key starts with STOP_QUESTION_DEDUP_PREFIX),
# never by its text alone: a coordinator may word a question the same way.
STOP_QUESTION_PREFIX = 'The task stopped: '
STOP_QUESTION_SUFFIX = '. What should happen next?'
STOP_QUESTION_DEDUP_PREFIX = 'runtime:stop:'
# Finish wording earlier builds stored or rendered (en, ru) for stop
# questions that may still be open.
_LEGACY_STOP_OPTIONS = ((), ('Finish: accept the current results and close the task',
                             'Завершить: принять текущие результаты и закрыть задачу'))


def _normalized(value):
    return value.strip().casefold().rstrip('.!')


def is_stop_question(question, runtime_stop=False):
    """Whether ``question`` is the runtime's own stop question: it carries
    the runtime's structured marker (``runtime_stop``) and its text."""
    return (runtime_stop is True and isinstance(question, str) and question.startswith(STOP_QUESTION_PREFIX)
            and question.endswith(STOP_QUESTION_SUFFIX))


def runtime_stop_request(payload):
    """Whether a ``USER_QUESTION_REQUESTED`` payload is the runtime's own
    stop question: only the runtime records its stop blocker there."""
    return isinstance(payload, dict) and bool(payload.get('runtime_stop') or payload.get('blocker'))


def runtime_stop_dedup_key(dedup_key):
    """Whether a pending question's dedup key is the runtime stop question's.
    A coordinator or worker question's key names its channel instead."""
    return isinstance(dedup_key, str) and dedup_key.startswith(STOP_QUESTION_DEDUP_PREFIX)


def _stop_option_answers(index):
    from ..acp import messages
    from .state import STOP_OPTIONS
    key = ('stop_option_continue', 'stop_option_finish')[index]
    texts = {STOP_OPTIONS[index], *_LEGACY_STOP_OPTIONS[index]}
    texts.update(messages.render(key, language) for language in messages.SUPPORTED_LANGUAGES)
    # The full option, its label before the colon, or its 1-based number.
    return {_normalized(text) for text in texts} | {_normalized(text.split(':', 1)[0]) for text in texts} | {
        str(index + 1)}


def stop_choice(value, question, *, runtime_stop=False):
    """``'finish'`` or ``'continue'`` when ``value`` picks one of the runtime
    stop question's own options -- by its stored or rendered (en/ru) text,
    its label, or its number -- else ``None`` (an answer in the user's own
    words, or any other question). ``runtime_stop`` is the question's
    structured runtime marker (:func:`runtime_stop_request`,
    :func:`runtime_stop_dedup_key`)."""
    if not isinstance(value, str) or not is_stop_question(question, runtime_stop):
        return None
    answer = _normalized(value)
    for index, choice in enumerate(('continue', 'finish')):
        if answer in _stop_option_answers(index):
            return choice
    return None


def is_force_finish_answer(value, question=None, *, runtime_stop=False):
    if not isinstance(value, str):
        return False
    answer = _normalized(value)
    if answer in FORCE_FINISH_ANSWERS:
        return True
    if stop_choice(value, question, runtime_stop=runtime_stop) == 'finish':
        # The stop question's own Finish closes the task CANCELLED with
        # USER_FORCE_FINISHED; it never certifies the stopped work.
        return True
    prompt = question.casefold() if isinstance(question, str) else ''
    return answer in {'завершить', 'закрыть', 'finish', 'close'} and (
        ('задач' in prompt and any(word in prompt for word in ('заверш', 'закры'))) or
        ('task' in prompt and any(word in prompt for word in ('finish', 'close')))
    )


def request(store, channel, args):
    if args.get('skill_proposal') is not None and channel.role != 'coordinator':
        raise StoreError('skill_proposal_not_allowed', 'skill_proposal', channel.role, 'the coordinator')
    if channel.role == 'worker':
        from .worker_waits import request as worker_request
        return worker_request(store, channel, args)
    from .question_batches import forms, create_tx
    question_forms = forms(args)
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
        details={}
        if resume_state=='RECONCILING':
            # A decision left after verified work (approve a proposed plan,
            # confirm manual steps) either starts new work or ends the task:
            # both are planning decisions, so the answer returns there.
            revision=db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?',(channel.task_id,)).fetchone()[0]
            resume_state='PLANNING'
            details=dict(after_verification=True,revision=revision)
        if args.get('skill_proposal') is not None:
            # A proposal is checked before the question exists and retained
            # with it; nothing here touches the project.
            from .skill_proposals import validate
            details['skill_proposal'] = validate(db, store.project_root, channel, args)
        saved = create_tx(db, channel.task_id, channel.channel_id, args['request_key'], question_forms,
                          owner_epoch=getattr(channel, 'owner_epoch', None))
        event(db, channel.task_id, 'USER_QUESTION_REQUESTED',
              dict(**saved, resume_state=resume_state, **details), channel.channel_id)
        transition(db, channel.task_id, 'WAITING_USER')
        receipt = dict(question_id=saved['question_id'], state='WAITING_USER')
        if 'batch_id' in saved:
            receipt.update(batch_id=saved['batch_id'], question_ids=[q['question_id'] for q in saved['questions']])
        if channel.role == 'coordinator':
            # Only the coordinator's text before the call reaches the user.
            receipt['next'] = QUESTION_RECEIPT_NEXT
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
        answer = db.execute('SELECT status,answer_text,answer_source FROM pending_questions WHERE task_id=? AND id=?', (task_id, saved['question_id'])).fetchone()
        if saved.get('batch_id'):
            from .question_batches import find_tx, completed_tx
            batch = find_tx(db, task_id, batch_id=saved['batch_id'])
            if not batch or not completed_tx(db, batch):
                return False
        if not answer or answer['status'] != 'answered':
            return False
        if not saved.get('batch_id') and answer['answer_source'] in {'text', 'elicitation'} and is_force_finish_answer(
                answer['answer_text'], saved['question'], runtime_stop=runtime_stop_request(saved)):
            # Leave WAITING_USER in place until stop_task atomically records
            # the stop marker and fences workers. A crash here is retryable.
            received = db.execute(
                "SELECT 1 FROM runtime_events WHERE task_id=? AND type='USER_ANSWER_RECEIVED' "
                "AND json_extract(payload_json,'$.question_id')=? LIMIT 1",
                (task_id, saved['question_id']),
            ).fetchone()
            if received is None:
                event(db, task_id, 'USER_ANSWER_RECEIVED', dict(
                    question_id=saved['question_id'], answer=answer['answer_text'],
                ))
            db.commit()
            return ('force_finish', saved['question_id'])
        event(db, task_id, 'USER_ANSWER_RECEIVED', dict(question_id=saved['question_id'], answer=answer['answer_text']))
        from .skill_proposals import record_decision
        record_decision(db, task_id, saved, answer['answer_text'])
        transition(db, task_id, saved['resume_state'])
        db.commit()
    return True
