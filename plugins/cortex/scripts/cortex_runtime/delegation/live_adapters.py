"""Production seams for ``acp.orchestrating_runtime.OrchestratingRuntime``.

Only the two seams that construct cleanly from existing registries/state
with no new user configuration are provided here:

- :class:`SharedProjectRootWorkspaceProvisioner` -- T016's own explicit,
  documented choice (see the T016 task's lead design notes): every
  delegated attempt shares the task's one project root instead of an
  isolated git worktree. This is intentionally **not** isolated; T025 owns
  real per-attempt worktree isolation. ``create``/``lookup``/``remove`` are
  therefore pure bookkeeping over the attempt id, never a filesystem
  mutation.
- :class:`ConfigurationServiceSnapshotSource` -- a thin read over the
  existing :class:`configuration.service.ConfigurationService` (T008),
  itself already constructed from a project ``Store`` with no new
  configuration surface.

:class:`EngineSessionManagerAdapter` is the live
:class:`~acp.orchestrating_runtime.SessionRuntime`: one
``engine.session_manager.EngineSessionManager`` per attempt (coordinator or
worker -- consistent with T012's "no two engine instances share a process"
design), constructed with that attempt's own trusted-channel
``channel_credentials`` so ``create_session`` sends the real
``mcpServers`` entry (see ``engine.mcp_bridge.build_mcp_servers``) that
launches ``cortex.py`` as the engine's own MCP child, authenticated for
exactly that channel. Route selection itself (which ``ConnectionRecord`` to
launch) is intentionally **not** this adapter's decision: it is injected as
a ``connection_record_factory(task_id) -> ConnectionRecord`` callable,
resolved by the caller (``cortex_acp.py``) from the task's current T008
configuration/catalog state.

Only ``tests/test_orchestrating_runtime_live_session.py`` exercises this
adapter, and only against the test-only, standalone
``tests/fake_engine_process.py`` subprocess (never a real engine) launched
through the real ``EngineSessionManager`` -- proving the prompt/continuation
path goes through the real session-manager machinery, not a bespoke
shortcut. No test in this repository launches a real engine.
"""
from __future__ import annotations

import threading
import time
from typing import Callable, Mapping


class SharedProjectRootWorkspaceProvisioner:
    """Non-isolated ``WorkspaceProvisioner``: every attempt shares the task's
    one project root. See module docstring: T025 owns real isolation."""

    def create(self, attempt_id: str) -> str:
        return "shared_project_root"

    def lookup(self, attempt_id: str) -> str:
        return "present"

    def remove(self, attempt_id: str) -> None:
        return None


class ConfigurationServiceSnapshotSource:
    """``ConfigSnapshotSource`` over an existing ``ConfigurationService``.

    Requires a task to already have a current configuration revision (set
    by the normal T008 route-selection flow) before a coordinator or
    delegated worker can be provisioned; this is a read-only seam, it never
    selects a route on the caller's behalf.
    """

    def __init__(self, configuration_service) -> None:
        self._service = configuration_service

    def snapshot(self, task_id: str) -> Mapping[str, object]:
        current = self._service.current(task_id)
        if not current:
            from ..portable.errors import PortableError, PortableErrorCategory

            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="no_configuration_revision",
                remediation="This task has no current configuration revision; select a route (T008 configuration flow) before starting orchestration.",
                retry_eligible=False,
            )
        engine = current.get("route_engine")
        return dict(
            engine=engine,
            # The exact evidence-qualified version per engine (see
            # engine.instruction_delivery_registry's ("codex-acp","1.12.0")
            # pin, T013); configuration_revisions does not itself track a
            # downstream engine version column, so this mirrors the one
            # pinned mapping instead of inventing a second source of truth.
            version="1.12.0" if engine == "codex-acp" else "*",
            config_revision=current.get("id"),
        )


_ENGINE_CAPABILITIES = None


def _requested_capabilities():
    global _ENGINE_CAPABILITIES
    if _ENGINE_CAPABILITIES is None:
        from ..portable.engine_contract import EngineCapability

        _ENGINE_CAPABILITIES = (
            EngineCapability.SESSION_CREATION,
            EngineCapability.PROMPT_STREAMING,
            EngineCapability.CANCELLATION,
        )
    return _ENGINE_CAPABILITIES


class QueuedOpenCancelled(RuntimeError):
    """Raised by :meth:`EngineSessionManagerAdapter.open` (from
    ``_reserve_capacity_blocking``) when a queued-for-capacity open is
    cancelled before ever reserving -- never a leaked reservation, never a
    live engine process started for it. ``kind`` is ``"attempt"`` (a
    targeted ``cancel_attempt``), ``"task"`` (``cancel_task``), or
    ``"shutdown"`` (:meth:`EngineSessionManagerAdapter.shutdown`)."""

    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


class EngineSessionManagerAdapter:
    """Live ``acp.orchestrating_runtime.SessionRuntime`` over one
    ``EngineSessionManager`` per attempt. See module docstring.

    ``store``/``capacity_limit`` (T017, optional, off by default): when a
    ``store`` is supplied, every :meth:`open` atomically reserves one
    capacity unit for the resolved connection (see ``delegation.capacity``)
    before launching the process, and :meth:`terminate_owned` releases it
    only once the underlying process's exit is *positively observed*
    (``EngineProcess.wait_exited``); an unconfirmed termination (grace
    expired with the process still not observed exited) is marked
    ``ambiguous`` instead and keeps counting against the limit.
    """

    def __init__(
        self,
        connection_record_factory: Callable[[str], object],
        workspace_root_factory: Callable[[str], str],
        *,
        environ: Mapping[str, str] | None = None,
        store=None,
        capacity_limit: int | None = None,
        capacity_queue_timeout_seconds: float = 600.0,
        **manager_kwargs,
    ) -> None:
        self._connection_record_factory = connection_record_factory
        self._workspace_root_factory = workspace_root_factory
        self._environ = environ
        self._manager_kwargs = manager_kwargs
        self._store = store
        self._capacity_limit = capacity_limit
        self._queue_timeout_seconds = capacity_queue_timeout_seconds
        self._lock = threading.Lock()
        self._managers: dict[str, object] = {}
        self._sessions: dict[str, str] = {}
        self._session_to_attempt: dict[str, str] = {}
        self._reservations: dict[str, str] = {}
        self._attempt_task: dict[str, str] = {}
        # T017 (fresh review, MEDIUM): a queued open must observe
        # cancellation promptly instead of polling forever.
        self._cancelled_attempts: set[str] = set()
        self._cancelled_tasks: set[str] = set()
        self._shutdown_event = threading.Event()
        # T017 (load-test fix): notified on every confirmed/ambiguous
        # release and every cancel/shutdown signal, so a queued open wakes
        # promptly instead of relying purely on its own backoff poll
        # interval (up to 1s) under real scheduling contention.
        self._activity_cond = threading.Condition()

    def open(self, workspace_binding_id: str, launch: Mapping[str, object], instructions: str) -> str:
        from ..engine.session_manager import EngineSessionManager

        attempt_id = launch["attempt_id"]
        task_id = launch["task_id"]
        channel_id = launch.get("channel_id")
        channel_secret = launch.get("channel_secret")
        if channel_id and not channel_secret:
            # T016 review (HIGH): never silently launch a worker/coordinator
            # engine session with a channel_id but no secret -- that would
            # start a live engine process with no way to authenticate its
            # own Cortex MCP subprocess, reaching 'ready' with a session
            # that can never actually call a delegation tool.
            raise RuntimeError(
                "EngineSessionManagerAdapter.open refuses to open an engine session for "
                f"channel_id={channel_id!r} with no channel_secret; the caller must supply "
                "the freshly-issued secret or fail the delegation explicitly instead of "
                "calling open()."
            )
        workspace_root = self._workspace_root_factory(task_id)
        channel_credentials = (
            (channel_id, channel_secret, workspace_root)
            if channel_id and channel_secret else None
        )
        record = self._connection_record_factory(task_id)
        reservation_id = None
        if self._store is not None:
            reservation_id = self._reserve_capacity_blocking(record.id, task_id, attempt_id, launch)
        manager = EngineSessionManager(
            record, workspace_root, environ=self._environ,
            channel_credentials=channel_credentials, **self._manager_kwargs,
        )
        manager.negotiate(_requested_capabilities())
        session_id = manager.create_session(task_id, launch.get("assignment_id") or attempt_id)
        with self._lock:
            self._managers[attempt_id] = manager
            self._sessions[attempt_id] = session_id
            self._session_to_attempt[session_id] = attempt_id
            self._attempt_task[attempt_id] = task_id
            if reservation_id is not None:
                self._reservations[attempt_id] = reservation_id
        return session_id

    def _reserve_capacity_blocking(self, connection_id: str, task_id: str, attempt_id: str, launch: Mapping[str, object]) -> str:
        """Block (bounded backoff poll, never a model prompt) until a
        capacity unit is reserved. Deliberately *inside* ``open()`` -- below
        ``delegation.provisioning``'s idempotency-key claim/steal protocol
        -- so a queued delegation is indistinguishable from "the engine is
        just taking a while to launch", never interacting with that
        protocol's own contention recovery (see
        ``acp.orchestrating_runtime.OrchestratingRuntime._provision_and_prompt``'s
        docstring for why retrying ``delegate()`` itself would be unsafe)."""
        from . import capacity as delegation_capacity
        from . import events as delegation_events

        kind = "worker" if launch.get("assignment_id") else "coordinator"
        limit = self._capacity_limit or delegation_capacity.DEFAULT_MAX_CONCURRENT_SESSIONS
        queued_note_recorded = False
        backoff = 0.05
        deadline = time.monotonic() + self._queue_timeout_seconds
        while True:
            if self._shutdown_event.is_set():
                raise QueuedOpenCancelled("shutdown")
            if attempt_id in self._cancelled_attempts:
                raise QueuedOpenCancelled("attempt")
            if task_id in self._cancelled_tasks:
                raise QueuedOpenCancelled("task")
            reservation_id = delegation_capacity.reserve(
                self._store, connection_id, task_id, attempt_id, kind, limit=limit,
            )
            if reservation_id is not None:
                return reservation_id
            if not queued_note_recorded:
                delegation_events.append_event(
                    self._store, task_id, "progress", f"capacity_queued:{attempt_id}",
                    dict(note=f"Attempt {attempt_id} is queued: connection {connection_id!r} capacity is full."),
                    origin="runtime",
                )
                queued_note_recorded = True
            if delegation_capacity.count_active(self._store, connection_id) >= limit and kind == "worker":
                # T017 (fresh review, MEDIUM starvation): if every active
                # reservation on this connection is a *coordinator*'s own
                # (permanent-for-task-lifetime) reservation, this worker can
                # never succeed no matter how long it waits -- fail fast and
                # explicitly instead of queueing forever.
                if delegation_capacity.all_active_are_kind(self._store, connection_id, "coordinator"):
                    raise QueuedOpenCancelled("capacity_unsatisfiable")
            if time.monotonic() > deadline:
                raise QueuedOpenCancelled("capacity_timeout")
            with self._activity_cond:
                self._activity_cond.wait(timeout=backoff)
            backoff = min(backoff * 2, 1.0)

    def _notify_activity(self) -> None:
        with self._activity_cond:
            self._activity_cond.notify_all()

    def cancel_attempt(self, attempt_id: str) -> None:
        """Cancel a specific queued (not-yet-opened) attempt promptly."""
        self._cancelled_attempts.add(attempt_id)
        self._notify_activity()

    def cancel_task(self, task_id: str) -> None:
        """Cancel every queued open for ``task_id`` promptly."""
        self._cancelled_tasks.add(task_id)
        self._notify_activity()

    def shutdown(self) -> None:
        """Signal every queued open to abort, then terminate every owned
        engine process with a confirmed/ambiguous release (never a
        reservation leak)."""
        self._shutdown_event.set()
        self._notify_activity()
        with self._lock:
            attempt_ids = list(self._sessions)
        for attempt_id in attempt_ids:
            self.terminate_owned(attempt_id)

    def lookup_owned(self, attempt_id: str) -> str:
        with self._lock:
            return "alive" if attempt_id in self._sessions else "absent"

    def terminate_owned(self, attempt_id: str) -> None:
        with self._lock:
            manager = self._managers.pop(attempt_id, None)
            session_id = self._sessions.pop(attempt_id, None)
            reservation_id = self._reservations.pop(attempt_id, None)
            if session_id is not None:
                self._session_to_attempt.pop(session_id, None)
        if manager is not None:
            manager.shutdown(grace=2.0)
        if reservation_id is not None and self._store is not None:
            from . import capacity as delegation_capacity

            process = getattr(manager, "_process", None)
            # shutdown() above already waited up to its own grace for exit;
            # a non-blocking check here just reads whatever it observed --
            # never a second wait, never assumed just because shutdown()
            # returned (see EngineProcess.release/wait_exited).
            exited = bool(process is not None and process.wait_exited(0))
            if exited:
                delegation_capacity.release_confirmed(self._store, reservation_id, "process_exited")
            else:
                delegation_capacity.mark_ambiguous(self._store, reservation_id, "timeout_alive")
            # Confirmed release frees a slot a queued open is waiting on;
            # ambiguous does not free anything, but still wakes queued
            # waiters promptly so they can re-check current capacity
            # instead of sitting on their own backoff interval.
            self._notify_activity()

    def prompt(self, session_ref: str, message: str, cancel_event: threading.Event):
        from ..acp.orchestrating_runtime import EngineTurnFailed, PromptOutcome
        from ..portable.engine_contract import EngineEventKind, StopReason
        from ..portable.errors import PortableError

        with self._lock:
            attempt_id = self._session_to_attempt.get(session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is None:
            raise RuntimeError(f"prompt() called on an unopened/terminated session {session_ref!r}.")
        task_id = self._attempt_task.get(attempt_id)
        text_parts: list[str] = []
        stop_reason = StopReason.END_TURN
        try:
            for event in manager.prompt_turn(session_ref, message):
                if cancel_event.is_set():
                    manager.cancel(session_ref)
                if event.kind == EngineEventKind.TURN_DELTA and event.delta_text:
                    text_parts.append(event.delta_text)
                elif event.kind == EngineEventKind.TOOL_CALL_OBSERVED:
                    # T017: durable evidence, never raw tool output/args --
                    # see delegation.evidence module docstring.
                    if self._store is not None and task_id is not None:
                        from . import evidence as delegation_evidence

                        delegation_evidence.record_tool_result_observed(
                            self._store, task_id, attempt_id, event.tool_call_ref,
                            event.tool_call_kind, event.tool_call_title, event.tool_call_status,
                        )
                elif event.kind == EngineEventKind.ERROR:
                    # T017: a yielded EngineEventKind.ERROR (transport_eof,
                    # timeout_alive -- see engine.session_manager's
                    # _prompt_turn_locked) must never be silently treated as
                    # a normal end_turn; the caller (OrchestratingRuntime)
                    # decides the durable outcome, never this adapter.
                    raise EngineTurnFailed(event.error_message or "engine_error")
                elif event.kind == EngineEventKind.TURN_END:
                    stop_reason = event.stop_reason or StopReason.END_TURN
        except PortableError as error:
            if error.code == "engine_session_unusable":
                # session_manager marks a session "poisoned" after an
                # unfinished cancellation race (see its module docstring);
                # a poisoned session can never usefully be prompted again.
                raise EngineTurnFailed("poisoned") from error
            raise
        return PromptOutcome(stop_reason, "".join(text_parts))
