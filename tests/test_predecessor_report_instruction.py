"""Evidence references and the shared report protocol survive engine delivery."""
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import load_role, read_reference


def test_routing_uses_runtime_references_and_bounded_reads():
    routing=read_reference('worker-routing',role_kind='coordinator')
    assert 'exact reports returned in task context' in routing
    assert 'bounded report reads' in routing


def test_coordinator_and_qa_use_retained_reports_through_both_engines():
    for role_id in ('orchestrator','worker-qa-engineer'):
        role=load_role(role_id);body=role.body_portable
        assert 'report' in body
        assert delivery_for('codex-acp','1.12.0').plan(role,[]).session_new_meta['systemPrompt']==body
        assert delivery_for('claude-agent-acp','0.78.0').plan(role,[]).session_new_meta['systemPrompt']['append']==body
    normalized_body = ' '.join(load_role('worker-qa-engineer').body_portable.split())
    assert 'exact predecessor report references' in normalized_body
