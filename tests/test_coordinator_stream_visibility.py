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

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
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


def test_coordinator_text_chunks_stream_before_the_turn_ends(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project, "Check in.")
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    # Two chunks, each delayed, followed by the final stopReason response.
    # If streaming worked, each chunk's own arrival timestamp lands well
    # before the call returns; if it were still buffered (the pre-fix
    # bug), both would only ever be observed after this whole block.
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Checking"), delay=0.3),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=" in now."), delay=0.3),
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
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Trivial check-in.", request_key="acc1",
        ))

        arrivals: list[tuple[float, str]] = []
        start = time.monotonic()

        def on_update(chunk_text):
            arrivals.append((time.monotonic() - start, chunk_text))
            return True

        result = runtime.start_prompt(task_id, "s1", "Just check in.", on_update, threading.Event())
        finished_at = time.monotonic() - start

        assert result == StopReason.END_TURN
        assert [text for _t, text in arrivals] == ["Checking", " in now."]
        # First chunk arrived close to its own scripted 0.3s delay, not
        # bunched at the very end with the response.
        first_arrival, second_arrival = arrivals[0][0], arrivals[1][0]
        assert first_arrival < finished_at - 0.2, (
            f"first chunk arrived at {first_arrival:.3f}s but the turn only "
            f"finished at {finished_at:.3f}s -- streaming did not happen "
            "before the turn ended"
        )
        assert first_arrival < second_arrival < finished_at + 0.05
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


def test_provisioning_step_reaches_the_client_while_still_in_progress(tmp_path):
    """T035 visibility fix: ``_emit_provisioning_tool_calls`` must surface a
    still-``in_progress`` provisioning row's current step on the client
    *while it is still running*, not only after it eventually finishes --
    otherwise this is no better than the pre-fix silence the real Zed
    session hit."""
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
    assert any("Provisioning: workspace" in (u.get("text") or "") for u in provisioning_updates)


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

    assert updates, "expected the coordinator's own turn text to have been streamed"
    assert not any("sk-liveFAKESECRETVALUE1234567890abcdef" in u for u in updates)
    assert any("[REDACTED_TOKEN]" in u for u in updates)
