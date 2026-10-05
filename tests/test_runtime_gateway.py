import hashlib
from pathlib import Path
import subprocess
from types import SimpleNamespace
import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel, issue_channel
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.orchestration.gateway import dispatch, _write, _read
from cortex_runtime.orchestration.sandbox import gateway_command
from cortex_runtime.orchestration.policy import resume_planning
from cortex_runtime.delegation import mcp_tools


@pytest.fixture
def context(tmp_path):
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    boot=bootstrap_task(store,SourceEvent(origin='gateway-test',event_id='one'),'Implement','owner')
    machine=TaskMachine(store); machine.create(boot.task_id,'Implement')
    channel=resolve_channel(store,boot.channel_id,boot.secret)
    return store,channel,machine,tmp_path


def test_coordinator_can_inspect_but_cannot_modify_or_run(context):
    store,ch,_,root=context
    (root/'hello').write_text('hello')
    assert dispatch(store,ch,'project_read',dict(path='hello'))['content']=='hello'
    for name,args,error in [
        ('project_write',dict(path='hello',content='bad',expected_sha256='absent',request_key='a'),'gateway_scope_denied'),
        ('command_run',dict(argv=['true'],request_key='b'),'gateway_command_denied'),
        ('project_read',dict(path='.cortex/cortex.sqlite3'),'gateway_path_denied')]:
        with pytest.raises(StoreError,match=error): dispatch(store,ch,name,args)
    assert (root/'hello').read_text()=='hello'


def test_files_reject_links_escape_and_conflicting_writes(tmp_path):
    root=tmp_path/'project';root.mkdir()
    outside=tmp_path/'secret';outside.write_text('secret')
    (root/'link').symlink_to(outside)
    (root/'parent').symlink_to(tmp_path,target_is_directory=True)
    for path in ['link','parent/secret','../secret']:
        with pytest.raises((OSError,StoreError)): _read(root,dict(path=path))
        with pytest.raises((OSError,StoreError)): _write(root,dict(path=path,content='bad',expected_sha256='absent'))
    result=_write(root,dict(path='new',content='good',expected_sha256='absent'))
    assert result['sha256']==hashlib.sha256(b'good').hexdigest()
    with pytest.raises(StoreError,match='gateway_write_conflict'):
        _write(root,dict(path='new',content='bad',expected_sha256='absent'))
    assert outside.read_text()=='secret'


def test_registered_draft_gateway_preserves_publication(context):
    store,ch,_,root=context
    draft=store.call('create_draft',dict(template='general',request_key='d'),ch.principal_thread_id,
                     ch.parent_principal_thread_id,trusted=True)
    content=draft['required_first_line']+'\n\n# Verified report\n\nActual evidence.\n'
    args=dict(draft_id=draft['draft_id'],content=content,request_key='write')
    result=dispatch(store,ch,'draft_write',args)
    assert result==dispatch(store,ch,'draft_write',args)
    assert Path(draft['draft_path']).read_text()==content
    with pytest.raises(StoreError,match='request_key_conflict'):
        dispatch(store,ch,'draft_write',dict(args,content=content+'changed'))


def test_inactive_task_fences_actions(context):
    store,ch,machine,root=context
    machine.cancel(ch.task_id)
    with pytest.raises(StoreError,match='gateway_task_inactive'):
        dispatch(store,ch,'project_read',dict(path='.'))


def test_replan_recovery_explains_the_next_turn_and_preserves_receipts(context):
    store,ch,machine,_=context
    replan_args=dict(reason='Need another plan.',request_key='replan')
    receipt=mcp_tools.dispatch(store,ch,'replan_request',replan_args)
    assert receipt['next']=='End this turn. The runtime will stop workers and request the new plan.'
    assert mcp_tools.dispatch(store,ch,'replan_request',replan_args)==receipt
    plan=dict(tasks=[dict(id='retry',role='backend-dev',objective='Retry with new evidence.',
                         dependencies=[],scope=['.'],model='auto')],verification=[['true']])
    with pytest.raises(StoreError,match='gateway_task_recovering') as error:
        mcp_tools.dispatch(store,ch,'plan_submit',dict(plan=plan,request_key='same-turn-plan'))
    assert str(error.value)=='gateway_task_recovering'
    resume_planning(store,ch.task_id)
    assert mcp_tools.dispatch(store,ch,'plan_submit',dict(plan=plan,request_key='resumed-plan'))['state']=='PLAN_READY'



def test_replan_budget_restores_after_user_input(context):
    from cortex_runtime.orchestration.policy import steer
    store,ch,_,_=context
    def replan(key):
        receipt=mcp_tools.dispatch(store,ch,'replan_request',dict(reason='More evidence needed.',request_key=key))
        resume_planning(store,ch.task_id)
        return receipt
    for key in ('one','two','three'): assert replan(key)['accepted']
    with pytest.raises(StoreError,match='replan_limit_requires_user_input'): replan('four')
    assert steer(store,ch.task_id,'Continue with the remaining checks.','user:test:continue')
    for key in ('five','six','seven'): assert replan(key)['accepted']
    with pytest.raises(StoreError,match='replan_limit_requires_user_input'): replan('eight')

@pytest.mark.parametrize('state',['BLOCKED','WAITING_USER'])
def test_blocked_and_waiting_tasks_remain_inactive(context,state):
    store,ch,_,_=context
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?",(state,ch.task_id))
        db.commit()
    with pytest.raises(StoreError,match='gateway_task_inactive'):
        dispatch(store,ch,'project_read',dict(path='.'))


def test_real_scoped_command_cannot_write_other_scope_or_read_runtime(tmp_path):
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    (root/'src').mkdir();(root/'other').write_text('original')
    (root/'.cortex').mkdir();(root/'.cortex'/'secret').write_text('secret')
    outside=tmp_path/'private';outside.write_text('private')
    script="""from pathlib import Path
import os,socket
Path('src/allowed').write_text('allowed')
for path in ['other','.cortex/new']:
 try: Path(path).write_text('forbidden')
 except OSError: pass
 else: raise AssertionError(path)
assert not Path('.cortex/secret').exists()
outside=Path(%r)
assert outside.read_text()=='private'  # the host is visible (this one is under the shared /tmp)
assert 'INHERITED_SECRET' not in os.environ
print('isolated')
""" % str(outside)
    with gateway_command(root,scratch,['python3','-B','-c',script],['src/**']) as (command,fds):
        result=subprocess.run(command,pass_fds=fds,capture_output=True,text=True,env={'INHERITED_SECRET':'secret'})
    assert result.returncode==0,result.stderr
    assert result.stdout=='isolated\n'
    assert (root/'src'/'allowed').read_text()=='allowed'
    assert (root/'other').read_text()=='original'
    assert (root/'.cortex'/'secret').read_text()=='secret'


def test_a_hardlink_to_outside_is_read_only_and_never_refuses_the_worker(tmp_path):
    """A build's hardlinks refused the whole worker; writing through one must
    still never reach the file outside the project."""
    import subprocess
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    outside=tmp_path/'private';outside.write_text('private')
    (root/'linked').hardlink_to(outside);(root/'work').mkdir()
    script='echo x >> linked && echo WROTE; echo ok > work/own.txt && echo OWN'
    with gateway_command(root,scratch,['/bin/sh','-c',script],['.']) as (command,fds):
        done=subprocess.run(command,pass_fds=fds,capture_output=True,text=True,env={'PATH':'/usr/bin:/bin'})
    assert 'WROTE' not in done.stdout and 'OWN' in done.stdout
    assert outside.read_text()=='private' and (root/'work'/'own.txt').read_text()=='ok\n'


def test_hardlinks_wholly_inside_the_writable_project_can_be_reinstalled(tmp_path):
    """esbuild links its two binaries together; pinning them made every
    `npm ci` fail with EBUSY. A file whose every link is a writable project
    path is writable, removable and replaceable."""
    import subprocess
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    one=root/'node_modules'/'esbuild'/'bin'/'esbuild';two=root/'node_modules'/'@esbuild'/'linux-x64'/'bin'/'esbuild'
    one.parent.mkdir(parents=True);two.parent.mkdir(parents=True)
    one.write_text('old');two.hardlink_to(one)
    script=('echo new > node_modules/esbuild/bin/esbuild && rm node_modules/@esbuild/linux-x64/bin/esbuild '
            '&& rm -r node_modules && mkdir -p node_modules/esbuild/bin && echo fresh > node_modules/esbuild/bin/esbuild && echo DONE')
    with gateway_command(root,scratch,['/bin/sh','-c',script],['.']) as (command,fds):
        assert str(one) not in command and str(two) not in command  # never a mount point
        done=subprocess.run(command,pass_fds=fds,capture_output=True,text=True,env={'PATH':'/usr/bin:/bin'})
    assert 'DONE' in done.stdout,done.stderr
    assert one.read_text()=='fresh\n' and not two.exists()


@pytest.mark.parametrize('shape',['outside','denied','ungranted'])
def test_a_project_hardlink_pair_stays_read_only_unless_every_link_is_writable(tmp_path,shape):
    """A third link outside the project, a peer under a deny-write path, or a
    peer outside the granted scope keeps the granted link read-only."""
    import subprocess
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    (root/'work').mkdir();(root/'other').mkdir()
    first=root/'work'/'a';first.write_text('private');(root/'work'/'b').hardlink_to(first)
    readonly=()
    if shape=='outside':
        (tmp_path/'outside').hardlink_to(first);scopes=['.']
    elif shape=='denied':
        (root/'other'/'c').hardlink_to(first);scopes=['.'];readonly=('other/**',)
    else:
        (root/'other'/'c').hardlink_to(first);scopes=['work']
    script='echo x >> work/a && echo WROTE; rm work/b && echo REMOVED; echo ok > work/own.txt && echo OWN'
    with gateway_command(root,scratch,['/bin/sh','-c',script],scopes,readonly=readonly) as (command,fds):
        done=subprocess.run(command,pass_fds=fds,capture_output=True,text=True,env={'PATH':'/usr/bin:/bin'})
    assert 'WROTE' not in done.stdout and 'REMOVED' not in done.stdout and 'OWN' in done.stdout,done
    assert first.read_text()=='private' and (root/'work'/'b').exists()


def test_project_shapes_that_used_to_refuse_the_worker_now_run(tmp_path, monkeypatch):
    """A private-name symlink, a socket in scope, and the tree limit each
    stopped a worker from starting at all."""
    import socket, subprocess
    from cortex_runtime.orchestration import sandbox
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    (root/'shared').mkdir();(root/'shared'/'settings.json').write_text('secret')
    (root/'.claude').symlink_to(root/'shared')
    # Bind by a relative name: an absolute path under a long TMPDIR exceeds
    # the 108-byte AF_UNIX limit.
    monkeypatch.chdir(root)
    server=socket.socket(socket.AF_UNIX);server.bind('db.sock')
    monkeypatch.undo()
    try:
        with gateway_command(root,scratch,['/bin/sh','-c','ls shared; echo done'],['.']) as (command,fds):
            done=subprocess.run(command,pass_fds=fds,capture_output=True,text=True,env={'PATH':'/usr/bin:/bin'})
        assert 'done' in done.stdout and 'settings.json' not in done.stdout  # masked through the link
    finally:
        server.close()
    original=sandbox._TREE_LIMIT
    sandbox._TREE_LIMIT=1
    try:
        with gateway_command(root,scratch,['true'],['.']): pass
        sandbox.verification_command(root,scratch,['true'])
    finally:
        sandbox._TREE_LIMIT=original


@pytest.mark.parametrize('role',['coordinator','worker'])
def test_native_permission_allows_jailed_tools_but_never_subagents(tmp_path,role):
    from cortex_runtime.engine.permissions import PermissionBridge
    from cortex_runtime.engine.resource_binding import BindingRegistry,ResourceBinding
    from cortex_runtime.engine.correlation import CorrelationTables
    bindings=BindingRegistry()
    bindings.bind(ResourceBinding(engine_instance_id='eirt_a',engine_session_id='es_a',context_generation_id='gen_a',
        assignment_id='asg_a',attempt_id='att_a',workspace_binding_id='wb_a',workspace_root=str(tmp_path),ownership_epoch='epoch_a',role=role))
    host_calls=[]
    bridge=PermissionBridge(correlation=CorrelationTables(),bindings=bindings,
                            host_permission_requester=lambda *args: host_calls.append(args))
    base=dict(options=[dict(optionId='allow',kind='allow_once'),dict(optionId='deny',kind='reject_once')])
    for index,name in enumerate(['Bash','Read','Write','apply_patch','Agent','Task']):
        result=bridge.handle(engine_session_id='es_a',wire_session_id='w',wire_request_id=index,
            params=dict(base,toolCall=dict(name=name,title='mcp__cortex__project_read',kind='other',rawInput={'name':'mcp__cortex__project_read'})))
        assert result['outcome']['optionId']==('deny' if name in {'Agent','Task'} else 'allow')
    assert not host_calls
    result=bridge.handle(engine_session_id='es_a',wire_session_id='w',wire_request_id=100,
        params=dict(base,toolCall=dict(name='mcp__cortex__project_read',kind='other')))
    assert result['outcome']['optionId']=='allow'


def test_readonly_git_diff_is_retained_and_never_runs_repository_helpers(context):
    from cortex_runtime.recovery.large_output_store import read_output
    import base64
    store,ch,_,root=context
    subprocess.run(['git','init','-q',str(root)],check=True)
    (root/'file').write_text('before\n')
    subprocess.run(['git','-C',str(root),'add','file'],check=True)
    subprocess.run(['git','-C',str(root),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','-qm','baseline'],check=True)
    (root/'file').write_text('after\n')
    trap=root/'trap'
    subprocess.run(['git','-C',str(root),'config','diff.external',f'touch {trap}'],check=True)
    subprocess.run(['git','-C',str(root),'config','core.fsmonitor',f'touch {trap}'],check=True)
    args=dict(kind='diff',request_key='snapshot')
    result=dispatch(store,ch,'project_git',args)
    assert result['available'] and result['complete']
    assert dispatch(store,ch,'project_git',args)==result
    output=read_output(store,task_id=ch.task_id,owner_scope='coordinator',record_id=result['large_output_id'])
    assert not trap.exists()
    assert '-before' in str(output) or b'-before' in base64.b64decode(output.get('data_base64',''))
    with pytest.raises(StoreError): dispatch(store,ch,'project_read',dict(path='.git/config'))


def test_missing_path_is_read_failure_not_uncertain_publication(context):
    store, channel, _, _ = context
    with pytest.raises(StoreError, match='gateway_path_not_found') as error:
        dispatch(store, channel, 'project_read', {'path': 'absent/notes.md'})
    assert error.value.field == 'path'
    assert 'does not exist' in error.value.correction


def _role_worker(tmp_path,role):
    from runtime_scenarios import provision_fixture
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    boot=bootstrap_task(store,SourceEvent(origin='gateway-role',event_id=role),'Implement','owner')
    coordinator=resolve_channel(store,boot.channel_id,boot.secret)
    worker,_=provision_fixture(store,coordinator,key=role,role=role)
    return store,worker,tmp_path


def test_consultant_reads_no_project_source_and_runs_nothing(tmp_path):
    """The senior consultant works from its consultation packet and retained
    reports only: no project source reads, no commands, no writes."""
    store,worker,root=_role_worker(tmp_path,'senior-consultant')
    (root/'hello').write_text('hello')
    outside=root.parent/('consultant-'+root.name)
    for name,args,error in [
        ('project_read',dict(path='hello'),'gateway_read_denied'),
        ('project_git',dict(kind='status',request_key='git'),'gateway_read_denied'),
        ('command_run',dict(argv=['true'],request_key='run'),'gateway_command_denied'),
        ('project_write',dict(path='hello',content='bad',expected_sha256='absent',request_key='w'),'gateway_scope_denied'),
        ('outside_write',dict(path=str(outside),content='x',request_key='o'),'gateway_scope_denied')]:
        with pytest.raises(StoreError,match=error): dispatch(store,worker,name,args)
    assert (root/'hello').read_text()=='hello' and not outside.exists()


@pytest.mark.parametrize('role',['database-architect','performance-engineer','build-verification'])
def test_read_only_specialists_hold_no_write_grant(tmp_path,role):
    """Their profiles declare them read-only; the gateway agrees."""
    if role == 'build-verification':
        # Reserved roles cannot be planned. Exercise defense in depth on a
        # real channel without enabling dispatch of the reserved profile.
        store,worker,root=_role_worker(tmp_path,'backend-dev')
        with store.connection() as db:
            db.execute('UPDATE runtime_nodes SET role=? WHERE assignment_ref IN '
                       '(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)',
                       ('worker-build-verification',worker.attempt_id))
    else:
        store,worker,root=_role_worker(tmp_path,role)
    (root/'hello').write_text('hello')
    outside=root.parent/(role+'-'+root.name)
    assert dispatch(store,worker,'project_read',dict(path='hello'))['content']=='hello'
    for name,args in [('project_write',dict(path='hello',content='bad',expected_sha256='absent',request_key='w')),
                      ('outside_write',dict(path=str(outside),content='x',request_key='o'))]:
        with pytest.raises(StoreError,match='gateway_scope_denied'): dispatch(store,worker,name,args)
    assert (root/'hello').read_text()=='hello' and not outside.exists()
    names={t['name'] for t in mcp_tools.tool_definitions('worker',role_id='worker-'+role,stage='execution')}
    assert not names & {'project_write','project_mkdir','project_delete','project_skill_link','outside_write','outside_command'}
    assert {'project_read','command_run'} <= names

    if role == 'build-verification':
        result=dispatch(store,worker,'command_run',dict(
            argv=['python3','-B','-c',
                  "from pathlib import Path; p=Path('hello'); assert p.read_text()=='hello'; "
                  "p.write_text('bad')"],request_key='readonly-command'))
        assert result['complete'] and result['exit_code'] != 0
        assert (root/'hello').read_text()=='hello'


@pytest.mark.parametrize('answer',['allow','deny','timeout',None])
@pytest.mark.parametrize('operation',['outside_write','outside_command'])
@pytest.mark.parametrize('mode',['guarded','auto'])
def test_outside_actions_follow_the_tasks_access_mode(tmp_path,tmp_path_factory,operation,answer,mode):
    """Guarded requires a live allow; Auto approves without asking a host."""
    import threading,time
    from cortex_runtime.configuration import access_mode
    from cortex_runtime.orchestration import consent
    from test_access_mode import _runtime
    store,worker,_=_role_worker(tmp_path,'backend-dev')
    target=tmp_path_factory.mktemp('outside')/'made.txt'
    args=(dict(path=str(target),content='x',request_key='o') if operation=='outside_write'
          else dict(argv=['touch',str(target)],cwd=str(target.parent),request_key='o'))
    cards=[]
    def requester(_session,params,_timeout):
        cards.append(params)
        if answer=='timeout': raise TimeoutError('no answer')
        return {'outcome':{'outcome':'selected','optionId':answer}}
    access_mode.switch(store,worker.task_id,mode,origin='acp_set_mode')
    runtime=_runtime(store,{worker.task_id:requester} if answer else {})
    outcome={}
    def run():
        try: outcome['result']=dispatch(store,worker,operation,args)
        except StoreError as error: outcome['error']=str(error)
    thread=threading.Thread(target=run);thread.start()
    deadline=time.monotonic()+20
    while thread.is_alive() and time.monotonic()<deadline:
        consent.relay(runtime,worker.task_id,'en');time.sleep(0.05)
    thread.join(5)
    assert not thread.is_alive()
    with store.connection() as db:
        rows=[tuple(row) for row in db.execute('SELECT kind,status FROM consent_requests WHERE task_id=?',(worker.task_id,))]
    if mode=='auto' or answer=='allow':
        assert 'result' in outcome and target.exists(),outcome
        assert rows==[(operation,'approved')]
        if mode=='auto': assert cards==[]
        else: assert cards and cards[0]['toolCall']['rawInput']['kind']==operation
    else:
        assert outcome=={'error':'outside_action_not_confirmed'}
        assert not target.exists()
        assert rows==[(operation,'denied')]
        assert len(cards)==(1 if answer else 0)
    assert [row['kind'] for row in access_mode.history(store,worker.task_id)]==(
        ['switched','auto_approved'] if mode=='auto' else ['switched'])
