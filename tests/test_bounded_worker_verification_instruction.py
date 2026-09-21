"""Bounded environment failures remain terminal in every worker delivery."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import WORKER_ALIASES, load_role


_RULE_FRAGMENTS = (
    "do not retry that check",
    "equivalent runner, browser, or dependency installation",
    "Do not fetch or install dependencies",
    "mark the claim unverified",
    "let the coordinator route any independent verification",
)


def test_bounded_environment_failure_rule_reaches_every_standard_worker_engine():
    role_ids = sorted(set(WORKER_ALIASES.values()) - {"worker-senior-consultant"})
    assert len(role_ids) == 22
    for role_id in role_ids:
        role = load_role(role_id)
        normalized = " ".join(role.body_portable.split())
        for fragment in _RULE_FRAGMENTS:
            assert fragment in normalized, (role_id, fragment)

        codex = delivery_for("codex-acp", "1.12.0").plan(role, [])
        claude = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
        assert role.body_portable == codex.session_new_meta["systemPrompt"]
        assert role.body_portable == claude.session_new_meta["systemPrompt"]["append"]
