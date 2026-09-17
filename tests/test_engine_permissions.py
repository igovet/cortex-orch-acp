"""PermissionBridge + BindingRegistry: correlation, invalidation, and
schema-valid outcome shaping (T014). Unit-level (no subprocess) -- see
``test_engine_resource_dispatch.py`` for the end-to-end wire-level cases."""
from __future__ import annotations

import threading

import pytest

from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import DuplicateEnginePermissionRequestId, PermissionBridge
from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding, ResourceBindingError


def _binding(tmp_path, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
        workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def _bridge(*, host_permission_requester=None, correlation=None, bindings=None):
    return PermissionBridge(
        correlation=correlation or CorrelationTables(), bindings=bindings or BindingRegistry(),
        host_permission_requester=host_permission_requester, call_timeout=1.0,
    )


PARAMS = dict(
    toolCall=dict(toolCallId="tc-1"),
    options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
)


def test_binding_requires_every_field_nonempty(tmp_path):
    with pytest.raises(ResourceBindingError):
        ResourceBinding(
            engine_instance_id="", engine_session_id="es_a", context_generation_id="gen_1",
            assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
            workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
        )


def test_binding_requires_resolved_absolute_workspace_root(tmp_path):
    with pytest.raises(ResourceBindingError):
        _binding(tmp_path, workspace_root=str(tmp_path) + "/../" + tmp_path.name)


def test_no_binding_for_session_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bridge = _bridge(bindings=bindings)
    result = bridge.handle(engine_session_id="es_missing", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_no_host_requester_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_host_selects_offered_option_is_relayed(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_host_selects_unknown_option_id_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "not-offered"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_duplicate_wire_request_id_errors(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(2.0)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=slow_requester)
    results = []

    def first():
        results.append(bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=42, params=PARAMS))

    worker = threading.Thread(target=first)
    worker.start()
    import time
    time.sleep(0.05)
    with pytest.raises(DuplicateEnginePermissionRequestId):
        bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=42, params=PARAMS)
    release_event.set()
    worker.join(timeout=2.0)
    assert results == [{"outcome": {"outcome": "selected", "optionId": "allow"}}]


def test_session_invalidated_while_host_deciding_never_relays_selected(tmp_path):
    """The invalidation happens *during* the host round-trip (simulated by
    the host_permission_requester callback itself invalidating the
    session) -- the bridge must re-check binding currency after the host
    replies, not only before calling it, and this is exactly the
    'late_permission_response' path."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))

    def requester(engine_session_id, params, timeout):
        bindings.invalidate_session(engine_session_id, "session_replaced")
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_epoch_change_invalidates_pending_permission(tmp_path):
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)
    bindings.invalidate_epoch(binding.workspace_binding_id, "epoch_2")
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_generation_change_invalidates_pending_permission(tmp_path):
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)
    bindings.invalidate_generation(binding.engine_session_id, "gen_2")
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_action_supersession_invalidates_only_that_action(tmp_path):
    correlation = CorrelationTables()
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    # Pre-allocate the portable action ref exactly as the bridge would.
    action_ref = correlation.portable_tool_call_ref("w1", "tc-1")
    bindings.invalidate_action("es_a", action_ref)
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, correlation=correlation, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}
    # A *different* tool call (never superseded) on the same session/binding
    # is unaffected.
    other_params = dict(PARAMS, toolCall=dict(toolCallId="tc-2"))
    result2 = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=other_params)
    assert result2 == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_host_cancelled_outcome_is_passed_through(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "cancelled"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_host_timeout_or_error_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))

    def raising_requester(engine_session_id, params, timeout):
        raise TimeoutError("host_timeout")

    bridge = _bridge(bindings=bindings, host_permission_requester=raising_requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_worker_a_binding_never_visible_to_worker_b_engine_session(tmp_path):
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path / "a", engine_session_id="es_a", workspace_binding_id="wb_a"))
    bindings.bind(_binding(tmp_path / "b", engine_session_id="es_b", workspace_binding_id="wb_b"))
    binding_a = bindings.get("es_a")
    binding_b = bindings.get("es_b")
    assert binding_a.workspace_root != binding_b.workspace_root
    # Invalidating B's epoch never affects A's snapshot.
    bindings.invalidate_epoch("wb_b", "epoch_2")
    snapshot_a = bindings.snapshot(binding_a)
    assert binding_a.is_current(snapshot_a)


def test_allow_always_never_persists_a_standing_rule(tmp_path):
    """Two calls for the *same* toolCall id, second with a fresh binding
    (simulating a brand new action) -- an 'allow_always' selection on the
    first must never silently apply to the second without the host being
    asked again."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow_always"}}

    params_with_always = dict(PARAMS, options=[dict(optionId="allow_always", kind="allow_always")])
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    first = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params_with_always)
    second = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=params_with_always)
    assert first == {"outcome": {"outcome": "selected", "optionId": "allow_always"}}
    assert second == {"outcome": {"outcome": "selected", "optionId": "allow_always"}}
    # The host requester was consulted for *both* -- nothing was decided
    # locally from a remembered "always" choice.
    assert len(calls) == 2
