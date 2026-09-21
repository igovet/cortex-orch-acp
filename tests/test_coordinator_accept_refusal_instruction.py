"""Acceptance-refusal recovery reaches the coordinator in both ACP engines."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import load_role


def test_refused_accept_requires_changed_durable_state_before_retry():
    role = load_role("orchestrator")
    body = role.body_portable
    normalized = " ".join(body.split())
    assert "repair the missing durable evidence or state" in normalized
    assert "wait for pending events to be delivered" in normalized
    assert "Never repeat a refused `decide(accept)` against unchanged" in normalized
    assert "while workers are active, remain silent" in normalized

    codex = delivery_for("codex-acp", "1.12.0").plan(role, [])
    claude = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
    assert codex.session_new_meta["systemPrompt"] == body
    assert claude.session_new_meta["systemPrompt"]["append"] == body
