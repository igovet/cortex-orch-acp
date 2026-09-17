"""ResourceBinding: engine/session/action/workspace identity snapshot used
by the permission, filesystem, and terminal bridges (T014).

A :class:`ResourceBinding` is a frozen, fully-validated (fail-closed --
every field required and non-empty) snapshot of exactly which engine
instance/session/context-generation/assignment/attempt/workspace-binding/
ownership-epoch/role a resource operation is scoped to. It is registered
once per ``engine_session_id`` (see
``session_manager.EngineSessionManager.bind_resource``) and looked up again
whenever an agent-to-client request (``session/request_permission``,
``fs/*``, ``terminal/*``) arrives for that wire session.

:class:`BindingRegistry` is the live, mutable, per-``EngineSessionManager``
authority this binding is checked *against* at the moment of use (not only
at receipt): a session replacement, an ownership-epoch bump, a context
generation change, an explicit action supersession, or a session release
all show up here, and :meth:`ResourceBinding.is_current` is how a caller
(see ``permissions.PermissionBridge``) re-checks a pending permission
request is still valid immediately before relaying a host's selection --
never trusting a snapshot taken only once, before a potentially slow host
round-trip.
"""
from __future__ import annotations

import threading
from dataclasses import dataclass
from pathlib import Path

_ROLES = ("coordinator", "worker")


class ResourceBindingError(ValueError):
    """A :class:`ResourceBinding` field failed validation; fail closed."""


def _require_nonempty_str(value, field_name: str) -> str:
    if not isinstance(value, str) or not value:
        raise ResourceBindingError(f"{field_name} must be a non-empty string.")
    return value


@dataclass(frozen=True)
class ResourceBinding:
    """See module docstring. ``workspace_root`` must already be an absolute,
    symlink-resolved path (callers resolve it themselves with
    ``os.path.realpath``/``Path.resolve`` before constructing this -- this
    class only verifies the invariant, it does not perform the resolution,
    so a caller can never accidentally bind to an unresolved/ambiguous
    root)."""

    engine_instance_id: str
    engine_session_id: str
    context_generation_id: str
    assignment_id: str
    attempt_id: str
    workspace_binding_id: str
    workspace_root: str
    ownership_epoch: str
    role: str

    def __post_init__(self) -> None:
        for field_name in (
            "engine_instance_id", "engine_session_id", "context_generation_id",
            "assignment_id", "attempt_id", "workspace_binding_id", "workspace_root",
            "ownership_epoch", "role",
        ):
            _require_nonempty_str(getattr(self, field_name), field_name)
        if self.role not in _ROLES:
            raise ResourceBindingError(f"role must be one of {_ROLES!r}.")
        if not self.workspace_root.startswith("/"):
            raise ResourceBindingError("workspace_root must be an absolute path.")
        resolved = str(Path(self.workspace_root).resolve())
        if resolved != self.workspace_root:
            raise ResourceBindingError(
                "workspace_root must already be a symlink-resolved absolute path "
                "(the caller resolves it before constructing this binding)."
            )

    def is_current(self, snapshot: "BindingSnapshot") -> bool:
        """True only if ``snapshot`` (see ``BindingRegistry.snapshot``)
        shows this binding's session is not released, the action (if any)
        was not individually invalidated, the ownership epoch for this
        binding's workspace has not moved on, and the context generation
        for this engine session has not moved on. A delayed/late approval
        for anything that has changed underneath it must never be relayed
        -- see ``permissions.PermissionBridge``."""
        if snapshot is None:
            return False
        if snapshot.released or snapshot.cancelled:
            return False
        if snapshot.ownership_epoch != self.ownership_epoch:
            return False
        if snapshot.context_generation_id != self.context_generation_id:
            return False
        return True


@dataclass(frozen=True)
class BindingSnapshot:
    """A point-in-time read of the live invalidation state relevant to one
    :class:`ResourceBinding`, as tracked by :class:`BindingRegistry`."""

    context_generation_id: str
    ownership_epoch: str
    released: bool = False
    cancelled: bool = False


class BindingRegistry:
    """Live, per-``EngineSessionManager`` authority for every currently
    bound engine session's :class:`ResourceBinding`, plus the invalidation
    state (session release, ownership-epoch bump, context-generation bump,
    per-action cancellation) that :meth:`ResourceBinding.is_current` checks
    against. One instance is owned by exactly one ``EngineSessionManager``
    (never shared across engine instances), matching
    ``correlation.CorrelationTables``'s own per-instance scoping -- so a
    binding registered on engine A's manager is never visible to engine B's
    dispatch even if both happen to choose an identical ``engine_session_id``
    string (which cannot happen in practice: portable ids are UUID-derived,
    but this is defense in depth, not the only thing preventing cross-worker
    access -- workspace_root containment in ``fs_bridge``/``terminal_bridge``
    is the actual boundary for a same-manager cross-session case)."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._bindings: dict[str, ResourceBinding] = {}
        self._released: set[str] = set()
        self._cancelled_actions: set[tuple[str, str]] = set()
        self._epoch_by_workspace: dict[str, str] = {}
        self._generation_by_session: dict[str, str] = {}

    def bind(self, binding: ResourceBinding) -> None:
        with self._lock:
            self._bindings[binding.engine_session_id] = binding
            self._released.discard(binding.engine_session_id)
            self._epoch_by_workspace.setdefault(binding.workspace_binding_id, binding.ownership_epoch)
            self._generation_by_session[binding.engine_session_id] = binding.context_generation_id

    def get(self, engine_session_id: str) -> ResourceBinding | None:
        with self._lock:
            return self._bindings.get(engine_session_id)

    def all_session_ids(self) -> list[str]:
        with self._lock:
            return list(self._bindings.keys())

    def snapshot(self, binding: ResourceBinding, *, action_key: str | None = None) -> BindingSnapshot:
        with self._lock:
            released = binding.engine_session_id in self._released
            cancelled = action_key is not None and (binding.engine_session_id, action_key) in self._cancelled_actions
            epoch = self._epoch_by_workspace.get(binding.workspace_binding_id, binding.ownership_epoch)
            generation = self._generation_by_session.get(binding.engine_session_id, binding.context_generation_id)
        return BindingSnapshot(
            context_generation_id=generation, ownership_epoch=epoch, released=released, cancelled=cancelled,
        )

    # -- invalidation APIs (T014 design decision 3) -----------------------

    def invalidate_session(self, engine_session_id: str, reason: str | None = None) -> None:
        del reason  # observation-only at the caller; this registry only tracks the flag
        with self._lock:
            self._released.add(engine_session_id)

    def invalidate_epoch(self, workspace_binding_id: str, new_epoch: str) -> None:
        with self._lock:
            self._epoch_by_workspace[workspace_binding_id] = new_epoch

    def invalidate_generation(self, engine_session_id: str, new_generation: str) -> None:
        with self._lock:
            self._generation_by_session[engine_session_id] = new_generation

    def invalidate_action(self, engine_session_id: str, action_key: str) -> None:
        with self._lock:
            self._cancelled_actions.add((engine_session_id, action_key))

    def release(self, engine_session_id: str) -> None:
        """Called on engine-session release: invalidates the session
        immediately (any pending permission on it becomes non-current) and
        forgets the binding/action bookkeeping for it."""
        with self._lock:
            self._released.add(engine_session_id)
            self._bindings.pop(engine_session_id, None)
            self._generation_by_session.pop(engine_session_id, None)
            stale_actions = [key for key in self._cancelled_actions if key[0] == engine_session_id]
            for key in stale_actions:
                self._cancelled_actions.discard(key)
