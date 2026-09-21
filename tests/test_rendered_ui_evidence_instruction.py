"""UI evidence requirements survive profile generation and engine delivery."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import load_role


def test_qa_and_accessibility_roles_receive_rendered_focus_evidence_rule():
    for role_id in ("worker-qa-engineer", "worker-accessibility-auditor"):
        role = load_role(role_id)
        body = role.body_portable
        assert "rendered page" in body
        assert "keyboard" in body
        assert "unverified" in body
        assert "viewport" in body

        codex = delivery_for("codex-acp", "1.12.0").plan(role, [])
        claude = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
        assert body == codex.session_new_meta["systemPrompt"]
        assert body == claude.session_new_meta["systemPrompt"]["append"]
