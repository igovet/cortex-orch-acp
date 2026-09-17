"""The incoming ACP v1 stdio agent: wires transport + protocol + storage + runtime.

Entry point: ``main()`` (see ``cortex_acp.py``, a thin bootstrap identical in
spirit to ``cortex.py``). No network server, no MITM proxy, no custom
editor, no CODEX_HOME dependency. stdout is exclusively protocol traffic;
every log/banner goes to stderr (or the optional
``CORTEX_OBSERVATION_DIR`` value-free JSONL sink, mirroring
``server._write_observation``).

``main(runtime=...)`` is the only way to inject a non-production runtime: the
production entry point (``cortex_acp.py``) never passes ``runtime=``, so it
always gets :class:`runtime.UnavailableRuntime`. Tests that need
``tests.acp_fakes.FakeRuntime`` import it themselves and pass it explicitly
(see ``tests/test_acp_*.py``); this module never imports anything from
``tests/``.
"""
from __future__ import annotations

import collections
import json
import os
import sys
import threading
import time
import uuid
from pathlib import Path
from types import SimpleNamespace

from . import commands as acp_commands
from . import config_options as acp_config_options
from . import protocol
from . import sessions as acp_sessions
from .protocol import RpcError
from .runtime import OrchestrationUnavailable, UnavailableRuntime
from .transport import StdioTransport, EOF
from ..bootstrap import append_source_event, bootstrap_task
from ..catalog.model import CatalogEntry
from ..configuration.resolution import ConfigurationInputs
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
        return json.loads((PLUGIN / ".codex-plugin" / "plugin.json").read_text())["version"]
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

    def __init__(self, runtime, *, environ=None):
        self.runtime = runtime
        self.environ = environ if environ is not None else os.environ
        self.version = _agent_version()
        self.stores = {}
        self.sessions = {}
        self.transport = None
        self.initialized = False
        self.client_info = {}
        self.client_capabilities = {}
        self._workers = []
        self._workers_lock = threading.Lock()
        self._seen_request_ids = collections.OrderedDict()

    def bind_transport(self, transport):
        self.transport = transport

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
                self._reply_error(request_id, -32603, "Storage failed while handling this request.")
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
        stripped_servers = protocol.strip_mcp_servers(raw_mcp_servers)
        session_id = new_id("acps_")
        acp_sessions.create_session(store, session_id, project, self.client_info, self.client_capabilities, stripped_servers)
        session = dict(
            project=project, task_id=None, channel=None, active=None, lock=threading.Lock(),
            owner_epoch=None, pending_selection=None, default_selection=None,
            client_capabilities=dict(self.client_capabilities))
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
        fields, never a Cortex-specific "pending" marker -- or ``None``."""
        defaults = configuration_store.read_user_defaults(self.environ)["defaults"]
        route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
        if not isinstance(route_id, str) or not route_id:
            return None
        parameters = defaults.get("parameters") if isinstance(defaults, dict) else None
        return dict(route_id=route_id, parameters=parameters if isinstance(parameters, dict) else {})

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

    def _config_options_for_session(self, session, store):
        entries, _connections = self._selectable_entries_and_connections()
        current = self._current_selection_context(store, session)
        return acp_config_options.build_config_options(entries, current)

    def _connection_setup_summary(self, connections):
        if not connections:
            return "no connection registered yet; see /cortex-setup"
        configured = sum(1 for record in connections.values() if record.state.value == "configured")
        return f"{configured}/{len(connections)} connection(s) configured"

    def _session_prompt(self, request_id, params):
        session_id, text = protocol.validate_session_prompt_params(params)
        raw_blocks = params.get("prompt") if isinstance(params, dict) else None
        recognized = acp_commands.recognize(raw_blocks)
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
            args=(request_id, session_id, session, text, cancel_event, recognized), daemon=True)
        with self._workers_lock:
            self._workers.append(worker)
        worker.start()

    def _run_prompt(self, request_id, session_id, session, text, cancel_event, recognized=None):
        try:
            if recognized is not None:
                self._run_command_body(request_id, session_id, session, recognized, cancel_event)
                return
            self._run_prompt_body(request_id, session_id, session, text, cancel_event)
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

    def _run_prompt_body(self, request_id, session_id, session, text, cancel_event):
        """The actual prompt turn; wrapped by ``_run_prompt`` for the
        catch-all -32603 safety net and the ``session["active"]`` cleanup.
        """
        store = self._store_for(session["project"])
        origin = f"acp:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
        is_first_prompt = session["task_id"] is None
        try:
            if is_first_prompt:
                result = bootstrap_task(store, source_event, text, owner_ref=origin)
                session["task_id"] = result.task_id
                session["owner_epoch"] = result.owner_epoch
                if result.channel_id is not None:
                    # The channel secret is kept only in this process's
                    # memory (this dict), never logged, never sent to
                    # the ACP client, never persisted.
                    session["channel"] = (result.channel_id, result.secret)
                acp_sessions.bind_task(store, session_id, result.task_id)
            else:
                result = append_source_event(store, source_event, text, session["task_id"])
                if result.owner_epoch is not None:
                    session["owner_epoch"] = result.owner_epoch
        except StoreError:
            self._reply_error(request_id, -32603, "Durable capture failed; nothing was forwarded to orchestration.")
            observe(self.environ, "session/prompt", "capture_failed")
            return
        task_id = session["task_id"]
        if is_first_prompt:
            self._propose_pending_selection_for_new_task(store, session, task_id)

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            delivered = self.transport.send_notification(protocol.agent_message_chunk(session_id, chunk_text))
            if not delivered:
                # The client cannot keep up: never grow memory without
                # bound and never drop a *response* -- only this
                # notification is dropped, and the prompt is cancelled.
                cancel_event.set()
            return delivered

        try:
            reason = self.runtime.start_prompt(task_id, session_id, text, emit_update, cancel_event)
        except OrchestrationUnavailable:
            self.transport.send_response(protocol.error_response(
                request_id, RpcError(-32001, "orchestration_unavailable", data={"retryable": False})))
            observe(self.environ, "session/prompt", "orchestration_unavailable")
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
        """
        pending = session.get("pending_selection") or session.get("default_selection")
        session["pending_selection"] = None
        session["default_selection"] = None
        if pending is None:
            return
        entries, _connections = self._selectable_entries_and_connections()
        catalog_entry = next((entry for entry in entries if entry.route.id == pending["route_id"]), None)
        if catalog_entry is None:
            observe(self.environ, "session/prompt", "pending_selection_route_unavailable")
            return
        service = ConfigurationService(store)
        inputs = ConfigurationInputs(route=catalog_entry.route, parameters=pending.get("parameters") or {})
        try:
            service.ui_select(task_id, catalog_entry, inputs, 0, session["owner_epoch"])
        except PortableError:
            observe(self.environ, "session/prompt", "pending_selection_apply_failed")

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

        # cortex-model / cortex-effort: share the exact selection code path
        # with session/set_config_option (see _apply_config_selection).
        entries, _connections = self._selectable_entries_and_connections()
        if command_id == acp_commands.CORTEX_MODEL:
            config_id = acp_config_options.MODEL_OPTION_ID
            resolved = acp_commands.resolve_model_argument(argument, entries)
            if resolved is None:
                _reply(f"No matching model for {argument!r}. Use /cortex-status to see the current options.")
                return
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
            accepted, options, error = self._apply_config_selection(store, session, config_id, "value_id", resolved)
        if not accepted:
            self.transport.send_notification(protocol.config_option_update(session_id, options))
            message = error.message if error is not None else "The selection could not be applied."
            _reply(f"Could not apply {command_id}: {message}")
            return
        self.transport.send_notification(protocol.config_option_update(session_id, options))
        _reply(f"{command_id} set to {resolved!r} (pending — applies to the next run).")

    @staticmethod
    def _portable_error_to_rpc(error: PortableError) -> RpcError:
        data = dict(code=error.code, remediation=error.remediation)
        if error.category == PortableErrorCategory.VALIDATION:
            return RpcError(-32602, error.remediation, data=data)
        return RpcError(-32001, error.remediation, data=data)

    def _apply_config_selection(self, store, session, config_id, kind, value):
        """Shared by ``session/set_config_option`` and the ``/cortex-model``/
        ``/cortex-effort`` commands (see module + ``commands.py`` docstrings).

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

        if config_id == acp_config_options.MODEL_OPTION_ID:
            target_route_id = value
            parameters_override: dict = {}
        else:
            model_option = next((item for item in previous_options if item["id"] == acp_config_options.MODEL_OPTION_ID), None)
            target_route_id = model_option["currentValue"] if model_option is not None else None
            parameters_override = {config_id: value}
        entries, _connections = self._selectable_entries_and_connections()
        catalog_entry = next((entry for entry in entries if entry.route.id == target_route_id), None)
        if catalog_entry is None:
            return False, previous_options, RpcError(-32602, "The selected route is no longer selectable.")

        if session["task_id"] is None:
            return self._apply_pre_task_selection(store, session, catalog_entry, parameters_override, config_id)
        return self._apply_mid_task_selection(store, session, catalog_entry, parameters_override)

    def _apply_pre_task_selection(self, store, session, catalog_entry, parameters_override, config_id):
        """Persist the choice as the *normal* T008 user-default route/parameters
        (plain ``route_id``/``parameters`` fields -- never a Cortex-specific
        "pending" marker; see the F2 review finding) and record it on this
        session (in-memory only) as its own pending selection, shown with
        state "pending_next_use" until the first prompt consumes it (see
        ``_propose_pending_selection_for_new_task``)."""
        current = configuration_store.read_user_defaults(self.environ)
        defaults = dict(current["defaults"])
        if config_id == acp_config_options.MODEL_OPTION_ID:
            merged_parameters: dict = {}
        else:
            previous = session.get("pending_selection") or session.get("default_selection") or {}
            merged_parameters = dict(previous.get("parameters") or {})
            merged_parameters.update(parameters_override)
        defaults["route_id"] = catalog_entry.route.id
        defaults["parameters"] = merged_parameters
        configuration_store.write_user_defaults(defaults, current["ceiling"], self.environ)
        session["pending_selection"] = dict(route_id=catalog_entry.route.id, parameters=merged_parameters)
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
        for session in list(self.sessions.values()):
            with session["lock"]:
                active = session["active"]
            if active is not None:
                active.set()

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
    transport = StdioTransport(agent.handle_message, raw_stdout)
    agent.bind_transport(transport)
    transport.start()
    transport.join_reader()
    agent.join_workers()
    transport.stop_writer()
    return 0


if __name__ == "__main__":  # pragma: no cover - exercised via cortex_acp.py
    raise SystemExit(main())
