"""Closed proposal contracts: no model-owned lifecycle or routing controls."""
from copy import deepcopy
import pytest
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as dc,mcp_tools
from cortex_runtime.orchestration.contracts import COORDINATOR,WORKER
from cortex_runtime.orchestration.gateway import SCHEMAS

PLAN=dict(plan=dict(tasks=[dict(id='inspect',role='explorer',objective='Inspect the project',dependencies=[],scope=['src/**'])],
                    verification=[['python3','-B','-c','print(1)']]),request_key='plan')


def test_schemas_are_fresh_and_role_surfaces_are_closed():
    first=dc.schemas_for('coordinator');first['plan_submit']['properties']['request_key']['maxLength']=1
    assert dc.schemas_for('coordinator')['plan_submit']['properties']['request_key']['maxLength']==128
    assert set(dc.schemas_for('coordinator'))==set(dc.SHARED)|set(COORDINATOR)|set(SCHEMAS)|{'read_diagnostics'}
    assert set(dc.schemas_for('worker'))==set(dc.SHARED)|set(WORKER)|set(SCHEMAS)|{'ask_question'}
    with pytest.raises(StoreError,match='unknown_role_kind'): dc.schemas_for('unknown')


@pytest.mark.parametrize('role',['coordinator','worker'])
@pytest.mark.parametrize('operation',['delegate','decide','cancel','integrate','update_plan','prepare_candidate','request_check','assess_check','complete_assignment','publish_result'])
def test_retired_operations_have_no_schema_or_dispatch_alias(role,operation):
    assert operation not in dc.schemas_for(role)
    with pytest.raises(StoreError,match='unknown_operation'): dc.validate(role,operation,{})


def test_no_model_contract_accepts_authority_selectors():
    forbidden={'task_id','attempt_id','session_id','channel_id','workspace_id','workspace_binding_id','epoch','owner_epoch','assignment_id','model','effort','engine'}
    def inspect(schema):
        if schema.get('type')=='object':
            assert schema['additionalProperties'] is False
            assert not forbidden & set(schema['properties'])
            for child in schema['properties'].values(): inspect(child)
        if schema.get('type')=='array': inspect(schema['items'])
    for role in ['coordinator','worker']:
        for schema in dc.schemas_for(role).values(): inspect(schema)


def test_plan_contract_accepts_valid_dag_proposal():
    assert dc.validate('coordinator','plan_submit',PLAN)==PLAN


@pytest.mark.parametrize('mutation',['unknown','missing','wrong_type','engine','model','effort','empty_tasks','empty_verification','reserved_role'])
def test_plan_schema_rejects_invalid_or_runtime_owned_fields(mutation):
    args=deepcopy(PLAN)
    if mutation=='unknown': args['id']='invented'
    elif mutation=='missing': del args['request_key']
    elif mutation=='wrong_type': args['plan']['tasks']=False
    elif mutation in {'engine','model','effort'}: args['plan']['tasks'][0][mutation]='model-choice'
    elif mutation=='empty_tasks': args['plan']['tasks']=[]
    elif mutation=='empty_verification': args['plan']['verification']=[]
    else: args['plan']['tasks'][0]['role']='code-reviewer'
    with pytest.raises(StoreError,match='invalid_arguments'): dc.validate('coordinator','plan_submit',args)


def test_report_identity_and_verdict_are_structured():
    args=dict(report_ref='r_'+'a'*12,summary='Observed',checks=[],request_key='result')
    dc.validate('worker','result_submit',args)
    for value in ['not-a-report','r_'+'g'*12,'../report.md']:
        with pytest.raises(StoreError): dc.validate('worker','result_submit',dict(args,report_ref=value))
    with pytest.raises(StoreError): dc.validate('worker','result_submit',dict(args,verdict='looks fine'))


def test_read_skill_uses_only_the_advertised_selector():
    dc.validate('worker','read_skill',dict(skill='context-compaction'))
    for args in [dict(topic='context-compaction'),dict(name='context-compaction'),dict(skill='context-compaction',topic='extra')]:
        with pytest.raises(StoreError): dc.validate('worker','read_skill',args)


def test_integer_fields_reject_boolean_and_array_bounds_are_real():
    with pytest.raises(StoreError): dc.validate('worker','project_read',dict(path='src',offset=True))
    with pytest.raises(StoreError): dc.validate('worker','command_run',dict(argv=[],request_key='empty'))
    with pytest.raises(StoreError): dc.validate('worker','scope_request',dict(scope=[],reason='why',request_key='scope'))
