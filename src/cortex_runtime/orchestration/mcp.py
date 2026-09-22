"""Channel-scoped proposal dispatch; never executes a model-proposed action."""
from .contracts import COORDINATOR, WORKER, validate
from .state import TaskMachine, event, transition, encoded
import hashlib
import json
from ..contracts import StoreError


def schemas(role):
    return COORDINATOR if role == 'coordinator' else WORKER


def dispatch(store, channel, operation, args):
    schema = schemas(channel.role).get(operation)
    if schema is None:
        raise StoreError('unknown_operation')
    validate(schema, args)
    machine = TaskMachine(store)
    from .gateway import _authority
    if operation == 'task_get_context':
        return machine.context(channel.task_id)
    if operation == 'answer_submit':
        return machine.submit_answer(channel, args['text'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'plan_submit':
        return machine.submit_plan(channel, args['plan'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'synthesis_submit':
        return machine.submit_synthesis(channel, args['text'], authorization=lambda db: _authority(db,channel))
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
                from .gateway import verified_check
                _authority(db, channel)
                for index, check in enumerate(args['checks']):
                    if check['outcome'] == 'passed' and not verified_check(
                        db, channel.task_id, channel.attempt_id, check.get('tool_call_ref')
                    ):
                        raise StoreError('check_evidence_required',
                            f'arguments.checks[{index}].tool_call_ref', 'missing or unverified',
                            'a command this attempt ran: a zero-exit command_run receipt or a completed shell command of your own',
                            'Cite the exact command_run tool_call_ref, or omit tool_call_ref for a check you ran with your own shell. '
                            'Unexecuted or unresolved checks must be not_run or unverified; they are retained as disclosed gaps. '
                            'Descriptive review observations belong in the report, not automated checks.')
                db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                           (channel.task_id, args['request_key'], channel.channel_id, operation, digest,
                            encoded(dict(complete=False, assignment_ref=provision['idempotency_key'], result=args))))
            db.commit()
        result = _worker_publish_result(store, channel, {key: value for key, value in args.items() if key != 'verdict'})
        machine.record_result(channel.task_id, provision['idempotency_key'], args)
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                       (encoded(dict(complete=True, receipt=result, assignment_ref=provision['idempotency_key'], result=args)),
                        channel.task_id, channel.channel_id, args['request_key']))
            db.commit()
        return result
    raise StoreError('unknown_operation')
