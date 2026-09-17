"""T016: OrchestratingRuntime driven through the REAL EngineSessionManager
(EngineSessionManagerAdapter), coordinator session backed by the standalone
``tests/fake_engine_process.py`` subprocess -- never a real engine, but a
real ``session/new`` + ``session/prompt`` ACP wire exchange through the
production session-manager machinery (negotiate/create_session/prompt_turn),
proving the coordinator prompt/continuation path is not a bespoke shortcut
around it.

The fake engine process is a scripted stdio ACP peer; it cannot itself call
a Cortex MCP tool. Per the T016 lead's guidance, the delegation-operation
side effect a real model's tool call would produce is instead injected
directly via ``delegation.mcp_tools.dispatch`` using the coordinator's own
resolved ``ChannelContext`` -- the same handler a real engine's Cortex MCP
subprocess would call.
"""
from __future__ import annotations

import threading

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
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid_str())), "Check in.", "owner-1")
    return store, result.task_id


def uuid_str():
    import uuid

    return str(uuid.uuid4())


def test_coordinator_turn_runs_through_the_real_session_manager(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Checked in."))],
        prompt_stop_reason="end_turn",
    ))
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    try:
        # Open the real coordinator session up front (through the real
        # EngineSessionManager + fake_engine_process subprocess) and, in its
        # place, inject the single delegation operation call a real model
        # would have made via its Cortex MCP subprocess: a task-level
        # accept (nothing needed further work for this trivial check-in).
        channel, session_ref = runtime._ensure_coordinator(task_id)
        assert session_ref.startswith("es_")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Trivial check-in, nothing to delegate.", request_key="acc1",
        ))

        updates = []
        result = runtime.start_prompt(task_id, "s1", "Just check in.", updates.append, threading.Event())

        assert result == StopReason.END_TURN
        # The visible text came from the real fake_engine_process turn, not
        # a fabricated/fixed string.
        assert any("Checked in." in u for u in updates)
        with store.connection() as db:
            state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
        assert state == "accepted"
        # Proves the real session-manager path: a live wire session id was
        # negotiated/created and the correlation table knows it.
        manager = adapter._managers[list(adapter._managers)[0]]
        assert manager.correlation.wire_session_id(session_ref) is not None
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_adapter_refuses_to_open_a_channel_id_with_no_secret(tmp_path):
    """T016 review (HIGH): EngineSessionManagerAdapter.open must never
    silently launch an engine session with a channel_id but no secret."""
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    try:
        with __import__("pytest").raises(RuntimeError, match="channel_secret"):
            adapter.open("shared_project_root", dict(
                task_id="t_aaaaaaaaaaaa", attempt_id="xa_aaaaaaaaaaaa",
                channel_id="ch_aaaaaaaaaaaa", channel_secret=None,
            ), "instructions")
        assert not adapter._managers  # never registered a manager for the refused open
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)
