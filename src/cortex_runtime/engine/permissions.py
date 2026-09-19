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
slowed down by a confirmation prompt.

T035 FINDING-12 closure ("the coordinator must not be able to work on the
project" -- see ``.cortex/acp-migration/evidence/T035/FINDING-12-read-only-
mode-does-not-prevent-edits.md`` and ``configuration.policy
.COORDINATOR_MODE_FLOOR``'s own docstring): a **coordinator** binding
(``ResourceBinding.role == "coordinator"``) never reaches the host either,
in either direction. Live evidence proved the previously-chosen mechanism
(the engine's own non-editing mode) does not stop a coordinator from
editing the project, and additionally hides the attempt from this bridge
entirely (that mode never issues ``session/request_permission`` for the
edits it silently performs). With the coordinator floor now on the same
mode the worker floor already uses, edit attempts DO reach this bridge --
and every one :func:`is_coordinator_mutating` classifies as mutating is
refused right here, offline, deterministically, every time; every
non-mutating (read/search/fetch/...) request is auto-approved right here
instead, so a coordinator can still inspect the repository to decide what
to delegate. A coordinator's permission decision is simply never a host's
to make, in either direction.

HONEST BOUNDARY: this only ever inspects the ACP ``toolCall`` payload the
engine itself chose to declare (``kind``/``rawInput``/``title``) on a
``session/request_permission`` call the engine actually issued. A native
tool that deletes a file without ever issuing that request (see
``engine/fs_bridge.py``, ``terminal_bridge.py``, ``resource_policy.py``'s
own docstrings on the same limit) is invisible here and is not, and
cannot be, caught by this bridge. See
``delegation.revision``/``acp.orchestrating_runtime``'s coordinator-turn
workspace-digest comparison for the engine-independent backstop this gap
is exactly why that backstop exists: detection after the fact, not
prevention, for whatever a coordinator's engine executes without ever
asking this bridge at all.
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

# Everything above except the truncating redirect, which is handled
# separately: see _redirects_only_into_the_store.
_DELETION_COMMAND_WITHOUT_REDIRECT = re.compile(
    r"(?:^|[;&|]\s*)(?:sudo\s+)?(?:rm|rmdir|unlink|shred|git\s+clean|mv|find\b[^;&|]*-delete)\b"
    r"|\bdelet(?:e|ing|ed)\b",
    re.IGNORECASE,
)

# A truncating redirect and the path it writes to.
_REDIRECT_TARGET = re.compile(r"(?<!>)>(?!>)\s*'?\"?([^\s'\";&|]+)")


def _redirects_only_into_the_store(text: str) -> bool:
    """True when every truncating redirect in ``text`` writes into
    Cortex's own store (``.cortex/``), and there is at least one.

    A worker fills its report by writing the draft file the runtime handed
    it, and a shell heredoc does that with ``>``. Treating every ``>`` as
    a deletion sent that to the user for approval as an unexplained "Bash"
    prompt offering to "allow access to draft-reports/" -- a question
    about Cortex's own bookkeeping that the user has no way to judge and
    should never have been asked.

    Deliberately narrow: only the redirect is reconsidered. ``rm``,
    ``shred``, ``git clean`` and the rest still count as deletions
    wherever they point, including inside the store.
    """
    targets = _REDIRECT_TARGET.findall(text)
    if not targets:
        return False
    return all(_CORTEX_STORE_PATH.search(target) for target in targets)


# The store's own directory as it appears in a path, absolute or relative.
_CORTEX_STORE_PATH = re.compile(r"(?:^|/)\.cortex/")


# How much of a command a permission card can carry before it stops being
# a question and becomes a wall of text.
_PERMISSION_COMMAND_MAX_CHARS = 400


def _named_for_the_user(params):
    """The same request, with the command it is actually asking about put
    where the user will see it.

    An engine names a shell call after its tool ("Bash"), and the command
    itself sits in ``rawInput`` -- which a client renders, at best, behind
    a disclosure. What reached the user was a prompt reading "Run Command
    / Bash" with a Yes/No: a decision with the one fact needed to make it
    missing. This only ever *adds* to the title; nothing else about the
    request is altered, and a request that already names its command is
    left alone.
    """
    tool_call = _tool_call(params)
    if not tool_call or tool_call.get("kind") != "execute":
        return params
    command = " ".join(_command_text_only(tool_call).split())
    if not command:
        return params
    title = tool_call.get("title")
    title = title if isinstance(title, str) else ""
    if command[:40] in title:
        return params
    if len(command) > _PERMISSION_COMMAND_MAX_CHARS:
        command = command[: _PERMISSION_COMMAND_MAX_CHARS - 1] + "\u2026"
    named = dict(tool_call)
    named["title"] = f"{title}: {command}" if title else command
    updated = dict(params)
    updated["toolCall"] = named
    return updated


def _command_text_only(tool_call: dict) -> str:
    """The command as the engine declared it -- never the title, which is
    what :func:`_command_text` falls back to for classification."""
    raw_input = tool_call.get("rawInput")
    if not isinstance(raw_input, dict):
        return ""
    parts = [raw_input[key] for key in ("command", "cmd", "script") if isinstance(raw_input.get(key), str)]
    return "\n".join(parts)


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


# This runtime's own MCP tools, however an engine spells the server
# prefix ("mcp__cortex__delegate", "mcp.cortex.create_task", ...). They
# are Cortex's own control surface, not an action on the project.
_CORTEX_MCP_TOOL = re.compile(r"(?:^|[^A-Za-z0-9])mcp[._]{1,2}cortex[._]{1,2}", re.I)


def is_cortex_own_tool(params) -> bool:
    """True when this permission request is for one of Cortex's own MCP
    tools -- ``delegate``, ``set_governance``, ``create_task`` and the
    rest.

    They carry no ACP ``kind`` of their own (an MCP call is ``"other"``),
    and an unrecognized kind is refused for a coordinator by design. Live,
    on claude-agent-acp -- which asks permission for every tool, including
    MCP ones -- that refused the coordinator's own ``delegate`` and
    ``set_governance``: it could not delegate, could not set governance,
    and ended its turn asking the user to grant tool access. Codex never
    surfaced it because it does not route MCP calls through this prompt.

    Recognized by tool name only (``toolCall.title``/``rawInput`` naming
    the server-prefixed tool), never by kind -- the point is precisely
    that the kind says nothing.
    """
    tool_call = _tool_call(params)
    for value in (tool_call.get("title"), tool_call.get("name"), (params or {}).get("toolName")):
        if isinstance(value, str) and _CORTEX_MCP_TOOL.search(value):
            return True
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, dict):
        for key in ("name", "tool", "toolName"):
            value = raw_input.get(key)
            if isinstance(value, str) and _CORTEX_MCP_TOOL.search(value):
                return True
    return False


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
    if is_cortex_own_tool(params):
        # Cortex's own MCP surface deletes nothing: a worker publishing a
        # result or reading a skill must not be held for confirmation
        # merely because an MCP call carries no ACP kind.
        return False
    tool_call = _tool_call(params)
    kind = tool_call.get("kind")
    if kind in _DELETION_TOOL_KINDS:
        return True
    if kind == "execute":
        text = _command_text(tool_call)
        if _DELETION_COMMAND_WITHOUT_REDIRECT.search(text):
            return True
        if _redirects_only_into_the_store(text):
            return False
        return bool(_DELETION_COMMAND_PATTERN.search(text))
    if kind in _NON_DELETION_TOOL_KINDS:
        return False
    return True


# T035 FINDING-12 closure ("the coordinator must not be able to work on the
# project" -- see this module's own module docstring and
# configuration.policy.COORDINATOR_MODE_FLOOR's docstring for the live
# evidence that an engine-native non-editing mode does not enforce this):
# reuses requires_confirmation's own toolCall.kind buckets and its
# command-text inspection for "execute" rather than inventing a second
# classification scheme. Every request requires_confirmation already
# treats as ask-worthy for a worker (a deletion, an unrecognized/missing
# kind, or a destructive shell idiom under "execute") is refused outright
# here for a coordinator -- never merely asked about, since a coordinator
# has no legitimate need to mutate anything and the whole point is to
# refuse the intent instead of relaying it to a host. The one addition:
# toolCall.kind == "edit" is never a deletion (requires_confirmation
# returns False for it -- a worker may edit freely) but is always a
# mutation, so it is refused here too.
def is_coordinator_mutating(params) -> bool:
    """True when this ``session/request_permission`` call must be refused
    for a coordinator binding rather than answered or relayed to a host.

    Explicit about what counts as mutating: ``toolCall.kind`` of
    ``"edit"``/``"delete"``/``"move"`` always is; a missing/unrecognized
    kind (including ACP's own ``"other"``) always is (an unclassifiable
    request is refused, never guessed at -- see module docstring);
    ``"execute"`` is only when its command text matches
    :data:`_DELETION_COMMAND_PATTERN`. Every other recognized kind
    (``read``/``search``/``fetch``/``think``/``switch_mode``) -- and an
    ``"execute"`` call whose command text matches none of that pattern --
    is *not* refused, so a coordinator can still inspect the repository
    (list/read/search/fetch/run an ordinary non-destructive shell command)
    to decide what to delegate.

    HONEST BOUNDARY (the same one :func:`requires_confirmation` already
    discloses, reused as-is here): the ``"execute"`` command-text scan only
    ever recognizes a known *destructive* idiom; a non-destructive *write*
    under ``"execute"`` (an append, ``touch``, ``mkdir``, a package
    install, ...) is not caught by this reused pattern and is not refused.
    This is the same documented incompleteness as the deletion
    classifier's own, not a new gap this function introduces.
    """
    if is_cortex_own_tool(params):
        # Never a project mutation: this is the coordinator using the
        # runtime it is a coordinator *of*.
        return False
    tool_call = _tool_call(params)
    if tool_call.get("kind") == "edit":
        return True
    return requires_confirmation(params)


# T035 FINDING-12 closure: the observability-log reason recorded alongside
# every coordinator mutation refusal (see PermissionBridge._handle_one).
# ACP's RequestPermissionResponse has no text field at all (verified against
# the vendored tests/fixtures/acp/schema-1.4.0.json -- RequestPermissionOutcome
# is only ever {"outcome":"cancelled"} or {"outcome":"selected","optionId":
# ...}; its "_meta" property is explicitly documented as something "clients
# MUST NOT make assumptions about"), so this text cannot be attached to the
# wire reply itself -- see module docstring's own HONEST BOUNDARY. It is
# delivered to the coordinator model instead through the same continuation-
# text channel every other runtime-authored explanation already uses (see
# ``delegation.live_adapters.EngineSessionManagerAdapter``'s
# ``coordinator_refusal_notifier`` wiring and ``acp.orchestrating_runtime
# ._render_continuation_text``'s "progress" handling): the coordinator reads
# it, as ordinary continuation text, the next time its own engine session is
# prompted -- never synchronously inside the same already-in-flight turn,
# since ACP has no channel for that either.
COORDINATOR_MUTATION_REFUSAL_REASON = (
    "This mutating action was refused: the coordinator is a route, not a worker, and this "
    "runtime never lets it edit, delete, move, or run a destructive command against the "
    "project (see T035). Delegate this work to a worker instead of attempting it yourself."
)


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


def _auto_reject_option_id(params):
    """The offered ``optionId`` whose own ``kind`` is ``"reject_once"``
    (preferred) or, failing that, ``"reject_always"`` -- or ``None`` if the
    engine offered neither, in which case the caller falls back to
    ``CANCELLED_OUTCOME``. Never invents an id the engine did not itself
    offer (same invariant as :func:`_auto_allow_option_id` /
    :meth:`PermissionBridge._validate_host_response`). ``"reject_once"`` is
    preferred over ``"reject_always"`` for the same reason
    ``_auto_allow_option_id`` never picks ``"allow_always"``: this is a
    fresh per-request decision, not a persisted standing rule."""
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    by_kind = {
        option.get("kind"): option.get("optionId")
        for option in options if isinstance(option, dict)
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
        """``host_permission_requester`` -- ``(engine_session_id, params,
        timeout) -> dict`` or ``None`` -- is the caller-injected seam to the
        actual incoming ACP session/host that owns this engine session (see
        ``acp.host_requests.HostRequester``); when ``None`` every permission
        request is answered ``cancelled`` without ever reaching a host.

        ``coordinator_refusal_notifier`` -- ``(engine_session_id, reason) ->
        None`` or ``None`` (default) -- is called, synchronously and never
        allowed to raise into the caller (see :meth:`_notify_refusal`),
        every time this bridge refuses a coordinator binding's mutating
        request (see :func:`is_coordinator_mutating`). This is the only
        way ``COORDINATOR_MUTATION_REFUSAL_REASON`` ever reaches anything
        beyond this process's own observability log -- see that constant's
        own module-level docstring for exactly how (and when) it reaches
        the coordinator model."""
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

    def _notify_refusal(self, engine_session_id: str) -> None:
        if self._coordinator_refusal_notifier is None:
            return
        try:
            self._coordinator_refusal_notifier(engine_session_id, COORDINATOR_MUTATION_REFUSAL_REASON)
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
        if binding.role == "worker" and not requires_confirmation(params):
            option_id = _auto_allow_option_id(params)
            if option_id is not None:
                self._observe("permission_auto_approved", engine_session_id=engine_session_id)
                return {"outcome": {"outcome": "selected", "optionId": option_id}}
        if binding.role == "coordinator":
            # T035 FINDING-12 closure: a coordinator never reaches the host
            # at all -- its intent is decided right here, offline, every
            # time, regardless of whether a host_permission_requester is
            # even configured (see is_coordinator_mutating's own docstring
            # for exactly what counts as mutating).
            if is_coordinator_mutating(params):
                self._observe(
                    "coordinator_mutation_refused", engine_session_id=engine_session_id,
                    reason=COORDINATOR_MUTATION_REFUSAL_REASON,
                )
                self._notify_refusal(engine_session_id)
                reject_option_id = _auto_reject_option_id(params)
                if reject_option_id is not None:
                    return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                return dict(CANCELLED_OUTCOME)
            option_id = _auto_allow_option_id(params)
            if option_id is not None:
                self._observe("coordinator_read_auto_approved", engine_session_id=engine_session_id)
                return {"outcome": {"outcome": "selected", "optionId": option_id}}
            # No allow_once option offered for a request already classified
            # as non-mutating -- fail closed to cancelled rather than ever
            # relaying a coordinator's request to a host (a coordinator's
            # permission decisions are never a host's to make -- see
            # module docstring).
            self._observe("coordinator_read_no_allow_option", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        if self._host_permission_requester is None:
            self._observe("permission_no_host_requester", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        try:
            host_response = self._host_permission_requester(
                engine_session_id, _named_for_the_user(params), self._call_timeout,
            )
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
