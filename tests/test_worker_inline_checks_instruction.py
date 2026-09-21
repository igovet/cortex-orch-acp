"""A one-off worker check must not create an outside-project helper script."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import WORKER_ALIASES, load_role


def test_short_inline_check_rule_reaches_both_engines_for_every_standard_worker():
    role_ids = sorted(set(WORKER_ALIASES.values()) - {"worker-senior-consultant"})
    assert len(role_ids) == 22
    for role_id in role_ids:
        role = load_role(role_id)
        body = " ".join(role.body_portable.split())
        assert "CSS contrast calculations short and in memory" in body, role_id
        assert "never write a scratch helper script under `/tmp`" in body, role_id
        assert "report the gap instead of building a new outside-project checker" in body, role_id
        assert delivery_for("codex-acp", "1.12.0").plan(role, []).session_new_meta["systemPrompt"] == role.body_portable
        assert delivery_for("claude-agent-acp", "0.78.0").plan(role, []).session_new_meta["systemPrompt"]["append"] == role.body_portable
