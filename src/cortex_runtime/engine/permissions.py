"""Final ACP permission fence: native tools cannot be restored by approval.

Only explicitly named MCP calls can proceed to their channel-bound gateway.
Titles, descriptions and raw inputs never supply tool identity or authority.
"""
import re
import threading

CANCELLED_OUTCOME = {'outcome': {'outcome': 'cancelled'}}
COORDINATOR_EDIT_REFUSAL_REASON = 'Native tools are disabled. Use the advertised Cortex gateway.'
COORDINATOR_MUTATION_REFUSAL_REASON = COORDINATOR_EDIT_REFUSAL_REASON
COORDINATOR_PRIVATE_OR_CHECK_REFUSAL_REASON = COORDINATOR_EDIT_REFUSAL_REASON

def _tool_call(params) -> dict:
    tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
    return tool_call if isinstance(tool_call, dict) else {}



def _auto_allow_option_id(params):
    """Use an offered MCP allow option; gateway authorization still follows.

    Prefer a one-shot option. No native action reaches this helper and no
    engine approval establishes a persistent Cortex authorization.
    """
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    for kind in ("allow_once", "allow_always"):
        for option in options:
            if isinstance(option, dict) and option.get("kind") == kind and isinstance(option.get('optionId'),str) and option['optionId']:
                return option.get("optionId")
    return None



def _auto_reject_option_id(params):
    """The offered ``optionId`` whose own ``kind`` is ``"reject_once"``
    (preferred) or, failing that, ``"reject_always"`` -- or ``None`` if the
    engine offered neither, in which case the caller falls back to
    ``CANCELLED_OUTCOME``. Never invents an id the engine did not itself
    offer (same invariant as :func:`_auto_allow_option_id`). ``"reject_once"`` is
    preferred over ``"reject_always"`` for the same reason
    ``_auto_allow_option_id`` prefers ``"allow_once"`` but may use an
    engine-offered ``"allow_always"`` when it is the only allow option. The
    bridge still makes a fresh local decision and does not persist a standing
    Cortex rule."""
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    by_kind = {
        option.get("kind"): option.get("optionId")
        for option in options if isinstance(option, dict) and isinstance(option.get('optionId'),str) and option['optionId']
    }
    return by_kind.get("reject_once") or by_kind.get("reject_always")



class DuplicateEnginePermissionRequestId(Exception):
    """The engine reused a still-outstanding (or never-cleaned-up) wire
    request id for ``session/request_permission``; refused as a protocol
    error, never treated as a fresh request."""



class PermissionBridge:
    def __init__(
        self, *, correlation, bindings, logs=None, host_permission_requester=None, call_timeout=30.0,
        coordinator_refusal_notifier=None,
    ):
        self._correlation = correlation
        self._bindings = bindings
        self._logs = logs
        self._host_permission_requester = host_permission_requester
        self._call_timeout = call_timeout
        self._coordinator_refusal_notifier = coordinator_refusal_notifier
        self._lock = threading.Lock()
        self._in_flight: set[tuple[str, object]] = set()
        # T035 EXT-01 action-supersession gap closure: which portable
        # action refs (see ``_action_ref``) currently have a host round
        # trip outstanding, indexed by wire session id -- a plain refcount
        # rather than a set because two concurrent requests can legitimately
        # name the *same* toolCallId (an engine retry while the first
        # round trip is still outstanding), and the second's own eventual
        # completion must never make the first look "no longer pending" out
        # from under it. The only reader is
        # :meth:`pending_actions_for_wire_session`, called by
        # ``session_manager.EngineSessionManager`` at the one place a whole
        # turn (and everything it requested permission for) is abandoned or
        # explicitly cancelled -- see that method's own docstring for why
        # this bridge, not that caller, is the one with this data at hand.
        self._pending_actions: dict[str, dict[str, int]] = {}

    def pending_actions_for_wire_session(self, wire_session_id: str) -> list[str]:
        """Snapshot of every action ref with a host round trip currently
        outstanding for ``wire_session_id`` -- see
        ``session_manager.EngineSessionManager._abandon_turn``/``cancel``,
        the only production callers (T035 EXT-01 action-supersession gap
        closure)."""
        with self._lock:
            return list(self._pending_actions.get(wire_session_id, {}))

    def _track_action_start(self, wire_session_id, action_ref) -> None:
        if action_ref is None:
            return
        with self._lock:
            by_action = self._pending_actions.setdefault(wire_session_id, {})
            by_action[action_ref] = by_action.get(action_ref, 0) + 1

    def _track_action_end(self, wire_session_id, action_ref) -> None:
        if action_ref is None:
            return
        with self._lock:
            by_action = self._pending_actions.get(wire_session_id)
            if not by_action or action_ref not in by_action:
                return
            by_action[action_ref] -= 1
            if by_action[action_ref] <= 0:
                del by_action[action_ref]
            if not by_action:
                self._pending_actions.pop(wire_session_id, None)

    def _observe(self, event, **fields) -> None:
        if self._logs is not None:
            self._logs.observe(event, **fields)

    def _notify_refusal(self, engine_session_id: str, reason: str = COORDINATOR_MUTATION_REFUSAL_REASON) -> None:
        if self._coordinator_refusal_notifier is None:
            return
        try:
                self._coordinator_refusal_notifier(engine_session_id, reason)
        except Exception:  # noqa: BLE001 - a notifier failure must never affect the refusal itself
            self._observe("coordinator_refusal_notifier_failed", engine_session_id=engine_session_id)

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
        action_ref = self._action_ref(wire_session_id, params)
        self._track_action_start(wire_session_id, action_ref)
        try:
            return self._handle_one(engine_session_id, wire_session_id, params, action_ref)
        finally:
            with self._lock:
                self._in_flight.discard(dedup_key)
            self._track_action_end(wire_session_id, action_ref)

    def _action_ref(self, wire_session_id, params):
        tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
        wire_tool_call_id = (tool_call or {}).get("toolCallId") if isinstance(tool_call, dict) else None
        if not wire_tool_call_id or not wire_session_id:
            return None
        return self._correlation.portable_tool_call_ref(wire_session_id, wire_tool_call_id)

    def _handle_one(self, engine_session_id, wire_session_id, params, action_ref) -> dict:
        binding = self._bindings.get(engine_session_id)
        if binding is None:
            self._observe("permission_denied_no_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        snapshot = self._bindings.snapshot(binding, action_key=action_ref)
        if not binding.is_current(snapshot):
            self._observe("permission_denied_stale_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        # Native actions are absent from the engine registry. A permission
        # request cannot restore one, even with host approval. MCP calls are
        # mediated again by their channel-bound runtime gateway before effects.
        call=_tool_call(params)
        name=call.get('name')
        mediated=isinstance(name,str) and re.fullmatch(r'mcp__[A-Za-z0-9_-]+__[A-Za-z0-9_-]+',name) is not None
        if mediated:
            option=_auto_allow_option_id(params)
            if option is not None:
                return dict(outcome=dict(outcome='selected',optionId=option))
        self._observe('native_tool_denied',engine_session_id=engine_session_id)
        self._notify_refusal(engine_session_id,'Native tools are disabled. Use the advertised Cortex gateway.')
        option=_auto_reject_option_id(params)
        return dict(outcome=dict(outcome='selected',optionId=option)) if option is not None else dict(CANCELLED_OUTCOME)
