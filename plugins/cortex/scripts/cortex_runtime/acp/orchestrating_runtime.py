"""T016 model-owned coordination: an :class:`IncomingRuntime` that runs a
real coordinator-led multi-assignment task instead of forwarding user text
to a single downstream agent.

Process boundary (see ``docs/acp/orchestration.md``): this module is the
*main* process. It never itself executes a delegation MCP tool call --
those are handled inside the Cortex MCP subprocess an engine launches as
its own child (``delegation.mcp_tools``). This module only (a) opens/owns
engine sessions via the injected :class:`SessionRuntime`, (b) reacts to the
durable ``task_events`` log those tool calls produce, and (c) performs the
one privileged effect the MCP subprocess is forbidden from doing itself:
calling :class:`delegation.provisioning.ProvisioningService` to actually
provision a delegated worker and prompting its engine session.

Coordinator sessions are provisioned the same way (this module opens them
directly, not via ``ProvisioningService.delegate``, since a coordinator is
not itself a delegation).
"""
from __future__ import annotations

from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
import json
import os
import threading
import time
from typing import Any, Callable, Mapping, Protocol

from ..contracts import StoreError
from ..delegation import events as delegation_events
from ..delegation import integration as delegation_integration
from ..delegation import revision as delegation_revision
from ..delegation import workspace_isolation as delegation_workspace_isolation
from ..delegation.deliveries import MAX_DELIVERY_RETRIES
from ..delegation.provisioning import ProvisioningService
from ..engine import failure_classification
from ..engine import reaper as engine_reaper
from ..engine.instruction_delivery_registry import delivery_for
from ..observability.redact import scrub_text
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..portable.instruction_delivery import DeliveryQualification, qualify_delivery
from ..portable.roles import load_role
from ..recovery.capsule import build_capsule, render_capsule
from ..trusted_channel import ChannelContext, issue_channel_tx
from . import messages as acp_messages
from . import questions_store
from . import steering as acp_steering

COORDINATOR_ROLE_ID = "orchestrator"
DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS = 3
DEFAULT_MAX_TOTAL_CONTINUATIONS = 50
DEFAULT_WORKER_POOL_SIZE = 4
DEFAULT_WAIT_TIMEOUT_SECONDS = 5.0
_WAIT_BACKOFF_START = 0.05
_WAIT_BACKOFF_MAX = 1.0
_UNSET = object()
# Bound on the actionable-error text (e.g. resolve_role's own remediation,
# which lists every valid role id) carried in an assignment_failed event
# payload -- large enough to hold the full ~30-role-id catalog uncut today,
# small enough to never make one event payload unbounded as that catalog
# grows. Truncated with a marker, never silently dropped.
_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS = 1200


def _bounded_detail(text: str) -> str:
    if len(text) <= _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS:
        return text
    return text[: _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS - 3] + "..."


# T035: one ACP ToolCall card per delegated assignment (see
# ``_emit_assignment_tool_call``/``_continuation_loop``). ``ToolCallStatus``
# (tests/fixtures/acp/schema-1.4.0.json) is exactly pending/in_progress/
# completed/failed -- there is no "cancelled" value, so a cancelled
# assignment is rendered as a failed card carrying the cancellation reason
# in its content (see _TOOL_CALL_STATUS_FOR_CANCELLED below). ``kind`` is a
# deliberate "execute" for every worker card: a delegated assignment runs
# arbitrary worker actions (edits, commands, searches, ...), never just one
# of the more specific kinds (read/edit/search/...), and "execute" reads
# truer than the catch-all "other".
_WORKER_TOOL_CALL_KIND = "execute"
_TOOL_CALL_STATUS_FOR_CANCELLED = "failed"


def _worker_tool_call_id(assignment_ref: str) -> str:
    """Stable id for one delegated assignment's card, derived from its own
    durable ``assignment_ref`` (the coordinator-facing delegation
    idempotency key -- see ``delegation.mcp_tools._coordinator_delegate``)
    so every event this assignment ever produces (queued/running/finished/
    failed/cancelled) updates the exact same card, never a new one."""
    return f"worker:{assignment_ref}"


@dataclass(frozen=True)
class PromptOutcome:
    """One engine session turn's outcome, as observed by this runtime.

    ``text`` is the visible assistant text for that turn (streamed via
    ``emit_update`` only for the coordinator -- never a raw worker chunk).
    Any delegation MCP tool call the engine made during the turn has
    already been durably committed by the time this is returned (the ACP
    ``session/prompt`` response only arrives after the engine's own tool
    calls to its Cortex MCP subprocess complete).
    """

    stop_reason: StopReason
    text: str = ""


class HostElicitor(Protocol):
    """T020: optional structured-input dependency, injected the same way
    ``SessionRuntime`` is (never required -- every existing caller/test
    passes none and gets the text-fallback path unconditionally). A real
    implementation (see ``acp.agent``) relays through the connected host's
    ``elicitation/create`` request via that connection's own
    ``HostRequester``, scoped to one ACP session, only when the host
    advertised ``clientCapabilities.elicitation.form`` at ``initialize``.

    ``elicit`` must never raise for an ordinary decline/cancel/timeout/
    unsupported outcome -- it returns ``None`` for all of those (the caller
    then falls back to the plain text question), and only a genuine bug
    propagates as an exception (also treated as ``None`` by the caller, see
    ``OrchestratingRuntime._handle_ask_user``)."""

    def elicit(self, session_id: str, question: str, options: list[str] | None) -> str | None: ...


class SessionRuntime(Protocol):
    """Everything this runtime needs from a live (or fake) engine session
    manager: the T015 ``EngineSessionOpener`` surface (duck-type compatible
    with ``delegation.provisioning.EngineSessionOpener`` -- passed directly
    to ``ProvisioningService``) plus the ability to actually send a prompt.
    """

    def open(self, workspace_binding_id: str, launch: Mapping[str, Any], instructions: str) -> str: ...
    def lookup_owned(self, attempt_id: str) -> str: ...
    def terminate_owned(self, attempt_id: str) -> None: ...
    def prompt(self, session_ref: str, message: str, cancel_event: threading.Event) -> PromptOutcome: ...


class EngineTurnFailed(Exception):
    """Raised by a :class:`SessionRuntime`.prompt() implementation when an
    engine turn failed for a durable, named reason instead of returning a
    normal :class:`PromptOutcome` -- never silently treated as ``end_turn``.

    ``kind`` is a short machine-readable failure kind (for example
    ``"transport_eof"``, ``"timeout_alive"``, or ``"poisoned"``; see
    ``engine.session_manager``'s ``EngineEventKind.ERROR`` messages and
    ``_session_unusable_error``), recorded verbatim in the durable
    ``assignment_failed``/``progress`` event this runtime writes for it.
    """

    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


class SecondCoordinatorRefused(PortableError):
    def __init__(self) -> None:
        super().__init__(
            category=PortableErrorCategory.AUTHORIZATION,
            code="second_coordinator_refused",
            remediation="A coordination turn for this task is already active in this runtime; wait for it to finish or cancel it first.",
            retry_eligible=False,
        )


def _now() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _coalesce(events: list[delegation_events.TaskEvent], revisions: dict[int, dict] | None = None) -> str:
    """Coalesce a batch of claimed events into ONE continuation prompt.

    Every claimed event contributes a line (informative-only event types
    included) so nothing silently vanishes from the record even when it
    does not by itself gate continuation; see
    ``delegation.events.MEANINGFUL_EVENT_TYPES`` for what *does* gate it.

    ``revisions`` (T019), keyed by event id, is this exact batch's own
    freshly captured observed artifact revision for each ``worker_result``
    event (see ``OrchestratingRuntime._process_new_events`` and
    ``delegation.revision``) -- appended as a plain label so the
    coordinator can see a check's evidence is bound to one specific
    revision, never silently assumed still current.

    T023 (fresh review, LOW): a ``worker_result`` published concurrently
    with ``recovery.worker_reconciliation``'s own ``assignment_failed
    (recovery_worker_lost)`` write (the narrow window between that
    module's transactional re-check and a worker's own already-in-flight
    ``publish_result`` call -- see that module's docstring; the far more
    common ordering is caught there directly) can still land both events
    in the *same* claimed batch. Never render the contradictory synthetic
    failure in that case -- a real result for the assignment always wins;
    this is scoped to ``recovery_worker_lost`` specifically, never a
    genuine engine-reported failure, which can legitimately follow a
    result for a different check/step.
    """
    revisions = revisions or {}
    superseded_recovery_failures = {
        event.assignment_ref for event in events
        if event.event_type == "worker_result" and event.assignment_ref
    }
    lines = []
    for event in events:
        payload = event.payload
        ref = event.assignment_ref or ""
        if event.event_type == "user_message":
            lines.append(f"User message: {payload.get('content', '')}")
        elif (
            event.event_type == "assignment_failed"
            and payload.get("error") == "recovery_worker_lost"
            and ref in superseded_recovery_failures
        ):
            # Superseded by the worker_result rendered above for the same
            # assignment in this batch -- never shown as a contradictory
            # failure (see this function's T023 docstring note).
            continue
        elif event.event_type == "worker_result":
            line = f"Assignment {ref} result ({payload.get('report_ref')}): {payload.get('summary', '')}"
            checks = payload.get("checks") or []
            if checks:
                # T017: label each claim's evidence state explicitly --
                # never let a claimed outcome read as if it were confirmed.
                rendered = ", ".join(
                    f"{c.get('name')}={c.get('outcome')} ({c.get('evidence', 'claimed')})" for c in checks
                )
                line += f" Checks: {rendered}."
                line += " " + delegation_revision.label(revisions.get(event.id))
            lines.append(line)
        elif event.event_type == "worker_question":
            lines.append(f"Assignment {ref} question: {payload.get('question', '')}")
        elif event.event_type == "assignment_failed":
            lines.append(f"Assignment {ref} failed: {payload.get('error', 'unknown error')}")
        elif event.event_type == "assignment_cancelled":
            lines.append(f"Assignment {ref} was cancelled: {payload.get('reason', '')}")
        elif event.event_type == "assignment_ready":
            lines.append(f"Assignment {ref} is now running.")
        elif event.event_type == "coordinator_clarification":
            lines.append(f"(delivered) Your clarification to assignment {ref}: {payload.get('message', '')}")
        elif event.event_type == "progress" and payload.get("kind") == "integration_applied":
            paths = ", ".join(payload.get("applied_paths") or []) or "(no changes needed)"
            lines.append(
                f"Integration of assignment {ref} ({payload.get('request_key')}): "
                f"{payload.get('status')}. Paths: {paths}. "
                f"host_buffer_state={payload.get('host_buffer_state')}."
            )
        elif event.event_type == "progress" and payload.get("kind") == "integration_conflict":
            conflicts = payload.get("conflict_paths") or {}
            rendered = "; ".join(f"{path}: {detail.get('reason')}" for path, detail in conflicts.items())
            lines.append(
                f"Integration of assignment {ref} ({payload.get('request_key')}) CONFLICTED, "
                f"nothing was written: {rendered}. host_buffer_state={payload.get('host_buffer_state')}."
            )
        elif event.event_type == "progress":
            lines.append(f"Progress: {payload.get('note', '')}")
        # delegation_requested / coordinator_decision are the coordinator's
        # own prior tool calls; they need no echo back to the coordinator.
    return "\n".join(lines)


@dataclass(frozen=True)
class _IntegrationHostBinding:
    """Minimal stand-in for ``engine.resource_binding.ResourceBinding``
    (T025): ``engine.fs_bridge.HostFsForwarder.read_text_file`` only ever
    reads ``binding.workspace_binding_id`` (its ``RevisionLog`` key) --
    coordinator-side integration has no real per-attempt ``ResourceBinding``
    to reuse, so this supplies just that one field, scoped to the task
    rather than any one attempt (a single project-root-wide "integration"
    revision-log bucket)."""

    workspace_binding_id: str


class OrchestratingRuntime:
    """Model-owned coordination loop; implements ``acp.runtime.IncomingRuntime``."""

    # T027: defensive hard ceiling on consecutive fallback hops for one
    # worker prompt. ``_attempt_worker_route_fallback`` already excludes
    # every previously-tried connection via the predecessor chain, so this
    # never binds for any realistic route-profile size; it only guards
    # against a pathological/corrupt chain ever looping unbounded.
    _MAX_FALLBACK_HOPS = 8

    def __init__(
        self,
        store,
        session_runtime: SessionRuntime,
        workspace_provisioner,
        config_snapshot_source,
        *,
        allow_unqualified_delivery: bool = False,
        max_no_progress_continuations: int = DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS,
        max_total_continuations: int = DEFAULT_MAX_TOTAL_CONTINUATIONS,
        worker_pool_size: int = DEFAULT_WORKER_POOL_SIZE,
        wait_timeout_seconds: float = DEFAULT_WAIT_TIMEOUT_SECONDS,
        host_elicitor: HostElicitor | None = None,
        host_fs_requester: Callable[[str, Mapping[str, Any]], Mapping[str, Any]] | None = None,
    ) -> None:
        self._store = store
        self._engine = session_runtime
        self._workspace = workspace_provisioner
        self._config = config_snapshot_source
        self._allow_unqualified_delivery = bool(allow_unqualified_delivery)
        # T020: optional structured-elicitation dependency (see
        # HostElicitor); None for every pre-T020 construction (production
        # wiring passes a real one only once acp.agent's per-connection
        # HostRequester exists -- see acp.production_runtime).
        self._elicitor = host_elicitor
        # T025 (lead follow-up): optional ``(method, params) -> dict``
        # outbound call on the incoming ACP connection (see
        # ``acp.host_requests.HostRequester``), supplied by the caller only
        # when the connected host actually negotiated an ``fs`` capability
        # -- exactly the same "None means not negotiated/available, never
        # guessed" convention as ``host_elicitor`` above. Used by
        # ``_integrate_assignment`` to build a per-path unsaved-buffer
        # check via the existing T014 ``engine.fs_bridge.HostFsForwarder``;
        # ``None`` here durably records ``host_buffer_state: 'unknown'`` on
        # every integration instead of silently treating "no host fs" the
        # same as "checked, no conflict".
        self._host_fs_requester = host_fs_requester
        # T020: per-task de-duplication of identical consecutive runtime
        # status lines (never persisted -- see acp.messages.Dedup).
        self._status_dedup = acp_messages.Dedup()
        self._provisioning = ProvisioningService(
            store, workspace_provisioner, session_runtime, config_snapshot_source,
            allow_unqualified_delivery=allow_unqualified_delivery,
        )
        self._max_no_progress = max_no_progress_continuations
        self._max_total = max_total_continuations
        self._wait_timeout = wait_timeout_seconds
        self._lock = threading.Lock()
        self._cond = threading.Condition(self._lock)
        self._active_tasks: set[str] = set()
        # In-process caches (lost on process restart; see class docstring
        # and docs/acp/orchestration.md "what's deferred").
        self._coordinator_channels: dict[str, ChannelContext] = {}
        self._coordinator_sessions: dict[str, str] = {}
        self._worker_sessions: dict[tuple[str, str], str] = {}
        self._running: dict[str, set[Future]] = {}
        # In-memory "already submitted" markers, checked in addition to the
        # durable event log: the durable assignment_ready/assignment_failed/
        # assignment_cancelled/clarification-delivered marker for a given
        # ref is only written *after* the corresponding external effect
        # finishes, so relying on the DB alone would let a second
        # _process_new_events call (from a later coordinator continuation,
        # while the first effect is still in flight) submit a duplicate.
        self._dispatched_delegations: set[str] = set()
        self._dispatched_cancels: set[str] = set()
        self._dispatched_integrations: set[int] = set()
        self._delivered_clarification_ids: set[int] = set()
        # T017: attempt_ids whose owning process has already been asked to
        # terminate after reaching a terminal state (worker_result or
        # assignment_failed) -- so a confirmed/ambiguous capacity release
        # actually happens once terminal, and this is only ever requested
        # once per attempt.
        self._terminated_attempts: set[str] = set()
        self._executor = ThreadPoolExecutor(max_workers=worker_pool_size, thread_name_prefix="cortex-worker")
        # T020 (stop-race fix): the cancel_event of whichever start_prompt/
        # answer_question call currently owns this task's continuation loop
        # *in this process* -- lets stop_task() signal that loop to exit
        # promptly instead of only relying on the next ordinary prompt to
        # observe stopped_at. Registered/deregistered under self._lock;
        # never more than one entry per task_id (start_prompt's own
        # SecondCoordinatorRefused guard already prevents two concurrent
        # loops for the same task in this process).
        self._active_cancel_events: dict[str, threading.Event] = {}
        # T023: set by _ensure_coordinator right after a successful
        # recovery/replacement, consumed (popped) by the next _run call for
        # this task_id to emit an honest "new-session recovery" status line
        # -- never "resume" (see recovery.coordinator_recovery module
        # docstring and acp.messages' coordinator_recovery_* keys).
        self._pending_recovery_notice: dict[str, dict] = {}
        # T028: the most recent ACP session_id -> task_id this process has
        # seen start a turn for (start_prompt/answer_question), so
        # cancel(session_id) -- called synchronously on the transport
        # reader thread the instant a session/cancel notification arrives,
        # concurrently with the in-flight turn -- can find the coordinator
        # engine session to invalidate without waiting on that turn at all.
        # Never cleared (a session_id is effectively unique for the life of
        # this process); a stale/unknown session_id is simply a no-op below.
        self._session_task: dict[str, str] = {}

    # -- IncomingRuntime -----------------------------------------------

    def start_prompt(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
        host_elicitor: HostElicitor | None = None,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        """T018: ``source_origin``/``source_event_id`` (when supplied by the
        caller -- see ``acp.agent.Agent``'s signature detection, mirroring
        T016's ``project_root``) identify *this exact call's own* already-
        durably-captured source event (``bootstrap.bootstrap_task``/
        ``append_source_event``): the coordinator's ``user_message`` event
        is deduplicated by that identity (``user_message:<origin>:<event_id>``),
        not a recomputed turn count -- two distinct source events with
        identical text remain two distinct, both-delivered user_message
        events; a replayed identical source event (same origin+event_id)
        is never delivered twice. Omitted (``None``) falls back to the
        pre-T018 session-turn-count identity for callers that do not (yet)
        thread a source event through (every existing direct-call test).

        ``host_elicitor`` (T020, per-call -- never stored as instance state:
        ``OrchestratingRuntime`` is one long-lived instance per project,
        shared across every ACP connection for that project, while a
        ``HostElicitor`` is scoped to exactly one connection's own
        ``HostRequester`` -- see ``acp.agent``) overrides the constructor's
        own ``host_elicitor`` default for this call only; ``None`` (the
        default -- every pre-T020 caller) falls back to that default.

        ``emit_tool_call`` (T035, per-call the same way): when given, one
        ACP ToolCall card (``session/update`` ``tool_call``/``tool_call_update``
        -- see ``acp.protocol.tool_call``) is emitted per delegated
        assignment, in addition to (T035, replacing the plain-text
        ``assignment_delegation_failed`` line for the one case a card fully
        covers -- see ``_emit_assignment_tool_call``) ``emit_update``'s own
        narration. ``None`` (every pre-T035 caller/test) falls back to the
        plain-text-only behavior unchanged."""
        elicitor = host_elicitor if host_elicitor is not None else self._elicitor
        with self._lock:
            if task_id in self._active_tasks:
                raise SecondCoordinatorRefused()
            self._active_tasks.add(task_id)
            self._active_cancel_events[task_id] = cancel_event
            self._session_task[session_id] = task_id
        try:
            return self._run(task_id, session_id, content, emit_update, cancel_event,
                              source_origin=source_origin, source_event_id=source_event_id, host_elicitor=elicitor,
                              emit_tool_call=emit_tool_call)
        finally:
            with self._lock:
                self._active_tasks.discard(task_id)
                if self._active_cancel_events.get(task_id) is cancel_event:
                    del self._active_cancel_events[task_id]

    def cancel(self, session_id: str) -> None:
        """T028 (session/cancel, distinct from the task-wide ``/cortex-stop``
        in :meth:`stop_task`): best-effort, in-process only.

        ``cancel_event`` (set by the caller -- ``acp.agent.Agent
        ._session_cancel`` -- around this same call) is the primary path
        the continuation loop itself observes promptly between steps; that
        alone does *not* interrupt a coordinator turn already blocked
        inside a synchronous host round trip (a permission request via
        ``engine.permissions.PermissionBridge``, or a structured
        elicitation via ``HostElicitor``) -- those keep waiting on their own
        bounded timeout regardless of ``cancel_event``. This method closes
        that specific gap for the one engine session this exact turn is
        using: :meth:`SessionRuntime.invalidate_session` (a duck-typed
        optional hook -- see
        ``delegation.live_adapters.EngineSessionManagerAdapter``) marks
        that engine session's own :class:`engine.resource_binding.BindingRegistry`
        entry released, so a permission reply this bridge is still waiting
        to relay (or a late one arriving after this call) is answered
        ``cancelled`` instead of ever being relayed as if still authorized
        (see ``ResourceBinding.is_current``). Never touches any *other*
        task's or session's engine sessions, never cancels an independent,
        already-dispatched worker assignment this task delegated (scope
        isolation -- see :meth:`stop_task` for the task-wide equivalent),
        and never rolls back anything already applied.

        A ``session_id`` this process never saw start a turn (unknown, or
        only ever handled by a *different* process before a restart) is a
        documented no-op -- true out-of-band cross-process cancellation is
        not this method's job (see the historical note this replaces)."""
        task_id = self._session_task.get(session_id)
        if task_id is None:
            return None
        with self._lock:
            active_event = self._active_cancel_events.get(task_id)
        if active_event is not None:
            active_event.set()
        session_ref = self._coordinator_sessions.get(task_id)
        if session_ref is not None:
            from .. import _fault_injection

            _fault_injection.checkpoint("t028_cancel_before_permission_invalidate")
            invalidate = getattr(self._engine, "invalidate_session", None)
            if invalidate is not None:
                try:
                    invalidate(session_ref, "session_cancelled")
                except Exception:
                    pass
        return None

    def disconnect_session(self, session_id: str) -> None:
        """T028 (host transport EOF/writer-closed): called once per
        session on the disconnected ACP connection (see ``acp.agent.Agent
        ._handle_eof``/``handle_writer_closed``). State up to this point is
        already durable (every mutation this runtime performs is committed
        before it is ever visible); this call itself:

        1. signals this task's in-flight turn (if any, *in this process*)
           to stop via the same ``cancel_event``/binding-invalidation path
           :meth:`cancel` uses -- STOPS starting any new paid work for this
           task by default, per the task's own "no hidden paid work"
           requirement, rather than continuing a coordinator turn no one
           can any longer observe the result of;
        2. records a durable ``progress`` event naming what was actually
           paused/cancelled/unconfirmed -- never silently continuing
           without a trace;
        3. runs the bounded owned-resource reaper sweep
           (:mod:`engine.reaper`) so any capacity reservation or isolated
           workspace a crash concurrent with the disconnect left ambiguous
           is re-probed promptly, never assumed released.

        An unknown ``session_id`` (never started a turn in this process) is
        a no-op beyond the reaper sweep, which is unconditional and safe to
        call redundantly (idempotent, bounded)."""
        task_id = self._session_task.get(session_id)
        if task_id is not None:
            with self._lock:
                active_event = self._active_cancel_events.get(task_id)
                session_ref = self._coordinator_sessions.get(task_id)
            turn_state = "cancel_event_set" if active_event is not None else "no_active_turn_in_process"
            if active_event is not None:
                active_event.set()
            permission_invalidated = False
            if session_ref is not None:
                invalidate = getattr(self._engine, "invalidate_session", None)
                if invalidate is not None:
                    try:
                        invalidate(session_ref, "host_disconnected")
                        permission_invalidated = True
                    except Exception:
                        pass
            try:
                delegation_events.append_event(
                    self._store, task_id, "progress", f"disconnect:{session_id}:{time.monotonic_ns()}",
                    dict(note="host disconnected", turn=turn_state, permission_invalidated=permission_invalidated),
                    origin="runtime",
                )
            except Exception:
                pass
        try:
            engine_reaper.sweep(self._store, self._engine)
        except Exception:
            pass

    def shutdown(self) -> None:
        """T017 (fresh review, MEDIUM): signal every queued-for-capacity
        open to abort promptly (never a reservation leak), terminate every
        owned coordinator/worker process with a confirmed/ambiguous
        release, and shut down the worker-dispatch executor. Idempotent;
        safe to call once at process/runtime teardown."""
        shutdown = getattr(self._engine, "shutdown", None)
        if shutdown is not None:
            # Capacity-aware SessionRuntime (see
            # delegation.live_adapters.EngineSessionManagerAdapter.shutdown):
            # aborts every queued open and terminates every owned process
            # with a confirmed/ambiguous release.
            shutdown()
        self._executor.shutdown(wait=False, cancel_futures=True)

    # -- coordinator provisioning ----------------------------------------

    def _ensure_coordinator(self, task_id: str) -> tuple[ChannelContext, str]:
        cached = self._coordinator_channels.get(task_id)
        if cached is not None:
            return cached, self._coordinator_sessions[task_id]
        # T017: this runtime instance's first use of this task -- reconcile
        # any claim (consumed_by set) whose delivery was never confirmed by
        # a matching mark_delivered (a crash between claim_unconsumed and
        # engine.prompt returning in a prior process), so those events are
        # reclaimed and delivered exactly once instead of stuck forever.
        delegation_events.reconcile_undelivered_claims(self._store, task_id)
        role_set = load_role(COORDINATOR_ROLE_ID)
        preview = self._config.snapshot(task_id)
        qualify_delivery(
            role_set, delivery_for(preview["engine"], preview["version"]), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
                if owner is None:
                    raise PortableError(
                        category=PortableErrorCategory.AUTHORIZATION, code="task_not_owned",
                        remediation="This task has no recorded owner epoch; bootstrap the task before starting orchestration.",
                        retry_eligible=False,
                    )
                owner_epoch = owner["epoch"]
                existing = db.execute(
                    "SELECT * FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
                if existing is None:
                    attempt_ref = new_id("co_")
                    db.execute(
                        "INSERT INTO task_orchestration (task_id, state, coordinator_attempt_ref, updated_at) VALUES (?,?,?,?)",
                        (task_id, "provisioning", attempt_ref, _now()),
                    )
                    channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner_epoch, attempt_id=attempt_ref)
                else:
                    attempt_ref = existing["coordinator_attempt_ref"]
                    row = db.execute(
                        "SELECT channel_id FROM trusted_channels WHERE task_id=? AND role='coordinator' AND attempt_id=? AND revoked_at IS NULL",
                        (task_id, attempt_ref),
                    ).fetchone()
                    if row is None:
                        raise PortableError(
                            category=PortableErrorCategory.INDETERMINATE_WRITE, code="coordinator_channel_missing",
                            remediation="This task's coordinator channel is missing from a live trusted_channels row; this indicates data corruption or a process-restart limitation (see acp.orchestrating_runtime docstring).",
                            retry_eligible=False,
                        )
                    channel_id, secret = row["channel_id"], None
                db.commit()
            except BaseException:
                db.rollback()
                raise
        if secret is None:
            # T023: this process holds neither the channel secret nor an
            # open session for an already-provisioned coordinator -- almost
            # always a process restart. Run the full recovery/replacement
            # sequence (fencing, old-attempt/worker reconciliation, fresh
            # channel, new session, config/permission-carryover check --
            # see recovery.coordinator_recovery) instead of failing closed.
            from ..recovery import coordinator_recovery

            def _open_session(recovery_task_id, recovery_attempt_ref, recovery_channel_id, recovery_secret):
                return self._open_coordinator_session(
                    recovery_task_id, recovery_attempt_ref, recovery_channel_id, recovery_secret, role_set,
                    recovery=True,
                )

            outcome = coordinator_recovery.recover(
                self._store, self._engine, task_id,
                old_owner_epoch=owner_epoch, old_attempt_ref=attempt_ref, open_session=_open_session,
            )
            channel = outcome["channel"]
            session_ref = outcome["session_ref"]
            self._coordinator_channels[task_id] = channel
            self._coordinator_sessions[task_id] = session_ref
            self._pending_recovery_notice[task_id] = outcome
            return channel, session_ref
        from ..trusted_channel import resolve_channel

        channel = resolve_channel(self._store, channel_id, secret)
        # T017: capacity queuing (if the injected SessionRuntime is
        # capacity-aware) happens *inside* open() itself -- see
        # delegation.live_adapters.EngineSessionManagerAdapter.open's
        # docstring; this call simply blocks a bounded amount longer while
        # queued, exactly like a slow real engine launch.
        session_ref = self._open_coordinator_session(task_id, attempt_ref, channel_id, secret, role_set)
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "UPDATE task_orchestration SET state='running', updated_at=? WHERE task_id=?",
                    (_now(), task_id),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        self._coordinator_channels[task_id] = channel
        self._coordinator_sessions[task_id] = session_ref
        return channel, session_ref

    def _open_coordinator_session(
        self, task_id: str, attempt_ref: str, channel_id: str, secret: str, role_set, *, recovery: bool = False,
    ) -> str:
        preview = self._config.snapshot(task_id)
        launch = dict(
            task_id=task_id, attempt_id=attempt_ref, channel_id=channel_id,
            channel_secret=secret, config_revision=preview.get("config_revision"),
        )
        if recovery:
            # T023: tells a capacity/generation-tracking engine (see
            # delegation.live_adapters.EngineSessionManagerAdapter.open) this
            # is a coordinator *replacement*, not this attempt id's first
            # session -- cause="session_replacement", never "session_open",
            # regardless of whether this process itself ever opened this
            # (freshly allocated) attempt id before.
            launch["recovery"] = True
        session_id = self._engine.open("shared_project_root", launch, role_set.body_portable)
        # T035 FINDING-02 fix: ``self._engine.open`` (see
        # ``delegation.live_adapters.EngineSessionManagerAdapter
        # ._maybe_apply_configuration``) runs ``ConfigurationService.apply()``
        # synchronously *inside* this call for a coordinator's own
        # 'selected' revision, and deliberately never re-raises a
        # 'failed'/'rejected' outcome into open()'s caller -- an
        # already-opened engine *session* is not itself aborted over a
        # configuration refusal (see that method's own docstring). That
        # left the other half missing: nothing ever stopped the *prompt*
        # that triggered the refused apply from running anyway on whatever
        # the engine defaulted to. Re-reading this exact revision's
        # settled state right here closes that gap without touching the
        # detection logic (already correct) or the session-open behavior
        # (unchanged): 'selected'/'pending_next_use'/'active'/'indeterminate'
        # all pass through untouched (including the ordinary case of a
        # fresh task's still-unapplied 'selected' revision), only a
        # terminal 'failed'/'rejected' outcome for *this* revision blocks
        # the turn.
        self._reject_if_configuration_failed(task_id, preview.get("config_revision"))
        return session_id

    # T035 FINDING-02: the one error_code that means "this engine has no
    # configuration surface at all" (engine.session_manager
    # .EngineSessionManager.configure: ``options`` -- this session's own
    # observed configOptions -- was empty, so nothing was ever sent to the
    # wire). Pre-existing, documented, intentional behavior (see
    # tests/test_acp_setup_fresh_session.py::
    # test_rejected_configuration_never_becomes_active): there is no
    # specific requested value to have silently substituted here -- the
    # coordinator simply runs the engine's one and only mode, exactly as
    # it always would have. Every *other* 'failed'/'rejected' error_code
    # (an engine that DOES have configurable options but refused/did not
    # confirm a specific requested one -- e.g. the live
    # claude_acp_option_value_unsupported:model this fix targets) is a real
    # requested-and-refused value and must still gate the turn.
    _BENIGN_CONFIGURATION_ERROR_CODES = frozenset({"engine_does_not_support_configuration_options"})

    def _reject_if_configuration_failed(self, task_id: str, revision_id: str | None) -> None:
        """Raise a :class:`~..portable.errors.PortableError` carrying the
        engine's own refusal reason when ``revision_id`` has settled into
        'failed' or 'rejected' with a real (non-benign) error_code --
        never for any other state (see :meth:`_open_coordinator_session`'s
        call site for why 'selected' must never be gated here). The raised
        error's ``code`` is that revision's own recorded ``error_code``
        verbatim (falling back to a generic code only if none was
        recorded), so it reaches the client through the exact same
        ``data.reason`` channel ``acp.production_runtime.RouteUnresolved``
        already uses for ``no_configuration_revision``/
        ``instruction_delivery_unqualified`` -- never a silent
        substitution."""
        if not revision_id:
            return
        with self._store.connection() as db:
            row = db.execute(
                "SELECT state, error_code FROM configuration_revisions WHERE id=?", (revision_id,),
            ).fetchone()
        if row is None or row["state"] not in ("failed", "rejected"):
            return
        if row["error_code"] in self._BENIGN_CONFIGURATION_ERROR_CODES:
            return
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code=row["error_code"] or "configuration_apply_failed",
            remediation="This task's configuration revision was refused by the engine; select and apply a "
                        "working configuration before starting a new turn.",
            retry_eligible=False,
        )

    # -- T022: fresh-generation capsule/role/tool-schema delivery --------

    def _role_id_for_pending(self, task_id: str, pending: Mapping[str, Any]) -> str | None:
        """Resolve the role_id for a pending worker generation from its
        original ``delegation_requested`` event payload -- the same source
        ``recovery.capsule._section_role_and_tool_access`` reads. Returns
        ``None`` (no redelivery attempted) only if that event was somehow
        never recorded -- never guesses a role."""
        generation = pending["generation"]
        assignment_ref = generation.get("assignment_ref")
        if pending["recipient"] == "coordinator":
            return COORDINATOR_ROLE_ID
        if not assignment_ref:
            return None
        import json as _json

        with self._store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
        if row is None:
            return None
        return _json.loads(row["payload_json"]).get("role")

    def _resolve_context_window_tokens(self, task_id: str, preview: Mapping[str, Any]) -> int | None:
        """T024: the real, evidenced context-window budget for this task's
        currently selected route, when the catalog actually recorded one
        (``catalog.model.CatalogEntry.context_window_tokens`` -- see that
        module; absent for every route no discovery path has populated it
        for yet). Best-effort/never raises: any lookup failure (no route
        selected, no cached snapshot, a test double with no catalog state
        at all) falls back to :mod:`recovery.token_budget`'s labeled
        conservative default, never a guessed number."""
        try:
            from ..catalog import store as catalog_store

            selection = catalog_store.current_task_route(self._store, task_id)
            if selection is None:
                return None
            connection_id = preview.get("route_connection_id")
            if not connection_id:
                return None
            view = catalog_store.load_connection_snapshot(connection_id, selection["catalog_snapshot_id"])
            if view is None:
                return None
            for entry in view.entries:
                if entry.route.id == selection["route_id"]:
                    return entry.context_window_tokens
        except Exception:
            return None
        return None

    def _block_generation_delivery(
        self, task_id: str, session_ref: str, generation: Mapping[str, Any], capsule_revision: str,
        reason: str, message_key: str, message_params: Mapping[str, Any],
        emit_update: Callable[[str], bool] | None, language: str | None,
    ) -> bool:
        """Shared bounded-retry blocked-delivery path for both a
        ``capsule_oversize`` and a T024 ``budget_overflow`` condition (see
        :meth:`_block_generation_immediately` for the distinct, already-
        bounded ``compaction_blocked`` case): begin/re-check a delivery row
        for ``capsule_revision``, and only once :data:`MAX_DELIVERY_RETRIES` is
        reached, permanently block it *and* durably record + (when a real
        channel is available) emit the localized explicit message -- never
        silently, and never more than once per bounded-retry exhaustion.
        Returns ``True`` once permanently blocked (caller must not prompt
        the engine this turn), ``False`` while still within bounded retries
        (caller proceeds without the oversized/overflowing content this
        turn, exactly like the pre-T024 ``capsule_oversize`` path)."""
        delivery = self._engine.begin_delivery(session_ref, capsule_revision)
        if delivery is None:
            return False
        # T024: check the row's *current* retry_count directly, regardless
        # of whether ``begin_delivery`` just reopened it to ``built`` (this
        # attempt) or returned it unchanged still ``blocked`` (a caller
        # that fell through from an already-exhausted row -- see
        # ``_begin_generation_delivery``'s own state check) -- either shape
        # carries the accurate count for "how many times has this exact
        # generation/capsule_revision been retried".
        if delivery["retry_count"] >= MAX_DELIVERY_RETRIES:
            self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason, permanent=True)
            explanation = acp_messages.render(message_key, language or "en", **message_params)
            self._set_state(task_id, "blocked", wait_reason=f"limit:{reason}", wait_detail=dict(explanation=explanation))
            if emit_update is not None:
                self._emit_localized(task_id, emit_update, language or "en", message_key, **message_params)
            return True
        self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason)
        return False

    def _block_generation_immediately(
        self, task_id: str, session_ref: str, capsule_revision: str, reason: str,
        message_key: str, message_params: Mapping[str, Any],
        emit_update: Callable[[str], bool] | None, language: str | None,
    ) -> None:
        """T024: unlike :meth:`_block_generation_delivery`'s bounded-retry
        path (appropriate when redelivering the *same* content again next
        turn might legitimately differ, e.g. task state changed), a
        ``recovery.compaction_guard`` block is already the final verdict of
        its own bounded window/ratio history -- it is recorded and
        surfaced permanently on the very first sighting, never after a
        second round of otherwise-pointless retries."""
        delivery = self._engine.begin_delivery(session_ref, capsule_revision)
        if delivery is not None:
            self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason, permanent=True)
        explanation = acp_messages.render(message_key, language or "en", **message_params)
        self._set_state(task_id, "blocked", wait_reason=f"limit:{reason}", wait_detail=dict(explanation=explanation))
        if emit_update is not None:
            self._emit_localized(task_id, emit_update, language or "en", message_key, **message_params)

    def _begin_generation_delivery(
        self, task_id: str, session_ref: str, role_id: str, prompt_text: str,
        *, emit_update: Callable[[str], bool] | None = None, language: str | None = None,
    ) -> tuple[str, str | None, bool]:
        """If ``session_ref`` has a queued fresh context generation (see
        ``delegation.live_adapters.EngineSessionManagerAdapter.pending_delivery``
        -- an optional ``SessionRuntime`` extension, never required),
        prefix ``prompt_text`` with the T021 capsule plus current role
        instructions (via the T015 delivery mechanism, which also
        re-asserts current tool-schema access -- see
        ``recovery.capsule._section_role_and_tool_access``) as the first
        blocks of the continuation for that generation, and mark the
        delivery ``built``/``transmitted``.

        T024: before transmitting, plans the full mandatory budget (role
        instructions + tool schemas + capsule's critical sections + chat/
        continuation + reserves -- see ``recovery.token_budget.plan_budget``)
        against the route's real (catalog) or conservatively-estimated
        context window, and -- only for a generation whose ``cause`` is an
        actual observed/controlled compaction, never an ordinary
        session_open/session_replacement -- runs the compaction-loop guard
        (``recovery.compaction_guard.evaluate_compaction``). A ``fits``
        budget proceeds unchanged; ``needs_reference_mode`` applies the
        budget-selected compact role presentation
        (``recovery.presentation.select_presentation``) rather than
        dropping any mandatory content; ``overflow`` or a compaction-guard
        block goes through :meth:`_block_generation_delivery`'s identical
        bounded-retry/explicit-message path the pre-existing
        ``capsule_oversize`` case already uses -- never a silent loop or a
        silently truncated constraint.

        Returns ``(prompt_text, delivery_id, blocked)``. ``delivery_id`` is
        ``None`` when nothing was pending, already permanently blocked, or
        the engine offers no delivery tracking at all (every pre-T022
        fake/test session runtime). ``blocked`` is ``True`` only once this
        exact generation has just become permanently blocked -- the caller
        must end the turn without prompting the engine.
        """
        pending_delivery = getattr(self._engine, "pending_delivery", None)
        if pending_delivery is None:
            return prompt_text, None, False
        pending = pending_delivery(session_ref)
        if not pending:
            return prompt_text, None, False
        generation = pending["generation"]
        recipient = pending["recipient"]
        if recipient == "worker" and not (generation.get("assignment_ref") and pending.get("attempt_id")):
            # A worker capsule recipient requires both a recorded
            # assignment_ref and attempt_id (see
            # recovery.capsule._normalize_recipient); a generation that
            # somehow lacks either (never expected in practice -- this is
            # defense in depth) is never redelivered rather than raising.
            return prompt_text, None, False
        recipient_map: dict[str, Any] = dict(role=recipient)
        if recipient == "worker":
            recipient_map["assignment_ref"] = generation["assignment_ref"]
            recipient_map["attempt_id"] = pending["attempt_id"]
        try:
            capsule = build_capsule(self._store, task_id, recipient_map)
        except StoreError as error:
            if str(error) != "capsule_oversize":
                raise
            blocked = self._block_generation_delivery(
                task_id, session_ref, generation, f"oversize:{generation['id']}", "capsule_oversize",
                "budget_overflow", dict(
                    limit_name="capsule_bytes", context_window_tokens=0, context_window_basis="estimated",
                    message="The recovery capsule's own critical sections exceed the hard byte cap.",
                ),
                emit_update, language,
            )
            return prompt_text, None, blocked
        delivery = self._engine.begin_delivery(session_ref, capsule["capsule_revision"])
        if delivery is None:
            return prompt_text, None, False
        if delivery["state"] not in ("built", "blocked"):
            # Already transmitted/confirmed/continuation_observed for this
            # exact (recipient, generation, capsule_revision) -- redelivery
            # is idempotent, never resent.
            return prompt_text, None, False
        # T024: a still-``blocked`` row here means ``record_built`` refused
        # to reopen it -- its bounded retries (see
        # ``_block_generation_delivery``) are already exhausted. Never
        # silently short-circuit that case: fall through and recompute
        # today's decision fresh (deterministic given unchanged task state)
        # so the eventual permanent-block message stays accurate, then
        # ``_block_generation_delivery``'s own retry-count check finalizes
        # it below instead of looping forever.
        role_set = load_role(role_id)
        preview = self._config.snapshot(task_id)
        plan = qualify_delivery(
            role_set, delivery_for(preview["engine"], preview["version"]), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )
        capsule_text = render_capsule(capsule)
        if plan.qualification == DeliveryQualification.UNSUPPORTED:
            role_blocks_text = ""
        else:
            role_blocks_text = "\n\n".join(
                block.get("text", "") for block in plan.blocks if isinstance(block, dict) and block.get("type") == "text"
            )

        # -- T024: budget planning + compaction-loop guard ----------------
        from ..delegation import contracts as delegation_contracts
        from ..recovery import compaction_guard, token_budget
        from ..recovery.presentation import select_presentation

        size_estimate = capsule.get("size_estimate", {})
        critical_bytes = size_estimate.get("critical_bytes", len(capsule_text.encode("utf-8")))
        total_capsule_bytes = size_estimate.get("bytes", critical_bytes)
        reference_bytes = max(total_capsule_bytes - critical_bytes, 0)
        try:
            tool_schemas_bytes = len(json.dumps(delegation_contracts.schemas_for(recipient), sort_keys=True).encode("utf-8"))
        except StoreError:
            tool_schemas_bytes = 0
        context_window_tokens = self._resolve_context_window_tokens(task_id, preview)
        decision = token_budget.plan_budget(
            context_window_tokens=context_window_tokens,
            role_instructions_bytes=len(role_blocks_text.encode("utf-8")),
            tool_schemas_bytes=tool_schemas_bytes,
            chat_history_bytes=len(prompt_text.encode("utf-8")),
            capsule_bytes=critical_bytes,
            reference_candidate_bytes=reference_bytes,
        )
        if decision.decision == "overflow":
            blocked = self._block_generation_delivery(
                task_id, session_ref, generation, capsule["capsule_revision"], "budget_overflow",
                "budget_overflow", dict(
                    limit_name=decision.limit_name, context_window_tokens=decision.context_window_tokens,
                    context_window_basis=decision.context_window_basis, message=decision.message,
                ),
                emit_update, language,
            )
            return prompt_text, None, blocked
        if decision.decision == "needs_reference_mode" and role_blocks_text:
            # Large *evidence* (bounded non-critical capsule content, e.g. a
            # long publications/checks list) is already delivered as an
            # addressable ref by the capsule's own section builders (see
            # recovery.capsule's _MAX_LIST_ENTRIES bounding); this only
            # additionally compacts the role presentation to leave more
            # headroom, never touching a mandatory constraint/acceptance
            # criterion (see recovery.presentation module docstring).
            remaining = max(decision.context_window_tokens - (decision.mandatory_tokens - token_budget.estimate_tokens(len(role_blocks_text.encode("utf-8")))), 0)
            presentation = select_presentation(
                role_text=role_blocks_text, brief_text="", acceptance_text="", remaining_budget_tokens=remaining,
            )
            role_blocks_text = presentation.role_text

        if generation.get("cause") in ("observed_compaction", "observed_reset", "controlled_boundary_refresh"):
            last_usage = getattr(self._engine, "last_usage", None)
            usage = last_usage(session_ref) if last_usage is not None else None
            if usage and usage.get("tokens") is not None:
                before_tokens, before_basis = usage["tokens"], usage.get("basis", token_budget.ENGINE_REPORTED_BASIS)
            else:
                before_tokens, before_basis = token_budget.estimate_tokens(total_capsule_bytes), token_budget.ESTIMATED_BASIS
            usable_window = int(decision.context_window_tokens * token_budget.SAFETY_MARGIN_FRACTION)
            guard_result = compaction_guard.evaluate_compaction(
                self._store, task_id=task_id, engine_session_ref=session_ref, generation_id=generation["id"],
                before_tokens=before_tokens, before_basis=before_basis,
                after_tokens=decision.mandatory_tokens, after_basis=token_budget.ESTIMATED_BASIS,
                mandatory_tokens_after=decision.mandatory_tokens, usable_window_tokens=usable_window,
            )
            if guard_result["status"] == "blocked":
                self._block_generation_immediately(
                    task_id, session_ref, capsule["capsule_revision"], guard_result["reason"],
                    "compaction_blocked", dict(reason=guard_result["reason"], message=guard_result["message"]),
                    emit_update, language,
                )
                return prompt_text, None, True

        prefix = f"{role_blocks_text}\n\n{capsule_text}" if role_blocks_text else capsule_text
        new_prompt_text = f"{prefix}\n\n---\n\n{prompt_text}"
        self._engine.mark_delivery_transmitted(session_ref, delivery["id"])
        from .. import _fault_injection

        _fault_injection.checkpoint("t023_after_capsule_delivery_transmitted")
        return new_prompt_text, delivery["id"], False

    def _finalize_generation_delivery(self, session_ref: str, delivery_id: str | None, *, failure_kind: str | None, stop_reason: StopReason | None) -> None:
        if delivery_id is None:
            return
        if failure_kind is not None:
            # T022: receipt timeout / transport failure -- the row stays
            # 'transmitted' with uncertainty recorded, never promoted to
            # 'delivery_confirmed' on a guess.
            self._engine.mark_delivery_uncertain(session_ref, delivery_id, f"engine_turn_failed:{failure_kind}")
            return
        if stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
            # T022: immediate overflow right after delivery -- explicit
            # blocked state with a reason; bounded retries happen on the
            # next ordinary continuation (see _begin_generation_delivery /
            # delegation.deliveries.MAX_DELIVERY_RETRIES), never a loop
            # within this same turn.
            self._engine.mark_delivery_blocked(session_ref, delivery_id, f"engine_limit_after_delivery:{stop_reason.value}")
            return
        self._engine.mark_delivery_confirmed(session_ref, delivery_id)

    # -- main loop ----------------------------------------------------

    def _run(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
        host_elicitor: HostElicitor | None = None,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        # T020: an urgent /cortex-stop takes precedence over everything --
        # a stopped task never resumes through an ordinary prompt/steering/
        # answer. This message is durably recorded (never silently dropped)
        # but pre-marked superseded so it can never be claimed/delivered.
        if acp_steering.is_stopped(self._store, task_id):
            acp_steering.record_post_stop_message(
                self._store, task_id, source_origin or session_id, source_event_id or new_id("post_"), content,
            )
            language = self._resolve_language(task_id, content_hint=content)
            emit_update(acp_messages.render("task_stopped", language))
            return StopReason.END_TURN
        # T020: a task_orchestration row already existing *before* this call
        # opens/reuses the coordinator session means this is a follow-up
        # prompt on an already-started task -- steering-eligible (see
        # below), as opposed to the task's very first prompt.
        pre_existing = acp_steering.orchestration_exists(self._store, task_id)
        channel, session_ref = self._ensure_coordinator(task_id)
        recovery_notice = self._pending_recovery_notice.pop(task_id, None)
        if recovery_notice is not None:
            notice_language = self._resolve_language(task_id, content_hint=content)
            if recovery_notice.get("route_changed"):
                self._emit_localized(
                    task_id, emit_update, notice_language, "coordinator_recovery_new_engine",
                    engine=recovery_notice.get("engine") or "?",
                )
            else:
                self._emit_localized(task_id, emit_update, notice_language, "coordinator_recovery_new_session")
        # T020: route this prompt against any still-open coordinator
        # question *before* recording it as an ordinary user_message/
        # steering turn -- see docs/acp/steering-and-questions.md's
        # "question lifecycle" section. Two or more open questions is never
        # guessed at: the user is asked to disambiguate with
        # /cortex-answer, and this prompt's text is not itself recorded as
        # an answer or as steering.
        open_questions = questions_store.list_open(self._store, task_id)
        language = self._resolve_language(task_id, content_hint=content)
        if len(open_questions) >= 2:
            listing = "\n".join(f"{index + 1}. {q.text}" for index, q in enumerate(open_questions))
            emit_update(acp_messages.render("question_ambiguous", language, questions=listing))
            return StopReason.END_TURN
        answering_question = open_questions[0] if len(open_questions) == 1 else None
        # T017: a new user prompt is a fresh turn -- surface any explanation
        # a prior blocked/limited/refused turn left behind (resumable, not
        # repeated once delivered), and reset the durable no-progress
        # counter for this fresh attempt (see delegation.events_store's
        # T017 column docstring).
        self._emit_resume_context(task_id)
        # T018: recover any source_events durably captured (bootstrap.
        # bootstrap_task/append_source_event) but never recorded as a
        # user_message task_events row -- a crash between capture and
        # recording in a prior process/attempt -- *before* this call's own
        # message, in captured order, each labeled 'recovered: true'. Never
        # creates a task/assignment; a source event already recorded (the
        # normal case) is a no-op here.
        self._recover_unrecorded_source_events(task_id, exclude=(source_origin, source_event_id))
        from . import replay_store

        if source_origin is not None and source_event_id is not None:
            dedup_key = f"user_message:{source_origin}:{source_event_id}"
            ledger_origin, ledger_event_id = source_origin, source_event_id
        else:
            with self._store.connection() as db:
                turn_index = db.execute(
                    "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND event_type='user_message' AND origin=?",
                    (task_id, session_id),
                ).fetchone()["n"]
            dedup_key = f"user_message:{session_id}:{turn_index}"
            # No real source identity was threaded through (a caller not
            # yet updated for T018) -- nothing to mark in the delivery
            # ledger, but still recorded through the same atomic path.
            ledger_origin, ledger_event_id = None, None
        replay_store.record_user_message_event(
            self._store, task_id, dedup_key, session_id, dict(content=content, session_id=session_id),
            ledger_origin=ledger_origin, ledger_event_id=ledger_event_id,
        )
        # T020: this exact prompt is either (a) the answer to the single
        # open question, (b) steering on an already-active task, or (c) the
        # task's ordinary first turn -- never more than one of these for
        # the same prompt. Recorded *after* the durable user_message above
        # (itself already persisted before any delivery -- see the T018
        # dedup_key/record_user_message_event call above) so "steering
        # persists before delivery" holds for both the coalescing text and
        # this extra bookkeeping event.
        if answering_question is not None:
            questions_store.answer(self._store, answering_question.id, content, "text")
            delegation_events.append_event(
                self._store, task_id, "progress", f"question_answered:{answering_question.id}",
                dict(kind="question_answered", question_id=answering_question.id, answer_text=content),
                origin="runtime",
            )
        elif pre_existing:
            acp_steering.record_steering(
                self._store, task_id, source_origin or session_id, source_event_id or dedup_key, content,
            )
        return self._continuation_loop(
            task_id, session_id, channel, session_ref, content, emit_update, cancel_event, language, host_elicitor,
            emit_tool_call=emit_tool_call,
        )

    def _continuation_loop(
        self, task_id, session_id, channel, session_ref, content, emit_update, cancel_event, language,
        host_elicitor: HostElicitor | None = None, *,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        """The coordinator continuation loop proper: claim/coalesce/prompt/
        process, repeated until a terminal outcome. Factored out of
        :meth:`_run` (T020) so :meth:`answer_question` -- an out-of-band
        ``/cortex-answer`` delivery that must never re-run the open-question
        gating or steering/first-turn detection in :meth:`_run`'s preamble
        (its own answer event is already durably recorded by the caller) --
        can drive the exact same loop starting from an already-ensured
        coordinator session."""
        no_progress = 0
        total = 0
        first = True
        while True:
            if cancel_event.is_set():
                if acp_steering.is_stopped(self._store, task_id):
                    # T020: an urgent /cortex-stop (possibly issued from a
                    # *different* session/thread in this same process --
                    # see stop_task(), which looks this exact cancel_event
                    # up via self._active_cancel_events and sets it) always
                    # takes precedence: stop_task() already set the
                    # authoritative "stopped" state and is cancelling every
                    # assignment itself -- this loop just exits without
                    # overwriting that state or delivering anything further.
                    return StopReason.CANCELLED
                # T020: session/cancel interrupts *this coordination turn*
                # only; it never cancels/terminates an already-*running*
                # assignment (that would need SessionRuntime.terminate_owned/
                # cancel_attempt, never called here -- see stop_task() for
                # the urgent, everything-cancelling /cortex-stop path
                # instead) and the task stays active ("interrupted", not
                # "cancelled"): worker events keep accumulating durably and
                # the next ordinary prompt delivers them coalesced with any
                # steering in one continuation (see
                # docs/acp/steering-and-questions.md). The one exception
                # (T017, pre-existing, unchanged by this task): a worker
                # still only *queued* for capacity (its engine session
                # never actually opened yet) must still observe task
                # cancellation promptly rather than leak that reservation
                # forever -- cancel_task() only affects that queued-open
                # wait loop (delegation.capacity/EngineSessionManagerAdapter),
                # never an already-opened session's in-flight prompt.
                cancel_task = getattr(self._engine, "cancel_task", None)
                if cancel_task is not None:
                    cancel_task(task_id)
                self._set_state(
                    task_id, "interrupted", wait_reason="interrupted",
                    wait_detail=dict(explanation="Coordination turn interrupted by session/cancel; already-running workers keep running."),
                )
                return StopReason.CANCELLED
            total += 1
            if total > self._max_total:
                self._emit_localized(task_id, emit_update, language, "max_total_continuations")
                self._set_state(task_id, "blocked", wait_reason="limit:max_total_continuations")
                return StopReason.END_TURN
            continuation_id = new_id("cont_")
            claimed = delegation_events.claim_unconsumed(self._store, task_id, continuation_id)
            # T019: capture this batch's worker_result events' observed
            # artifact revision now, at the moment this runtime processes
            # them (see delegation.revision module docstring on the shared-
            # workspace-root limitation). Never blocks/fails the turn: an
            # unavailable capture is recorded and labeled "unknown", not
            # refused (refusal is the completion gate's job, at acceptance
            # time -- see delegation.completion.gate).
            revisions = self._capture_result_revisions(task_id, claimed)
            # T035: one ACP tool-call card per delegated assignment,
            # queued/running/finished/failed/cancelled -- see
            # _emit_assignment_tool_calls. Runs over the FULL claimed batch
            # (delegation_requested/assignment_ready are informative-only
            # for the coordinator's own continuation prompt below -- see
            # MEANINGFUL_EVENT_TYPES -- but are exactly the "queued"/
            # "running" transitions this card lifecycle needs).
            self._emit_assignment_tool_calls(task_id, emit_tool_call, claimed)
            meaningful = [e for e in claimed if e.event_type in delegation_events.MEANINGFUL_EVENT_TYPES]
            # FINDING 11: a refused delegation must never be silently
            # absorbed into "the coordinator did it all itself" -- surfaced
            # above as a failed tool-call card (T035) whenever a card
            # channel is wired; the plain-text line below is kept ONLY as
            # the fallback for a caller that never wired emit_tool_call at
            # all (no pre-T035 caller/test does) -- a card and this same
            # text line together would just duplicate the refusal, so the
            # text only fires when there is no card to carry it.
            if emit_tool_call is None:
                for event in meaningful:
                    if event.event_type == "assignment_failed":
                        self._emit_localized(
                            task_id, emit_update, language, "assignment_delegation_failed",
                            assignment_ref=event.assignment_ref or "?",
                            error=event.payload.get("error", "unknown"),
                        )
            if not meaningful and not first:
                # T020 (stop-race fix): if a /cortex-stop landed in this
                # exact window (this batch was claimed but was never going
                # to be delivered anyway, since it carries no meaningful
                # event), release it and supersede any steering rather than
                # leaving it claimed-but-undelivered forever, and exit
                # without ever reaching another engine.prompt call. Never
                # released in the ordinary (not-stopped) case -- that would
                # change the pre-existing no-progress-budget timing (an
                # informative-only claimed batch intentionally stays
                # claimed, never redelivered, until a later meaningful
                # event's own claim naturally supersedes/consumes it).
                if acp_steering.is_stopped(self._store, task_id):
                    if claimed:
                        delegation_events.release_claim(self._store, task_id, continuation_id)
                    acp_steering.supersede_undelivered_steering(self._store, task_id)
                    return StopReason.CANCELLED
                progressed = self._wait_for_progress(task_id, cancel_event)
                if not progressed:
                    no_progress += 1
                    self._set_state(task_id, "blocked", wait_reason="workers_running", no_progress_count=no_progress)
                    if no_progress >= self._max_no_progress:
                        explanation = acp_messages.render("blocked_no_progress", language)
                        self._emit_localized(task_id, emit_update, language, "blocked_no_progress")
                        self._set_state(
                            task_id, "blocked", wait_reason="blocked",
                            wait_detail=dict(explanation=explanation), no_progress_count=no_progress,
                        )
                        return StopReason.END_TURN
                continue
            first = False
            no_progress = 0
            # T020 (stop-race fix): re-check immediately before ever calling
            # the coordinator's engine again -- a /cortex-stop that lands
            # between this iteration's own top-of-loop check and here (for
            # example while claim_unconsumed/_capture_result_revisions ran)
            # must still prevent one more engine.prompt call and must never
            # let a just-claimed-but-undelivered steering batch reach the
            # coordinator. The claim is released (not lost -- a stopped
            # task's undelivered events stay superseded/inert, never
            # resurrected) and any steering in this exact batch is marked
            # superseded the same way stop_task() marks it for anything it
            # was still unclaimed.
            if acp_steering.is_stopped(self._store, task_id):
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                acp_steering.supersede_undelivered_steering(self._store, task_id)
                return StopReason.CANCELLED
            prompt_text = _coalesce(claimed, revisions) if claimed else content
            prompt_text, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                task_id, session_ref, COORDINATOR_ROLE_ID, prompt_text,
                emit_update=emit_update, language=language,
            )
            if delivery_blocked:
                # T024: a permanent budget-overflow/compaction-loop block --
                # the explicit localized message and blocked task state were
                # already recorded/emitted inside _begin_generation_delivery;
                # never prompt the engine with an oversized/looping turn.
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                return StopReason.END_TURN
            # T035 FINDING-12 closure: the engine-independent backstop.
            # ``engine.permissions.PermissionBridge`` refuses a coordinator's
            # mutating *intent* whenever the engine actually asks -- but
            # that still depends on the engine choosing to ask at all (see
            # that module's own HONEST BOUNDARY). This is the part that
            # does not depend on the engine's goodwill: capture the shared
            # project workspace's observed tree digest immediately before
            # and immediately after this one coordinator turn and fail the
            # turn, visibly, if it moved -- detection, never prevention (the
            # write already happened by the time this runs), but it turns a
            # silent violation into a recorded, blocked task state instead
            # of nothing at all. See ``delegation.revision.workspace_changed``'s
            # own docstring for exactly what is compared and what it costs
            # (two full capture_revision walks of the workspace per turn).
            pre_turn_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
            try:
                outcome = self._engine.prompt(session_ref, prompt_text, cancel_event)
                from .. import _fault_injection

                _fault_injection.checkpoint("t023_after_continuation_prompt")
            except EngineTurnFailed as failure:
                # T017: an engine failure on the coordinator's own turn is
                # never silently treated as end_turn, and never rerouted to
                # a second engine/connection -- record it durably and stop
                # with an honest blocker instead of a fabricated success.
                self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                explanation = acp_messages.render("engine_failure", language, kind=failure.kind)
                self._emit_localized(task_id, emit_update, language, "engine_failure", kind=failure.kind)
                self._set_state(
                    task_id, "blocked", wait_reason=f"engine_failure:{failure.kind}",
                    wait_detail=dict(explanation=explanation),
                )
                return StopReason.END_TURN
            except BaseException:
                # T016 review (MEDIUM): a raise from engine.prompt() means
                # these claimed events were never actually delivered to the
                # coordinator model. Release the claim (consumed_by -> NULL)
                # so the next continuation reclaims and delivers them
                # exactly once, instead of silently losing them, then
                # propagate -- this turn genuinely failed.
                self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind="unexpected_exception", stop_reason=None)
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                raise
            self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
            if claimed:
                delegation_events.mark_delivered(self._store, task_id, continuation_id)
            post_turn_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
            if delegation_revision.workspace_changed(pre_turn_revision, post_turn_revision):
                # T035 FINDING-12 closure: the coordinator's own turn just
                # changed (or could not prove it did not change) the shared
                # project workspace -- refused as a blocking failure of this
                # turn, never silently accepted and never treated as if the
                # turn had ended normally (see this loop's own comment above
                # the capture that made ``pre_turn_revision``).
                self._emit_localized(task_id, emit_update, language, "coordinator_turn_mutated_workspace")
                self._set_state(
                    task_id, "blocked", wait_reason="coordinator_turn_mutated_workspace",
                    wait_detail=dict(explanation=acp_messages.render("coordinator_turn_mutated_workspace", language)),
                )
                return StopReason.END_TURN
            if outcome.text:
                emit_update(outcome.text)
            if outcome.stop_reason == StopReason.REFUSAL:
                # Never re-route to another engine/connection/model on
                # refusal -- persist and hand the refusal straight to the host.
                self._set_state(
                    task_id, "refused",
                    wait_reason="refused", wait_detail=dict(explanation=outcome.text or "Coordinator refused this turn."),
                )
                return StopReason.REFUSAL
            self._process_new_events(task_id, channel, cancel_event)
            terminal = self._check_terminal_decision(task_id)
            if terminal == "accepted":
                self._set_state(task_id, "accepted")
                return StopReason.END_TURN
            if isinstance(terminal, dict) and terminal.get("kind") == "ask_user":
                # T020: persist-then-try-elicitation-then-fallback -- see
                # _handle_ask_user. A resolved question (elicitation accept)
                # continues the SAME turn with the answer coalesced into the
                # next continuation; an unresolved one ends the turn with
                # the plain question (existing pre-T020 behavior).
                resolved = self._handle_ask_user(task_id, session_id, emit_update, language, terminal, host_elicitor)
                if resolved:
                    no_progress = 0
                    continue
                self._set_state(
                    task_id, "waiting_for_user", wait_reason="user_input_required",
                    wait_detail=dict(question=terminal["question"], pending_question_id=terminal["question_id"]),
                )
                return StopReason.END_TURN
            if outcome.stop_reason == StopReason.CANCELLED:
                self._set_state(task_id, "cancelled")
                return StopReason.CANCELLED
            has_unconsumed = delegation_events.has_unconsumed(self._store, task_id)
            running = bool(self._running.get(task_id))
            if outcome.stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
                # T017: a token/turn-request limit counts toward the budget
                # and is recorded, but only continues the loop when a
                # meaningful event or a known unfinished action (pending
                # events, a running worker) actually exists; otherwise this
                # is an honest limit blocker, never a fabricated idle wait.
                kind = outcome.stop_reason.value
                self._emit_localized(task_id, emit_update, language, "coordinator_turn_limit", kind=kind)
                if not has_unconsumed and not running:
                    explanation = acp_messages.render("blocked_limit", language, kind=kind)
                    self._set_state(
                        task_id, "blocked", wait_reason=f"limit:{kind}",
                        wait_detail=dict(explanation=explanation),
                    )
                    return StopReason.END_TURN
                continue
            if not has_unconsumed and not running:
                self._set_state(task_id, "idle_awaiting_user", wait_reason="idle_awaiting_user")
                return StopReason.END_TURN
            # else: loop again -- either events already pending or we will
            # wait for progress at the top of the next iteration.

    # -- /cortex-answer / /cortex-stop entrypoints (T020) ----------------

    def answer_question(
        self, task_id: str, session_id: str, question: questions_store.PendingQuestion, text: str,
        emit_update: Callable[[str], bool], cancel_event: threading.Event, *,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        """Deliver an explicit ``/cortex-answer`` (see ``acp.commands``) for
        one specific, already-disambiguated open ``question`` -- used when
        two or more questions are open and an ordinary prompt cannot be
        auto-matched to one of them (see ``_run``'s ``question_ambiguous``
        path). ``question`` must be one this exact task's own still-``open``
        row (the caller -- ``acp.agent`` -- resolves the 1-based index the
        user typed against ``questions_store.list_open`` immediately before
        calling this).

        A stopped task's late answer is recorded and inert, exactly like an
        ordinary prompt after ``/cortex-stop`` (see ``_run``)."""
        if acp_steering.is_stopped(self._store, task_id):
            acp_steering.record_post_stop_message(self._store, task_id, session_id, new_id("post_"), text)
            language = self._resolve_language(task_id, content_hint=text)
            emit_update(acp_messages.render("task_stopped", language))
            return StopReason.END_TURN
        if not questions_store.answer(self._store, question.id, text, "text"):
            # Resolved/superseded/cancelled between the caller's own
            # list_open() read and this call (a genuine race, or a stale
            # /cortex-answer against an already-answered question) -- a
            # late answer, recorded and otherwise inert.
            questions_store.record_late_answer(self._store, question.id, task_id, text, "text")
            language = self._resolve_language(task_id, content_hint=text)
            emit_update(acp_messages.render("question_answer_unknown_index", language))
            return StopReason.END_TURN
        channel, session_ref = self._ensure_coordinator(task_id)
        language = self._resolve_language(task_id, content_hint=text)
        recovery_notice = self._pending_recovery_notice.pop(task_id, None)
        if recovery_notice is not None:
            if recovery_notice.get("route_changed"):
                self._emit_localized(
                    task_id, emit_update, language, "coordinator_recovery_new_engine",
                    engine=recovery_notice.get("engine") or "?",
                )
            else:
                self._emit_localized(task_id, emit_update, language, "coordinator_recovery_new_session")
        delegation_events.append_event(
            self._store, task_id, "progress", f"question_answered:{question.id}",
            dict(kind="question_answered", question_id=question.id, answer_text=text), origin="runtime",
        )
        # A user_message (meaningful) event, distinct from the informative
        # progress row above, so the continuation loop's claim/coalesce
        # gating (delegation_events.MEANINGFUL_EVENT_TYPES) actually wakes
        # for it -- exactly like an ordinary prompt's own user_message.
        delegation_events.append_event(
            self._store, task_id, "user_message", f"cortex_answer:{question.id}",
            dict(content=f"Answered question ({question.text!r}): {text}", session_id=session_id),
            origin=session_id,
        )
        with self._lock:
            self._active_cancel_events[task_id] = cancel_event
        try:
            return self._continuation_loop(
                task_id, session_id, channel, session_ref, text, emit_update, cancel_event, language,
                emit_tool_call=emit_tool_call,
            )
        finally:
            with self._lock:
                if self._active_cancel_events.get(task_id) is cancel_event:
                    del self._active_cancel_events[task_id]

    def stop_task(self, task_id: str, session_id: str, emit_update: Callable[[str], bool]) -> bool:
        """T020 ``/cortex-stop``: an urgent safety stop, routed through
        cancellation -- never a normal steering queue entry. Takes
        precedence over an in-flight coordinator loop for this same task
        *in this process*: signals that loop's own cancel_event (see
        ``self._active_cancel_events``/``start_prompt``/``answer_question``)
        so it exits promptly without delivering queued steering or starting
        a new delegation (see ``_continuation_loop``'s stopped-state
        checks), then independently cancels every non-terminal assignment
        (and any capacity-queued open, via ``engine.cancel_task``),
        invalidates every still-open pending question, and marks any
        not-yet-delivered steering event superseded so it can never be
        delivered after this point (see
        ``acp.steering.supersede_undelivered_steering``). Idempotent: a
        second call on an already-stopped task is a no-op (returns
        ``False``) rather than a second cancellation wave.

        Raises :class:`PortableError` (code ``unknown_task``) for a
        ``task_id`` that does not exist at all -- never a false "stopped"
        confirmation for a task that was never real (T020 fresh-review
        fix)."""
        try:
            transitioned = acp_steering.mark_stopped(self._store, task_id)
        except LookupError:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="unknown_task",
                remediation="This task id does not exist; there is nothing to stop.",
                retry_eligible=False,
            ) from None
        if not transitioned:
            return False
        with self._lock:
            active_event = self._active_cancel_events.get(task_id)
        if active_event is not None:
            active_event.set()
        cancel_task = getattr(self._engine, "cancel_task", None)
        if cancel_task is not None:
            try:
                cancel_task(task_id)
            except Exception:
                pass
        with self._store.connection() as db:
            open_refs = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested'",
                (task_id,),
            ).fetchall()
            terminal_refs = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? "
                "AND event_type IN ('worker_result','assignment_failed','assignment_cancelled')",
                (task_id,),
            ).fetchall()
        terminal = {row["assignment_ref"] for row in terminal_refs}
        for row in open_refs:
            assignment_ref = row["assignment_ref"]
            if assignment_ref in terminal:
                continue
            self._cancel_assignment(task_id, assignment_ref, "urgent stop (/cortex-stop)")
            questions_store.cancel_for_assignment(self._store, task_id, assignment_ref, "task stopped")
        questions_store.supersede_open(self._store, task_id, "task stopped")
        acp_steering.supersede_undelivered_steering(self._store, task_id)
        self._set_state(
            task_id, "stopped", wait_reason="stopped",
            wait_detail=dict(explanation="Stopped by /cortex-stop."),
        )
        language = self._resolve_language(task_id)
        emit_update(acp_messages.render("stop_confirmed", language))
        return True

    def _recover_unrecorded_source_events(self, task_id: str, *, exclude: tuple[str | None, str | None] = (None, None)) -> None:
        from . import replay_store

        for entry in replay_store.find_unrecorded_source_events(self._store, task_id):
            if (entry["origin"], entry["event_id"]) == exclude:
                # This call's own just-captured source event (see
                # acp.agent, which durably captures before invoking
                # start_prompt): recorded below as this turn's normal,
                # non-recovered user_message -- never swept in here as a
                # false 'recovered' label.
                continue
            if entry["text"] is None:
                # The durably published body could not be resolved (should
                # not happen for a source event this runtime itself will
                # ever be asked to continue from); skip rather than record a
                # fabricated empty user_message.
                continue
            replay_store.record_user_message_event(
                self._store, task_id, f"user_message:{entry['origin']}:{entry['event_id']}", entry["origin"],
                dict(content=entry["text"], session_id=entry["origin"], recovered=True),
                ledger_origin=entry["origin"], ledger_event_id=entry["event_id"],
            )

    def _emit_resume_context(self, task_id: str) -> None:
        with self._store.connection() as db:
            row = db.execute(
                "SELECT wait_reason, wait_detail_json FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()
        # T020: "interrupted" (session/cancel) needs no resume-context echo
        # -- the interrupted turn's own accumulated events are already
        # delivered coalesced by the normal claim/coalesce path on the next
        # continuation; a resume_context line here would just be noise.
        if row is None or not row["wait_reason"] or row["wait_reason"] in ("workers_running", "idle_awaiting_user", "interrupted"):
            return
        import json

        detail = json.loads(row["wait_detail_json"]) if row["wait_detail_json"] else {}
        explanation = detail.get("explanation") or f"Previously waiting: {row['wait_reason']}."
        delegation_events.append_event(
            self._store, task_id, "progress", f"resume_context:{new_id('rc_')}",
            dict(note=f"Resuming after a prior wait ({row['wait_reason']}): {explanation}"),
            origin="runtime",
        )
        self._set_state(task_id, "running", wait_reason=None, wait_detail=None, no_progress_count=0)

    def _resolve_language(self, task_id: str, *, content_hint: str | None = None) -> str:
        explicit = acp_steering.get_language_override(self._store, task_id)
        texts = acp_steering.recent_user_direct_texts(self._store, task_id)
        if content_hint:
            texts = list(texts) + [content_hint]
        return acp_messages.select_language(explicit, texts)

    def _set_state(
        self, task_id: str, state: str, *, wait_reason: str | None | object = _UNSET,
        wait_detail: Mapping[str, Any] | None | object = _UNSET, no_progress_count: int | None = None,
    ) -> None:
        import json

        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                if wait_reason is _UNSET and wait_detail is _UNSET and no_progress_count is None:
                    db.execute(
                        "UPDATE task_orchestration SET state=?, updated_at=? WHERE task_id=?",
                        (state, _now(), task_id),
                    )
                else:
                    set_clauses = ["state=?", "updated_at=?"]
                    params: list[Any] = [state, _now()]
                    if wait_reason is not _UNSET:
                        set_clauses.append("wait_reason=?")
                        params.append(wait_reason)
                    if wait_detail is not _UNSET:
                        set_clauses.append("wait_detail_json=?")
                        params.append(json.dumps(dict(wait_detail), sort_keys=True, ensure_ascii=False) if wait_detail else None)
                    if no_progress_count is not None:
                        set_clauses.append("no_progress_count=?")
                        params.append(no_progress_count)
                    params.append(task_id)
                    db.execute(
                        f"UPDATE task_orchestration SET {', '.join(set_clauses)} WHERE task_id=?", params,
                    )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _emit_and_record(self, task_id: str, emit_update: Callable[[str], bool], text: str) -> None:
        # T020: never repeat the exact same status line back to back (same
        # task, same text) -- see acp.messages.Dedup. Still recorded
        # durably every time it is *called* (the durable event log is never
        # deduplicated by this guard, only the host-visible chunk is), so
        # nothing is lost from the audit trail, only from the live stream.
        if self._status_dedup.should_emit(task_id, "text", dict(text=text)):
            emit_update(text)
        delegation_events.append_event(
            self._store, task_id, "progress", f"progress:{new_id('p_')}", dict(note=text), origin="runtime",
        )

    def _emit_localized(self, task_id: str, emit_update: Callable[[str], bool], language: str, key: str, **params) -> None:
        """Like :meth:`_emit_and_record` but selects EN/RU text from
        ``acp.messages`` and de-duplicates by ``(key, params)`` -- not the
        rendered text -- so a dedup decision never depends on which
        language happened to be selected."""
        if self._status_dedup.should_emit(task_id, key, params):
            emit_update(acp_messages.render(key, language, **params))
        delegation_events.append_event(
            self._store, task_id, "progress", f"progress:{new_id('p_')}",
            dict(note=acp_messages.render(key, language, **params), message_key=key, message_params=params),
            origin="runtime",
        )

    # -- T035: worker-assignment ACP tool-call cards ----------------------

    def _delegation_requested_payload(self, task_id: str, assignment_ref: str) -> Mapping[str, Any] | None:
        import json as _json

        with self._store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
        return _json.loads(row["payload_json"]) if row is not None else None

    def _delegation_provisioning_role(self, task_id: str, assignment_ref: str) -> str | None:
        """Best-effort resolved role id for a delegation, once
        ``ProvisioningService.delegate`` has actually resolved and recorded
        one (``delegation_provisioning.role_id`` -- see that module).
        ``None`` before provisioning has run that far (e.g. an unknown-role
        delegation refused before ``resolve_role`` ever succeeded, so no
        row was ever inserted) -- the caller then keeps the raw role hint
        it already has instead of guessing a resolved id."""
        with self._store.connection() as db:
            row = db.execute(
                "SELECT role_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (task_id, assignment_ref),
            ).fetchone()
        return row["role_id"] if row is not None else None

    def _assignment_ref_for_attempt(self, task_id: str, attempt_id: str) -> str | None:
        """``worker_result`` events are keyed by the worker's own attempt id
        (``channel.attempt_id`` -- see
        ``delegation.mcp_tools._worker_publish_result``), a different id
        space from the coordinator-facing delegation ``assignment_ref``
        (the idempotency key) every other assignment lifecycle event uses;
        resolves attempt id back to that same delegation ref so a
        ``worker_result`` updates the exact same card its
        ``delegation_requested`` created, never a second, unrelated one."""
        with self._store.connection() as db:
            row = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?",
                (task_id, attempt_id),
            ).fetchone()
        return row["idempotency_key"] if row is not None else None

    def _emit_assignment_tool_calls(
        self, task_id: str, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None, claimed,
    ) -> None:
        """T035: render each delegated assignment's lifecycle as one ACP
        ToolCall card (``acp.protocol.tool_call``/``tool_call_update``),
        created once and updated in place -- from the SAME claimed batch
        :meth:`_continuation_loop` already reads for the coordinator's own
        continuation prompt (``claim_unconsumed`` claims every event type,
        not only ``delegation_events.MEANINGFUL_EVENT_TYPES`` -- see that
        function's docstring), so this observes ``delegation_requested``/
        ``assignment_ready`` exactly once each, the same instant the
        coordinator continuation loop itself does.

        A cancelled assignment is rendered ``failed`` (the pinned
        ``ToolCallStatus`` schema has no ``cancelled`` value -- see the
        module-level docstring near ``_TOOL_CALL_STATUS_FOR_CANCELLED``)
        with the cancellation reason carried in its content, so it stays
        distinguishable from an ordinary failure by its text, never by a
        status a client cannot render.

        A no-op entirely when ``emit_tool_call`` is ``None`` (see
        :meth:`start_prompt`'s docstring) -- every pre-T035 caller/test."""
        if emit_tool_call is None:
            return
        for event in claimed:
            if event.event_type == "delegation_requested":
                assignment_ref = event.assignment_ref
                if not assignment_ref:
                    continue
                role_hint = event.payload.get("role", "?")
                work_title = event.payload.get("title", "")
                emit_tool_call(dict(
                    new=True, tool_call_id=_worker_tool_call_id(assignment_ref),
                    title=f"Worker: {role_hint} — {work_title}",
                    kind=_WORKER_TOOL_CALL_KIND, status="pending",
                    text=scrub_text(f"Queued: {event.payload.get('brief', '')}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                ))
            elif event.event_type == "assignment_ready" and event.assignment_ref:
                assignment_ref = event.assignment_ref
                fields: dict[str, Any] = dict(
                    new=False, tool_call_id=_worker_tool_call_id(assignment_ref), status="in_progress",
                )
                role_id = self._delegation_provisioning_role(task_id, assignment_ref)
                if role_id:
                    requested = self._delegation_requested_payload(task_id, assignment_ref)
                    if requested is not None:
                        # Requirement: the title names the *resolved* role
                        # id, not the raw hint the coordinator typed --
                        # this is the first point a resolved id exists.
                        fields["title"] = f"Worker: {role_id} — {requested.get('title', '')}"
                emit_tool_call(fields)
            elif event.event_type == "worker_result" and event.assignment_ref:
                assignment_ref = self._assignment_ref_for_attempt(task_id, event.assignment_ref)
                if assignment_ref is None:
                    continue
                checks = event.payload.get("checks") or []
                lines = [event.payload.get("summary", "")]
                if checks:
                    rendered = ", ".join(
                        f"{c.get('name')}={c.get('outcome')} ({c.get('evidence', 'claimed')})" for c in checks
                    )
                    lines.append(f"Checks: {rendered}.")
                report_ref = event.payload.get("report_ref")
                if report_ref:
                    lines.append(f"Report: {report_ref}")
                emit_tool_call(dict(
                    new=False, tool_call_id=_worker_tool_call_id(assignment_ref), status="completed",
                    text=scrub_text("\n".join(lines), max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                ))
            elif event.event_type == "assignment_failed" and event.assignment_ref:
                text = f"Not delegated/failed: {event.payload.get('error', 'unknown error')}"
                detail = event.payload.get("detail")
                if detail:
                    text += f" {detail}"
                emit_tool_call(dict(
                    new=False, tool_call_id=_worker_tool_call_id(event.assignment_ref), status="failed",
                    text=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                ))
            elif event.event_type == "assignment_cancelled" and event.assignment_ref:
                text = f"Cancelled: {event.payload.get('reason', '')}"
                emit_tool_call(dict(
                    new=False, tool_call_id=_worker_tool_call_id(event.assignment_ref),
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    text=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                ))

    def _handle_ask_user(
        self, task_id: str, session_id: str, emit_update: Callable[[str], bool], language: str, decision: dict,
        elicitor: HostElicitor | None,
    ) -> bool:
        """T020: persist the pending question *before* any host contact,
        then try structured elicitation (only when a ``HostElicitor`` was
        injected -- see the class docstring), falling back to the plain
        text question otherwise. Returns ``True`` iff an answer was
        captured (the caller then continues the SAME turn instead of
        ending it), ``False`` otherwise."""
        question = questions_store.create_if_absent(
            self._store, task_id, f"ask_user:{decision['decision_id']}", decision["question"],
            options=decision.get("options"),
        )
        decision["question_id"] = question.id
        if question.status != "open":
            # Already resolved/superseded by an earlier call for this exact
            # decision (crash-recovery re-check) -- treat as unresolved
            # here; the caller's normal terminal-decision recheck on the
            # next loop iteration observes the real state.
            return False
        if elicitor is not None:
            try:
                answer_text = elicitor.elicit(session_id, decision["question"], decision.get("options"))
            except Exception:
                answer_text = None
            if answer_text is not None:
                if questions_store.answer(self._store, question.id, answer_text, "elicitation"):
                    delegation_events.append_event(
                        self._store, task_id, "progress", f"question_answered:{question.id}",
                        dict(kind="question_answered", question_id=question.id, answer_text=answer_text),
                        origin="runtime",
                    )
                    # A user_message (meaningful) event, distinct from the
                    # informative progress row above, so the continuation
                    # loop's claim/coalesce gating
                    # (delegation_events.MEANINGFUL_EVENT_TYPES) actually
                    # delivers this answer to the coordinator on the next
                    # iteration -- exactly like acp.orchestrating_runtime
                    # .answer_question's own /cortex-answer text-fallback
                    # delivery.
                    delegation_events.append_event(
                        self._store, task_id, "user_message", f"elicitation_answer:{question.id}",
                        dict(content=f"Answered question ({question.text!r}) via elicitation: {answer_text}", session_id=session_id),
                        origin=session_id,
                    )
                    return True
        emit_update(decision["question"])
        return False

    def _capture_result_revisions(self, task_id: str, claimed) -> dict[int, dict]:
        """T019: for every ``worker_result`` in ``claimed``, durably capture
        (and return, keyed by event id) the observed artifact revision at
        this exact instant, bound to that assignment's attempt id (see
        ``delegation.revision``). A task with no resolvable project root
        (should not happen for a bound task) yields no captures rather than
        raising.

        T025: when this assignment's attempt actually ran in an isolated
        snapshot workspace (see ``delegation.workspace_isolation``), the
        revision captured here is that workspace's own tree -- not the
        shared project root's -- because that is the tree the worker's
        checks actually ran against. Since the tree digest is computed over
        *relative* paths/content (identical either way), a later successful
        ``delegation.integration.integrate`` of that exact tree onto an
        unchanged project root reproduces the same digest (still bound,
        never stale); an integration that merges in a concurrent main-root
        change, or a conflicting worker, necessarily changes the digest --
        the existing ``stale_verification`` comparison below then correctly
        refuses to let a revision-A report certify revision B. An
        attempt with no isolated workspace (serialized fallback, or
        pre-T025 data) falls back to the shared project root exactly as
        before."""
        worker_results = [event for event in claimed if event.event_type == "worker_result"]
        if not worker_results:
            return {}
        with self._store.connection() as db:
            row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        if row is None:
            return {}
        shared_root = row["project_root"]
        captures = {}
        for event in worker_results:
            if event.assignment_ref is None:
                continue
            # A worker_result event's own ``assignment_ref`` is keyed by the
            # provisioned attempt id itself (see ``delegation.completion``'s
            # ``gate`` docstring on the same worker-facing-vs-coordinator-
            # facing distinction) -- resolved directly, no
            # delegation_provisioning bridge needed here.
            workspace_root = delegation_workspace_isolation.resolve_attempt_root(
                self._store, task_id, event.assignment_ref,
            ) or shared_root
            captures[event.id] = delegation_revision.record_attempt_revision(
                self._store, event.assignment_ref, f"worker_result:{event.id}", workspace_root,
            )
        return captures

    def _check_terminal_decision(self, task_id: str):
        with self._store.connection() as db:
            row = db.execute(
                "SELECT id, payload_json FROM task_events WHERE task_id=? AND event_type='coordinator_decision' "
                "ORDER BY id DESC LIMIT 1",
                (task_id,),
            ).fetchone()
        if row is None:
            return None
        import json

        payload = json.loads(row["payload_json"])
        if payload.get("decision") == "accept" and payload.get("assignment_ref") == "task":
            return self._recheck_accept(task_id, row["id"])
        if payload.get("decision") == "ask_user":
            options = payload.get("options")
            return dict(
                kind="ask_user", decision_id=row["id"], question=payload.get("question") or "",
                options=list(options) if isinstance(options, list) else None,
            )
        return None

    def _recheck_accept(self, task_id: str, decision_row_id: int):
        """T017 review (HIGH): a task-level accept decision must never be
        honored *stale*. Re-run the same completion gate the runtime relied
        on to allow recording it in the first place (see
        ``delegation.completion.gate``) here too, at the point the runtime
        is about to actually finalize a host-success response -- the
        coordinator may have delegated (or a worker may have asked a
        question) in the *same* turn, after its own accept tool call, and
        that later event never gets a chance to gate a fresh decide() call.
        A decision with any delegation_requested/meaningful event committed
        after it is treated as superseded outright (never honored, no
        matter what the gate would otherwise say); still-running/queued
        futures for this task also block honoring it.
        """
        from ..delegation import completion as delegation_completion

        with self._store.connection() as db:
            # Deliberately excludes 'user_message': the current turn's own
            # user_message always precedes (a lower id than) the
            # coordinator's own decide() call that reacted to it -- checking
            # it here would wrongly treat every normal accept as stale. A
            # genuinely new user turn is handled by _run() starting a fresh
            # continuation from scratch, never through this recheck.
            newer = db.execute(
                "SELECT event_type, id FROM task_events WHERE task_id=? AND id>? "
                "AND event_type IN ('delegation_requested','worker_result','worker_question',"
                "'assignment_failed','assignment_cancelled') ORDER BY id LIMIT 1",
                (task_id, decision_row_id),
            ).fetchone()
        if newer is not None:
            reason = f"a newer {newer['event_type']} event (id {newer['id']}) arrived after this accept decision (id {decision_row_id})"
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note=f"Acceptance refused: {reason}."), origin="runtime",
            )
            return None
        running = bool(self._running.get(task_id))
        if running:
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note="Acceptance refused: assignment(s) still running/queued for this task."), origin="runtime",
            )
            return None
        ok, reasons = delegation_completion.gate(self._store, task_id)
        if not ok:
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note="Acceptance refused: " + " ".join(reasons)), origin="runtime",
            )
            return None
        return "accepted"

    # -- waiting --------------------------------------------------------

    def _wait_for_progress(self, task_id: str, cancel_event: threading.Event) -> bool:
        """Block (bounded, no model prompt) until a meaningful event arrives.

        Returns True if progress was observed (an unconsumed event now
        exists), False if the wait timed out with nothing pending and no
        worker running (a genuine stall to count against the no-progress
        budget).
        """
        deadline = time.monotonic() + self._wait_timeout
        backoff = _WAIT_BACKOFF_START
        while time.monotonic() < deadline:
            if cancel_event.is_set():
                return True
            if delegation_events.has_unconsumed(self._store, task_id):
                return True
            with self._cond:
                running = bool(self._running.get(task_id))
                self._cond.wait(timeout=min(backoff, deadline - time.monotonic()) if deadline > time.monotonic() else 0)
                still_running = bool(self._running.get(task_id))
            if delegation_events.has_unconsumed(self._store, task_id):
                return True
            if not running and not still_running:
                return False
            backoff = min(backoff * 2, _WAIT_BACKOFF_MAX)
        return delegation_events.has_unconsumed(self._store, task_id)

    def _notify(self) -> None:
        with self._cond:
            self._cond.notify_all()

    # -- delegation_requested / assignment_cancel_requested processing --

    def _process_new_events(self, task_id: str, coordinator_channel: ChannelContext, cancel_event: threading.Event) -> None:
        with self._store.connection() as db:
            requests = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='delegation_requested' ORDER BY id",
                (task_id,),
            ).fetchall()
            cancels = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='assignment_cancel_requested' ORDER BY id",
                (task_id,),
            ).fetchall()
            clarifications = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='coordinator_clarification' ORDER BY id",
                (task_id,),
            ).fetchall()
            # T025: carried as 'progress' rows (see delegation.events
            # .INTEGRATION_PROGRESS_KINDS docstring on why a brand new
            # event_type is not used); the 'integrate:' dedup_key prefix is
            # this operation's own reserved namespace (see mcp_tools
            # ._coordinator_integrate), never colliding with an unrelated
            # progress note.
            integrations = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND dedup_key LIKE 'integrate:%' ORDER BY id",
                (task_id,),
            ).fetchall()
        import json

        with self._lock:
            for row in requests:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_delegations:
                    continue
                self._dispatched_delegations.add(assignment_ref)
                payload = json.loads(row["payload_json"])
                future = self._executor.submit(
                    self._provision_and_prompt, task_id, coordinator_channel, payload, cancel_event,
                )
                self._running.setdefault(task_id, set()).add(future)
                future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

            for row in cancels:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_cancels:
                    continue
                self._dispatched_cancels.add(assignment_ref)
                self._cancel_assignment(task_id, assignment_ref, json.loads(row["payload_json"]).get("reason", ""))

            for row in clarifications:
                assignment_ref = row["assignment_ref"]
                key = (task_id, assignment_ref)
                session_ref = self._worker_sessions.get(key)
                if session_ref is None or row["id"] in self._delivered_clarification_ids:
                    continue
                self._delivered_clarification_ids.add(row["id"])
                message = json.loads(row["payload_json"]).get("message", "")
                future = self._executor.submit(self._deliver_clarification, task_id, key, session_ref, message, cancel_event, row["id"])
                self._running.setdefault(task_id, set()).add(future)
                future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

            for row in integrations:
                if row["id"] in self._dispatched_integrations:
                    continue
                self._dispatched_integrations.add(row["id"])
                payload = json.loads(row["payload_json"])
                self._integrate_assignment(task_id, payload["assignment_ref"], payload["request_key"])

        self._terminate_completed_workers(task_id)

    def _terminate_completed_workers(self, task_id: str) -> None:
        """T017: a worker that reached a terminal state (published a
        result, or failed) has its owned engine process shut down here --
        the *coordinator* side that actually observes the terminal event --
        so capacity is only reused once that shutdown reaches a confirmed
        (or ambiguous) release (see ``delegation.capacity`` /
        ``EngineSessionManagerAdapter.terminate_owned``)."""
        with self._store.connection() as db:
            results = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='worker_result'",
                (task_id,),
            ).fetchall()
            failures = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
                (task_id,),
            ).fetchall()
        attempt_ids: set[str] = {row["assignment_ref"] for row in results if row["assignment_ref"]}
        for row in failures:
            hash_ref = row["assignment_ref"]
            if not hash_ref:
                continue
            with self._store.connection() as db:
                prov = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (hash_ref,),
                ).fetchone()
            if prov is not None and prov["attempt_id"]:
                attempt_ids.add(prov["attempt_id"])
        with self._lock:
            newly_terminal = [a for a in attempt_ids if a not in self._terminated_attempts]
            self._terminated_attempts.update(newly_terminal)
        for attempt_id in newly_terminal:
            try:
                self._engine.terminate_owned(attempt_id)
            except Exception:
                pass
            self._cleanup_isolated_workspace_if_integrated(task_id, attempt_id)
        if newly_terminal:
            self._notify()

    def _cleanup_isolated_workspace_if_integrated(self, task_id: str, attempt_id: str) -> None:
        """T025 design decision 5: remove only a Cortex-owned isolated
        snapshot workspace, and only once this attempt is both terminal
        (already established by the caller) and its integration outcome is
        durably recorded (``workspace_integrations`` has *some* row for its
        assignment -- applied, conflict, or noop; a cancelled assignment
        with no integration ever attempted is also eligible, since there is
        nothing left to integrate). An attempt whose isolated workspace was
        never even created (serialized fallback, or isolation never used)
        is a silent no-op here. Never touches ``project_root`` or any
        user-owned path -- see ``workspace_isolation.IsolatedSnapshotWorkspaceProvisioner
        .cleanup_binding``."""
        with self._store.connection() as db:
            snapshot = db.execute(
                "SELECT workspace_binding_id, mode FROM workspace_snapshots WHERE task_id=? AND attempt_id=? "
                "AND cleaned_up_at IS NULL ORDER BY created_at DESC LIMIT 1",
                (task_id, attempt_id),
            ).fetchone()
            if snapshot is None or snapshot["mode"] != "isolated_snapshot":
                return
            provisioning = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
            ).fetchone()
            cancelled = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' "
                "AND assignment_ref=?", (task_id, provisioning["idempotency_key"] if provisioning else None),
            ).fetchone() if provisioning else None
            integrated = db.execute(
                "SELECT 1 FROM workspace_integrations WHERE workspace_binding_id=?",
                (snapshot["workspace_binding_id"],),
            ).fetchone()
        if integrated is None and cancelled is None:
            return
        try:
            delegation_workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(
                self._store, self._task_project_root(task_id),
            ).cleanup_binding(snapshot["workspace_binding_id"], reason="attempt_terminal_integrated")
        except Exception:
            pass

    def _task_project_root(self, task_id: str) -> str:
        with self._store.connection() as db:
            row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        return row["project_root"]

    def _deliver_clarification(self, task_id, key, session_ref, message, cancel_event, event_row_id) -> None:
        generation_delivery_id = None
        try:
            pending_delivery = getattr(self._engine, "pending_delivery", None)
            role_id = None
            if pending_delivery is not None:
                pending = pending_delivery(session_ref)
                if pending:
                    role_id = self._role_id_for_pending(task_id, pending)
            if role_id is not None:
                message, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                    task_id, session_ref, role_id, message,
                )
                if delivery_blocked:
                    # T024: permanently blocked (budget overflow / compaction
                    # loop) -- never prompt this worker with it; the
                    # explicit blocked task state was already recorded.
                    delegation_events.append_event(
                        self._store, task_id, "assignment_failed", f"assignment_failed:{key[1]}",
                        dict(error="context_budget_blocked"), origin="runtime", assignment_ref=key[1],
                    )
                    return
            self._engine.prompt(session_ref, message, cancel_event)
        except EngineTurnFailed as failure:
            self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{key[1]}",
                dict(error=f"engine_failure:{failure.kind}", kind=failure.kind),
                origin="runtime", assignment_ref=key[1],
            )
            return
        self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=None, stop_reason=StopReason.END_TURN)
        delegation_events.append_event(
            self._store, task_id, "progress", f"clarification_delivered:{event_row_id}",
            dict(note="clarification delivered"), origin="runtime", assignment_ref=key[1],
        )

    def _on_worker_done(self, task_id: str, future: Future) -> None:
        with self._cond:
            pending = self._running.get(task_id)
            if pending is not None:
                pending.discard(future)
            self._cond.notify_all()

    def _attempt_worker_route_fallback(
        self, task_id: str, assignment_ref: str, assignment_id: str, role_id: str, old_attempt_id: str,
        workspace_binding_id: str, config_revision: str | None, failure_class: str,
        instructions: str,
    ) -> tuple[str | None, bool, str | None]:
        """T027: on a fallback-eligible worker failure, consult the durable
        route profile/ceiling/fallback policy this task's active
        configuration revision actually recorded and, only when a fresh
        :func:`routing.authorization.authorize_fallback` check on the
        engine adapter's own :meth:`SessionRuntime.attempt_fallback`
        authorizes it, open a replacement attempt on the single
        deterministic next allowed target route.

        Never tries a second candidate route on refusal (that would be
        exactly the silent route-shopping this task forbids): returns
        ``(None, explicitly_refused)`` on any refusal, missing policy/
        consent, exhausted budget, or a candidate this configuration
        revision never recorded a resolvable ``(engine, connection_id,
        model)`` for. ``explicitly_refused`` is ``True`` only once a real
        configuration revision/route profile was actually consulted and a
        concrete, coordinator-visible ``assignment_failed:...:fallback``
        reason was already recorded for it (the caller must never write a
        second, generic one in that case); ``False`` means fallback was
        never evaluated at all (no configuration revision resolvable, or
        this engine has no ``attempt_fallback`` -- the caller's own plain
        failure message is the only one written). Reconciles the old
        attempt first (T023's own worker-lost
        primitives) so a confirmed/ambiguous capacity reservation and its
        trusted channel are never left dangling; re-derives canonical
        context via the normal :meth:`SessionRuntime.open` generation-
        delivery path (``cause='session_replacement'``, T021/T022) for the
        new engine.
        """
        def _refuse(reason: str) -> None:
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}:fallback",
                dict(error=f"fallback_refused:{reason}", failure_class=failure_class), origin="runtime",
                assignment_ref=assignment_ref,
            )

        attempt_fallback = getattr(self._engine, "attempt_fallback", None)
        if attempt_fallback is None or not config_revision:
            # Fallback was never even evaluated (no real configuration
            # revision to consult at all -- e.g. a test double or a task
            # whose engine adapter never implements attempt_fallback): the
            # caller's own plain failure message is the only one written.
            return None, False, None
        from ..configuration import store as configuration_store
        from ..routing import authorization as routing_authorization
        from ..routing import store as routing_store

        with self._store.connection() as db:
            row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (config_revision,)).fetchone()
        if row is None:
            return None, False, None
        revision = configuration_store._row_dict(row)
        extras = routing_store.get_route_extras(self._store, config_revision)
        if extras is None:
            return None, False, None
        ceiling = revision["ceiling_snapshot"]
        allowed = routing_authorization.allowed_routes_for_role(
            extras["route_profile"], ceiling, role_id, revision["route_id"],
        )
        fallback_targets = ceiling.fallback_policy.allowed_target_routes
        candidates = allowed if fallback_targets is None else (allowed & fallback_targets)
        # T027 review fix (MEDIUM): never re-select a connection already
        # tried for THIS assignment -- walk the predecessor chain from
        # ``old_attempt_id`` (every prior fallback hop, not just the
        # original coordinator route) collecting each hop's own audited
        # connection id, bounded so a corrupt/cyclic chain can never loop
        # forever.
        tried_connection_ids = {revision["route_connection_id"]}
        current_attempt_id, seen = old_attempt_id, set()
        while current_attempt_id and current_attempt_id not in seen and len(seen) < 50:
            seen.add(current_attempt_id)
            hops = routing_store.list_audit_for_attempt(self._store, current_attempt_id)
            if not hops:
                break
            last = hops[-1]
            tried_connection_ids.add(last["connection_id"])
            current_attempt_id = last["predecessor_attempt_id"]
        candidates = {
            route_id for route_id in candidates
            if route_id in extras["route_details"]
            and extras["route_details"][route_id]["connection_id"] not in tried_connection_ids
        }
        if not candidates:
            _refuse("no_untried_authorized_route")
            return None, True, None
        target_route_id = sorted(candidates)[0]
        detail = extras["route_details"][target_route_id]

        from ..recovery import worker_reconciliation

        worker_reconciliation._best_effort_terminate(self._store, self._engine, old_attempt_id)
        worker_reconciliation._revoke_worker_channel(self._store, task_id, old_attempt_id)

        # T027 (scout finding): a fallback attempt needs its OWN worker
        # channel -- the old attempt's channel was just revoked above, and
        # a channel is never valid for a different attempt id -- so its own
        # Cortex MCP subprocess can actually authenticate against the new
        # session, exactly like a first-time delegation's own
        # delegation.provisioning._step_channel.
        from .. import ownership
        from ..trusted_channel import issue_channel_tx

        new_attempt_id = new_id("xa_")
        coordinator_channel = self._coordinator_channels.get(task_id)
        try:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    owner_epoch = ownership.current_owner_epoch(db, task_id)
                    channel_id, channel_secret = issue_channel_tx(
                        db, task_id, "worker", owner_epoch,
                        parent_channel_id=coordinator_channel.channel_id if coordinator_channel is not None else None,
                        assignment_id=assignment_id, attempt_id=new_attempt_id, workspace_binding_id=workspace_binding_id,
                    )
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise
            launch = dict(
                task_id=task_id, assignment_id=assignment_id, attempt_id=new_attempt_id, config_revision=config_revision,
                channel_id=channel_id, channel_secret=channel_secret, role_id=role_id,
            )
            new_session_ref = attempt_fallback(
                old_attempt_id=old_attempt_id, new_attempt_id=new_attempt_id, task_id=task_id, role=role_id,
                failure_class=failure_class, target_connection_id=detail["connection_id"],
                target_route_id=target_route_id, workspace_binding_id=workspace_binding_id, launch=launch,
                instructions=instructions, target_model=detail.get("model"),
            )
        except PortableError as error:
            # Never silently retried on a third route: one deterministic
            # candidate, one authorization decision, surfaced verbatim.
            _refuse(error.code)
            return None, True, None
        except BaseException as error:
            # An unexpected (non-PortableError) failure while attempting
            # fallback must still produce a durable, coordinator-visible
            # event -- never a silently lost assignment (same precedent as
            # this method's own first-prompt BaseException handler).
            _refuse(f"unexpected_error:{type(error).__name__}")
            return None, True, None
        self._worker_sessions[(task_id, assignment_ref)] = new_session_ref
        # delegation.completion.gate's own worker-event bridge resolves a
        # coordinator-facing assignment_ref (this delegation's idempotency
        # key) to the *provisioned* attempt_id before ever looking up a
        # worker_result/worker_question event (those are always keyed by
        # attempt_id, never the hash) -- update it here so the fallback
        # attempt's own worker-facing events resolve, exactly as if this
        # had been the delegation's attempt_id from the start.
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "UPDATE delegation_provisioning SET attempt_id=?, engine_session_ref=?, updated_at=? WHERE idempotency_key=?",
                    (new_attempt_id, new_session_ref, _now(), assignment_ref),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        delegation_events.append_event(
            self._store, task_id, "progress", f"fallback_dispatched:{assignment_ref}:{new_attempt_id}",
            dict(
                note=f"Assignment {assignment_ref} fell over to an authorized alternate route after {failure_class}.",
                predecessor_attempt_id=old_attempt_id, new_attempt_id=new_attempt_id, target_route_id=target_route_id,
            ),
            origin="runtime", assignment_ref=assignment_ref,
        )
        return new_session_ref, True, new_attempt_id

    def _provision_and_prompt(self, task_id: str, coordinator_channel: ChannelContext, payload: Mapping[str, Any], cancel_event: threading.Event) -> None:
        # T017: capacity queuing happens *inside* SessionRuntime.open() (see
        # delegation.live_adapters.EngineSessionManagerAdapter.open) -- never
        # by retrying this whole delegate() call. delegate() uses an
        # idempotency-key claim/steal protocol per provisioning step
        # (delegation.provisioning._acquire_step); retrying it wholesale
        # after a mid-step exception would fight that protocol's own
        # contention recovery instead of just taking longer, exactly like a
        # slow real engine launch it is already built to tolerate.
        assignment_ref = payload["assignment_ref"]
        try:
            result = self._provisioning.delegate(
                coordinator_channel, payload["title"], payload["brief"], payload["role"],
                payload["acceptance"], payload["request_key"],
            )
        except PortableError as error:
            result = None
            if error.code in ("capacity_unsatisfiable", "capacity_timeout"):
                # T027: this delegation's own attempt was allocated (and its
                # capacity reservation/channel opened) before open() itself
                # refused it -- resolvable via the identical idempotency key
                # delegate() itself derives, never a guess.
                from ..delegation.provisioning import _idempotency_key

                idempotency_key = _idempotency_key(task_id, coordinator_channel.channel_id, payload["request_key"])
                with self._store.connection() as db:
                    provisioning_row = db.execute(
                        "SELECT * FROM delegation_provisioning WHERE idempotency_key=?", (idempotency_key,),
                    ).fetchone()
                if provisioning_row is not None and provisioning_row["attempt_id"]:
                    from ..portable.roles import load_role

                    role_set = load_role(provisioning_row["role_id"])
                    new_session_ref, fallback_refused, new_attempt_id = self._attempt_worker_route_fallback(
                        task_id, assignment_ref, provisioning_row["assignment_id"], provisioning_row["role_id"],
                        provisioning_row["attempt_id"], provisioning_row["workspace_binding_id"],
                        provisioning_row["config_revision"], "capacity_unavailable", role_set.body_portable,
                    )
                    if new_session_ref is not None:
                        from types import SimpleNamespace

                        result = SimpleNamespace(
                            engine_session_ref=new_session_ref, attempt_id=new_attempt_id,
                            assignment_id=provisioning_row["assignment_id"], role_id=provisioning_row["role_id"],
                            workspace_binding_id=provisioning_row["workspace_binding_id"],
                            config_revision=provisioning_row["config_revision"],
                        )
                    elif fallback_refused:
                        # _attempt_worker_route_fallback already recorded
                        # the concrete refusal reason itself.
                        return
            if result is None:
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error=error.code, role=payload["role"], detail=_bounded_detail(error.remediation)),
                    origin="runtime", assignment_ref=assignment_ref,
                )
                return
        delegation_events.append_event(
            self._store, task_id, "assignment_ready", f"assignment_ready:{assignment_ref}",
            dict(engine_session_ref=result.engine_session_ref), origin="runtime", assignment_ref=assignment_ref,
        )
        # The worker's own trusted channel is resolved inside its Cortex MCP
        # subprocess from its launch env (see delegation.provisioning /
        # engine session launch); this main process never needs to hold the
        # worker's ChannelContext, only its engine session ref.
        self._worker_sessions[(task_id, assignment_ref)] = result.engine_session_ref
        brief = f"{payload['title']}\n\n{payload['brief']}\n\nAcceptance: {payload['acceptance']}"
        generation_delivery_id = None
        # T027 review fix (MEDIUM): a loop, not a single hardcoded retry --
        # each EngineTurnFailed may fall over to a *different* untried
        # route (``_attempt_worker_route_fallback`` itself excludes every
        # already-tried connection via the predecessor chain, so this
        # always terminates: the number of authorized routes is finite).
        # ``_MAX_FALLBACK_HOPS`` is only a defensive hard ceiling, never
        # expected to bind in practice.
        for _hop in range(self._MAX_FALLBACK_HOPS + 1):
            try:
                brief, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                    task_id, result.engine_session_ref, payload["role"], brief,
                )
                if delivery_blocked:
                    delegation_events.append_event(
                        self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                        dict(error="context_budget_blocked"), origin="runtime", assignment_ref=assignment_ref,
                    )
                    return
                outcome = self._engine.prompt(result.engine_session_ref, brief, cancel_event)
                break
            except EngineTurnFailed as failure:
                # T017: a worker's own engine turn failing (transport
                # closed, timed out, poisoned session, ...) must produce an
                # assignment_failed event -- unless T027's own
                # authorize_fallback explicitly permits a deterministic,
                # never-yet-tried alternate route for the 'engine_failure'
                # class; a route this task never authorized/recorded, or a
                # task with no fallback policy at all, still gets the
                # ordinary assignment_failed and is never silently retried
                # on the same route.
                self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
                # T028: classify this failure before ever considering
                # fallback -- only a genuine ENGINE_FAILURE classification
                # is fallback-eligible (see engine.failure_classification's
                # module docstring). A rate-limit/auth-loss/provider-
                # restriction/refusal/cancellation/indeterminate reading of
                # the same failure.kind text never reaches
                # _attempt_worker_route_fallback at all -- it is refused
                # here, before any route/consent lookup, not merely relying
                # on routing.authorization's own denylist as the only gate.
                failure_class = failure_classification.classify(failure.kind)
                old_attempt_id = getattr(result, "attempt_id", None)
                new_session_ref = None
                fallback_refused = False
                new_attempt_id = None
                role_id = getattr(result, "role_id", payload["role"])
                assignment_id = getattr(result, "assignment_id", None)
                workspace_binding_id = getattr(result, "workspace_binding_id", None)
                config_revision = getattr(result, "config_revision", None)
                if (
                    old_attempt_id is not None and _hop < self._MAX_FALLBACK_HOPS
                    and failure_classification.is_fallback_eligible(failure_class)
                ):
                    from ..portable.roles import load_role

                    role_set = load_role(role_id)
                    new_session_ref, fallback_refused, new_attempt_id = self._attempt_worker_route_fallback(
                        task_id, assignment_ref, assignment_id, role_id, old_attempt_id,
                        workspace_binding_id, config_revision, failure_class, role_set.body_portable,
                    )
                if new_session_ref is None:
                    if not fallback_refused:
                        # Fallback was never even consulted (no attempt id
                        # to reconcile/replace, no real configuration
                        # revision to evaluate it against, the hop ceiling
                        # was reached, or this failure_class is not
                        # fallback-eligible at all) -- the ordinary
                        # engine-failure event is the only one written for
                        # this path, carrying the distinguishable class.
                        delegation_events.append_event(
                            self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                            dict(error=f"engine_failure:{failure.kind}", kind=failure.kind, failure_class=failure_class),
                            origin="runtime", assignment_ref=assignment_ref,
                        )
                    # Else: _attempt_worker_route_fallback already recorded
                    # the concrete refusal reason itself -- never a second,
                    # duplicate assignment_failed event for the same failure.
                    return
                # Re-deliver the identical brief to the fallback attempt's
                # fresh session on the next loop iteration -- the same
                # canonical instructions/capsule path a first-time worker
                # prompt uses (T021/T022), never a retried in-flight prompt
                # against the dead session.
                self._worker_sessions[(task_id, assignment_ref)] = new_session_ref
                from types import SimpleNamespace

                result = SimpleNamespace(
                    engine_session_ref=new_session_ref, attempt_id=new_attempt_id, assignment_id=assignment_id,
                    role_id=role_id, workspace_binding_id=workspace_binding_id, config_revision=config_revision,
                )
            except BaseException as error:
                # T017 fix: this call was previously unwrapped -- any other
                # exception from a worker's prompt silently lost the
                # assignment (no durable record at all). Record it instead.
                self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind="unexpected_exception", stop_reason=None)
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error=type(error).__name__), origin="runtime", assignment_ref=assignment_ref,
                )
                return
        self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
        if outcome.stop_reason == StopReason.REFUSAL:
            # A worker refusal is the coordinator's to evaluate, never
            # silently retried on another engine/connection/model.
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                dict(error="refusal", kind="refusal", text=outcome.text or ""),
                origin="runtime", assignment_ref=assignment_ref,
            )
        elif outcome.stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
            # Recorded as informative progress (not a failure): the worker
            # may still ask a question or publish a partial result on a
            # later turn; this durably notes the limit for the coordinator.
            delegation_events.append_event(
                self._store, task_id, "progress", f"worker_limit:{assignment_ref}:{outcome.stop_reason.value}",
                dict(note=f"Assignment {assignment_ref} worker turn ended: {outcome.stop_reason.value} limit."),
                origin="runtime", assignment_ref=assignment_ref,
            )

    def _cancel_assignment(self, task_id: str, assignment_ref: str, reason: str) -> None:
        with self._store.connection() as db:
            row = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
        if row is not None:
            cancel_attempt = getattr(self._engine, "cancel_attempt", None)
            if cancel_attempt is not None:
                try:
                    cancel_attempt(row["attempt_id"])
                except Exception:
                    pass
            try:
                self._engine.terminate_owned(row["attempt_id"])
            except Exception:
                pass
        delegation_events.append_event(
            self._store, task_id, "assignment_cancelled", f"assignment_cancelled:{assignment_ref}",
            dict(reason=reason), origin="runtime", assignment_ref=assignment_ref,
        )
        # Not self._notify() here: this runs while _process_new_events holds
        # self._lock (a plain, non-reentrant Lock shared with self._cond);
        # a waiting coordinator observes the new event via its own bounded
        # DB poll instead (see _wait_for_progress).

    def _make_host_fs_checker(self, project_root: str, task_id: str):
        """T025 (lead follow-up): builds the per-path unsaved-host-buffer
        check ``delegation.integration.integrate`` consults, reusing the
        existing T014 ``engine.fs_bridge.HostFsForwarder``/``RevisionLog``
        abstraction (never a bespoke second wire protocol) rather than
        calling ``self._host_fs_requester`` directly. Returns ``None`` when
        no host fs requester was ever supplied to this runtime (host never
        negotiated fs, or no live connection is wired yet -- see this
        class's own constructor docstring); the caller then durably
        records ``host_buffer_state: 'unknown'`` instead of silently
        treating "unknown" the same as "checked, no conflict"."""
        if self._host_fs_requester is None:
            return None
        from ..engine.fs_bridge import HostFsForwarder

        forwarder = HostFsForwarder(host_fs_requester=self._host_fs_requester)
        binding_stub = _IntegrationHostBinding(f"integration:{task_id}")

        def checker(relpath: str):
            absolute_path = os.path.join(project_root, relpath)
            try:
                result = forwarder.read_text_file(binding_stub, dict(path=absolute_path))
            except Exception:
                # The host has no buffer for this exact path (or an
                # unrelated transport hiccup) -- never treated as a
                # positive "matches disk" signal; simply not consulted for
                # this one path (see integrate()'s own None handling).
                return None
            content = result.get("content") if isinstance(result, dict) else None
            if content is None:
                return None
            return content.encode("utf-8", "surrogateescape")

        return checker

    def _integrate_assignment(self, task_id: str, assignment_ref: str, request_key: str) -> None:
        """T025 (lead follow-up): performs the actual technical application
        (``delegation.integration.integrate``) for an ``integrate``
        request, then appends the coordinator-visible outcome as its own
        durable ``progress`` event (never silently absorbed) --
        ``payload["kind"]`` is ``integration_applied`` (status
        ``applied``/``noop``) or ``integration_conflict`` (status
        ``conflict``), each dedup-keyed by ``request_key`` so a retried
        request never double-appends (see ``delegation.events
        .INTEGRATION_PROGRESS_KINDS`` on why these are ``progress`` rows,
        not a new ``event_type``)."""
        with self._store.connection() as db:
            provisioning_row = db.execute(
                "SELECT attempt_id, workspace_binding_id, task_id FROM delegation_provisioning "
                "WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            task_row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        if provisioning_row is None or task_row is None or provisioning_row["workspace_binding_id"] is None:
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_conflict:{request_key}",
                dict(kind="integration_conflict", assignment_ref=assignment_ref, request_key=request_key,
                     conflict_paths={"*": {"reason": "unknown_assignment_or_workspace"}},
                     host_buffer_state="unknown"),
                origin="runtime", assignment_ref=assignment_ref,
            )
            return
        project_root = task_row["project_root"]
        host_fs_checker = self._make_host_fs_checker(project_root, task_id)
        result = delegation_integration.integrate(
            self._store, task_id=task_id, project_root=project_root, assignment_ref=assignment_ref,
            request_key=request_key, attempt_id=provisioning_row["attempt_id"],
            workspace_binding_id=provisioning_row["workspace_binding_id"], host_fs_checker=host_fs_checker,
        )
        if result["status"] == "conflict":
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_conflict:{request_key}",
                dict(kind="integration_conflict", assignment_ref=assignment_ref, request_key=request_key,
                     conflict_paths=result["conflict_paths"], host_buffer_state=result["host_buffer_state"]),
                origin="runtime", assignment_ref=assignment_ref,
            )
        else:
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_applied:{request_key}",
                dict(kind="integration_applied", assignment_ref=assignment_ref, request_key=request_key,
                     status=result["status"], applied_paths=result["applied_paths"],
                     integrated_head=result["integrated_head"],
                     integrated_dirty_digest=result["integrated_dirty_digest"],
                     host_buffer_state=result["host_buffer_state"]),
                origin="runtime", assignment_ref=assignment_ref,
            )
        # Not self._notify() here -- same reasoning as _cancel_assignment
        # above (called while _process_new_events holds self._lock).
