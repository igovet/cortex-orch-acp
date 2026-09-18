"""ConfigurationService: propose/apply/reconcile/current/history and reads (T008)."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import FallbackPolicy, PermissionLevel, PolicyCeiling
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import (
    ConfigurationRejected,
    ConfigurationService,
    ConnectionUnavailable,
    RecoveryRequested,
)
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store


def _isolate(tmp_path, monkeypatch):
    # A sibling of tmp_path, never an ancestor of it: tmp_path itself becomes
    # a project root (gains a ".cortex" directly inside it) in these tests,
    # and connection_registry rejects XDG roots nested under any directory
    # that contains a project marker.
    xdg_root = tmp_path.parent / (tmp_path.name + "-xdg")
    config_home = xdg_root / "config"; state_home = xdg_root / "state"; home = xdg_root / "home"
    for path in (config_home, state_home, home):
        path.mkdir(parents=True)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))


def _connection(connection_id="cn_" + "a" * 12, state=ConnectionState.CONFIGURED):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine="codex-acp", provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=state, qualified=True, created_at=now, updated_at=now,
    )


def _entry(connection=None, effort="high", model="model-a"):
    connection = connection or _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort,
         "options": [{"value": effort, "name": effort}]},
    ]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    return snapshot.entries[0]


def _bootstrap(tmp_path, owner="owner-1"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", owner)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner=owner)


class _FakeAdapter:
    def __init__(self, outcome="confirm"):
        self.outcome = outcome
        self.calls = []

    def configure(self, engine_session_id, revision):
        self.calls.append((engine_session_id, revision["id"]))
        if self.outcome == "confirm":
            return "evref_" + revision["id"]
        if self.outcome == "reject":
            raise ConfigurationRejected("engine_declined")
        if self.outcome == "raise":
            raise RuntimeError("transport hiccup")
        return None  # unknown/empty outcome -> indeterminate


def test_propose_creates_first_selected_revision(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"})
    row = service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    assert row["state"] == "selected" and row["revision"] == 1
    assert service.history(task)[0]["id"] == row["id"]


def test_propose_rejects_stale_expected_revision_cas(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    assert excinfo.value.code == "stale_configuration_revision"
    assert excinfo.value.retry_eligible is True


def test_propose_rejects_missing_and_stale_ownership_epoch(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=None)
    assert excinfo.value.code == "missing_ownership_epoch"
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=OwnershipEpoch(epoch=99, owner="owner-1"))
    assert excinfo.value.code == "stale_ownership_epoch"


def test_propose_rejects_ceiling_expansion_and_accepts_narrowing(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(permission_level=PermissionLevel.ELEVATED))
    user_layer = dict(defaults={}, ceiling=PolicyCeiling(permission_level=PermissionLevel.STANDARD))
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch, user_layer=user_layer)
    assert excinfo.value.code == "ceiling_violation"
    narrower = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(permission_level=PermissionLevel.READ_ONLY))
    row = service.propose(task, entry, narrower, expected_revision=0, current_epoch=epoch, user_layer=user_layer)
    assert row["state"] == "selected"


def test_propose_rejects_unsupported_dependent_setting_bundle_atomically(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry(effort="high")
    inputs = ConfigurationInputs(route=entry.route, parameters={"thought_level": "extreme"})
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    assert excinfo.value.code == "unsupported_dependent_setting"
    assert service.history(task) == ()  # no partial apply: nothing written


def test_propose_rederives_dependent_settings_on_route_change(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    connection = _connection()
    first = _entry(connection=connection, effort="high", model="model-a")
    second = _entry(connection=connection, effort="high", model="model-b")
    inputs_1 = ConfigurationInputs(route=first.route, parameters={"thought_level": "high", "custom": "kept"})
    row1 = service.propose(task, first, inputs_1, expected_revision=0, current_epoch=epoch)
    # The new route only supports effort=high too, so proposing with no
    # explicit effort must not silently inherit anything incompatible; and
    # switching routes still re-derives rather than blindly carrying settings.
    inputs_2 = ConfigurationInputs(route=second.route, parameters={"thought_level": "high"})
    row2 = service.propose(task, second, inputs_2, expected_revision=row1["revision"], current_epoch=epoch)
    assert row2["parameters"]["custom"] == "kept"  # engine-agnostic field still merged from task layer
    assert row2["route_id"] == second.route.id


def test_apply_lifecycle_happy_path_with_fake_adapter(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    row = service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    adapter = _FakeAdapter("confirm")
    result = service.apply(task, row["id"], adapter, "es_" + "a" * 12, epoch)
    assert result["state"] == "active" and result["confirmation_evidence_ref"]
    assert service.current(task)["id"] == row["id"]


def test_apply_failed_keeps_prior_active_with_no_mixed_state_via_current(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    first_row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, first_row["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    second_row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"}), expected_revision=1, current_epoch=epoch)
    result = service.apply(task, second_row["id"], _FakeAdapter("reject"), "es_" + "a" * 12, epoch)
    assert result["state"] == "failed"
    current = service.current(task)
    assert current["id"] == first_row["id"]  # prior active is untouched


def test_apply_exception_marks_indeterminate_and_reconcile_both_ways(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    result = service.apply(task, row["id"], _FakeAdapter("raise"), "es_" + "a" * 12, epoch)
    assert result["state"] == "indeterminate"
    assert service.current(task) is None  # never surfaced as active/pending

    reconciled = service.reconcile(task, row["id"], epoch, evidence_ref="evref_manual")
    assert reconciled["state"] == "active"
    assert service.current(task)["id"] == row["id"]

    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"}), expected_revision=1, current_epoch=epoch)
    service.apply(task, row2["id"], _FakeAdapter("raise"), "es_" + "a" * 12, epoch)
    reconciled_failed = service.reconcile(task, row2["id"], epoch, failed=True, error_code="engine_timeout")
    assert reconciled_failed["state"] == "failed"
    assert service.current(task)["id"] == row["id"]  # first active revision still current


def test_current_never_returns_pending_or_indeterminate(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row["id"], _FakeAdapter(None), "es_" + "a" * 12, epoch)  # unknown outcome -> indeterminate
    assert service.current(task) is None


def test_mid_run_revision_snapshot_is_retained_after_a_later_revision_becomes_active(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"}), expected_revision=1, current_epoch=epoch)
    service.apply(task, row2["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    history = {row["id"]: row for row in service.history(task)}
    assert history[row1["id"]]["revision"] == 1  # snapshot never changes retroactively
    assert history[row1["id"]]["parameters"] == {"thought_level": "high"}
    assert service.current(task)["id"] == row2["id"]


def test_concurrent_propose_race_has_exactly_one_cas_winner(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()

    def attempt(_):
        try:
            return service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
        except PortableError as error:
            return error.code

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(attempt, range(8)))
    winners = [r for r in results if isinstance(r, dict)]
    assert len(winners) == 1
    assert all(r == "stale_configuration_revision" for r in results if not isinstance(r, dict))


def test_reconnect_restore_after_user_default_change_still_restores_task_route(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    connection = _connection()
    with connection_registry.transaction() as state:
        state.connections[connection.id] = connection
    entry = _entry(connection=connection)
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    # Changing user defaults afterward never affects an already-active revision.
    from cortex_runtime.configuration.store import write_user_defaults
    write_user_defaults({"effort": "low"}, PolicyCeiling())
    restored = service.reconnect_restore(task)
    assert restored["id"] == row["id"]


def test_reconnect_restore_unavailable_connection_never_substitutes(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    connection = _connection()
    with connection_registry.transaction() as state:
        state.connections[connection.id] = connection
    entry = _entry(connection=connection)
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    with connection_registry.transaction() as state:
        del state.connections[connection.id]
    with pytest.raises(ConnectionUnavailable):
        service.reconnect_restore(task)


def test_apply_returns_recovery_requested_on_engine_or_connection_change(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    connection_a = _connection("cn_" + "a" * 12)
    connection_b = _connection("cn_" + "b" * 12)
    entry_a = _entry(connection=connection_a)
    entry_b = _entry(connection=connection_b)
    row1 = service.propose(task, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    row2 = service.propose(task, entry_b, ConfigurationInputs(route=entry_b.route, parameters={}), expected_revision=1, current_epoch=epoch)
    outcome = service.apply(task, row2["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    assert isinstance(outcome, RecoveryRequested)
    history = {r["id"]: r for r in service.history(task)}
    assert history[row2["id"]]["state"] == "selected"  # left selected, never applied
    assert service.current(task)["id"] == row1["id"]


def test_worker_resolution_default_permitted_and_forbidden_override(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)

    default = service.resolve_worker(task, "reviewer", {"allows_role_alternates": False})
    assert default["id"] == row["id"]

    worker_route = _entry(model="model-b").route
    override = ConfigurationInputs(route=worker_route, parameters={}, ceiling_override=None)
    with pytest.raises(PortableError) as excinfo:
        service.resolve_worker(task, "reviewer", {"allows_role_alternates": False}, override)
    assert excinfo.value.code == "forbidden_worker_override"

    # T027: default (single-route) profile still forbids a *different*
    # route even when the legacy allows_role_alternates flag says True --
    # the stored route profile is now the authoritative source of truth.
    with pytest.raises(PortableError) as excinfo:
        service.resolve_worker(task, "reviewer", {"allows_role_alternates": True}, override)
    assert excinfo.value.code == "forbidden_worker_override"

    # Only an explicit mixed-team route profile naming this role/route
    # authorizes the override.
    from cortex_runtime.configuration.policy import RouteProfile

    row2 = service.propose(
        task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=1, current_epoch=epoch,
        route_profile=RouteProfile(role_routes={"reviewer": {worker_route.id}}),
    )
    service.apply(task, row2["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    permitted = service.resolve_worker(task, "reviewer", {"allows_role_alternates": True}, override)
    assert permitted["route_id"] == worker_route.id

    # Never accepts any cost field anywhere in the override shape.
    field_names = {f.name for f in __import__("dataclasses").fields(type(override))}
    assert not any("cost" in name or "spend" in name for name in field_names)


def test_fallback_denied_for_hard_denylist_even_when_policy_permits(tmp_path):
    # Construction itself rejects a denylisted class regardless of consent.
    with pytest.raises(PortableError) as excinfo:
        FallbackPolicy(allowed_failure_classes={"cancellation"}, data_recipient_consent=True)
    assert excinfo.value.code == "fallback_denylisted_failure_class"


def test_fallback_allowed_only_for_listed_class_with_consent(tmp_path):
    policy = FallbackPolicy(allowed_failure_classes={"rate_limited"}, data_recipient_consent=True)
    assert policy.permits("rate_limited") is True
    assert policy.permits("refusal") is False
    assert policy.permits("unrelated_class") is False


def test_ui_select_and_command_set_parity(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    ui_error = None
    try:
        service.ui_select(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "bogus"}), expected_revision=0, current_epoch=epoch)
    except PortableError as error:
        ui_error = error.code
    command_error = None
    try:
        service.command_set(task, entry, ConfigurationInputs(route=entry.route, parameters={"thought_level": "bogus"}), expected_revision=0, current_epoch=epoch)
    except PortableError as error:
        command_error = error.code
    assert ui_error == command_error == "unsupported_dependent_setting"
    row = service.ui_select(task, entry, inputs, expected_revision=0, current_epoch=epoch)
    assert row["state"] == "selected"
