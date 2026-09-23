"""Coordinator instructions describe reasoning; executable policy lives in runtime."""
from pathlib import Path
from cortex_runtime.delegation.mcp_tools import tool_definitions
from cortex_runtime.portable.roles import load_role,WORKER_ALIASES

ROOT=Path(__file__).resolve().parents[1]


def test_coordinator_has_proposals_and_questions_but_no_lifecycle_or_route_controls():
    tools={t['name']:t for t in tool_definitions('coordinator')}
    assert {'plan_submit','synthesis_submit','user_question_request','task_get_context'} <= tools.keys()
    assert not {'delegate','decide','cancel','complete_task','set_task_state','integrate'} & tools.keys()
    plan=tools['plan_submit']['inputSchema']['properties']['plan']
    node=plan['properties']['tasks']['items']
    assert set(node['properties'])=={'id','role','objective','dependencies','scope','complexity','provider'}  # bounded hints, never a route
    assert node['properties']['provider']['enum']==['codex-acp','claude-agent-acp']  # a provider preference, never a model
    assert node['additionalProperties'] is False
    assert 'question' in tools['user_question_request']['inputSchema']['required']


def test_instruction_sources_do_not_recreate_retired_model_authority():
    paths=[ROOT/'skills/orchestrator/SKILL.md',*(ROOT/'skills/orchestrator/references').glob('*.md')]
    for path in paths:
        text=path.read_text()
        assert not any('`'+name+'`' in text for name in ['delegate','decide','complete_task','integrate']),path
    text=load_role('orchestrator').body_portable
    assert 'runtime owns the task lifecycle' in text
    assert 'A model response does not mark the task complete' in text
    assert 'immutable reports' in text


def test_all_specialists_remain_discoverable_and_reports_remain_selective():
    assert len(WORKER_ALIASES)==23
    routing=(ROOT/'skills/orchestrator/references/worker-routing.md').read_text()
    from cortex_runtime.orchestration.contracts import NODE
    import re
    plannable = set(NODE['properties']['role']['enum'])
    reserved = {'code-reviewer', 'build-verification'}
    catalogue = set(re.findall(r'^- `([^`]+)`:', routing, re.M))
    assert plannable == set(WORKER_ALIASES) - reserved
    assert catalogue == plannable
    assert '## Runtime-reserved (not plannable)' in routing
    assert all('`'+role+'`' in routing for role in reserved)
    assert 'bounded report reads' in routing
    assert 'structured result' in routing


def test_harvest_activation_is_explicit_and_does_not_bypass_runtime_review():
    from cortex_runtime.acp.commands import recognize,HARVEST,HARVEST_REFRESH
    assert recognize([dict(type='text',text='/harvest')])[0]==HARVEST
    assert recognize([dict(type='text',text='/harvest-refresh')])[0]==HARVEST_REFRESH
    text=(ROOT/'skills/knowledge-harvest/SKILL.md').read_text()
    assert 'explicit' in text.lower()
