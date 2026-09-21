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
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, _is_cortex_decide_tool, _scrub_accepted_final
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
        # Acceptance releases the owned manager, so inspect the negotiated
        # wire correlation while the session is still live.
        manager = adapter._managers[list(adapter._managers)[0]]
        assert manager.correlation.wire_session_id(session_ref) is not None
        _publish_current_pipeline(store, channel)
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
        assert not adapter._managers  # accepted task releases its owned engine
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


@pytest.mark.parametrize("engine,title", [
    ("codex-acp", "mcp.cortex.decide"),
    ("claude-agent-acp", "mcp__cortex__decide"),
])
def test_two_failed_accept_updates_cancel_only_current_wire_turn(tmp_path, request, engine, title):
    """Both engines' ACP tool identities cancel one turn, then deliver the event."""

    task_area = tmp_path / "work"
    task_area.mkdir()
    store, task_id = _task(task_area)
    # /tmp/.git on this host is intentionally rejected as an ancestor of
    # connection state; keep the fake engine's HOME outside that project.
    engine_root = tempfile.TemporaryDirectory(prefix="cortex-turn-wire-", dir="/var/tmp")
    request.addfinalizer(engine_root.cleanup)
    engine_dir = Path(engine_root.name)
    environ = isolated_environ(engine_dir)
    prompt_log = engine_dir / "prompts.jsonl"
    correction = (
        "Task-level accept refused: unconsumed meaningful event(s) remain for this task: progress. "
        "Finish this coordinator turn without retrying acceptance; the runtime will deliver those events."
    )
    error = dict(result=dict(content=[dict(type="text", text=json.dumps(dict(
        error="acceptance_refused", correction=correction,
    )))]))
    updates = []
    for index in range(2):
        ref = f"ref-{index}"
        updates.extend((
            dict(sessionUpdate="tool_call", toolCallId=ref, title=title,
                 kind="execute", status="pending", rawInput=dict(assignment_ref="task", decision="accept")),
            dict(sessionUpdate="tool_call_update", toolCallId=ref, status="failed", rawOutput=error),
        ))
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_behavior="updates_then_wait_for_cancel_once",
        prompt_updates=updates, prompt_log_path=str(prompt_log),
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), engine=engine, environ=environ)
    adapter = EngineSessionManagerAdapter(
        lambda _task_id: record, lambda _task_id: str(task_area / "project"),
        environ=environ, store=store, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(
            engine=engine, version="0.78.0" if engine == "claude-agent-acp" else "1.12.0",
        ),
        allow_unqualified_delivery=engine == "codex-acp", wait_timeout_seconds=2.0,
    )
    try:
        channel, _ = runtime._ensure_coordinator(task_id)
        _publish_current_pipeline(store, channel)
        original_prompt = adapter.prompt
        calls = 0
        seeded = False

        def prompt_with_real_event(_session_ref, message, cancel_event, **kwargs):
            nonlocal calls, seeded
            calls += 1
            if calls == 2:
                # A new coordinator continuation has claimed the event.
                # The same durable gate now permits a fresh task decision.
                mcp_tools.dispatch(store, channel, "decide", dict(
                    assignment_ref="task", decision="accept", rationale="Event delivered.",
                    request_key="wire-after-delivery",
                ))
            original_callback = kwargs["on_tool_call"]

            def observed(*args):
                nonlocal seeded
                if not seeded and args[2] == title:
                    seeded = True
                    delegation_events.append_event(
                        store, task_id, "progress", "assignment-accept-followup:wire",
                        dict(kind="assignment_accept_requires_task", assignment_ref="wire"),
                        origin="runtime", assignment_ref="wire",
                    )
                    with pytest.raises(StoreError, match="acceptance_refused"):
                        mcp_tools.dispatch(store, channel, "decide", dict(
                            assignment_ref="task", decision="accept", rationale="Too early.",
                            request_key="wire-before-delivery",
                        ))
                return original_callback(*args)

            kwargs["on_tool_call"] = observed
            return original_prompt(_session_ref, message, cancel_event, **kwargs)

        adapter.prompt = prompt_with_real_event
        assert runtime.start_prompt(task_id, "s1", "Just check in.", lambda _text, **_kw: True,
                                    threading.Event()) == StopReason.END_TURN
        assert calls == 2
        prompts = [json.loads(line) for line in prompt_log.read_text(encoding="utf-8").splitlines()]
        assert len(prompts) == 2
        assert "Assignment wire was accepted as a worker result" in prompts[1]["prompt"][0]["text"]
        with store.connection() as db:
            assert db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()[0] == "accepted"
            usage = db.execute(
                "SELECT stop_reason FROM usage_records WHERE task_id=? ORDER BY created_at", (task_id,),
            ).fetchall()
            assert [row["stop_reason"] for row in usage] == ["cancelled", "end_turn"]
            evidence = db.execute(
                "SELECT status FROM tool_result_observations WHERE task_id=? ORDER BY id", (task_id,),
            ).fetchall()
            assert [row["status"] for row in evidence] == ["pending", "failed"] * 2
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_turn_boundary_requires_exact_cortex_decide_identity():
    assert _is_cortex_decide_tool("mcp.cortex.decide")
    assert _is_cortex_decide_tool("mcp__cortex__decide")
    assert not _is_cortex_decide_tool("mcp__other__decide")
    assert not _is_cortex_decide_tool("decide")
    assert not _is_cortex_decide_tool(None)


def test_accepted_final_withholds_sha256_not_in_published_pipeline(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    false_digest = "b" * 64
    verified_digest = "a" * 64
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[dict(
            sessionUpdate="agent_message_chunk",
            content=dict(type="text", text="SHA-256: " + false_digest),
        )],
        prompt_stop_reason="end_turn",
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ)
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0)
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0)
    try:
        channel, _session_ref = runtime._ensure_coordinator(task_id)
        _publish_current_pipeline(store, channel, "\nVerified SHA-256: " + verified_digest + "\n")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Verified.", request_key="acc-digest"))
        updates = []
        assert runtime.start_prompt(
            task_id, "s1", "Just check in.", updates.append, threading.Event()) == StopReason.END_TURN
        visible = "".join(updates)
        assert false_digest not in visible
        assert "[unverified digest omitted]" in visible
        assert "published pipeline" in visible
        with store.connection() as db:
            state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()[0]
        assert state == "accepted"
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_accepted_streamed_verified_sha256_is_exactly_preserved(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    verified_digest = "1" * 64
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Verified ")),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="SHA-256: " + verified_digest + ".")),
        ],
        prompt_stop_reason="end_turn",
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ)
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0)
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0)
    try:
        channel, _session_ref = runtime._ensure_coordinator(task_id)
        _publish_current_pipeline(store, channel, "\nVerified SHA-256: " + verified_digest + "\n")
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Verified.", request_key="acc-streamed-digest"))
        updates = []
        assert runtime.start_prompt(
            task_id, "s1", "Just check in.", updates.append, threading.Event()) == StopReason.END_TURN
        visible = "".join(updates)
        assert "SHA-256: " + verified_digest + "." in visible
        assert visible.count(verified_digest) == 1
        with store.connection() as db:
            assert db.execute(
                "SELECT state FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()[0] == "accepted"
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_accepted_final_keeps_exact_verified_sha256_and_redacts_credentials():
    verified = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    typo = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5388f7ace2efcde9"
    pipeline = "Verified SHA-256: " + verified + "\n\n---\n\nOlder edition: " + typo
    final = "Hash: " + verified + "\nTypo: " + typo + "\nBearer sk-secret1234567890"

    shown = _scrub_accepted_final(final, pipeline, "Digest did not match the published pipeline.")

    assert verified in shown
    assert typo not in shown
    assert "[unverified digest omitted]" in shown
    assert "sk-secret1234567890" not in shown
    assert "Digest did not match" in shown


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
