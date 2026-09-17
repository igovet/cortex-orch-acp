"""EngineSessionManager: the outgoing ACP client + process/session lifecycle.

Implements ``portable.engine_contract.EngineRuntime`` (structural typing --
no inheritance) by composing exactly one
:class:`~.process.EngineProcess`, one :class:`~.client.EngineClient`, and
one :class:`~.correlation.CorrelationTables` per engine instance. See those
modules' docstrings for why none of the three is ever shared across engine
instances (this is what makes identical wire ids across two engines never
cross-route).
"""
from __future__ import annotations

import queue
import threading

from ..configuration.service import ConfigurationRejected
from ..portable.engine_contract import EngineCapability, EngineEvent, EngineEventKind, StopReason, UsageBasis
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from . import stub_handlers
from .client import EngineClient, EngineProtocolError, EngineTimeout, EngineTransportClosed
from .correlation import CorrelationTables
from .fs_bridge import FsBridge, FsBridgeError
from .mcp_bridge import build_mcp_servers
from .permissions import DuplicateEnginePermissionRequestId, PermissionBridge
from .process import EngineProcess
from .resource_binding import BindingRegistry, ResourceBinding
from .resource_policy import ResourcePolicy
from .terminal_bridge import TerminalBridge, TerminalBridgeError

DEFAULT_INITIALIZE_TIMEOUT = 15.0
DEFAULT_CALL_TIMEOUT = 30.0
DEFAULT_UPDATE_QUEUE_MAXSIZE = 2048
DEFAULT_ABANDON_WAIT_SECONDS = 5.0
# T014 review (LOW): a bound on concurrent agent-to-client request handler
# threads per engine instance -- an engine that floods
# permission/fs/terminal requests must never be able to spawn unbounded
# threads; excess requests are refused immediately (never queued/blocked)
# instead.
DEFAULT_MAX_CONCURRENT_CLIENT_REQUESTS = 16
DEFAULT_CLIENT_REQUEST_JOIN_TIMEOUT = 5.0

_STOP_REASON_FROM_WIRE = {
    "end_turn": StopReason.END_TURN,
    "max_tokens": StopReason.MAX_TOKENS,
    "max_turn_requests": StopReason.MAX_TURN_REQUESTS,
    "refusal": StopReason.REFUSAL,
    "cancelled": StopReason.CANCELLED,
}


def _option_currently_supported(observed_config_options, option_id, value) -> bool:
    """Generic (engine-agnostic) re-check: is ``option_id=value`` present in
    ``observed_config_options`` right now? Used by ``configure()`` (F3, T013
    review) to catch an engine's option set narrowing *mid-call* (e.g. as a
    direct effect of an earlier option in the same call being applied) --
    distinct from, and in addition to, whatever pre-check an adapter-specific
    ``option_mapping`` (e.g. ``engine.codex_adapter_acp.codex_option_mapping``)
    already did against the *original* observed options before this call
    started."""
    for item in observed_config_options or ():
        if not isinstance(item, dict) or item.get("id") != option_id:
            continue
        if item.get("type") == "boolean":
            return isinstance(value, bool)
        allowed = {choice.get("value") for choice in (item.get("options") or []) if isinstance(choice, dict)}
        return value in allowed
    return False


def _default_option_mapping(revision, observed_config_options):
    """Default (route model + supported_parameters) -> ``session/set_config_option``
    mapping. Used only by tests against the fake engine: the exact,
    adapter-specific mapping from a ``ConfigurationRevision`` to real engine
    config option ids/values is T013's responsibility (see
    ``engine.codex_adapter_acp.codex_option_mapping``), injected here via the
    ``option_mapping`` constructor argument (a callable
    ``(revision, observed_config_options) -> list[(option_id, value)]``).
    ``observed_config_options`` is accepted for signature parity with a real
    adapter mapping but is unused by this default (it performs no
    engine-specific pre-check of its own -- callers relying on that must
    supply their own ``option_mapping``)."""
    del observed_config_options
    mapping = [("model", revision.route.model)]
    mapping.extend(revision.route.supported_parameters)
    return mapping


def _fail(code, remediation, *, retry_eligible=False, field=None):
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=retry_eligible, field=field,
    )


class EngineConfigurationOutcomeUnknown(Exception):
    """A ``session/set_config_option`` reply gave no evidence of the actual
    outcome (see ``configure()``'s "no configOptions" branch). A plain
    exception (never ``ConfigurationRejected``, never a ``PortableError``
    special-cased by ``ConfigurationService.apply()``) so it always maps to
    ``indeterminate`` there, the same as a transport timeout/exit."""


class EngineConfigurationPartiallyApplied(Exception):
    """F3 (T013 review): at least one option in this same ``configure()``
    call was already confirmed applied on the engine before a later option
    in the same call was rejected, mismatched, errored, or became
    unsupported. Raising plain ``ConfigurationRejected`` here would be
    wrong: :meth:`configuration.service.ConfigurationService.apply` maps a
    ``ConfigurationRejected`` to the revision's terminal ``failed`` state,
    which would falsely claim *nothing* was applied when the engine is
    actually left in a mixed state (some requested options confirmed,
    others not). A plain exception -- never ``ConfigurationRejected``,
    never a ``PortableError`` special-cased there -- so ``apply()``'s
    catch-all maps it to ``indeterminate`` instead, the same family as a
    transport timeout/exit or an unconfirmed-outcome reply, prompting an
    explicit ``reconcile()`` using fresh engine evidence rather than a
    silent false 'failed'."""


def _session_unusable_error() -> PortableError:
    return PortableError(
        category=PortableErrorCategory.ENGINE_FAILURE,
        code="engine_session_unusable",
        remediation=(
            "A previous turn on this engine session was abandoned while the "
            "engine's wire session/prompt call never confirmed completion "
            "within the abandonment bound; this session can never be safely "
            "reused (a still-outstanding wire turn could otherwise overlap "
            "with a new one). Release it and create a fresh engine session."
        ),
        retry_eligible=False,
    )


class EngineSessionManager:
    """One outgoing engine instance; launches its own process on demand.

    ``channel_credentials`` -- ``(channel_id, secret, project_root)`` -- is
    used only to build the single Cortex MCP server entry passed on
    ``session/new`` (see ``mcp_bridge.build_mcp_servers``); it is never
    placed into the adapter process's own environment (see
    ``process.EngineProcess``).
    """

    def __init__(
        self,
        record,
        workspace_root: str,
        *,
        environ=None,
        channel_credentials=None,
        option_mapping=None,
        install=None,
        initialize_timeout: float = DEFAULT_INITIALIZE_TIMEOUT,
        call_timeout: float = DEFAULT_CALL_TIMEOUT,
        update_queue_maxsize: int = DEFAULT_UPDATE_QUEUE_MAXSIZE,
        abandon_wait_seconds: float = DEFAULT_ABANDON_WAIT_SECONDS,
        policy: ResourcePolicy | None = None,
        host_permission_requester=None,
        max_concurrent_client_requests: int = DEFAULT_MAX_CONCURRENT_CLIENT_REQUESTS,
        terminal_output_persist=None,
    ):
        """``install`` -- a
        :class:`~.codex_install.CodexAcpInstall`, or ``None`` -- is passed
        straight through to :class:`~.process.EngineProcess` on first
        launch (see that class's own docstring); only meaningful for
        ``record.engine == "codex-acp"``.

        ``policy`` (T014) -- a :class:`~.resource_policy.ResourcePolicy`,
        default all-off -- governs both what ``clientCapabilities`` this
        engine instance advertises on ``initialize`` and which of
        ``fs/*``/``terminal/*`` agent-to-client requests are ever serviced
        rather than refused with ``-32601``. ``host_permission_requester``
        -- ``(engine_session_id, params, timeout) -> dict`` or ``None`` --
        is the injected seam to the actual incoming ACP host that owns each
        engine session (see :class:`~.permissions.PermissionBridge`); when
        ``None`` every ``session/request_permission`` is answered
        ``cancelled`` without ever reaching a host.
        """
        self._record = record
        self._workspace_root = workspace_root
        self._environ = environ
        self._channel_credentials = channel_credentials
        self._option_mapping = option_mapping or _default_option_mapping
        self._install = install
        self._initialize_timeout = initialize_timeout
        self._call_timeout = call_timeout
        self._update_queue_maxsize = update_queue_maxsize
        self._abandon_wait_seconds = abandon_wait_seconds
        self._policy = policy if policy is not None else ResourcePolicy()
        self._host_permission_requester = host_permission_requester
        # T024: optional sink -- see ``engine.terminal_bridge.TerminalBridge``
        # and ``delegation.live_adapters``'s real binding of this to
        # ``recovery.large_output_store``; ``None`` (default) never persists.
        self._terminal_output_persist = terminal_output_persist

        self._lock = threading.RLock()
        self._process: EngineProcess | None = None
        self._client: EngineClient | None = None
        self.correlation = CorrelationTables()
        # T014: resource-binding/permission/fs/terminal bridging.
        self.bindings = BindingRegistry()
        self._permission_bridge: PermissionBridge | None = None
        self._fs_bridge: FsBridge | None = None
        self._terminal_bridge: TerminalBridge | None = None
        # T014 review (LOW): bounded concurrency for client-request handler
        # threads, plus tracking so shutdown() can join them with a bounded
        # timeout rather than leaking daemon threads past process release.
        self._client_request_semaphore = threading.Semaphore(max_concurrent_client_requests)
        self._client_request_threads: list[threading.Thread] = []
        self._client_request_threads_lock = threading.Lock()
        self._agent_capabilities: dict = {}
        self._negotiated: tuple = ()
        self._session_config_options: dict[str, list] = {}
        self._session_locks: dict[str, threading.Lock] = {}
        self._update_queues: dict[str, tuple[queue.Queue, threading.Event, object]] = {}
        # F1 (T012 review): per-wire-session turn identity/serialization
        # bookkeeping so an abandoned (generator-closed mid-stream) turn's
        # still-outstanding wire call can never deliver updates into, or
        # overlap in time with, a later turn on the same session.
        self._active_turn_token: dict[str, object] = {}
        self._turn_call_done: dict[str, threading.Event] = {}
        self._poisoned_sessions: set[str] = set()
        # F2 (T012 review): which engine_session_id currently has a
        # prompt_turn actively streaming, so release() can deliver a
        # distinct terminal marker to it.
        self._in_flight_prompt: dict[str, bool] = {}

    # -- lifecycle -------------------------------------------------------

    def _ensure_started(self) -> None:
        with self._lock:
            if self._process is not None:
                return
            self._process = EngineProcess(self._record, self._workspace_root, environ=self._environ, install=self._install)
            self._permission_bridge = PermissionBridge(
                correlation=self.correlation, bindings=self.bindings, logs=self._process.logs,
                host_permission_requester=self._host_permission_requester, call_timeout=self._call_timeout,
            )
            self._fs_bridge = FsBridge()
            self._terminal_bridge = TerminalBridge(persist_sink=self._terminal_output_persist)
            self._client = EngineClient(
                self._process,
                correlation=self.correlation,
                on_update=self._on_update,
                on_client_request=self._handle_client_request,
                on_unknown_notification=self._on_unknown_notification,
                logs=self._process.logs,
            ).start()

    def _handle_client_request(self, method, params, request_id) -> None:
        # T014: never service an agent-to-client request on the transport's
        # own reader thread -- a permission/fs/terminal handler may block
        # (on a host round-trip or process I/O), and other sessions on this
        # same engine instance (or their own engines entirely) must keep
        # making progress while it does.
        #
        # T014 review (LOW): bounded concurrency -- an engine flooding
        # requests must never spawn unbounded threads. A non-blocking
        # semaphore acquire (never waits on the reader thread) that fails
        # is answered immediately: 'busy' as a JSON-RPC error for fs/
        # terminal, a cancelled outcome for a permission request (never
        # left unanswered either way).
        if not self._client_request_semaphore.acquire(blocking=False):
            self._process.logs.observe("client_request_capacity_exceeded", method=method)
            self._reply_busy(method, request_id)
            return
        worker = threading.Thread(
            target=self._run_client_request, name="engine-client-request",
            args=(method, params, request_id), daemon=True,
        )
        with self._client_request_threads_lock:
            self._client_request_threads = [t for t in self._client_request_threads if t.is_alive()]
            self._client_request_threads.append(worker)
        worker.start()

    def _reply_busy(self, method, request_id) -> None:
        if method == "session/request_permission":
            self._client.reply_result(request_id, dict(outcome=dict(outcome="cancelled")))
            return
        self._client.reply_error(request_id, -32000, "busy: too many concurrent client requests on this engine instance.")

    def _run_client_request(self, method, params, request_id) -> None:
        try:
            self._dispatch_client_request(method, params, request_id)
        finally:
            self._client_request_semaphore.release()

    def _dispatch_client_request(self, method, params, request_id) -> None:
        try:
            wire_session_id = (params or {}).get("sessionId") if isinstance(params, dict) else None
            engine_session_id = self.correlation.portable_session_id(wire_session_id) if wire_session_id else None
            if method == "session/request_permission":
                self._dispatch_permission(request_id, wire_session_id, engine_session_id, params)
                return
            if method in ("fs/read_text_file", "fs/write_text_file"):
                self._dispatch_fs(method, request_id, engine_session_id, params)
                return
            if method.startswith("terminal/"):
                self._dispatch_terminal(method, request_id, engine_session_id, params)
                return
            self._process.logs.observe("unexpected_client_request", method=method)
            code, message = stub_handlers.unsupported_client_request_error(method)
            self._client.reply_error(request_id, code, message)
        except Exception:  # noqa: BLE001 - a client request must always get a reply
            self._process.logs.observe("client_request_dispatch_error", method=method)
            try:
                self._client.reply_error(request_id, -32603, "internal_error")
            except Exception:  # noqa: BLE001 - transport may already be closed
                pass

    def _dispatch_permission(self, request_id, wire_session_id, engine_session_id, params) -> None:
        if engine_session_id is None:
            self._process.logs.observe("permission_unknown_wire_session", wire_session_id=wire_session_id)
            self._client.reply_result(request_id, dict(outcome=dict(outcome="cancelled")))
            return
        try:
            result = self._permission_bridge.handle(
                engine_session_id=engine_session_id, wire_session_id=wire_session_id,
                wire_request_id=request_id, params=params,
            )
        except DuplicateEnginePermissionRequestId:
            self._client.reply_error(request_id, -32600, "duplicate_request_id")
            return
        self._client.reply_result(request_id, result)

    def _dispatch_fs(self, method, request_id, engine_session_id, params) -> None:
        if not self._policy.fs_local:
            code, message = stub_handlers.unsupported_client_request_error(method)
            self._client.reply_error(request_id, code, message)
            return
        binding = self.bindings.get(engine_session_id) if engine_session_id else None
        if binding is None:
            self._client.reply_error(request_id, -32001, "no_resource_binding_for_session")
            return
        try:
            if method == "fs/read_text_file":
                result = self._fs_bridge.read_text_file(binding, params)
            else:
                result = self._fs_bridge.write_text_file(binding, params)
        except FsBridgeError as error:
            self._client.reply_error(request_id, error.code, error.message)
            return
        self._client.reply_result(request_id, result)

    def _dispatch_terminal(self, method, request_id, engine_session_id, params) -> None:
        if not self._policy.terminal:
            code, message = stub_handlers.unsupported_client_request_error(method)
            self._client.reply_error(request_id, code, message)
            return
        binding = self.bindings.get(engine_session_id) if engine_session_id else None
        if binding is None:
            self._client.reply_error(request_id, -32001, "no_resource_binding_for_session")
            return
        terminal_id = (params or {}).get("terminalId")
        try:
            if method == "terminal/create":
                new_terminal_id = self._terminal_bridge.create(
                    binding, params, output_byte_limit_max=self._policy.output_byte_limit_max)
                self._client.reply_result(request_id, dict(terminalId=new_terminal_id))
            elif method == "terminal/output":
                self._client.reply_result(request_id, self._terminal_bridge.output(engine_session_id, terminal_id))
            elif method == "terminal/wait_for_exit":
                self._client.reply_result(request_id, self._terminal_bridge.wait_for_exit(engine_session_id, terminal_id))
            elif method == "terminal/kill":
                self._client.reply_result(request_id, self._terminal_bridge.kill(engine_session_id, terminal_id))
            elif method == "terminal/release":
                self._client.reply_result(request_id, self._terminal_bridge.release(engine_session_id, terminal_id))
            else:
                code, message = stub_handlers.unsupported_client_request_error(method)
                self._client.reply_error(request_id, code, message)
        except TerminalBridgeError as error:
            self._client.reply_error(request_id, error.code, error.message)

    def _on_unknown_notification(self, method, params) -> None:
        self._process.logs.observe("unknown_notification_ignored", method=method)

    # -- T014: resource binding registration/invalidation ----------------

    def bind_resource(self, binding: ResourceBinding) -> None:
        self.bindings.bind(binding)

    def invalidate_session(self, engine_session_id: str, reason: str | None = None) -> None:
        self.bindings.invalidate_session(engine_session_id, reason)

    def invalidate_epoch(self, workspace_binding_id: str, new_epoch: str) -> None:
        self.bindings.invalidate_epoch(workspace_binding_id, new_epoch)

    def invalidate_generation(self, engine_session_id: str, new_generation: str) -> None:
        self.bindings.invalidate_generation(engine_session_id, new_generation)

    def invalidate_action(self, engine_session_id: str, action_key: str) -> None:
        self.bindings.invalidate_action(engine_session_id, action_key)

    # -- EngineRuntime -----------------------------------------------

    def negotiate(self, requested_capabilities: tuple[EngineCapability, ...]) -> tuple[EngineCapability, ...]:
        self._ensure_started()
        with self._lock:
            if not self._agent_capabilities:
                # T014: advertise exactly what self._policy opts into (fs/
                # terminal); default policy advertises nothing, matching
                # T012's original empty clientCapabilities object.
                result = self._client.call(
                    "initialize",
                    dict(protocolVersion=1, clientCapabilities=self._policy.client_capabilities()),
                    timeout=self._initialize_timeout,
                )
                self._agent_capabilities = result.get("agentCapabilities") or {}
                self._process.logs.observe("initialized", agent_capabilities=self._agent_capabilities)
            always_granted = {
                EngineCapability.PROTOCOL_VERSION,
                EngineCapability.SESSION_CREATION,
                EngineCapability.PROMPT_STREAMING,
                EngineCapability.CANCELLATION,
            }
            granted = set()
            for capability in requested_capabilities:
                if capability in always_granted:
                    granted.add(capability)
                elif capability == EngineCapability.CONFIGURATION_OPTIONS:
                    # Finalized per-session, gated by whether session/new
                    # actually returned config options -- see configure().
                    granted.add(capability)
                elif capability == EngineCapability.HISTORY_LOAD_WITH_REPLAY and self._agent_capabilities.get("loadSession"):
                    granted.add(capability)
                elif capability == EngineCapability.RESUME_WITHOUT_REPLAY and (
                    (self._agent_capabilities.get("sessionCapabilities") or {}).get("resume")
                ):
                    granted.add(capability)
            self._negotiated = tuple(cap for cap in requested_capabilities if cap in granted)
            return self._negotiated

    def create_session(self, task_id: str, route_id: str, *, session_new_meta: dict | None = None) -> str:
        """``route_id`` is accepted for signature/traceability parity with
        every other caller (never itself sent over the wire here).

        ``session_new_meta`` (T026, additive, default ``None`` -- every
        pre-T026 caller is unaffected): when supplied, sent verbatim as
        this ``session/new`` call's own ``_meta`` field. The only current
        producer is
        ``delegation.live_adapters._session_new_meta_for``, which never
        builds one for an engine/version whose
        ``engine.instruction_delivery_registry`` mechanism is not
        QUALIFIED via exactly this channel (today: claude-agent-acp 0.78.0
        only, its real ``_meta.systemPrompt`` channel -- see that
        registry's own docstring for the dist evidence). Never validated
        here beyond "is a dict" -- the producer is solely responsible for
        its shape.
        """
        self._ensure_started()
        params: dict = dict(cwd=self._workspace_root)
        if self._channel_credentials is not None:
            channel_id, secret, project_root = self._channel_credentials
            params["mcpServers"] = build_mcp_servers(channel_id, secret, project_root)
        else:
            params["mcpServers"] = []
        if session_new_meta is not None:
            if not isinstance(session_new_meta, dict):
                raise _fail("invalid_field", "session_new_meta must be a dict.", field="session_new_meta")
            params["_meta"] = session_new_meta
        with self._lock:
            result = self._client.call("session/new", params, timeout=self._call_timeout)
            wire_session_id = result.get("sessionId")
            if not isinstance(wire_session_id, str) or not wire_session_id:
                raise _fail("engine_invalid_session_response", "session/new did not return a sessionId.")
            portable_session_id = new_id("es_")
            self.correlation.register_session(portable_session_id, wire_session_id)
            self._session_config_options[portable_session_id] = result.get("configOptions") or []
            self._session_locks[portable_session_id] = threading.Lock()
        self._process.logs.observe("session_created", portable_session_id=portable_session_id)
        return portable_session_id

    def configure(self, engine_session_id: str, revision) -> str:
        wire_session_id = self.correlation.wire_session_id(engine_session_id)
        if wire_session_id is None:
            raise _fail("unknown_engine_session", "engine_session_id is not a live session on this engine instance.")
        options = self._session_config_options.get(engine_session_id) or []
        if not options:
            # Unsupported: never reaches the wire.
            raise ConfigurationRejected("engine_does_not_support_configuration_options")
        # ``options`` (this session's already-observed configOptions) is
        # passed through to the mapping callable so an adapter-specific
        # mapping (see ``engine.codex_adapter_acp.codex_option_mapping``)
        # can pre-check a requested option id/value against what this
        # engine instance actually advertised and raise
        # ConfigurationRejected before any wire call, not only after one.
        mapping = list(self._option_mapping(revision, options))
        latest_options = options
        applied_pairs: list[tuple[str, object]] = []
        for option_id, value in mapping:
            # F3 (T013 review): re-validate this option against the *most
            # recently observed* configOptions -- which may have narrowed
            # since session creation as a direct result of an earlier
            # option in this same call being applied (codex-acp's
            # reasoning_effort choices depend on the selected model) --
            # before ever sending it. If an earlier option in this same
            # call already succeeded, a now-unsupported later option is a
            # partial application, never a clean rejection: nothing is
            # sent for it either way.
            if applied_pairs and not _option_currently_supported(latest_options, option_id, value):
                raise EngineConfigurationPartiallyApplied(
                    f"session/set_config_option for {option_id!r} is no longer offered (or {value!r} is no "
                    f"longer an allowed value) after {applied_pairs!r} were already applied in this same "
                    "configure() call; never sent."
                )
            try:
                result = self._client.call(
                    "session/set_config_option",
                    dict(sessionId=wire_session_id, configId=option_id, value=value),
                    timeout=self._call_timeout,
                )
            except EngineProtocolError as protocol_error:
                if applied_pairs:
                    raise EngineConfigurationPartiallyApplied(
                        f"session/set_config_option for {option_id!r} was rejected by the engine "
                        f"({protocol_error.message or 'engine_rejected_configuration'!r}) after "
                        f"{applied_pairs!r} were already applied in this same configure() call."
                    ) from protocol_error
                raise ConfigurationRejected(protocol_error.message or "engine_rejected_configuration") from protocol_error
            if "configOptions" not in result:
                # The wire call itself succeeded (no JSON-RPC error), but
                # the response gives no evidence at all of what -- if
                # anything -- was actually applied: this is an unknown
                # outcome, not an explicit rejection. Documented choice
                # (T012 review F4): raise a plain exception here (never
                # ConfigurationRejected, never a PortableError this
                # method's own callers might special-case) so
                # ConfigurationService.apply()'s catch-all maps it to
                # 'indeterminate', matching a mutation whose outcome is
                # genuinely unknown rather than a confirmed engine refusal.
                # F3: this is also correct -- and never a bare
                # ConfigurationRejected -- for the *first* option, since
                # "unknown" is exactly the right label whether or not
                # anything else in this call already applied.
                raise EngineConfigurationOutcomeUnknown(
                    f"session/set_config_option for {option_id!r} returned no configOptions; "
                    "outcome is unknown, not confirmed and not explicitly rejected."
                )
            latest_options = result["configOptions"]
            current = next((item.get("currentValue") for item in latest_options if item.get("id") == option_id), None)
            if current != value:
                if applied_pairs:
                    raise EngineConfigurationPartiallyApplied(
                        f"engine_did_not_confirm_option:{option_id} after {applied_pairs!r} were already "
                        "applied in this same configure() call."
                    )
                raise ConfigurationRejected(f"engine_did_not_confirm_option:{option_id}")
            applied_pairs.append((option_id, value))
        self._session_config_options[engine_session_id] = latest_options
        pairs = ";".join(f"{option_id}={value}" for option_id, value in mapping)
        return f"{self._process.engine_instance_id}:{wire_session_id}:{pairs}"

    def prompt_turn(self, engine_session_id: str, prompt: str):
        if engine_session_id in self._poisoned_sessions:
            raise _session_unusable_error()
        wire_session_id = self.correlation.wire_session_id(engine_session_id)
        if wire_session_id is None:
            raise _fail("unknown_engine_session", "engine_session_id is not a live session on this engine instance.")
        session_lock = self._session_locks.setdefault(engine_session_id, threading.Lock())
        session_lock.acquire()
        self._in_flight_prompt[engine_session_id] = True
        lock_released = False
        turn_gen = self._prompt_turn_locked(wire_session_id, prompt)
        try:
            for event in turn_gen:
                yield event
        except GeneratorExit:
            # The caller stopped iterating before TURN_END (closed/abandoned
            # the generator): never simply drop the session_lock here --
            # the wire session/prompt call this turn started may still be
            # outstanding, and releasing the lock immediately would let a
            # *new* prompt_turn on the same session start while it is,
            # producing exactly the cross-turn update leak this guards
            # against (see F1, T012 review).
            self._abandon_turn(engine_session_id, wire_session_id, turn_gen)
            lock_released = True
            raise
        finally:
            self._in_flight_prompt.pop(engine_session_id, None)
            if not lock_released:
                session_lock.release()

    def _abandon_turn(self, engine_session_id: str, wire_session_id: str, turn_gen) -> None:
        """Bounded-wait cleanup for a turn whose generator was closed mid-stream.

        Invalidates this turn's active token immediately (any further
        inbound updates for the abandoned wire call are discarded rather
        than delivered anywhere -- see ``_on_update``), requests
        cancellation, and waits up to ``abandon_wait_seconds`` for the
        in-flight ``session/prompt`` call to actually finish. If it does
        not finish in time the engine session is marked ``poisoned``
        (every future ``prompt_turn`` on it raises immediately) rather than
        ever letting a new turn start while the old one's wire call is
        still outstanding.
        """
        self._process.logs.observe("prompt_turn_abandoned", engine_session_id=engine_session_id, wire_session_id=wire_session_id)
        self._active_turn_token.pop(wire_session_id, None)
        self.cancel_wire(wire_session_id)
        done_event = self._turn_call_done.get(wire_session_id)
        completed = done_event.wait(self._abandon_wait_seconds) if done_event is not None else True
        if not completed:
            self._poisoned_sessions.add(engine_session_id)
            self._process.logs.observe(
                "engine_session_poisoned", engine_session_id=engine_session_id, wire_session_id=wire_session_id,
                reason="abandoned_turn_did_not_confirm_completion",
            )
        session_lock = self._session_locks.get(engine_session_id)
        if session_lock is not None:
            try:
                session_lock.release()
            except RuntimeError:
                pass
        try:
            turn_gen.close()
        except Exception:  # noqa: BLE001 - best-effort cleanup only
            pass

    def _prompt_turn_locked(self, wire_session_id: str, prompt: str):
        update_queue: "queue.Queue" = queue.Queue(maxsize=self._update_queue_maxsize)
        overflowed = threading.Event()
        token = object()
        self._update_queues[wire_session_id] = (update_queue, overflowed, token)
        self._active_turn_token[wire_session_id] = token
        call_done = threading.Event()
        self._turn_call_done[wire_session_id] = call_done
        try:
            outcome: dict = {}

            def run_call():
                try:
                    outcome["result"] = self._client.call(
                        "session/prompt",
                        dict(sessionId=wire_session_id, prompt=[dict(type="text", text=prompt)]),
                        timeout=self._call_timeout,
                    )
                except Exception as exc:  # noqa: BLE001 - surfaced to the generator's caller below
                    outcome["error"] = exc
                finally:
                    call_done.set()
                    update_queue.put(None)

            call_thread = threading.Thread(target=run_call, name="engine-prompt-call", daemon=True)
            call_thread.start()

            while True:
                item = update_queue.get()
                if item is None:
                    break
                yield item
                if overflowed.is_set():
                    overflowed.clear()
                    self.cancel_wire(wire_session_id)

            call_thread.join()
            if "error" in outcome:
                error = outcome["error"]
                if isinstance(error, EngineTransportClosed):
                    yield EngineEvent(kind=EngineEventKind.ERROR, error_message="transport_eof")
                    return
                if isinstance(error, EngineTimeout):
                    yield EngineEvent(kind=EngineEventKind.ERROR, error_message="timeout_alive")
                    return
                raise error
            result = outcome.get("result") or {}
            stop_reason = _STOP_REASON_FROM_WIRE.get(result.get("stopReason"))
            if stop_reason is None:
                self._process.logs.observe("unknown_stop_reason", stop_reason=result.get("stopReason"))
                stop_reason = StopReason.END_TURN
            yield EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=stop_reason)
        finally:
            self._update_queues.pop(wire_session_id, None)
            if self._active_turn_token.get(wire_session_id) is token:
                self._active_turn_token.pop(wire_session_id, None)
            self._turn_call_done.pop(wire_session_id, None)

    def cancel(self, engine_session_id: str) -> None:
        wire_session_id = self.correlation.wire_session_id(engine_session_id)
        if wire_session_id is None:
            return
        self.cancel_wire(wire_session_id)

    def cancel_wire(self, wire_session_id: str) -> None:
        self._process.logs.observe("cancel_requested", wire_session_id=wire_session_id)
        try:
            self._client.notify("session/cancel", dict(sessionId=wire_session_id))
        except Exception:  # noqa: BLE001 - cancellation is always best-effort
            pass

    def release(self, engine_session_id: str) -> None:
        wire_session_id = self.correlation.wire_session_id(engine_session_id)
        if wire_session_id is not None and self._in_flight_prompt.get(engine_session_id):
            # F2 (T012 review): a running turn must see a distinct outcome
            # from release(), not silently vanish or be mistaken for a
            # transport failure -- deliver an ERROR event carrying
            # error_message="released_mid_turn" into its update queue
            # (best-effort: if the queue is momentarily full the observation
            # below is still recorded) and record the release itself.
            #
            # Doc clarification (T012 re-review item 3): this is an early,
            # in-stream *marker*, not a guaranteed terminal event. release()
            # only requests cancellation (`cancel_wire` below) -- it cannot
            # force the wire's still-in-flight `session/prompt` call to stop
            # immediately -- so a `TURN_END` (or another `ERROR`) may still
            # follow this marker in the same stream once that call actually
            # resolves. A caller must not assume iteration stops right after
            # observing `released_mid_turn`; it only means release() was
            # requested while this turn was still running.
            self._process.logs.observe(
                "release_during_prompt", engine_session_id=engine_session_id, wire_session_id=wire_session_id,
            )
            entry = self._update_queues.get(wire_session_id)
            if entry is not None:
                update_queue, _overflowed, _token = entry
                try:
                    update_queue.put_nowait(EngineEvent(kind=EngineEventKind.ERROR, error_message="released_mid_turn"))
                except queue.Full:
                    self._process.logs.observe("release_marker_dropped_queue_full", wire_session_id=wire_session_id)
            self.cancel_wire(wire_session_id)
        self.correlation.release_session(engine_session_id)
        self._session_config_options.pop(engine_session_id, None)
        self._session_locks.pop(engine_session_id, None)
        self._poisoned_sessions.discard(engine_session_id)
        if wire_session_id is not None:
            self._update_queues.pop(wire_session_id, None)
            self._active_turn_token.pop(wire_session_id, None)
        # T014: a released session's resource binding is invalidated
        # immediately (any permission request still in flight on it can
        # never be relayed as "selected" after this point -- see
        # permissions.PermissionBridge) and every terminal it opened is
        # killed and freed.
        self.bindings.release(engine_session_id)
        if self._terminal_bridge is not None:
            self._terminal_bridge.release_session(engine_session_id)
        if self._process is not None:
            self._process.logs.observe("session_released", engine_session_id=engine_session_id)

    def recovery_observation(self, engine_session_id: str):
        return None

    def shutdown(self, *, grace: float = 5.0) -> None:
        """Terminate the underlying process; capacity is freed only once
        the process is *observed* to have actually exited (see
        ``process.EngineProcess.release``).

        T014: engine exit invalidates every still-bound resource binding on
        this engine instance and cleans up every terminal it opened,
        exactly like an individual session release (see
        ``BindingRegistry.release``/``TerminalBridge.release_session``)."""
        with self._lock:
            if self._client is not None:
                self._client.close()
            if self._process is not None:
                self._process.release(grace=grace)
            for engine_session_id in self.bindings.all_session_ids():
                self.bindings.release(engine_session_id)
                if self._terminal_bridge is not None:
                    self._terminal_bridge.release_session(engine_session_id)
            with self._client_request_threads_lock:
                threads = list(self._client_request_threads)
            for thread in threads:
                thread.join(timeout=DEFAULT_CLIENT_REQUEST_JOIN_TIMEOUT)

    # -- session/update translation ------------------------------------

    def _on_update(self, params) -> None:
        if not isinstance(params, dict):
            return
        wire_session_id = params.get("sessionId")
        update = params.get("update") or {}
        entry = self._update_queues.get(wire_session_id)
        event = self._translate_update(wire_session_id, update)
        if entry is None:
            if event is not None:
                self._process.logs.observe("update_for_inactive_session", wire_session_id=wire_session_id)
            return
        if event is None:
            return
        update_queue, overflowed, entry_token = entry
        # F1 (T012 review): an update only ever belongs to the *currently
        # active* turn for this wire session -- never to a prior, abandoned
        # turn whose wire call has not yet confirmed completion. This is a
        # defense-in-depth check on top of `_abandon_turn` invalidating the
        # token immediately on abandonment: even in a narrow race it is the
        # last line of defense against delivering a stale turn's updates.
        if self._active_turn_token.get(wire_session_id) is not entry_token:
            self._process.logs.observe("discarded_stale_turn_update", wire_session_id=wire_session_id)
            return
        try:
            update_queue.put_nowait(event)
        except queue.Full:
            overflowed.set()
            self._process.logs.observe("update_backpressure_overflow", wire_session_id=wire_session_id)

    def _translate_update(self, wire_session_id, update):
        kind = update.get("sessionUpdate")
        if kind == "agent_message_chunk":
            text = (update.get("content") or {}).get("text", "")
            return EngineEvent(kind=EngineEventKind.TURN_DELTA, delta_text=text)
        if kind in ("tool_call", "tool_call_update"):
            wire_tool_call_id = update.get("toolCallId")
            ref = (
                self.correlation.portable_tool_call_ref(wire_session_id, wire_tool_call_id)
                if wire_tool_call_id else None
            )
            # T022: a tool_call/tool_call_update carrying `_meta.contextCompaction`
            # is codex-acp's and claude-agent-acp's shared (legacy-presentation)
            # signal for their own internal context compaction -- see
            # delegation.generations module docstring for the exact static dist
            # evidence. Reported as a distinct COMPACTION_SIGNAL (never also as
            # an ordinary TOOL_CALL_OBSERVED -- this synthetic entry is not real
            # tool evidence), keyed by the same stable portable tool_call_ref as
            # the dedup/evidence id, phase carried in compaction_reason.
            meta = update.get("_meta")
            if isinstance(meta, dict) and isinstance(meta.get("contextCompaction"), dict) and ref is not None:
                status = update.get("status") if isinstance(update.get("status"), str) else "unknown"
                return EngineEvent(kind=EngineEventKind.COMPACTION_SIGNAL, tool_call_ref=ref, compaction_reason=status)
            title = update.get("title")
            if isinstance(title, str) and len(title) > 200:
                title = title[:200]
            return EngineEvent(
                kind=EngineEventKind.TOOL_CALL_OBSERVED, tool_call_ref=ref,
                tool_call_kind=update.get("kind") if isinstance(update.get("kind"), str) else None,
                tool_call_title=title if isinstance(title, str) else None,
                tool_call_status=update.get("status") if isinstance(update.get("status"), str) else None,
            )
        if kind == "usage_update":
            usage = update.get("usage") or {}
            values = {key: value for key, value in usage.items() if isinstance(value, int)}
            return EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.ENGINE_REPORTED, usage_values=values)
        if kind == "user_message_chunk":
            # History replay (session/load): never translated into an
            # EngineEvent, never queued, never re-delivered as a user
            # input/mutation -- see the T012 acceptance bullet "A history
            # replay does not cause a mutation or user-input re-delivery."
            self._process.logs.observe("replay_user_message_chunk_ignored")
            return None
        self._process.logs.observe("unrecognized_session_update", update_kind=kind)
        return None
