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
assert not Path(%r).exists()
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


def test_project_shapes_that_used_to_refuse_the_worker_now_run(tmp_path):
    """A private-name symlink, a socket in scope, and the tree limit each
    stopped a worker from starting at all."""
    import socket, subprocess
    from cortex_runtime.orchestration import sandbox
    root=tmp_path/'project';root.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    (root/'shared').mkdir();(root/'shared'/'settings.json').write_text('secret')
    (root/'.claude').symlink_to(root/'shared')
    server=socket.socket(socket.AF_UNIX);server.bind(str(root/'db.sock'))
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
