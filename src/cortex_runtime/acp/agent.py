"""The incoming ACP v1 stdio agent: wires transport + protocol + storage + runtime.

Entry point: ``main()`` (see ``cortex_acp.py``, a thin bootstrap identical in
spirit to ``cortex.py``). No network server, no MITM proxy, no custom
editor, no CODEX_HOME dependency. stdout is exclusively protocol traffic;
every log/banner goes to stderr (or the optional
``CORTEX_OBSERVATION_DIR`` value-free JSONL sink, mirroring
``server._write_observation``).

``main(runtime=...)`` injects the runtime this agent orchestrates with. The
production entry point (``cortex_acp.py``, T016) passes a real
``acp.production_runtime.ProductionOrchestratingRuntime``; omitting
``runtime=`` (as every non-orchestration test that does not need a live
runtime does) falls back to :class:`runtime.UnavailableRuntime`, which
fails closed on every prompt. Tests that need
``tests.acp_fakes.FakeRuntime`` import it themselves and pass it explicitly
(see ``tests/test_acp_*.py``); this module never imports anything from
``tests/``.
"""
from __future__ import annotations

import collections
import inspect
import json
import os
import sys
import threading
from typing import Mapping
import time
import uuid
from pathlib import Path
from types import SimpleNamespace

from . import commands as acp_commands
from . import config_options as acp_config_options
from . import messages as acp_messages
from . import protocol
from . import sessions as acp_sessions
from . import steering as acp_steering
from .host_requests import HostRequestClosed, HostRequester, HostRequestError, HostRequestTimeout
from .protocol import RpcError
from .runtime import OrchestrationUnavailable, UnavailableRuntime
from .transport import StdioTransport, EOF
from . import replay_store as acp_replay_store
from ..bootstrap import _current_owner_epoch_obj, append_source_event, bootstrap_task
from ..catalog.model import CatalogEntry
from ..configuration.resolution import ConfigurationInputs, resolve_saved_route
from ..configuration.service import ConfigurationService
from ..configuration import store as configuration_store
from ..connections import registry as connection_registry
from ..contracts import StoreError
from ..diagnostic_boundary import emit_bounded_diagnostic, run_passive_diagnostic
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..project_storage import canonical_project, logical_project_root, migrate_legacy_project_storage, project_store_directory
from ..store import Store, private_directory

PLUGIN = Path(__file__).resolve().parents[3]
_PROMPT_JOIN_TIMEOUT = 5.0

# Bound on how many recent JSON-RPC request ids one connection remembers for
# the session/prompt duplicate-id rejection below (Agent._track_request_id).
# A long-lived connection cannot grow this without bound: once full, the
# oldest tracked id is evicted first (so an id far enough in the past could
# in principle be reused undetected -- accepted, since it is already well
# outside any plausible client retry window).
MAX_TRACKED_REQUEST_IDS = 10_000


def _request_id_identity(value):
    """An exact-JSON-type identity for a JSON-RPC request id.

    Python's ``==``/``hash`` treat ``1 == 1.0 == True`` (and would collapse
    them to the same dict key), but they are three distinct JSON-RPC ids
    (an integer, a number with a fractional part, and a boolean -- not
    actually a spec-legal id type at all, but still distinct from either
    number if a client sends one). Keying by ``(type name, exact JSON
    text)`` keeps every JSON type distinct without that collision, and
    without rejecting any id type here (transport-level id-shape validation,
    if ever added, is a separate, earlier concern -- see
    ``transport.parse_frame``).
    """
    return (type(value).__name__, json.dumps(value, sort_keys=True))


def _agent_version():
    try:
        return json.loads((PLUGIN / "package-identity.json").read_text())["version"]
    except (OSError, ValueError, KeyError):
        return "0.0.0"


def _write_observation(environ, operation, outcome):
    location = environ.get("CORTEX_OBSERVATION_DIR")
    if not location:
        return
    root = private_directory(location)
    path = root / f"{os.getpid()}.jsonl"
    row = dict(pid=os.getpid(), time_ns=time.time_ns(), operation=operation, outcome=outcome)
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        if os.fstat(fd).st_size < 256_000:
            os.write(fd, (json.dumps(row) + "\n").encode())
    finally:
        os.close(fd)


def observe(environ, operation, outcome):
    """Optional bounded passive observation; failures never alter behavior."""
    return run_passive_diagnostic(
        lambda: _write_observation(environ, operation, outcome),
        operation="observation",
        emit=lambda finding: emit_bounded_diagnostic(sys.stderr, finding),
    )



def _store_error_message(error: StoreError) -> str:
    """What the client is told when the store refuses.

    ``StoreError`` already carries why -- the refused field, what it
    received, what it expected -- and dropping all of it left a user
    staring at "Storage failed while handling this request." for causes
    they could have fixed in a second, such as a ``.cortex`` directory
    that is not owner-only. Everything here is the user's own local
    storage state (paths and modes, already truncated by
    ``contracts._received``), not engine or account data.
    """
    parts = [str(error)]
    if error.field:
        parts.append(f"field {error.field}")
    if error.received is not None:
        parts.append(f"got {error.received}")
    if error.expected:
        parts.append(f"expected {error.expected}")
    if error.correction:
        parts.append(str(error.correction))
    return "Storage failed while handling this request: " + "; ".join(parts) + "."


class _RealHostElicitor:
    """T020: the real ``acp.orchestrating_runtime.HostElicitor`` -- relays a
    coordinator ``ask_user`` question through the connected host's own
    ``elicitation/create`` request over that connection's own
    ``HostRequester`` (see ``Agent._make_elicitor``, which only ever
    constructs this when the host advertised
    ``clientCapabilities.elicitation.form`` at ``initialize``).

    Request shape (pinned schema-1.4.0 ``CreateElicitationRequest``, form
    mode, session scope): ``{"message", "mode": "form", "requestedSchema":
    {"type": "object", "properties": {"answer": {"type": "string", ...}},
    "required": ["answer"]}, "sessionId"}`` -- a single required text
    field, or (when ``options`` were given) a single-select enum on that
    same field (``StringPropertySchema``'s ``enum``). Response shape
    (``CreateElicitationResponse``): ``{"action": "accept", "content":
    {"answer": "..."}}`` on accept; ``{"action": "decline"}`` or
    ``{"action": "cancel"}`` otherwise -- both, plus a timeout/closed
    transport, map to ``None`` (the caller's plain-text fallback), never
    raised.
    """

    def __init__(self, host_requester: HostRequester, *, timeout: float = 30.0) -> None:
        self._host_requester = host_requester
        self._timeout = timeout

    def elicit(self, session_id: str, question: str, options: list[str] | None) -> str | None:
        answer_schema: dict = {"type": "string"}
        if options:
            answer_schema["enum"] = list(options)
        params = dict(
            message=question,
            mode="form",
            requestedSchema=dict(
                type="object",
                properties=dict(answer=answer_schema),
                required=["answer"],
            ),
            sessionId=session_id,
        )
        try:
            result = self._host_requester.request("elicitation/create", params, timeout=self._timeout)
        except (HostRequestTimeout, HostRequestClosed, HostRequestError):
            return None
        except Exception:
            # A malformed/unexpected host reply is a decline-equivalent,
            # never an exception that aborts the coordinator's turn.
            return None
        if not isinstance(result, dict) or result.get("action") != "accept":
            return None
        content = result.get("content")
        if not isinstance(content, dict):
            return None
        answer = content.get("answer")
        return answer if isinstance(answer, str) and answer else None


class Agent:
    """One ACP connection's worth of state: sessions, stores, dispatch.

    JSON-RPC request id reuse: within one connection, a ``session/prompt``
    whose id was already used by any earlier request on this connection
    (still in flight or already completed) is rejected with ``-32600``
    ("duplicate_request_id") before any durable capture is attempted --
    see ``_track_request_id``/``_dispatch``. Ids are tracked per connection
    only (bounded to the most recent ``MAX_TRACKED_REQUEST_IDS``); a fresh
    connection (a new agent process/reconnect) starts with an empty set, so
    an id that was used on a prior connection is not remembered and a
    request reusing it is treated as new. This is intentional: ACP clients
    do not auto-retry a ``session/prompt`` across a reconnect, so an id
    collision after a reconnect is not the same in-flight-retry hazard this
    guard defends against, and it durably becomes a new source event
    (``bootstrap.append_source_event`` allocates a fresh ``event_id`` per
    delivery regardless of the JSON-RPC id); any reconciliation of
    duplicate *content* across a reconnect is a later recovery concern, not
    this guard's job.
    """

    #: Default (production) ``elicitation/create`` request timeout in
    #: seconds -- see ``_make_elicitor``/``elicitation_timeout_seconds``.
    DEFAULT_ELICITATION_TIMEOUT_SECONDS = 30.0
    #: Bounds a caller-supplied ``elicitation_timeout_seconds`` (T020
    #: fresh-review fix: previously an unbounded os.environ lookup in
    #: production code -- see the constructor docstring).
    _MIN_ELICITATION_TIMEOUT_SECONDS = 0.05
    _MAX_ELICITATION_TIMEOUT_SECONDS = 300.0

    def __init__(self, runtime, *, environ=None, elicitation_timeout_seconds=None):
        """``elicitation_timeout_seconds`` (T020 fresh-review fix, MEDIUM):
        an explicit constructor parameter for the real
        ``elicitation/create`` request timeout (see ``_make_elicitor``) --
        never an ``os.environ`` lookup in production code. ``None`` (every
        production caller) uses ``DEFAULT_ELICITATION_TIMEOUT_SECONDS``; a
        test that needs a short, bounded wait (see
        ``tests/test_acp_elicitation.py``) passes a small value directly.
        Validated positive and bounded -- an out-of-range value is rejected
        immediately (construction-time, never a silent clamp)."""
        if elicitation_timeout_seconds is None:
            elicitation_timeout_seconds = self.DEFAULT_ELICITATION_TIMEOUT_SECONDS
        elif not (self._MIN_ELICITATION_TIMEOUT_SECONDS <= elicitation_timeout_seconds <= self._MAX_ELICITATION_TIMEOUT_SECONDS):
            raise ValueError(
                f"elicitation_timeout_seconds must be between {self._MIN_ELICITATION_TIMEOUT_SECONDS} and "
                f"{self._MAX_ELICITATION_TIMEOUT_SECONDS}; got {elicitation_timeout_seconds!r}."
            )
        self._elicitation_timeout_seconds = float(elicitation_timeout_seconds)
        self.runtime = runtime
        # T016: a multi-project-capable runtime (see
        # acp.production_runtime.ProductionOrchestratingRuntime) accepts an
        # optional 'project_root' kwarg to resolve its own per-project
        # store/route; detected once here (never per-call) so every other
        # IncomingRuntime (UnavailableRuntime, tests.acp_fakes.FakeRuntime)
        # keeps receiving the exact same 5-positional-argument call it
        # always has.
        try:
            self._runtime_accepts_project_root = 'project_root' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_project_root = False
        # T018: detected the exact same way as project_root above -- a
        # runtime (real or FakeRuntime) that has not been updated for T018
        # simply never receives source_origin/source_event_id and falls
        # back to its own pre-T018 turn-count identity (see
        # acp.orchestrating_runtime.start_prompt's docstring).
        try:
            self._runtime_accepts_source_identity = 'source_event_id' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_source_identity = False
        # T020: detected the same way -- a runtime not updated for T020
        # (every FakeRuntime) simply never receives host_elicitor and
        # stays on the plain-text ask_user fallback unconditionally.
        try:
            self._runtime_accepts_host_elicitor = 'host_elicitor' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_host_elicitor = False
        # T035: detected the same way -- a runtime not updated for T035
        # (every pre-T035 FakeRuntime/test) simply never receives
        # emit_tool_call and stays on the plain-text-only narration it
        # already had.
        try:
            self._runtime_accepts_emit_tool_call = 'emit_tool_call' in inspect.signature(runtime.start_prompt).parameters
            self._runtime_accepts_emit_display_update = (
                'emit_display_update' in inspect.signature(runtime.start_prompt).parameters
            )
            self._runtime_accepts_host_mcp_servers = (
                'host_mcp_servers' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_emit_tool_call = False
            self._runtime_accepts_emit_display_update = False
            self._runtime_accepts_host_mcp_servers = False
        # EXT-01 #5: detected the same way -- a runtime not updated for this
        # fix (every pre-existing FakeRuntime/test) simply never receives
        # host_permission_requester/host_fs_requester and keeps its own
        # pre-existing (unwired) behavior unchanged.
        try:
            self._runtime_accepts_host_permission_requester = (
                'host_permission_requester' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_host_permission_requester = False
        try:
            self._runtime_accepts_host_fs_requester = (
                'host_fs_requester' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_host_fs_requester = False
        self.environ = environ if environ is not None else os.environ
        self.version = _agent_version()
        self.stores = {}
        self.sessions = {}
        self.transport = None
        self.host_requester = None
        self.initialized = False
        self.client_info = {}
        self.client_capabilities = {}
        self._workers = []
        self._workers_lock = threading.Lock()
        self._seen_request_ids = collections.OrderedDict()

    def bind_transport(self, transport):
        self.transport = transport
        # T014: this agent can also *initiate* a request to the host (for
        # example relaying a downstream engine's session/request_permission
        # -- see engine.permissions.PermissionBridge), tracked in a
        # namespace ("hr_"-prefixed ids) disjoint from any id the host
        # itself chooses for its own requests to this agent.
        self.host_requester = HostRequester(transport)

    def _track_request_id(self, request_id):
        """Record ``request_id`` as used on this connection; return whether
        it was already used (in flight or completed) by an earlier request.

        Bounded to the most recent ``MAX_TRACKED_REQUEST_IDS`` ids: marks a
        request id as "in flight" immediately on receipt (not only once
        processing completes), so a resend of the same id while the
        original is still being handled is also caught.

        Keyed by an exact JSON identity (``_request_id_identity``), not the
        raw Python value: JSON-RPC ids of different JSON types must never
        collide even when Python's own equality/hashing would consider them
        equal -- ``1 == 1.0 == True`` in Python, but the ids ``1``, ``1.0``
        and ``true`` are three distinct JSON-RPC request ids.
        """
        key = _request_id_identity(request_id)
        already_used = key in self._seen_request_ids
        if not already_used:
            self._seen_request_ids[key] = True
            if len(self._seen_request_ids) > MAX_TRACKED_REQUEST_IDS:
                self._seen_request_ids.popitem(last=False)
        return already_used

    # -- storage -------------------------------------------------------

    def _store_for(self, project):
        store = self.stores.get(project)
        if store is None:
            directory = project_store_directory(project)
            store = Store(directory, initialize=True, project_root=project)
            self.stores[project] = store
        return store

    # -- inbound dispatch (called synchronously on the transport reader
    #    thread; only session/prompt hands work off to a worker thread so
    #    the reader keeps servicing session/cancel while a turn runs) ----

    def handle_message(self, message, error):
        if message is EOF:
            self._handle_eof()
            return
        if error is not None:
            self._reply_error(error.request_id, error.code, error.message)
            return
        if "id" in message and "method" not in message and ("result" in message or "error" in message):
            # A response, not a request/notification: only ever a reply to
            # one of this agent's own HostRequester calls (T014). Never
            # dispatched as a method call and never replied to again.
            if self.host_requester is None or not self.host_requester.handle_response(message):
                sys.stderr.write("acp: unmatched response-shaped message ignored\n")
            return
        has_id = "id" in message
        request_id = message.get("id") if has_id else None
        method = message.get("method")
        if not isinstance(method, str):
            if has_id:
                self._reply_error(request_id, -32600, "Missing or invalid \"method\".")
            return
        if method != "initialize" and not self.initialized:
            if has_id:
                self._reply_error(request_id, -32600, "Call initialize before any other method.")
            else:
                sys.stderr.write(f"acp: notification {method!r} before initialize, ignored\n")
            return
        try:
            self._dispatch(method, message.get("params"), request_id, has_id)
        except RpcError as exc:
            if has_id:
                self._reply_error(request_id, exc.code, exc.message, exc.data)
            observe(self.environ, method, "error")
        except StoreError as exc:
            if has_id:
                self._reply_error(request_id, -32603, _store_error_message(exc))
            observe(self.environ, method, str(exc))
        except Exception:
            if has_id:
                self._reply_error(request_id, -32603, "Internal error while handling this request.")
            observe(self.environ, method, "internal_error")

    def _dispatch(self, method, params, request_id, has_id):
        if has_id:
            already_used = self._track_request_id(request_id)
            if method == "session/prompt" and already_used:
                # Reject before any durable capture is attempted (see the
                # class docstring): bootstrap_task/append_source_event must
                # never even be reached for a resent id.
                raise RpcError(-32600, "duplicate_request_id")
        if method == "initialize":
            result = self._initialize(params)
            self.initialized = True
            self.transport.send_response(protocol.result_response(request_id, result))
            observe(self.environ, method, "success")
        elif method == "session/new":
            result = self._session_new(params)
            self.transport.send_response(protocol.result_response(request_id, result))
            self._post_session_new_notifications(result["sessionId"])
            observe(self.environ, method, "success")
        elif method == "session/prompt":
            self._session_prompt(request_id, params)
        elif method == "session/cancel":
            self._session_cancel(params)
            observe(self.environ, method, "success")
        elif method == "session/set_config_option":
            self._session_set_config_option(request_id, params)
            observe(self.environ, method, "success")
        elif method == "session/load":
            self._session_load(request_id, params)
            observe(self.environ, method, "success")
        elif method in protocol.UNSUPPORTED_METHODS:
            if has_id:
                self._reply_error(request_id, -32601, f"Method '{method}' is not supported.")
            observe(self.environ, method, "unsupported")
        else:
            if has_id:
                self._reply_error(request_id, -32601, f"Unknown method '{method}'.")
            else:
                sys.stderr.write(f"acp: unknown notification {method!r} ignored\n")
            observe(self.environ, method, "unknown")

    def _reply_error(self, request_id, code, message, data=None):
        if self.transport is None:
            return
        self.transport.send_response(protocol.error_response(request_id, RpcError(code, message, data)))

    # -- method handlers -------------------------------------------------

    def _initialize(self, params):
        protocol.validate_initialize_params(params)
        self.client_info = params.get("clientInfo") or {}
        self.client_capabilities = params.get("clientCapabilities") or {}
        return protocol.initialize_result(self.version)

    def _session_new(self, params):
        cwd, raw_mcp_servers = protocol.validate_session_new_params(params)
        try:
            project = logical_project_root(canonical_project(cwd))
        except StoreError:
            raise RpcError(-32602, "cwd must be an absolute, canonical, existing directory.") from None
        migrate_legacy_project_storage(project)
        store = self._store_for(project)
        # T018 (fresh-review follow-up): opportunistic, best-effort sweep of
        # any attachment staging temp files orphaned by a prior crashed
        # process (see acp.replay_store.sweep_orphaned_staging's docstring
        # for why an age-only sweep here can never touch a referenced
        # body). Never fails session/new -- a sweep failure is silently
        # skipped, not surfaced to the client.
        try:
            acp_replay_store.sweep_orphaned_staging(project)
        except Exception:
            pass
        stripped_servers = protocol.strip_mcp_servers(raw_mcp_servers)
        session_id = new_id("acps_")
        acp_sessions.create_session(store, session_id, project, self.client_info, self.client_capabilities, stripped_servers)
        session = dict(
            project=project, task_id=None, channel=None, active=None, lock=threading.Lock(),
            owner_epoch=None, pending_selection=None, default_selection=None,
            pending_language=None, client_capabilities=dict(self.client_capabilities),
            # The client's own MCP servers, kept whole and in memory only so
            # the engines this session drives are offered the same tools the
            # user configured in their editor. Storage keeps the stripped
            # shape above and never these values: an entry's env/headers are
            # the client's secrets.
            mcp_servers=[dict(e) for e in raw_mcp_servers if isinstance(e, dict)],
        )
        self.sessions[session_id] = session
        # A prior explicit choice persisted as the *normal* T008 user-default
        # route/parameters (see _apply_pre_task_selection) is only ever this
        # session's *default* (shown with state "default", never "pending")
        # until this session itself makes its own explicit choice -- see
        # _current_selection_context. It lives only in this session's memory
        # (never re-marked as "pending" anywhere durable).
        session["default_selection"] = self._default_selection_from_user_defaults()
        options = self._config_options_for_session(session, store)
        result = dict(sessionId=session_id)
        if options:
            result["configOptions"] = options
        return result

    def _post_session_new_notifications(self, session_id):
        """Sent by the caller after the session/new response itself (see
        ``_dispatch``): ``available_commands_update`` is not part of the
        ``NewSessionResponse`` payload, only ever a follow-up notification."""
        if self.transport is not None:
            self.transport.send_notification(
                protocol.available_commands_update(session_id, acp_commands.AVAILABLE_COMMANDS))

    # -- configuration options / commands (shared by session/new,
    #    session/set_config_option, and the /cortex-* command equivalents) --

    def _selectable_entries_and_connections(self):
        registry_state = connection_registry.read_registry(self.environ)
        entries = acp_config_options.selectable_entries_from_snapshots(registry_state.connections, self.environ)
        return entries, registry_state.connections

    def _default_selection_from_user_defaults(self):
        """The last explicit ``/cortex-model``/``set_config_option`` choice
        persisted as the *normal* T008 user-default route/parameters (see
        ``_apply_pre_task_selection``) -- plain ``route_id``/``parameters``
        fields, never a Cortex-specific "pending" marker -- or ``None``.

        ``connection_id``/``model`` (T035 FINDING-05 fix): the saved
        choice's stable identity, persisted alongside ``route_id`` by every
        write site since this fix -- ``None``/``None`` for a defaults.json
        written before this fix existed, which
        ``_propose_pending_selection_for_new_task`` recovers one-time via
        ``catalog.store.find_route_identity`` and then persists forward."""
        defaults = configuration_store.read_user_defaults(self.environ)["defaults"]
        route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
        if not isinstance(route_id, str) or not route_id:
            return None
        parameters = defaults.get("parameters") if isinstance(defaults, dict) else None
        connection_id = defaults.get("route_connection_id") if isinstance(defaults, dict) else None
        model = defaults.get("route_model") if isinstance(defaults, dict) else None
        return dict(
            route_id=route_id, parameters=parameters if isinstance(parameters, dict) else {},
            connection_id=connection_id if isinstance(connection_id, str) and connection_id else None,
            model=model if isinstance(model, str) and model else None,
        )

    def _current_selection_context(self, store, session):
        task_id = session["task_id"]
        if task_id is not None:
            history = ConfigurationService(store).history(task_id)
            best = acp_config_options.most_recent_live_selection(history)
            if best is not None:
                return dict(route_id=best["route_id"], state=best["state"],
                            revision=best["revision"], parameters=best["parameters"])
            return None
        pending = session.get("pending_selection")
        if pending is not None:
            # This session itself made an explicit choice pre-task: shown as
            # pending, applies on the next (first) prompt.
            return dict(route_id=pending["route_id"], state="pending_next_use",
                        revision=None, parameters=pending.get("parameters") or {})
        default = session.get("default_selection")
        if default is not None:
            # Inherited from a prior session's persisted user default: shown
            # as this session's plain default, never "pending" -- nothing in
            # *this* session has changed it yet.
            return dict(route_id=default["route_id"], state="default",
                        revision=None, parameters=default.get("parameters") or {})
        return None

    def _default_language_from_user_defaults(self):
        """The last explicit ``/cortex-language``/``set_config_option``
        choice persisted pre-task (see ``_apply_language_selection``), or
        ``None`` (meaning "auto") -- mirrors ``_default_selection_from_user_defaults``."""
        defaults = configuration_store.read_user_defaults(self.environ)["defaults"]
        value = defaults.get("language") if isinstance(defaults, dict) else None
        return value if value in ("en", "ru") else None

    def _make_elicitor(self, session_id):
        """T020: a real ``_RealHostElicitor`` iff the host advertised
        ``clientCapabilities.elicitation.form`` at ``initialize`` --
        schema-1.4.0's own convention: ``form`` omitted or ``null`` means
        no support; ``{}`` (present but empty) already means support (see
        ``ElicitationCapabilities.form``'s pinned description). ``None``
        otherwise -- the caller's plain-text ask_user fallback.

        The request timeout is ``self._elicitation_timeout_seconds`` (an
        explicit, validated constructor parameter -- T020 fresh-review fix,
        MEDIUM: previously an unbounded ``os.environ`` lookup reachable in
        production code; see ``__init__``)."""
        capabilities = self.client_capabilities if isinstance(self.client_capabilities, dict) else {}
        elicitation = capabilities.get("elicitation")
        if not isinstance(elicitation, dict) or elicitation.get("form", None) is None:
            return None
        if self.host_requester is None:
            return None
        return _RealHostElicitor(self.host_requester, timeout=self._elicitation_timeout_seconds)

    def _make_permission_requester(self, session_id):
        """EXT-01 #5: a real, connection-bound
        ``(engine_session_id, params, timeout) -> dict`` relay for a
        downstream engine's ``session/request_permission`` (see
        ``engine.permissions.PermissionBridge``), or ``None`` when this
        connection has no transport yet. Unlike ``_make_elicitor``, ACP
        defines no client capability flag for permission requests at all
        (``session/request_permission`` is a core, not-optional host
        feature -- verified against this repository's own vendored
        ``ClientCapabilities`` schema, which has no such property): the only
        gate is whether a live ``HostRequester`` exists, exactly matching
        ``engine.permissions.PermissionBridge``'s own pre-existing
        "``None`` means never reaches a host" contract.

        ``engine_session_id`` is accepted and discarded here -- this
        connection has exactly one ``session_id`` per ``start_prompt`` call
        (the caller's own), and the *engine*-session-to-*task* identity
        mapping this relay is answering for is already resolved one layer
        below, by ``delegation.live_adapters.EngineSessionManagerAdapter``,
        before this callable is ever invoked (see
        ``EngineSessionManagerAdapter._host_permission_requester``); this
        closure only ever needs to know which ACP ``sessionId`` (this
        connection's own) to put on the outbound wire request."""
        if self.host_requester is None:
            return None
        host_requester = self.host_requester

        def request_permission(engine_session_id, params, timeout=30.0):
            del engine_session_id  # see docstring: resolved one layer below
            body = dict(params or {})
            body["sessionId"] = session_id
            try:
                return host_requester.request("session/request_permission", body, timeout=timeout or 30.0)
            except (HostRequestTimeout, HostRequestClosed, HostRequestError):
                return None

        return request_permission

    def _make_fs_requester(self, session_id):
        """EXT-01 #5: a real, connection-bound ``(method, params) -> dict``
        relay for the host's own negotiated ``fs/*`` capability (see
        ``engine.fs_bridge.HostFsForwarder``), built -- like
        ``_make_elicitor`` -- iff the host actually advertised
        ``clientCapabilities.fs.readTextFile`` at ``initialize`` (schema-
        1.4.0's own ``FileSystemCapabilities``; ``fs`` omitted, or with
        ``readTextFile`` false/missing, both mean "cannot read a host
        buffer" -- never guessed at). ``None`` otherwise -- the caller
        (``acp.orchestrating_runtime._make_host_fs_checker``) then records
        the integration's own ``host_buffer_state: 'unknown'`` rather than
        ever treating "capability not negotiated" as "checked, no
        conflict".

        Deliberately never catches an exception from the underlying
        ``HostRequester.request`` call: the caller distinguishes a clean "no
        buffer for this path" (``None``/empty content) from a failed
        round-trip (timeout/closed transport/error) by whether *this*
        callable raises -- see ``delegation.integration.integrate``'s own
        ``host_fs_checker`` contract docstring for exactly why swallowing an
        exception here would be the one thing this fix must not do."""
        capabilities = self.client_capabilities if isinstance(self.client_capabilities, dict) else {}
        fs = capabilities.get("fs")
        if not isinstance(fs, dict) or not fs.get("readTextFile"):
            return None
        if self.host_requester is None:
            return None
        host_requester = self.host_requester

        def request_fs(method, params):
            # ACP's own fs/* requests are session-scoped: schema 1.4.0's
            # ReadTextFileRequest requires ``sessionId``, and a real client
            # rejects a request without one outright -- Zed answered
            # -32602 "missing field `sessionId`", which
            # ``delegation.integration.integrate`` could only read as a
            # failed round-trip, so every integration against a real client
            # conflicted with ``host_buffer_check_failed`` and no worker
            # change could ever land. This relay is bound to exactly one
            # session, so it is the one place that knows it; a caller that
            # supplies its own is never overridden.
            payload = dict(params or {})
            payload.setdefault("sessionId", session_id)
            return host_requester.request(method, payload)

        return request_fs

    def _make_emit_display_update(self, session_id, cancel_event):
        """T035: relay one engine ``session/update`` that exists only to be
        shown -- the model's reasoning (``agent_thought_chunk``) and its
        task list (``plan``) -- onto this agent's own session, unchanged
        except for the session id.

        These were previously dropped as unrecognized, so a person watching
        saw neither. They are forwarded rather than reinterpreted: the
        schema already defines how a client renders them, and inventing a
        second representation would only lose detail.

        Only the kinds ``engine.session_manager._DISPLAY_ONLY_UPDATES``
        allows ever reach here; the engine's own session-state updates are
        not among them, because this agent's session is not that session.
        """
        def emit_display_update(update):
            if cancel_event.is_set() or self.transport is None:
                return False
            if not isinstance(update, Mapping) or not update.get("sessionUpdate"):
                return False
            payload = {key: value for key, value in update.items() if key != "sessionId"}
            return self.transport.send_notification(
                protocol.notification("session/update", dict(sessionId=session_id, update=payload)),
            )
        return emit_display_update

    def _make_emit_tool_call(self, session_id, cancel_event):
        """T035: the runtime-facing ``emit_tool_call`` callable -- accepts
        one already-shaped update dict (``tool_call_id``, ``new`` [bool],
        ``title``/``kind``/``status``/``text`` -- fields the runtime omits
        are left unchanged on an existing card, per
        ``acp.protocol.tool_call_update``'s own "only changed fields"
        contract) and sends the matching ``session/update`` notification:
        ``tool_call`` the first time a given ``tool_call_id`` is seen
        (``new=True``, set by ``acp.orchestrating_runtime`` itself, which
        already knows -- from the durable event log -- exactly once per
        assignment when that is), ``tool_call_update`` every time after.
        Never raises (same convention as ``emit_update`` below); ``None``
        transport or an already-cancelled turn is a silent no-op, exactly
        like a dropped text chunk."""
        def emit_tool_call(update):
            if cancel_event.is_set() or self.transport is None:
                return False
            tool_call_id = update["tool_call_id"]
            if update.get("new"):
                notif = protocol.tool_call(
                    session_id, tool_call_id, update.get("title") or "Worker assignment",
                    update.get("kind") or "other", update.get("status") or "pending",
                    text=update.get("text"), locations=update.get("locations"),
                    content=update.get("content"),
                )
            else:
                notif = protocol.tool_call_update(
                    session_id, tool_call_id, status=update.get("status"), title=update.get("title"),
                    text=update.get("text"), locations=update.get("locations"),
                    content=update.get("content"),
                )
            return self.transport.send_notification(notif)
        return emit_tool_call

    def _current_language_for_session(self, store, session):
        """T020: the ``language`` SessionConfigOption's current value for
        this session -- the task's own durable per-task override once a
        task exists (``acp.steering.get_language_override``), else this
        session's own not-yet-applied pending choice, else the inherited
        persisted user default; ``None`` at every level means "auto"."""
        task_id = session["task_id"]
        if task_id is not None:
            return acp_steering.get_language_override(store, task_id)
        pending = session.get("pending_language")
        if pending is not None:
            return pending
        return self._default_language_from_user_defaults()

    def _config_options_for_session(self, session, store):
        entries, connections = self._selectable_entries_and_connections()
        current = self._current_selection_context(store, session)
        language_current = self._current_language_for_session(store, session)
        return acp_config_options.build_config_options(
            entries, connections, current, include_language=True, language_current=language_current,
            environ=self.environ)

    def _connection_setup_summary(self, connections):
        if not connections:
            return "no connection registered yet; see /cortex-setup"
        configured = sum(1 for record in connections.values() if record.state.value == "configured")
        return f"{configured}/{len(connections)} connection(s) configured"

    def _session_prompt(self, request_id, params):
        session_id, text, raw_blocks = protocol.validate_session_prompt_params(params)
        recognized = acp_commands.recognize(raw_blocks)
        meta = params.get("_meta") if isinstance(params, dict) else None
        session = self.sessions.get(session_id)
        if session is None:
            raise RpcError(-32602, "Unknown sessionId.")
        cancel_event = threading.Event()
        with session["lock"]:
            if session["active"] is not None:
                raise RpcError(-32602, "A prompt is already in flight for this session.")
            session["active"] = cancel_event
        worker = threading.Thread(
            target=self._run_prompt, name="acp-prompt",
            args=(request_id, session_id, session, text, cancel_event, recognized, raw_blocks, meta), daemon=True)
        with self._workers_lock:
            self._workers.append(worker)
        worker.start()

    def _run_prompt(self, request_id, session_id, session, text, cancel_event, recognized=None, raw_blocks=None, meta=None):
        try:
            if recognized is not None:
                self._run_command_body(request_id, session_id, session, recognized, cancel_event)
                return
            self._run_prompt_body(request_id, session_id, session, text, cancel_event, raw_blocks, meta)
        except Exception:
            # Anything unexpected here -- including a failure in
            # _store_for/SimpleNamespace construction *before* the durable-
            # capture try/except below is even reached -- must still reply.
            # Otherwise the client hangs forever waiting for this request's
            # response (never dropped, per the transport's own contract).
            # No content is echoed back.
            self._reply_error(request_id, -32603, "Internal error while processing this prompt.")
            observe(self.environ, "session/prompt", "internal_error")
        finally:
            with session["lock"]:
                session["active"] = None

    def _run_prompt_body(self, request_id, session_id, session, text, cancel_event, raw_blocks=None, meta=None):
        """The actual prompt turn; wrapped by ``_run_prompt`` for the
        catch-all -32603 safety net and the ``session["active"]`` cleanup.
        """
        store = self._store_for(session["project"])
        origin = f"acp:{session_id}"
        event_id = str(uuid.uuid4())
        source_event = SimpleNamespace(origin=origin, event_id=event_id)
        is_first_prompt = session["task_id"] is None
        # T018: resolve every non-text content block (EmbeddedResource
        # 'resource' / 'resource_link') and stage its body to an
        # owner-private temp file *before* the durable capture transaction
        # opens (no DB lock held during this file I/O; see
        # acp.replay_store.resolve_prompt_blocks/stage_blocks -- neither
        # ever raises, an unexpected per-block failure is recorded as
        # availability='inaccessible', reason='capture_error', never
        # dropped). bootstrap_task/append_source_event then records every
        # staged block's metadata (and renames its body into place)
        # *inside the same transaction* as the source text/source_events
        # row -- atomic: a crash before commit persists neither; a crash
        # after commit has both together (a replay unlinks the staged
        # bodies instead of recording anything new).
        staged_blocks = []
        if raw_blocks:
            resolved_blocks = acp_replay_store.resolve_prompt_blocks(raw_blocks, session["project"])
            if resolved_blocks:
                staged_blocks = acp_replay_store.stage_blocks(session["project"], resolved_blocks)
        try:
            if is_first_prompt:
                # Read before the task exists, which is the whole point:
                # with a task_id in hand this same lookup answers with the
                # brand-new task's own (empty) override instead of the
                # choice the session is carrying.
                session_language = self._current_language_for_session(store, session)
                result = bootstrap_task(store, source_event, text, owner_ref=origin, blocks=staged_blocks, meta=meta)
                session["task_id"] = result.task_id
                session["owner_epoch"] = result.owner_epoch
                if result.channel_id is not None:
                    # The channel secret is kept only in this process's
                    # memory (this dict), never logged, never sent to
                    # the ACP client, never persisted.
                    session["channel"] = (result.channel_id, result.secret)
                acp_sessions.bind_task(store, session_id, result.task_id)
                # The language the user already chose, carried onto the
                # task the moment it exists. Before a task there is none,
                # so the choice lives in the session (and in the user's
                # defaults); nothing moved it across, so a brand-new
                # thread had no language override at all and the
                # coordinator was never told to answer in it. Live: the
                # composer read "Russian" and every reply came back in
                # English.
                if session_language is not None:
                    acp_steering.set_language_override(store, result.task_id, session_language)
            else:
                result = append_source_event(store, source_event, text, session["task_id"], blocks=staged_blocks, meta=meta)
                if result.owner_epoch is not None:
                    session["owner_epoch"] = result.owner_epoch
        except StoreError:
            acp_replay_store.cleanup_staged(staged_blocks)
            self._reply_error(request_id, -32603, "Durable capture failed; nothing was forwarded to orchestration.")
            observe(self.environ, "session/prompt", "capture_failed")
            return
        task_id = session["task_id"]
        attachment_descriptors = [
            dict(source_ref=f"{result.report_id}:{b.capture.block_index}", name=b.capture.name, uri=b.capture.uri,
                 kind=b.capture.kind, availability=b.capture.availability, reason=b.capture.reason)
            for b in staged_blocks
        ] if not result.replayed else []
        if is_first_prompt:
            self._propose_pending_selection_for_new_task(store, session, task_id)
        acp_replay_store.append_host_transcript(store, session_id, "user_message", text, attachment_descriptors)

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            delivered = self.transport.send_notification(protocol.agent_message_chunk(session_id, chunk_text))
            if delivered:
                try:
                    acp_replay_store.append_host_transcript(store, session_id, "agent_message", chunk_text)
                except StoreError:
                    pass
            if not delivered:
                # The client cannot keep up: never grow memory without
                # bound and never drop a *response* -- only this
                # notification is dropped, and the prompt is cancelled.
                cancel_event.set()
            return delivered

        start_prompt_kwargs = {}
        if self._runtime_accepts_project_root:
            start_prompt_kwargs["project_root"] = session["project"]
        if self._runtime_accepts_source_identity:
            start_prompt_kwargs["source_origin"] = origin
            start_prompt_kwargs["source_event_id"] = event_id
        if self._runtime_accepts_host_elicitor:
            # T020: only a real, per-connection HostRequester-backed
            # elicitor when the host actually advertised
            # clientCapabilities.elicitation.form at initialize -- see
            # _make_elicitor. None (the ordinary case for every host that
            # doesn't advertise it, and every pre-T020 runtime) falls back
            # to the runtime's own constructor default (also None), which
            # is the unconditional plain-text ask_user fallback.
            start_prompt_kwargs["host_elicitor"] = self._make_elicitor(session_id)
        if self._runtime_accepts_host_permission_requester:
            # EXT-01 #5: same "never guessed, None is the honest default"
            # convention as host_elicitor above.
            start_prompt_kwargs["host_permission_requester"] = self._make_permission_requester(session_id)
        if self._runtime_accepts_host_fs_requester:
            start_prompt_kwargs["host_fs_requester"] = self._make_fs_requester(session_id)
        if self._runtime_accepts_emit_tool_call:
            start_prompt_kwargs["emit_tool_call"] = self._make_emit_tool_call(session_id, cancel_event)
        if self._runtime_accepts_emit_display_update:
            start_prompt_kwargs["emit_display_update"] = self._make_emit_display_update(session_id, cancel_event)
        if self._runtime_accepts_host_mcp_servers:
            # The servers the client sent with this session, forwarded to
            # the coordinator and to every worker (see
            # ``engine.mcp_bridge.host_mcp_servers``). They were parsed and
            # kept on the session from the first day and then never passed
            # anywhere: the bridge that forwards them had tests, and
            # nothing ever called it. Live evidence: an engine Cortex
            # launched carried only its own ``cortex`` server, while the
            # same editor's own agent had the user's three.
            start_prompt_kwargs["host_mcp_servers"] = session.get("mcp_servers") or None
        try:
            reason = self.runtime.start_prompt(task_id, session_id, text, emit_update, cancel_event, **start_prompt_kwargs)
        except OrchestrationUnavailable as exc:
            # str(exc) is "orchestration_unavailable" for the default
            # UnavailableRuntime (unchanged wire behavior); a
            # ProductionOrchestratingRuntime instead names the concrete,
            # non-silent reason (e.g. "no_configuration_revision",
            # "instruction_delivery_unqualified") so the client sees what
            # is actually missing rather than a generic refusal.
            reason_code = str(exc) or "orchestration_unavailable"
            self.transport.send_response(protocol.error_response(
                request_id, RpcError(-32001, "orchestration_unavailable", data={"retryable": False, "reason": reason_code})))
            observe(self.environ, "session/prompt", reason_code)
            return
        except Exception:
            if cancel_event.is_set():
                reason = StopReason.CANCELLED
            else:
                self._reply_error(request_id, -32603, "The runtime failed while processing this prompt.")
                observe(self.environ, "session/prompt", "runtime_error")
                return
        if cancel_event.is_set() and reason != StopReason.CANCELLED:
            # session/cancel MUST be reflected even if the runtime
            # otherwise reports a different outcome.
            reason = StopReason.CANCELLED
        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", reason.value)

    def _propose_pending_selection_for_new_task(self, store, session, task_id):
        """Apply this session's pre-task selection to the just-bootstrapped
        task, before the runtime is ever invoked (see the module docstring's
        "First prompt" rule): this session's own explicit ``pending_selection``
        if it made one, else the inherited ``default_selection`` (the
        persisted user default from a prior session). Either way the session
        field consumed is cleared afterward -- it is never shown as
        "pending" again once a real task/revision exists (see
        ``_current_selection_context``, which now reads task history
        instead). Best-effort: a failure here never fails the prompt itself
        -- the client still sees its ordinary model/thought_level defaults
        via the next ``configOptions`` read, just without this choice
        honored.

        If the saved route is no longer selectable (its connection was
        removed/deauthorized since the choice was made), this never
        auto-proposes the config_options.py substitute value as if it were
        the saved choice: the task is deliberately left without a
        Cortex-proposed revision at all, and the skip is reported via
        ``observe`` (never silent) -- see the F-minor "unavailable route"
        review finding.

        T035 FINDING-05 fix: a catalog refresh mints a new route id for an
        unchanged (connection, model) pair whenever the observed
        ``supported_parameters`` differ (e.g. per-model thought levels were
        recorded) -- see ``catalog.discovery.route_fingerprint``. The saved
        route is therefore resolved by its stable ``(connection_id, model)``
        identity (``configuration.resolution.resolve_saved_route``) against
        *this call's own freshly-read* ``entries``, never by a remembered
        id alone; a legacy ``defaults.json`` with no saved identity (written
        before this fix) is recovered once via
        ``catalog.store.find_route_identity`` scanning still-cached older
        snapshots. Either way, once resolved, the fresh ``route_id`` (and
        identity, for the legacy-recovery case) is written back to
        ``defaults.json`` so this never has to re-resolve on the next task.
        A connection+model match failure (the model was genuinely
        withdrawn) still fails closed, but now names the model instead of
        the opaque ``pending_selection_route_unavailable``.
        """
        pending = session.get("pending_selection") or session.get("default_selection")
        session["pending_selection"] = None
        session["default_selection"] = None
        if pending is None:
            return
        entries, connections = self._selectable_entries_and_connections()
        catalog_entry, withdrawn_model = self._resolve_pending_route(entries, connections, pending)
        if catalog_entry is None:
            if withdrawn_model is not None:
                observe(self.environ, "session/prompt", f"pending_selection_model_withdrawn:{withdrawn_model}")
            else:
                observe(self.environ, "session/prompt", "pending_selection_route_unavailable")
            return
        if catalog_entry.route.id != pending.get("route_id"):
            self._persist_migrated_default(catalog_entry, pending.get("parameters") or {})
            observe(self.environ, "session/prompt", "pending_selection_route_migrated")
        service = ConfigurationService(store)
        inputs = ConfigurationInputs(route=catalog_entry.route, parameters=pending.get("parameters") or {})
        try:
            service.ui_select(task_id, catalog_entry, inputs, 0, session["owner_epoch"])
        except PortableError:
            observe(self.environ, "session/prompt", "pending_selection_apply_failed")

    def _resolve_pending_route(self, entries, connections, pending):
        """T035 FINDING-05: resolve a saved pre-task selection (``pending``,
        the ``route_id``/``connection_id``/``model``/``parameters`` shape
        ``_default_selection_from_user_defaults``/``_apply_pre_task_selection``
        both produce) against ``entries`` -- this call's own freshly-read
        selectable catalog entries.

        Returns ``(catalog_entry, withdrawn_model)``: on success
        ``withdrawn_model`` is ``None`` (whether or not the id actually
        changed -- the caller compares ``catalog_entry.route.id`` against
        ``pending["route_id"]`` itself to decide whether a migration write
        is needed); on failure ``catalog_entry`` is ``None`` and
        ``withdrawn_model`` names the model that disappeared, or is itself
        ``None`` when there was not even a recoverable identity to check
        (an unknown connection/route with nothing to name).

        See ``configuration.resolution.resolve_saved_route`` for the actual
        identity match (connection_id + model, never model alone) --
        exactly what makes a different model or a different connection
        impossible to land on here by construction.
        """
        connection_id, model = pending.get("connection_id"), pending.get("model")
        if not connection_id or not model:
            # Legacy defaults.json (written before this fix): try the
            # direct id match first (still the common case -- most saved
            # defaults are not yet stale), then a one-time reverse lookup
            # of this route id's own identity from older cached snapshots.
            route_id = pending.get("route_id")
            direct = next((entry for entry in entries if entry.route.id == route_id), None)
            if direct is not None:
                return direct, None
            from ..catalog import store as catalog_store

            identity = catalog_store.find_route_identity(connections.keys(), route_id, self.environ)
            if identity is None:
                return None, None
            connection_id, model = identity
        resolution = resolve_saved_route(entries, connection_id, model)
        return resolution.entry, resolution.withdrawn_model

    def _persist_migrated_default(self, catalog_entry, parameters) -> None:
        """T035 FINDING-05: record a saved default's migration to its
        current route id (a refresh minted a new one for the same
        connection+model) -- self-healing ``defaults.json`` so the next
        task resolves directly by id again, without needing
        :meth:`_resolve_pending_route`'s fallback path at all."""
        current = configuration_store.read_user_defaults(self.environ)
        defaults = dict(current["defaults"])
        defaults["route_id"] = catalog_entry.route.id
        defaults["route_connection_id"] = catalog_entry.connection_id
        defaults["route_model"] = catalog_entry.route.model
        defaults["parameters"] = dict(parameters)
        configuration_store.write_user_defaults(defaults, current["ceiling"], self.environ)

    def _run_command_body(self, request_id, session_id, session, recognized, cancel_event):
        command_id, argument = recognized
        store = self._store_for(session["project"])

        def _reply(text_reply):
            # A command never streams incrementally and never calls the
            # runtime; a request that was cancelled while this synchronous
            # handling ran still gets its one, final response -- the
            # transport contract never drops a response, only intermediate
            # notifications (see agent_message_chunk emit_update elsewhere).
            if self.transport is None:
                return
            if not cancel_event.is_set():
                self.transport.send_notification(protocol.agent_message_chunk(session_id, text_reply))
            reason = StopReason.CANCELLED.value if cancel_event.is_set() else StopReason.END_TURN.value
            self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason)))
            observe(self.environ, "session/prompt", "command:" + command_id)

        if command_id == acp_commands.CORTEX_STATUS:
            entries, connections = self._selectable_entries_and_connections()
            options = self._config_options_for_session(session, store)
            _reply(acp_commands.build_status_text(
                config_options=options, connection_setup_summary=self._connection_setup_summary(connections)))
            return
        if command_id == acp_commands.CORTEX_SETUP:
            _reply(acp_commands.build_setup_text())
            return
        if command_id == acp_commands.CORTEX_STOP:
            self._run_cortex_stop(request_id, session_id, session, cancel_event, _reply)
            return
        if command_id == acp_commands.CORTEX_ANSWER:
            self._run_cortex_answer(request_id, session_id, session, argument, cancel_event, _reply)
            return
        if command_id == acp_commands.CORTEX_NEW_TASK:
            self._run_cortex_new_task(request_id, session_id, session, argument, cancel_event)
            return
        if command_id == acp_commands.HARVEST:
            self._run_harvest(request_id, session_id, session, argument, cancel_event)
            return

        # cortex-model / cortex-effort / cortex-language: share the exact
        # selection code path with session/set_config_option (see
        # _apply_config_selection) -- except cortex-model, which (T035)
        # still resolves a *whole* route (any provider, by route id or
        # disambiguated label; see resolve_model_argument) rather than a
        # provider-scoped model name, and so goes through
        # _apply_route_selection (provider+model bundled) instead of the
        # single-axis config_id='model' path session/set_config_option uses.
        route_selection_entry = None
        if command_id == acp_commands.CORTEX_LANGUAGE:
            config_id = acp_config_options.LANGUAGE_OPTION_ID
            resolved = acp_commands.resolve_language_argument(argument)
            if resolved is None:
                _reply("Usage: /cortex-language auto|en|ru")
                return
        else:
            entries, _connections = self._selectable_entries_and_connections()
            if command_id == acp_commands.CORTEX_MODEL:
                config_id = acp_config_options.MODEL_OPTION_ID
                resolved = acp_commands.resolve_model_argument(argument, entries)
                if resolved is None:
                    _reply(f"No matching model for {argument!r}. Use /cortex-status to see the current options.")
                    return
                route_selection_entry = next(entry for entry in entries if entry.route.id == resolved)
            else:
                config_id = acp_config_options.THOUGHT_LEVEL_OPTION_ID
                resolved = argument.strip() or None
                if resolved is None:
                    _reply("/cortex-effort requires a value. Use /cortex-status to see the current options.")
                    return

        if session["task_id"] is not None:
            # Durable capture first, mirroring an ordinary prompt turn (see
            # module docstring); pre-task commands never create a task and
            # record nothing durable beyond the user-default write inside
            # _apply_config_selection.
            origin = f"acp-command:{session_id}"
            source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
            try:
                append_source_event(store, source_event, f"/{command_id} {argument}".rstrip(), session["task_id"])
            except StoreError:
                self._reply_error(request_id, -32603, "Durable capture failed; the command was not applied.")
                observe(self.environ, "session/prompt", "capture_failed")
                return

        with session["lock"]:
            if route_selection_entry is not None:
                accepted, options, error = self._apply_route_selection(store, session, route_selection_entry)
            else:
                accepted, options, error = self._apply_config_selection(store, session, config_id, "value_id", resolved)
        if not accepted:
            self.transport.send_notification(protocol.config_option_update(session_id, options))
            message = error.message if error is not None else "The selection could not be applied."
            _reply(f"Could not apply {command_id}: {message}")
            return
        self.transport.send_notification(protocol.config_option_update(session_id, options))
        _reply(f"{command_id} set to {resolved!r} (pending — applies to the next run).")

    # -- T020 steering commands ------------------------------------------

    def _runtime_call_kwargs(self, method_name, **candidate_kwargs):
        """Only pass kwargs the bound runtime method actually declares (the
        same detect-then-pass pattern as ``project_root``/``source_origin``
        elsewhere in this class) -- lets a plain ``OrchestratingRuntime``
        (no ``project_root`` concept; used directly in tests) and the
        multi-project ``ProductionOrchestratingRuntime`` share this code."""
        method = getattr(self.runtime, method_name, None)
        if method is None:
            return None, {}
        try:
            accepted = set(inspect.signature(method).parameters)
        except (TypeError, ValueError):
            accepted = set()
        return method, {key: value for key, value in candidate_kwargs.items() if key in accepted}

    def _run_cortex_stop(self, request_id, session_id, session, cancel_event, reply):
        task_id = session["task_id"]
        if task_id is None:
            reply("There is no active task on this session to stop.")
            return
        method, kwargs = self._runtime_call_kwargs("stop_task", project_root=session["project"])
        if method is None:
            reply("This runtime does not support /cortex-stop.")
            return
        try:
            method(task_id, session_id, lambda text: True, **kwargs)
        except Exception:
            self._reply_error(request_id, -32603, "Failed to stop the task.")
            observe(self.environ, "session/prompt", "cortex_stop_failed")
            return
        language = acp_messages.select_language(None, [])
        reply(acp_messages.render("stop_confirmed", language))

    def _run_cortex_answer(self, request_id, session_id, session, argument, cancel_event, reply):
        task_id = session["task_id"]
        parsed = acp_commands.parse_answer_argument(argument)
        if task_id is None or parsed is None:
            reply("Usage: /cortex-answer <question-number> <answer text>")
            return
        index, text = parsed
        list_method, list_kwargs = self._runtime_call_kwargs("list_open_questions", project_root=session["project"])
        answer_method, answer_kwargs = self._runtime_call_kwargs(
            "answer_question", project_root=session["project"],
            emit_tool_call=self._make_emit_tool_call(session_id, cancel_event),
        )
        if list_method is None or answer_method is None:
            reply("This runtime does not support /cortex-answer.")
            return
        open_questions = list_method(task_id, **list_kwargs)
        if not open_questions:
            reply(acp_messages.render("question_answer_none_open", acp_messages.select_language(None, [])))
            return
        if index < 1 or index > len(open_questions):
            reply(acp_messages.render("question_answer_unknown_index", acp_messages.select_language(None, [])))
            return
        question = open_questions[index - 1]
        store = self._store_for(session["project"])
        origin = f"acp-command:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
        try:
            append_source_event(store, source_event, f"/cortex-answer {argument}".rstrip(), task_id)
        except StoreError:
            self._reply_error(request_id, -32603, "Durable capture failed; the answer was not applied.")
            observe(self.environ, "session/prompt", "capture_failed")
            return

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            return self.transport.send_notification(protocol.agent_message_chunk(session_id, chunk_text))

        try:
            reason = answer_method(task_id, session_id, question, text, emit_update, cancel_event, **answer_kwargs)
        except Exception:
            self._reply_error(request_id, -32603, "Failed to deliver the answer.")
            observe(self.environ, "session/prompt", "cortex_answer_failed")
            return
        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", "command:" + acp_commands.CORTEX_ANSWER)

    def _run_harvest(self, request_id, session_id, session, argument, cancel_event):
        """T035: `/harvest` -- an explicit knowledge-harvest request for the
        task this session is already on.

        The coordinator's role keeps a harvest explicit-only, and until this
        command existed "explicit" had nothing to express it: a model had to
        decide from prose whether the user had really asked. A command was
        pressed or it was not.

        Deliberately an ordinary turn on the existing task, not a mechanism
        of its own: the request is recorded as the user's own message, so
        steering, continuation, language and every durable guarantee apply to
        it exactly as they do to anything else the user says.
        """
        store = self._store_for(session["project"])
        language = self._current_language_for_session(store, session) or "en"
        if session.get("task_id") is None:
            if self.transport is not None:
                self.transport.send_notification(protocol.agent_message_chunk(
                    session_id, acp_messages.render("harvest_no_task", language)))
                self.transport.send_response(
                    protocol.result_response(request_id, protocol.prompt_result(StopReason.END_TURN.value)))
            return
        scope = argument.strip()
        text = acp_messages.render(
            "harvest_requested", language,
            scope=(f" ({scope})" if scope else ""),
        )
        self._run_prompt_body(request_id, session_id, session, text, cancel_event)

    def _run_cortex_new_task(self, request_id, session_id, session, argument, cancel_event):
        """T020: creates a brand-new, separate task from ``argument`` and
        gives it its own first coordinator turn -- ``session["task_id"]``
        (the active task) is never read or mutated here. Continuing this
        new task from a later ordinary prompt on the same ACP session is
        out of this task's scope (see docs/acp/steering-and-questions.md's
        UNKNOWNs) -- this command's own turn is this new task's only
        guaranteed turn for now."""
        text = argument.strip()
        if not text:
            if self.transport is not None:
                self.transport.send_notification(protocol.agent_message_chunk(session_id, "Usage: /cortex-new-task <task text>"))
                self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(StopReason.END_TURN.value)))
            return
        store = self._store_for(session["project"])
        origin = f"acp-new-task:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
        try:
            result = bootstrap_task(store, source_event, text, owner_ref=origin)
        except StoreError:
            self._reply_error(request_id, -32603, "Durable capture failed; the new task was not created.")
            observe(self.environ, "session/prompt", "capture_failed")
            return
        new_task_id = result.task_id

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            return self.transport.send_notification(protocol.agent_message_chunk(session_id, chunk_text))

        try:
            if self._runtime_accepts_project_root and self._runtime_accepts_source_identity:
                reason = self.runtime.start_prompt(
                    new_task_id, session_id, text, emit_update, cancel_event,
                    project_root=session["project"], source_origin=origin, source_event_id=source_event.event_id)
            elif self._runtime_accepts_project_root:
                reason = self.runtime.start_prompt(new_task_id, session_id, text, emit_update, cancel_event, project_root=session["project"])
            else:
                reason = self.runtime.start_prompt(new_task_id, session_id, text, emit_update, cancel_event)
        except OrchestrationUnavailable as exc:
            self.transport.send_response(protocol.error_response(
                request_id, RpcError(-32001, "orchestration_unavailable", data={"retryable": False, "reason": str(exc) or "orchestration_unavailable"})))
            observe(self.environ, "session/prompt", "cortex_new_task_unavailable")
            return
        except Exception:
            self._reply_error(request_id, -32603, "Failed to start the new task.")
            observe(self.environ, "session/prompt", "cortex_new_task_failed")
            return
        language = acp_messages.select_language(None, [])
        if self.transport is not None:
            self.transport.send_notification(protocol.agent_message_chunk(
                session_id, acp_messages.render("new_task_created", language, task_id=new_task_id)))
            self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", "command:" + acp_commands.CORTEX_NEW_TASK)

    @staticmethod
    def _portable_error_to_rpc(error: PortableError) -> RpcError:
        data = dict(code=error.code, remediation=error.remediation)
        if error.category == PortableErrorCategory.VALIDATION:
            return RpcError(-32602, error.remediation, data=data)
        return RpcError(-32001, error.remediation, data=data)

    def _apply_config_selection(self, store, session, config_id, kind, value):
        """Shared by ``session/set_config_option`` and the ``/cortex-effort``/
        ``/cortex-language`` commands (see module + ``commands.py``
        docstrings; ``/cortex-model`` instead goes through
        ``_apply_route_selection`` -- see ``_run_command_body``).

        T035: ``provider``, ``model``, and ``thought_level`` are three
        independent configIds (see ``acp.config_options`` module
        docstring) -- never one fused route select. Changing ``provider``
        necessarily changes engine and so may change the valid model/effort
        sets: the same model *name* is kept if the new provider happens to
        offer it too, otherwise the new provider's own first model is used
        explicitly (never a silently-kept invalid model, never a silently
        dropped selection) and ``thought_level`` is always reset (a
        different route's supported values may differ entirely). Changing
        ``model`` (within the current provider) also resets
        ``thought_level`` for the same reason; only changing
        ``thought_level`` itself preserves the current provider+model.

        Returns ``(accepted: bool, options: list[dict], error: RpcError | None)``.
        ``options`` is the *new* full configOptions list on success, or the
        *unchanged, previous* list (never a partially-applied one) on failure.
        """
        previous_options = self._config_options_for_session(session, store)
        option = next((item for item in previous_options if item["id"] == config_id), None)
        if option is None:
            return False, previous_options, RpcError(-32602, f"Unknown configId {config_id!r}.")
        if kind == "boolean":
            # No boolean-typed option is ever emitted by this agent (see
            # config_options.py); a boolean value is therefore always
            # rejected regardless of any client capability advertisement.
            return False, previous_options, RpcError(-32602, f"configId {config_id!r} does not accept a boolean value.")
        allowed_values = {item["value"] for item in option.get("options", [])}
        if value not in allowed_values:
            return False, previous_options, RpcError(-32602, f"{value!r} is not a valid value for configId {config_id!r}.")

        if config_id == acp_config_options.LANGUAGE_OPTION_ID:
            # T020: an independent settings axis -- never a route/parameter
            # override, never gated on model selectability, and (unlike
            # provider/model/thought_level) never revision-gated against
            # ConfigurationService history: this value lives on a single
            # durable task_orchestration column (or, pre-task, the same
            # plain T008 user-default row provider/model/thought_level use),
            # so there is no concurrent-revision race to detect/reject here.
            return self._apply_language_selection(store, session, value)

        provider_option = next(
            (item for item in previous_options if item["id"] == acp_config_options.PROVIDER_OPTION_ID), None)
        current_connection_id = provider_option["currentValue"] if provider_option is not None else None
        model_option = next(
            (item for item in previous_options if item["id"] == acp_config_options.MODEL_OPTION_ID), None)
        current_model = model_option["currentValue"] if model_option is not None else None

        entries, _connections = self._selectable_entries_and_connections()

        if config_id == acp_config_options.PROVIDER_OPTION_ID:
            provider_entries = [entry for entry in entries if entry.connection_id == value]
            target_entry = next((entry for entry in provider_entries if entry.route.model == current_model), None)
            if target_entry is None and provider_entries:
                target_entry = provider_entries[0]
            parameters_override: dict = {}
        elif config_id == acp_config_options.MODEL_OPTION_ID:
            target_entry = next(
                (entry for entry in entries
                 if entry.connection_id == current_connection_id and entry.route.model == value), None)
            parameters_override = {}
        else:
            target_entry = next(
                (entry for entry in entries
                 if entry.connection_id == current_connection_id and entry.route.model == current_model), None)
            parameters_override = {config_id: value}

        if target_entry is None:
            return False, previous_options, RpcError(-32602, "The selected route is no longer selectable.")

        if session["task_id"] is None:
            return self._apply_pre_task_selection(store, session, target_entry, parameters_override, config_id)
        return self._apply_mid_task_selection(store, session, target_entry, parameters_override)

    def _apply_route_selection(self, store, session, catalog_entry):
        """Select a whole route (provider+model together) in one step --
        used only by the ``/cortex-model`` command's free-text resolution
        (which matches by full route id/label across *every* provider, see
        ``resolve_model_argument``/``_run_command_body``), unlike the
        protocol-level ``configId='model'`` axis, which is always scoped to
        the currently selected provider (see ``_apply_config_selection``).
        Always resets ``thought_level``, exactly like a protocol-level
        provider/model change."""
        if session["task_id"] is None:
            return self._apply_pre_task_selection(
                store, session, catalog_entry, {}, acp_config_options.MODEL_OPTION_ID)
        return self._apply_mid_task_selection(store, session, catalog_entry, {})

    def _apply_pre_task_selection(self, store, session, catalog_entry, parameters_override, config_id):
        """Persist the choice as the *normal* T008 user-default route/parameters
        (plain ``route_id``/``parameters`` fields -- never a Cortex-specific
        "pending" marker; see the F2 review finding) and record it on this
        session (in-memory only) as its own pending selection, shown with
        state "pending_next_use" until the first prompt consumes it (see
        ``_propose_pending_selection_for_new_task``)."""
        current = configuration_store.read_user_defaults(self.environ)
        defaults = dict(current["defaults"])
        if config_id in (acp_config_options.PROVIDER_OPTION_ID, acp_config_options.MODEL_OPTION_ID):
            merged_parameters: dict = {}
        else:
            previous = session.get("pending_selection") or session.get("default_selection") or {}
            merged_parameters = dict(previous.get("parameters") or {})
            merged_parameters.update(parameters_override)
        defaults["route_id"] = catalog_entry.route.id
        # T035 FINDING-05: the stable identity alongside the fingerprint id
        # -- see resolve_saved_route/_resolve_pending_route.
        defaults["route_connection_id"] = catalog_entry.connection_id
        defaults["route_model"] = catalog_entry.route.model
        defaults["parameters"] = merged_parameters
        configuration_store.write_user_defaults(defaults, current["ceiling"], self.environ)
        session["pending_selection"] = dict(
            route_id=catalog_entry.route.id, parameters=merged_parameters,
            connection_id=catalog_entry.connection_id, model=catalog_entry.route.model,
        )
        session["default_selection"] = None
        return True, self._config_options_for_session(session, store), None

    def _apply_mid_task_selection(self, store, session, catalog_entry, parameters_override):
        """Propose exactly once against the caller's own view of the latest
        revision; never automatically retry on ``stale_configuration_revision``.

        An automatic retry that silently re-reads the latest revision and
        re-proposes would build its ``parameters``/route merge from state a
        concurrent session may have just changed, clobbering that change
        without ever telling either caller (a lost update -- see F1 review
        finding). Instead this always surfaces staleness to the caller as an
        explicit error carrying freshly rebuilt current options, so the
        caller re-reads and decides, rather than this method guessing.
        """
        task_id = session["task_id"]
        service = ConfigurationService(store)
        history = service.history(task_id)
        expected_revision = history[-1]["revision"] if history else 0
        inputs = ConfigurationInputs(route=catalog_entry.route, parameters=parameters_override)
        try:
            service.ui_select(task_id, catalog_entry, inputs, expected_revision, session["owner_epoch"])
        except PortableError as violation:
            previous_options = self._config_options_for_session(session, store)
            if violation.code == "stale_configuration_revision":
                error = RpcError(-32001, "configuration changed concurrently; review current options and retry",
                                  data={"code": "stale_configuration_revision",
                                        "remediation": "configuration changed concurrently; review current options and retry"})
                return False, previous_options, error
            return False, previous_options, self._portable_error_to_rpc(violation)
        return True, self._config_options_for_session(session, store), None

    def _apply_language_selection(self, store, session, value):
        """``value`` is already validated against ``_LANGUAGE_VALUES`` by
        the caller (``_apply_config_selection``). ``"auto"`` is stored as
        ``None`` everywhere (task column and user-default row alike) --
        the same "unset means auto" convention
        ``acp.messages.select_language`` already uses."""
        normalized = None if value == "auto" else value
        task_id = session["task_id"]
        if task_id is None:
            current = configuration_store.read_user_defaults(self.environ)
            defaults = dict(current["defaults"])
            defaults["language"] = normalized
            configuration_store.write_user_defaults(defaults, current["ceiling"], self.environ)
            session["pending_language"] = normalized
        else:
            acp_steering.set_language_override(store, task_id, normalized)
        return True, self._config_options_for_session(session, store), None

    def _session_set_config_option(self, request_id, params):
        session_id, config_id, kind, value = protocol.validate_set_config_option_params(params)
        session = self.sessions.get(session_id)
        if session is None:
            raise RpcError(-32602, "Unknown sessionId.")
        with session["lock"]:
            store = self._store_for(session["project"])
            accepted, options, error = self._apply_config_selection(store, session, config_id, kind, value)
        if error is not None:
            self.transport.send_response(protocol.error_response(request_id, error))
            if self.transport is not None:
                self.transport.send_notification(protocol.config_option_update(session_id, options))
            return
        self.transport.send_response(protocol.result_response(request_id, protocol.set_config_option_response(options)))
        self.transport.send_notification(protocol.config_option_update(session_id, options))

    def _session_load(self, request_id, params):
        """(T018) Display-only history replay -- see ``acp.replay_store``
        module docstring and ``docs/acp/input-history-and-replay.md``.

        Pure read over the durable ``acp_host_sessions``/``host_transcript``
        rows: never calls ``runtime.start_prompt``, never opens an engine
        session, never writes ``source_events``/``task_events``/
        ``deliveries``/reports -- this method issues no ``BEGIN IMMEDIATE``
        transaction at all. Unknown ``sessionId`` (never durably created, or
        durably created for a different ``cwd``/project) is a plain
        ``-32602`` error, exactly like an unknown ``sessionId`` on any other
        method here.
        """
        session_id, cwd, raw_mcp_servers = protocol.validate_load_session_params(params)
        try:
            project = logical_project_root(canonical_project(cwd))
        except StoreError:
            raise RpcError(-32602, "cwd must be an absolute, canonical, existing directory.") from None
        store = self._store_for(project)
        durable = acp_sessions.get_session(store, session_id)
        if durable is None or durable["project_root"] != project:
            raise RpcError(-32602, "Unknown sessionId.")
        for entry in acp_replay_store.replay_session(store, session_id):
            text = entry["text"]
            for attachment in entry["attachments"]:
                text += (
                    f"\n[attachment: {attachment.get('name') or attachment.get('uri') or attachment.get('kind')} "
                    f"({attachment.get('availability')}) source_ref={attachment.get('source_ref')}]"
                )
            if entry["kind"] == "user_message":
                self.transport.send_notification(protocol.user_message_chunk(session_id, text))
            else:
                self.transport.send_notification(protocol.agent_message_chunk(session_id, text))
        if session_id not in self.sessions:
            # Best-effort: lets a subsequent session/prompt on this
            # connection continue the same durable task (via
            # bootstrap.append_source_event, which needs no held channel
            # secret) without re-running session/new. Not itself asserted by
            # any T018 acceptance line -- session/load's own contract is the
            # replay above; this is only convenience continuity.
            # The task's ownership epoch as it stands at reconnect. Without
            # it every session/set_config_option on a reloaded thread was
            # refused with "missing_ownership_epoch" (live in Zed: the
            # client's declared default_config_options and the user's own
            # picks in the composer alike). Read once, here, where this
            # session's claim to the task is already established (the
            # durable row and its project match, checked above) -- never
            # re-read at proposal time, so a *later* ownership transfer
            # correctly makes this session's epoch stale and its proposal
            # refused, exactly as for a session that never disconnected.
            with store.connection() as db:
                restored_epoch = _current_owner_epoch_obj(db, durable["task_id"])
            self.sessions[session_id] = dict(
                project=project, task_id=durable["task_id"], channel=None, active=None, lock=threading.Lock(),
                owner_epoch=restored_epoch, pending_selection=None, default_selection=None,
                pending_language=None, client_capabilities=dict(self.client_capabilities))
        session = self.sessions[session_id]
        # T035 FINDING-08 fix: report the same selection surface as
        # session/new, not a bare null. ``_config_options_for_session`` ->
        # ``_current_selection_context`` already does the right thing here
        # for free: with ``session["task_id"]`` set (from durable storage
        # above), it reads the task's own configuration_revisions history
        # (state "active"/etc, never "default") -- exactly the task-owned
        # configuration this reconnect must show, distinguishable from a
        # fresh session merely inheriting the current global default (state
        # "default", the ``task_id is None`` branch). A task that never got
        # an active configuration falls through to ``None`` there, which
        # ``build_config_options`` reports with nothing marked as current --
        # itself distinguishable from a task with a real selection.
        options = self._config_options_for_session(session, store)
        response = protocol.load_session_response(config_options=options)
        self.transport.send_response(protocol.result_response(request_id, response))

    def _session_cancel(self, params):
        session_id = protocol.validate_session_cancel_params(params)
        session = self.sessions.get(session_id)
        if session is None:
            return
        try:
            self.runtime.cancel(session_id)
        except Exception:
            pass
        with session["lock"]:
            active = session["active"]
        if active is not None:
            active.set()

    # -- shutdown ----------------------------------------------------

    def _handle_eof(self):
        """Cancel every in-flight prompt; the caller (see ``main``) joins workers."""
        from .. import _fault_injection

        _fault_injection.checkpoint("t028_disconnect_before_cancel")
        if self.host_requester is not None:
            self.host_requester.close()
        for session in list(self.sessions.values()):
            with session["lock"]:
                active = session["active"]
            if active is not None:
                active.set()
        # T028: stop this connection's tasks from starting any new paid
        # work and run the bounded owned-resource reaper -- see
        # acp.production_runtime.ProductionOrchestratingRuntime.disconnect/
        # acp.orchestrating_runtime.OrchestratingRuntime.disconnect_session.
        # A runtime that predates T028 (UnavailableRuntime, most
        # FakeRuntime test doubles) simply has no ``disconnect`` attribute
        # -- never required, never fabricated here.
        disconnect = getattr(self.runtime, "disconnect", None)
        if disconnect is not None:
            for session_id in list(self.sessions):
                try:
                    disconnect(session_id)
                except Exception:
                    pass

    def handle_writer_closed(self):
        """Called (on the transport's writer thread; see
        ``transport.StdioTransport``'s ``on_writer_closed``) when a write
        actually failed -- the client's stdout reader is gone, so this
        agent can never successfully reply to anything again, even though
        its own stdin may still be open and its reader thread still
        blocked waiting on it (T012 review item 1: without this, the
        process would wedge forever in ``join_reader()``, unable to either
        reply or exit). Treated identically to inbound EOF: cancel every
        in-flight prompt so workers unblock and ``main`` can proceed to
        shut down."""
        self._handle_eof()

    def join_workers(self, timeout=_PROMPT_JOIN_TIMEOUT):
        deadline = time.monotonic() + timeout
        with self._workers_lock:
            workers = list(self._workers)
        for worker in workers:
            remaining = max(0.0, deadline - time.monotonic())
            worker.join(remaining)


def main(argv=None, runtime=None, environ=None):
    """Production entry point. ``argv`` is accepted but currently unused."""
    os.umask(0o077)
    environ = environ if environ is not None else os.environ
    # Redirect every non-protocol stdout use to stderr *before* constructing
    # anything else; keep a private binary handle onto the real stdout for
    # the transport's writer thread.
    raw_stdout = sys.stdout.buffer
    sys.stdout = sys.stderr

    agent = Agent(runtime if runtime is not None else UnavailableRuntime(), environ=environ)
    transport = StdioTransport(agent.handle_message, raw_stdout, on_writer_closed=agent.handle_writer_closed)
    agent.bind_transport(transport)
    transport.start()
    # Stop waiting on whichever happens first: the reader hits EOF (normal
    # client-initiated shutdown) or the writer stops because a write
    # actually failed (the client's stdout reader is gone -- T012 review
    # item 1). Blocking only on `join_reader()` would otherwise wedge this
    # process forever in the writer-closed-but-stdin-still-open case.
    reason = transport.wait_for_shutdown_signal()
    if reason == "writer_closed":
        agent.join_workers()
        transport.stop_writer()
        # If stdin is still open, the reader thread may be permanently
        # blocked in `readline()` with no way for this process to make it
        # return (only the peer closing its write end can do that, and
        # this process has no standing to force that). It is a daemon
        # thread, so it would not stop a normal `sys.exit`/interpreter
        # shutdown from happening either way -- but CPython's own
        # finalization sequence has a known race where a daemon thread
        # still holding a buffered stream's internal lock during shutdown
        # can abort the process with a fatal error ("could not acquire
        # lock ... at interpreter shutdown"). Exiting via `os._exit()`
        # here (there is nothing left worth flushing: the writer already
        # failed) sidesteps that finalization sequence entirely rather
        # than trying to force an impossible clean join.
        os._exit(0)
    agent.join_workers()
    transport.stop_writer()
    return 0


if __name__ == "__main__":  # pragma: no cover - exercised via cortex_acp.py
    raise SystemExit(main())
