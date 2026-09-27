"""Channel-scoped proposal dispatch; never executes a model-proposed action."""
from .contracts import COORDINATOR, WORKER, validate
from .state import TaskMachine, event, transition, encoded
import hashlib
import json
import threading
from ..contracts import StoreError


def schemas(role):
    return COORDINATOR if role == 'coordinator' else WORKER


_NOTHING_NEEDED_HINT = (
    'Nothing here needs you: workers are dispatched and the runtime decides next steps. '
    'End this turn now instead of polling task_get_context again -- the runtime resumes '
    'you with a fresh message when a result, question or decision needs you.'
)
# Per-channel digest of the last task_get_context read, so an unchanged repeat
# poll (the same coordinator turn calling this over and over while nothing
# progressed, see r_310cd92bc78e's 57-call regression) costs a small receipt
# instead of resending the whole plan/nodes payload. Bounded like the wire
# emitter's own repeat memory; a lost entry after eviction or restart only
# costs one full resend, never a wrong answer.
_CONTEXT_READ_MEMORY = 256
_last_context_digest: dict[str, str] = {}
_last_context_lock = threading.Lock()


def _context_needs_nothing(value):
    if value['state'] not in {'PLAN_READY', 'EXECUTING', 'REVIEWING'}:
        return False
    if value.get('ask_user_required'):
        return False
    return not any(n['status'] == 'failed' for n in value['nodes'])


def _task_get_context(store, channel):
    machine = TaskMachine(store)
    value = machine.context(channel.task_id)
    needs_nothing = _context_needs_nothing(value)
    if needs_nothing:
        value = dict(value, hint=_NOTHING_NEEDED_HINT)
    digest = hashlib.sha256(encoded(value).encode()).hexdigest()
    with _last_context_lock:
        unchanged = _last_context_digest.get(channel.channel_id) == digest
        _last_context_digest[channel.channel_id] = digest
        while len(_last_context_digest) > _CONTEXT_READ_MEMORY:
            _last_context_digest.pop(next(iter(_last_context_digest)))
    if not unchanged:
        return value
    compact = dict(unchanged_since_last_read=True, state=value['state'],
                   revision=value['revision'], review_cycle=value['review_cycle'])
    if needs_nothing:
        compact['hint'] = _NOTHING_NEEDED_HINT
    return compact


def dispatch(store, channel, operation, args):
    schema = schemas(channel.role).get(operation)
    if schema is None:
        raise StoreError('unknown_operation')
    # A persisted pre-model plan can be delivered again after recovery.
    # Accept only its exact recorded request; fresh plans use the new schema.
    if operation == 'plan_submit' and isinstance(args, dict) and isinstance(args.get('plan'), dict) and isinstance(args.get('request_key'), str):
        candidate = args['plan']
        nodes = candidate.get('tasks')
        if isinstance(nodes, list) and any(isinstance(n, dict) and 'model' not in n for n in nodes):
            digest = hashlib.sha256(encoded(candidate).encode()).hexdigest()
            with store.connection() as db:
                prior = db.execute('SELECT operation,digest FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                   (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
            if prior is not None and prior['operation'] == 'plan_submit' and prior['digest'] == digest:
                from .gateway import _authority
                return TaskMachine(store).submit_plan(
                    channel, candidate, args['request_key'], authorization=lambda db: _authority(db, channel))
    validate(schema, args)
    machine = TaskMachine(store)
    from .gateway import _authority
    if operation == 'task_get_context':
        return _task_get_context(store, channel)
    if operation == 'answer_submit':
        return machine.submit_answer(channel, args['text'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'plan_submit':
        return machine.submit_plan(channel, args['plan'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'verification_submit':
        return machine.submit_verification(channel, args['verification'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'synthesis_submit':
        return machine.submit_synthesis(channel, args['text'], args.get('not_verified') or [], authorization=lambda db: _authority(db,channel))
    if operation == 'user_question_request':
        from .questions import request
        return request(store, channel, args)
    if operation in {'replan_request','scope_request','escalation_request'}:
        from .policy import propose_replan
        return propose_replan(store,channel,operation,args)
    if operation == 'result_submit':
        from ..delegation.mcp_tools import _worker_publish_result
        with store.connection() as db:
            provision = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                                   (channel.task_id, channel.attempt_id)).fetchone()
            node = db.execute('SELECT stage FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                              (channel.task_id, provision['idempotency_key'] if provision else None)).fetchone()
        if not node:
            raise StoreError('runtime_assignment_missing')
        if (node['stage'] == 'review') != ('verdict' in args):
            raise StoreError('review_verdict_required' if node['stage'] == 'review' else 'reviewer_only_verdict')
        digest = hashlib.sha256(encoded(args).encode()).hexdigest()
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                  (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
            if previous and (previous['operation'] != operation or previous['digest'] != digest):
                raise StoreError('request_key_conflict')
            if previous and json.loads(previous['result_json']).get('complete'):
                return json.loads(previous['result_json'])['receipt']
            if previous is None:
                from ..delegation.evidence import accept_checks
                _authority(db, channel)
                for index, check in enumerate(args['checks']):
                    gap = check['outcome'] in {'not_run', 'unverified'}
                    if gap and not (check.get('blocker') or '').strip():
                        raise StoreError('check_blocker_required', f'arguments.checks[{index}].blocker', 'missing',
                                         'the concrete blocker that made this check impossible',
                                         'Run the check and report its real outcome, or give the concrete blocker.')
                    if not gap and 'blocker' in check:
                        raise StoreError('check_blocker_unexpected', f'arguments.checks[{index}].blocker', check['outcome'],
                                         'no blocker on a passed or failed check', 'Remove blocker from checks that passed or failed.')
                # A pass the runtime cannot verify is kept unverified (or
                # failed, when its cited command exited nonzero) with the
                # structured gap; a failure an exact later rerun closed keeps
                # closed_by. The original claim stays in the digest.
                accepted = dict(args, checks=accept_checks(db, channel.task_id, channel.attempt_id, args['checks']))
                db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                           (channel.task_id, args['request_key'], channel.channel_id, operation, digest,
                            encoded(dict(complete=False, assignment_ref=provision['idempotency_key'], result=args,
                                         accepted=accepted))))
            else:
                saved = json.loads(previous['result_json'])
                accepted = saved.get('accepted', saved['result'])
            db.commit()
        result = _worker_publish_result(store, channel, {key: value for key, value in accepted.items() if key != 'verdict'})
        machine.record_result(channel.task_id, provision['idempotency_key'], accepted)
        # The receipt shows the worker every check the runtime did not accept as claimed.
        result = dict(result, checks=[{key: check[key] for key in ('name', 'outcome', 'claimed_outcome', 'evidence_gap', 'blocker', 'closed_by')
                                       if key in check} for check in accepted['checks']])
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                       (encoded(dict(complete=True, receipt=result, assignment_ref=provision['idempotency_key'], result=args,
                                     accepted=accepted)),
                        channel.task_id, channel.channel_id, args['request_key']))
            db.commit()
        return result
    raise StoreError('unknown_operation')
