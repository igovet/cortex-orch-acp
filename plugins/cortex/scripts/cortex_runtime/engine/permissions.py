"""PermissionBridge: relay an engine's ``session/request_permission`` to the
host, bound to engine/session/action/workspace/ownership-epoch identity, and
never relay a stale decision (T014).

Every reply this bridge produces to the *engine* is schema-valid ACP:
either ``{"outcome": {"outcome": "cancelled"}}`` or ``{"outcome":
{"outcome": "selected", "optionId": <one of the engine's own offered
optionIds>}}`` -- never any other shape, and never an ``optionId`` the
engine itself did not offer in this same request. ``allow_always``/
``reject_always`` are deliberately treated as an ordinary single-action
``selected``/``cancelled`` decision: this bridge never writes or consults
any durable "standing rule" state, so no host "always" choice is ever
silently reused for a later, different action.
"""
from __future__ import annotations

import threading

CANCELLED_OUTCOME = {"outcome": {"outcome": "cancelled"}}


class DuplicateEnginePermissionRequestId(Exception):
    """The engine reused a still-outstanding (or never-cleaned-up) wire
    request id for ``session/request_permission``; refused as a protocol
    error, never treated as a fresh request."""


class PermissionBridge:
    def __init__(self, *, correlation, bindings, logs=None, host_permission_requester=None, call_timeout=30.0):
        """``host_permission_requester`` -- ``(engine_session_id, params,
        timeout) -> dict`` or ``None`` -- is the caller-injected seam to the
        actual incoming ACP session/host that owns this engine session (see
        ``acp.host_requests.HostRequester``); when ``None`` every permission
        request is answered ``cancelled`` without ever reaching a host."""
        self._correlation = correlation
        self._bindings = bindings
        self._logs = logs
        self._host_permission_requester = host_permission_requester
        self._call_timeout = call_timeout
        self._lock = threading.Lock()
        self._in_flight: set[tuple[str, object]] = set()

    def _observe(self, event, **fields) -> None:
        if self._logs is not None:
            self._logs.observe(event, **fields)

    def handle(self, *, engine_session_id, wire_session_id, wire_request_id, params) -> dict:
        """Returns the JSON-RPC *result* payload to send back to the
        engine. Raises :class:`DuplicateEnginePermissionRequestId` for a
        wire request id already in flight on this engine session -- the
        caller (see ``session_manager``) turns that into a JSON-RPC error
        reply, never a result."""
        dedup_key = (engine_session_id, wire_request_id)
        with self._lock:
            if dedup_key in self._in_flight:
                self._observe("permission_duplicate_request_id", engine_session_id=engine_session_id)
                raise DuplicateEnginePermissionRequestId(str(wire_request_id))
            self._in_flight.add(dedup_key)
        try:
            return self._handle_one(engine_session_id, wire_session_id, params)
        finally:
            with self._lock:
                self._in_flight.discard(dedup_key)

    def _action_ref(self, wire_session_id, params):
        tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
        wire_tool_call_id = (tool_call or {}).get("toolCallId") if isinstance(tool_call, dict) else None
        if not wire_tool_call_id or not wire_session_id:
            return None
        return self._correlation.portable_tool_call_ref(wire_session_id, wire_tool_call_id)

    def _handle_one(self, engine_session_id, wire_session_id, params) -> dict:
        binding = self._bindings.get(engine_session_id)
        if binding is None:
            self._observe("permission_denied_no_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        action_ref = self._action_ref(wire_session_id, params)
        snapshot = self._bindings.snapshot(binding, action_key=action_ref)
        if not binding.is_current(snapshot):
            self._observe("permission_denied_stale_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        if self._host_permission_requester is None:
            self._observe("permission_no_host_requester", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        try:
            host_response = self._host_permission_requester(engine_session_id, params, self._call_timeout)
        except Exception:  # noqa: BLE001 - any host-side failure/timeout/EOF maps to cancelled
            self._observe("permission_host_request_failed", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        # Re-check binding currency immediately before relaying a selection:
        # never trust the snapshot taken before the (potentially slow) host
        # round-trip -- a session replacement/epoch bump/generation bump/
        # cancellation that happened *during* that round-trip must still
        # win. A late reply that loses this race is exactly the
        # "late_permission_response" case the caller/tests exercise.
        snapshot_after = self._bindings.snapshot(binding, action_key=action_ref)
        if not binding.is_current(snapshot_after):
            self._observe("late_permission_response", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        return self._validate_host_response(host_response, params)

    def _validate_host_response(self, host_response, params) -> dict:
        if not isinstance(host_response, dict):
            return dict(CANCELLED_OUTCOME)
        outcome = host_response.get("outcome")
        if not isinstance(outcome, dict) or outcome.get("outcome") != "selected":
            return dict(CANCELLED_OUTCOME)
        option_id = outcome.get("optionId")
        options = (params or {}).get("options") if isinstance(params, dict) else None
        offered_ids = {item.get("optionId") for item in options if isinstance(item, dict)} if isinstance(options, list) else set()
        if option_id not in offered_ids:
            self._observe("permission_unknown_option_id", option_id=option_id)
            return dict(CANCELLED_OUTCOME)
        return {"outcome": {"outcome": "selected", "optionId": option_id}}
