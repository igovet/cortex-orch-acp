"""Evidence honesty is shared by all specialist instructions."""
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import WORKER_ALIASES,load_role


def test_evidence_rule_reaches_all_standard_workers_on_both_engines():
    ids=set(WORKER_ALIASES.values())-{'worker-senior-consultant'}
    assert len(ids)==22
    for role_id in ids:
        role=load_role(role_id)
        assert 'Unrun, failed, truncated and unverifiable checks remain explicit' in role.body_portable
        assert delivery_for('codex-acp','1.12.0').plan(role,[]).session_new_meta['systemPrompt']==role.body_portable
        assert delivery_for('claude-agent-acp','0.78.0').plan(role,[]).session_new_meta['systemPrompt']['append']==role.body_portable
