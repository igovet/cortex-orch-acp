"""T035 EXT-01 remaining findings ("Ownership and action invalidation still
unwired" -- see ``.cortex/acp-migration/reviews/EXT-01-independent-
production-chain-review.md``'s own outcome table): both
``engine.resource_binding.BindingRegistry.invalidate_epoch`` and
``invalidate_action`` are now wired to real production call sites --
``delegation.live_adapters.EngineSessionManagerAdapter.invalidate_owner_epoch``
(ownership transfer) and
``engine.session_manager.EngineSessionManager._invalidate_actions_pending_for_wire_session``
(called from ``_abandon_turn``/``cancel`` -- action supersession).

Both invalidation tests below share the exact shape
``test_ext01_generation_binding_race.py``'s own generation-bump test uses:
a stale reply (its host round trip already outstanding when the
invalidating event lands) is refused, and a brand new request made *after*
that event is served normally -- refusing the wrong thing (a fresh request
made after the fact) is exactly the class of bug this class of fix must not
reintroduce (see that file's own module docstring / T035 EXT-01 #3).

The final test asserts the listener-vs-binding window
``engine.session_manager.EngineSessionManager._ensure_started``'s own
docstring describes: a real fake-engine subprocess, a real ``session/new``
round trip, and a permission request dispatched for that session before
anything ever calls ``bind_resource`` -- proving the window's only
observable effect is a refusal, never a silent approval.
"""
from __future__ import annotations

import threading
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.permissions import PermissionBridge
from cortex_runtime.engine.resource_binding import ResourceBinding
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.ownership import acquire_task_owner_tx, current_owner_epoch
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

CANCELLED = {"outcome": {"outcome": "cancelled"}}
ALLOWED = {"outcome": {"outcome": "selected", "optionId": "allow"}}


def _params(tool_call_id: str) -> dict:
    return dict(
        toolCall=dict(toolCallId=tool_call_id),
        options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
    )


class _FakeLogs:
    def observe(self, event, **fields) -> None:
        pass


class _FakeProcess:
    """Stands in for ``process.EngineProcess`` -- only ``.logs.observe``
    is ever touched by the methods exercised here (``_abandon_turn``/
    ``cancel_wire``), never a real subprocess."""

    def __init__(self):
        self.logs = _FakeLogs()


def _bare_manager(tmp_path) -> EngineSessionManager:
    """A manager with no real subprocess -- every method exercised below
    (``bind_resource``/``correlation.register_session``/``cancel``/
    ``_abandon_turn``) works without one; ``cancel_wire`` tolerates
    ``self._client is None`` (best-effort, see its own ``except
    Exception``); ``_process`` is a minimal fake since both methods
    observe through it."""
    manager = EngineSessionManager(object(), str(tmp_path))
    manager._process = _FakeProcess()
    return manager


def _bind(manager, session_id, workspace_root, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id=session_id, context_generation_id="gen_1",
        assignment_id="a1", attempt_id="att1", workspace_binding_id="wb1",
        workspace_root=str(workspace_root), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    manager.bind_resource(ResourceBinding(**fields))


def _closed_generator():
    """A trivial real generator, so ``turn_gen.close()`` inside
    ``_abandon_turn`` has something realistic to call."""
    yield


def test_abandoned_turn_invalidates_only_its_own_pending_permission(tmp_path):
    manager = _bare_manager(tmp_path)
    session_id, wire_session_id = "es_a", "w1"
    manager.correlation.register_session(session_id, wire_session_id)
    _bind(manager, session_id, tmp_path)

    started = threading.Event()
    release = threading.Event()

    def blocking_then_allow(engine_session_id, params, timeout):
        started.set()
        release.wait(timeout=5.0)
        return dict(ALLOWED)

    manager._permission_bridge = PermissionBridge(
        correlation=manager.correlation, bindings=manager.bindings,
        host_permission_requester=blocking_then_allow, call_timeout=5.0,
    )

    results: list = []

    def run_stale_request():
        results.append(
            manager._permission_bridge.handle(
                engine_session_id=session_id, wire_session_id=wire_session_id,
                wire_request_id=1, params=_params("tc-1"),
            )
        )

    worker = threading.Thread(target=run_stale_request)
    worker.start()
    assert started.wait(timeout=5.0)

    # The turn this pending "tc-1" request belongs to is abandoned mid-
    # stream (F1/T012's own generator-closed-mid-stream case) while its
    # host round trip is still outstanding.
    manager._abandon_turn(session_id, wire_session_id, _closed_generator())

    release.set()
    worker.join(timeout=5.0)
    assert results == [CANCELLED]

    # A brand new request for a *different* tool call (the next turn on
    # the same wire session) must not be caught by the same invalidation.
    manager._permission_bridge = PermissionBridge(
        correlation=manager.correlation, bindings=manager.bindings,
        host_permission_requester=lambda *a: dict(ALLOWED), call_timeout=5.0,
    )
    fresh = manager._permission_bridge.handle(
        engine_session_id=session_id, wire_session_id=wire_session_id,
        wire_request_id=2, params=_params("tc-2"),
    )
    assert fresh == ALLOWED


def test_explicit_cancel_invalidates_its_sessions_pending_permission(tmp_path):
    """Same shape as the abandonment test above, but through
    ``EngineSessionManager.cancel`` -- the entry point EXT-01 #4's own
    independent cancellation caller (``delegation.live_adapters
    .EngineSessionManagerAdapter.prompt``) actually uses."""
    manager = _bare_manager(tmp_path)
    session_id, wire_session_id = "es_b", "w2"
    manager.correlation.register_session(session_id, wire_session_id)
    _bind(manager, session_id, tmp_path)

    started = threading.Event()
    release = threading.Event()

    def blocking_then_allow(engine_session_id, params, timeout):
        started.set()
        release.wait(timeout=5.0)
        return dict(ALLOWED)

    manager._permission_bridge = PermissionBridge(
        correlation=manager.correlation, bindings=manager.bindings,
        host_permission_requester=blocking_then_allow, call_timeout=5.0,
    )

    results: list = []
    worker = threading.Thread(
        target=lambda: results.append(
            manager._permission_bridge.handle(
                engine_session_id=session_id, wire_session_id=wire_session_id,
                wire_request_id=1, params=_params("tc-1"),
            )
        )
    )
    worker.start()
    assert started.wait(timeout=5.0)

    manager.cancel(session_id)

    release.set()
    worker.join(timeout=5.0)
    assert results == [CANCELLED]


def test_owner_epoch_transfer_rejects_stale_reply_but_serves_a_fresh_request(tmp_path):
    """``EngineSessionManagerAdapter.invalidate_owner_epoch`` against a
    *real* ``ownership.acquire_task_owner_tx`` transfer (never a hand-typed
    epoch string) -- the actual mechanism ``recovery.coordinator_recovery``
    uses, resolved by this test through the adapter's own attempt_id ->
    manager -> session -> binding chain exactly like production would."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build it.", "owner-1")
    task_id = result.task_id
    with store.connection() as db:
        epoch_1 = current_owner_epoch(db, task_id)

    manager = _bare_manager(tmp_path)
    session_id, wire_session_id, attempt_id = "es_c", "w3", "att_owner"
    manager.correlation.register_session(session_id, wire_session_id)
    _bind(manager, session_id, tmp_path, attempt_id=attempt_id, ownership_epoch=str(epoch_1))

    adapter = EngineSessionManagerAdapter(
        connection_record_factory=lambda t: (_ for _ in ()).throw(AssertionError("not exercised")),
        workspace_root_factory=lambda t: (_ for _ in ()).throw(AssertionError("not exercised")),
        store=store,
    )
    adapter._managers[attempt_id] = manager
    adapter._sessions[attempt_id] = session_id

    new_owner_holder: dict = {}

    def late_requester_during_real_transfer(engine_session_id, params, timeout):
        # The host round trip for this request is still outstanding when a
        # real ownership transfer (the same ``acquire_task_owner_tx``
        # ``recovery.coordinator_recovery`` calls) commits underneath it.
        with store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                new_owner = acquire_task_owner_tx(db, task_id, "recovery:owner2", epoch_1)
                db.commit()
            except BaseException:
                db.rollback()
                raise
        new_owner_holder["epoch"] = new_owner.epoch
        adapter.invalidate_owner_epoch(attempt_id, new_owner.epoch)
        return dict(ALLOWED)

    stale_bridge = PermissionBridge(
        correlation=manager.correlation, bindings=manager.bindings,
        host_permission_requester=late_requester_during_real_transfer, call_timeout=5.0,
    )
    stale_result = stale_bridge.handle(
        engine_session_id=session_id, wire_session_id=wire_session_id, wire_request_id=1, params=_params("tc-1"),
    )
    assert stale_result == CANCELLED

    # Unlike ``invalidate_generation`` (which atomically swaps in a fresh
    # *current* binding for the bumped session -- T035 EXT-01 #3),
    # ``invalidate_epoch`` never replaces the old attempt's own binding:
    # this exact old binding is correctly refused forever now, by design
    # (it belongs to the attempt recovery just replaced). "a fresh request
    # under the new epoch works" is instead proven the way production
    # actually gets a fresh, current epoch -- a *new* session/binding for
    # the replacement attempt (``EngineSessionManagerAdapter._bind_resource``
    # reads ``ownership.current_owner_epoch`` fresh at bind time), bound
    # here exactly as recovery's own replacement session would be.
    stale_again = stale_bridge.handle(
        engine_session_id=session_id, wire_session_id=wire_session_id, wire_request_id=2, params=_params("tc-2"),
    )
    assert stale_again == CANCELLED

    new_manager = _bare_manager(tmp_path)
    new_session_id, new_wire_session_id = "es_c2", "w4"
    new_manager.correlation.register_session(new_session_id, new_wire_session_id)
    _bind(
        new_manager, new_session_id, tmp_path, attempt_id="att_owner_2",
        ownership_epoch=str(new_owner_holder["epoch"]),
    )
    fresh_bridge = PermissionBridge(
        correlation=new_manager.correlation, bindings=new_manager.bindings,
        host_permission_requester=lambda *a: dict(ALLOWED), call_timeout=5.0,
    )
    fresh_result = fresh_bridge.handle(
        engine_session_id=new_session_id, wire_session_id=new_wire_session_id, wire_request_id=1,
        params=_params("tc-3"),
    )
    assert fresh_result == ALLOWED


def test_invalidate_owner_epoch_is_a_no_op_with_no_live_manager_for_the_attempt(tmp_path):
    """No live manager for ``attempt_id`` in this process (already
    terminated, or opened by a different process before a restart) -- a
    no-op, never a raise (T023's own "no manager -> fall back to durable
    state, never crash" convention)."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    adapter = EngineSessionManagerAdapter(
        connection_record_factory=lambda t: (_ for _ in ()).throw(AssertionError("not exercised")),
        workspace_root_factory=lambda t: (_ for _ in ()).throw(AssertionError("not exercised")),
        store=store,
    )
    adapter.invalidate_owner_epoch("no_such_attempt", 7)


REQUESTED_CAPABILITIES = (
    EngineCapability.SESSION_CREATION,
    EngineCapability.PROMPT_STREAMING,
    EngineCapability.CANCELLATION,
)


def _make_wired_manager(tmp_path, scenario) -> EngineSessionManager:
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(record, str(tmp_path), environ=environ, call_timeout=5.0)


def test_listener_window_before_bind_resource_fails_closed_never_open(tmp_path):
    """``EngineSessionManager._ensure_started``'s own docstring: the
    client-request listener (started by ``negotiate()``) is live, and a
    real ``session/new`` round trip has already registered this session
    id, but nothing has called ``bind_resource`` for it yet -- exactly the
    window between that call and
    ``delegation.live_adapters.EngineSessionManagerAdapter._bind_resource``
    running later in ``open()``. A permission request dispatched for it
    right now must be refused, never approved and never hang."""
    manager = _make_wired_manager(tmp_path, default_scenario())
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24)
        wire_session_id = manager.correlation.wire_session_id(session_id)
        assert wire_session_id is not None
        # Deliberately no manager.bind_resource(...) call here -- this test
        # IS the window.
        result = manager._permission_bridge.handle(
            engine_session_id=session_id, wire_session_id=wire_session_id, wire_request_id=1,
            params=_params("tc-1"),
        )
        assert result == CANCELLED
    finally:
        manager.shutdown(grace=2.0)
