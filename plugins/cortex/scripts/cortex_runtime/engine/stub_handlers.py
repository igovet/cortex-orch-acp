"""Agent-to-client request handling.

Cortex's outgoing ACP client advertises no client capabilities (no ``fs``,
no ``terminal``) -- see ``session_manager.EngineSessionManager.negotiate``,
which sends an empty ``clientCapabilities`` object. Consequently every
agent-initiated request (``request_permission``, ``fs/*``, ``terminal/*``,
or anything else) is refused with ``-32601`` and never actually serviced;
this module is the one place that response is shaped, so it stays
consistent and testable independent of the client/session manager.
"""
from __future__ import annotations

UNSUPPORTED_CLIENT_REQUEST_CODE = -32601


def unsupported_client_request_error(method: str) -> tuple[int, str]:
    """The ``(code, message)`` this client sends back for any agent-to-client
    request, since it advertises no capability that could service one."""
    return (
        UNSUPPORTED_CLIENT_REQUEST_CODE,
        f"Method '{method}' is not supported by this client; no fs/terminal "
        "capability is advertised.",
    )
