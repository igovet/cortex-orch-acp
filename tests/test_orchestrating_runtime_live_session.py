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
import json
import tempfile
from pathlib import Path
import pytest

from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.contracts import StoreError
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


def _publish_current_pipeline(store, channel, extra=""):
    """Give the task-level acceptance gate a real coordinator edition."""
    draft = store.call(
        "create_draft", {"template": "pipeline"},
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    body = pipeline_bootstrap._fill_template(draft["markdown"], channel.task_id, "Check in.", None)
    Path(draft["draft_path"]).write_text(body + extra, encoding="utf-8")
    return store.call(
        "write_report",
        {
            "draft_id": draft["draft_id"],
            "title": "Current pipeline",
            "summary": "Coordinator-authored current pipeline edition",
            "author": "coordinator",
            "request_key": "test-current-pipeline-" + uuid_str(),
        },
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )














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
