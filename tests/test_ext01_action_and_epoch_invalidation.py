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


import pytest


@pytest.mark.parametrize('method',['abandon','cancel'])
def test_manager_invalidates_pending_action_without_disabling_fresh_gateway(tmp_path,method):
    manager=_bare_manager(tmp_path)
    manager.correlation.register_session('es_a','w1')
    _bind(manager,'es_a',tmp_path)
    bridge=PermissionBridge(correlation=manager.correlation,bindings=manager.bindings)
    manager._permission_bridge=bridge
    action=manager.correlation.portable_tool_call_ref('w1','tc-1')
    bridge._track_action_start('w1',action)
    if method=='abandon':manager._abandon_turn('es_a','w1',_closed_generator())
    else:manager.cancel('es_a')
    def ask(ref):
        params=_params(ref);params['toolCall']['name']='mcp__cortex__project_read'
        return bridge.handle(engine_session_id='es_a',wire_session_id='w1',wire_request_id=ref,params=params)
    assert ask('tc-1')==CANCELLED
    assert ask('tc-2')==ALLOWED
    bridge._track_action_end('w1',action)


def test_adapter_owner_epoch_invalidation_fences_old_binding(tmp_path):
    manager=_bare_manager(tmp_path);manager.correlation.register_session('es_a','w1')
    _bind(manager,'es_a',tmp_path)
    adapter=EngineSessionManagerAdapter(lambda _:None,lambda _:str(tmp_path))
    adapter._managers['att1']=manager
    adapter._sessions['att1']='es_a'
    adapter.invalidate_owner_epoch('att1','epoch_2')
    bridge=PermissionBridge(correlation=manager.correlation,bindings=manager.bindings)
    params=_params('tc-1');params['toolCall']['name']='mcp__cortex__project_read'
    assert bridge.handle(engine_session_id='es_a',wire_session_id='w1',wire_request_id=1,params=params)==CANCELLED
    _bind(manager,'es_a',tmp_path,ownership_epoch='epoch_2')
    assert bridge.handle(engine_session_id='es_a',wire_session_id='w1',wire_request_id=2,params=params)==ALLOWED


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
