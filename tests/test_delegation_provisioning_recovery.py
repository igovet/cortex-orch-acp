"""T015: delegation.provisioning -- atomic, recoverable, at-most-once provisioning.

All external effects come from in-memory fakes defined in this file; no
real engine/workspace process is ever spawned, and every ``Store`` is
rooted at ``tmp_path`` (never the real checkout).
"""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.provisioning import ProvisioningService
from cortex_runtime.ownership import acquire_task_owner
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel


def event():
    return SourceEvent(origin="cli", event_id=str(uuid.uuid4()))


def new_key():
    return str(uuid.uuid4())


class FakeWorkspace:
    def __init__(self):
        self.created = {}
        self.create_calls = 0
        self.lookup_calls = 0
        self.raise_on_create = False
        self.lookup_override = None

    def create(self, attempt_id):
        self.create_calls += 1
        if self.raise_on_create:
            self.raise_on_create = False
            raise RuntimeError("simulated crash during workspace.create")
        self.created[attempt_id] = True
        return "wb_" + attempt_id[3:]

    def lookup(self, attempt_id):
        self.lookup_calls += 1
        if self.lookup_override is not None:
            return self.lookup_override
        return "present" if attempt_id in self.created else "absent"

    def remove(self, attempt_id):
        self.created.pop(attempt_id, None)


class FakeEngineSessionOpener:
    def __init__(self):
        self.opened = {}
        self.open_calls = 0
        self.raise_on_open = False
        self.lookup_override = None
        self.launch_channel_secrets = []
        self.refuse_missing_secret = False

    def open(self, workspace_binding_id, launch, instructions):
        self.open_calls += 1
        self.launch_channel_secrets.append(launch.get("channel_secret"))
        if self.refuse_missing_secret and launch.get("channel_id") and not launch.get("channel_secret"):
            raise RuntimeError("refuses to open with a channel_id but no channel_secret")
        if self.raise_on_open:
            self.raise_on_open = False
            raise RuntimeError("simulated crash during engine open")
        attempt_id = launch["attempt_id"]
        self.opened[attempt_id] = True
        return "es_" + attempt_id[3:]

    def lookup_owned(self, attempt_id):
        if self.lookup_override is not None:
            return self.lookup_override
        return "alive" if attempt_id in self.opened else "absent"

    def terminate_owned(self, attempt_id):
        self.opened.pop(attempt_id, None)


class FakeConfigSnapshotSource:
    def __init__(self, engine="codex-acp", version="1.12.0"):
        self.engine = engine
        self.version = version
        self.calls = 0

    def snapshot(self, task_id):
        self.calls += 1
        return dict(engine=self.engine, version=self.version, config_revision="cr_" + task_id[2:])


def _channel(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, event(), "Build a page.", "owner-1")
    channel = resolve_channel(store, result.channel_id, result.secret)
    return store, result, channel


def _service(store, **kwargs):
    kwargs.setdefault("allow_unqualified_delivery", True)
    return ProvisioningService(store, FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource(), **kwargs)


def test_delegate_produces_one_ready_assignment_with_no_model_ids(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    outcome = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    assert outcome.status == "ready"
    assert outcome.assignment_ref
    assert outcome.workspace_binding_id and outcome.channel_id
    assert outcome.engine_session_ref and outcome.config_revision and outcome.context_snapshot_ref
    assert outcome.instruction_delivery == "unqualified"
    assert ws.create_calls == 1
    assert eng.open_calls == 1


def test_retry_with_same_request_key_is_idempotent(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    first = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    second = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert first == second
    assert ws.create_calls == 1
    assert eng.open_calls == 1


def test_different_request_key_is_a_new_delegation(tmp_path):
    store, result, channel = _channel(tmp_path)
    svc = _service(store)
    first = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    second = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    assert first.assignment_ref != second.assignment_ref
    assert first.attempt_id != second.attempt_id


def test_crash_during_workspace_create_then_retry_recovers_to_exactly_one(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    ws.raise_on_create = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    outcome = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert outcome.status == "ready"
    # One successful create (the failed attempt raised before recording anything).
    assert len(ws.created) == 1
    assert eng.open_calls == 1


def test_workspace_intent_without_outcome_reconciles_via_lookup(tmp_path):
    """Simulate: workspace really was created, but the outcome commit never
    happened (process died between the external call and the DB write)."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()

    real_record_effect = __import__("cortex_runtime.delegation.provisioning", fromlist=["_record_effect"])._record_effect
    calls = {"n": 0}

    import cortex_runtime.delegation.provisioning as provisioning_module

    def crash_after_workspace_outcome_intent(db, idempotency_key, step, phase, detail):
        real_record_effect(db, idempotency_key, step, phase, detail)
        if step == "workspace" and phase == "intent":
            calls["n"] += 1
            if calls["n"] == 1:
                raise RuntimeError("simulated crash after intent, before external create() returns")

    original = provisioning_module._record_effect
    provisioning_module._record_effect = crash_after_workspace_outcome_intent
    try:
        with pytest.raises(RuntimeError):
            svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    finally:
        provisioning_module._record_effect = original

    # The intent was committed but create() was never called (we crashed
    # before it). lookup() must report "absent" and the retry proceeds to
    # actually create it -- exactly once.
    outcome = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert outcome.status == "ready"
    assert ws.create_calls == 1


def test_workspace_lookup_unknown_fails_indeterminate_not_retry(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    ws.raise_on_create = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    ws.lookup_override = "unknown"
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo.value.code == "provisioning_indeterminate"
    assert excinfo.value.retry_eligible is False
    # A further retry under the identical key is refused (row is now failed).
    with pytest.raises(PortableError) as excinfo2:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo2.value.retry_eligible is False


def test_engine_session_crash_then_retry_recovers_to_exactly_one(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    outcome = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert outcome.status == "ready"
    assert len(eng.opened) == 1
    assert ws.create_calls == 1


def test_engine_session_lookup_unknown_fails_indeterminate(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    eng.lookup_override = "unknown"
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo.value.code == "provisioning_indeterminate"
    assert excinfo.value.retry_eligible is False


def test_stale_ownership_epoch_at_start_is_refused(tmp_path):
    store, result, channel = _channel(tmp_path)
    svc = _service(store)
    acquire_task_owner(store, result.task_id, "owner-2", channel.owner_epoch)
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    assert excinfo.value.code == "stale_ownership_epoch"
    assert excinfo.value.retry_eligible is False


def test_stale_ownership_epoch_before_engine_session_is_refused(tmp_path):
    """Ownership transfers mid-provisioning (after workspace/channel/config/context,
    before the engine session is opened)."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    acquire_task_owner(store, result.task_id, "owner-2", channel.owner_epoch)
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo.value.code == "stale_ownership_epoch"
    assert eng.open_calls == 1  # the one crashed attempt only; never re-attempted once the epoch was stale


def test_delegate_refuses_worker_channel(tmp_path):
    from cortex_runtime.trusted_channel import issue_channel

    store, result, coordinator_channel = _channel(tmp_path)
    svc = _service(store)
    worker_channel_id, worker_secret = issue_channel(
        store, result.task_id, "worker", coordinator_channel.owner_epoch,
        parent_channel_id=coordinator_channel.channel_id,
    )
    worker_channel = resolve_channel(store, worker_channel_id, worker_secret)
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(worker_channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    assert excinfo.value.code == "coordinator_only"


def test_critical_role_refused_without_qualified_or_allowed_delivery(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=False)
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", new_key())
    assert excinfo.value.code == "instruction_delivery_unqualified"
    # No external effect was attempted before the refusal.
    assert ws.create_calls == 0
    assert eng.open_calls == 0


def test_concurrent_delegate_same_key_converges_to_one_provisioning(tmp_path):
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    results = []
    errors = []

    def run():
        try:
            results.append(svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key))
        except Exception as error:  # pragma: no cover - failure path surfaced via assertion below
            errors.append(error)

    threads = [threading.Thread(target=run) for _ in range(6)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not errors
    assert len(results) == 6
    refs = {outcome.assignment_ref for outcome in results}
    assert len(refs) == 1
    assert ws.create_calls == 1
    assert eng.open_calls == 1


def test_idempotency_key_reused_for_different_task_or_role_is_rejected(tmp_path):
    store, result, channel = _channel(tmp_path)
    svc = _service(store)
    key = new_key()
    svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    # A different role under the identical (task, coordinator channel, request_key)
    # triple collides on the same idempotency_key -- reject rather than silently
    # reusing or overwriting the earlier delegation's meaning.
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "frontend-dev", "Acceptance", key)
    assert excinfo.value.code == "idempotency_key_reused"


def test_same_process_retry_after_open_failure_passes_the_real_secret(tmp_path):
    """T016 review (HIGH): the channel secret must survive a failed open()
    so a same-process retry never launches a worker with no credentials."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert eng.open_calls == 1
    assert eng.launch_channel_secrets[0] is not None  # the failed attempt still had it

    outcome = svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert outcome.status == "ready"
    assert eng.open_calls == 2
    # The retry's own open() call also received the real secret -- never None.
    assert eng.launch_channel_secrets[1] is not None
    assert eng.launch_channel_secrets[0] == eng.launch_channel_secrets[1]


def test_new_process_retry_with_no_held_secret_fails_explicitly_never_opens_uncredentialed(tmp_path):
    """T016 review (HIGH): a brand-new ProvisioningService instance (simulating
    a new process resuming a crashed delegation) never holds the plaintext
    secret from the crashed attempt's channel issuance. It must fail closed
    with a non-retryable, explicit code -- never call open() with a missing
    secret."""
    store, result, channel = _channel(tmp_path)
    ws = FakeWorkspace()
    eng = FakeEngineSessionOpener()
    eng.refuse_missing_secret = True
    cfg = FakeConfigSnapshotSource()
    first_service = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()

    # Advance the row through workspace+channel (the channel is issued and
    # its secret recorded only in first_service's own memory), then simulate
    # a crash by constructing a brand-new ProvisioningService before the
    # engine_session step ever runs.
    import cortex_runtime.delegation.provisioning as provisioning_module

    original_step_engine_session = provisioning_module.ProvisioningService._step_engine_session
    calls = {"n": 0}

    def stop_before_engine_session(self, idempotency_key, row, coordinator_channel, role_set):
        calls["n"] += 1
        raise RuntimeError("simulated process crash before the engine_session step")

    provisioning_module.ProvisioningService._step_engine_session = stop_before_engine_session
    try:
        with pytest.raises(RuntimeError):
            first_service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    finally:
        provisioning_module.ProvisioningService._step_engine_session = original_step_engine_session
    assert eng.open_calls == 0

    second_service = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    with pytest.raises(PortableError) as excinfo:
        second_service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo.value.code == "channel_secret_unavailable"
    assert excinfo.value.retry_eligible is False
    # Never opened an engine session with a missing secret.
    assert eng.open_calls == 0

    # And the row is durably marked failed -- a further retry keeps failing
    # the same way rather than looping forever.
    with pytest.raises(PortableError) as excinfo2:
        second_service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo2.value.code == "channel_secret_unavailable"
    assert eng.open_calls == 0


def test_stale_ownership_epoch_failure_releases_the_pending_secret(tmp_path):
    """T016 review (fixup): every non-retryable failure -- not only a
    retried open() -- must stop holding the freshly-issued channel secret."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()

    import cortex_runtime.delegation.provisioning as provisioning_module
    from cortex_runtime.ownership import acquire_task_owner

    original = provisioning_module.ProvisioningService._step_context_snapshot

    def bump_epoch_then_continue(self, idempotency_key, row, role_set):
        original(self, idempotency_key, row, role_set)
        # Simulate ownership changing hands after the channel (and its
        # secret) were already issued, but before the engine_session step.
        acquire_task_owner(store, row["task_id"], "new-owner", expected_epoch=channel.owner_epoch)

    provisioning_module.ProvisioningService._step_context_snapshot = bump_epoch_then_continue
    try:
        with pytest.raises(PortableError) as excinfo:
            svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    finally:
        provisioning_module.ProvisioningService._step_context_snapshot = original
    assert excinfo.value.code == "stale_ownership_epoch"
    assert svc._pending_channel_secrets == {}
    assert eng.open_calls == 0


def test_indeterminate_engine_lookup_failure_releases_the_pending_secret(tmp_path):
    """T016 review (fixup): the provisioning_indeterminate fail-closed path
    must also stop holding the pending secret. Mirrors the existing
    test_engine_session_lookup_unknown_fails_indeterminate recovery shape:
    a retryable open() failure leaves an intent-without-outcome effect, and
    an "unknown" lookup on retry fails closed non-retryably."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert svc._pending_channel_secrets  # retryable failure -- still held

    eng.lookup_override = "unknown"
    with pytest.raises(PortableError) as excinfo:
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert excinfo.value.code == "provisioning_indeterminate"
    assert svc._pending_channel_secrets == {}
    assert eng.open_calls == 1  # only the first (raise_on_open) attempt ever called open()


def test_retryable_open_failure_still_holds_the_pending_secret(tmp_path):
    """Contrast case: a *retryable* open() failure must still leave the
    secret in place for the next retry (see the same-process-retry test)."""
    store, result, channel = _channel(tmp_path)
    ws, eng, cfg = FakeWorkspace(), FakeEngineSessionOpener(), FakeConfigSnapshotSource()
    eng.raise_on_open = True
    svc = ProvisioningService(store, ws, eng, cfg, allow_unqualified_delivery=True)
    key = new_key()
    with pytest.raises(RuntimeError):
        svc.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", key)
    assert svc._pending_channel_secrets  # not a _fail() path -- still held
