"""Production ``IncomingRuntime``: a real, per-project
``OrchestratingRuntime`` backed by live adapters, constructed lazily per
project root and refusing a prompt explicitly (never silently) when this
task's route/configuration cannot yet be resolved.

``cortex_acp.py`` passes an instance of this class as ``runtime=`` to
``acp.agent.main`` whenever ``connection_record_factory`` can be built with
no new user configuration (see that entry point). ``acp.agent.Agent``
detects (once, at construction) that this class's ``start_prompt`` accepts
an extra ``project_root`` kwarg and supplies the caller's already-resolved
``session["project"]`` -- the one piece ``acp.runtime.IncomingRuntime``'s
original 5-argument contract does not carry, needed here because one ACP
agent process can serve more than one project (``Agent.stores``, mirrored
here).
"""
from __future__ import annotations

import threading

from .orchestrating_runtime import OrchestratingRuntime
from .runtime import OrchestrationUnavailable
from ..configuration.service import ConfigurationService
from ..delegation.live_adapters import (
    ConfigurationServiceSnapshotSource, EngineSessionManagerAdapter,
)
from ..delegation.workspace_isolation import DirectProjectWorkspaceProvisioner
from ..contracts import StoreError
from ..portable.errors import PortableError
from ..project_storage import canonical_project, migrate_legacy_project_storage, project_store_directory
from ..store import Store
from .. import task_workspace


class RouteUnresolved(OrchestrationUnavailable):
    """Raised instead of a bare ``OrchestrationUnavailable`` when this
    task's route/configuration/qualification is the concrete reason a
    prompt cannot be orchestrated yet. ``str(this)`` is the specific
    ``PortableError.code`` (for example ``no_configuration_revision`` or
    ``instruction_delivery_unqualified``), which ``acp.agent.Agent`` places
    in the ACP error response's ``data.reason`` -- never a silent generic
    refusal.

    ``remediation`` carries the refusal's own sentence, when the underlying
    :class:`PortableError` had one, so the person reading it is told what
    to change. Live (2026-09-19) a thread became unusable with nothing but
    a bare refusal code on screen, while the refusal itself
    already knew which role, engine, model and effort it had refused and
    exactly what the policy allows instead -- and that sentence was thrown
    away one frame before the client.
    """

    #: Never: a route/configuration/qualification refusal is settled, not a
    #: transient failure a retry could survive. Read duck-typed by
    #: ``delegation.provisioning``, which must not import this module.
    retry_eligible = False

    def __init__(self, code: str, remediation: str | None = None) -> None:
        super().__init__(code)
        self.code = code
        self.remediation = remediation


class ProductionOrchestratingRuntime:
    """Lazily builds/caches one live ``OrchestratingRuntime`` per project.

    Resolves each task's current user-selected downstream engine route
    itself, from that project's existing T008
    ``configuration.service.ConfigurationService`` and the existing
    connections registry (see ``connections.registry.read_registry``) --
    the same route a normal (non-orchestration) engine session would use.
    No new user configuration surface is introduced. If a task has no
    resolvable/qualified route yet, ``start_prompt`` raises
    :class:`RouteUnresolved` -- mapped by ``acp.agent.Agent`` to an
    explicit ``-32001`` ACP error naming the reason, never a silent
    fallback.
    """

    def __init__(self, *, environ=None) -> None:
        self._environ = environ
        self._lock = threading.Lock()
        self._runtimes: dict[str, OrchestratingRuntime] = {}
        self.last_reconcile = None

    def _connection_record_factory(self, configuration_service):
        def factory(task_id):
            from ..connections import onboarding as connection_onboarding
            from ..connections import registry as connection_registry

            # T035 fix: a fresh task's first coordinator open must resolve
            # the same not-yet-active revision
            # ``ConfigurationServiceSnapshotSource.snapshot`` (T031) already
            # tolerates -- ``EngineSessionManagerAdapter.open`` calls this
            # factory *before* its own post-open ``ConfigurationService
            # .apply()`` call, which is the only thing that ever promotes a
            # 'selected' revision to 'active'. Requiring an already-active
            # revision here made that promotion unreachable: open() needed
            # an active revision, and a revision only becomes active after
            # open() succeeds. ``current_or_latest`` grants the same
            # tolerance as the snapshot source (and, like it, never returns
            # a 'rejected'/'failed' revision -- see its own docstring).
            current = configuration_service.current_or_latest(task_id)
            if current is None:
                raise RouteUnresolved("no_configuration_revision")
            connection_id = current["route_connection_id"]
            state = connection_registry.read_registry(self._environ)
            if connection_id not in state.connections:
                raise RouteUnresolved("connection_not_found")
            # FINDING 09 fix: a route naming a *known* connection that is
            # not actually usable (login expired, credential file missing/
            # invalid, keyring unreachable, ...) must never be collapsed
            # into the same "no_configuration_revision" refusal a genuinely
            # unconfigured task gets -- that sends the user hunting through
            # routes/defaults instead of at their expired login. Re-derive
            # this connection's live state the same way the non-
            # orchestration engine-session path already does
            # (``connections.onboarding.resolve_connection_for_route``,
            # itself reusing ``_STATE_ERROR_CODE``'s existing
            # ``connection_requires_reauthorization``/``connection_unavailable``/
            # ``connection_invalid`` vocabulary -- never a new code) and
            # fail closed, naming both the reason and the connection,
            # instead of silently returning an unusable record for
            # ``EngineSessionManagerAdapter.open`` to fail on some other,
            # opaquer way. Never substitutes a different connection: this is
            # a refusal, not a fallback.
            try:
                return connection_onboarding.resolve_connection_for_route(connection_id, environ=self._environ)
            except PortableError as error:
                raise RouteUnresolved(f"{error.code}:{connection_id}", error.remediation) from error
        return factory

    def _runtime_for_project(self, project_root: str) -> OrchestratingRuntime:
        with self._lock:
            existing = self._runtimes.get(project_root)
            if existing is not None:
                return existing
        project = canonical_project(project_root)
        migrate_legacy_project_storage(project)
        directory = project_store_directory(project)
        if not (directory / "cortex.sqlite3").exists():
            raise RouteUnresolved("task_not_bound")
        store = Store(directory, initialize=False, project_root=project)
        # T019: best-effort, observed crash reconciliation once per project
        # at first use, in addition to the recovery every store call already
        # performs for its own task -- surfaces/quarantines leftovers from a
        # crash between requests rather than waiting for the next call that
        # happens to touch that exact task.
        try:
            self.last_reconcile = store.reconcile()
        except StoreError as error:
            self.last_reconcile = str(error)
        configuration_service = ConfigurationService(store)
        # The runtime reserves disjoint scopes transactionally across this
        # project's tasks. Each engine process runs in the engine jail, which
        # makes only its assignment's scopes writable.
        workspace_provisioner = DirectProjectWorkspaceProvisioner(store, str(project))

        def task_tree(task_id):
            # A task opened in a linked worktree works there; the store
            # (and the MCP server's project root below) stays the project.
            return task_workspace.store_task_root(store, task_id)

        def resolve_workspace(binding_id, launch):
            if binding_id == "shared_project_root":
                return task_tree(launch["task_id"])
            root = workspace_provisioner.root_for(binding_id)
            if not root:
                raise StoreError("workspace_snapshot_unavailable")
            return root

        adapter = EngineSessionManagerAdapter(
            self._connection_record_factory(configuration_service), task_tree,
            environ=self._environ,
            # T035 fix: production was never passing ``store=`` here, so
            # every ``self._store is not None``-gated feature in
            # ``EngineSessionManagerAdapter.open`` (T017 capacity tracking,
            # T022 context-generation/delivery tracking, T023 effective-
            # context recording, T027 dispatch authorization/route audit,
            # and -- the specific cause a fresh task could never actually
            # confirm its configuration even once the connection-record
            # factory above stopped deadlocking -- T031's own
            # ``_maybe_apply_configuration``) was silently inert in
            # production. Every one of those already only activates when a
            # store is supplied (see ``EngineSessionManagerAdapter``'s own
            # docstring); this project's real store is exactly that.
            store=store,
            # ``shared_project_root`` is the literal sentinel used for the
            # coordinator and direct-project workers. It resolves to the
            # task's own working tree (the exact directory the client opened,
            # a linked worktree included), never a stale coordinator snapshot.
            workspace_resolver=resolve_workspace,
            broker_mcp=True,
            # The Cortex MCP server opens the store by the logical project
            # root; its file tools resolve the task's tree themselves.
            project_root_factory=lambda task_id: str(project),
        )
        # T028: bounded owned-resource reaper sweep, once per project at
        # first use in this process (see engine.reaper's module docstring)
        # -- re-probes any capacity reservation/isolated workspace a prior
        # crash left ambiguous/uncleaned; never touches a process/path this
        # Cortex process did not itself own, and never blocks start_prompt
        # on anything beyond this one bounded pass.
        try:
            from ..engine import reaper as engine_reaper

            engine_reaper.sweep(store, adapter)
        except Exception:
            pass
        runtime = OrchestratingRuntime(
            store, adapter, workspace_provisioner,
            ConfigurationServiceSnapshotSource(configuration_service),
        )
        # EXT-01 #5 fix: previously never wired at all in production, so
        # ``host_permission_requester``/``host_fs_requester`` existed only on
        # paper here -- a worker's deletion confirmation could never reach a
        # host, and an unsaved-editor-buffer check was always skipped (see
        # EXT-01's own finding #5). ``adapter`` must exist before ``runtime``
        # (the runtime's own constructor requires an already-built
        # ``SessionRuntime``), so this back-reference is set only now, right
        # after both exist -- see ``EngineSessionManagerAdapter.set_runtime``
        # for exactly how it is used (never anything more than a read-only
        # lookup: this adapter never mutates the runtime).
        adapter.set_runtime(runtime)
        with self._lock:
            self._runtimes.setdefault(project_root, runtime)
            return self._runtimes[project_root]

    def start_prompt(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        project_root=None, source_origin=None, source_event_id=None, host_elicitor=None,
        emit_tool_call=None, host_permission_requester=None, host_fs_requester=None,
        host_mcp_servers=None, prompt_blocks=None, client_surface=None, emit_display_update=None,
    ):
        if not project_root:
            raise RouteUnresolved("project_root_unresolved")
        try:
            runtime = self._runtime_for_project(project_root)
            return runtime.start_prompt(
                task_id, session_id, content, emit_update, cancel_event,
                source_origin=source_origin, source_event_id=source_event_id, host_elicitor=host_elicitor,
                host_permission_requester=host_permission_requester, host_fs_requester=host_fs_requester,
                emit_tool_call=emit_tool_call, emit_display_update=emit_display_update,
                host_mcp_servers=host_mcp_servers,
                prompt_blocks=prompt_blocks,
                client_surface=client_surface,
            )
        except PortableError as error:
            # Every qualification/configuration/ownership refusal this
            # runtime raises (no_configuration_revision,
            # instruction_delivery_unqualified, second_coordinator_refused,
            # ...) is surfaced by its own concrete code, never collapsed
            # into one generic message.
            raise RouteUnresolved(error.code, error.remediation) from error

    def cancel(self, session_id):
        for runtime in list(self._runtimes.values()):
            runtime.cancel(session_id)

    def active_session_owner_epoch(self, project_root, task_id, session_id):
        """Return only an already-open, session-bound coordinator's epoch."""
        with self._lock:
            runtime = self._runtimes.get(project_root)
        return runtime.active_session_owner_epoch(task_id, session_id) if runtime is not None else None

    def cancel_task(self, task_id, session_id, *, project_root, emit_tool_call=None):
        """Forward Paseo's task-wide ``session/cancel`` to the owning project."""
        runtime = self._runtime_for_project(project_root)
        return runtime.cancel_task(task_id, session_id, emit_tool_call=emit_tool_call)

    def disconnect(self, session_id, trigger=None):
        """T028: forwarded from ``acp.agent.Agent._handle_eof``/
        ``handle_writer_closed`` for every session on the disconnected
        connection -- see ``OrchestratingRuntime.disconnect_session``.
        Returns whether any project runtime recorded the disconnect."""
        recorded = False
        for runtime in list(self._runtimes.values()):
            recorded = runtime.disconnect_session(session_id, trigger=trigger) or recorded
        return recorded

    def shutdown(self):
        """Shut down every project runtime owned by this ACP daemon.

        The ACP host may close stdin or its response channel without sending
        a task stop. ``disconnect`` cancels host-bound work, but only this
        path terminates owned engine processes and releases their capacity
        reservations. It never opens a replacement session or performs a
        model call during daemon teardown.
        """
        for runtime in list(self._runtimes.values()):
            try:
                runtime.shutdown()
            except Exception:
                # Keep tearing down other project runtimes. The durable
                # registry remains available to the next recovery/reaper for
                # any process whose final state is ambiguous.
                pass

    # -- T020 /cortex-stop / /cortex-answer entrypoints ------------------

    def stop_task(self, task_id, session_id, emit_update, *, project_root, emit_tool_call=None,
                  stop_source="cortex_stop_command", question_id=None, source_origin=None, source_event_id=None):
        runtime = self._runtime_for_project(project_root)
        return runtime.stop_task(
            task_id, session_id, emit_update, emit_tool_call=emit_tool_call,
            stop_source=stop_source, question_id=question_id,
            source_origin=source_origin, source_event_id=source_event_id,
        )

    def answer_question(self, task_id, session_id, question, text, emit_update, cancel_event, *, project_root, emit_tool_call=None):
        runtime = self._runtime_for_project(project_root)
        return runtime.answer_question(task_id, session_id, question, text, emit_update, cancel_event, emit_tool_call=emit_tool_call)

    def list_open_questions(self, task_id, *, project_root):
        runtime = self._runtime_for_project(project_root)
        from . import questions_store

        return questions_store.list_open(runtime._store, task_id)
