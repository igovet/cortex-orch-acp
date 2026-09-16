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
