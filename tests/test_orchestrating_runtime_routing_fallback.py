"""T027: the automatic fallback trigger wired into
``OrchestratingRuntime._provision_and_prompt`` -- against
``FakeSessionRuntime`` (no real engine subprocess; the real
``routing.authorization.authorize_fallback`` check runs against a real
``ConfigurationService`` revision, never scripted/stubbed authorization).
"""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import (
    CAPACITY_UNAVAILABLE,
    ENGINE_FAILURE,
    FallbackPolicy,
    PolicyCeiling,
    RouteProfile,
)
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import ConfigurationServiceSnapshotSource
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.routing import store as routing_store
from cortex_runtime.store import Store

from orchestration_fakes import FakeSessionRuntime, FakeWorkspace
from test_orchestrating_runtime import PromptOutcome, _assignment_title, _publish
from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed


class _FakeConfigAdapter:
    def configure(self, engine_session_id, revision):
        return "evidence-ref"


class _Conn:
    def __init__(self, id, engine, provider):
        self.id, self.engine, self.provider = id, engine, provider


ROLE_ID = resolve_role("backend-dev")


def _bootstrap(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner="owner-1")


def _connection_entry(connection_id, engine, model):
    from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir

    now = "2026-01-01T00:00:00+00:00"
    record = ConnectionRecord(
        id=connection_id, name="fake", engine=engine, provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )
    options = [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
                "options": [{"value": model, "name": model}]}]
    snapshot = build_catalog({connection_id: record}, {connection_id: parse_observation(options)})
    return snapshot.entries[0]


def _setup_two_route_task(tmp_path, *, cross_provider=False, ceiling_extra=None):
    store, task_id, epoch = _bootstrap(tmp_path)
    entry_a = _connection_entry("cn_" + "a" * 12, "codex-acp", "shared-model")
    entry_b = _connection_entry("cn_" + "b" * 12, "claude-agent-acp", "other-model")
    conn_a = _Conn(entry_a.route.connection_id, "codex-acp", "acme")
    conn_b = _Conn(entry_b.route.connection_id, "claude-agent-acp", "other-vendor" if cross_provider else "acme")

    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={entry_b.route.id},
            allowed_failure_classes={ENGINE_FAILURE, CAPACITY_UNAVAILABLE},
            data_recipient_consent=True,
        ),
        **(ceiling_extra or {}),
    )
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch, user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={ROLE_ID: {entry_a.route.id, entry_b.route.id}}),
        route_details={entry_b.route.id: entry_b.route},
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)
    connections = {conn_a.id: conn_a, conn_b.id: conn_b}
    return store, task_id, connections, entry_a, entry_b


def _register_fake_coordinator_channel(runtime, store, task_id):
    """Test-only: a real trusted coordinator channel so
    ``_attempt_worker_route_fallback``'s own worker-channel reissue
    (``issue_channel_tx(..., parent_channel_id=...)``) has a real parent to
    bind against, without running a full coordinator prompt loop first."""
    from cortex_runtime.trusted_channel import issue_channel_tx

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, _secret = issue_channel_tx(db, task_id, "coordinator", epoch, attempt_id="co_manual")
        db.commit()
    from cortex_runtime.trusted_channel import ChannelContext

    runtime._coordinator_channels[task_id] = ChannelContext(
        channel_id=channel_id, task_id=task_id, principal_thread_id="p_manual",
        parent_principal_thread_id=None, role="coordinator", owner_epoch=epoch,
        assignment_id=None, attempt_id="co_manual", workspace_binding_id=None, context_generation_id=None,
    )


def _make_runtime(store, router, connections, **kwargs):
    service = ConfigurationService(store)
    engine = FakeSessionRuntime(store, router, connections=connections)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), ConfigurationServiceSnapshotSource(service),
        allow_unqualified_delivery=True, wait_timeout_seconds=30.0, **kwargs,
    )
    return runtime, engine


def test_engine_failure_triggers_authorized_fallback_and_audits_both_attempts(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)
    decided = {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        if "A finished" in message and not decided["done"]:
            decided["done"] = True
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="Delivered.", request_key="acceptall"))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_failing_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("transport_eof")

    def worker_success_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return coordinator_behavior
        return worker_success_behavior if launch.get("route_override") is not None else worker_failing_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Build A.", updates.append, cancel_event)
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    assert len(engine.open_calls) == 3  # coordinator + failed worker + fallback worker
    assert len(engine.fallback_calls) == 1
    old_attempt_id, new_attempt_id, failure_class, target_route_id = engine.fallback_calls[0]
    assert failure_class == ENGINE_FAILURE
    assert target_route_id == entry_b.route.id
    audit = routing_store.list_audit_for_task(store, task_id)
    fallback_rows = [row for row in audit if row["decision"] == "fallback"]
    assert len(fallback_rows) == 1
    assert fallback_rows[0]["predecessor_attempt_id"] == old_attempt_id
    assert fallback_rows[0]["attempt_id"] == new_attempt_id
    assert fallback_rows[0]["connection_id"] == entry_b.route.connection_id
    assert fallback_rows[0]["engine"] == "claude-agent-acp"


def test_refusal_never_triggers_fallback_and_reports_explicitly(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_refuses(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.REFUSAL, "I will not do this.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_refuses

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build A.", lambda u: None, cancel_event)
    assert len(engine.fallback_calls) == 0
    assert len(engine.open_calls) == 2  # coordinator + the ONE refused worker; never a second
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    import json as _json
    assert _json.loads(row["payload_json"])["error"] == "refusal"


def test_cross_provider_fallback_without_consent_refused_before_any_open(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path, cross_provider=True)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_failing_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("transport_eof")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_failing_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build A.", lambda u: None, cancel_event)
    # Refused before any session was opened for connection b -- never any
    # data transfer.
    assert len(engine.open_calls) == 2  # coordinator + the one failed worker; no fallback open
    fallback_rows = [row for row in routing_store.list_audit_for_task(store, task_id) if row["decision"] == "fallback"]
    assert fallback_rows == []
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    import json as _json
    assert "fallback_refused:fallback_consent_required" in _json.loads(row["payload_json"])["error"]


def test_attempt_budget_exhausted_refuses_fallback(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(
        tmp_path, ceiling_extra=dict(limits={"attempts": 1}),
    )

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_failing_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("transport_eof")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_failing_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build A.", lambda u: None, cancel_event)
    assert len(engine.open_calls) == 2  # coordinator + the one failed worker only
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    import json as _json
    assert "fallback_refused:attempt_budget_exhausted" in _json.loads(row["payload_json"])["error"]


def test_running_worker_keeps_route_when_profile_changes_mid_task(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)
    opened = threading.Event()
    finish = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        if "A finished" in message:
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="Delivered.", request_key="acceptall"))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_success_behavior(store, channel, session_ref, message, call_index):
        opened.set()
        finish.wait(timeout=10.0)
        title = _assignment_title(store, channel)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_success_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    updates = []
    thread = threading.Thread(target=lambda: updates.append(runtime.start_prompt(task_id, "s1", "Build A.", lambda u: None, cancel_event)))
    thread.start()
    assert opened.wait(timeout=10.0)
    audit_before = routing_store.list_audit_for_task(store, task_id)
    running_attempt = next(row for row in audit_before if row["role"] == "worker")

    # Coordinator's route/profile changes mid-task (new revision selecting
    # route b outright) -- the already-running worker attempt above must
    # never be touched by this.
    service = ConfigurationService(store)
    with store.connection() as db:
        epoch = current_owner_epoch(db, task_id)
    from cortex_runtime.portable.identity import OwnershipEpoch as _Epoch

    service.propose(
        task_id, entry_b, ConfigurationInputs(route=entry_b.route, parameters={}),
        expected_revision=1, current_epoch=_Epoch(epoch=epoch, owner="owner-1"),
    )
    finish.set()
    thread.join(timeout=15.0)

    audit_after = routing_store.list_audit_for_task(store, task_id)
    same_attempt = next(row for row in audit_after if row["attempt_id"] == running_attempt["attempt_id"])
    assert same_attempt["connection_id"] == entry_a.route.connection_id  # unchanged
    assert same_attempt["config_revision_id"] == running_attempt["config_revision_id"]  # unchanged


def test_second_consecutive_failure_tries_a_different_route_then_refuses(tmp_path):
    """T027 review fix (MEDIUM): the first failure falls over a->b; the
    SECOND failure (on the fallback route b) must never re-select the
    already-tried route a -- with only two routes total, no untried
    candidate remains and fallback is refused explicitly."""
    store, task_id, epoch = _bootstrap(tmp_path)
    entry_a = _connection_entry("cn_" + "a" * 12, "codex-acp", "shared-model")
    entry_b = _connection_entry("cn_" + "b" * 12, "claude-agent-acp", "other-model")
    conn_a = _Conn(entry_a.route.connection_id, "codex-acp", "acme")
    conn_b = _Conn(entry_b.route.connection_id, "claude-agent-acp", "acme")
    connections = {conn_a.id: conn_a, conn_b.id: conn_b}

    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes=None, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=True,
        ),
    )
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch, user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={ROLE_ID: {entry_a.route.id, entry_b.route.id}}),
        route_details={entry_b.route.id: entry_b.route},
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_failing_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("transport_eof")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_failing_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build A.", lambda u: None, cancel_event)

    assert len(engine.open_calls) == 3  # coordinator + failed attempt on a + failed attempt on b
    assert len(engine.fallback_calls) == 1  # never a second fallback call/route
    fallback_rows = [row for row in routing_store.list_audit_for_task(store, task_id) if row["decision"] == "fallback"]
    assert len(fallback_rows) == 1
    assert fallback_rows[0]["connection_id"] == entry_b.route.connection_id
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    import json as _json
    assert "fallback_refused:no_untried_authorized_route" in _json.loads(row["payload_json"])["error"]


@pytest.mark.parametrize("failure_class", ["cancellation", "authorization_loss", "provider_restriction"])
def test_hard_denylisted_classes_never_trigger_fallback(tmp_path, failure_class):
    """T027 review: refusal is already covered by
    test_refusal_never_triggers_fallback_and_reports_explicitly; this
    covers the remaining three hard-denylisted failure classes against the
    real ``_attempt_worker_route_fallback`` production method (the only two
    current callers hardcode 'engine_failure'/'capacity_unavailable', but
    the method itself -- and its ``authorize_fallback`` gate -- must refuse
    any hard-denylisted class regardless of caller): no second open, and
    exactly one refusal event."""
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)
    runtime, engine = _make_runtime(store, lambda *a: None, connections)
    config_revision = ConfigurationService(store).current(task_id)["id"]
    _register_fake_coordinator_channel(runtime, store, task_id)

    new_session_ref, refused, _new_attempt_id = runtime._attempt_worker_route_fallback(
        task_id, "manual-assignment-ref", "asg1", ROLE_ID, "xa_manual_old",
        "shared_project_root", config_revision, failure_class, "instructions",
    )
    assert new_session_ref is None
    assert refused is True
    assert len(engine.open_calls) == 0  # never opened anything, including on this connection -- no session
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchall()
    assert len(rows) == 1
    import json as _json
    assert _json.loads(rows[0]["payload_json"])["error"] == "fallback_refused:fallback_not_authorized"
