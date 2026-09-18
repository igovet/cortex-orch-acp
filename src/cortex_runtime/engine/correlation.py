"""Per-engine-instance correlation tables: portable id <-> wire id.

Exactly one :class:`CorrelationTables` instance exists per engine instance
(see ``session_manager.EngineSessionManager.__init__``); tables are never
shared across engine instances. This is the whole reason two engine
instances can emit identical wire ids (for example both using
``sessionId: "1"`` or the same ``toolCallId``) without ever cross-routing
requests, tool events, permissions, or terminals to each other: a wire id
is only ever looked up inside the one table that produced/owns it.
"""
from __future__ import annotations

import itertools
import threading
import uuid


def _new_portable_id(prefix: str) -> str:
    return prefix + uuid.uuid4().hex[:12]


class CorrelationTables:
    """Session and tool-call id maps, plus this instance's own JSON-RPC id
    sequence for outbound (host -> engine) requests."""

    def __init__(self):
        self._lock = threading.Lock()
        self._session_portable_to_wire: dict[str, str] = {}
        self._session_wire_to_portable: dict[str, str] = {}
        self._tool_call_wire_to_portable: dict[tuple[str, str], str] = {}
        self._tool_call_portable_to_wire: dict[str, tuple[str, str]] = {}
        self._request_ids = itertools.count(1)

    def next_request_id(self) -> int:
        return next(self._request_ids)

    def register_session(self, portable_session_id: str, wire_session_id: str) -> None:
        with self._lock:
            self._session_portable_to_wire[portable_session_id] = wire_session_id
            self._session_wire_to_portable[wire_session_id] = portable_session_id

    def wire_session_id(self, portable_session_id: str) -> str | None:
        with self._lock:
            return self._session_portable_to_wire.get(portable_session_id)

    def portable_session_id(self, wire_session_id: str) -> str | None:
        with self._lock:
            return self._session_wire_to_portable.get(wire_session_id)

    def portable_tool_call_ref(self, wire_session_id: str, wire_tool_call_id: str) -> str:
        """Stable portable tool-call reference for ``(session, wire id)``,
        allocated on first sight. Scoped by ``wire_session_id`` (itself
        already scoped to this instance's own table) so an engine that
        reuses a ``toolCallId`` across its own sessions never collides
        either."""
        key = (wire_session_id, wire_tool_call_id)
        with self._lock:
            portable = self._tool_call_wire_to_portable.get(key)
            if portable is None:
                portable = _new_portable_id("tc_")
                self._tool_call_wire_to_portable[key] = portable
                self._tool_call_portable_to_wire[portable] = key
            return portable

    def release_session(self, portable_session_id: str) -> None:
        with self._lock:
            wire = self._session_portable_to_wire.pop(portable_session_id, None)
            if wire is None:
                return
            self._session_wire_to_portable.pop(wire, None)
            stale = [key for key in self._tool_call_wire_to_portable if key[0] == wire]
            for key in stale:
                portable = self._tool_call_wire_to_portable.pop(key)
                self._tool_call_portable_to_wire.pop(portable, None)
