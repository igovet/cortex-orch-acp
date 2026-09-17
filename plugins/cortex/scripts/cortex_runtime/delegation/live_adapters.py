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
            # T023: the connection actually selected by this revision --
            # recovery.coordinator_recovery/registry compares this against
            # a recorded attempt's own route to detect an unauthorized
            # silent provider switch.
            route_connection_id=current.get("route_connection_id"),
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
        workspace_resolver: Callable[[str, Mapping[str, object]], str] | None = None,
        **manager_kwargs,
    ) -> None:
        self._connection_record_factory = connection_record_factory
        self._workspace_root_factory = workspace_root_factory
        # T025 (optional, opt-in -- every existing caller keeps its
        # single-argument ``workspace_root_factory(task_id)`` behaviour
        # unchanged): when supplied, resolves the *actual* per-attempt
        # workspace root from this open()'s own ``workspace_binding_id``
        # (the ref ``delegation.provisioning.WorkspaceProvisioner.create``
        # returned -- see ``delegation.workspace_isolation
        # .IsolatedSnapshotWorkspaceProvisioner``) instead of the
        # task-wide shared root, so two attempts of the same task can
        # actually run against two different, non-overlapping directories.
        self._workspace_resolver = workspace_resolver
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
        # T022: context-generation/delivery tracking, active only when
        # ``store`` is supplied (same gate T017's capacity tracking uses --
        # a caller/test with no store gets the pre-T022 behavior
        # unchanged). ``_ever_opened_attempts`` distinguishes a brand new
        # attempt's first session (cause=session_open) from a retried
        # attempt reopening a session (cause=session_replacement).
        # ``_pending_generation``/``_session_meta`` are keyed by portable
        # ``session_ref``.
        self._ever_opened_attempts: set[str] = set()
        self._pending_generation: dict[str, dict] = {}
        self._session_meta: dict[str, dict] = {}
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
        if self._workspace_resolver is not None:
            workspace_root = self._workspace_resolver(workspace_binding_id, launch)
        else:
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
            channel_credentials=channel_credentials,
            terminal_output_persist=self._persist_terminal_output,
            **self._manager_kwargs,
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
            replacement = attempt_id in self._ever_opened_attempts or bool(launch.get("recovery"))
            self._ever_opened_attempts.add(attempt_id)
        if self._store is not None:
            self._record_process_identity(record, session_id, task_id, attempt_id, launch)
            self._record_effective_context(record, task_id, attempt_id, launch)
            self._open_generation(
                manager, record, session_id, task_id, attempt_id, launch,
                cause="session_replacement" if replacement else "session_open",
            )
        return session_id

    def _record_process_identity(self, record, session_id, task_id, attempt_id, launch) -> None:
        """T023: durably record enough OS-level identity for this attempt's
        freshly-launched engine process that a *different* future adapter
        instance (a Cortex process restart) can tell "still running" apart
        from "exited" apart from "cannot tell" for a process it never
        itself launched (see ``engine.process_identity``/
        ``recovery.registry``) -- otherwise :meth:`lookup_owned` always
        answers "absent" after a restart regardless of the real OS state.
        Best-effort only: a manager with no real ``EngineProcess`` (a fake
        test double) or an unreadable ``/proc`` records nothing, and a
        later :meth:`lookup_owned` for that attempt then falls back to
        "unknown" rather than a guessed "alive"/"absent"."""
        from ..engine import process_identity
        from ..recovery import registry as recovery_registry

        manager = self._managers.get(attempt_id)
        process = getattr(manager, "_process", None)
        popen = getattr(process, "process", None)
        pid = getattr(popen, "pid", None)
        if pid is None:
            return
        identity = process_identity.capture(pid)
        if identity is None:
            return
        import hashlib

        digest = hashlib.sha256("\x00".join(record.launch_command or []).encode("utf-8")).hexdigest()
        role = "worker" if launch.get("assignment_id") else "coordinator"
        recovery_registry.record_process_launch(
            self._store, attempt_id=attempt_id, task_id=task_id, role=role,
            pid=identity.pid, pgid=identity.pgid, start_ticks=identity.start_ticks, boot_id=identity.boot_id,
            launch_command_digest=digest, connection_id=record.id, engine_session_ref=session_id,
        )

    def _record_effective_context(self, record, task_id, attempt_id, launch) -> None:
        """T023: durably record the configuration revision and
        :class:`~..engine.resource_policy.ResourcePolicy` this attempt was
        actually opened with -- ``recovery.coordinator_recovery`` reads
        this back to refuse a replacement that would silently broaden
        rights or switch engine/connection with no new, explicitly
        selected configuration revision authorizing it."""
        from ..engine.resource_policy import ResourcePolicy
        from ..recovery import registry as recovery_registry

        policy = self._manager_kwargs.get("policy") or ResourcePolicy()
        role = "worker" if launch.get("assignment_id") else "coordinator"
        config_revision = launch.get("config_revision")
        recovery_registry.record_effective_context(
            self._store, attempt_id=attempt_id, task_id=task_id, role=role,
            selected_config_revision_id=config_revision, effective_config_revision_id=config_revision,
            route_engine=record.engine, route_connection_id=record.id, resource_policy=policy,
        )

    def _open_generation(self, manager, record, session_id, task_id, attempt_id, launch, *, cause: str) -> None:
        from . import generations as delegation_generations

        recipient = "worker" if launch.get("assignment_id") else "coordinator"
        # NOTE: ``launch["assignment_id"]`` is ``delegation_provisioning.
        # assignment_id`` (a distinct column from its primary key). Every
        # other consumer of a generation's own recorded ``assignment_ref``
        # (``recovery.capsule``'s ``recipient["assignment_ref"]``,
        # ``task_events.assignment_ref``, ...) means the provisioning row's
        # ``idempotency_key`` instead (see
        # ``acp.orchestrating_runtime.DelegateResult.assignment_ref`` /
        # ``ProvisioningService.delegate``) -- resolved here by
        # ``attempt_id`` so the generation's own ``assignment_ref`` column
        # actually matches what a later capsule build looks up.
        assignment_ref = None
        if recipient == "worker":
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
                ).fetchone()
            assignment_ref = row["idempotency_key"] if row is not None else None
        # Same documented pinned-version convention as
        # ``ConfigurationServiceSnapshotSource.snapshot`` (T016): no real
        # installed-version discovery is wired into ``ConnectionRecord``
        # yet, so codex-acp resolves to its one pinned evidence-qualified
        # version and every other engine resolves to the wildcard tier
        # entry (see ``delegation.generations.tier_for``'s own fallback --
        # an engine with no wildcard evidence still correctly falls to
        # ``controlled_boundary``/``unqualified``, never a guessed tier).
        version = "1.12.0" if record.engine == "codex-acp" else "*"
        tier = delegation_generations.ensure_owner(
            self._store, task_id=task_id, engine_session_ref=session_id, engine=record.engine, version=version,
        )
        generation = delegation_generations.open_generation(
            self._store, task_id=task_id, engine_session_ref=session_id, recipient=recipient,
            cause=cause, evidence_source=f"engine_session_manager:{cause}", assignment_ref=assignment_ref,
        )
        with self._lock:
            self._session_meta[session_id] = dict(
                task_id=task_id, attempt_id=attempt_id, engine=record.engine, version=version,
                recipient=recipient, assignment_ref=assignment_ref, tier=tier.tier,
            )
            self._pending_generation[session_id] = generation

    def _persist_terminal_output(
        self, engine_session_id: str, terminal_id: str, data: bytes, *,
        truncated: bool, exit_code: int | None, signal: str | None, failure: str | None,
    ) -> str | None:
        """T024: real ``engine.terminal_bridge.TerminalBridge`` persist sink
        binding -- looks up this exact session's already-recorded
        ``task_id``/``recipient``/``assignment_ref`` (see
        ``_open_generation``, populated before any real terminal can be
        created) and persists via ``recovery.large_output_store``, scoped
        by owner exactly like a capsule recipient (``"coordinator"`` or the
        worker's own ``assignment_ref`` -- never ambiguous, never another
        assignment's scope). Best-effort: returns ``None`` (no
        ``large_output_id`` surfaced) rather than raising, if this session's
        meta or the store is unavailable."""
        if self._store is None:
            return None
        with self._lock:
            meta = self._session_meta.get(engine_session_id)
        if meta is None:
            return None
        owner_scope = "coordinator" if meta["recipient"] == "coordinator" else meta.get("assignment_ref")
        if not owner_scope:
            return None
        from ..recovery import large_output_store

        record = large_output_store.persist_output(
            self._store, task_id=meta["task_id"], owner_scope=owner_scope, source_kind="terminal",
            data=data, source_ref=terminal_id, exit_code=exit_code, signal=signal,
            failure=failure, truncated=truncated,
        )
        return record["id"]

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
        """"alive"/"absent"/"unknown" for the process owned by
        ``attempt_id``. A same-process manager (this exact adapter instance
        launched it) is always authoritative and answers "alive"
        immediately. Otherwise (this adapter never launched it -- almost
        always because a *different* process/adapter instance did, before a
        Cortex process restart) falls back to the durable
        ``recovery.registry`` identity captured at launch time (T023: see
        that module and ``engine.process_identity`` -- a bare "no record in
        memory" no longer means "absent" the way it did pre-T023)."""
        with self._lock:
            if attempt_id in self._sessions:
                return "alive"
        if self._store is None:
            return "absent"
        from ..recovery import registry as recovery_registry

        row = recovery_registry.lookup_process(self._store, attempt_id)
        if row is None:
            return "absent"
        state = recovery_registry.probe_liveness(row)
        if state == "absent":
            recovery_registry.mark_terminated(self._store, attempt_id, "lookup_owned_absent")
        return state

    def terminate_owned(self, attempt_id: str) -> None:
        with self._lock:
            manager = self._managers.pop(attempt_id, None)
            session_id = self._sessions.pop(attempt_id, None)
            reservation_id = self._reservations.pop(attempt_id, None)
            if session_id is not None:
                self._session_to_attempt.pop(session_id, None)
            self._session_meta.pop(session_id, None) if session_id is not None else None
            self._pending_generation.pop(session_id, None) if session_id is not None else None
        if manager is not None:
            manager.shutdown(grace=2.0)
            exited = None
            process = getattr(manager, "_process", None)
            # shutdown() above already waited up to its own grace for exit;
            # a non-blocking check here just reads whatever it observed --
            # never a second wait, never assumed just because shutdown()
            # returned (see EngineProcess.release/wait_exited).
            if process is not None:
                exited = bool(process.wait_exited(0))
            if self._store is not None:
                from ..recovery import registry as recovery_registry

                recovery_registry.mark_terminated(
                    self._store, attempt_id, "process_exited" if exited else "timeout_alive",
                )
        elif self._store is not None:
            # T023: no same-process manager owns this attempt -- either it
            # was launched by a *different* process (a restart), or it was
            # never launched at all. Terminate it directly from the
            # durable registry record: never a second .wait() (this
            # process is not that child's parent and cannot reap it), so
            # exit is confirmed only by re-probing OS-level identity after
            # signalling, exactly like the ambiguous/confirmed distinction
            # ``EngineProcess.terminate`` already makes for a same-process
            # child.
            exited = self._terminate_cross_process(attempt_id)
            if reservation_id is None:
                from . import capacity as delegation_capacity

                # T023: this adapter instance never itself reserved capacity
                # for this attempt (a different process's own adapter did,
                # before a restart) -- its reservation id lives only in
                # capacity_reservations, never in this instance's own
                # in-memory _reservations dict.
                reservation_id = delegation_capacity.reservation_for_attempt(self._store, attempt_id)
        else:
            exited = None
        if self._store is not None and session_id is not None:
            from . import deliveries as delegation_deliveries

            # T022: a released session's not-yet-``continuation_observed``
            # deliveries can never later report confirmation from a stale
            # in-flight call -- "stale session (released/replaced) -> never
            # confirmed".
            delegation_deliveries.supersede_session_deliveries(self._store, session_id, "attempt_terminated")
        if reservation_id is not None and self._store is not None:
            from . import capacity as delegation_capacity

            if exited:
                delegation_capacity.release_confirmed(self._store, reservation_id, "process_exited")
            else:
                delegation_capacity.mark_ambiguous(self._store, reservation_id, "timeout_alive")
            # Confirmed release frees a slot a queued open is waiting on;
            # ambiguous does not free anything, but still wakes queued
            # waiters promptly so they can re-check current capacity
            # instead of sitting on their own backoff interval.
            self._notify_activity()

    def _terminate_cross_process(self, attempt_id: str) -> bool:
        """Best-effort SIGTERM-then-SIGKILL of an attempt's process group
        this adapter instance never itself launched, by durable pid/pgid
        identity alone (see ``recovery.registry``). Returns ``True`` only
        once its exit is actually re-observed (never assumed from the
        signal alone); ``False`` (ambiguous, never freed) otherwise --
        including when there was never a registry record to act on (no
        capacity was ever reserved for it in that case either)."""
        import os
        import signal
        import time

        from ..engine.process import TERMINATE_GRACE_SECONDS
        from ..recovery import registry as recovery_registry

        row = recovery_registry.lookup_process(self._store, attempt_id)
        if row is None:
            return False
        if recovery_registry.probe_liveness(row) == "absent":
            recovery_registry.mark_terminated(self._store, attempt_id, "already_absent")
            return True
        try:
            os.killpg(row["pgid"], signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
        while time.monotonic() < deadline:
            if recovery_registry.probe_liveness(row) == "absent":
                recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
                return True
            time.sleep(0.05)
        try:
            os.killpg(row["pgid"], signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
        while time.monotonic() < deadline:
            if recovery_registry.probe_liveness(row) == "absent":
                recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
                return True
            time.sleep(0.05)
        return False

    def _observe_compaction_signal(self, session_ref: str, event) -> None:
        """T022: an observed compaction reaching its terminal phase opens
        (at most once per evidence id -- see
        ``delegation.generations.record_compaction_event``) a fresh
        generation, queued the same way ``open()`` queues its own initial
        one -- picked up by :meth:`pending_delivery` on the *next*
        continuation, never mid-turn (Cortex cannot rewrite a turn already
        in flight)."""
        from . import generations as delegation_generations

        with self._lock:
            meta = self._session_meta.get(session_ref)
        if meta is None or event.tool_call_ref is None:
            return
        phase = "completed" if event.compaction_reason == "completed" else (
            "failed" if event.compaction_reason in ("failed",) else "in_progress"
        )
        generation = delegation_generations.record_compaction_event(
            self._store, task_id=meta["task_id"], engine_session_ref=session_ref,
            evidence_id=event.tool_call_ref, phase=phase,
            evidence_source="tool_call_meta_contextCompaction", recipient=meta["recipient"],
            assignment_ref=meta["assignment_ref"],
        )
        if generation is not None:
            with self._lock:
                self._pending_generation[session_ref] = generation

    def pending_delivery(self, session_ref: str) -> Mapping[str, object] | None:
        """T022 (optional ``SessionRuntime`` extension, checked via
        ``getattr`` by ``acp.orchestrating_runtime`` -- never required):
        the queued fresh :class:`context_generations` row for
        ``session_ref``, if any, that has not yet been acknowledged via
        :meth:`begin_delivery`/:meth:`clear_pending_delivery`."""
        with self._lock:
            generation = self._pending_generation.get(session_ref)
            meta = self._session_meta.get(session_ref)
        if generation is None or meta is None:
            return None
        return dict(
            generation=generation, recipient=meta["recipient"], task_id=meta["task_id"],
            attempt_id=meta.get("attempt_id"),
        )

    def begin_delivery(self, session_ref: str, capsule_revision: str) -> dict | None:
        from . import deliveries as delegation_deliveries

        with self._lock:
            generation = self._pending_generation.get(session_ref)
            meta = self._session_meta.get(session_ref)
        if generation is None or meta is None or self._store is None:
            return None
        return delegation_deliveries.record_built(
            self._store, recipient=meta["recipient"], context_generation_id=generation["id"],
            capsule_revision=capsule_revision, task_id=meta["task_id"], engine_session_ref=session_ref,
            evidence_source="orchestrating_runtime:generation_delivery_build",
            assignment_ref=meta["assignment_ref"],
        )

    def mark_delivery_transmitted(self, session_ref: str, delivery_id: str) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_transmitted(self._store, delivery_id, "session_prompt_call_issued")

    def mark_delivery_confirmed(self, session_ref: str, delivery_id: str) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_delivery_confirmed(self._store, delivery_id, "session_prompt_response_received")
        with self._lock:
            self._pending_generation.pop(session_ref, None)

    def mark_delivery_uncertain(self, session_ref: str, delivery_id: str, reason: str) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_receipt_uncertain(self._store, delivery_id, reason)

    def mark_delivery_blocked(self, session_ref: str, delivery_id: str, reason: str, *, permanent: bool = False) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_blocked(self._store, delivery_id, reason, "orchestrating_runtime:generation_delivery_blocked")
        if permanent:
            with self._lock:
                self._pending_generation.pop(session_ref, None)

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
        if self._store is not None:
            from . import deliveries as delegation_deliveries

            # T022: initiating a new turn on this session is itself the
            # "subsequent engine turn" evidence for any delivery this
            # session already confirmed but had not yet observed a
            # continuation for.
            delegation_deliveries.mark_continuation_observed_if_pending(
                self._store, session_ref, "next_session_prompt_call_initiated",
            )
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
                elif event.kind == EngineEventKind.COMPACTION_SIGNAL:
                    if self._store is not None:
                        self._observe_compaction_signal(session_ref, event)
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
