"""EXT-01 #5 fix: ``EngineSessionManagerAdapter`` correctly maps an
engine's own ``engine_session_id`` back to the *one task* it belongs to,
and only ever asks *that task's* currently-registered
(``acp.orchestrating_runtime.OrchestratingRuntime.permission_requester_for_task``)
connection-bound requester -- never a different task's, never a stale one,
and never any requester at all when this adapter has no runtime wired
(the same fail-closed-to-cancelled default this fix must not weaken).
"""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from test_delegation_worker_mode_floor import _MODE_OPTIONS, _entry


class _FakeConfigAdapter:
    def configure(self, engine_session_id, revision):
        return "evidence-ref"


class _FakeTaskRuntime:
    """Stands in for ``OrchestratingRuntime.permission_requester_for_task``:
    records every lookup and returns whatever was registered for that exact
    ``task_id``, nothing more -- the same "no guessing, no fallback to a
    different task's requester" contract the real method promises."""

    def __init__(self):
        self._requesters = {}
        self.lookups = []

    def register(self, task_id, requester):
        self._requesters[task_id] = requester

    def permission_requester_for_task(self, task_id):
        self.lookups.append(task_id)
        return self._requesters.get(task_id)


def _bootstrap(tmp_path, name):
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), f"Do work {name}.", "owner-1")
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner="owner-1")


def _connection_and_config(tmp_path):
    environ = isolated_environ(tmp_path)
    config_options = [
        dict(id="model", currentValue="shared-model", options=[dict(value="shared-model", name="shared-model")]),
        dict(id="mode", currentValue=_MODE_OPTIONS[0]["value"], options=_MODE_OPTIONS),
    ]
    scenario = default_scenario(config_options=config_options)
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), environ=environ)
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    return environ, connection


def _propose_and_apply(store, task_id, epoch, connection, session_ref="es_" + "a" * 12):
    entry = _entry(connection, "shared-model")
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry, ConfigurationInputs(route=entry.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), session_ref, epoch)
    return row


def test_adapter_routes_permission_request_to_the_correct_tasks_requester(tmp_path):
    """Two attempts belonging to two *different* tasks, opened on the same
    (project-scoped, per T016) adapter -- exactly the real production
    layout, since ``ProductionOrchestratingRuntime`` caches one adapter per
    project, reused across every task in it. A request for task A's own
    engine session must reach *only* task A's registered requester; task
    B's is never even looked up for it, and vice versa."""
    store, task_a, epoch_a = _bootstrap(tmp_path, "a")
    store_b, task_b, epoch_b = store, None, None
    # Same store/project, a second task in it.
    result_b = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work b.", "owner-1")
    task_b = result_b.task_id
    with store.connection() as db:
        epoch_b = OwnershipEpoch(epoch=current_owner_epoch(db, task_b), owner="owner-1")

    environ, connection = _connection_and_config(tmp_path)
    row_a = _propose_and_apply(store, task_a, epoch_a, connection, session_ref="es_" + "a" * 12)
    row_b = _propose_and_apply(store, task_b, epoch_b, connection, session_ref="es_" + "b" * 12)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    fake_runtime = _FakeTaskRuntime()
    calls_a, calls_b = [], []
    fake_runtime.register(task_a, lambda engine_session_id, params, timeout: (
        calls_a.append((engine_session_id, params, timeout)) or {"outcome": {"outcome": "selected", "optionId": "deny-a"}}
    ))
    fake_runtime.register(task_b, lambda engine_session_id, params, timeout: (
        calls_b.append((engine_session_id, params, timeout)) or {"outcome": {"outcome": "selected", "optionId": "deny-b"}}
    ))
    adapter.set_runtime(fake_runtime)

    session_a = adapter.open(
        "shared_project_root", dict(task_id=task_a, attempt_id="xa_a", assignment_id="asg_a", config_revision=row_a["id"]),
        "instructions",
    )
    session_b = adapter.open(
        "shared_project_root", dict(task_id=task_b, attempt_id="xa_b", assignment_id="asg_b", config_revision=row_b["id"]),
        "instructions",
    )
    try:
        result_a = adapter._host_permission_requester(session_a, dict(toolCall=dict(toolCallId="tc-a")), 5.0)
        assert result_a == {"outcome": {"outcome": "selected", "optionId": "deny-a"}}
        assert calls_a == [(session_a, dict(toolCall=dict(toolCallId="tc-a")), 5.0)]
        assert calls_b == []
        assert fake_runtime.lookups == [task_a]

        result_b = adapter._host_permission_requester(session_b, dict(toolCall=dict(toolCallId="tc-b")), 5.0)
        assert result_b == {"outcome": {"outcome": "selected", "optionId": "deny-b"}}
        assert calls_b == [(session_b, dict(toolCall=dict(toolCallId="tc-b")), 5.0)]
        assert len(calls_a) == 1  # unchanged: task A's own call from above, never a second one
        assert fake_runtime.lookups == [task_a, task_b]
    finally:
        adapter.terminate_owned("xa_a")
        adapter.terminate_owned("xa_b")


def test_adapter_fails_closed_with_no_runtime_wired(tmp_path):
    """``set_runtime`` never called (every pre-EXT-01 caller/test) -- the
    same unconditional ``None`` this seam always returned before this fix,
    never an exception, never a guess."""
    store, task_id, epoch = _bootstrap(tmp_path, "solo")
    environ, connection = _connection_and_config(tmp_path)
    row = _propose_and_apply(store, task_id, epoch, connection)
    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root", dict(task_id=task_id, attempt_id="xa_1", assignment_id="asg1", config_revision=row["id"]),
        "instructions",
    )
    try:
        assert adapter._host_permission_requester(session_ref, dict(toolCall=dict(toolCallId="tc-1")), 5.0) is None
    finally:
        adapter.terminate_owned("xa_1")


def test_adapter_fails_closed_for_an_unknown_engine_session_id(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path, "solo2")
    environ, connection = _connection_and_config(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    fake_runtime = _FakeTaskRuntime()
    fake_runtime.register(task_id, lambda *a: {"outcome": {"outcome": "selected", "optionId": "x"}})
    adapter.set_runtime(fake_runtime)
    assert adapter._host_permission_requester("es_never_opened", dict(), 5.0) is None
    assert fake_runtime.lookups == []


def test_worker_deletion_reaches_the_registered_requester_end_to_end_via_real_fake_engine(tmp_path):
    """Real ``EngineSessionManager``/``PermissionBridge`` machinery, a real
    fake-engine subprocess (never a stub bypassing the wire), routed through
    the exact production seam (``adapter.set_runtime`` ->
    ``permission_requester_for_task``) this fix adds: an ordinary edit is
    auto-approved locally (never reaching the registered requester at all),
    a deletion reaches it, and the registered task's own decision is
    relayed back to the engine verbatim."""
    store, task_id, epoch = _bootstrap(tmp_path, "delete")
    environ = isolated_environ(tmp_path)

    edit_request = dict(
        method="session/request_permission",
        params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-edit", kind="edit"),
                     options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]),
    )
    delete_request = dict(
        method="session/request_permission",
        params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-delete", kind="delete"),
                     options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]),
    )
    reply_log = tmp_path / "replies.jsonl"
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="shared-model", options=[dict(value="shared-model", name="shared-model")]),
            dict(id="mode", currentValue="agent", options=_MODE_OPTIONS),
        ],
        prompt_behavior="client_requests_sequence",
        client_requests=[edit_request, delete_request],
        client_reply_log_path=str(reply_log),
    )
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), environ=environ)
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    fake_runtime = _FakeTaskRuntime()
    host_calls = []
    fake_runtime.register(task_id, lambda engine_session_id, params, timeout: (
        host_calls.append(params) or {"outcome": {"outcome": "selected", "optionId": "deny"}}
    ))
    adapter.set_runtime(fake_runtime)

    session_ref = adapter.open(
        "shared_project_root", dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]),
        "instructions",
    )
    manager = adapter._managers["xa_w1"]
    try:
        import json

        list(manager.prompt_turn(session_ref, "do it"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert len(replies) == 2
        assert replies[0]["result"] == {"outcome": {"outcome": "selected", "optionId": "deny"}}
        assert replies[1]["result"] == {"outcome": {"outcome": "selected", "optionId": "deny"}}
        assert host_calls == []
        assert fake_runtime.lookups == []
    finally:
        adapter.terminate_owned("xa_w1")
