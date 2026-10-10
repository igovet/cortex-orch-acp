"""T016: in-process fakes for OrchestratingRuntime tests.

No real engine, workspace, or network I/O anywhere in this module. The
"fake model" a scripted worker/coordinator behavior represents calls
``delegation.mcp_tools.dispatch`` directly with the session's own resolved
``ChannelContext`` -- exactly the in-process substitute for "an engine's
own tool call to its Cortex MCP subprocess" the T016 task card asks tests
to use.
"""
from __future__ import annotations

import threading
import time

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.trusted_channel import ChannelContext


def channel_context_by_id(store, channel_id):
    """Test-only: resolve a ``ChannelContext`` by id without a secret.

    Production code always authenticates via ``trusted_channel.resolve_channel``
    (channel_id + secret). This fake stands in for "the engine process
    already authenticated over its own trusted channel" without this test
    double needing to also simulate secret delivery to a spawned process --
    see ``delegation.live_adapters`` module docstring for why that wiring is
    out of this task's scope.
    """
    with store.connection() as db:
        row = db.execute("SELECT * FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
    if row is None:
        return None
    return ChannelContext(
        channel_id=row["channel_id"], task_id=row["task_id"],
        principal_thread_id=row["principal_thread_id"],
        parent_principal_thread_id=row["parent_principal_thread_id"],
        role=row["role"], owner_epoch=row["owner_epoch"],
        assignment_id=row["assignment_id"], attempt_id=row["attempt_id"],
        workspace_binding_id=row["workspace_binding_id"],
        context_generation_id=row["context_generation_id"],
    )


class FakeWorkspace:
    def create(self, attempt_id):
        return "shared_project_root"

    def lookup(self, attempt_id):
        return "present"

    def remove(self, attempt_id):
        return None


class FakeConfigSnapshotSource:
    def __init__(self, engine="codex-acp", version="1.12.0"):
        self.engine = engine
        self.version = version

    def snapshot(self, task_id, role_id=None):
        return dict(engine=self.engine, version=self.version, config_revision="cr_" + task_id[2:])


class FakeSessionRuntime:
    """A ``SessionRuntime`` whose ``prompt`` calls a test-supplied ``router``.

    ``router(store, channel, session_ref, launch, instructions) -> behavior``
    is called once, at ``open()`` time, to pick the scripted behavior for
    that session; ``behavior(store, channel, session_ref, message,
    call_index) -> PromptOutcome`` is then invoked once per ``prompt()``
    call. This lets each test script exactly what a coordinator/worker
    "model" does on each turn, including calling ``mcp_tools.dispatch``.
    """

    def __init__(self, store, router, *, connection_id=None, capacity_limit=None,
                 capacity_queue_timeout_seconds=30.0, connections=None,
                 emit_outcome_text_as_delta=True):
        self._store = store
        self._router = router
        self._emit_outcome_text_as_delta = emit_outcome_text_as_delta
        # T027: optional {connection_id: object-with-.provider/.engine/.id}
        # map, used only by attempt_fallback below to run the SAME real
        # routing.authorization.authorize_fallback check a production
        # EngineSessionManagerAdapter runs -- the fake engine's own open()
        # never discriminates by connection, only this authorization step
        # does.
        self._connections = connections or {}
        self.fallback_calls = []
        self.sessions = {}
        self._owned = {}
        self._counter = 0
        self.prompt_calls = []
        self.open_calls = []
        self.terminate_calls = []
        self._lock = threading.Lock()
        # T017 (lead follow-up): when configured, exercise the SAME
        # production capacity ledger (delegation.capacity) a real
        # EngineSessionManagerAdapter uses -- open() blocks (bounded
        # backoff poll, recording one capacity_queued progress event, never
        # raising/failing on ordinary contention) until a reservation is
        # free, mirroring
        # EngineSessionManagerAdapter._reserve_capacity_blocking exactly,
        # including its cancellation/starvation/timeout handling (T017
        # fresh review); terminate_owned() releases per a test-controlled
        # confirmed/ambiguous outcome (default confirmed).
        self._connection_id = connection_id
        self._capacity_limit = capacity_limit
        # A few queue-state tests control the release themselves and need
        # their fake waiter to remain queued until that explicit release (or
        # cancel/shutdown), regardless of unrelated suite scheduling load.
        # Production's bounded queue timeout is unchanged.
        self._queue_timeout_seconds = capacity_queue_timeout_seconds
        self._reservations = {}
        self._release_kind = {}
        # T018 fresh-review fix: every delegation_capacity.reserve() attempt
        # (success or refusal), appended before _notify_activity() -- lets a
        # test synchronize on "the queued worker actually re-attempted and
        # was refused" as an explicit event, instead of a fixed-duration
        # sleep standing in for "nothing happened" (see
        # test_ambiguous_release_keeps_second_delegation_queued).
        self.reserve_attempts = []
        self._cancelled_attempts = set()
        self._cancelled_tasks = set()
        self._shutdown_event = threading.Event()
        # T017 (load-test determinism fix): notified on every reserve
        # attempt (success or fail), every open, every release, and every
        # cancel/shutdown signal -- tests wait_until() on this instead of a
        # fixed sleep-then-poll, and the queue loop itself wakes on it
        # instead of relying purely on its own backoff interval.
        self._activity_cond = threading.Condition()

    def _notify_activity(self):
        with self._activity_cond:
            self._activity_cond.notify_all()

    def wait_until(self, predicate, timeout=30.0):
        """Test helper: block until ``predicate()`` is true or ``timeout``
        (generous upper bound; returns as soon as an activity notification
        makes the predicate true, never a fixed sleep). Returns the
        predicate's final truthiness."""
        deadline = time.monotonic() + timeout
        with self._activity_cond:
            result = predicate()
            while not result and time.monotonic() < deadline:
                self._activity_cond.wait(timeout=max(0.0, min(0.5, deadline - time.monotonic())))
                result = predicate()
        return result

    def set_release_kind(self, attempt_id, kind):
        """Test hook: force the *next* terminate_owned(attempt_id) to
        release 'confirmed' or leave it 'ambiguous' (never auto-released)."""
        self._release_kind[attempt_id] = kind

    def cancel_attempt(self, attempt_id):
        self._cancelled_attempts.add(attempt_id)
        self._notify_activity()

    def cancel_task(self, task_id):
        self._cancelled_tasks.add(task_id)
        self._notify_activity()

    def shutdown(self):
        self._shutdown_event.set()
        self._notify_activity()
        for attempt_id in list(self._owned):
            self.terminate_owned(attempt_id)

    def open(self, workspace_binding_id, launch, instructions):
        from cortex_runtime.delegation.live_adapters import QueuedOpenCancelled

        attempt_id = launch.get("attempt_id")
        task_id = launch["task_id"]
        if self._connection_id is not None:
            from cortex_runtime.delegation import capacity as delegation_capacity
            from cortex_runtime.delegation import events as delegation_events

            limit = self._capacity_limit or delegation_capacity.DEFAULT_MAX_CONCURRENT_SESSIONS
            kind = "worker" if launch.get("assignment_id") else "coordinator"
            queued_note_recorded = False
            backoff = 0.05
            reservation_id = None
            deadline = (time.monotonic() + self._queue_timeout_seconds
                        if self._queue_timeout_seconds is not None else None)
            while reservation_id is None:
                if self._shutdown_event.is_set():
                    raise QueuedOpenCancelled("shutdown")
                if attempt_id in self._cancelled_attempts:
                    raise QueuedOpenCancelled("attempt")
                if task_id in self._cancelled_tasks:
                    raise QueuedOpenCancelled("task")
                reservation_id = delegation_capacity.reserve(
                    self._store, self._connection_id, task_id, attempt_id, kind, limit=limit,
                )
                self.reserve_attempts.append((attempt_id, reservation_id))
                self._notify_activity()  # every attempt (success or fail) is observable to wait_until()
                if reservation_id is not None:
                    break
                if not queued_note_recorded:
                    delegation_events.append_event(
                        self._store, task_id, "progress", f"capacity_queued:{attempt_id}",
                        dict(note=f"Attempt {attempt_id} is queued: capacity is full."), origin="runtime",
                    )
                    queued_note_recorded = True
                if deadline is not None and time.monotonic() > deadline:
                    raise QueuedOpenCancelled("capacity_timeout")
                with self._activity_cond:
                    self._activity_cond.wait(timeout=backoff)
                backoff = min(backoff * 2, 0.5)
            self._reservations[attempt_id] = reservation_id
        self.open_calls.append(attempt_id)
        self._notify_activity()
        with self._lock:
            self._counter += 1
            session_ref = f"es_{self._counter}"
        channel = channel_context_by_id(self._store, launch.get("channel_id")) if launch.get("channel_id") else None
        behavior = self._router(self._store, channel, session_ref, launch, instructions)
        self.sessions[session_ref] = dict(channel=channel, behavior=behavior, calls=0, attempt_id=launch.get("attempt_id"))
        if launch.get("attempt_id"):
            self._owned[launch["attempt_id"]] = session_ref
        # T027: audit every *non*-fallback attempt this fake ever opens the
        # same way a production EngineSessionManagerAdapter.open does --
        # attempt_fallback (above) records its own 'fallback' row itself,
        # never duplicated here.
        config_revision = launch.get("config_revision")
        if config_revision and launch.get("route_override") is None:
            from cortex_runtime.configuration import store as configuration_store
            from cortex_runtime.routing import authorization as routing_authorization
            from cortex_runtime.routing import store as routing_store

            with self._store.connection() as db:
                row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (config_revision,)).fetchone()
            if row is not None:
                revision = configuration_store._row_dict(row)
                extras = routing_store.get_route_extras(self._store, config_revision)
                connection = self._connections.get(revision["route_connection_id"])
                routing_store.record_attempt_audit(
                    self._store, attempt_id=launch.get("attempt_id") or session_ref, task_id=task_id,
                    role="worker" if launch.get("assignment_id") else "coordinator",
                    engine=(connection.engine if connection is not None else revision["route_engine"]),
                    connection_id=revision["route_connection_id"],
                    model=(extras["route_model"] if extras is not None else "unknown"),
                    params_fingerprint=routing_authorization.fingerprint_params(launch.get("parameters") or {}),
                    config_revision_id=config_revision, decision="authorized", detail={},
                )
        return session_ref

    def lookup_owned(self, attempt_id):
        return "alive" if attempt_id in self._owned else "absent"

    def lookup_owned_tx(self, attempt_id, db):
        return self.lookup_owned(attempt_id)

    def terminate_owned(self, attempt_id):
        self.terminate_calls.append(attempt_id)
        self._owned.pop(attempt_id, None)
        reservation_id = self._reservations.pop(attempt_id, None)
        if reservation_id is not None:
            from cortex_runtime.delegation import capacity as delegation_capacity

            if self._release_kind.pop(attempt_id, "confirmed") == "confirmed":
                delegation_capacity.release_confirmed(self._store, reservation_id, "process_exited")
            else:
                delegation_capacity.mark_ambiguous(self._store, reservation_id, "timeout_alive")
            self._notify_activity()

    def prompt(self, session_ref, message, cancel_event, *, on_delta=None, on_tool_call=None, on_display_update=None):
        info = self.sessions[session_ref]
        self.prompt_calls.append(session_ref)
        call_index = info["calls"]
        info["calls"] += 1
        behavior = info["behavior"]
        if behavior is None:
            outcome = PromptOutcome(StopReason.END_TURN, "")
        else:
            # A behavior that declares ``on_delta`` wants to emit chunks
            # *during* its turn (the only way to exercise anything that
            # reacts to a turn still in flight); every other behavior keeps
            # the original five-argument call exactly.
            import inspect

            declared = inspect.signature(behavior).parameters
            extra = {}
            if "on_delta" in declared:
                extra["on_delta"] = on_delta
            if "on_tool_call" in declared:
                extra["on_tool_call"] = on_tool_call
            if "on_display_update" in declared:
                extra["on_display_update"] = on_display_update
            if "cancel_event" in declared:
                extra["cancel_event"] = cancel_event
            outcome = behavior(
                self._store, info["channel"], session_ref, message, call_index, **extra,
            )
        # T035 visibility fix: this fake has no real per-chunk timeline, so
        # it simulates streaming with the one chunk it actually has (its
        # own whole outcome text) -- enough for a caller asserting
        # ``on_delta`` was exercised without needing the full
        # fake_engine_process JSON-RPC harness for that.
        if on_delta is not None and outcome.text and self._emit_outcome_text_as_delta:
            on_delta(outcome.text)
        return outcome

    def attempt_fallback(
        self, *, old_attempt_id, new_attempt_id, task_id, role, failure_class,
        target_connection_id, target_route_id, workspace_binding_id, launch, instructions, target_model=None,
    ):
        """T027: the fake-engine equivalent of
        ``delegation.live_adapters.EngineSessionManagerAdapter
        .attempt_fallback`` -- runs the real
        ``routing.authorization.authorize_fallback`` check (never a
        scripted/stubbed authorization result) against this task's actual
        stored configuration revision/route-profile/ceiling, and only on
        success opens a fresh fake session via the SAME ``open()`` this
        class already uses for every other attempt."""
        from cortex_runtime.configuration import store as configuration_store
        from cortex_runtime.configuration.policy import RouteProfile
        from cortex_runtime.routing import authorization as routing_authorization
        from cortex_runtime.routing import store as routing_store

        self.fallback_calls.append((old_attempt_id, new_attempt_id, failure_class, target_route_id))
        config_revision = launch.get("config_revision")
        with self._store.connection() as db:
            row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (config_revision,)).fetchone()
        revision = configuration_store._row_dict(row)
        extras = routing_store.get_route_extras(self._store, config_revision)
        route_profile = extras["route_profile"] if extras is not None else RouteProfile()
        current_connection = self._connections[revision["route_connection_id"]]
        target_connection = self._connections[target_connection_id]
        routing_authorization.authorize_fallback(
            self._store, task_id=task_id, ceiling=revision["ceiling_snapshot"], failure_class=failure_class,
            role=role, route_profile=route_profile, coordinator_route_id=revision["route_id"],
            current_connection=current_connection, target_connection=target_connection, target_route_id=target_route_id,
        )
        fallback_launch = dict(launch)
        fallback_launch["attempt_id"] = new_attempt_id
        fallback_launch["route_override"] = dict(connection_id=target_connection_id, model=target_model)
        fallback_launch["fallback_of"] = old_attempt_id
        session_ref = self.open(workspace_binding_id, fallback_launch, instructions)
        routing_store.record_attempt_audit(
            self._store, attempt_id=new_attempt_id, task_id=task_id, role=role, engine=target_connection.engine,
            connection_id=target_connection_id, model=target_model or "unknown",
            params_fingerprint=routing_authorization.fingerprint_params(launch.get("parameters") or {}),
            config_revision_id=config_revision, decision="fallback", detail=dict(failure_class=failure_class),
            predecessor_attempt_id=old_attempt_id,
        )
        return session_ref


def coordinator_prompt_count(fake: FakeSessionRuntime, coordinator_session_ref: str) -> int:
    return sum(1 for ref in fake.prompt_calls if ref == coordinator_session_ref)
