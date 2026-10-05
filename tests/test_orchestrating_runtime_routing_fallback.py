"""T027: the automatic fallback trigger wired into
``OrchestratingRuntime._provision_and_prompt`` -- against
``FakeSessionRuntime`` (no real engine subprocess; the real
``routing.authorization.authorize_fallback`` check runs against a real
``ConfigurationService`` revision, never scripted/stubbed authorization).
"""
from __future__ import annotations

import threading
import uuid
from pathlib import Path

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
from cortex_runtime.delegation import events as delegation_events, mcp_tools
from cortex_runtime.delegation.work_graph import WorkGraph
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.delegation.live_adapters import ConfigurationServiceSnapshotSource
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.project_docs import build_catalog as build_project_docs_catalog
from cortex_runtime.routing import store as routing_store
from cortex_runtime.store import Store

from orchestration_fakes import FakeSessionRuntime, FakeWorkspace
from test_orchestrating_runtime import PromptOutcome, _assignment_title, _publish
from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
from runtime_scenarios import publish_result

def _publish(store,channel,*args):
    return publish_result(store,channel)



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
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)), ConfigurationServiceSnapshotSource(service),
        allow_unqualified_delivery=True, wait_timeout_seconds=30.0, **kwargs,
    )
    runtime._cleanup_isolated_workspace_if_integrated = lambda task_id, attempt_id: None
    return runtime, engine


def _publish_current_pipeline(store, channel):
    """Publish a coordinator-authored current edition before accepting."""
    draft = store.call(
        "create_draft", {"template": "pipeline", "request_key": "pipeline-current"},
        channel.principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    body = path.read_text(encoding="utf-8")
    values = {
        "{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}": "Objective: build the page. Decision basis: delegated work is reviewable.",
        "{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}": "Requirements: delegated work is complete and reviewable.",
        "{{CURRENT_WORK_GRAPH}}": "The delegated assignment has completed.",
        "{{CURRENT_ASSIGNMENTS_AND_ROUTING}}": "Coordinator reviewed the backend-dev assignment.",
        "{{CURRENT_EVIDENCE_AND_VERIFICATION}}": "The worker report is published and available for review.",
        "{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}": "Decision: accept the completed work.",
    }
    for marker, value in values.items():
        body = body.replace(marker, value)
    for marker in draft["replaceable_markers"]:
        if marker.startswith("<!--"):
            body = body.replace(marker, "Evidence links: the worker report is the selected result.")
    path.write_text(body, encoding="utf-8")
    return store.call(
        "write_report", {
            "title": "Current pipeline", "summary": "Coordinator current plan.",
            "author": "coordinator", "draft_id": draft["draft_id"], "request_key": "pipeline-current-publish",
        }, channel.principal_thread_id, trusted=True,
    )


def test_engine_failure_triggers_authorized_fallback_and_audits_both_attempts(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)
    decided = {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

    def worker_failing_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("transport_eof")

    def worker_success_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_implementing_behavior(store, channel, session_ref, message, call_index):
        # The fallback attempt builds A, so the runtime schedules review.
        (Path(store.project_root) / "a.py").write_text("A = True\n", encoding="utf-8")
        return worker_success_behavior(store, channel, session_ref, message, call_index)

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return coordinator_behavior
        if launch.get('role_id') == 'worker-code-reviewer': return worker_success_behavior
        return worker_implementing_behavior if launch.get("route_override") is not None else worker_failing_behavior

    runtime, engine = _make_runtime(store, router, connections)
    cancel_event = threading.Event()
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Build A.", updates.append, cancel_event)
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    assert len(engine.open_calls) == 4  # coordinator, failed worker, fallback, independent review
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


def test_fallback_worker_prompt_rebuilds_project_docs_catalog(tmp_path):
    index = tmp_path / "docs/project/index.md"
    index.parent.mkdir(parents=True)
    index.write_text("# Before fallback\n", encoding="utf-8")
    store, task_id, connections, _entry_a, _entry_b = _setup_two_route_task(tmp_path)
    prompts = []
    versions = [build_project_docs_catalog(tmp_path)["catalog_version"]]
    decided = {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

    def failed_worker(store, channel, session_ref, message, call_index):
        prompts.append(message)
        index.write_text("# After fallback\n", encoding="utf-8")
        versions.append(build_project_docs_catalog(tmp_path)["catalog_version"])
        raise EngineTurnFailed("transport_eof")

    def fallback_worker(store, channel, session_ref, message, call_index):
        prompts.append(message)
        _publish(store, channel, _assignment_title(store, channel), call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        if channel.role == "coordinator":
            return coordinator_behavior
        if launch.get('role_id') == 'worker-code-reviewer': return fallback_worker
        return fallback_worker if launch.get("route_override") is not None else failed_worker

    runtime, engine = _make_runtime(store, router, connections)
    try:
        assert runtime.start_prompt(task_id, "s1", "Build A.", lambda update: None, threading.Event()) == StopReason.END_TURN
        assert len(prompts) == 3
        assert versions[0] != versions[1]
        assert versions[0] in prompts[0] and versions[1] not in prompts[0]
        assert versions[1] in prompts[1] and versions[0] not in prompts[1]
        assert "Before fallback" in prompts[0] and "After fallback" in prompts[1]
        assert all("Acceptance:" in prompt for prompt in prompts)
        assert len(engine.fallback_calls) == 1
    finally:
        runtime.shutdown()


def test_refusal_never_triggers_fallback_and_reports_explicitly(tmp_path):
    store, task_id, connections, entry_a, entry_b = _setup_two_route_task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

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
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

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
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

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
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

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
        from runtime_scenarios import propose_inspection
        context=mcp_tools.dispatch(store,channel,'task_get_context',{})
        if context['state']=='PLANNING':
            mcp_tools.dispatch(store,channel,'plan_submit',dict(plan=dict(tasks=[dict(id='A',role='backend-dev',objective='A',dependencies=[],scope=['.'],model='auto')],verification=[['true']]),request_key='plan'))
        else:
            mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified.'))
        return PromptOutcome(StopReason.END_TURN,'Visible coordinator text')

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


def test_a_session_closing_after_a_published_result_is_not_a_failure(tmp_path):
    """Live: a worker published its result at 02:04:39, its engine session
    then closed, and the runtime read that close as an engine failure and
    recorded `fallback_refused` for the assignment one second later -- a
    failure event for an assignment that had just succeeded.

    Recovery has re-checked for exactly this since T023; the engine-failure
    path never did."""
    import uuid as _uuid

    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.delegation import events as delegation_events
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store

    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    task_id = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(_uuid.uuid4())), "Do X.", "owner-1",
    ).task_id
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store

    assignment_ref = "a" * 64
    assert not runtime._assignment_already_reported(task_id, assignment_ref)

    # The worker reports through its own channel, so the result carries the
    # attempt's ref, not the assignment's.
    now = "2026-09-19T00:00:00+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, "xa_1", "worker-backend-dev", "ready", "ready", 1, now, now),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "worker_result", "worker_result:xa_1",
        dict(summary="done", report_ref="r_1"), origin="worker", assignment_ref="xa_1",
    )

    assert runtime._assignment_already_reported(task_id, assignment_ref)


def test_late_engine_failure_does_not_sync_reported_worker_as_failed(tmp_path):
    """A worker result may commit just before its engine reports EOF.

    The result path owns the graph transition to ``reported``. The later
    cleanup path must leave that state intact and must not emit a
    ``work_attempt_sync_failed`` diagnostic.
    """
    store, task_id, _epoch = _bootstrap(tmp_path)
    graph = WorkGraph(store)
    assignment = graph.add_assignment(task_id, "implementation", idempotency_key="graph-assignment")
    attempt = graph.create_attempt(assignment["assignment_id"], idempotency_key="graph-attempt")
    graph.transition_attempt(attempt["attempt_id"], "running", idempotency_key="graph-running")
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    assignment_ref = "a" * 64
    now = "2026-09-19T00:00:00+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, "worker-attempt-1", "worker-backend-dev", "ready", "ready", 1, now, now),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegation-requested:late-eof",
        {"work_attempt_ref": attempt["attempt_id"]}, origin="runtime", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "worker-result:late-eof",
        {"summary": "done", "report_ref": "report-1"}, origin="worker", assignment_ref="worker-attempt-1",
    )
    graph.transition_attempt(attempt["attempt_id"], "reported", idempotency_key="graph-reported")

    runtime._mark_work_attempt_failed(task_id, assignment_ref, "engine_failure:transport_eof", "late-eof-sync")
    assert runtime._append_assignment_failure_if_unreported(
        task_id, assignment_ref,
        {"error": "engine_failure:transport_eof", "kind": "transport_eof"},
    ) is False

    with store.connection() as db:
        status = db.execute(
            "SELECT status FROM work_assignment_attempts WHERE attempt_id=?", (attempt["attempt_id"],),
        ).fetchone()["status"]
        sync_events = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND json_extract(payload_json, '$.kind')='work_attempt_sync_failed'", (task_id,),
        ).fetchall()
        failure_events = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchall()
    assert status == "reported"
    assert sync_events == []
    assert failure_events == []


def test_recancelling_an_interrupted_worker_keeps_the_graph_in_sync(tmp_path):
    """Live: Paseo's cancel-and-replace sent session/cancel while a worker
    ran. The interrupted turn cancelled the assignment (``turn_interrupted``),
    then the next prompt resumed RECOVERING and cancelled it again
    (``replanning``). The second mirror reused ``work-cancelled:<ref>`` with
    another reason and recorded ``work_attempt_sync_failed``
    (``idempotency_request_changed``) although the graph already showed
    the cancelled attempt as failed."""
    store, task_id, _epoch = _bootstrap(tmp_path)
    graph = WorkGraph(store)
    assignment = graph.add_assignment(task_id, "implementation", idempotency_key="graph-assignment")
    attempt = graph.create_attempt(assignment["assignment_id"], idempotency_key="graph-attempt")
    graph.transition_attempt(attempt["attempt_id"], "running", idempotency_key="graph-running")

    class _Engine:
        def terminate_owned(self, attempt_id):
            return None

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._engine = _Engine()
    runtime._lock = threading.Lock()
    runtime._worker_card_emitters = {}
    assignment_ref = "b" * 64
    now = "2026-09-24T20:11:37+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, "worker-attempt-1", "worker-backend-dev", "ready", "ready", 1, now, now),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegation-requested:recancel",
        {"work_attempt_ref": attempt["attempt_id"]}, origin="runtime", assignment_ref=assignment_ref,
    )

    runtime._cancel_assignment(task_id, assignment_ref, "turn_interrupted")
    runtime._cancel_assignment(task_id, assignment_ref, "replanning")

    with store.connection() as db:
        row = db.execute(
            "SELECT status, failure_reason FROM work_assignment_attempts WHERE attempt_id=?",
            (attempt["attempt_id"],),
        ).fetchone()
        sync_events = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND json_extract(payload_json, '$.kind')='work_attempt_sync_failed'", (task_id,),
        ).fetchall()
    assert (row["status"], row["failure_reason"]) == ("failed", "turn_interrupted")
    assert [r["payload_json"] for r in sync_events] == []


@pytest.mark.parametrize("via_predecessor", [False, True])
def test_user_selected_model_never_falls_back_to_a_different_identity(tmp_path, via_predecessor):
    store, task_id, connections, a, b = _setup_two_route_task(tmp_path)
    runtime, engine = _make_runtime(store, lambda *_: None, connections)
    revision = ConfigurationService(store).current(task_id)
    routing_store.record_attempt_audit(
        store, attempt_id="strict-attempt", task_id=task_id, role=ROLE_ID,
        engine=a.route.engine, connection_id=a.connection_id, model=a.route.model,
        params_fingerprint="strict-fp", config_revision_id=revision["id"], decision="authorized",
        detail={"route_basis": "auto:user_request"})
    old_attempt = "strict-attempt"
    if via_predecessor:
        old_attempt = "strict-successor"
        routing_store.record_attempt_audit(
            store, attempt_id=old_attempt, task_id=task_id, role=ROLE_ID,
            engine=a.route.engine, connection_id=a.connection_id, model=a.route.model,
            params_fingerprint="strict-fp", config_revision_id=revision["id"], decision="fallback",
            predecessor_attempt_id="strict-attempt", detail={})
    result = runtime._attempt_worker_route_fallback(
        task_id, "strict-assignment", "strict-assignment-id", ROLE_ID, old_attempt,
        "strict-workspace", revision["id"], ENGINE_FAILURE, "Build it")
    assert result == (None, True, None)
    assert engine.fallback_calls == []
    failures = [e for e in delegation_events.list_since(store, task_id) if e.event_type == "assignment_failed"]
    assert any(e.payload["error"] == "fallback_refused:user_requested_route_unavailable" for e in failures)
