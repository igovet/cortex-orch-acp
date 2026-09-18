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

T035 ("workers get full access except deletions", user requirement):
a **worker** binding (``ResourceBinding.role == "worker"``) whose request
:func:`requires_confirmation` classifies as *not* a deletion is answered
locally with that request's own offered ``allow_once`` option, without
ever reaching the host -- so a worker's ordinary operations are never
slowed down by a confirmation prompt. A coordinator binding is never
auto-approved here regardless of classification: coordinators run in a
non-editing engine mode already (see ``configuration.service``/the T035
mode-floor work) and any permission request they somehow still trigger
must keep reaching the host exactly as before, unchanged by this feature.

HONEST BOUNDARY: this only ever inspects the ACP ``toolCall`` payload the
engine itself chose to declare (``kind``/``rawInput``/``title``) on a
``session/request_permission`` call the engine actually issued. A native
tool that deletes a file without ever issuing that request (see
``engine/fs_bridge.py``, ``terminal_bridge.py``, ``resource_policy.py``'s
own docstrings on the same limit) is invisible here and is not, and
cannot be, caught by this bridge.
"""
from __future__ import annotations

import re
import threading

CANCELLED_OUTCOME = {"outcome": {"outcome": "cancelled"}}

# ToolKind values (ACP schema-1.4.0's $defs/ToolKind) that unambiguously
# remove or replace a file/target. "move" is included because a rename can
# silently destroy an existing file at the destination path.
_DELETION_TOOL_KINDS = frozenset({"delete", "move"})

# ToolKind values that are never a deletion under the ACP spec's own
# definition, and get no further command-text scan.
_NON_DELETION_TOOL_KINDS = frozenset({"read", "edit", "search", "fetch", "think", "switch_mode", "execute"})

# Deliberately conservative shell idioms that remove or irreversibly
# truncate file content: bare rm/rmdir/unlink/shred, git clean, `find
# -delete`, mv (can overwrite/destroy a target), and a truncating '>'
# redirect (never '>>', which only appends). This is pattern-matching over
# a command string -- inherently incomplete (see module docstring) -- so it
# is only ever used to *upgrade* an "execute" tool call to "ask", never to
# downgrade a "delete"/"move" kind the engine already declared.
_DELETION_COMMAND_PATTERN = re.compile(
    r"(?:^|[;&|]\s*)(?:sudo\s+)?(?:rm|rmdir|unlink|shred|git\s+clean|mv|find\b[^;&|]*-delete)\b"
    r"|(?<!>)>(?!>)"
    r"|\bdelet(?:e|ing|ed)\b",
    re.IGNORECASE,
)


def _tool_call(params) -> dict:
    tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
    return tool_call if isinstance(tool_call, dict) else {}


def _command_text(tool_call: dict) -> str:
    raw_input = tool_call.get("rawInput")
    parts = []
    if isinstance(raw_input, dict):
        for key in ("command", "cmd", "script"):
            value = raw_input.get(key)
            if isinstance(value, str):
                parts.append(value)
            elif isinstance(value, list):
                parts.append(" ".join(str(item) for item in value))
    title = tool_call.get("title")
    if isinstance(title, str):
        parts.append(title)
    return " ".join(parts)


def requires_confirmation(params) -> bool:
    """True unless this ``session/request_permission`` call is confidently
    classified as *not* a file deletion -- the default is to ask.

    - ``toolCall.kind`` of ``"delete"``/``"move"`` (ACP's own vocabulary)
      is always a deletion.
    - ``toolCall.kind`` of ``"execute"`` is a deletion only when its
      ``rawInput`` command text (or, failing that, its ``title``) matches
      a known destructive shell idiom (``rm``, ``rmdir``, ``unlink``,
      ``shred``, ``git clean``, ``find ... -delete``, ``mv``, or a
      truncating ``>`` redirect) -- see the module-level pattern and its
      own caveat.
    - Every other recognized ``toolCall.kind`` (``read``, ``edit``,
      ``search``, ``fetch``, ``think``, ``switch_mode``) is never a
      deletion.
    - A missing, null, or unrecognized ``kind`` (including ACP's own
      catch-all ``"other"``) is never guessed at -- it is always treated
      as a deletion (i.e. this returns ``True``), so an unclassifiable
      request always asks rather than silently proceeding.
    """
    tool_call = _tool_call(params)
    kind = tool_call.get("kind")
    if kind in _DELETION_TOOL_KINDS:
        return True
    if kind == "execute":
        return bool(_DELETION_COMMAND_PATTERN.search(_command_text(tool_call)))
    if kind in _NON_DELETION_TOOL_KINDS:
        return False
    return True


def _auto_allow_option_id(params):
    """The offered ``optionId`` whose own ``kind`` is ``"allow_once"``, or
    ``None`` if the engine did not offer one -- never invents an id the
    engine did not itself offer (same invariant as
    :meth:`PermissionBridge._validate_host_response`), and never picks
    ``"allow_always"`` (this auto-approval is a fresh per-request decision,
    not a persisted standing rule -- see module docstring)."""
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    for option in options:
        if isinstance(option, dict) and option.get("kind") == "allow_once":
            return option.get("optionId")
    return None


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
        if binding.role == "worker" and not requires_confirmation(params):
            option_id = _auto_allow_option_id(params)
            if option_id is not None:
                self._observe("permission_auto_approved", engine_session_id=engine_session_id)
                return {"outcome": {"outcome": "selected", "optionId": option_id}}
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
