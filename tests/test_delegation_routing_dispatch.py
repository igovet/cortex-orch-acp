"""T027: real ``EngineSessionManagerAdapter``/fake-engine-subprocess
integration for mixed-team routing, per-attempt audit, pre-dispatch
enforcement, and authorized fallback -- see ``tests/test_routing_policy.py``
for the pure policy/store unit coverage this builds on."""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import (
    CAPACITY_UNAVAILABLE,
    FallbackPolicy,
    PolicyCeiling,
    RouteProfile,
)
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.connections.model import ConnectionRecord
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.routing import store as routing_store
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


def _make_pair(tmp_path, *, same_provider=True):
    """Two independently-launchable fake-engine connections carrying the
    exact same model string ("shared-model") -- proves route identity is
    (engine, connection, model), never model alone, and that dispatching
    one never touches the other's own credential/process."""
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    # T035: this file's own fixture entries always name "shared-model" or
    # "other-model" (never "gpt-x", ``default_scenario``'s own placeholder)
    # -- override just the "model" option's values so the worker-mode-floor
    # confirm wire call (which, unlike a real coordinator ``configure()``
    # here, always re-sends ``model`` too -- codex-acp's own option mapping
    # requires it on every call) actually matches what these tests'
    # ``_entry()`` calls declare; ``default_scenario``'s own "mode" option
    # (added for this same T035 gap closure) is kept as-is.
    model_options = dict(
        id="model", currentValue="shared-model",
        options=[dict(value="shared-model", name="Shared"), dict(value="other-model", name="Other")],
    )
    scenario_a = write_scenario(engine_dir, default_scenario(
        config_options=[model_options, default_scenario()["config_options"][1]],
    ))
    engine_dir_b = engine_dir / "b"
    engine_dir_b.mkdir()
    scenario_b = write_scenario(engine_dir_b, default_scenario(
        config_options=[model_options, default_scenario()["config_options"][1]],
    ))
    connection_a = make_connection_record(new_connection_id(), fake_launch_command(scenario_a), environ=environ)
    connection_b = make_connection_record(new_connection_id(), fake_launch_command(scenario_b), environ=environ)
    if not same_provider:
        connection_b = ConnectionRecord(**{**vars(connection_b), "provider": "other-vendor"})
    with connection_registry.transaction(environ) as state:
        state.connections[connection_a.id] = connection_a
        state.connections[connection_b.id] = connection_b
    return environ, connection_a, connection_b


def test_attempt_open_records_engine_connection_model_fingerprint_and_revision(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_coord1", config_revision=row["id"], parameters={"note": "high"}),
        "instructions",
    )
    assert session_ref.startswith("es_")
    audit = routing_store.list_audit_for_task(store, task_id)
    assert len(audit) == 1
    entry = audit[0]
    assert entry["engine"] == connection_a.engine
    assert entry["connection_id"] == connection_a.id
    assert entry["model"] == "shared-model"
    assert entry["decision"] == "authorized"
    assert entry["config_revision_id"] == row["id"]
    assert entry["params_fingerprint"]  # non-empty, deterministic
    adapter.terminate_owned("xa_coord1")


def test_dispatch_refused_before_capacity_when_attempt_budget_exhausted(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    service = ConfigurationService(store)
    ceiling = PolicyCeiling(limits={"attempts": 1})
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}, ceiling_override=ceiling),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    adapter.open(
        "shared_project_root", dict(task_id=task_id, attempt_id="xa_c1", config_revision=row["id"]), "instructions",
    )
    # Second attempt: budget already exhausted -- refused before any
    # capacity reservation or process launch (no reservation row, no
    # second audit row).
    with pytest.raises(PortableError) as excinfo:
        adapter.open(
            "shared_project_root", dict(task_id=task_id, attempt_id="xa_c2", config_revision=row["id"]), "instructions",
        )
    assert excinfo.value.code == "attempt_budget_exhausted"
    assert len(routing_store.list_audit_for_task(store, task_id)) == 1
    assert capacity.count_active(store, connection_a.id) == 1  # only the first attempt ever reserved
    adapter.terminate_owned("xa_c1")


def test_worker_role_not_in_mixed_team_profile_is_refused_before_dispatch(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    entry_b = _entry(connection_b, "shared-model")
    service = ConfigurationService(store)
    ceiling = PolicyCeiling(allowed_routes={entry_a.route.id, entry_b.route.id})
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}, ceiling_override=ceiling),
        expected_revision=0, current_epoch=epoch,
        route_profile=RouteProfile(role_routes={"reviewer": {entry_b.route.id}}),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    # T027 review fix (HIGH): a NAMED role ("implementer") absent from an
    # explicit mixed-team profile (which only lists "reviewer") is denied
    # by default -- never silently inherits the coordinator's own route via
    # the generic worker/coordinator bucket fallback. Exercised at the
    # dispatch (open()) layer itself via launch["role_id"], never through
    # ConfigurationService.resolve_worker.
    with pytest.raises(PortableError) as excinfo:
        adapter.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"],
                 role_id="implementer", resolved_route_id=entry_a.route.id),
            "instructions",
        )
    assert excinfo.value.code == "role_route_not_authorized"
    assert routing_store.list_audit_for_task(store, task_id) == ()
    assert capacity.count_active(store, connection_a.id) == 0
    assert capacity.count_active(store, connection_b.id) == 0


def test_named_role_explicitly_denied_refused_even_bypassing_resolve_worker(tmp_path):
    """The profile names 'reviewer' with an EMPTY route set (never
    dispatched); calling open() directly with that role id -- never going
    through ConfigurationService.resolve_worker at all -- must still be
    refused, including for the coordinator's own default route."""
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        route_profile=RouteProfile(role_routes={"reviewer": frozenset()}),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    with pytest.raises(PortableError) as excinfo:
        adapter.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"],
                 role_id="reviewer"),
            "instructions",
        )
    assert excinfo.value.code == "role_route_not_authorized"
    assert routing_store.list_audit_for_task(store, task_id) == ()
    assert capacity.count_active(store, connection_a.id) == 0


def test_mid_task_route_change_leaves_running_attempt_route_unchanged(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    entry_b = _entry(connection_b, "shared-model")
    service = ConfigurationService(store)
    row1 = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    service.apply(task_id, row1["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)
    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    adapter.open(
        "shared_project_root", dict(task_id=task_id, attempt_id="xa_old", config_revision=row1["id"]), "instructions",
    )

    # Coordinator selection changes mid-task to a new route/revision.
    row2 = service.propose(
        task_id, entry_b, ConfigurationInputs(route=entry_b.route, parameters={}),
        expected_revision=1, current_epoch=epoch,
    )
    outcome = service.apply(task_id, row2["id"], _FakeConfigAdapter(), "es_" + "b" * 12, epoch)
    from cortex_runtime.configuration.service import RecoveryRequested
    assert isinstance(outcome, RecoveryRequested)  # engine/connection change requires explicit recovery

    old_audit = routing_store.list_audit_for_attempt(store, "xa_old")
    assert len(old_audit) == 1
    assert old_audit[0]["connection_id"] == connection_a.id
    assert old_audit[0]["config_revision_id"] == row1["id"]  # unchanged by the later proposal

    adapter.terminate_owned("xa_old")


def test_fallback_denied_without_consent_never_opens_or_transfers_data(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path, same_provider=False)
    entry_a = _entry(connection_a, "shared-model")
    entry_b = _entry(connection_b, "shared-model")
    service = ConfigurationService(store)
    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={entry_b.route.id}, allowed_failure_classes={CAPACITY_UNAVAILABLE},
            data_recipient_consent=True,
        ),
    )
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={"worker": {entry_a.route.id, entry_b.route.id}}),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    launch = dict(task_id=task_id, assignment_id="asg1", config_revision=row["id"])
    adapter.open("shared_project_root", dict(launch, attempt_id="xa_w1"), "instructions")

    with pytest.raises(PortableError) as excinfo:
        adapter.attempt_fallback(
            old_attempt_id="xa_w1", new_attempt_id="xa_w1fb", task_id=task_id, role="worker",
            failure_class=CAPACITY_UNAVAILABLE, target_connection_id=connection_b.id,
            target_route_id=entry_b.route.id, workspace_binding_id="shared_project_root",
            launch=launch, instructions="instructions",
        )
    assert excinfo.value.code == "fallback_consent_required"
    # No data transfer: connection_b never reserved capacity, never got an
    # audit row, and its own fake-engine process was never spawned.
    assert capacity.count_active(store, connection_b.id) == 0
    assert routing_store.list_audit_for_attempt(store, "xa_w1fb") == ()
    adapter.terminate_owned("xa_w1")


def test_authorized_cross_provider_fallback_dispatches_and_audits_predecessor(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path, same_provider=False)
    entry_a = _entry(connection_a, "shared-model")
    entry_b = _entry(connection_b, "other-model")
    service = ConfigurationService(store)
    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={entry_b.route.id}, allowed_failure_classes={CAPACITY_UNAVAILABLE},
            data_recipient_consent=True,
        ),
    )
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={"worker": {entry_a.route.id, entry_b.route.id}}),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)
    routing_store.record_consent(
        store, task_id=task_id, connection_id=connection_b.id, provider=connection_b.provider,
        data_scope="task workspace and prompt history", granted_by="owner-1",
    )

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
    )
    launch = dict(task_id=task_id, assignment_id="asg1", config_revision=row["id"])
    adapter.open("shared_project_root", dict(launch, attempt_id="xa_w1"), "instructions")

    session_ref = adapter.attempt_fallback(
        old_attempt_id="xa_w1", new_attempt_id="xa_w1fb", task_id=task_id, role="worker",
        failure_class=CAPACITY_UNAVAILABLE, target_connection_id=connection_b.id,
        target_route_id=entry_b.route.id, workspace_binding_id="shared_project_root",
        launch=launch, instructions="instructions", target_model="other-model",
    )
    assert session_ref.startswith("es_")
    fb_audit = routing_store.list_audit_for_attempt(store, "xa_w1fb")
    assert len(fb_audit) == 1
    assert fb_audit[0]["decision"] == "fallback"
    assert fb_audit[0]["predecessor_attempt_id"] == "xa_w1"
    assert fb_audit[0]["connection_id"] == connection_b.id
    assert fb_audit[0]["model"] == "other-model"
    adapter.terminate_owned("xa_w1")
    adapter.terminate_owned("xa_w1fb")


def test_concurrent_allocation_respects_connection_limit_and_fallback_uses_other_connection(tmp_path):
    store, task_id, epoch = _bootstrap(tmp_path)
    environ, connection_a, connection_b = _make_pair(tmp_path)
    entry_a = _entry(connection_a, "shared-model")
    entry_b = _entry(connection_b, "shared-model")
    service = ConfigurationService(store)
    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={entry_b.route.id}, allowed_failure_classes={CAPACITY_UNAVAILABLE},
            data_recipient_consent=True,
        ),
    )
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={"worker": {entry_a.route.id, entry_b.route.id}}),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store,
        capacity_limit=1,
    )
    launch = dict(task_id=task_id, assignment_id="asg1", config_revision=row["id"])
    adapter.open("shared_project_root", dict(launch, attempt_id="xa_w1"), "instructions")
    assert capacity.count_active(store, connection_a.id) == 1
    # connection_a is at its concurrency limit (1); capacity.reserve itself
    # (the atomic, already-existing T017 gate) refuses a second reservation
    # on the SAME connection outright.
    from cortex_runtime.delegation import capacity as capacity_mod
    assert capacity_mod.reserve(store, connection_a.id, task_id, "xa_w2", "worker", limit=1) is None
    # An authorized fallback to a DIFFERENT connection is unaffected by
    # connection_a's own exhausted limit.
    session_ref = adapter.attempt_fallback(
        old_attempt_id="xa_w1", new_attempt_id="xa_w1fb", task_id=task_id, role="worker",
        failure_class=CAPACITY_UNAVAILABLE, target_connection_id=connection_b.id,
        target_route_id=entry_b.route.id, workspace_binding_id="shared_project_root",
        launch=launch, instructions="instructions",
    )
    assert session_ref.startswith("es_")
    assert capacity.count_active(store, connection_b.id) == 1
    adapter.terminate_owned("xa_w1")
    adapter.terminate_owned("xa_w1fb")
