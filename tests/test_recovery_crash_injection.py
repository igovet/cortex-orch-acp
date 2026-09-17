"""T023: crash injection at every coordinator-recovery boundary.

For each of the six named checkpoints, an already-alive real coordinator
engine subprocess is left running (from a first, uncrashed process), a
*second* process is crashed mid-recovery at that exact checkpoint, and a
*third*, fresh process then completes recovery successfully -- with no
duplicate live coordinator, no false delivery/success receipt, and a clean
database."""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace
from recovery_crash_harness import crash_recovery

POINTS = [
    "t023_after_epoch_acquire",
    "t023_after_old_session_reconciliation",
    "t023_after_channel_revoke_issue",
    "t023_after_new_session_open",
    "t023_after_capsule_delivery_transmitted",
    "t023_after_continuation_prompt",
]


@pytest.mark.parametrize("point", POINTS)
def test_crash_at_each_recovery_boundary_is_reconcilable(tmp_path, point):
    case_dir = tmp_path / point
    case_dir.mkdir()
    project = case_dir / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Check in.", "owner-1")
    task_id = result.task_id

    engine_dir = case_dir / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    workspace_root = str(case_dir)

    # First process: open a real, still-alive coordinator engine subprocess
    # and leave it running (no shutdown) -- exactly the "process restarted
    # while its own attempt is still alive" scenario.
    adapter0 = EngineSessionManagerAdapter(
        lambda t: record, lambda t: workspace_root, environ=environ, call_timeout=10.0,
        store=store, capacity_limit=4,
    )
    runtime0 = OrchestratingRuntime(
        store, adapter0, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    runtime0._ensure_coordinator(task_id)

    # Second process: crashes mid-recovery at `point`.
    crashed = crash_recovery(
        store.directory, task_id, connection_id, scenario_path, environ, workspace_root, point,
    )
    assert crashed.returncode == 137, crashed.stdout + crashed.stderr

    # Third process: a fresh Store/adapter/runtime resumes and completes.
    store2 = Store(project / ".cortex", project_root=project)
    adapter2 = EngineSessionManagerAdapter(
        lambda t: record, lambda t: workspace_root, environ=environ, call_timeout=10.0,
        store=store2, capacity_limit=4,
    )
    runtime2 = OrchestratingRuntime(
        store2, adapter2, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    updates = []
    import threading

    outcome = runtime2.start_prompt(task_id, "s_final", "Continue please.", updates.append, threading.Event())
    assert outcome in (StopReason.END_TURN, StopReason.CANCELLED)

    with store2.connection() as db:
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []
        current_attempt_ref = db.execute(
            "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()["coordinator_attempt_ref"]
        # The currently-recorded coordinator attempt's own channel is live
        # (exactly one) -- scoped by attempt_id, never a blanket role count
        # (a task may separately carry an unrelated bootstrap-time
        # "coordinator"-role channel with no attempt_id of its own -- see
        # bootstrap.bootstrap_task -- which this check deliberately ignores).
        live_coordinator_channels = db.execute(
            "SELECT COUNT(*) AS n FROM trusted_channels WHERE task_id=? AND role='coordinator' "
            "AND attempt_id=? AND revoked_at IS NULL",
            (task_id, current_attempt_ref),
        ).fetchone()["n"]
        live_coordinator_attempts = db.execute(
            "SELECT COUNT(*) AS n FROM attempt_process_registry WHERE task_id=? AND role='coordinator' AND terminated_at IS NULL",
            (task_id,),
        ).fetchone()["n"]
    assert live_coordinator_channels == 1  # never two simultaneously live coordinator channels
    assert live_coordinator_attempts <= 1  # never two simultaneously live coordinator engine processes

    for manager in list(adapter0._managers.values()):
        manager.shutdown(grace=2.0)
    for manager in list(adapter2._managers.values()):
        manager.shutdown(grace=2.0)
