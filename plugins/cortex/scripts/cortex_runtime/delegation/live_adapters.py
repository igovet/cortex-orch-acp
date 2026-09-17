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


class EngineSessionManagerAdapter:
    """Live ``acp.orchestrating_runtime.SessionRuntime`` over one
    ``EngineSessionManager`` per attempt. See module docstring."""

    def __init__(
        self,
        connection_record_factory: Callable[[str], object],
        workspace_root_factory: Callable[[str], str],
        *,
        environ: Mapping[str, str] | None = None,
        **manager_kwargs,
    ) -> None:
        self._connection_record_factory = connection_record_factory
        self._workspace_root_factory = workspace_root_factory
        self._environ = environ
        self._manager_kwargs = manager_kwargs
        self._lock = threading.Lock()
        self._managers: dict[str, object] = {}
        self._sessions: dict[str, str] = {}
        self._session_to_attempt: dict[str, str] = {}

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
        return session_id

    def lookup_owned(self, attempt_id: str) -> str:
        with self._lock:
            return "alive" if attempt_id in self._sessions else "absent"

    def terminate_owned(self, attempt_id: str) -> None:
        with self._lock:
            manager = self._managers.pop(attempt_id, None)
            session_id = self._sessions.pop(attempt_id, None)
            if session_id is not None:
                self._session_to_attempt.pop(session_id, None)
        if manager is not None:
            manager.shutdown(grace=2.0)

    def prompt(self, session_ref: str, message: str, cancel_event: threading.Event):
        from ..acp.orchestrating_runtime import PromptOutcome
        from ..portable.engine_contract import EngineEventKind, StopReason

        with self._lock:
            attempt_id = self._session_to_attempt.get(session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is None:
            raise RuntimeError(f"prompt() called on an unopened/terminated session {session_ref!r}.")
        text_parts: list[str] = []
        stop_reason = StopReason.END_TURN
        for event in manager.prompt_turn(session_ref, message):
            if cancel_event.is_set():
                manager.cancel(session_ref)
            if event.kind == EngineEventKind.TURN_DELTA and event.delta_text:
                text_parts.append(event.delta_text)
            elif event.kind == EngineEventKind.TURN_END:
                stop_reason = event.stop_reason or StopReason.END_TURN
        return PromptOutcome(stop_reason, "".join(text_parts))
