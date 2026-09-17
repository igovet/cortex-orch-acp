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
    refusal."""


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
            from ..connections import registry as connection_registry

            current = configuration_service.current(task_id)
            if current is None:
                raise RouteUnresolved("no_configuration_revision")
            state = connection_registry.read_registry(self._environ)
            record = state.connections.get(current["route_connection_id"])
            if record is None:
                raise RouteUnresolved("connection_not_found")
            return record
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
            workspace_resolver=lambda binding_id, launch: (
                workspace_provisioner.root_for(binding_id) or str(project)
            ),
        )
        runtime = OrchestratingRuntime(
            store, adapter, workspace_provisioner,
            ConfigurationServiceSnapshotSource(configuration_service),
        )
        with self._lock:
            self._runtimes.setdefault(project_root, runtime)
            return self._runtimes[project_root]

    def start_prompt(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        project_root=None, source_origin=None, source_event_id=None, host_elicitor=None,
    ):
        if not project_root:
            raise RouteUnresolved("project_root_unresolved")
        try:
            runtime = self._runtime_for_project(project_root)
            return runtime.start_prompt(
                task_id, session_id, content, emit_update, cancel_event,
                source_origin=source_origin, source_event_id=source_event_id, host_elicitor=host_elicitor,
            )
        except PortableError as error:
            # Every qualification/configuration/ownership refusal this
            # runtime raises (no_configuration_revision,
            # instruction_delivery_unqualified, second_coordinator_refused,
            # ...) is surfaced by its own concrete code, never collapsed
            # into one generic message.
            raise RouteUnresolved(error.code) from error

    def cancel(self, session_id):
        for runtime in list(self._runtimes.values()):
            runtime.cancel(session_id)

    # -- T020 /cortex-stop / /cortex-answer entrypoints ------------------

    def stop_task(self, task_id, session_id, emit_update, *, project_root):
        runtime = self._runtime_for_project(project_root)
        return runtime.stop_task(task_id, session_id, emit_update)

    def answer_question(self, task_id, session_id, question, text, emit_update, cancel_event, *, project_root):
        runtime = self._runtime_for_project(project_root)
        return runtime.answer_question(task_id, session_id, question, text, emit_update, cancel_event)

    def list_open_questions(self, task_id, *, project_root):
        runtime = self._runtime_for_project(project_root)
        from . import questions_store

        return questions_store.list_open(runtime._store, task_id)
