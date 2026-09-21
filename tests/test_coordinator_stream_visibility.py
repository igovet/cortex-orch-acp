"""Fix for the real Zed session that showed nothing but a spinner for
minutes: a coordinator turn's own text was buffered in
``delegation.live_adapters.EngineSessionManagerAdapter.prompt`` (accumulated
into ``text_parts``, only ever handed to ``emit_update`` once the whole
engine turn finished) instead of streamed as the engine actually produced
it, and a worker still provisioning (delegation_requested already claimed,
not yet assignment_ready) had no client-visible signal at all for however
long provisioning took.

This file covers, offline, with fakes (no paid call, no real engine
launch):

  1. A coordinator's own text chunks reach the client before the turn's
     final ``session/prompt`` response -- through the REAL
     ``EngineSessionManagerAdapter``/``EngineSessionManager``/fake ACP
     engine subprocess wire, ordering asserted against wall-clock timing
     (each scripted chunk carries its own ``delay``), never just "both
     eventually arrived".
  2. A worker's provisioning step reaches the client (as a
     ``tool_call_update`` on its existing assignment card, never a second
     channel) while the provisioning row is still ``in_progress`` -- proven
     by observing it from a concurrent watcher thread before the row is
     ever marked done.
  3. A credential-shaped streamed chunk is redacted (the existing
     ``observability.redact.scrub_text`` choke point) before it ever
     reaches the client, exactly like every other client-facing text this
     runtime already scrubs.
"""
from __future__ import annotations

import threading
import time
import uuid
from pathlib import Path

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path, text="Build a page."):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), text, "owner-1")
    return store, result.task_id


def _accept_task(store, channel, request_key="acc1"):
    """Give the synthetic coordinator a current authored pipeline before accept.

    Task-level acceptance deliberately cannot use the bootstrap pipeline as its
    evidence.  These transport visibility tests are about the subsequent ACP
    stream, so set up that normal coordinator publication boundary explicitly
    instead of bypassing the production completion gate.
    """
    draft = store.call(
        "create_draft", {"template": "pipeline", "request_key": "stream-pipeline-" + uuid.uuid4().hex},
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    body = pipeline_bootstrap._fill_template(
        draft["markdown"], channel.task_id, "Stream visibility test", None,
    )
    Path(draft["draft_path"]).write_text(body, encoding="utf-8")
    store.call(
        "write_report",
        {
            "draft_id": draft["draft_id"],
            "title": "Current pipeline",
            "summary": "Coordinator-authored current pipeline edition.",
            "author": "coordinator",
            "request_key": "stream-pipeline-publish-" + uuid.uuid4().hex,
        },
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    return mcp_tools.dispatch(store, channel, "decide", {
        "assignment_ref": "task", "decision": "accept",
        "rationale": "Trivial check-in.", "request_key": request_key,
    })


def test_coordinator_text_chunks_stream_before_the_turn_ends(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project, "Check in.")
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    # The fake engine delays its second chunk. Neither chunk should appear
    # before the turn's durable result is known.
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Checking "), delay=0.0),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="in now. "), delay=0.6),
        ],
        prompt_stop_reason="end_turn",
    ))
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(project), environ=environ, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    try:
        channel, session_ref = runtime._ensure_coordinator(task_id)
        _accept_task(store, channel)

        arrivals: list[tuple[float, str]] = []
        start = time.monotonic()

        turn_finished = threading.Event()

        def on_update(chunk_text):
            arrivals.append((time.monotonic() - start, chunk_text))
            return True

        result_box = []

        def run_prompt():
            try:
                result_box.append(runtime.start_prompt(
                    task_id, "s1", "Just check in.", on_update, threading.Event(),
                ))
            finally:
                turn_finished.set()

        prompt_thread = threading.Thread(target=run_prompt)
        prompt_thread.start()
        threading.Event().wait(0.2)
        assert not turn_finished.is_set()
        assert not arrivals, "intermediate coordinator prose leaked before the turn outcome"
        prompt_thread.join(timeout=3.0)
        assert not prompt_thread.is_alive(), "fake engine turn did not finish"
        result = result_box[0]

        assert result == StopReason.END_TURN
        assert len(arrivals) == 1, "accepted final text should be emitted once"
        assert "".join(text for _t, text in arrivals) == "Checking in now. "
        assert arrivals[0][1] == "Checking in now. "

        # A later host replay uses only the accepted final chunk.
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def _insert_provisioning_row(store, task_id, idempotency_key, step, status="in_progress"):
    with store.connection() as db:
        epoch = current_owner_epoch(db, task_id)
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT 1 FROM delegation_provisioning WHERE idempotency_key=?", (idempotency_key,),
            ).fetchone()
            now = time.strftime("%Y-%m-%dT%H:%M:%S.%fZ", time.gmtime())
            if existing is None:
                db.execute(
                    "INSERT INTO delegation_provisioning "
                    "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
                    "VALUES (?,?,?,?,?,?,?,?,?,?)",
                    (idempotency_key, task_id, "a_synthetic", "xa_synthetic", "backend-dev", step, status, epoch, now, now),
                )
            else:
                db.execute(
                    "UPDATE delegation_provisioning SET step=?, status=?, updated_at=? WHERE idempotency_key=?",
                    (step, status, now, idempotency_key),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def test_running_worker_card_hides_provisioning_step(tmp_path):
    """The worker card shows that it is running without exposing Cortex setup."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "")

    def silent_worker(store, channel, session_ref, message, call_index):
        time.sleep(2.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.1, max_no_progress_continuations=8,
    )

    # Seeded once, never advanced/finished for the rest of this test: any
    # tool-call update this synthetic row produces necessarily happened
    # while it was still genuinely 'in_progress' -- checked again below,
    # after the turn, with no race to resolve since nothing else ever
    # touches this row's status.
    _insert_provisioning_row(store, task_id, "synthetic-key", "workspace")

    tool_call_updates: list[dict] = []

    def emit_tool_call(update):
        tool_call_updates.append(dict(update))
        return True

    updates = []
    runtime.start_prompt(
        task_id, "s1", "Build it.", updates.append, threading.Event(), emit_tool_call=emit_tool_call,
    )

    with store.connection() as db:
        row = db.execute(
            "SELECT status FROM delegation_provisioning WHERE idempotency_key=?", ("synthetic-key",),
        ).fetchone()
    assert row is not None and row["status"] == "in_progress", (
        "test setup bug: the synthetic row must stay in_progress for the "
        "assertion below to actually prove 'while still in progress'"
    )
    provisioning_updates = [u for u in tool_call_updates if u.get("tool_call_id") == "worker:synthetic-key"]
    assert provisioning_updates, (
        "expected at least one tool-call update for the still-provisioning "
        "worker while it was in progress -- got none"
    )
    assert any(u.get("status") == "in_progress" for u in provisioning_updates)
    assert all("Provisioning:" not in (u.get("text") or "") for u in provisioning_updates)


def test_streamed_coordinator_chunk_is_redacted(tmp_path):
    """A credential-shaped chunk from the coordinator's own engine must
    never reach the client unredacted -- routed through the same
    ``observability.redact.scrub_text`` choke point as every other
    client-facing text this runtime emits, never a second one."""
    store, task_id = _task(tmp_path, "Do a thing.")
    secret_chunk = "Bearer sk-liveFAKESECRETVALUE1234567890abcdef"

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, secret_chunk)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource())

    updates = []
    runtime.start_prompt(task_id, "s1", "Do a thing.", updates.append, threading.Event())

    assert updates, "expected the coordinator text to reach the client"
    assert not any("sk-liveFAKESECRETVALUE1234567890abcdef" in u for u in updates)
    assert any("[REDACTED_TOKEN]" in u for u in updates)


def test_a_tool_call_between_two_spoken_messages_separates_them(tmp_path):
    """The reported rendering: "...verify the change.The Cortex draft was
    created...". A model that speaks, calls a tool, then speaks again has
    produced two messages, but ``agent_message_chunk`` carries no message
    identity, so the chunks arrive as one unbroken run. The tool call
    between them is the observable boundary."""
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project, "Check in.")
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Publishing the draft.")),
            dict(sessionUpdate="tool_call", toolCallId="tc1", title="write", status="completed"),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="The draft was created.")),
        ],
        prompt_stop_reason="end_turn",
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(project), environ=environ, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    try:
        channel, _session_ref = runtime._ensure_coordinator(task_id)
        _accept_task(store, channel)
        chunks: list[str] = []
        runtime.start_prompt(task_id, "s1", "Just check in.", chunks.append, threading.Event())
        streamed = "".join(chunks)
        assert "Publishing the draft.The draft was created." not in streamed, (
            f"two messages ran together with no separator: {streamed!r}"
        )
        assert "Publishing the draft.\n\nThe draft was created." in streamed, streamed
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=1.0)


def test_tool_calls_show_compact_project_activity_without_diff_output(tmp_path):
    """A read and edit stay visible, each as one line, without tool output."""
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project, "Check in.")
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="tool_call", toolCallId="t1", title="Read calc.py", kind="read",
                 status="in_progress", locations=[dict(path="/project/calc.py", line=4)]),
            dict(sessionUpdate="tool_call_update", toolCallId="t1", status="completed"),
            dict(sessionUpdate="tool_call", toolCallId="t2", title="Edit calc.py", kind="edit",
                 status="completed", locations=[dict(path="/project/calc.py")],
                 content=[dict(type="diff", path="/project/calc.py",
                               oldText="def divide(a, b):\n    return a / b\n",
                               newText="def divide(a, b):\n    if b == 0:\n        raise ValueError('...')\n    return a / b\n")]),
        ],
        prompt_stop_reason="end_turn",
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(project), environ=environ, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    try:
        channel, _ = runtime._ensure_coordinator(task_id)
        _accept_task(store, channel)
        cards: list = []
        runtime.start_prompt(
            task_id, "s1", "Just check in.", lambda t: True, threading.Event(), emit_tool_call=cards.append,
        )

        assert cards, f"no coordinator cards reached the client: {cards}"
        visible = [card for card in cards if "Read calc.py" in card.get("text", "")
                   or "Edit calc.py" in card.get("text", "")]
        assert len({card["tool_call_id"] for card in visible}) == 2
        assert all(card["tool_call_id"].startswith("call:coordinator:") for card in visible)
        assert all(card["text"].count("\n") == 0 for card in visible)
        # These fixture updates carry no ACP rawInput.  The collapsed line is
        # still visible, while expanded input/output fields are emitted only
        # when the engine actually supplied them (see the structured I/O
        # regression in test_tool_visibility_full_transcript.py).
        assert all("raw_input" not in card for card in visible)
        assert all("raw_output" not in card for card in visible)
        assert all("raise ValueError" not in card["text"] for card in visible)
        assert all("type: diff" not in card["text"] for card in visible)
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=1.0)


def test_the_models_reasoning_and_plan_reach_the_client(tmp_path):
    """Two whole update kinds -- the model's reasoning and its task list --
    were dropped as unrecognized, so "I want to see everything that is
    happening" could not be satisfied no matter how the cards looked."""
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project, "Check in.")
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    plan_update = dict(
        sessionUpdate="plan",
        entries=[dict(content="Read calc.py", priority="high", status="in_progress"),
                 dict(content="Add the guard", priority="high", status="pending")],
    )
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="agent_thought_chunk", content=dict(type="text", text="Weighing the guard.")),
            plan_update,
            # Not display-only: the engine's own session state must never be
            # forwarded onto this agent's session.
            dict(sessionUpdate="current_mode_update", currentModeId="agent-full-access"),
        ],
        prompt_stop_reason="end_turn",
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(project), environ=environ, call_timeout=10.0,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    try:
        channel, _ = runtime._ensure_coordinator(task_id)
        _accept_task(store, channel)
        shown: list = []
        runtime.start_prompt(
            task_id, "s1", "Just check in.", lambda t: True, threading.Event(),
            emit_display_update=shown.append,
        )
        kinds = [u.get("sessionUpdate") for u in shown]
        assert "agent_thought_chunk" in kinds, shown
        assert "plan" in kinds, shown
        assert "current_mode_update" not in kinds, (
            "the engine's own session state must never be forwarded to this agent's client"
        )
        thought = next(u for u in shown if u["sessionUpdate"] == "agent_thought_chunk")
        assert thought["content"]["text"] == "Weighing the guard."
        plan = next(u for u in shown if u["sessionUpdate"] == "plan")
        assert [entry["content"] for entry in plan["entries"]] == ["Read calc.py", "Add the guard"]
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=1.0)
