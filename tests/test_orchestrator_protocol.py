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
    assert set(node['properties'])=={'id','role','objective','dependencies','scope','complexity','model','model_reason'}
    assert 'model' in node['required'] and 'auto' in node['properties']['model']['description']
    assert node['additionalProperties'] is False
    assert tools['user_question_request']['inputSchema']['else']['required'] == ['question', 'options']


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
    # Owner decision D8: the consultant is escalation-only, added by the
    # runtime after an escalation_request, never named in a plan.
    escalation_only = {'senior-consultant'}
    catalogue = set(re.findall(r'^- `([^`]+)`:', routing, re.M))
    assert plannable == set(WORKER_ALIASES) - reserved - escalation_only
    assert catalogue == plannable
    assert '## Runtime-reserved (not plannable)' in routing
    assert all('`'+role+'`' in routing for role in reserved | escalation_only)
    assert '`senior-consultant` is escalation-only' in routing
    assert 'bounded report reads' in routing
    assert 'structured result' in routing


def test_harvest_activation_is_explicit_and_does_not_bypass_runtime_review():
    from cortex_runtime.acp.commands import recognize,HARVEST,HARVEST_REFRESH
    assert recognize([dict(type='text',text='/harvest')])[0]==HARVEST
    assert recognize([dict(type='text',text='/harvest-refresh')])[0]==HARVEST_REFRESH
    text=(ROOT/'skills/knowledge-harvest/SKILL.md').read_text()
    assert 'explicit' in text.lower()


def test_reconciliation_is_exclusive_and_language_is_runtime_selected():
    text = load_role('orchestrator').body_portable
    assert "Choose exactly one next action" in text
    assert "do not synthesize" in text
    assert "Otherwise, request a structured user question" in text
    assert "missing information\n  materially changes the outcome" in text
    assert "Otherwise, submit synthesis" in text
    for name in ('orchestrator', 'coordinator-communication', 'context-compaction'):
        body = load_role(name).body_portable
        assert "runtime's per-turn language directive" in body
        assert "on auto" not in body
    assert "interactive-resources" not in text
    assert "code-and-evidence" not in text


def test_routing_guidance_defers_argument_and_receipt_contracts_to_live_schemas():
    routing = (ROOT / 'skills/orchestrator/references/worker-routing.md').read_text()
    assert "Consult `list_route_options`" in routing
    assert "live schema and the plan schema" in routing
    assert "explicitly requests that model family" in routing
    for duplicate in ("affinity_not_eligible", "standard_work_kinds",
                      "flagship.request_names", "ignored:owner_preferred",
                      "pending_dispatch:user_request", "ignored:tier_mismatch"):
        assert duplicate not in routing
