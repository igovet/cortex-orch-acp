"""T035 gap closure: a worker's own engine mode is confirmed (not merely
requested) via ``EngineSessionManagerAdapter._apply_worker_mode_floor``,
using the exact same real fake-engine-subprocess machinery
``test_delegation_routing_dispatch.py`` already exercises for the
coordinator's own floor. See ``configuration.policy.WORKER_MODE_FLOOR``'s
own docstring for why "agent"/"default" (codex-acp/claude-agent-acp) are
the two floor values, and ``delegation.live_adapters
.EngineSessionManagerAdapter._apply_worker_mode_floor``'s own docstring for
exactly what "cannot confirm" means and why it blocks only the one worker
attempt, never the whole task.
"""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import COORDINATOR_MODE_FLOOR, WORKER_MODE_FLOOR
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter, WorkerModeNotConfirmed
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


class _FakeConfigAdapter:
    def configure(self, engine_session_id, revision):
        return "evidence-ref"


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner="owner-1")


def _entry(connection, model):
    options = [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
                "options": [{"value": model, "name": model}]}]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    return snapshot.entries[0]


_MODE_OPTIONS = [
    dict(value="read-only", name="Ask for approval"),
    dict(value="agent", name="Approve for me"),
    dict(value="agent-full-access", name="Full access"),
]


def _connection_and_config(tmp_path, *, mode_options=_MODE_OPTIONS, model="shared-model", configure_behavior_by_option=None):
    environ = isolated_environ(tmp_path)
    config_options = [
        dict(id="model", currentValue=model, options=[dict(value=model, name=model)]),
    ]
    if mode_options is not None:
        config_options.append(dict(id="mode", currentValue=mode_options[0]["value"], options=mode_options))
    scenario = default_scenario(config_options=config_options)
    if configure_behavior_by_option:
        scenario["configure_behavior_by_option"] = configure_behavior_by_option
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), environ=environ)
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    return environ, connection


def _propose_and_apply(store, task_id, epoch, connection, model="shared-model"):
    entry = _entry(connection, model)
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry, ConfigurationInputs(route=entry.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)
    return row


def test_worker_mode_floor_matches_pinned_engine_values():
    """The exact values this task closes the gap around -- see
    ``WORKER_MODE_FLOOR``'s own docstring for the pinned-build evidence."""
    # OpenCode v2: the one Cortex agent, whose rules ask for every native
    # tool call (tests/test_engine_opencode_qualification.py).
    assert WORKER_MODE_FLOOR == {"codex-acp": "agent", "claude-agent-acp": "default", "opencode": "cortex"}


def test_coordinator_floor_matches_the_worker_floor_after_finding_12():
    """FINDING-12 (see ``configuration.policy.COORDINATOR_MODE_FLOOR``'s own
    docstring): the coordinator's engine mode used to be a genuinely
    non-editing mode (codex read-only / claude plan), but that mode never
    issues ``session/request_permission`` for the edits it silently allows,
    so it removed Cortex's only interception point. The floor now matches
    ``WORKER_MODE_FLOOR`` -- the narrowest mode still known to always ask --
    and enforcement moved to ``engine.permissions.PermissionBridge``."""
    assert COORDINATOR_MODE_FLOOR == {"codex-acp": "agent", "claude-agent-acp": "default", "opencode": "cortex"}


def test_worker_session_confirms_its_own_mode_over_the_wire(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection = _connection_and_config(tmp_path)
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]),
        "instructions",
    )
    assert session_ref.startswith("es_")
    manager = adapter._managers["xa_w1"]
    observed = manager.observed_config_options(session_ref)
    mode_option = next(option for option in observed if option.get("id") == "mode")
    # Confirmed, not merely requested: the wire echoed "agent" back as the
    # session's own currentValue after the real session/set_config_option
    # round-trip -- see EngineSessionManager.configure's own confirm-or-fail
    # gate.
    assert mode_option["currentValue"] == "agent"
    adapter.terminate_owned("xa_w1")


def test_worker_mode_not_offered_blocks_this_attempt_and_leaks_nothing(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    # No "mode" option at all in this engine session's own observed
    # configOptions -- the engine can never be shown to route a permission
    # decision through session/request_permission at all.
    environ, connection = _connection_and_config(tmp_path, mode_options=None)
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    with pytest.raises(WorkerModeNotConfirmed):
        adapter.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]),
            "instructions",
        )
    # The just-opened session/process was torn down, not leaked: no
    # capacity reservation survives, and the attempt is untracked.
    assert capacity.count_active(store, connection.id) == 0
    assert "xa_w1" not in adapter._managers
    assert "xa_w1" not in adapter._sessions


def test_worker_mode_wire_rejection_blocks_this_attempt(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    # "mode" is offered, but the engine's own wire confirm rejects it --
    # never a silent "assume it worked".
    environ, connection = _connection_and_config(
        tmp_path, configure_behavior_by_option={"mode": "error"},
    )
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    with pytest.raises(WorkerModeNotConfirmed):
        adapter.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]),
            "instructions",
        )
    assert capacity.count_active(store, connection.id) == 0


def test_coordinator_open_never_touches_the_worker_mode_floor(tmp_path):
    """A coordinator launch (no assignment_id) must never run
    ``_apply_worker_mode_floor`` -- even when the engine offers no "mode"
    option at all (which would immediately block a worker)."""
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection = _connection_and_config(tmp_path, mode_options=None)
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root", dict(task_id=task_id, attempt_id="xa_c1", config_revision=row["id"]), "instructions",
    )
    assert session_ref.startswith("es_")
    adapter.terminate_owned("xa_c1")


def test_confirmed_worker_mode_and_permission_classifier_work_together(tmp_path):
    """End to end with fakes (no paid call, no real engine launch): once
    this worker's own mode is confirmed via ``_apply_worker_mode_floor``,
    an ordinary tool call is auto-approved by ``PermissionBridge`` without
    ever reaching the host, and a deletion reaches the host confirmation
    path -- proving the confirmed mode and the classifier actually work
    together (the point of this task), not merely that each exists in
    isolation."""
    store, task_id, epoch = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)

    host_calls = []

    def host_permission_requester(engine_session_id, params, timeout):
        host_calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "deny"}}

    reply_log = tmp_path / "replies.jsonl"
    edit_request = dict(
        method="session/request_permission",
        params=dict(
            sessionId="sess-1",
            toolCall=dict(toolCallId="tc-edit", kind="edit"),
            options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
        ),
    )
    delete_request = dict(
        method="session/request_permission",
        params=dict(
            sessionId="sess-1",
            toolCall=dict(toolCallId="tc-delete", kind="delete"),
            options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
        ),
    )
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
        host_permission_requester=host_permission_requester,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]),
        "instructions",
    )
    manager = adapter._managers["xa_w1"]
    # T035 gap closure: ``EngineSessionManagerAdapter.open`` now calls
    # ``bind_resource`` itself (see ``_bind_resource``) for every session
    # opened with a real store, so no manual binding is set up here -- this
    # exercises the actual production wiring, not a hand-made binding.
    assert manager.bindings.get(session_ref) is not None
    assert manager.bindings.get(session_ref).role == "worker"
    try:
        list(manager.prompt_turn(session_ref, "do it"))
        import json

        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert len(replies) == 2
        # Native edit and delete act inside the engine jail: both are
        # allowed locally by PermissionBridge, never a host round-trip.
        assert replies[0]["result"] == {"outcome": {"outcome": "selected", "optionId": "allow"}}
        # The deletion asked the user (who denied it here); the edit did not.
        assert replies[1]["result"] == {"outcome": {"outcome": "selected", "optionId": "deny"}}
        assert [call["toolCall"]["toolCallId"] for call in host_calls] == ["tc-delete"]
    finally:
        adapter.terminate_owned("xa_w1")


def test_unbound_session_fails_closed_even_for_an_ordinary_operation(tmp_path):
    """T035 gap closure, the property protecting everything else: an
    engine session opened with no ``store`` (so ``_bind_resource`` never
    runs -- see its own docstring) must still refuse every permission
    request, never fall back to permissive/auto-approved behaviour, even
    for an ordinary (non-deletion) tool call that a *bound* worker session
    would auto-approve, and even though a host requester is wired and
    ready to answer. ``PermissionBridge`` must never be reached at all --
    it must short-circuit on the missing binding before ever consulting
    role/classification or a host."""
    environ = isolated_environ(tmp_path)

    host_calls = []

    def host_permission_requester(engine_session_id, params, timeout):
        host_calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    reply_log = tmp_path / "replies.jsonl"
    edit_request = dict(
        method="session/request_permission",
        params=dict(
            sessionId="sess-1",
            toolCall=dict(toolCallId="tc-edit", kind="edit"),
            options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
        ),
    )
    scenario = default_scenario(
        prompt_behavior="client_requests_sequence",
        client_requests=[edit_request],
        client_reply_log_path=str(reply_log),
    )
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), environ=environ)
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection

    # No ``store=`` supplied -- the same "fake/test double with no durable
    # state" case ``_bind_resource``'s own docstring names.
    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0,
        host_permission_requester=host_permission_requester,
    )
    session_ref = adapter.open(
        "shared_project_root", dict(task_id="task-1", attempt_id="xa_w1", assignment_id="asg1"), "instructions",
    )
    manager = adapter._managers["xa_w1"]
    assert manager.bindings.get(session_ref) is None
    try:
        import json

        list(manager.prompt_turn(session_ref, "do it"))
        replies = [json.loads(line) for line in reply_log.read_text().splitlines() if line]
        assert len(replies) == 1
        assert replies[0]["result"] == {"outcome": {"outcome": "cancelled"}}
        assert host_calls == []
    finally:
        adapter.terminate_owned("xa_w1")


def test_an_engine_without_an_effort_setting_still_runs_the_worker(tmp_path):
    """D-T035-03, and the risk the first attempt at it introduced: the
    configure call that carries a worker's own parameters is confirm-or-fail
    by design, so asking an engine for an option it does not offer refuses
    the worker outright. A policy about which effort a role may use must
    narrow what a worker runs with -- never stop it running on an engine
    that has no such setting.

    This fake engine offers `mode` and no effort option at all.
    """
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection = _connection_and_config(tmp_path)
    row = _propose_and_apply(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_eff", assignment_id="asg-eff", config_revision=row["id"],
             parameters=dict(effort="high")),
        "instructions",
    )
    assert session_ref.startswith("es_")
    manager = adapter._managers["xa_eff"]
    observed = manager.observed_config_options(session_ref)
    assert not [o for o in observed if o.get("id") in ("reasoning_effort", "thought_level", "effort")]
    # The mode floor is still confirmed: the effort being inapplicable
    # weakens nothing else.
    mode_option = next(o for o in observed if o.get("id") == "mode")
    assert mode_option["currentValue"] == "agent"
    adapter.terminate_owned("xa_eff")


# claude-agent-acp's own shape (see tests/fixtures/catalog/
# claude_agent_acp_config_options.json for the ids): session/new starts on
# the default model, which offers an effort selector; once the session's
# model is set to Haiku, the engine drops that selector entirely (the live
# catalog records Haiku with no effort values at all).
_CLAUDE_MODES = [dict(value=v, name=v) for v in ("default", "acceptEdits", "plan", "auto", "bypassPermissions")]
_CLAUDE_EFFORTS = [dict(value=v, name=v) for v in ("default", "low", "medium", "high", "xhigh", "max")]


def _claude_haiku_connection(tmp_path, request_log, *, drop_on_model_switch=("effort",)):
    environ = isolated_environ(tmp_path)
    config_options = [
        dict(id="mode", category="mode", type="select", currentValue="acceptEdits", options=_CLAUDE_MODES),
        dict(id="model", category="model", type="select", currentValue="default",
             options=[dict(value="default", name="Default (recommended)"), dict(value="haiku", name="Haiku 4.5")]),
        dict(id="effort", category="thought_level", type="select", currentValue="default", options=_CLAUDE_EFFORTS),
    ]
    scenario = default_scenario(
        config_options=config_options, request_log_path=str(request_log),
        remove_options_on_apply={"model": list(drop_on_model_switch)},
    )
    connection = make_connection_record(
        new_connection_id(), fake_launch_command(write_scenario(tmp_path, scenario)),
        engine="claude-agent-acp", environ=environ,
    )
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    return environ, connection


def _claude_coordinator_revision(store, task_id, epoch, connection):
    """The coordinator runs on the default model at medium effort."""
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "default",
         "options": [{"value": "default", "name": "Default (recommended)"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "default",
         "options": _CLAUDE_EFFORTS},
    ]
    entry = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)}).entries[0]
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "medium"}),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)
    return row


def _set_config_calls(request_log):
    import json

    return [
        (entry["params"]["configId"], entry["params"]["value"])
        for entry in map(json.loads, request_log.read_text(encoding="utf-8").splitlines())
        if entry.get("method") == "session/set_config_option"
    ]


def test_a_haiku_worker_whose_model_drops_the_effort_option_still_runs(tmp_path):
    """User-reported: every Haiku worker failed with WorkerModeNotConfirmed.
    Its session opened on the default model, which offers an effort; the
    coordinator's medium effort was judged against that, then sent after
    the switch to Haiku had already removed the effort option. The worker
    now runs with only what its Haiku session offers, and its mode is still
    confirmed over the wire."""
    store, task_id, epoch = _bootstrap(tmp_path)
    request_log = tmp_path / "requests.jsonl"
    environ, connection = _claude_haiku_connection(tmp_path, request_log)
    row = _claude_coordinator_revision(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_haiku", assignment_id="asg-haiku", config_revision=row["id"],
             resolved_model="haiku"),
        "instructions",
    )
    try:
        observed = adapter._managers["xa_haiku"].observed_config_options(session_ref)
        assert next(o for o in observed if o["id"] == "model")["currentValue"] == "haiku"
        assert next(o for o in observed if o["id"] == "mode")["currentValue"] == "default"
        calls = _set_config_calls(request_log)
        assert ("effort", "medium") not in calls
        assert [option_id for option_id, _ in calls if option_id == "effort"] == []
        assert calls[-1] == ("mode", "default")
    finally:
        adapter.terminate_owned("xa_haiku")


def test_a_model_switch_that_drops_the_mode_option_still_refuses_the_worker(tmp_path):
    """Applying the model first never weakens the floor: a model whose
    session offers no mode leaves the worker unconfirmed, and nothing runs."""
    store, task_id, epoch = _bootstrap(tmp_path)
    request_log = tmp_path / "requests.jsonl"
    environ, connection = _claude_haiku_connection(tmp_path, request_log, drop_on_model_switch=("effort", "mode"))
    row = _claude_coordinator_revision(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    with pytest.raises(WorkerModeNotConfirmed):
        adapter.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id="xa_haiku", assignment_id="asg-haiku", config_revision=row["id"],
                 resolved_model="haiku"),
            "instructions",
        )
    assert capacity.count_active(store, connection.id) == 0
    assert "xa_haiku" not in adapter._managers
    assert ("mode", "default") not in _set_config_calls(request_log)


def test_a_worker_on_the_default_model_keeps_its_effort(tmp_path):
    """The neighbouring case: a model that does offer an effort still gets
    the requested one, with no extra model switch."""
    store, task_id, epoch = _bootstrap(tmp_path)
    request_log = tmp_path / "requests.jsonl"
    environ, connection = _claude_haiku_connection(tmp_path, request_log, drop_on_model_switch=())
    row = _claude_coordinator_revision(store, task_id, epoch, connection)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_def", assignment_id="asg-def", config_revision=row["id"],
             resolved_model="default", parameters=dict(effort="high")),
        "instructions",
    )
    try:
        assert _set_config_calls(request_log) == [("model", "default"), ("effort", "high"), ("mode", "default")]
        observed = adapter._managers["xa_def"].observed_config_options(session_ref)
        assert next(o for o in observed if o["id"] == "mode")["currentValue"] == "default"
    finally:
        adapter.terminate_owned("xa_def")


@pytest.mark.parametrize("requested, offered, expected", [
    ("medium", {"low", "medium", "high"}, "medium"),
    ("xhigh", {"default", "low", "medium", "high", "max"}, "high"),
    ("low", {"medium", "high"}, "medium"),
    ("medium", set(), None),
    ("medium", {"default"}, None),
    ("default", {"default", "low"}, "default"),
    ("custom", {"low"}, None),
    (None, {"low"}, None),
])
def test_a_worker_effort_is_lowered_before_raised_to_what_its_session_offers(requested, offered, expected):
    from cortex_runtime.delegation.live_adapters import _effort_within

    assert _effort_within(requested, offered) == expected
