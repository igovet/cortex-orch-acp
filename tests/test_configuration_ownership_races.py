"""T008 fresh-review HIGH fix regression tests: apply()/reconcile() must
re-check ownership at the final (post-adapter-call) transaction, not only at
the start, and reconcile() must itself require the caller's current epoch.
"""
from __future__ import annotations

import multiprocessing
import threading
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationRejected, ConfigurationService
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.ownership import acquire_task_owner, current_owner_epoch
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


def _entry(model="model-a"):
    connection = _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
    ]
    return build_catalog({connection.id: connection}, {connection.id: parse_observation(options)}).entries[0]


def _bootstrap(tmp_path, owner="owner-1"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", owner)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner=owner)


class _GatedAdapter:
    """Blocks inside ``configure`` until released, so a test can transfer
    ownership while the (simulated) engine call is genuinely in flight."""

    def __init__(self, released: threading.Event, entered: threading.Event, evidence="evref"):
        self._released = released
        self._entered = entered
        self._evidence = evidence

    def configure(self, engine_session_id, revision):
        self._entered.set()
        if not self._released.wait(timeout=5):
            raise TimeoutError("test adapter was never released")
        return self._evidence


def test_apply_marks_stale_owner_indeterminate_when_ownership_transfers_mid_flight(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)

    released = threading.Event()
    entered = threading.Event()
    adapter = _GatedAdapter(released, entered)
    outcome = {}

    def run_apply():
        try:
            outcome["result"] = service.apply(task, row["id"], adapter, "es_" + "a" * 12, epoch)
        except PortableError as error:
            outcome["error"] = error

    thread = threading.Thread(target=run_apply)
    thread.start()
    assert entered.wait(timeout=5)  # adapter.configure is genuinely in flight now

    # Ownership transfers to a new owner while the (stale) caller's apply()
    # is still blocked inside adapter.configure().
    new_epoch = acquire_task_owner(store, task, "owner-2", epoch.epoch)
    assert new_epoch.epoch == epoch.epoch + 1

    released.set()
    thread.join(timeout=5)
    assert not thread.is_alive()

    assert "error" in outcome and "result" not in outcome
    assert outcome["error"].code == "stale_owner_apply"
    assert outcome["error"].category.value == "authorization"
    assert outcome["error"].retry_eligible is False

    history = {r["id"]: r for r in service.history(task)}
    assert history[row["id"]]["state"] == "indeterminate"
    assert history[row["id"]]["error_code"] == "stale_owner_apply"
    assert service.current(task) is None  # never promoted to active under the stale owner


def test_new_owner_reconciles_stale_owner_apply_both_ways(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()

    def make_stale_indeterminate(revision_expected):
        row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=revision_expected, current_epoch=OwnershipEpoch(epoch=current_owner_epoch_of(store, task), owner="owner-2"))
        released = threading.Event(); entered = threading.Event()
        adapter = _GatedAdapter(released, entered)
        stale_epoch = OwnershipEpoch(epoch=current_owner_epoch_of(store, task), owner="owner-2")
        outcome = {}

        def run_apply():
            try:
                outcome["result"] = service.apply(task, row["id"], adapter, "es_" + "a" * 12, stale_epoch)
            except PortableError as error:
                outcome["error"] = error

        thread = threading.Thread(target=run_apply)
        thread.start()
        assert entered.wait(timeout=5)
        newest = acquire_task_owner(store, task, "owner-3", stale_epoch.epoch)
        released.set()
        thread.join(timeout=5)
        assert outcome["error"].code == "stale_owner_apply"
        return row, newest

    def current_owner_epoch_of(store, task):
        with store.connection() as db:
            return current_owner_epoch(db, task)

    row_active, epoch_owner3 = make_stale_indeterminate(0)
    reconciled_active = service.reconcile(task, row_active["id"], epoch_owner3, evidence_ref="late_evidence_confirmed")
    assert reconciled_active["state"] == "active"
    assert service.current(task)["id"] == row_active["id"]

    row_failed, epoch_owner4_precursor = make_stale_indeterminate(row_active["revision"])
    # make_stale_indeterminate transferred ownership again inside itself
    # (to "owner-3" again via a fresh acquire); read the true current epoch
    # for the reconcile call below.
    with store.connection() as db:
        latest_epoch_value = current_owner_epoch(db, task)
    latest_epoch = OwnershipEpoch(epoch=latest_epoch_value, owner="owner-3")
    reconciled_failed = service.reconcile(task, row_failed["id"], latest_epoch, failed=True, error_code="engine_timeout")
    assert reconciled_failed["state"] == "failed"
    assert service.current(task)["id"] == row_active["id"]  # unchanged


def test_stale_owners_reconcile_is_rejected(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)

    released = threading.Event(); entered = threading.Event()
    adapter = _GatedAdapter(released, entered)
    outcome = {}

    def run_apply():
        try:
            outcome["result"] = service.apply(task, row["id"], adapter, "es_" + "a" * 12, epoch)
        except PortableError as error:
            outcome["error"] = error

    thread = threading.Thread(target=run_apply)
    thread.start()
    assert entered.wait(timeout=5)
    acquire_task_owner(store, task, "owner-2", epoch.epoch)
    released.set()
    thread.join(timeout=5)
    assert outcome["error"].code == "stale_owner_apply"

    # The ORIGINAL (now stale) owner's epoch can never reconcile this row.
    with pytest.raises(PortableError) as excinfo:
        service.reconcile(task, row["id"], epoch, evidence_ref="forged")
    assert excinfo.value.code == "stale_ownership_epoch"


# --- cross-process race -----------------------------------------------------

def _cross_process_apply_worker(store_directory, project_root, task_id, revision_id, owner_ref, epoch_value, evidence, barrier, result_queue):
    """Runs in its own OS process (spawn): opens an independent Store on the
    same on-disk project and races apply() against a sibling process."""
    import sys
    sys.path.insert(0, __import__("os").environ["CORTEX_TEST_SCRIPTS_PATH"])
    from cortex_runtime.configuration.service import ConfigurationService
    from cortex_runtime.portable.identity import OwnershipEpoch
    from cortex_runtime.portable.errors import PortableError
    from cortex_runtime.store import Store

    class _ImmediateAdapter:
        def configure(self, engine_session_id, revision):
            return evidence

    store = Store(store_directory, project_root=project_root)
    service = ConfigurationService(store)
    barrier.wait(timeout=10)
    try:
        outcome = service.apply(task_id, revision_id, _ImmediateAdapter(), "es_" + owner_ref[-1] * 12, OwnershipEpoch(epoch=epoch_value, owner=owner_ref))
        result_queue.put(("ok", getattr(outcome, "state", outcome.__class__.__name__) if not isinstance(outcome, dict) else outcome.get("state")))
    except PortableError as error:
        result_queue.put(("error", error.code))
    except BaseException as error:  # pragma: no cover - defensive
        result_queue.put(("crash", repr(error)))


def test_cross_process_apply_race_on_same_revision_never_produces_two_active_rows(tmp_path):
    import os

    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    store_directory = str((tmp_path / ".cortex"))
    project_root = str(tmp_path)

    scripts_path = os.environ.get("CORTEX_TEST_SCRIPTS_PATH")
    if not scripts_path:
        import cortex_runtime
        scripts_path = str(__import__("pathlib").Path(cortex_runtime.__file__).resolve().parents[1])
    os.environ["CORTEX_TEST_SCRIPTS_PATH"] = scripts_path

    ctx = multiprocessing.get_context("spawn")
    barrier = ctx.Barrier(2)
    result_queue = ctx.Queue()
    processes = [
        ctx.Process(
            target=_cross_process_apply_worker,
            args=(store_directory, project_root, task, row["id"], "owner-1", epoch.epoch, f"evref-{i}", barrier, result_queue),
        )
        for i in range(2)
    ]
    for process in processes:
        process.start()
    for process in processes:
        process.join(timeout=15)
        assert process.exitcode == 0

    results = [result_queue.get(timeout=5) for _ in processes]
    outcomes = [kind for kind, _ in results]
    assert "crash" not in outcomes

    successes = [payload for kind, payload in results if kind == "ok"]
    errors = [payload for kind, payload in results if kind == "error"]
    # Exactly one process reaches a live state (active, or at minimum not
    # rejected outright by the CAS on revision state); the other is turned
    # away by the same 'invalid_revision_state'/'stale_*' guard that
    # protects the single-process path.
    assert len(successes) == 1
    assert len(errors) == 1
    assert errors[0] in ("invalid_revision_state", "stale_configuration_revision", "stale_pending_apply", "stale_owner_apply")

    with store.connection() as db:
        active_rows = db.execute(
            "SELECT id FROM configuration_revisions WHERE task_id=? AND state='active'", (task,)
        ).fetchall()
    assert len(active_rows) <= 1


# --- reconcile() must recheck for a superseding revision (T008 second HIGH) -

def _make_stale_owner_indeterminate(store, service, task, entry, expected_revision, stale_owner):
    """Drives one revision into indeterminate/stale_owner_apply and returns
    ``(row, new_owner_epoch)``; mirrors the scenario in
    test_new_owner_reconciles_stale_owner_apply_both_ways."""
    with store.connection() as db:
        stale_epoch_value = current_owner_epoch(db, task)
    stale_epoch = OwnershipEpoch(epoch=stale_epoch_value, owner=stale_owner)
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=expected_revision, current_epoch=stale_epoch)
    released = threading.Event(); entered = threading.Event()
    adapter = _GatedAdapter(released, entered)
    outcome = {}

    def run_apply():
        try:
            outcome["result"] = service.apply(task, row["id"], adapter, "es_" + "a" * 12, stale_epoch)
        except PortableError as error:
            outcome["error"] = error

    thread = threading.Thread(target=run_apply)
    thread.start()
    assert entered.wait(timeout=5)
    new_owner_epoch = acquire_task_owner(store, task, "owner-next", stale_epoch.epoch)
    released.set()
    thread.join(timeout=5)
    assert outcome["error"].code == "stale_owner_apply"
    return row, new_owner_epoch


def test_reconcile_rejects_activating_a_superseded_revision_reviewer_scenario(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()

    # rev1 lands indeterminate/stale_owner_apply while ownership transfers.
    row1, owner2_epoch = _make_stale_owner_indeterminate(store, service, task, entry, 0, "owner-2")

    # The new owner (owner-3, after the transfer above landed at "owner-next"
    # inside the helper) proposes and applies rev2, which becomes active.
    with store.connection() as db:
        latest_epoch_value = current_owner_epoch(db, task)
    owner_next_epoch = OwnershipEpoch(epoch=latest_epoch_value, owner="owner-next")
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=row1["revision"], current_epoch=owner_next_epoch)
    service.apply(task, row2["id"], _FakeConfirmAdapter(), "es_" + "b" * 12, owner_next_epoch)
    assert service.current(task)["id"] == row2["id"]

    # Reconciling the now-superseded rev1 to 'active' must be rejected...
    with pytest.raises(PortableError) as excinfo:
        service.reconcile(task, row1["id"], owner_next_epoch, evidence_ref="late_evidence_after_rev2")
    assert excinfo.value.code == "stale_reconcile_target"
    assert excinfo.value.retry_eligible is False

    # ...current() must remain rev2, never disturbed by the rejected attempt.
    assert service.current(task)["id"] == row2["id"]

    # ...but reconciling it to 'failed' is still allowed, and always records
    # error_code='superseded' regardless of any caller-supplied error_code.
    reconciled = service.reconcile(task, row1["id"], owner_next_epoch, failed=True, error_code="ignored_by_design")
    assert reconciled["state"] == "failed"
    assert reconciled["error_code"] == "superseded"

    history = {r["id"]: r for r in service.history(task)}
    assert history[row1["id"]]["state"] == "failed"
    assert history[row1["id"]]["error_code"] == "superseded"
    assert history[row2["id"]]["state"] == "active"
    assert service.current(task)["id"] == row2["id"]


class _FakeConfirmAdapter:
    def configure(self, engine_session_id, revision):
        return "evref_" + revision["id"]


def test_direct_sql_activation_of_a_superseded_row_is_blocked_by_the_db_trigger(tmp_path):
    from cortex_runtime.configuration import store as configuration_store

    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _FakeConfirmAdapter(), "es_" + "a" * 12, epoch)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=row1["revision"], current_epoch=epoch)
    service.apply(task, row2["id"], _FakeConfirmAdapter(), "es_" + "b" * 12, epoch)
    assert service.current(task)["id"] == row2["id"]

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        # row1 is already 'active' (apply() never demotes an old active row
        # when a newer one is applied); row2, at a higher revision, is also
        # 'active'. Any direct SQL write that would (re)affirm row1 as
        # 'active' -- exactly what a buggy caller bypassing reconcile()'s
        # own check might attempt -- is blocked by the trigger itself,
        # independent of the service-level guard.
        with pytest.raises(Exception, match="stale_reconcile_target"):
            db.execute(
                "UPDATE configuration_revisions SET state='active',confirmation_evidence_ref='forged' WHERE id=?",
                (row1["id"],),
            )
        db.rollback()


def test_reconcile_of_the_latest_indeterminate_still_works(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)

    class _RaisingAdapter:
        def configure(self, engine_session_id, revision):
            raise RuntimeError("transport hiccup")

    service.apply(task, row["id"], _RaisingAdapter(), "es_" + "a" * 12, epoch)
    history = {r["id"]: r for r in service.history(task)}
    assert history[row["id"]]["state"] == "indeterminate"

    reconciled = service.reconcile(task, row["id"], epoch, evidence_ref="evref_confirmed_out_of_band")
    assert reconciled["state"] == "active"
    assert service.current(task)["id"] == row["id"]


# --- T008 final-review MEDIUM: widen the trigger/service guard to any live
# (not just active/pending_next_use) newer revision --------------------------

class _RaisingAdapter:
    def configure(self, engine_session_id, revision):
        raise RuntimeError("transport hiccup")


class _RejectingAdapter:
    def configure(self, engine_session_id, revision):
        raise ConfigurationRejected("engine_declined")


def _propose_and_strand_indeterminate(service, task, entry, epoch, expected_revision):
    row = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=expected_revision, current_epoch=epoch)
    service.apply(task, row["id"], _RaisingAdapter(), "es_" + "a" * 12, epoch)
    return row


def test_direct_sql_activation_of_older_indeterminate_blocked_by_newer_selected_row(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = _propose_and_strand_indeterminate(service, task, entry, epoch, 0)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=row1["revision"], current_epoch=epoch)
    assert row2["state"] == "selected"  # never applied

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="stale_reconcile_target"):
            db.execute(
                "UPDATE configuration_revisions SET state='active',confirmation_evidence_ref='forged' WHERE id=?",
                (row1["id"],),
            )
        db.rollback()
    # The service-level guard rejects it identically.
    with pytest.raises(PortableError) as excinfo:
        service.reconcile(task, row1["id"], epoch, evidence_ref="forged")
    assert excinfo.value.code == "stale_reconcile_target"


def test_direct_sql_activation_of_older_indeterminate_blocked_by_newer_indeterminate_row(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = _propose_and_strand_indeterminate(service, task, entry, epoch, 0)
    row2 = _propose_and_strand_indeterminate(service, task, entry, epoch, row1["revision"])
    assert row2["state"] == "selected"
    history = {r["id"]: r for r in service.history(task)}
    assert history[row2["id"]]["state"] == "indeterminate"

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="stale_reconcile_target"):
            db.execute(
                "UPDATE configuration_revisions SET state='active',confirmation_evidence_ref='forged' WHERE id=?",
                (row1["id"],),
            )
        db.rollback()
    with pytest.raises(PortableError) as excinfo:
        service.reconcile(task, row1["id"], epoch, evidence_ref="forged")
    assert excinfo.value.code == "stale_reconcile_target"


@pytest.mark.parametrize("newer_terminal_state", ["failed", "rejected"])
def test_activation_of_older_indeterminate_allowed_when_only_newer_rows_are_terminal(tmp_path, newer_terminal_state):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = _propose_and_strand_indeterminate(service, task, entry, epoch, 0)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=row1["revision"], current_epoch=epoch)
    if newer_terminal_state == "failed":
        service.apply(task, row2["id"], _RejectingAdapter(), "es_" + "b" * 12, epoch)
    else:
        from cortex_runtime.configuration import store as configuration_store
        with store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            configuration_store.transition_tx(db, task, row2["id"], "rejected", error_code="test_setup")
            db.commit()
    history = {r["id"]: r for r in service.history(task)}
    assert history[row2["id"]]["state"] == newer_terminal_state

    # A newer row that is itself already failed/rejected never blocks
    # activating the older indeterminate one -- neither at the DB trigger
    # level (direct SQL) nor through the service.
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "UPDATE configuration_revisions SET state='active',confirmation_evidence_ref='direct_sql_ok' WHERE id=?",
            (row1["id"],),
        )
        db.commit()
    with store.connection() as db:
        row = db.execute("SELECT state FROM configuration_revisions WHERE id=?", (row1["id"],)).fetchone()
    assert row["state"] == "active"


def test_service_reconcile_allows_activation_when_only_newer_rows_are_failed_or_rejected(tmp_path):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = _propose_and_strand_indeterminate(service, task, entry, epoch, 0)
    row2 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=row1["revision"], current_epoch=epoch)
    service.apply(task, row2["id"], _RejectingAdapter(), "es_" + "b" * 12, epoch)
    assert service.history(task)[-1]["state"] == "failed"

    reconciled = service.reconcile(task, row1["id"], epoch, evidence_ref="evref_out_of_band")
    assert reconciled["state"] == "active"
    assert service.current(task)["id"] == row1["id"]


def test_reconcile_of_the_latest_revision_after_older_rows_already_failed_or_rejected_still_works(tmp_path):
    """No legitimate path is blocked: activating the truly-latest revision
    after earlier ones failed/were rejected must keep working exactly as
    before this widening."""
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    row1 = service.propose(task, entry, ConfigurationInputs(route=entry.route, parameters={}), expected_revision=0, current_epoch=epoch)
    service.apply(task, row1["id"], _RejectingAdapter(), "es_" + "a" * 12, epoch)
    assert service.history(task)[-1]["state"] == "failed"

    row2 = _propose_and_strand_indeterminate(service, task, entry, epoch, row1["revision"])
    reconciled = service.reconcile(task, row2["id"], epoch, evidence_ref="evref_latest")
    assert reconciled["state"] == "active"
    assert service.current(task)["id"] == row2["id"]
