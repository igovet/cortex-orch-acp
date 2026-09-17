"""T008 lead-required addition C: lifecycle mixed-state and race invariants."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import threading
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import PermissionLevel, PolicyCeiling
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationRejected, ConfigurationService
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store


def _connection(connection_id="cn_" + "a" * 12):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine="codex-acp", provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )


def _entry(effort="high", model="model-a"):
    connection = _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort,
         "options": [{"value": effort, "name": effort}]},
    ]
    return build_catalog({connection.id: connection}, {connection.id: parse_observation(options)}).entries[0]


def _bootstrap(tmp_path, owner="owner-1"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", owner)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner=owner)


class _FakeAdapter:
    def __init__(self, outcome="confirm", barrier=None):
        self.outcome = outcome
        self.barrier = barrier

    def configure(self, engine_session_id, revision):
        if self.barrier is not None:
            self.barrier.wait(timeout=5)
        if self.outcome == "confirm":
            return "evref_" + revision["id"]
        if self.outcome == "reject":
            raise ConfigurationRejected("engine_declined")
        if self.outcome == "raise":
            raise RuntimeError("transport hiccup")
        return None


def test_failed_apply_never_yields_mixed_route_or_effort_via_current(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry_a = _entry(effort="high", model="model-a")
    entry_b = _entry(effort="high", model="model-b")
    row1 = service.propose(task, entry_a, ConfigurationInputs(route=entry_a.route, parameters={"effort": "high"}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    active_before = service.current(task)

    row2 = service.propose(task, entry_b, ConfigurationInputs(route=entry_b.route, parameters={"effort": "high"}), expected_revision=1, current_epoch=epoch)
    service.apply(task, row2["id"], _FakeAdapter("reject"), "es_" + "a" * 12, epoch)
    active_after = service.current(task)

    # current() is exactly the row1 snapshot, unchanged in every field --
    # never a blend of row1's route with row2's parameters or vice versa.
    assert active_after == active_before
    assert active_after["route_id"] == entry_a.route.id
    assert active_after["parameters"] == {"effort": "high"}


def test_indeterminate_apply_never_yields_mixed_state_via_current(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry_a = _entry(model="model-a")
    entry_b = _entry(model="model-b")
    row1 = service.propose(task, entry_a, ConfigurationInputs(route=entry_a.route, parameters={"effort": "high"}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    snapshot = service.current(task)

    row2 = service.propose(task, entry_b, ConfigurationInputs(route=entry_b.route, parameters={"effort": "high"}), expected_revision=1, current_epoch=epoch)
    service.apply(task, row2["id"], _FakeAdapter("raise"), "es_" + "a" * 12, epoch)
    assert service.current(task) == snapshot  # still exactly the first active row


def test_concurrent_propose_and_apply_interleaving_produces_no_lost_update_or_two_active_rows(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    barrier = threading.Barrier(2)

    row_a = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=0, current_epoch=epoch)

    def apply_worker():
        try:
            return service.apply(task, row_a["id"], _FakeAdapter("confirm", barrier=barrier), "es_" + "a" * 12, epoch)
        except PortableError as error:
            return error.code

    def propose_worker():
        barrier.wait(timeout=5)
        try:
            return service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=row_a["revision"], current_epoch=epoch)
        except PortableError as error:
            return error.code

    with ThreadPoolExecutor(max_workers=2) as pool:
        apply_future = pool.submit(apply_worker)
        propose_future = pool.submit(propose_worker)
        apply_result = apply_future.result()
        propose_result = propose_future.result()

    with store.connection() as db:
        active_rows = db.execute(
            "SELECT id FROM configuration_revisions WHERE task_id=? AND state='active'", (task,)
        ).fetchall()
    assert len(active_rows) <= 1  # never two simultaneously active rows

    # Either the apply won its race and later became active/failed/
    # stale_pending_apply (if the propose happened first and superseded it),
    # or the propose's own CAS caught a stale expected_revision -- in every
    # outcome exactly the DB's own state machine decided it, never a lost
    # update silently overwriting the other side.
    assert isinstance(propose_result, dict) or propose_result == "stale_configuration_revision"
    assert isinstance(apply_result, dict) or apply_result in ("stale_pending_apply",)


def test_apply_rejects_stale_revision_at_start_when_newer_one_already_proposed(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=0, current_epoch=epoch)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=row1["revision"], current_epoch=epoch)
    with pytest.raises(PortableError) as excinfo:
        service.apply(task, row1["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    assert excinfo.value.code == "stale_configuration_revision"
    # row1 was never touched: still 'selected'.
    history = {r["id"]: r for r in service.history(task)}
    assert history[row1["id"]]["state"] == "selected"


def test_apply_rejects_stale_pending_apply_that_completes_after_a_newer_revision_was_proposed(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=0, current_epoch=epoch)

    class _AdapterThatProposesANewerRevisionMidFlight:
        def configure(self, engine_session_id, revision):
            # A newer revision gets proposed *while* this adapter call is
            # still outstanding -- simulating a slow/late engine response
            # racing a fresh propose() from elsewhere.
            service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={"effort": "high"}), expected_revision=row1["revision"], current_epoch=epoch)
            return "evref_late"

    with pytest.raises(PortableError) as excinfo:
        service.apply(task, row1["id"], _AdapterThatProposesANewerRevisionMidFlight(), "es_" + "a" * 12, epoch)
    assert excinfo.value.code == "stale_pending_apply"
    history = {r["id"]: r for r in service.history(task)}
    assert history[row1["id"]]["state"] == "failed"
    assert history[row1["id"]]["error_code"] == "stale_pending_apply"
    assert service.current(task) is None  # never made active


def test_apply_rechecks_tightened_project_ceiling(tmp_path):
    from cortex_runtime.configuration import store as configuration_store

    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    project_root = str(tmp_path)
    configuration_store.write_project_defaults(
        store, project_root, {}, PolicyCeiling(permission_level=PermissionLevel.ELEVATED)
    )
    inputs = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(permission_level=PermissionLevel.ELEVATED))
    row = service.propose(
        task, entry, inputs, expected_revision=0, current_epoch=epoch,
        user_layer=dict(defaults={}, ceiling=PolicyCeiling(permission_level=PermissionLevel.ELEVATED)),
        project_layer=dict(defaults={}, ceiling=PolicyCeiling(permission_level=PermissionLevel.ELEVATED)),
    )
    # Tighten the project ceiling after selection but before application.
    configuration_store.write_project_defaults(
        store, project_root, {}, PolicyCeiling(permission_level=PermissionLevel.READ_ONLY)
    )
    with pytest.raises(PortableError) as excinfo:
        service.apply(task, row["id"], _FakeAdapter("confirm"), "es_" + "a" * 12, epoch)
    assert excinfo.value.code == "ceiling_violation"
    history = {r["id"]: r for r in service.history(task)}
    assert history[row["id"]]["state"] == "rejected"
    assert history[row["id"]]["error_code"] == "ceiling_tightened"
