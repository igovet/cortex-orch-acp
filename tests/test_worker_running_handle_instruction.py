"""Running command handles are reused instead of duplicated by every worker."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import WORKER_ALIASES, load_role


_RULE_FRAGMENTS = (
    "retain and poll or resume that same handle",
    "never launch the same or an equivalent command",
    "terminal receipt",
    "cancel or close only the owned handle",
    "report the check unverified",
)


def test_running_handle_rule_reaches_every_standard_worker_engine():
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


def test_running_handle_rule_is_shared_with_optional_resource_guidance():
    from pathlib import Path

    root = Path(__file__).resolve().parents[1]
    paths = (
        root / "agent-sources" / "worker-protocol.md",
        root / "agent-sources" / "references" / "interactive-resources.md",
        root / "skills" / "tool-discipline" / "SKILL.md",
    )
    for path in paths:
        normalized = " ".join(path.read_text(encoding="utf-8").split()).lower()
        for fragment in _RULE_FRAGMENTS:
            assert fragment.lower() in normalized, (path, fragment)
