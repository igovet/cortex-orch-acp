"""A continuation must use the instructions and route of its own session."""

from types import SimpleNamespace
from unittest.mock import patch

from cortex_runtime.acp import orchestrating_runtime as orchestration
from cortex_runtime.portable.roles import composed_instructions, load_role


def _deliver(*, coordinator_engine: str, worker_engine: str, instructions: str,
             cause: str = "session_open", pending_delivery: bool = True) -> str:
    runtime = object.__new__(orchestration.OrchestratingRuntime)
    runtime._store = None
    runtime._allow_unqualified_delivery = False
    runtime._config = SimpleNamespace(snapshot=lambda _task: {
        "engine": coordinator_engine,
        "version": "0.78.0" if coordinator_engine == "claude-agent-acp" else "1.12.0",
    })
    runtime._resolve_context_window_tokens = lambda *_args: 200_000
    pending = {
        "generation": {
            "id": "generation-fixture", "cause": cause, "assignment_ref": "assignment-fixture",
        },
        "recipient": "worker", "attempt_id": "attempt-fixture",
        "engine": worker_engine,
        "version": "0.78.0" if worker_engine == "claude-agent-acp" else "1.12.0",
        "instructions": instructions,
    }
    runtime._engine = SimpleNamespace(
        pending_delivery=lambda _session: pending if pending_delivery else None,
        begin_delivery=lambda *_args: {"id": "delivery-fixture", "state": "built"},
        mark_delivery_transmitted=lambda *_args: None,
    )
    with patch.object(orchestration, "build_capsule", return_value={
        "capsule_revision": "revision-fixture", "size_estimate": {"critical_bytes": 10, "bytes": 10},
    }), patch.object(orchestration, "render_capsule", return_value="CAPSULE_FIXTURE"), patch(
        "cortex_runtime.recovery.compaction_guard.evaluate_compaction", return_value={"status": "allowed"},
    ):
        text, delivery_id, blocked = runtime._begin_generation_delivery(
            "task-fixture", "worker-session", "worker-backend-dev", "ASSIGNMENT_FIXTURE",
        )
    assert delivery_id == ("delivery-fixture" if pending_delivery else None)
    assert not blocked
    return text


def test_codex_worker_role_stays_in_developer_channel_on_open():
    role = load_role("worker-backend-dev")
    instructions = composed_instructions(role) + "\nAllowed worker routes per role: fixture-route\n"
    text = _deliver(
        coordinator_engine="claude-agent-acp", worker_engine="codex-acp", instructions=instructions,
    )
    assert role.body_portable.strip() not in text
    assert "## Shared skills, and when to read one" not in text
    assert "fixture-route" not in text
    assert "CAPSULE_FIXTURE" in text
    assert text.endswith("ASSIGNMENT_FIXTURE")


def test_codex_worker_continuation_has_no_role_copy_in_user_prompt():
    instructions = composed_instructions(load_role("worker-backend-dev"))
    text = _deliver(
        coordinator_engine="claude-agent-acp", worker_engine="codex-acp",
        instructions=instructions, pending_delivery=False,
    )
    assert text == "ASSIGNMENT_FIXTURE"


def test_codex_worker_compaction_capsule_has_no_role_copy_in_user_prompt():
    instructions = composed_instructions(load_role("worker-backend-dev"))
    text = _deliver(
        coordinator_engine="claude-agent-acp", worker_engine="codex-acp",
        instructions=instructions, cause="observed_compaction",
    )
    assert "CAPSULE_FIXTURE" in text
    assert "## Shared skills, and when to read one" not in text
    assert "CORTEX_INSTRUCTIONS_BEGIN:" not in text
    assert text.endswith("ASSIGNMENT_FIXTURE")


def test_claude_worker_uses_its_system_channel_with_codex_coordinator():
    text = _deliver(
        coordinator_engine="codex-acp", worker_engine="claude-agent-acp",
        instructions=composed_instructions(load_role("worker-backend-dev")),
    )
    assert "CAPSULE_FIXTURE" in text
    assert "## Shared skills, and when to read one" not in text
    assert text.endswith("ASSIGNMENT_FIXTURE")
