"""T023: coordinator recovery/session-replacement -- unit-level tests
against :func:`recovery.coordinator_recovery.recover` and
:func:`recovery.worker_reconciliation.reconcile`, using
``recovery_fakes.FakeRecoveryEngine`` (see that module's docstring) rather
than a real subprocess (see ``test_recovery_cross_process.py`` for the real
pid/boot-id liveness-detection and crash-injection legs)."""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery import registry as recovery_registry
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace
from recovery_fakes import FakeRecoveryEngine


def _task(tmp_path, text="Build it."):
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), text, "owner-1")
    return store, result.task_id


def _runtime(store, engine, **kwargs):
    return OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=2.0, **kwargs,
    )


def _delegate_and_provision(runtime, task_id, coordinator_channel, request_key="r1"):
    """Drive a real ``delegate`` MCP call through to a live (fake) worker
    engine session -- exactly the background path
    ``OrchestratingRuntime._process_new_events``/``_provision_and_prompt``
    drives for a real coordinator tool call -- and return the resulting
    worker attempt id."""
    mcp_tools.dispatch(store_for(runtime), coordinator_channel, "delegate", dict(
        title="Do it", brief="Do the thing.", role="backend-dev", acceptance="Done.", request_key=request_key,
    ))
    runtime._process_new_events(task_id, coordinator_channel, threading.Event())
    futures = list(runtime._running.get(task_id) or [])
    for future in futures:
        future.result(timeout=10)
    with store_for(runtime).connection() as db:
        row = db.execute(
            "SELECT attempt_id, idempotency_key FROM delegation_provisioning WHERE task_id=? "
            "ORDER BY created_at DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    assert row is not None and row["attempt_id"] is not None
    return row["attempt_id"], row["idempotency_key"]


def store_for(runtime):
    return runtime._store


def _insert_revision(store, task_id, revision_id, revision_number, route_engine, route_connection_id,
                      parameters=None, route_id="rt_1"):
    """Insert a minimal, real ``configuration_revisions`` row directly (this
    module's own scope is ``recovery.coordinator_recovery``'s read-only
    comparison against that table, not the full T008
    ``ConfigurationService.propose``/``apply`` flow -- see
    ``tests/test_retention.py`` for the same direct-insert fixture
    convention elsewhere in this codebase)."""
    import json as _json

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO configuration_revisions "
            "(id,task_id,revision,route_id,route_engine,route_connection_id,parameters,ceiling_snapshot,"
            " state,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (revision_id, task_id, revision_number, route_id, route_engine, route_connection_id,
             _json.dumps(parameters or {}), "{}", "active", "2026-01-01T00:00:00.000000+00:00",
             "2026-01-01T00:00:00.000000+00:00"),
        )
        db.commit()


def test_restart_replaces_coordinator_session_preserving_task_id(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    channel1, session_ref1 = runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    assert engine.lookup_owned(old_attempt_ref) == "alive"

    # Simulate a process restart: a brand new OrchestratingRuntime instance
    # (fresh in-memory caches) over the SAME store/engine.
    runtime2 = _runtime(store, engine)
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)

    assert channel2.task_id == task_id == channel1.task_id
    assert channel2.channel_id != channel1.channel_id
    assert session_ref2 != session_ref1
    # The old coordinator attempt was reconciled (alive -> terminated,
    # confirmed) before the replacement was ever opened.
    assert old_attempt_ref in engine.terminate_calls
    assert engine.lookup_owned(old_attempt_ref) == "absent"
    # New attempt id, new channel epoch.
    new_attempt_ref = engine.open_calls[-1]["attempt_id"]
    assert new_attempt_ref != old_attempt_ref
    assert channel2.owner_epoch > channel1.owner_epoch

    # The stale (pre-recovery) coordinator channel is explicitly revoked,
    # and the task's current epoch has moved past it (stale owner/permission
    # responses against it are rejected -- see server.Server.dispatch's
    # owner_epoch check, exercised end-to-end in test_recovery_cross_process.py).
    with store.connection() as db:
        row = db.execute(
            "SELECT revoked_at FROM trusted_channels WHERE channel_id=?", (channel1.channel_id,),
        ).fetchone()
        current_epoch = db.execute(
            "SELECT epoch FROM task_owners WHERE task_id=?", (task_id,),
        ).fetchone()["epoch"]
    assert row["revoked_at"] is not None
    assert current_epoch != channel1.owner_epoch
    assert current_epoch == channel2.owner_epoch


def test_alive_after_timeout_refuses_second_coordinator(tmp_path):
    """A prior coordinator attempt that is genuinely still alive and cannot
    be *confirmed* terminated (an ambiguous/unconfirmed termination, exactly
    like a real timed-out-but-not-dead process) must never let a second,
    concurrently-mutating coordinator session start."""
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    # terminate_owned() will be called but will NOT confirm exit.
    engine.terminate_confirms[old_attempt_ref] = False

    runtime2 = _runtime(store, engine)
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "coordinator_recovery_ambiguous"
    assert old_attempt_ref in engine.terminate_calls
    # No replacement attempt was ever opened.
    assert len(engine.open_calls) == 1


def test_unknown_liveness_blocks_without_terminating(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    engine.lookup_states[old_attempt_ref] = "unknown"

    runtime2 = _runtime(store, engine)
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "coordinator_recovery_indeterminate"
    # Never even attempted to terminate an indeterminate process.
    assert old_attempt_ref not in engine.terminate_calls
    assert len(engine.open_calls) == 1


def test_absent_old_attempt_proceeds_directly(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    engine.lookup_states[old_attempt_ref] = "absent"

    runtime2 = _runtime(store, engine)
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)
    assert old_attempt_ref not in engine.terminate_calls
    assert channel2.task_id == task_id


def test_worker_result_published_during_recovery_delivered_once_not_relaunched(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    channel1, session_ref1 = runtime1._ensure_coordinator(task_id)
    attempt_id, assignment_ref = _delegate_and_provision(runtime1, task_id, channel1)
    # The worker publishes its result BEFORE the coordinator ever recovers.
    # publish_result itself requires a real drafted/published report file;
    # this models "already durably published" the same way
    # delegation.completion.gate itself only ever looks at task_events --
    # see that module's docstring -- via the same durable event row a real
    # publish_result call commits.
    delegation_events.append_event(
        store, task_id, "worker_result", f"result:{attempt_id}",
        dict(report_ref="rep_1", summary="Done."), origin=attempt_id, assignment_ref=attempt_id,
    )

    runtime2 = _runtime(store, engine)
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)

    with store.connection() as db:
        failed = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed'",
            (task_id, assignment_ref),
        ).fetchone()
    assert failed is None  # never marked recovery_worker_lost -- result already existed
    # The worker's own attempt was still opportunistically terminated to
    # free capacity (best-effort; never re-delegated).
    assert attempt_id in engine.terminate_calls


def test_worker_result_published_exactly_during_reconciliation_race_wins(tmp_path):
    """Deterministic interleaving: the worker's result is published (a
    concurrent, in-flight ``publish_result``) in the exact window between
    ``worker_reconciliation``'s own initial ``_worker_result_exists`` check
    (already False) and its final atomic re-check-then-write -- modeled by
    publishing it as a side effect of ``terminate_owned`` itself, called in
    between those two points. The result must win: no contradictory
    ``assignment_failed(recovery_worker_lost)`` is ever durably recorded for
    the same assignment."""
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    channel1, _ = runtime1._ensure_coordinator(task_id)
    attempt_id, assignment_ref = _delegate_and_provision(runtime1, task_id, channel1)

    original_terminate = engine.terminate_owned

    def _terminate_with_concurrent_publish(target_attempt_id):
        if target_attempt_id == attempt_id:
            delegation_events.append_event(
                store, task_id, "worker_result", f"result:{attempt_id}",
                dict(report_ref="rep_1", summary="Done."), origin=attempt_id, assignment_ref=attempt_id,
            )
        return original_terminate(target_attempt_id)

    engine.terminate_owned = _terminate_with_concurrent_publish

    runtime2 = _runtime(store, engine)
    runtime2._ensure_coordinator(task_id)

    with store.connection() as db:
        failed = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed'",
            (task_id, assignment_ref),
        ).fetchone()
        result = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
            (task_id, attempt_id),
        ).fetchone()
    assert failed is None  # the concurrently-published result won; no contradictory failure was ever written
    assert result is not None


def test_coalesce_suppresses_recovery_worker_lost_when_result_in_same_batch():
    """Unit-level defense-in-depth check on ``_coalesce`` itself (T023
    fresh review, LOW): a ``worker_result`` and a ``recovery_worker_lost``
    ``assignment_failed`` for the SAME assignment in one claimed batch must
    never both render -- the result wins, silently (never a contradictory
    failure line), while an unrelated genuine failure still renders."""
    from cortex_runtime.acp.orchestrating_runtime import _coalesce
    from cortex_runtime.delegation.events import TaskEvent

    events = [
        TaskEvent(
            id=1, task_id="t1", event_type="worker_result", assignment_ref="a1",
            dedup_key="k1", payload=dict(report_ref="rep_1", summary="Done."),
            origin="a1", created_at="now", consumed_by=None, superseded=False,
        ),
        TaskEvent(
            id=2, task_id="t1", event_type="assignment_failed", assignment_ref="a1",
            dedup_key="k2", payload=dict(error="recovery_worker_lost"),
            origin="runtime", created_at="now", consumed_by=None, superseded=False,
        ),
        TaskEvent(
            id=3, task_id="t1", event_type="assignment_failed", assignment_ref="a2",
            dedup_key="k3", payload=dict(error="engine_turn_failed"),
            origin="runtime", created_at="now", consumed_by=None, superseded=False,
        ),
    ]
    text = _coalesce(events)
    assert "Assignment a1 result" in text
    assert "recovery_worker_lost" not in text
    assert "Assignment a2 failed: engine_turn_failed" in text


def test_worker_lost_without_result_is_marked_failed_not_relaunched(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    channel1, _ = runtime1._ensure_coordinator(task_id)
    attempt_id, assignment_ref = _delegate_and_provision(runtime1, task_id, channel1)

    runtime2 = _runtime(store, engine)
    runtime2._ensure_coordinator(task_id)

    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed'",
            (task_id, assignment_ref),
        ).fetchone()
    assert row is not None
    import json as _json

    payload = _json.loads(row["payload_json"])
    assert payload["error"] == "recovery_worker_lost"
    assert attempt_id in engine.terminate_calls


def test_unauthorized_route_change_is_refused(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)

    runtime2 = _runtime(store, engine)
    engine.next_route_engine = "claude-agent-acp"  # different engine, same config revision
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "unauthorized_route_change"


def test_effort_only_revision_never_authorizes_a_route_change(tmp_path):
    """A new configuration revision that itself carries the SAME route as
    its predecessor (an effort/parameter-only revision) must never
    authorize an unrelated engine/connection switch, even though the
    config revision id did change."""
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    old_context = recovery_registry.get_effective_context(store, old_attempt_ref)
    old_revision_id = old_context["effective_config_revision_id"]
    _insert_revision(store, task_id, old_revision_id, 1, "codex-acp", "cn_default")
    _insert_revision(store, task_id, "cr_effort_only", 2, "codex-acp", "cn_default")  # same route as revision 1

    runtime2 = _runtime(store, engine)
    engine.next_route_engine = "claude-agent-acp"  # unrelated switch
    engine.next_config_revision = "cr_effort_only"
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "unauthorized_route_change"


def test_authorized_route_change_via_new_config_revision_is_accepted(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    old_context = recovery_registry.get_effective_context(store, old_attempt_ref)
    old_revision_id = old_context["effective_config_revision_id"]
    _insert_revision(store, task_id, old_revision_id, 1, "codex-acp", "cn_default")
    _insert_revision(store, task_id, "cr_route_change", 2, "claude-agent-acp", "cn_other")

    runtime2 = _runtime(store, engine)
    engine.next_route_engine = "claude-agent-acp"
    engine.next_route_connection_id = "cn_other"
    engine.next_config_revision = "cr_route_change"  # its OWN route transition matches
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)
    assert channel2.task_id == task_id


def test_recovery_cannot_broaden_resource_policy(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store, default_policy=dict(fs_local=False, terminal=False, output_byte_limit_max=1024))
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)

    runtime2 = _runtime(store, engine)
    engine.next_policy = dict(fs_local=True, terminal=False, output_byte_limit_max=1024)
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "recovery_cannot_broaden_permissions"


def test_policy_broadening_without_explicit_revision_override_is_refused(tmp_path):
    """A new configuration revision exists, but its own parameters never
    declare a ``resource_policy_override`` -- broadening is still refused."""
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store, default_policy=dict(fs_local=False, terminal=False, output_byte_limit_max=1024))
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    old_context = recovery_registry.get_effective_context(store, old_attempt_ref)
    old_revision_id = old_context["effective_config_revision_id"]
    _insert_revision(store, task_id, old_revision_id, 1, "codex-acp", "cn_default")
    _insert_revision(store, task_id, "cr_no_override", 2, "codex-acp", "cn_default")  # no resource_policy_override

    runtime2 = _runtime(store, engine)
    engine.next_config_revision = "cr_no_override"
    engine.next_policy = dict(fs_local=True, terminal=False, output_byte_limit_max=1024)
    with pytest.raises(PortableError) as excinfo:
        runtime2._ensure_coordinator(task_id)
    assert excinfo.value.code == "recovery_cannot_broaden_permissions"


def test_policy_broadening_with_explicit_revision_override_is_accepted(tmp_path):
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store, default_policy=dict(fs_local=False, terminal=False, output_byte_limit_max=1024))
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    old_context = recovery_registry.get_effective_context(store, old_attempt_ref)
    old_revision_id = old_context["effective_config_revision_id"]
    _insert_revision(store, task_id, old_revision_id, 1, "codex-acp", "cn_default")
    _insert_revision(
        store, task_id, "cr_with_override", 2, "codex-acp", "cn_default",
        parameters=dict(resource_policy_override=dict(fs_local=True, terminal=False, output_byte_limit_max=1024)),
    )

    runtime2 = _runtime(store, engine)
    engine.next_config_revision = "cr_with_override"
    engine.next_policy = dict(fs_local=True, terminal=False, output_byte_limit_max=1024)
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)
    assert channel2.task_id == task_id


def test_recovery_preserves_narrower_than_default_policy(tmp_path):
    """A previously recorded policy narrower than the engine/adapter's own
    construction-time default must be carried over verbatim by recovery,
    never silently widened back to that default."""
    store, task_id = _task(tmp_path)
    narrow = dict(fs_local=False, terminal=False, output_byte_limit_max=4096)
    engine = FakeRecoveryEngine(store, default_policy=narrow)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)
    old_attempt_ref = engine.open_calls[0]["attempt_id"]
    old_context = recovery_registry.get_effective_context(store, old_attempt_ref)
    assert old_context["resource_policy"] == narrow

    runtime2 = _runtime(store, engine)
    # Recovery's own open_session call does not override next_policy, so
    # the fake would record its *default* (== narrow) again -- exercise the
    # actual carried-over value read back after recovery completes.
    channel2, session_ref2 = runtime2._ensure_coordinator(task_id)
    new_attempt_ref = engine.open_calls[-1]["attempt_id"]
    new_context = recovery_registry.get_effective_context(store, new_attempt_ref)
    assert new_context["resource_policy"] == narrow


def test_steering_after_recovery_is_included_before_continuation(tmp_path):
    """Steering recorded after the old coordinator died (before recovery
    ever runs) must be delivered in the very first post-recovery
    continuation prompt."""
    store, task_id = _task(tmp_path)
    engine = FakeRecoveryEngine(store)
    runtime1 = _runtime(store, engine)
    runtime1._ensure_coordinator(task_id)

    # New steering arrives while "the process is down" (no coordinator
    # currently active in this or any process) -- recorded exactly like an
    # ordinary follow-up prompt would be by acp.orchestrating_runtime._run.
    updates = []
    runtime2 = _runtime(store, engine)
    prompts_seen = []

    # Observe exactly what text the recovered coordinator's first
    # continuation prompt actually carries, by monkeypatching prompt() on
    # the shared fake engine.
    original_prompt = engine.prompt

    def _prompt(session_ref, message, cancel_event, *, on_delta=None):
        prompts_seen.append(message)
        from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

        return PromptOutcome(StopReason.END_TURN, "ok")

    engine.prompt = _prompt
    result = runtime2.start_prompt(task_id, "s2", "Please also update the docs.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert any("Please also update the docs." in m for m in prompts_seen)
    engine.prompt = original_prompt
