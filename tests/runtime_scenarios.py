"""Scripted model proposals for exercising the actual deterministic controller."""
import uuid
from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.delegation import mcp_tools


def propose_inspection(store,channel):
    return mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(
        tasks=[dict(id='inspect',role='explorer',objective='Inspect requested change',dependencies=[],scope=['.'],model='auto')],
        verification=[['python3','-B','-c','print("verified")']]),request_key=str(uuid.uuid4())))


def publish_result(store,channel):
    draft=store.call('create_draft',dict(template='general'),channel.principal_thread_id,
                     channel.parent_principal_thread_id,trusted=True)
    mcp_tools.dispatch(store,channel,'draft_write',dict(draft_id=draft['draft_id'],
        content=draft['required_first_line']+'\n\n# Inspection\n\nInspected the disposable fixture. No project edits.\n',request_key=str(uuid.uuid4())))
    report=store.call('write_report',dict(draft_id=draft['draft_id'],title='Inspection',summary='Fixture observation',
        author='explorer',request_key=str(uuid.uuid4())),
        channel.principal_thread_id,channel.parent_principal_thread_id,trusted=True)
    with store.connection() as db:
        row=db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',(channel.attempt_id,)).fetchone()
    args=dict(report=dict(report_ref=report['report_id']),summary='Fixture observation',checks=[],request_key=str(uuid.uuid4()))
    if row['role_id']=='worker-code-reviewer': args['verdict']='APPROVE'
    mcp_tools.dispatch(store,channel,'result_submit',args)
    return PromptOutcome(StopReason.END_TURN,'Visible worker result')


def reserve_assignment(store, channel, args):
    """Unit-fixture reservation, never a model-facing compatibility operation.

    Storage/capsule tests supply a runtime decision directly. End-to-end tests
    instead use plan_submit and TaskMachine.claim_ready.
    """
    import hashlib
    from cortex_runtime.orchestration.state import TaskMachine, encoded, event
    from cortex_runtime.orchestration.scheduler import dispatch_assignment
    from cortex_runtime.portable.roles import resolve_role
    machine=TaskMachine(store);machine.create(channel.task_id,'Fixture goal')
    args=dict(args,role=resolve_role(args['role']))
    ref=hashlib.sha256(f"{channel.task_id}|{channel.channel_id}|{args['request_key']}".encode()).hexdigest()
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        if not db.execute('SELECT 1 FROM runtime_nodes WHERE assignment_ref=?',(ref,)).fetchone():
            db.execute("UPDATE runtime_tasks SET revision=1,state='EXECUTING' WHERE task_id=?",(channel.task_id,))
            db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                (channel.task_id,1,'fixture_'+ref[:12],args['role'],args['title'],'[]',encoded([f'fixture/{ref}/**']),'execution','running',ref,None))
            event(db,channel.task_id,'ASSIGNMENT_CREATED',dict(assignment_ref=ref,fixture=True))
        db.commit()
    return dispatch_assignment(store,channel,args)


def provision_fixture(store, coordinator, *, key='worker', role='backend-dev'):
    from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
    from cortex_runtime.portable.roles import resolve_role
    reserved=reserve_assignment(store,coordinator,dict(title=key,brief=key,role=role,acceptance='Exact report',request_key=key))
    assignment='a_'+uuid.uuid4().hex[:12];attempt='xa_'+uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        channel,secret=issue_channel_tx(db,coordinator.task_id,'worker',coordinator.owner_epoch,parent_channel_id=coordinator.channel_id,
                                      assignment_id=assignment,attempt_id=attempt)
        stamp=store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (reserved['assignment_ref'],coordinator.task_id,assignment,attempt,resolve_role(role),'ready','ready','shared_project_root',channel,None,None,None,'qualified',None,coordinator.owner_epoch,stamp,stamp))
        db.commit()
    return resolve_channel(store,channel,secret),reserved['assignment_ref']


def publish_fixture_report(store, worker, text='Observed report'):
    draft=store.call('create_draft',dict(template='general',request_key=str(uuid.uuid4())),worker.principal_thread_id,worker.parent_principal_thread_id,trusted=True)
    mcp_tools.dispatch(store,worker,'draft_write',dict(draft_id=draft['draft_id'],content=draft['required_first_line']+'\n\n# Report\n\n'+text+'\n',request_key=str(uuid.uuid4())))
    return store.call('write_report',dict(draft_id=draft['draft_id'],title='Report',summary=text,author='worker',request_key=str(uuid.uuid4())),worker.principal_thread_id,worker.parent_principal_thread_id,trusted=True)['report_id']


def bind_fixture_worker(store, channel_id, role='backend-dev'):
    """Give an already-issued channel a real runtime assignment in identity tests."""
    from types import SimpleNamespace
    from cortex_runtime.portable.roles import resolve_role
    with store.connection() as db:
        worker=dict(db.execute('SELECT * FROM trusted_channels WHERE channel_id=?',(channel_id,)).fetchone())
        parent=dict(db.execute('SELECT * FROM trusted_channels WHERE principal_thread_id=?',(worker['parent_principal_thread_id'],)).fetchone())
    coordinator=SimpleNamespace(**parent)
    reserved=reserve_assignment(store,coordinator,dict(title='Fixture',brief='Fixture',role=role,acceptance='Report',request_key=channel_id))
    assignment=worker['assignment_id'] or 'a_'+uuid.uuid4().hex[:12]
    attempt=worker['attempt_id'] or 'xa_'+uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('UPDATE trusted_channels SET assignment_id=?,attempt_id=? WHERE channel_id=?',(assignment,attempt,channel_id))
        stamp=store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (reserved['assignment_ref'],worker['task_id'],assignment,attempt,resolve_role(role),'ready','ready','shared_project_root',channel_id,None,None,None,'qualified',None,worker['owner_epoch'],stamp,stamp))
        db.commit()
    return reserved
