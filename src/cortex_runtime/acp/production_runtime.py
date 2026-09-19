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
from ..delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from ..contracts import StoreError
from ..portable.errors import PortableError
from ..project_storage import canonical_project, migrate_legacy_project_storage, project_store_directory
from ..store import Store


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
    ``route_policy_not_authorized`` on screen, while the refusal itself
    already knew which role, engine, model and effort it had refused and
    exactly what the policy allows instead -- and that sentence was thrown
    away one frame before the client.
    """

    def __init__(self, code: str, remediation: str | None = None) -> None:
        super().__init__(code)
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
        # T025: one isolated-snapshot provisioner per project (not per task
        # -- ``WorkspaceProvisioner.create`` only ever receives an
        # ``attempt_id``; see that class's own docstring for how it
        # resolves the owning task). Falls back to the pre-T025 shared
        # project root automatically (recorded, visible) whenever a
        # snapshot cannot safely be taken.
        workspace_provisioner = IsolatedSnapshotWorkspaceProvisioner(store, str(project))
        adapter = EngineSessionManagerAdapter(
            self._connection_record_factory(configuration_service), lambda task_id: str(project),
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
            # T035 (FINDING-12/-13 closure): "shared_project_root" is the
            # literal sentinel ``acp.orchestrating_runtime`` passes as
            # ``workspace_binding_id`` only for a *coordinator* open (see
            # that module's ``_open_coordinator_session`` call site) -- no
            # real ``workspace_snapshots`` row is ever keyed by that exact
            # string, so ``root_for`` on it always returned ``None`` and
            # this used to fall straight through to the real, writable
            # ``project`` -- the coordinator's whole prior workspace. Route
            # it instead to ``coordinator_root_for``'s read-only snapshot;
            # every other (worker) binding id is unaffected. See that
            # method's own docstring for exactly what this does and does
            # not prevent.
            workspace_resolver=lambda binding_id, launch: (
                workspace_provisioner.coordinator_root_for(launch["task_id"])[0]
                if binding_id == "shared_project_root"
                else workspace_provisioner.root_for(binding_id) or str(project)
            ),
            # EXT-01 finding #1 fix: an isolated worker's ``workspace_root``
            # above is a throwaway copy that excludes both ``.git`` and
            # ``.cortex`` -- it must never be handed to the engine's Cortex
            # MCP server as the project root (that store does not exist
            # there). This project's one real store always lives at
            # ``project`` itself, regardless of which workspace a given
            # attempt executes in.
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
    ):
        if not project_root:
            raise RouteUnresolved("project_root_unresolved")
        try:
            runtime = self._runtime_for_project(project_root)
            return runtime.start_prompt(
                task_id, session_id, content, emit_update, cancel_event,
                source_origin=source_origin, source_event_id=source_event_id, host_elicitor=host_elicitor,
                host_permission_requester=host_permission_requester, host_fs_requester=host_fs_requester,
                emit_tool_call=emit_tool_call,
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

    def disconnect(self, session_id):
        """T028: forwarded from ``acp.agent.Agent._handle_eof``/
        ``handle_writer_closed`` for every session on the disconnected
        connection -- see ``OrchestratingRuntime.disconnect_session``."""
        for runtime in list(self._runtimes.values()):
            runtime.disconnect_session(session_id)

    # -- T020 /cortex-stop / /cortex-answer entrypoints ------------------

    def stop_task(self, task_id, session_id, emit_update, *, project_root):
        runtime = self._runtime_for_project(project_root)
        return runtime.stop_task(task_id, session_id, emit_update)

    def answer_question(self, task_id, session_id, question, text, emit_update, cancel_event, *, project_root, emit_tool_call=None):
        runtime = self._runtime_for_project(project_root)
        return runtime.answer_question(task_id, session_id, question, text, emit_update, cancel_event, emit_tool_call=emit_tool_call)

    def list_open_questions(self, task_id, *, project_root):
        runtime = self._runtime_for_project(project_root)
        from . import questions_store

        return questions_store.list_open(runtime._store, task_id)
