"""Provision only assignments already reserved by the runtime scheduler."""
import hashlib

from ..contracts import StoreError
from ..delegation.events import append_event
from ..delegation.work_graph import WorkGraph


def dispatch_assignment(store, channel, args):
    key = args['request_key']
    assignment_ref = hashlib.sha256(f'{channel.task_id}|{channel.channel_id}|{key}'.encode()).hexdigest()
    with store.connection() as db:
        node = db.execute('SELECT n.* FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id '
                          "WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision AND n.status='running' "
                          "AND t.state IN ('EXECUTING','REVIEWING')", (channel.task_id, assignment_ref)).fetchone()
    if not node or node['role'] != args['role']:
        raise StoreError('assignment_not_reserved')
    graph = WorkGraph(store)
    graph.add_assignment(channel.task_id, node['objective'], assignment_id=assignment_ref,
                         idempotency_key=f'runtime-assignment:{assignment_ref}')
    attempt = graph.create_attempt(assignment_ref, idempotency_key=f'runtime-attempt:{assignment_ref}')['attempt_id']
    append_event(store, channel.task_id, 'delegation_requested', f'runtime-dispatch:{key}',
                 dict(request_key=key, title=args['title'], brief=args['brief'], role=node['role'],
                      acceptance=args['acceptance'], assignment_ref=assignment_ref,
                      work_attempt_ref=attempt, coordinator_channel_id=channel.channel_id,
                      structured_requirement_state='structured'),
                 origin='runtime', assignment_ref=assignment_ref)
    return dict(assignment_ref=assignment_ref)
