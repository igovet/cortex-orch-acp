"""Channel-scoped proposal dispatch; never executes a model-proposed action."""
from .contracts import COORDINATOR, WORKER, drop_null_optional_fields, validate
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
# progressed; one live coordinator polled it 57 times) costs a small receipt
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
    from .user_text import bound_context
    machine = TaskMachine(store)
    value = bound_context(store, channel.task_id, machine.context(channel.task_id))
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


def _task_language(store, task_id):
    """The task's turn language, resolved as the coordinator turn resolves it."""
    from ..acp import messages, steering
    return messages.select_language(steering.get_language_override(store, task_id),
                                    steering.recent_user_direct_texts(store, task_id))


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
    if operation == 'plan_submit':
        # null for an optional task field means omitted (the advertised
        # descriptions say so); required fields keep null and are refused.
        args = drop_null_optional_fields(schema, args)
    if operation == 'result_submit' and isinstance(args, dict):
        # Ignore only write_report fields that are not part of result_submit;
        # preserve all result fields and keep strict validation for everything else.
        write_report_only = {'artifacts', 'author', 'draft_id', 'title', 'redact_values', 'source_revision'}
        ignored = write_report_only.difference(schema.get('properties', {}))
        if ignored.intersection(args):
            args = {key: value for key, value in args.items() if key not in ignored}
        if args.get('verdict') in {'approve', 'request_changes', 'block'}:
            args = dict(args, verdict=args['verdict'].upper())
    validate(schema, args)
    machine = TaskMachine(store)
    from .gateway import _authority
    if operation == 'task_get_context':
        return _task_get_context(store, channel)
    if operation == 'answer_submit':
        return machine.submit_answer(channel, args['text'], args['request_key'],
                                     not_verified=args.get('not_verified') or [], authorization=lambda db: _authority(db,channel),
                                     language=_task_language(store, channel.task_id))
    if operation == 'plan_submit':
        return machine.submit_plan(channel, args['plan'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'verification_submit':
        return machine.submit_verification(channel, args['verification'], args['request_key'], authorization=lambda db: _authority(db,channel))
    if operation == 'synthesis_submit':
        return machine.submit_synthesis(channel, args['text'], args.get('not_verified') or [], authorization=lambda db: _authority(db,channel),
                                        language=_task_language(store, channel.task_id))
    if operation == 'user_question_request':
        from .questions import request
        return request(store, channel, args)
    if operation == 'access_request':
        from .access import request
        return request(store, channel, args)
    if operation in {'replan_request','scope_request','escalation_request'}:
        from .policy import propose_replan
        return propose_replan(store,channel,operation,args)
    if operation == 'result_submit':
        from ..delegation.mcp_tools import (_worker_publish_result, inline_report_form, publish_inline_report,
                                            report_argument)
        # The advertised report is one required plain string (validated
        # above): an exact report_id is retained as the top-level report_ref
        # it always was, inline Markdown as report={title, markdown}, so
        # request digests, delivery recovery and worker_result events keep
        # one internal shape.
        args = report_argument(args)
        with store.connection() as db:
            provision = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                                   (channel.task_id, channel.attempt_id)).fetchone()
            node = db.execute('SELECT stage, role FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                              (channel.task_id, provision['idempotency_key'] if provision else None)).fetchone()
            role_id = db.execute('SELECT role_id FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                                 (channel.task_id, channel.attempt_id)).fetchone()
            recorded = db.execute('SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                  (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
        if not node:
            raise StoreError('runtime_assignment_missing')
        if (node['stage'] == 'review') != ('verdict' in args):
            raise StoreError('review_verdict_required' if node['stage'] == 'review' else 'reviewer_only_verdict',
                             'arguments.verdict', 'missing' if node['stage'] == 'review' else 'present',
                             'one review verdict (APPROVE, REQUEST_CHANGES or BLOCK)' if node['stage'] == 'review'
                             else 'verdict omitted outside the review stage',
                             'Send verdict=APPROVE, REQUEST_CHANGES or BLOCK on review; omit it otherwise.')
        if 'findings' in args and node['stage'] != 'review':
            raise StoreError('reviewer_only_findings', 'arguments.findings', 'present',
                             'findings omitted outside the review stage',
                             'Only a review-stage worker may send findings; omit them here.')
        if recorded is None:
            # Every failed command receipt this submission presents is retained
            # first, in its own transaction, so no later refusal (skill gate,
            # open failure) lets a resubmission drop it.
            from ..delegation.evidence import record_presented_failures
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                _authority(db, channel)
                record_presented_failures(db, channel.task_id, channel.attempt_id, channel.channel_id, args['checks'])
                db.commit()
            # Observed changes that match an unread situational skill refuse
            # the result before anything is recorded, so a retry after
            # read_skill (even with this request_key) goes through. A request
            # already recorded is a replay and is never re-gated.
            from .skill_gate import enforce as enforce_situational_skills
            enforce_situational_skills(store, channel, stage=node['stage'],
                                       role_ids=(node['role'], role_id['role_id'] if role_id else None))
            from .ignore_guard import reconcile_submission as enforce_gitignore
            enforce_gitignore(store, channel)
        # An inline report's body lives in its published file; the request
        # retains and digests its SHA-256 (a changed body still conflicts).
        request = inline_report_form(args)
        digest = hashlib.sha256(encoded(request).encode()).hexdigest()
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                  (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
            if previous and (previous['operation'] != operation or previous['digest'] != digest):
                raise StoreError('request_key_conflict', 'arguments.request_key',
                                 'key already bound to a different operation or payload on this channel',
                                 'a fresh key for a new result or the identical original payload for a retry',
                                 'Retry with the exact original arguments, or use a new key for a new result and publish a new report when its content changed.')
            if previous and json.loads(previous['result_json']).get('complete'):
                closed = json.loads(previous['result_json'])
                if closed.get('error'):
                    # A result refused for good after its publication: an
                    # identical retry gets the same refusal, never a receipt.
                    raise StoreError(closed['error'], **(closed.get('error_detail') or {}))
                return closed['receipt']
            if previous is None:
                from ..delegation.evidence import (accept_checks, current_outcomes, assess_checks,
                                                   explain_open_failure, omitted_failures)
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
                omitted = omitted_failures(db, channel.task_id, channel.attempt_id, args['checks'])
                if omitted:
                    listed = ', '.join(omitted[:8]) + (f' and {len(omitted) - 8} more' if len(omitted) > 8 else '')
                    raise StoreError('omitted_failed_check', 'arguments.checks', f'{len(omitted)} omitted: {listed}',
                                     'every failed command receipt this attempt already presented, cited again '
                                     'by a passed or failed check',
                                     f"An earlier result_submit of this attempt presented the failed receipt(s) {listed}; "
                                     "a presented failure stays part of this attempt and is never dropped. Cite each "
                                     "again as a failed check with the same tool_call_ref, then close it with a passed "
                                     "check citing a verified later rerun of exactly the same argv, retract a supported "
                                     "own Python probe error with invocation_error, or send assignment_outcome=\"failed\". "
                                     "Relabelling it not_run or unverified, or a different argv, does not count. No "
                                     "result was recorded or published; your channel remains active and the same "
                                     "request_key may be reused.")
                # A pass the runtime cannot verify is kept unverified (or
                # failed, when its cited command exited nonzero) with the
                # structured gap; a failure an exact later rerun closed keeps
                # closed_by. The original claim stays in the digest.
                accepted_checks = accept_checks(db, channel.task_id, channel.attempt_id, args['checks'])
                if args.get('assignment_outcome') != 'failed':
                    outcomes=current_outcomes(db,channel.task_id,channel.attempt_id,accepted_checks)
                    assessments=assess_checks(db,channel.task_id,channel.attempt_id,accepted_checks)
                    for index,(check,current,assessment) in enumerate(zip(accepted_checks,outcomes,assessments)):
                        if current['outcome'] != 'failed':
                            continue
                        diagnosis, correction = explain_open_failure(db, channel.task_id, channel.attempt_id,
                                                                     accepted_checks, assessments, index)
                        others = sum(1 for later in outcomes[index + 1:] if later['outcome'] == 'failed')
                        raise StoreError('open_failed_check',f'arguments.checks[{index}]',
                                         f"{check['name'][:80]} ({check.get('tool_call_ref') or 'no receipt'})",
                                         'a later passed check citing a verified rerun of exactly this argv, a '
                                         'supported invocation_error retraction, or assignment_outcome="failed"',
                                         f"This check remains failed: {diagnosis}. {correction}"
                                         + (f" {others} later check(s) of this result also remain failed." if others else '')
                                         + " A supported standalone Python probe syntax/regex invocation error may "
                                         "instead be retracted with invocation_error (corrected_by and reason). No "
                                         "result was recorded or published; a cited failed receipt stays part of this "
                                         "attempt, your channel remains active and the same request_key may be reused.")
                # A supplied argv on a completed check is descriptive, never
                # evidence. Do not echo it in the retained result as if trusted.
                for check in accepted_checks:
                    if check['outcome'] in {'passed', 'failed'}:
                        check.pop('command', None)
                accepted = dict(request, checks=accepted_checks)
                db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                           (channel.task_id, args['request_key'], channel.channel_id, operation, digest,
                            encoded(dict(complete=False, assignment_ref=provision['idempotency_key'], result=request,
                                         accepted=accepted))))
            else:
                saved = json.loads(previous['result_json'])
                accepted = saved.get('accepted', saved['result'])
            db.commit()
        if 'report' in args and 'report_ref' not in accepted:
            # Publish before any result is visible: a crash here leaves at
            # most an unpublished draft or a report without a result, and
            # this same request's retry replays both deliveries.
            report_ref = publish_inline_report(store, channel, args['request_key'], args['report'], args['summary'])
            accepted = dict({key: value for key, value in accepted.items() if key != 'report'}, report_ref=report_ref)
            with store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                saved = json.loads(db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                              (channel.task_id, channel.channel_id, args['request_key'])).fetchone()[0])
                if saved.get('complete'):
                    if saved.get('error'):
                        raise StoreError(saved['error'], **(saved.get('error_detail') or {}))
                    return saved['receipt']
                db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                           (encoded(dict(saved, accepted=accepted)),
                            channel.task_id, channel.channel_id, args['request_key']))
                db.commit()
        if node['stage'] == 'review':
            # A review of a project that changed since it started is re-bound
            # and refused before its result is published (never accepted).
            machine.check_review_binding(channel.task_id, provision['idempotency_key'], args['request_key'])
        result = _worker_publish_result(store, channel, {key: value for key, value in accepted.items() if key not in {'verdict', 'findings'}})
        try:
            machine.record_result(channel.task_id, provision['idempotency_key'], accepted, args['request_key'])
        except StoreError as error:
            # Published, then refused for good (the node was cancelled or
            # replanned meanwhile, or the review re-bound): close the request
            # with its refusal now, or reconciliation would retry it on
            # every later turn. A non-terminal failure stays open for it.
            from .recovery import refuse_result_request
            refuse_result_request(store, channel.task_id, channel.channel_id, args['request_key'], error,
                                  result.get('cursor'))
            raise
        with store.connection() as db:
            node_status=db.execute('SELECT status FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                                   (channel.task_id,provision['idempotency_key'])).fetchone()[0]
        # The receipt shows the worker every check the runtime did not accept as claimed.
        result = dict(result, node_status=node_status, report_ref=accepted['report_ref'], checks=[{key: check[key] for key in ('name', 'outcome', 'claimed_outcome', 'evidence_gap', 'blocker', 'closed_by', 'invocation_error', 'retracted_by')
                                       if key in check} for check in accepted['checks']])
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                       (encoded(dict(complete=True, receipt=result, assignment_ref=provision['idempotency_key'], result=request,
                                     accepted=accepted)),
                        channel.task_id, channel.channel_id, args['request_key']))
            db.commit()
        return result
    raise StoreError('unknown_operation')
