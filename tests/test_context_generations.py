"""T022: context generations and qualified recovery delivery.

Three layers, no real engine subprocess for the OrchestratingRuntime-level
cases (fakes, matching test_orchestrating_runtime.py's own convention) plus
one real-subprocess-level case for the session_manager.py wire translation
itself (matching test_engine_session_manager.py's own fake-engine
convention -- see ``engine_fakes``/``fake_engine_process.py``).
"""
from __future__ import annotations

import json
import threading
import uuid

import pytest

from cortex_runtime.acp import orchestrating_runtime
from cortex_runtime.acp.orchestrating_runtime import COORDINATOR_ROLE_ID, OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import deliveries as delegation_deliveries
from cortex_runtime.delegation import generations as delegation_generations
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability, EngineEventKind, StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import load_role
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace

REQUESTED_CAPABILITIES = (
    EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING, EngineCapability.CANCELLATION,
)


# --------------------------------------------------------------------
# 1. session_manager.py wire translation (real fake-engine subprocess)
# --------------------------------------------------------------------


def _make_manager(tmp_path, scenario):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(record, str(tmp_path), environ=environ, call_timeout=5.0)


def test_compaction_tool_call_meta_translates_to_compaction_signal(tmp_path):
    """codex-acp's/claude-agent-acp's shared legacy presentation (a
    tool_call/tool_call_update pair carrying `_meta.contextCompaction`) is
    translated to a distinct COMPACTION_SIGNAL, never an ordinary
    TOOL_CALL_OBSERVED, with a stable evidence id (tool_call_ref) shared
    across the start/update pair."""
    scenario = default_scenario(
        prompt_updates=[
            dict(sessionUpdate="tool_call", toolCallId="compact-1", kind="think", title="Compact conversation",
                 status="in_progress", _meta=dict(contextCompaction=dict(version=1))),
            dict(sessionUpdate="tool_call_update", toolCallId="compact-1", status="completed",
                 _meta=dict(contextCompaction=dict(version=1))),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="done")),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
        signals = [e for e in events if e.kind == EngineEventKind.COMPACTION_SIGNAL]
        tool_observed = [e for e in events if e.kind == EngineEventKind.TOOL_CALL_OBSERVED]
        assert len(signals) == 2
        assert signals[0].compaction_reason == "in_progress"
        assert signals[1].compaction_reason == "completed"
        assert signals[0].tool_call_ref == signals[1].tool_call_ref
        assert signals[0].tool_call_ref is not None
        assert tool_observed == []  # never double-reported as ordinary tool evidence
    finally:
        manager.shutdown(grace=2.0)


def test_missing_meta_tool_call_is_ordinary_tool_evidence_not_a_compaction_signal(tmp_path):
    """A plain tool call with no `_meta.contextCompaction` is unaffected --
    the "missing compaction event" case at the wire-translation layer."""
    scenario = default_scenario(
        prompt_updates=[
            dict(sessionUpdate="tool_call", toolCallId="tc-1", kind="execute", title="ls", status="completed"),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="done")),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
        assert not any(e.kind == EngineEventKind.COMPACTION_SIGNAL for e in events)
        assert any(e.kind == EngineEventKind.TOOL_CALL_OBSERVED for e in events)
    finally:
        manager.shutdown(grace=2.0)


# --------------------------------------------------------------------
# 2. delegation.generations / delegation.deliveries unit coverage
# --------------------------------------------------------------------


def _store(tmp_path):
    return Store(tmp_path / ".cortex", project_root=tmp_path)


def _task(store):
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build it.", "owner-1")
    return result.task_id


def test_tier_for_pinned_adapters_and_unknown_engine(tmp_path):
    codex = delegation_generations.tier_for("codex-acp", "1.12.0")
    assert codex.tier == delegation_generations.AUTOMATIC
    claude = delegation_generations.tier_for("claude-agent-acp", "0.78.0")
    assert claude.tier == delegation_generations.AUTOMATIC
    claude_old = delegation_generations.tier_for("claude-agent-acp", "0.77.0")
    assert claude_old.tier == delegation_generations.AUTOMATIC
    claude_wild = delegation_generations.tier_for("claude-agent-acp", "9.9.9")
    assert claude_wild.tier == delegation_generations.AUTOMATIC  # wildcard evidence
    codex_other_version = delegation_generations.tier_for("codex-acp", "1.0.0")
    assert codex_other_version.tier == delegation_generations.CONTROLLED_BOUNDARY
    assert "controlled boundaries" in codex_other_version.limitation
    unknown = delegation_generations.tier_for("some-other-engine", "1.0")
    assert unknown.tier == delegation_generations.UNQUALIFIED


def test_ensure_owner_is_single_and_idempotent_per_session(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    first = delegation_generations.ensure_owner(
        store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0",
    )
    assert first.tier == "automatic"
    row = delegation_generations.owner_row(store, "es_1")
    assert row["owner"] == "engine_adapter"
    # A second call for the same session (even with a different engine
    # name -- should never happen in practice, but proves "exactly one
    # compaction owner per session" holds regardless) is a read-only no-op.
    second = delegation_generations.ensure_owner(
        store, task_id=task_id, engine_session_ref="es_1", engine="claude-agent-acp", version="0.78.0",
    )
    assert second.engine == "codex-acp"
    assert second.tier == "automatic"


def test_controlled_boundary_reports_exact_limitation(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    tier = delegation_generations.ensure_owner(
        store, task_id=task_id, engine_session_ref="es_cb", engine="codex-acp", version="0.1.0",
    )
    assert tier.tier == delegation_generations.CONTROLLED_BOUNDARY
    assert tier.limitation == delegation_generations._CONTROLLED_BOUNDARY_LIMITATION
    row = delegation_generations.owner_row(store, "es_cb")
    assert row["owner"] == "cortex_controlled"
    assert row["limitation"] == delegation_generations._CONTROLLED_BOUNDARY_LIMITATION


def test_duplicate_compaction_evidence_id_opens_at_most_one_generation(tmp_path):
    """Single compaction owner + duplicate triggers bounded: a
    re-delivered/duplicated terminal event for the exact same evidence id
    never opens a second generation."""
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator",
        cause="session_open", evidence_source="test",
    )
    first = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref="es_1", evidence_id="tc-1", phase="in_progress",
        evidence_source="test", recipient="coordinator",
    )
    assert first is None  # non-terminal: recorded, no generation yet
    second = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref="es_1", evidence_id="tc-1", phase="completed",
        evidence_source="test", recipient="coordinator",
    )
    assert second is not None
    assert second["sequence"] == 2
    duplicate = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref="es_1", evidence_id="tc-1", phase="completed",
        evidence_source="test", recipient="coordinator",
    )
    assert duplicate is None
    assert len(delegation_generations.generations_for_session(store, "es_1")) == 2
    assert delegation_generations.compaction_event_count(store, "es_1") == 1


def test_two_compactions_without_state_change_open_two_generations_and_permit_redelivery(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    g1 = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator", cause="session_open", evidence_source="test",
    )
    g2 = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref="es_1", evidence_id="tc-1", phase="completed",
        evidence_source="test", recipient="coordinator",
    )
    g3 = delegation_generations.record_compaction_event(
        store, task_id=task_id, engine_session_ref="es_1", evidence_id="tc-2", phase="completed",
        evidence_source="test", recipient="coordinator",
    )
    assert len({g1["id"], g2["id"], g3["id"]}) == 3
    capsule_revision = "same-revision"  # unchanged task state -> identical logical snapshot
    d1 = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=g2["id"], capsule_revision=capsule_revision,
        task_id=task_id, engine_session_ref="es_1", evidence_source="test",
    )
    d2 = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=g3["id"], capsule_revision=capsule_revision,
        task_id=task_id, engine_session_ref="es_1", evidence_source="test",
    )
    assert d1["id"] != d2["id"]
    assert d1["capsule_revision"] == d2["capsule_revision"] == capsule_revision


def test_delivery_state_machine_never_confirms_from_model_text(tmp_path):
    """Delivery-state records never infer confirmation from model
    self-report: nothing about a PromptOutcome's text is ever consulted by
    the ledger -- only explicit evidence-bearing calls advance it."""
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    generation = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator", cause="session_open", evidence_source="test",
    )
    delivery = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=generation["id"], capsule_revision="rev",
        task_id=task_id, engine_session_ref="es_1", evidence_source="test",
    )
    # A model claiming "I remember everything now" (arbitrary text) never
    # touches this ledger at all -- state stays 'built' until an explicit
    # technical-channel call is made.
    assert delegation_deliveries.get(store, delivery["id"])["state"] == "built"
    assert delegation_deliveries.mark_delivery_confirmed(store, delivery["id"], "test") is None  # wrong prior state
    delegation_deliveries.mark_transmitted(store, delivery["id"], "test")
    assert delegation_deliveries.get(store, delivery["id"])["state"] == "transmitted"
    confirmed = delegation_deliveries.mark_delivery_confirmed(store, delivery["id"], "session_prompt_response_received")
    assert confirmed["state"] == "delivery_confirmed"


def test_receipt_timeout_stays_transmitted_with_uncertainty(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    generation = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator", cause="session_open", evidence_source="test",
    )
    delivery = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=generation["id"], capsule_revision="rev",
        task_id=task_id, engine_session_ref="es_1", evidence_source="test",
    )
    delegation_deliveries.mark_transmitted(store, delivery["id"], "test")
    delegation_deliveries.mark_receipt_uncertain(store, delivery["id"], "engine_turn_failed:timeout_alive")
    row = delegation_deliveries.get(store, delivery["id"])
    assert row["state"] == "transmitted"
    assert row["uncertainty"] == "engine_turn_failed:timeout_alive"


def test_stale_session_deliveries_are_superseded_never_confirmed(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    generation = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator", cause="session_open", evidence_source="test",
    )
    delivery = delegation_deliveries.record_built(
        store, recipient="coordinator", context_generation_id=generation["id"], capsule_revision="rev",
        task_id=task_id, engine_session_ref="es_1", evidence_source="test",
    )
    delegation_deliveries.mark_transmitted(store, delivery["id"], "test")
    superseded = delegation_deliveries.supersede_session_deliveries(store, "es_1", "session_released")
    assert len(superseded) == 1
    assert superseded[0]["state"] == "superseded"
    assert delegation_deliveries.mark_delivery_confirmed(store, delivery["id"], "test") is None


def test_delivery_retry_bound_is_enforced(tmp_path):
    store = _store(tmp_path)
    task_id = _task(store)
    delegation_generations.ensure_owner(store, task_id=task_id, engine_session_ref="es_1", engine="codex-acp", version="1.12.0")
    generation = delegation_generations.open_generation(
        store, task_id=task_id, engine_session_ref="es_1", recipient="coordinator", cause="session_open", evidence_source="test",
    )
    delivery_id = None
    for _ in range(delegation_deliveries.MAX_DELIVERY_RETRIES + 2):
        delivery = delegation_deliveries.record_built(
            store, recipient="coordinator", context_generation_id=generation["id"], capsule_revision="oversize",
            task_id=task_id, engine_session_ref="es_1", evidence_source="test",
        )
        delivery_id = delivery["id"]
        delegation_deliveries.mark_blocked(store, delivery["id"], "capsule_oversize", "test")
    final = delegation_deliveries.get(store, delivery_id)
    assert final["state"] == "blocked"
    assert final["retry_count"] == delegation_deliveries.MAX_DELIVERY_RETRIES


# --------------------------------------------------------------------
# 3. end-to-end via OrchestratingRuntime + generation-aware fake engine
# --------------------------------------------------------------------


class GenerationAwareSessionRuntime:
    """Wraps ``FakeSessionRuntime``, adding the same T022 generation/
    delivery-tracking surface ``delegation.live_adapters.
    EngineSessionManagerAdapter`` offers -- backed by the *real*
    ``delegation.generations``/``delegation.deliveries`` modules (never a
    re-implementation), so ``acp.orchestrating_runtime``'s own T022 wiring
    is exercised against real ledger state."""

    def __init__(self, store, router, *, engine="codex-acp", version="1.12.0"):
        self._store = store
        self._inner = FakeSessionRuntime(store, router)
        self.engine_name = engine
        self.version = version
        self._ever_opened_attempts: set[str] = set()
        self._pending_generation: dict[str, dict] = {}
        self._session_meta: dict[str, dict] = {}

    @property
    def prompt_calls(self):
        return self._inner.prompt_calls

    def open(self, workspace_binding_id, launch, instructions):
        session_ref = self._inner.open(workspace_binding_id, launch, instructions)
        attempt_id = launch.get("attempt_id")
        task_id = launch["task_id"]
        replacement = attempt_id in self._ever_opened_attempts
        self._ever_opened_attempts.add(attempt_id)
        recipient = "worker" if launch.get("assignment_id") else "coordinator"
        assignment_ref = None
        if recipient == "worker":
            # See delegation.live_adapters.EngineSessionManagerAdapter.
            # _open_generation: launch["assignment_id"] is a distinct
            # column from the idempotency_key every other consumer means
            # by "assignment_ref" -- resolve it the same way.
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
                ).fetchone()
            assignment_ref = row["idempotency_key"] if row is not None else None
        tier = delegation_generations.ensure_owner(
            self._store, task_id=task_id, engine_session_ref=session_ref, engine=self.engine_name, version=self.version,
        )
        generation = delegation_generations.open_generation(
            self._store, task_id=task_id, engine_session_ref=session_ref, recipient=recipient,
            cause="session_replacement" if replacement else "session_open",
            evidence_source="test", assignment_ref=assignment_ref,
        )
        self._session_meta[session_ref] = dict(
            task_id=task_id, recipient=recipient, assignment_ref=assignment_ref, attempt_id=attempt_id, tier=tier.tier,
        )
        self._pending_generation[session_ref] = generation
        return session_ref

    def lookup_owned(self, attempt_id):
        return self._inner.lookup_owned(attempt_id)

    def terminate_owned(self, attempt_id):
        session_ref = self._inner._owned.get(attempt_id)
        self._inner.terminate_owned(attempt_id)
        if session_ref:
            delegation_deliveries.supersede_session_deliveries(self._store, session_ref, "attempt_terminated")
            self._pending_generation.pop(session_ref, None)

    def cancel_attempt(self, attempt_id):
        self._inner.cancel_attempt(attempt_id)

    def cancel_task(self, task_id):
        self._inner.cancel_task(task_id)

    def shutdown(self):
        self._inner.shutdown()

    def prompt(self, session_ref, message, cancel_event):
        delegation_deliveries.mark_continuation_observed_if_pending(self._store, session_ref, "next_prompt")
        return self._inner.prompt(session_ref, message, cancel_event)

    def pending_delivery(self, session_ref):
        generation = self._pending_generation.get(session_ref)
        meta = self._session_meta.get(session_ref)
        if generation is None or meta is None:
            return None
        return dict(
            generation=generation, recipient=meta["recipient"], task_id=meta["task_id"], attempt_id=meta.get("attempt_id"),
        )

    def begin_delivery(self, session_ref, capsule_revision):
        generation = self._pending_generation.get(session_ref)
        meta = self._session_meta.get(session_ref)
        if generation is None or meta is None:
            return None
        return delegation_deliveries.record_built(
            self._store, recipient=meta["recipient"], context_generation_id=generation["id"],
            capsule_revision=capsule_revision, task_id=meta["task_id"], engine_session_ref=session_ref,
            evidence_source="test", assignment_ref=meta["assignment_ref"],
        )

    def mark_delivery_transmitted(self, session_ref, delivery_id):
        delegation_deliveries.mark_transmitted(self._store, delivery_id, "test")

    def mark_delivery_confirmed(self, session_ref, delivery_id):
        delegation_deliveries.mark_delivery_confirmed(self._store, delivery_id, "test")
        self._pending_generation.pop(session_ref, None)

    def mark_delivery_uncertain(self, session_ref, delivery_id, reason):
        delegation_deliveries.mark_receipt_uncertain(self._store, delivery_id, reason)

    def mark_delivery_blocked(self, session_ref, delivery_id, reason, permanent=False):
        delegation_deliveries.mark_blocked(self._store, delivery_id, reason, "test")
        if permanent:
            self._pending_generation.pop(session_ref, None)

    def trigger_compaction(self, session_ref, evidence_id, phase="completed"):
        meta = self._session_meta[session_ref]
        generation = delegation_generations.record_compaction_event(
            self._store, task_id=meta["task_id"], engine_session_ref=session_ref, evidence_id=evidence_id,
            phase=phase, evidence_source="test_tool_call_meta", recipient=meta["recipient"],
            assignment_ref=meta["assignment_ref"],
        )
        if generation is not None:
            self._pending_generation[session_ref] = generation
        return generation


def _task_with_store(tmp_path):
    store = _store(tmp_path)
    return store, _task(store)


def _make_runtime(store, router, **kwargs):
    engine = GenerationAwareSessionRuntime(store, router, engine=kwargs.pop("engine", "codex-acp"), version=kwargs.pop("version", "1.12.0"))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 10.0), **kwargs,
    )
    return runtime, engine


def test_fresh_generation_prompt_contains_role_rules_capsule_and_tool_schema_access(tmp_path):
    store, task_id = _task_with_store(tmp_path)
    seen_messages = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen_messages.append(message)
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    result = runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    assert result == StopReason.END_TURN
    assert len(seen_messages) == 1
    message = seen_messages[0]
    role = load_role(COORDINATOR_ROLE_ID)
    assert role.body_portable[:80] in message
    assert "Cortex recovery capsule" in message
    assert "allowed_mcp_tools" in message  # current tool-schema access
    assert "Build a page." in message  # original content still delivered
    coordinator_session_ref = runtime._coordinator_sessions[task_id]
    deliveries_for_g1 = delegation_deliveries.for_generation(
        store, "coordinator", delegation_generations.latest_generation(store, coordinator_session_ref)["id"],
    )
    assert len(deliveries_for_g1) == 1
    assert deliveries_for_g1[0]["state"] == "delivery_confirmed"


def test_two_compactions_produce_two_generations_and_two_distinct_deliveries(tmp_path):
    """End-to-end (real OrchestratingRuntime + real generations/deliveries
    ledger): two observed compactions on the same coordinator session each
    open a fresh generation and each get their own delivery row (exact
    unchanged-capsule_revision redelivery is covered directly, with a
    controlled capsule_revision, by
    test_two_compactions_without_state_change_open_two_generations_and_permit_redelivery
    above -- calling start_prompt again here necessarily adds a new user
    message, which is itself new task state, so this end-to-end case
    asserts generation/delivery distinctness rather than a specific
    capsule_revision)."""
    store, task_id = _task_with_store(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, f"turn-{call_index}")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]

    g1 = delegation_generations.latest_generation(store, session_ref)
    d1 = delegation_deliveries.for_generation(store, "coordinator", g1["id"])[0]
    assert d1["state"] == "delivery_confirmed"

    engine.trigger_compaction(session_ref, "tc-1", phase="completed")
    runtime.start_prompt(task_id, "s1", "still building", lambda text: True, cancel_event)
    g2 = delegation_generations.latest_generation(store, session_ref)
    assert g2["id"] != g1["id"]
    assert g2["cause"] == "observed_compaction"
    d2 = delegation_deliveries.for_generation(store, "coordinator", g2["id"])[0]
    assert d2["id"] != d1["id"]
    assert d2["state"] == "delivery_confirmed"

    engine.trigger_compaction(session_ref, "tc-2", phase="completed")
    runtime.start_prompt(task_id, "s1", "still building 2", lambda text: True, cancel_event)
    g3 = delegation_generations.latest_generation(store, session_ref)
    assert g3["id"] not in (g1["id"], g2["id"])
    assert g3["cause"] == "observed_compaction"
    d3 = delegation_deliveries.for_generation(store, "coordinator", g3["id"])[0]
    assert len({d1["id"], d2["id"], d3["id"]}) == 3
    assert delegation_generations.compaction_event_count(store, session_ref) == 2


def test_controlled_boundary_adapter_only_generates_on_controlled_boundaries(tmp_path):
    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router, engine="some-unknown-engine", version="1.0")
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    owner = delegation_generations.owner_row(store, session_ref)
    assert owner["tier"] == delegation_generations.UNQUALIFIED
    # A follow-up ordinary continuation (no session replacement) never
    # produces a second generation for an unqualified/controlled-boundary
    # engine -- there is no evidenced signal to react to.
    runtime.start_prompt(task_id, "s1", "still going", lambda text: True, cancel_event)
    assert len(delegation_generations.generations_for_session(store, session_ref)) == 1


def test_stale_session_superseded_on_terminate_and_replacement_opens_new_generation(tmp_path):
    from cortex_runtime.delegation import events as delegation_events

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(
                store, channel, "delegate",
                dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"),
            )
            return PromptOutcome(StopReason.END_TURN, "delegated")
        return PromptOutcome(StopReason.END_TURN, "waiting")

    def worker_behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("timeout_alive")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)

    import json as _json

    with store.connection() as db:
        attempt_id = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=?", (task_id,)).fetchone()["attempt_id"]
        ready = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_ready'", (task_id,),
        ).fetchone()
    worker_session_ref = _json.loads(ready["payload_json"])["engine_session_ref"]
    assert worker_session_ref is not None
    worker_delivery = delegation_deliveries.for_generation(
        store, "worker", delegation_generations.latest_generation(store, worker_session_ref)["id"],
    )[0]
    # EngineTurnFailed during the delivery turn recorded receipt-timeout
    # style uncertainty (never confirmed) at the time it happened; the
    # runtime's own T017 _terminate_completed_workers then released this
    # now-terminal (assignment_failed) attempt's engine session -- which
    # supersedes any of its deliveries not yet continuation_observed,
    # exactly the "stale session (released) -> never confirmed" rule,
    # never leaving a released session's delivery reachable as confirmed.
    assert worker_delivery["state"] == "superseded"
    assert worker_delivery["reason"] == "attempt_terminated"
    assert delegation_deliveries.mark_delivery_confirmed(store, worker_delivery["id"], "test") is None

    # Re-provisioning the same attempt (a retry) opens a *new* engine
    # session with cause=session_replacement, never reusing the stale one.
    with store.connection() as db:
        request_row = db.execute(
            "SELECT workspace_binding_id FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
        ).fetchone()
    new_session_ref = engine.open(
        request_row["workspace_binding_id"] or "shared_project_root",
        dict(task_id=task_id, attempt_id=attempt_id, assignment_id="a_retry"), "instructions",
    )
    assert new_session_ref != worker_session_ref
    new_generation = delegation_generations.latest_generation(store, new_session_ref)
    assert new_generation["cause"] == "session_replacement"


def test_immediate_overflow_after_delivery_is_blocked_with_bounded_retries(tmp_path, monkeypatch):
    def coordinator_behavior(store, channel, session_ref, message, call_index):
        # The engine hits its own limit right after receiving the (large)
        # delivery-carrying prompt.
        return PromptOutcome(StopReason.MAX_TOKENS, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    generation = delegation_generations.latest_generation(store, session_ref)
    delivery = delegation_deliveries.for_generation(store, "coordinator", generation["id"])[0]
    assert delivery["state"] == "blocked"
    assert delivery["reason"].startswith("engine_limit_after_delivery")


# --------------------------------------------------------------------
# 4. T024: budget planning + compaction-loop guard wired into
#    _begin_generation_delivery, end-to-end through OrchestratingRuntime.
# --------------------------------------------------------------------


def test_repeated_observed_compactions_are_blocked_with_explicit_localized_message(tmp_path):
    """A session that keeps reporting a fresh compaction every turn (never
    meaningfully recovering: 'before' is always estimated from the full
    capsule size, 'after' is always the same small mandatory reconstruction,
    so recovery never drops below the frequency window) trips
    recovery.compaction_guard's bounded frequency check and stops -- no
    unbounded loop of ever-more compactions."""
    from cortex_runtime.recovery import compaction_guard

    seen_messages = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen_messages.append(message)
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]

    # Bounded upper loop count: some real, bounded compaction-guard reason
    # (frequency, immediate refill, or consecutive low recovery -- see
    # recovery.compaction_guard) must trip well within this many attempts,
    # never an unbounded loop of ever-more compactions/prompts.
    max_attempts = compaction_guard.MAX_COMPACTIONS_PER_WINDOW + 2
    result = None
    for index in range(max_attempts):
        engine.trigger_compaction(session_ref, f"tc-{index}", phase="completed")
        result = runtime.start_prompt(task_id, "s1", f"still building {index}", lambda text: True, cancel_event)
        orchestration = _orchestration_row(store, task_id)
        if orchestration["state"] == "blocked":
            break

    assert result == StopReason.END_TURN
    orchestration = _orchestration_row(store, task_id)
    assert orchestration["state"] == "blocked"
    assert orchestration["wait_reason"].startswith("limit:")
    reason = orchestration["wait_reason"].split(":", 1)[1]
    assert reason in ("compaction_frequency_exceeded", "immediate_refill", "consecutive_low_recovery")
    # The prompt that finally tripped the guard was still delivered without
    # ever escalating to a second/more-expensive route -- and no further
    # attempt happened after the blocked state was recorded.
    assert len(seen_messages) < max_attempts + 1
    explanation = json.loads(orchestration["wait_detail_json"])["explanation"]
    assert reason in explanation


def test_budget_overflow_reports_the_concrete_limit_and_never_truncates_constraints(tmp_path, monkeypatch):
    """A tiny (test-forced) context window makes even the ordinary fresh-
    generation mandatory content (role rules + capsule + tool schemas)
    exceed the usable budget -- an explicit, coordinator-facing 'overflow'
    message naming the limit, never a silently truncated capsule/role."""
    from cortex_runtime.delegation.deliveries import MAX_DELIVERY_RETRIES
    from cortex_runtime.recovery import token_budget

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    # First turn delivers normally (default window is generous).
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    # A fresh observed compaction queues a new pending generation to
    # redeliver -- then shrink the route's usable context window so that
    # very generation's own mandatory content (role rules + capsule + tool
    # schemas) alone overflows it.
    engine.trigger_compaction(session_ref, "tc-overflow", phase="completed")
    monkeypatch.setattr(token_budget, "DEFAULT_CONTEXT_WINDOW_TOKENS", 10)
    prompt_calls_before_overflow = len(engine.prompt_calls)
    seen = []
    # EXT-01 #6: the very first redelivery attempt for this exact pending
    # generation already blocks -- never proceeds without the mandatory
    # capsule while bounded retries remain. This is not yet a durable
    # task-orchestration "blocked" state (see _block_generation_delivery's
    # own docstring: doing that here would fold a fresh ``updated_at`` into
    # every subsequent build_capsule call via
    # continuation_intent.orchestration, changing capsule_revision on every
    # attempt and permanently preventing retry_count from ever reaching
    # MAX_DELIVERY_RETRIES) -- only a live "will retry" notice on
    # emit_update, and the pending generation is still stored.
    _, delivery_id, blocked = runtime._begin_generation_delivery(
        task_id, session_ref, COORDINATOR_ROLE_ID, "same pending content", emit_update=seen.append,
    )
    assert blocked
    assert delivery_id is None
    assert len(seen) == 1
    assert "retry" in seen[0].lower() or "attempt 0 of" in seen[0]
    orchestration = _orchestration_row(store, task_id)
    assert orchestration["state"] != "blocked"
    # The pending generation itself is still stored (never dropped while
    # retries remain) -- the fake engine's own pending_delivery still
    # surfaces it.
    assert engine.pending_delivery(session_ref) is not None

    # Repeated direct redelivery attempts for this exact, unchanged pending
    # generation (task state itself never advances between these calls --
    # exactly the "same capsule_revision" bounded-retry case
    # _block_generation_delivery documents) exercise the real production
    # method exactly as OrchestratingRuntime's own continuation loop would,
    # each one still blocking, until MAX_DELIVERY_RETRIES is exhausted and
    # the block becomes permanent.
    for _ in range(MAX_DELIVERY_RETRIES + 2):
        _, _, blocked = runtime._begin_generation_delivery(
            task_id, session_ref, COORDINATOR_ROLE_ID, "same pending content", emit_update=seen.append,
        )
        assert blocked
        orchestration = _orchestration_row(store, task_id)
        if orchestration["state"] == "blocked":
            break

    orchestration = _orchestration_row(store, task_id)
    assert orchestration["state"] == "blocked"
    assert orchestration["wait_reason"] == "limit:budget_overflow"
    explanation = json.loads(orchestration["wait_detail_json"])["explanation"]
    assert "context_window_tokens" in explanation
    assert "split the assignment" in explanation
    # Once permanently blocked, the pending generation is cleared -- there
    # is nothing left to retry.
    assert engine.pending_delivery(session_ref) is None
    # Distinguishable in what was surfaced along the way: every retry
    # notice differs from the final permanent one (no wording collision
    # that would let a client/coordinator confuse "will retry" with
    # "cannot be delivered at all").
    assert seen[-1] != seen[0]
    assert "split the assignment" in seen[-1]
    # Never once was the engine actually prompted for any of these blocked
    # redelivery attempts (EXT-01 #6's core regression: a missing mandatory
    # capsule blocks on the first attempt, not only after retries exhaust).
    assert len(engine.prompt_calls) == prompt_calls_before_overflow


def _orchestration_row(store, task_id):
    with store.connection() as db:
        row = db.execute(
            "SELECT state, wait_reason, wait_detail_json FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    return dict(row)


def test_budget_overflow_never_prompts_the_engine_on_first_attempt_end_to_end(tmp_path, monkeypatch):
    """EXT-01 #6 reviewer regression test, exercised through the *real*
    ``start_prompt``/``_run`` continuation loop (not a direct
    ``_begin_generation_delivery`` call): on the very first turn a mandatory
    capsule does not fit, the coordinator's engine is never prompted at
    all -- asserted via the actual call count, not just a returned flag --
    and the pending generation is kept for a legitimate retry (bounded-
    retry exhaustion itself, keyed on an *unchanged* capsule_revision, is
    covered directly against ``_begin_generation_delivery`` by
    ``test_budget_overflow_reports_the_concrete_limit_and_never_truncates_constraints``
    above -- every ordinary end-to-end turn here legitimately carries new
    user content, which is itself new task state and so never re-hits the
    exact same capsule_revision, exactly like every other "two turns ->
    two distinct generations/deliveries" case in this file)."""
    from cortex_runtime.recovery import token_budget

    call_count = 0

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        nonlocal call_count
        call_count += 1
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    assert call_count == 1

    engine.trigger_compaction(session_ref, "tc-overflow-e2e", phase="completed")
    monkeypatch.setattr(token_budget, "DEFAULT_CONTEXT_WINDOW_TOKENS", 10)

    # First turn after the overflow condition exists: the engine must NOT
    # be prompted at all this turn.
    result = runtime.start_prompt(task_id, "s1", "keep going", lambda text: True, cancel_event)
    assert result == StopReason.END_TURN
    assert call_count == 1  # unchanged -- the engine was never prompted
    assert engine.pending_delivery(session_ref) is not None  # kept for retry
    assert _orchestration_row(store, task_id)["state"] != "blocked"  # not yet a permanent give-up


def test_capsule_oversize_never_prompts_the_engine_on_first_attempt_end_to_end(tmp_path, monkeypatch):
    """The other named EXT-01 #6 condition, end to end: a ``capsule_oversize``
    ``StoreError`` on the very first delivery attempt blocks before ever
    prompting the engine, and once the oversize condition clears the
    generation delivers normally (the legitimate retry path -- a rebuilt
    capsule -- actually working, not just refusing forever)."""
    call_count = 0

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        nonlocal call_count
        call_count += 1
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()

    def boom(*args, **kwargs):
        raise StoreError("capsule_oversize", "critical_sections_bytes", 1, "<= 0", "too big")

    monkeypatch.setattr(orchestrating_runtime, "build_capsule", boom)
    result = runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    assert result == StopReason.END_TURN
    assert call_count == 0  # never prompted -- blocked on the very first attempt
    session_ref = runtime._coordinator_sessions[task_id]
    assert engine.pending_delivery(session_ref) is not None  # kept for a legitimate retry
    assert _orchestration_row(store, task_id)["state"] != "blocked"

    # The legitimate retry path: the oversize condition clears (a rebuilt
    # capsule, standing in for whatever made the next attempt actually
    # differ) and the very next turn now delivers and prompts normally.
    monkeypatch.undo()
    result = runtime.start_prompt(task_id, "s1", "keep going", lambda text: True, cancel_event)
    assert result == StopReason.END_TURN
    assert call_count == 1
    # capsule_deliveries.id is an opaque generated id, never insertion-
    # ordered -- count states rather than index into an assumed order.
    deliveries = delegation_deliveries.for_generation(
        store, "coordinator", delegation_generations.latest_generation(store, session_ref)["id"],
    )
    states = [d["state"] for d in deliveries]
    assert states.count("blocked") == 1
    assert states.count("delivery_confirmed") == 1
    assert engine.pending_delivery(session_ref) is None  # confirmed -- nothing left pending


def test_capsule_oversize_is_blocked_with_a_named_reason(tmp_path, monkeypatch):
    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)

    def boom(*args, **kwargs):
        raise StoreError("capsule_oversize", "critical_sections_bytes", 1, "<= 0", "too big")

    monkeypatch.setattr(orchestrating_runtime, "build_capsule", boom)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    generation = delegation_generations.latest_generation(store, session_ref)
    delivery = delegation_deliveries.for_generation(store, "coordinator", generation["id"])[0]
    assert delivery["state"] == "blocked"
    assert delivery["reason"] == "capsule_oversize"
