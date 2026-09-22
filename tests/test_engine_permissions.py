"""Native actions are allowed (the engine jail bounds them); native subagents never are."""
import threading
import pytest
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import DuplicateEnginePermissionRequestId,PermissionBridge
from cortex_runtime.engine.resource_binding import BindingRegistry,ResourceBinding,ResourceBindingError


def _binding(tmp_path,**overrides):
    return ResourceBinding(**(dict(engine_instance_id='eirt_a',engine_session_id='es_a',context_generation_id='gen_1',
        assignment_id='asg_1',attempt_id='att_1',workspace_binding_id='wb_1',workspace_root=str(tmp_path),ownership_epoch='epoch_1',role='worker')|overrides))


def _bridge(*,host_permission_requester=None,correlation=None,bindings=None):
    return PermissionBridge(correlation=correlation or CorrelationTables(),bindings=bindings or BindingRegistry(),host_permission_requester=host_permission_requester)


PARAMS=dict(toolCall=dict(toolCallId='tc-1',name='mcp__cortex__project_read'),
            options=[dict(optionId='allow',kind='allow_once'),dict(optionId='deny',kind='reject_once')])


def call(bridge,params=PARAMS,request_id=1,session='es_a'):
    return bridge.handle(engine_session_id=session,wire_session_id='w1',wire_request_id=request_id,params=params)


def test_binding_requires_complete_canonical_identity(tmp_path):
    for changes in [dict(engine_instance_id=''),dict(workspace_root=str(tmp_path)+'/../'+tmp_path.name)]:
        with pytest.raises(ResourceBindingError): _binding(tmp_path,**changes)


@pytest.mark.parametrize('role',['worker','coordinator'])
@pytest.mark.parametrize('name',['Bash','Read','Write','Edit','Glob','Grep','Skill','apply_patch','exec_command',None,'unknown'])
def test_native_actions_are_allowed_without_asking_the_host(tmp_path,role,name):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path,role=role));host=[]
    bridge=_bridge(bindings=bindings,host_permission_requester=lambda *a: host.append(a) or {'outcome':{'outcome':'selected','optionId':'deny'}})
    params=dict(PARAMS,toolCall=dict(name=name,kind='read',rawInput=dict(command='cat allowed')))
    assert call(bridge,params)=={'outcome':{'outcome':'selected','optionId':'allow'}}
    assert host==[]


@pytest.mark.parametrize('role',['worker','coordinator'])
@pytest.mark.parametrize('toolcall',[dict(name='Agent'),dict(name='Task'),dict(name='spawn_agent'),
                                     dict(_meta=dict(claudeCode=dict(toolName='Workflow'))),
                                     dict(_meta=dict(claudeCode=dict(toolName='TeamCreate')))])
def test_native_subagents_are_denied(tmp_path,role,toolcall):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path,role=role));host=[]
    bridge=_bridge(bindings=bindings,host_permission_requester=lambda *a: host.append(a) or {'outcome':{'outcome':'selected','optionId':'allow'}})
    assert call(bridge,dict(PARAMS,toolCall=dict(toolcall,title='Read')))=={'outcome':{'outcome':'selected','optionId':'deny'}}
    assert host==[]


@pytest.mark.parametrize('name',['mcp__cortex__project_read','mcp__runtime_external__inspect'])
@pytest.mark.parametrize('kind',['allow_once','allow_always'])
def test_mediated_call_uses_only_an_offered_option(tmp_path,name,kind):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path));bridge=_bridge(bindings=bindings)
    params=dict(toolCall=dict(name=name),options=[dict(optionId='exact-offered',kind=kind)])
    assert call(bridge,params)=={'outcome':{'outcome':'selected','optionId':'exact-offered'}}
    assert call(bridge,dict(params,options=[]))=={'outcome':{'outcome':'cancelled'}}


@pytest.mark.parametrize('invalidator',['missing','release','session','epoch','action'])
def test_stale_identity_never_allows_even_a_gateway_request(tmp_path,invalidator):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path));correlation=CorrelationTables()
    bridge=_bridge(bindings=bindings,correlation=correlation)
    if invalidator=='release': bindings.release('es_a')
    elif invalidator=='session': bindings.invalidate_session('es_a')
    elif invalidator=='epoch': bindings.invalidate_epoch('wb_1','new')
    elif invalidator=='action':
        action=correlation.portable_tool_call_ref('w1','tc-1');bindings.invalidate_action('es_a',action)
    assert call(bridge,session='missing' if invalidator=='missing' else 'es_a')=={'outcome':{'outcome':'cancelled'}}


def test_new_generation_accepts_only_its_fresh_binding(tmp_path):
    bindings=BindingRegistry();old=_binding(tmp_path);bindings.bind(old)
    bindings.invalidate_generation('es_a','gen_2')
    assert not old.is_current(bindings.snapshot(old))
    assert call(_bridge(bindings=bindings))['outcome']['optionId']=='allow'


def test_duplicate_inflight_request_is_protocol_error_and_bookkeeping_is_released(tmp_path,monkeypatch):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path));bridge=_bridge(bindings=bindings)
    entered=threading.Event();release=threading.Event();results=[];original=bridge._handle_one
    def slow(*args):
        entered.set();assert release.wait(3);return original(*args)
    monkeypatch.setattr(bridge,'_handle_one',slow)
    thread=threading.Thread(target=lambda: results.append(call(bridge,request_id=42)));thread.start()
    try:
        assert entered.wait(2)
        assert bridge.pending_actions_for_wire_session('w1')
        with pytest.raises(DuplicateEnginePermissionRequestId): call(bridge,request_id=42)
    finally: release.set();thread.join(3)
    assert len(results)==1 and results[0]['outcome']['optionId']=='allow'
    assert bridge.pending_actions_for_wire_session('w1')==[]


def test_notifier_failure_cannot_turn_refusal_into_approval(tmp_path):
    bindings=BindingRegistry();bindings.bind(_binding(tmp_path))
    def broken(*args): raise RuntimeError('observer failed')
    bridge=PermissionBridge(correlation=CorrelationTables(),bindings=bindings,coordinator_refusal_notifier=broken)
    assert call(bridge,dict(PARAMS,toolCall=dict(name='Agent')))['outcome']['optionId']=='deny'
