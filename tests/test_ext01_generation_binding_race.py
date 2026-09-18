"""T035 EXT-01 #3 (independent review finding): a context-generation bump
must not break a *new* permission request made after it -- only a request
whose answer is still straddling the bump (a genuinely stale reply) may be
refused. Two independent mechanisms, both exercised here:

1. ``engine.resource_binding.BindingRegistry.invalidate_generation`` must
   atomically install a fresh *current* binding for the bumped session
   (see ``ResourceBinding.is_current``/``PermissionBridge._handle_one``),
   not just bump a counter the pre-existing (now stale) stored binding
   object never sees.
2. ``delegation.live_adapters.EngineSessionManagerAdapter
   .mark_delivery_confirmed`` must be a conditional transition on
   ``_pending_generation``, not an unconditional delete keyed only by
   ``session_ref`` -- confirming an older generation's delivery must never
   erase a *newer* generation's still-outstanding delivery requirement.
"""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import deliveries as delegation_deliveries
from cortex_runtime.delegation import generations as delegation_generations
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import PermissionBridge
from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

PARAMS = dict(
    toolCall=dict(toolCallId="tc-1"),
    options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
)


def _binding(tmp_path, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
        workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def test_generation_bump_rejects_stale_g1_reply_but_serves_a_new_g2_request(tmp_path):
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)

    # The host round-trip for this request is still outstanding when a
    # compaction lands mid-flight and bumps the generation -- the reply
    # that then arrives belongs to the now-superseded G1 and must be
    # refused.
    def late_g1_requester(engine_session_id, params, timeout):
        bindings.invalidate_generation(binding.engine_session_id, "gen_2")
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    stale_bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=late_g1_requester, call_timeout=1.0,
    )
    stale_result = stale_bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=dict(PARAMS, toolCall=dict(toolCallId="tc-1")),
    )
    assert stale_result == {"outcome": {"outcome": "cancelled"}}

    # A brand new request made *after* the bump (generation is now G2)
    # must be served normally -- it is not itself stale.
    fresh_requester = lambda engine_session_id, params, timeout: {  # noqa: E731
        "outcome": {"outcome": "selected", "optionId": "allow"},
    }
    fresh_bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=fresh_requester, call_timeout=1.0,
    )
    fresh_result = fresh_bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=2,
        params=dict(PARAMS, toolCall=dict(toolCallId="tc-2")),
    )
    assert fresh_result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_confirming_g1_delivery_does_not_clear_a_pending_g2_delivery(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build it.", "owner-1")
    task_id = result.task_id
    session_ref = "es_1"

    delegation_generations.ensure_owner(
        store, task_id=task_id, engine_session_ref=session_ref, engine="codex-acp", version="1.12.0",
    )
    g1 = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref=session_ref, recipient="coordinator",
        cause="session_open", evidence_source="test",
    )
    g2 = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref=session_ref, evidence_id="tc-1", phase="completed",
        evidence_source="test", recipient="coordinator",
    )
    assert g1["id"] != g2["id"]

    adapter = EngineSessionManagerAdapter(
        connection_record_factory=lambda task_id: (_ for _ in ()).throw(AssertionError("not exercised")),
        workspace_root_factory=lambda task_id: (_ for _ in ()).throw(AssertionError("not exercised")),
        store=store,
    )
    adapter._session_meta[session_ref] = dict(
        task_id=task_id, recipient="coordinator", assignment_ref=None, attempt_id="att_1", tier="automatic",
    )

    # G1's delivery was built and transmitted before the compaction opened
    # G2. G2's delivery is now the pending requirement on this session
    # (mirrors ``_observe_compaction_signal`` recording the fresh
    # generation as pending the instant it is opened).
    d1 = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=g1["id"], capsule_revision="rev-1",
        task_id=task_id, engine_session_ref=session_ref, evidence_source="test",
    )
    delegation_deliveries.mark_transmitted(store, d1["id"], "test")
    adapter._pending_generation[session_ref] = g2

    # G1's late-arriving confirmation must not erase G2's pending
    # requirement.
    adapter.mark_delivery_confirmed(session_ref, d1["id"])

    assert delegation_deliveries.get(store, d1["id"])["state"] == "delivery_confirmed"
    assert adapter.pending_delivery(session_ref) is not None
    assert adapter.pending_delivery(session_ref)["generation"]["id"] == g2["id"]

    # G2's own confirmation, once it actually happens, does clear it.
    d2 = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=g2["id"], capsule_revision="rev-2",
        task_id=task_id, engine_session_ref=session_ref, evidence_source="test",
    )
    delegation_deliveries.mark_transmitted(store, d2["id"], "test")
    adapter.mark_delivery_confirmed(session_ref, d2["id"])
    assert adapter.pending_delivery(session_ref) is None
