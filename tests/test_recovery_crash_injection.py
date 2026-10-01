"""T023: crash injection at every coordinator-recovery boundary.

For each of the six named checkpoints, an already-alive real coordinator
engine subprocess is left running (from a first, uncrashed process), a
*second* process is crashed mid-recovery at that exact checkpoint, and a
*third*, fresh process then completes recovery successfully -- with no
duplicate live coordinator, no false delivery/success receipt, and a clean
database."""
from __future__ import annotations

import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter, ConfigurationServiceSnapshotSource
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.recovery.worker_reconciliation import unknown_outcome_actions, unknown_outcome_request
from cortex_runtime.trusted_channel import issue_channel, revoke_channel
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeWorkspace
from model_policy_fixtures import write_policy
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
    scenario = default_scenario()
    for option in scenario["config_options"]:
        option.update(name=option["id"], type="select", category=option["id"])
    scenario_path = write_scenario(engine_dir, scenario)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    workspace_root = str(case_dir)
    # Persist the exact settings/catalog/revision used by all three processes.
    # The real adapter confirms this selected revision on the first open.
    write_policy(environ, {"codex-acp": {"gpt-x": "standard"}}, automatic={"codex-acp": ["gpt-x"]})
    with connection_registry.transaction(environ) as state:
        state.connections[record.id] = record
    snapshot = build_catalog({record.id: record}, {record.id: parse_observation(scenario["config_options"])})
    catalog_store.save_snapshot(snapshot, environ)
    entry = snapshot.entries[0]
    config_service = ConfigurationService(store)
    with store.connection() as db:
        epoch = OwnershipEpoch(epoch=current_owner_epoch(db, task_id), owner="owner-1")
    selected = config_service.propose(
        task_id, entry, ConfigurationInputs(route=entry.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    assert selected["state"] == "selected"

    # First process: open a real, still-alive coordinator engine subprocess
    # and leave it running (no shutdown) -- exactly the "process restarted
    # while its own attempt is still alive" scenario.
    adapter0 = EngineSessionManagerAdapter(
        lambda t: record, lambda t: workspace_root, environ=environ, call_timeout=10.0,
        store=store, capacity_limit=4,
    )
    runtime0 = OrchestratingRuntime(
        store, adapter0, FakeWorkspace(), ConfigurationServiceSnapshotSource(config_service),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    adapter2 = None
    try:
        runtime0._ensure_coordinator(task_id)
        assert config_service.current_or_latest(task_id)["state"] == "active"
        assert capacity.count_active(store, record.id) == 1
        # A lost worker's dispatched, unretained mutation must survive every
        # recovery as unknown, never be converted to success or replayed.
        worker_channel, _ = issue_channel(
            store, task_id, "worker", epoch.epoch,
            parent_channel_id=result.channel_id, attempt_id="xa_" + "f" * 12,
        )
        revoke_channel(store, worker_channel)
        pending_receipt = json.dumps(dict(complete=False, tool_call_ref="gateway:pending-crash-action"))
        with store.connection() as db:
            db.execute("INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)",
                       (task_id, "pending-crash-action", worker_channel, "project_write", "fixture-digest", pending_receipt))

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
            store2, adapter2, FakeWorkspace(), ConfigurationServiceSnapshotSource(ConfigurationService(store2)),
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
            actions = unknown_outcome_actions(db, task_id, revoked_only=True)
            assert actions == [dict(operation="project_write", tool_call_ref="gateway:pending-crash-action",
                                    attempt_id="xa_" + "f" * 12)]
            assert "will not re-run" in unknown_outcome_request(db, task_id)
            assert db.execute("SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key=?",
                              (task_id, "pending-crash-action")).fetchone()["result_json"] == pending_receipt
        assert capacity.count_active(store2, record.id) == 1
        assert live_coordinator_channels == 1  # never two simultaneously live coordinator channels
        assert live_coordinator_attempts <= 1  # never two simultaneously live coordinator engine processes
    finally:
        adapters = [adapter0] + ([adapter2] if adapter2 is not None else [])
        for adapter in adapters:
            for attempt_id in list(adapter._managers):
                adapter.terminate_owned(attempt_id)
        # Also reconcile a child process left at a post-open checkpoint.
        with store.connection() as db:
            attempts = db.execute(
                "SELECT attempt_id FROM attempt_process_registry WHERE task_id=? AND terminated_at IS NULL",
                (task_id,),
            ).fetchall()
        for row in attempts:
            adapter0.terminate_owned(row["attempt_id"])
    assert capacity.count_active(store, record.id) == 0
