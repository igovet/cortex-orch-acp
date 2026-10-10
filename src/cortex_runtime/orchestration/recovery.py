"""Reconcile exact retained publication evidence after interrupted result delivery."""
import hashlib
import json

from ..contracts import StoreError
from .state import TaskMachine, encoded, event

MAX_ATTEMPT_RETRIES = 2
# Retries after a failure routing.availability recorded against the route
# (exhausted account, model unavailable, engine would not start) go to
# another route and do not use up MAX_ATTEMPT_RETRIES; this bounds them.
MAX_ROUTE_RETRIES = 6
# Refusals of routing itself: retrying the same request cannot change them.
ROUTING_REFUSALS = frozenset({'no_available_route', 'pinned_route_unavailable', 'invalid_routing_overrides',
                              'route_denied'})


def release_observed(runtime, task_id, attempt_id):
    """Reuse a reservation only after its engine's absence was observed."""
    if runtime._engine.lookup_owned(attempt_id) != 'absent':
        return False
    with runtime._store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        row=db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                       (task_id,attempt_id)).fetchone()
        if row and not db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='WORKER_RELEASED' AND json_extract(payload_json,'$.assignment_ref')=?",
                                  (task_id,row[0])).fetchone():
            event(db,task_id,'WORKER_RELEASED',dict(assignment_ref=row[0],attempt_id=attempt_id,observed_session='absent'))
        db.commit()
    return True


def retry_lost(runtime,task_id,node):
    """Retry only after absence is observed and ambiguous actions are ruled out.

    No fabricated result, successful check or session receipt closes a node.
    Old channels are revoked before a new assignment can be reserved.
    """
    from .state import now
    store=runtime._store
    assignment=node['assignment_ref']
    with store.connection() as db:
        provision=db.execute('SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
                             (task_id,assignment)).fetchone()
    if provision:
        status=runtime._engine.lookup_owned(provision['attempt_id'])
        if status!='absent':
            runtime._engine.terminate_owned(provision['attempt_id'])
            if runtime._engine.lookup_owned(provision['attempt_id'])!='absent':
                raise StoreError('lost_worker_termination_unconfirmed')
        release_observed(runtime,task_id,provision['attempt_id'])
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        failure=db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed' AND superseded=0 ORDER BY id DESC LIMIT 1",(task_id,assignment)).fetchone()
        payload=json.loads(failure[0]) if failure else {}
        reason=payload.get('error','')
        # A refused fallback after a capacity wait is transient: the same
        # route serves again once a slot frees, so it is retried (bounded by
        # MAX_ATTEMPT_RETRIES) instead of failing the worker for good.
        transient = payload.get('failure_class')=='capacity_unavailable'
        if reason in {'refusal','authorization_loss','provider_restriction','cancellation'} or (
                reason.startswith('fallback_refused:') and not transient):
            raise StoreError('worker_failure_not_retryable:'+reason)
        if reason in ROUTING_REFUSALS:
            # Named with the routes and why, so the coordinator and the user
            # see what to change instead of "the worker could not finish".
            detail=str(payload.get('detail') or '')[:600]
            raise StoreError('worker_failure_not_retryable:'+reason+(': '+detail if detail else ''))
        from ..routing import availability
        unavailable=availability.for_attempt(db,task_id,provision['attempt_id']) if provision else None
        attempts=db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'",(task_id,)).fetchall()
        retried=[p for p in map(lambda row: json.loads(row[0]),attempts)
                 if p.get('node')==node['node_id'] and p.get('revision')==node['revision']]
        count=sum(not p.get('route_failure') for p in retried)
        if unavailable is not None:
            if sum(bool(p.get('route_failure')) for p in retried)>=MAX_ROUTE_RETRIES:
                raise StoreError('worker_retry_limit'+(':'+reason if reason else ''))
        elif count>=MAX_ATTEMPT_RETRIES:
            # Carry the last cause, so the user is asked about what failed.
            raise StoreError('worker_retry_limit'+(':'+reason if reason else ''))
        if provision:
            receipts=db.execute("SELECT r.result_json FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor "
                                "WHERE r.task_id=? AND c.attempt_id=? AND r.operation IN ('command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call')",
                                (task_id,provision['attempt_id'])).fetchall()
            if any(not json.loads(row[0]).get('complete') for row in receipts):
                raise StoreError('lost_worker_action_outcome_unknown')
            db.execute('UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND attempt_id=? AND revoked_at IS NULL',
                        (now(),task_id,provision['attempt_id']))
        from .worker_waits import cancel_tx
        cancel_tx(db, task_id, 'recovered_replacement', assignment_ref=assignment, status='orphaned')
        changed=db.execute("UPDATE runtime_nodes SET status='pending',assignment_ref=NULL WHERE task_id=? AND assignment_ref=? AND result_json IS NULL",
                           (task_id,assignment)).rowcount
        if changed!=1:
            raise StoreError('lost_worker_result_changed')
        retry=dict(node=node['node_id'],revision=node['revision'],retry=len(retried)+1,
                   previous_assignment=assignment,observed_session='absent' if provision else 'not_provisioned')
        if unavailable is not None:
            retry.update(route_failure=True,unavailable_route=availability.describe(unavailable))
        event(db,task_id,'ASSIGNMENT_RETRIED',retry)
        db.commit()


def reconcile_results(store, task_id):
    with store.connection() as db:
        requests = db.execute("SELECT * FROM runtime_requests WHERE task_id=? AND operation='result_submit'", (task_id,)).fetchall()
    for request in requests:
        saved = json.loads(request['result_json'])
        if saved.get('complete'):
            continue
        if hashlib.sha256(encoded(saved['result']).encode()).hexdigest() != request['digest']:
            raise StoreError('result_receipt_digest_mismatch')
        # The checks the runtime accepted at submission (older rows carry
        # only the claim; record_result still refuses an unverified pass).
        result = saved.get('accepted', saved['result'])
        if 'report' in saved['result']:
            # An inline report: its report_ref is recorded only after its
            # publication, and must be the one this request's own keyed
            # write_report delivery produced. Before that nothing is
            # published for the result; a live worker's retry continues.
            if 'report_ref' not in result:
                continue
            from ..delegation.mcp_tools import inline_report_ref
            with store.connection() as db:
                expected_ref = inline_report_ref(db, task_id, request['actor'], request['request_key'])
        else:
            expected_ref = saved['result']['report_ref']
        if (result['report_ref'], result['summary']) != (expected_ref, saved['result']['summary']):
            raise StoreError('result_receipt_digest_mismatch')
        with store.connection() as db:
            provision = db.execute('SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
                                   (task_id, saved['assignment_ref'])).fetchone()
            if not provision:
                raise StoreError('result_provision_missing')
            rows = db.execute("SELECT id,payload_json,origin FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result' AND superseded=0",
                              (task_id, provision['attempt_id'])).fetchall()
        matching = [row for row in rows if json.loads(row['payload_json']).get('report_ref') == result['report_ref']]
        if not matching:
            # A still-live worker may retry. A lost worker remains blocked; no
            # publication event or success is manufactured from the receipt.
            continue
        if len(matching) != 1 or matching[0]['origin'] != request['actor']:
            raise StoreError('ambiguous_result_publication')
        observed = json.loads(matching[0]['payload_json'])
        observed_checks = [{k: c[k] for k in ('name', 'outcome', 'tool_call_ref', 'blocker', 'claimed_outcome', 'evidence_gap', 'closed_by', 'command')
                            if k in c and c[k] is not None}
                           for c in observed.get('checks', [])]
        if observed.get('summary') != result['summary'] or observed_checks != result['checks']:
            raise StoreError('result_publication_mismatch')
        TaskMachine(store).record_result(task_id, saved['assignment_ref'], result, request['request_key'])
        saved.update(complete=True, receipt=dict(recorded=True, cursor=matching[0]['id'], superseded=False))
        with store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                       (encoded(saved), task_id, request['actor'], request['request_key']))
            event(db, task_id, 'RESULT_DELIVERY_RECOVERED', dict(assignment_ref=saved['assignment_ref'], report_ref=result['report_ref']))
            db.commit()
