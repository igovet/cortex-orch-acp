"""T023: lightweight, deterministic fakes for coordinator/worker recovery
unit tests -- no real subprocess, no real EngineSessionManager. Complements
``orchestration_fakes.FakeSessionRuntime`` (which already answers
``lookup_owned``/``terminate_owned`` from its own in-memory ``_owned`` dict,
adequate for every *same-process* T016/T017 scenario) with explicit control
over the "alive"/"unknown"/"absent" answer independent of whether ``open()``
was ever called on *this* instance -- modeling a fresh process/adapter
instance after a restart, and over confirmed-vs-ambiguous termination.
"""
from __future__ import annotations

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.recovery import registry as recovery_registry


class FakeRecoveryEngine:
    """``lookup_states``: attempt_id -> "alive"/"unknown"/"absent", the
    answer :meth:`lookup_owned` gives *before* any ``terminate_owned`` call
    for that attempt (simulating a durable cross-process registry record
    with no same-process manager backing it). ``terminate_confirms``:
    attempt_id -> bool, whether :meth:`terminate_owned` actually flips that
    attempt's state to "absent" (a confirmed exit) or leaves it exactly as
    it was (an unconfirmed/ambiguous termination attempt).

    ``route``/``policy``/``config_revision`` (module-level defaults,
    overridable per call to :meth:`open`) control what
    ``recovery.registry.record_effective_context`` records for each freshly
    opened attempt, exactly like the real
    ``delegation.live_adapters.EngineSessionManagerAdapter`` does -- this is
    the only registry table this fake ever touches; liveness is answered
    purely from ``lookup_states``/``terminate_confirms``, never from the
    registry (a fresh coordinator-recovery ``engine.lookup_owned`` call is
    never itself an OS-level probe here, unlike the real adapter's T023
    cross-process fallback -- see ``tests/test_recovery_cross_process.py``
    for that)."""

    def __init__(self, store, *, lookup_states=None, terminate_confirms=None,
                 default_route_engine="codex-acp", default_route_connection_id="cn_default",
                 default_config_revision="cr_1", default_policy=None):
        self.store = store
        self.lookup_states = dict(lookup_states or {})
        self.terminate_confirms = dict(terminate_confirms or {})
        self.terminate_calls = []
        self.open_calls = []
        self._sessions = {}
        self._counter = 0
        self.default_route_engine = default_route_engine
        self.default_route_connection_id = default_route_connection_id
        self.default_config_revision = default_config_revision
        self.default_policy = default_policy or dict(fs_local=False, terminal=False, output_byte_limit_max=1048576)
        # Overrides applied only to the *next* open() call, keyed by role.
        self.next_route_engine = None
        self.next_route_connection_id = None
        self.next_config_revision = None
        self.next_policy = None

    def open(self, workspace_binding_id, launch, instructions):
        self.open_calls.append(dict(launch))
        self._counter += 1
        session_ref = f"es_fake_{self._counter}"
        role = "worker" if launch.get("assignment_id") else "coordinator"
        route_engine = self.next_route_engine or self.default_route_engine
        route_connection_id = self.next_route_connection_id or self.default_route_connection_id
        config_revision = self.next_config_revision if self.next_config_revision is not None else (
            launch.get("config_revision") or self.default_config_revision
        )
        policy = self.next_policy or self.default_policy
        self.next_route_engine = None
        self.next_route_connection_id = None
        self.next_config_revision = None
        self.next_policy = None
        recovery_registry.record_effective_context(
            self.store, attempt_id=launch["attempt_id"], task_id=launch["task_id"], role=role,
            selected_config_revision_id=config_revision, effective_config_revision_id=config_revision,
            route_engine=route_engine, route_connection_id=route_connection_id, resource_policy=policy,
        )
        self._sessions[session_ref] = launch["attempt_id"]
        self.lookup_states[launch["attempt_id"]] = "alive"
        return session_ref

    def lookup_owned(self, attempt_id):
        return self.lookup_states.get(attempt_id, "absent")

    def terminate_owned(self, attempt_id):
        self.terminate_calls.append(attempt_id)
        if self.terminate_confirms.get(attempt_id, True):
            self.lookup_states[attempt_id] = "absent"
        # else: leave whatever state it already had (ambiguous/unconfirmed).

    def prompt(self, session_ref, message, cancel_event, *, on_delta=None):
        return PromptOutcome(StopReason.END_TURN, "")
