"""Narrow delegation/control-plane boundary, separate from storage operations.

This module defines only the provisioning boundary: a ``ControlPlane`` may
``assign``, ``cancel``, and check ``status`` of engine-side work, distinct
from the Store's Markdown/report/governance API.  No tool schema is added by
this seam.  Full engine instruction provisioning (actually assigning routes,
workspaces, and attempts to a live engine) is implemented in T015;
``NotImplementedControlPlane`` fails closed until then.
"""
from __future__ import annotations

from typing import Any, Protocol

from .portable.errors import PortableError, PortableErrorCategory


class ControlPlane(Protocol):
    """Delegation boundary a portable engine adapter implements in T015."""

    def assign(self, task_id: str, **kwargs: Any) -> Any: ...

    def cancel(self, task_id: str, **kwargs: Any) -> Any: ...

    def status(self, task_id: str, **kwargs: Any) -> Any: ...


def _not_implemented() -> PortableError:
    return PortableError(
        category=PortableErrorCategory.ENGINE_FAILURE,
        code="not_implemented",
        remediation="Engine delegation/control-plane provisioning is implemented in T015; "
                    "this boundary intentionally rejects every call until then.",
        retry_eligible=False,
    )


class NotImplementedControlPlane:
    """Default ControlPlane that fails closed on every call."""

    def assign(self, task_id: str, **kwargs: Any) -> Any:
        raise _not_implemented()

    def cancel(self, task_id: str, **kwargs: Any) -> Any:
        raise _not_implemented()

    def status(self, task_id: str, **kwargs: Any) -> Any:
        raise _not_implemented()


class ProvisioningControlPlane:
    """Real ``ControlPlane`` for the delegation operations, backed by
    :class:`delegation.provisioning.ProvisioningService`.

    Bound to one already-resolved coordinator
    :class:`trusted_channel.ChannelContext` -- ``assign``/``cancel``/
    ``status`` still take ``task_id`` (to satisfy the ``ControlPlane``
    Protocol) but reject any call whose ``task_id`` does not match the
    bound channel's task, rather than trusting a model-supplied task id.

    Production wiring (constructing this against a real project ``Store``,
    real ``WorkspaceProvisioner``/``EngineSessionOpener``/
    ``ConfigSnapshotSource``, and a real incoming-ACP-resolved channel) is
    T016/T017 scope; this class is exercised here only against fakes (see
    ``tests/test_delegation_provisioning_recovery.py`` and
    ``tests/engine_fakes.py``).
    """

    def __init__(self, store, channel, provisioning_service) -> None:
        self._store = store
        self._channel = channel
        self._provisioning = provisioning_service

    def _check_task(self, task_id: str) -> None:
        if task_id != self._channel.task_id:
            raise PortableError(
                category=PortableErrorCategory.AUTHORIZATION,
                code="cross_task_channel",
                remediation="This control plane is bound to a different task's channel; obtain a channel for the requested task instead of passing a different task_id.",
                retry_eligible=False,
                field="task_id",
            )

    def assign(self, task_id: str, *, title: str, brief: str, role: str, acceptance: str, request_key: str) -> Any:
        self._check_task(task_id)
        return self._provisioning.delegate(self._channel, title, brief, role, acceptance, request_key)

    def cancel(self, task_id: str, *, assignment_ref: str, reason: str, request_key: str) -> Any:
        self._check_task(task_id)
        from .delegation import contracts as delegation_contracts

        delegation_contracts.validate(
            "coordinator", "cancel",
            dict(assignment_ref=assignment_ref, reason=reason, request_key=request_key),
        )
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                row = db.execute(
                    "SELECT * FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,)
                ).fetchone()
                if row is None or row["task_id"] != task_id:
                    raise PortableError(
                        category=PortableErrorCategory.VALIDATION,
                        code="unknown_assignment_ref",
                        remediation="'assignment_ref' does not match a known delegation for this task; copy it exactly from a prior delegate() result.",
                        retry_eligible=False,
                        field="assignment_ref",
                    )
                db.execute(
                    "UPDATE delegation_provisioning SET status='cancelled', error_code=?, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE idempotency_key=?",
                    (reason, assignment_ref),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        return dict(assignment_ref=assignment_ref, status="cancelled")

    def status(self, task_id: str, **kwargs: Any) -> Any:
        self._check_task(task_id)
        with self._store.connection() as db:
            rows = db.execute(
                "SELECT idempotency_key, role_id, status, step, workspace_binding_id, "
                "engine_session_ref, instruction_delivery, error_code, updated_at "
                "FROM delegation_provisioning WHERE task_id=? ORDER BY updated_at",
                (task_id,),
            ).fetchall()
        return [dict(row) for row in rows]
