"""T026: a claude-agent-acp connection driving the coordinator role through
the REAL production path (``acp.orchestrating_runtime.OrchestratingRuntime``
-> ``delegation.live_adapters.EngineSessionManagerAdapter`` ->
``engine.session_manager.EngineSessionManager``), never a fake
``SessionRuntime``.

Mirrors ``tests/test_orchestrating_runtime_live_session.py``'s own
coordinator test, with ``engine="claude-agent-acp"`` instead of the default
``"codex-acp"``, against the identical standalone
``tests/fake_engine_process.py`` subprocess (never real claude-agent-acp/
Node -- see that module's own docstring). This proves the coordinator
provisioning/negotiate/session-create/decide path carries zero residual
Codex-only branching: the same ``OrchestratingRuntime``/
``EngineSessionManagerAdapter``/``delegation.mcp_tools`` code that T016's
own test exercises for codex-acp runs unmodified for claude-agent-acp,
*and* succeeds -- the real coordinator role's ``critical_rules`` no longer
refuse it, because claude-agent-acp 0.78.0's own ``session/new``
``_meta.systemPrompt`` channel is QUALIFIED (T026 fresh dist re-verification;
see ``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``'s
own docstring for the full evidence chain), needing no
``allow_unqualified_delivery`` escape hatch at all.

This test additionally asserts the actual wire-level ``session/new``
call carried the real ``_meta.systemPrompt`` payload -- the delivery
mechanism's ``QUALIFIED`` classification is backed by a genuine runtime
effect here, never a paper-only registry entry (see
``delegation.live_adapters._session_new_meta_for``/``open()``'s own
wiring, and ``engine.session_manager.EngineSessionManager.create_session``'s
additive ``session_new_meta`` parameter).
"""
from __future__ import annotations

import threading
import uuid
from pathlib import Path

from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace


def _task(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Check in.", "owner-1")
    return store, result.task_id


def _publish_current_pipeline(store, channel):
    """Give the task-level acceptance gate a real coordinator edition."""
    draft = store.call(
        "create_draft", {"template": "pipeline"},
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    body = pipeline_bootstrap._fill_template(draft["markdown"], channel.task_id, "Check in.", None)
    Path(draft["draft_path"]).write_text(body, encoding="utf-8")
    return store.call(
        "write_report",
        {
            "draft_id": draft["draft_id"],
            "title": "Current pipeline",
            "summary": "Coordinator-authored current pipeline edition",
            "author": "coordinator",
            "request_key": "test-current-pipeline-" + uuid.uuid4().hex,
        },
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )


def test_claude_agent_acp_coordinator_session_runs_through_the_real_session_manager(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    session_new_dump = engine_dir / "session-new-params.json"
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Checked in."))],
        prompt_stop_reason="end_turn",
        session_new_params_dump_path=str(session_new_dump),
    ))
    connection_id = new_connection_id()
    # The only difference from test_orchestrating_runtime_live_session.py's
    # own coordinator test: engine="claude-agent-acp", never "codex-acp".
    record = make_connection_record(
        connection_id, launch_command=fake_launch_command(scenario_path), engine="claude-agent-acp", environ=environ,
    )
    assert record.engine == "claude-agent-acp"

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    # No allow_unqualified_delivery needed at all -- see module docstring.
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(engine="claude-agent-acp", version="0.78.0"),
        wait_timeout_seconds=10.0,
    )
    try:
        channel, session_ref = runtime._ensure_coordinator(task_id)
        assert session_ref.startswith("es_")
        manager = adapter._managers[list(adapter._managers)[0]]
        assert manager.correlation.wire_session_id(session_ref) is not None
        assert manager._record.engine == "claude-agent-acp"
        updates = []
        result = runtime.start_prompt(task_id, "s1", "Just check in.", updates.append, threading.Event())

        assert result == StopReason.END_TURN
        assert "Checked in." in "".join(updates)
        with store.connection() as db:
            state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
        assert state == "waiting_for_user"
        assert adapter._managers  # no structured plan; never accepted
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)

    # F: the real session/new wire call actually carried the qualified
    # _meta.systemPrompt payload -- a genuine runtime effect, not a
    # paper-only registry classification.
    import json

    dumped = json.loads(session_new_dump.read_text(encoding="utf-8"))
    prompt = dumped["_meta"]["systemPrompt"]
    assert prompt.startswith("# Cortex session\n\nYou are the Cortex coordinator")
    # The coordinator role body follows the preamble; its SKILL.md YAML
    # frontmatter is not role content and is stripped from delivery.
    assert "\n\n# Cortex coordinator\n\nYou analyze the user's goal" in prompt
    assert "name: orchestrator" not in prompt
