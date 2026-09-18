"""The orchestration boundary behind the ACP protocol surface.

``IncomingRuntime`` is the seam ``agent.Agent`` calls after durable source
capture (``bootstrap.bootstrap_task`` / ``bootstrap.append_source_event``)
has already committed. Production wiring uses ``UnavailableRuntime``, which
always fails closed: real orchestration provisioning is out of scope for
T009 (see ``control_plane.NotImplementedControlPlane``, the analogous seam
for engine delegation). A scripted, clearly test-only ``FakeRuntime`` lives
under ``tests/acp_fakes.py`` -- never here, and never imported by production
code -- so protocol/transport tests can exercise streaming updates and
cancellation without claiming to be real orchestration.
"""
from __future__ import annotations

import threading
from typing import Callable, Protocol, runtime_checkable

from ..portable.engine_contract import StopReason


@runtime_checkable
class IncomingRuntime(Protocol):
    """Structural contract every incoming-runtime implementation satisfies."""

    def start_prompt(
        self,
        task_id: str,
        session_id: str,
        content: str,
        emit_update: Callable[[str], bool],
        cancel_event: threading.Event,
    ) -> StopReason:
        """Run one turn, calling ``emit_update(text)`` for streamed output.

        Returns the :class:`StopReason` the turn ended with, or raises.
        ``cancel_event`` is set by the agent when a ``session/cancel``
        notification (or an undeliverable-update timeout) arrives; an
        implementation should stop promptly once it is set.
        """
        ...

    def cancel(self, session_id: str) -> None:
        """Best-effort request to stop ``session_id``'s in-flight turn, if any."""
        ...


class OrchestrationUnavailable(Exception):
    """Raised by :class:`UnavailableRuntime` after durable capture succeeds.

    ``agent.Agent`` maps this to a JSON-RPC error with code ``-32001``,
    message ``"orchestration_unavailable"`` and
    ``data={"retryable": False}`` -- never an ``end_turn`` result and never
    an echo of the prompt content.
    """


class UnavailableRuntime:
    """Production runtime: fails closed on every prompt.

    Durable capture (bootstrap/append) has already committed by the time
    this is called; only the actual orchestration turn is unimplemented.
    """

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        raise OrchestrationUnavailable("orchestration_unavailable")

    def cancel(self, session_id):
        return None
