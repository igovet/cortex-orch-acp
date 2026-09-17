"""T028: failure classification, retry policy, cancellation binding
invalidation, and the owned-resource reaper."""
from __future__ import annotations

import os
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration import policy as configuration_policy
from cortex_runtime.delegation import capacity as delegation_capacity
from cortex_runtime.delegation import live_adapters
from cortex_runtime.delegation import workspace_isolation
from cortex_runtime.engine import failure_classification, reaper, retry_policy
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(store):
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return result.task_id


# -- failure_classification ---------------------------------------------

def test_classify_distinguishes_rate_limit_auth_provider_from_engine_failure():
    assert failure_classification.classify("rate_limit_exceeded") == failure_classification.RATE_LIMITED
    assert failure_classification.classify("HTTP 429 Too Many Requests") == failure_classification.RATE_LIMITED
    assert failure_classification.classify("unauthorized: token_expired") == failure_classification.AUTHORIZATION_LOSS
    assert failure_classification.classify("model_not_available in this region") == failure_classification.PROVIDER_RESTRICTION
    assert failure_classification.classify("poisoned") == failure_classification.ENGINE_FAILURE
    # A downstream engine's own transport closing is an existing, already-
    # named engine-crash signal (T017/T027) -- genuinely fallback-eligible,
    # not the new TRANSPORT_FAILURE class (reserved for host-connection-
    # level markers this codebase does not yet raise as an EngineTurnFailed
    # kind).
    assert failure_classification.classify("transport_eof") == failure_classification.ENGINE_FAILURE
    assert failure_classification.classify("broken_pipe") == failure_classification.TRANSPORT_FAILURE
    assert failure_classification.classify(None) == failure_classification.INDETERMINATE
    assert failure_classification.classify("") == failure_classification.INDETERMINATE
    assert failure_classification.classify("something nobody has ever named") == failure_classification.INDETERMINATE


def test_classify_never_guesses_engine_failure_for_unknown_text():
    # Conservative: an unrecognized kind/message is INDETERMINATE, never
    # silently ENGINE_FAILURE (T028 lead decision 1).
    assert failure_classification.classify("frobnicate_widget_37") == failure_classification.INDETERMINATE


def test_only_engine_failure_is_fallback_eligible():
    for cls in failure_classification.ALL_CLASSES:
        expected = cls == failure_classification.ENGINE_FAILURE
        assert failure_classification.is_fallback_eligible(cls) is expected


def test_rate_limited_auth_loss_provider_restriction_never_fallback_eligible_per_t027_gate():
    # T027's own gate (routing.authorization.authorize_fallback) only ever
    # accepts a class inside KNOWN_FALLBACK_ELIGIBLE_CLASSES; assert every
    # T028 class this module can classify a real failure into is excluded,
    # except the one genuine engine-crash class.
    never_eligible = {
        failure_classification.RATE_LIMITED, failure_classification.AUTHORIZATION_LOSS,
        failure_classification.PROVIDER_RESTRICTION, failure_classification.REFUSAL,
        failure_classification.CANCELLATION, failure_classification.TRANSPORT_FAILURE,
        failure_classification.INDETERMINATE,
    }
    for cls in never_eligible:
        assert cls not in configuration_policy.KNOWN_FALLBACK_ELIGIBLE_CLASSES


# -- retry_policy ---------------------------------------------------------

def test_retry_policy_refuses_publication_retry_without_reconciliation():
    decision = retry_policy.decide(retry_policy.PUBLICATION, attempt_number=1, reconciled=False)
    assert decision.may_retry is False
    assert decision.reason == "reconciliation_required_but_not_performed"


def test_retry_policy_allows_publication_retry_once_reconciled():
    decision = retry_policy.decide(retry_policy.PUBLICATION, attempt_number=1, reconciled=True)
    assert decision.may_retry is True


def test_retry_policy_allows_capacity_reservation_without_reconciliation():
    # Never mutates anything external -- safely repeatable regardless.
    decision = retry_policy.decide(retry_policy.CAPACITY_RESERVATION, attempt_number=1, reconciled=False)
    assert decision.may_retry is True


def test_retry_policy_caps_attempts():
    decision = retry_policy.decide(retry_policy.CAPACITY_RESERVATION, attempt_number=4, max_attempts=3)
    assert decision.may_retry is False
    assert decision.reason == "max_attempts_exhausted"


def test_bounded_backoff_caps_and_doubles():
    gen = retry_policy.bounded_backoff(start=0.1, ceiling=1.0)
    values = [next(gen) for _ in range(6)]
    assert values[:4] == [0.1, 0.2, 0.4, 0.8]
    assert values[4] == 1.0 and values[5] == 1.0


# -- reaper: capacity -------------------------------------------------------

class _FakeEngine:
    def __init__(self, states):
        self._states = states
        self.terminated = []

    def lookup_owned(self, attempt_id):
        return self._states.get(attempt_id, "unknown")

    def terminate_owned(self, attempt_id):
        self.terminated.append(attempt_id)


def test_sweep_capacity_releases_only_confirmed_absent_never_unknown(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    task_id = _task(store)
    r_absent = delegation_capacity.reserve(store, "cn_absent", task_id, "xa_absent", "worker", limit=5)
    r_unknown = delegation_capacity.reserve(store, "cn_unknown", task_id, "xa_unknown", "worker", limit=5)
    delegation_capacity.mark_ambiguous(store, r_absent, "timeout_alive")
    delegation_capacity.mark_ambiguous(store, r_unknown, "timeout_alive")

    engine = _FakeEngine({"xa_absent": "absent", "xa_unknown": "unknown"})
    actions = reaper.sweep_capacity(store, engine)

    assert actions["released"] == 1
    assert actions["unknown_left_ambiguous"] == 1
    with store.connection() as db:
        states = {row["reservation_id"]: row["state"] for row in db.execute(
            "SELECT reservation_id, state FROM capacity_reservations")}
    assert states[r_absent] == "released"
    assert states[r_unknown] == "ambiguous"  # never falsely released


def test_sweep_capacity_asks_alive_attempts_to_terminate(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    task_id = _task(store)
    r_alive = delegation_capacity.reserve(store, "cn_alive", task_id, "xa_alive", "worker", limit=5)
    delegation_capacity.mark_ambiguous(store, r_alive, "timeout_alive")
    engine = _FakeEngine({"xa_alive": "alive"})
    reaper.sweep_capacity(store, engine)
    assert engine.terminated == ["xa_alive"]


def test_sweep_capacity_is_bounded_and_idempotent(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    task_id = _task(store)
    for i in range(5):
        r = delegation_capacity.reserve(store, f"cn_{i}", task_id, f"xa_{i}", "worker", limit=5)
        delegation_capacity.mark_ambiguous(store, r, "timeout_alive")
    engine = _FakeEngine({f"xa_{i}": "absent" for i in range(5)})
    first = reaper.sweep_capacity(store, engine, limit=2)
    assert first["released"] == 2
    second = reaper.sweep_capacity(store, engine, limit=10)
    assert second["released"] == 3
    # A third sweep finds nothing left ambiguous -- idempotent no-op.
    third = reaper.sweep_capacity(store, engine, limit=10)
    assert third == dict(released=0, terminate_attempted=0, unknown_left_ambiguous=0)


# -- reaper: owned workspaces -----------------------------------------------

def _bind_attempt(store, task_id, attempt_id):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            (f"idem-{attempt_id}", task_id, f"a-{attempt_id}", attempt_id, "backend-dev", "workspace",
             "in_progress", 1, store._now(), store._now()),
        )
        db.commit()
    return f"idem-{attempt_id}"


def test_sweep_owned_workspaces_removes_only_terminal_integrated(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    (project / "tracked.txt").write_text("hello\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref_done = _bind_attempt(store, task_id, "xa_done")
    assignment_ref_open = _bind_attempt(store, task_id, "xa_open")

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_done = provisioner.create("xa_done")
    binding_open = provisioner.create("xa_open")
    root_done = provisioner.root_for(binding_done)
    root_open = provisioner.root_for(binding_open)
    assert os.path.isdir(root_done) and os.path.isdir(root_open)

    # xa_done: integration outcome recorded -> eligible for cleanup.
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO workspace_integrations "
            "(id, task_id, assignment_ref, request_key, attempt_id, workspace_binding_id, status, "
            " applied_paths_json, conflict_paths_json, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("wi_" + str(uuid.uuid4()), task_id, assignment_ref_done, "req_1", "xa_done", binding_done,
             "applied", "[]", "[]", store._now()),
        )
        db.commit()
    # xa_open: nothing terminal recorded -> must survive the sweep.

    actions = reaper.sweep_owned_workspaces(store)
    assert actions["removed"] == 1
    assert actions["not_terminal"] == 1
    assert not os.path.exists(root_done)
    assert os.path.isdir(root_open)
    # Never touches the user's actual project files.
    assert (project / "tracked.txt").read_text() == "hello\n"

    # Idempotent: a second sweep finds nothing left to remove for xa_done.
    second = reaper.sweep_owned_workspaces(store)
    assert second["removed"] == 0


def test_sweep_owned_workspaces_removes_cancelled_assignment_with_no_integration(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_cancelled")
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_cancelled")
    root = provisioner.root_for(binding_id)
    from cortex_runtime.delegation import events as delegation_events

    delegation_events.append_event(
        store, task_id, "assignment_cancelled", f"assignment_cancelled:{assignment_ref}",
        dict(reason="test"), origin="runtime", assignment_ref=assignment_ref,
    )
    actions = reaper.sweep_owned_workspaces(store)
    assert actions["removed"] == 1
    assert not os.path.exists(root)


# -- cancellation binding invalidation (session/cancel, T028 decision 3) ---

def test_engine_session_manager_adapter_invalidate_session_forwards_to_owning_manager():
    class _FakeManager:
        def __init__(self):
            self.invalidated = []

        def invalidate_session(self, engine_session_id, reason=None):
            self.invalidated.append((engine_session_id, reason))

    adapter = live_adapters.EngineSessionManagerAdapter.__new__(live_adapters.EngineSessionManagerAdapter)
    import threading

    adapter._lock = threading.Lock()
    manager = _FakeManager()
    adapter._session_to_attempt = {"sess_1": "attempt_1"}
    adapter._managers = {"attempt_1": manager}

    adapter.invalidate_session("sess_1", "session_cancelled")
    assert manager.invalidated == [("sess_1", "session_cancelled")]

    # An unknown session_ref is a silent no-op -- never raises.
    adapter.invalidate_session("sess_unknown", "session_cancelled")
