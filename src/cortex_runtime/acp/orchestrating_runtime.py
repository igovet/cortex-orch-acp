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
import re
import json
import os
import threading
import time
from typing import Any, Callable, Mapping, Protocol

from ..configuration.policy import BENIGN_CONFIGURATION_ERROR_CODES
from ..contracts import StoreError
from ..delegation import events as delegation_events
from ..delegation import integration as delegation_integration
from ..delegation import revision as delegation_revision
from ..delegation import workspace_isolation as delegation_workspace_isolation
from ..delegation.deliveries import MAX_DELIVERY_RETRIES
from ..delegation import provisioning as delegation_provisioning
from ..delegation.provisioning import ProvisioningService
from ..engine import failure_classification
from ..engine import reaper as engine_reaper
from ..engine.instruction_delivery_registry import delivery_for
from ..observability.redact import scrub_display_text, scrub_text
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..portable.instruction_delivery import DeliveryQualification, qualify_delivery
from ..portable.roles import load_role
from ..recovery.capsule import build_capsule, render_capsule
from ..trusted_channel import ChannelContext, issue_channel_tx
from . import messages as acp_messages
from ..catalog import labels as catalog_labels
from . import pipeline_bootstrap
from . import questions_store
from ..routing.store import get_route_extras
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
# T035 visibility fix: bound on one streamed coordinator delta chunk after
# redaction (scrub_text's own default MAX_TEXT_CHARS=400 is sized for a
# short logged field, not an engine's own text-generation chunk, which can
# legitimately be a whole paragraph in one update). Generous, not
# unbounded: still protects against a single pathological chunk.
_COORDINATOR_DELTA_MAX_CHARS = 4000


def _bounded_detail(text: str) -> str:
    if len(text) <= _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS:
        return text
    return text[: _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS - 3] + "..."


def _provisioning_progress_note(provisioning_sig: tuple[tuple[str, str, str], ...]) -> str:
    """T035 FINDING-13 #2: a short, honest human-readable rendering of a
    ``delegation_provisioning.in_progress_snapshot`` result -- named
    attempts and their current step, so a host/log/human reading
    ``wait_detail`` (and the model itself, whenever this blocked note is
    eventually coalesced into a later continuation) can tell "still
    provisioning, currently at step X" from a generic, undifferentiated
    "workers_running"."""
    parts = [f"{assignment_ref} is provisioning (step: {step})" for assignment_ref, step, _updated_at in provisioning_sig]
    return "Still working: " + "; ".join(parts) + "."


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
# A worker assignment is not a command being run: a client renders
# "execute" under a "Run Command" heading, which describes nothing that
# is happening. The pinned schema has no "delegate"/"agent" kind, so the
# honest choice among its values is the catch-all.
_WORKER_TOOL_CALL_KIND = "other"
_TOOL_CALL_STATUS_FOR_CANCELLED = "failed"


# The pinned ACP schema's ToolCallKind values. An engine is free to send
# anything here (codex-acp and claude-agent-acp do not agree on the set),
# and a value the schema does not define would make the card unrenderable,
# so anything unrecognized becomes "other" rather than being dropped --
# the call still shows, only its icon is generic.
_ACP_TOOL_CALL_KINDS = frozenset({
    "read", "edit", "delete", "move", "search", "execute", "think", "fetch", "switch_mode", "other",
})


def _acp_tool_call_kind(kind: str | None) -> str:
    return kind if kind in _ACP_TOOL_CALL_KINDS else "other"


# Likewise for status: the schema has exactly these four.
_ACP_TOOL_CALL_STATUSES = frozenset({"pending", "in_progress", "completed", "failed"})


def _acp_tool_call_status(status: str | None) -> str:
    return status if status in _ACP_TOOL_CALL_STATUSES else "in_progress"


# A worker's card carries the one thing it is doing right now, not its
# whole feed -- so this bounds a single line, not a transcript.
_WORKER_ACTIVITY_MAX_CHARS = 300

# How many past actions a worker's expanded card keeps.
_WORKER_ACTION_LOG_MAX_ENTRIES = 50

# A tool call's title is one line on a card, never a place for a payload.
_TOOL_CALL_TITLE_MAX_CHARS = 200

# How soon a delegation asked for mid-turn is picked up. Small enough that
# a worker starts while the coordinator is still talking, large enough that
# an idle turn is not a busy loop against the store.
_EARLY_DISPATCH_POLL_SECONDS = 0.25

# How many times the identical integration conflict may repeat for one
# assignment before the coordinator is told that retrying is not a strategy.
_IDENTICAL_INTEGRATION_CONFLICT_LIMIT = 3

# This runtime's own MCP tools, as an engine names them in a tool call.
# Their individual cards are noise in a session feed: a reader wants the
# work, not the bookkeeping that supports it.
_INTERNAL_TOOL_CALL = re.compile(r"^(?:mcp\.)?cortex[._]")

# How many distinct bookkeeping operations the collapsed card lists.
_INTERNAL_OPERATIONS_MAX_ENTRIES = 40

# A worker card's title (role, work, and the action it is on right now).
_WORKER_CARD_TITLE_MAX_CHARS = 300

# Its expanded body.
_WORKER_ACTION_LOG_MAX_CHARS = 4000


def _project_relative(text: str | None, project_root: str | None) -> str | None:
    """Rewrite absolute paths inside ``project_root`` as paths relative to
    it, and leave everything else exactly as it is.

    A card reading "Read file '/var/tmp/cortex-zed-demo/calc.py'" spends
    most of its width on the one part the reader already knows -- which
    project they are in. ``calc.py`` says the same thing. A path *outside*
    the project is the opposite: that it is elsewhere is the important
    part, so it keeps its full spelling.
    """
    if not text or not project_root:
        return text
    root = project_root.rstrip(os.sep)
    if not root:
        return text
    return text.replace(root + os.sep, "")


def _touches_store(title: str | None, locations) -> bool:
    """Whether a tool call is working on this project's own store rather
    than on the project.

    Writing a report or filling a draft is done with ordinary file
    operations against ``<project>/.cortex`` -- by design. To a reader
    those are not edits to their code: they are the bookkeeping that
    surrounds it, and one "Editing files" card per report buried the work
    the report is about. They fold into the same collapsed block as this
    runtime's own tool calls.

    Judged by where the call actually points -- its ``locations`` first,
    and its title only as the fallback for an engine that sends none.
    """
    for location in locations or ():
        path = location.get("path") if isinstance(location, Mapping) else None
        if isinstance(path, str) and f"{os.sep}.cortex{os.sep}" in path:
            return True
    return bool(title) and f"{os.sep}.cortex{os.sep}" in title


def _display_role(role: str) -> str:
    """The role as a reader sees it on a worker's card.

    ``portable.roles`` stores every role under its canonical skill id
    (``worker-backend-dev``); the ``worker-`` prefix is storage identity,
    and on a card already titled "Worker:" it reads as a stutter. Strips
    only that one prefix, never any other part of the id, so an unprefixed
    id (the alias form, e.g. ``backend-dev``) is returned unchanged.
    """
    return role[len("worker-"):] if role.startswith("worker-") else role


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
    def prompt(
        self, session_ref: str, message: str, cancel_event: threading.Event, *,
        on_delta: Callable[[str], bool] | None = None,
        on_tool_call: Callable[..., None] | None = None,
        on_display_update: Callable[[dict], None] | None = None,
    ) -> PromptOutcome: ...


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
            line = (
                f"Integration of assignment {ref} ({payload.get('request_key')}) CONFLICTED, "
                f"nothing was written: {rendered}. host_buffer_state={payload.get('host_buffer_state')}."
            )
            repeated = payload.get("repeated")
            if repeated:
                line += (
                    f" This identical conflict has now happened {repeated} times: the worker's"
                    " work is not the problem and redoing it will not change this. Stop retrying,"
                    " and tell the user what is blocking the integration."
                )
            lines.append(line)
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
        # EXT-01 #5 fix: ``OrchestratingRuntime`` is one long-lived instance
        # per project, shared across every ACP connection for that project
        # (see this class's own docstring above and ``start_prompt``'s own
        # ``host_elicitor`` precedent) -- so a *connection-scoped* requester
        # can never be a frozen constructor default alone (a later
        # connection's own requester would never replace it). Each
        # ``start_prompt`` call registers this call's own real,
        # connection-bound ``host_permission_requester``/``host_fs_requester``
        # (built by ``acp.agent.Agent`` from that connection's own
        # ``HostRequester``/``client_capabilities``) here, keyed by
        # ``task_id`` -- exactly the same per-task registration pattern
        # ``_active_cancel_events`` already uses -- and de-registers it when
        # that call returns (``SecondCoordinatorRefused`` already guarantees
        # only one ``start_prompt`` is ever active for a given ``task_id`` at
        # a time, so there is never a second call to clobber). A permission
        # request that arrives for a task with no currently-registered
        # requester (no ``start_prompt`` call for it in flight right now)
        # fails closed exactly like "no host requester at all" always has.
        self._task_permission_requesters: dict[str, Callable] = {}
        self._task_fs_requesters: dict[str, Callable] = {}
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
        # T035 FINDING-13 #2: assignment refs whose *worker* engine turn is
        # in flight right now, in this process. A worker that is actually
        # running is real work with no MEANINGFUL event of its own until it
        # finishes (delegation_events.MEANINGFUL_EVENT_TYPES), and was
        # therefore indistinguishable from nothing happening -- measured
        # live: assignment_ready at 18:37:58, blocked_no_progress at
        # 18:38:12, fourteen seconds later (evidence/T035/RUN-07). Read by
        # _continuation_loop's no-progress branch under self._lock.
        # ponytail: a worker whose engine turn never returns holds this
        # entry and the coordinator waits on it -- the same unbounded wait
        # FINDING-06 already accepted for the coordinator's own turn; both
        # end on host cancel/stop, which this loop re-checks every tick.
        self._worker_turns_in_flight: set[str] = set()
        # The most recent scrubbed excerpt of each in-flight worker's own
        # engine turn, so its card can show what it is doing rather than
        # only that it is running. Written by the worker's own on_delta,
        # read by _continuation_loop's workers_running tick -- the tick
        # that already exists, never a second channel or a second card.
        self._worker_activity: dict[str, str] = {}
        # Every action a worker has taken, in order -- the card's body,
        # which a client shows only when the card is expanded. The title
        # carries just the latest one, so the feed stays readable while
        # the detail stays one click away.
        self._worker_action_log: dict[str, list[str]] = {}
        # Each worker card's own base title (role and work), so an
        # activity update can extend it instead of replacing it.
        self._worker_card_title: dict[str, str] = {}
        # The last card payload actually sent per assignment, so an
        # unchanged one is never sent twice.
        self._worker_card_sent: dict[str, tuple] = {}
        # Each worker card's route line (engine, model, effort), kept so
        # every later body update still says what it is running on.
        self._worker_card_route: dict[str, str] = {}
        # Worker cards this runtime has actually emitted a create for.
        # See _emit_worker_card: the one place that decides create vs
        # update, so the two can never arrive out of order again.
        self._emitted_worker_cards: set[str] = set()
        # Project revisions this runtime itself produced by applying a
        # worker's integration. See _integrate_assignment and the
        # coordinator-turn digest check.
        self._authorized_project_revisions: dict[str, list] = {}
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
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
        host_permission_requester: Callable | None = None,
        host_fs_requester: Callable[[str, Mapping[str, Any]], Mapping[str, Any]] | None = None,
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

        ``host_permission_requester``/``host_fs_requester`` (EXT-01 #5,
        per-call -- registered under ``self._lock`` into
        ``self._task_permission_requesters``/``self._task_fs_requesters``
        keyed by ``task_id`` for the duration of this call, exactly the same
        "never frozen at construction, always this connection's own" reason
        as ``host_elicitor`` above): ``None`` (every pre-EXT-01 caller)
        leaves this task with no live requester for this call, the same
        fail-closed behavior as before this fix existed.
        ``host_permission_requester`` reaches a worker/coordinator engine
        session's own ``session/request_permission`` relay via
        ``delegation.live_adapters.EngineSessionManagerAdapter``'s own
        engine_session_id -> task_id lookup (see
        ``EngineSessionManagerAdapter.set_runtime``); ``host_fs_requester``
        is consulted by :meth:`_make_host_fs_checker` for the unsaved-editor-
        buffer check ``delegation.integration.integrate`` performs.

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
            if host_permission_requester is not None:
                self._task_permission_requesters[task_id] = host_permission_requester
            if host_fs_requester is not None:
                self._task_fs_requesters[task_id] = host_fs_requester
        try:
            return self._run(task_id, session_id, content, emit_update, cancel_event,
                              source_origin=source_origin, source_event_id=source_event_id, host_elicitor=elicitor,
                              emit_tool_call=emit_tool_call,
                              emit_display_update=emit_display_update)
        finally:
            with self._lock:
                self._active_tasks.discard(task_id)
                if self._active_cancel_events.get(task_id) is cancel_event:
                    del self._active_cancel_events[task_id]
                if host_permission_requester is not None:
                    self._task_permission_requesters.pop(task_id, None)
                if host_fs_requester is not None:
                    self._task_fs_requesters.pop(task_id, None)

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
            session_ref = self._coordinator_sessions[task_id]
            # EXT-01 #2a: a cached, still-open coordinator session is
            # exactly the case the old (session-open-only) configuration
            # glue never revisited -- a mid-task model/provider change
            # (``acp.agent._apply_mid_task_selection``) only ever proposes
            # a fresh ``'selected'`` revision, it never itself re-opens this
            # session. Reconciling here, on every call (cached or not),
            # right after resolving *which* session this turn will use and
            # strictly before any prompt content for it is composed/sent
            # below (see ``_run``/``answer_question``'s own call sites),
            # is the one safe boundary: never mid-turn, which would leave
            # that turn's own effective model ambiguous.
            self._reconcile_configuration(task_id, session_ref)
            return cached, session_ref
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
            self._reconcile_configuration(task_id, session_ref)
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
        self._reconcile_configuration(task_id, session_ref)
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
        # T035 FINDING-02 fix (superseded, EXT-01 #2): ``self._engine.open``
        # (see ``delegation.live_adapters.EngineSessionManagerAdapter
        # .reconcile_configuration``) runs the coordinator's own first-turn
        # configuration confirmation synchronously *inside* this call, and
        # deliberately never re-raises an unconfirmed outcome into open()'s
        # own caller -- an already-opened engine *session* is not itself
        # aborted over a configuration refusal (see that method's own
        # docstring). This cheap, preview-time check catches the common
        # case (a fresh session's own open() just settled 'failed'/
        # 'rejected') immediately; ``_ensure_coordinator``'s own
        # ``_reconcile_configuration`` call right after this method returns
        # is the authoritative, always-run gate (it also covers
        # 'indeterminate', and re-reads the *current* revision rather than
        # this snapshot, which can be stale for a cached-session turn --
        # see that method's own docstring).
        self._reject_if_configuration_failed(task_id, preview.get("config_revision"))
        return session_id

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
        substitution.

        Superseded (but not removed -- still a cheap, harmless early check
        right after a fresh/recovered open) as the authoritative gate by
        :meth:`_reconcile_configuration`/``EngineSessionManagerAdapter
        .reconcile_configuration``, EXT-01 #2c, which additionally blocks
        'indeterminate' and runs on every turn, not only session-open."""
        if not revision_id:
            return
        with self._store.connection() as db:
            row = db.execute(
                "SELECT state, error_code FROM configuration_revisions WHERE id=?", (revision_id,),
            ).fetchone()
        if row is None or row["state"] not in ("failed", "rejected"):
            return
        if row["error_code"] in BENIGN_CONFIGURATION_ERROR_CODES:
            return
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code=row["error_code"] or "configuration_apply_failed",
            remediation="This task's configuration revision was refused by the engine; select and apply a "
                        "working configuration before starting a new turn.",
            retry_eligible=False,
        )

    def _reconcile_configuration(self, task_id: str, session_ref: str) -> None:
        """EXT-01 finding #2: the authoritative, always-run configuration
        gate for a coordinator turn -- called by :meth:`_ensure_coordinator`
        every time it resolves a session (a reused cached one, a freshly
        opened one, or a recovered one alike), strictly before returning to
        its own caller (``_run``/``answer_question``), both of which then
        go on to compose and send this turn's own prompt content -- i.e.
        exactly the one safe per-turn boundary: never mid-turn, which would
        leave that turn's own effective model ambiguous.

        Delegates the actual per-session wire (re)confirmation to the
        session runtime's own ``reconcile_configuration(task_id,
        session_ref)`` (real production case:
        ``delegation.live_adapters.EngineSessionManagerAdapter
        .reconcile_configuration`` -- see its own docstring for exactly how
        it closes all three EXT-01 #2 cases (a mid-task selection on an
        already-cached session, a fresh session after restart/recovery
        whose task-level revision was already 'active', and a partial/
        indeterminate application) -- looked up via ``getattr`` exactly
        like ``shutdown`` above, so every existing ``SessionRuntime`` test
        double that does not implement it (i.e. carries no real
        configuration behind it at all) is an unchanged no-op, never a
        ``AttributeError``.

        That call returns a :class:`~..portable.errors.PortableError`
        (never raises itself) when this session's own confirmation for the
        task's current revision is refused or unconfirmed; this method
        raises it, blocking the turn -- reusing the existing
        'failed'/'rejected'/'indeterminate' vocabulary, never a fourth
        state."""
        reconcile = getattr(self._engine, "reconcile_configuration", None)
        if reconcile is None:
            return
        error = reconcile(task_id, session_ref)
        if error is not None:
            raise error

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
        """Shared blocked-delivery path for both a ``capsule_oversize`` and a
        T024 ``budget_overflow`` condition (see
        :meth:`_block_generation_immediately` for the distinct, already-
        bounded ``compaction_blocked`` case): begin/re-check a delivery row
        for ``capsule_revision`` and durably record + (when a real channel
        is available) emit a localized explicit message -- never silently.

        EXT-01 #6: mandatory context that does not fit blocks the engine
        call on the *first* attempt, not only once :data:`MAX_DELIVERY_RETRIES`
        is exhausted -- absence of required context is never a transient
        condition to run the turn through while retries remain. This method
        therefore always returns ``True`` (the caller must not prompt the
        engine this turn). Retrying is still legitimate -- a rebuilt
        capsule, a different presentation, or the coordinator being told
        the assignment must change, each naturally tried again on a later
        turn via the ordinary continuation loop -- so while bounded retries
        remain the delivery row is only ``blocked`` (not ``permanent``, see
        ``begin_delivery``'s reopen-to-``built`` rule) and a live "will
        retry" notice goes straight to ``emit_update`` (the *transport*
        callback for this turn -- no store write). It is deliberately never
        routed through ``_set_state``/``_emit_localized``: both durably
        touch ``task_orchestration``/``task_events``, which
        ``recovery.capsule``'s own section builders and
        ``_high_watermark`` fold straight into ``capsule_revision`` (see
        that module's ``continuation_intent.orchestration`` section and its
        docstring on ``source_high_watermark``) -- recording the retry
        itself would mint a fresh ``capsule_revision`` on every attempt,
        making this exact "still overflowing on the *same* content" check
        (keyed on an unchanged ``capsule_revision``) never accumulate
        ``retry_count`` and never reach :data:`MAX_DELIVERY_RETRIES` at all.
        Only once retries are exhausted does the block become permanent,
        durably recorded with a ``limit:`` prefix and the plain overflow
        message via the normal task_orchestration/emit_update path -- safe
        precisely because the pending generation is cleared at that point,
        so no further capsule build for it will ever observe the change.
        The two outcomes are therefore always distinguishable: a live-only
        "will retry" notice (this turn) versus a durable "permanently
        blocked" task state (every future read)."""
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
        if emit_update is not None:
            retry_key = f"{message_key}_retry"
            retry_params = dict(message_params, retry_count=delivery["retry_count"], max_retries=MAX_DELIVERY_RETRIES)
            emit_update(acp_messages.render(retry_key, language or "en", **retry_params))
        return True

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
        first-attempt-blocks path the pre-existing ``capsule_oversize`` case
        already uses -- never a silent loop, never a turn run without the
        mandatory content, and never a silently truncated constraint.

        Returns ``(prompt_text, delivery_id, blocked)``. ``delivery_id`` is
        ``None`` when nothing was pending, blocked (retriable or
        permanent), or the engine offers no delivery tracking at all (every
        pre-T022 fake/test session runtime). ``blocked`` is ``True``
        whenever this exact generation could not be packaged to fit this
        turn -- whether still within :data:`MAX_DELIVERY_RETRIES` (a live
        "will retry" notice on ``emit_update`` this turn, no durable
        ``task_orchestration`` state change, and the pending generation
        kept for a later attempt) or permanently exhausted (durable
        ``blocked`` task state with a ``limit:`` prefix, pending generation
        cleared) -- the caller must end the turn without prompting the
        engine either way (EXT-01 #6)."""
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

    def _publish_initial_pipeline(self, task_id: str, channel, request_text: str) -> None:
        """Best-effort route context for the runtime-published initial
        pipeline (see ``pipeline_bootstrap``): the bound engine/model, and
        the effort this task's current configuration revision actually
        recorded, when one is already resolvable. Never raises -- a failure
        building this optional context still lets the pipeline publish with
        an honest "not yet determined" route line (pipeline_bootstrap's own
        fallback) rather than blocking the turn.
        """
        # ponytail: ``configuration_route_extras`` records engine/model but
        # never a resolved "effort" value (see routing.store's own schema
        # docstring -- reasoning effort lives only in per-attempt engine
        # parameters, never durably joined back to a route here), so this
        # is honestly left unknown rather than guessed; add a real lookup
        # if a durable source for it ever exists.
        route_info = None
        try:
            preview = self._config.snapshot(task_id)
            model = None
            revision_id = preview.get("config_revision")
            if revision_id:
                extras = get_route_extras(self._store, revision_id)
                if extras is not None:
                    model = extras["route_model"]
            route_info = dict(engine=preview.get("engine"), model=model, effort=None)
        except Exception:
            route_info = None
        pipeline_bootstrap.publish_initial_pipeline(self._store, task_id, channel, request_text, route_info)

    def _run(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
        host_elicitor: HostElicitor | None = None,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
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
        if not pre_existing:
            # Runtime-owned record, no model turn spent on it (see
            # pipeline_bootstrap module docstring). Gated on ``not
            # pre_existing`` only as a cheap skip for every later turn --
            # publish_initial_pipeline's own durable check (the task's
            # ``reports`` row) is the actual idempotency guarantee, so a
            # second first-turn (recovery, replacement session) is still
            # safe even if this in-process flag were ever wrong.
            self._publish_initial_pipeline(task_id, channel, content)
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
            emit_display_update=emit_display_update,
        )

    def _continuation_loop(
        self, task_id, session_id, channel, session_ref, content, emit_update, cancel_event, language,
        host_elicitor: HostElicitor | None = None, *,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        """The coordinator continuation loop proper: claim/coalesce/prompt/
        process, repeated until a terminal outcome. Factored out of
        :meth:`_run` (T020) so :meth:`answer_question` -- an out-of-band
        ``/cortex-answer`` delivery that must never re-run the open-question
        gating or steering/first-turn detection in :meth:`_run`'s preamble
        (its own answer event is already durably recorded by the caller) --
        can drive the exact same loop starting from an already-ensured
        coordinator session."""
        # Everything this loop sends to the client is a separate block: a
        # continuation turn's streamed text, a localized notice, a blocked
        # explanation. They arrived concatenated ("...before
        # delegating.Pipeline published.", "First turn text.Blocked: ...")
        # because each was emitted as a bare chunk with nothing between
        # them. One blank line before every block except the first, and
        # none *inside* a turn's own run of deltas, is the whole fix --
        # ``emit_update`` is rebound here so every existing emission in
        # this loop gets it without a second, parallel emit path.
        _raw_emit_update = emit_update
        _emit_state = dict(any=False, in_delta_run=False)

        def emit_update(text, *, delta=False):  # noqa: F811 -- deliberate rebind, see above
            if _emit_state["any"] and not (delta and _emit_state["in_delta_run"]):
                _raw_emit_update("\n\n")
            _emit_state["any"] = True
            _emit_state["in_delta_run"] = delta
            return _raw_emit_update(text)

        no_progress = 0
        total = 0
        first = True
        # T035 FINDING-13 #2: the last-observed delegation_provisioning
        # in-progress signature (see delegation_provisioning
        # .in_progress_snapshot) -- None until the first no-progress wait
        # tick actually checks it, so the very first tick after dispatch
        # never spuriously counts as "no change" before anything was ever
        # observed.
        last_provisioning_sig = None
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
                    # T035 FINDING-13 #2: a no-progress *tick* is not
                    # itself a stall -- provisioning a worker/coordinator
                    # engine session is real work with no MEANINGFUL event
                    # of its own until it finishes (see
                    # delegation_events.MEANINGFUL_EVENT_TYPES's own
                    # docstring for why it must stay that way: an
                    # informative-only notice must never itself wake this
                    # loop). Before ever charging this tick against the
                    # no-progress budget, check whether provisioning
                    # actually advanced a step since the last tick
                    # (delegation_provisioning.in_progress_snapshot); if it
                    # did, this is "still coming", not "stuck" -- the
                    # budget resets instead of depleting, and the
                    # coordinator's own blocked state names the real
                    # step/attempt so a human/host (and the model, on
                    # whatever later continuation actually reaches it) can
                    # tell the difference. Only a signature that is empty
                    # (nothing provisioning at all) or unchanged from the
                    # last tick (provisioning exists but has not moved)
                    # ever counts as "no progress" -- ties the coordinator's
                    # patience to observed backend reality instead of a
                    # fixed tick count with no relationship to it.
                    provisioning_sig = delegation_provisioning.in_progress_snapshot(self._store, task_id)
                    if provisioning_sig and provisioning_sig != last_provisioning_sig:
                        last_provisioning_sig = provisioning_sig
                        no_progress = 0
                        self._set_state(
                            task_id, "blocked", wait_reason="workers_provisioning", no_progress_count=0,
                            wait_detail=dict(explanation=_provisioning_progress_note(provisioning_sig)),
                        )
                        self._emit_provisioning_tool_calls(emit_tool_call, provisioning_sig)
                        continue
                    last_provisioning_sig = provisioning_sig
                    with self._lock:
                        worker_running = bool(self._worker_turns_in_flight)
                    if worker_running:
                        # See self._worker_turns_in_flight: a worker's own
                        # engine turn being in flight is observed backend
                        # reality, exactly like an advancing provisioning
                        # step above, so it resets the budget rather than
                        # depleting it. A worker whose turn has *ended*
                        # without producing a meaningful event leaves this
                        # set, and the budget then depletes normally --
                        # which is the genuine stall this budget exists for.
                        no_progress = 0
                        self._set_state(task_id, "blocked", wait_reason="workers_running", no_progress_count=0)
                        self._emit_worker_activity_tool_calls(emit_tool_call)
                        continue
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
            chosen_language = self._explicit_language(task_id)
            if chosen_language is not None:
                prompt_text = (
                    f"{acp_messages.render('answer_in_language', chosen_language)}\n\n{prompt_text}"
                )
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
            # T035 visibility fix: stream the coordinator's own text as the
            # engine actually produces it (SessionRuntime.prompt's new
            # ``on_delta``) instead of only learning ``outcome.text`` once
            # the whole turn ends -- see PromptOutcome's own docstring for
            # why this is the coordinator-only path (a delegated worker's
            # prompt() call below never passes ``on_delta``). Routed through
            # the same redaction choke point (``scrub_text``) as every other
            # client-facing text this runtime emits (``_emit_assignment_tool_calls``).
            def _on_coordinator_delta(chunk_text: str) -> bool:
                return emit_update(scrub_text(chunk_text, max_chars=_COORDINATOR_DELTA_MAX_CHARS), delta=True)

            # T035 FINDING-13 #1, measured: the coordinator calls delegate()
            # in the middle of its turn, but the delegation was only ever
            # acted on after prompt() returned (see _process_new_events'
            # single call site below), so a worker could not start until
            # the coordinator stopped talking. Live, that was the whole gap
            # the finding called "provisioning takes eight minutes":
            # delegation_requested 18:59:08, coordinator gives up 19:04:40,
            # worker ready 19:05:33 -- one second after the cancellation,
            # with the open itself taking 0.738s (evidence/T035/RUN-07).
            #
            # This watcher runs the same _process_new_events while the turn
            # is still in flight, so provisioning overlaps the turn that
            # asked for it. It is the same method, deliberately: its own
            # dispatch bookkeeping is already written to be race-free
            # against a concurrent call (see its "P1 #7 fix" comment), so
            # this adds a caller, not a second code path.
            _turn_done = threading.Event()

            def _dispatch_while_turn_runs() -> None:
                while not _turn_done.wait(timeout=_EARLY_DISPATCH_POLL_SECONDS):
                    if cancel_event.is_set():
                        return
                    try:
                        self._process_new_events(task_id, channel, cancel_event, emit_tool_call)
                        # A worker that starts during this turn also works
                        # during it: its card must follow along now, not
                        # once the coordinator finally stops talking.
                        self._emit_worker_activity_tool_calls(emit_tool_call)
                    except Exception:  # noqa: BLE001 - never fail the turn over an early start
                        return

            _early_dispatch = threading.Thread(
                target=_dispatch_while_turn_runs, name="cortex-early-dispatch", daemon=True,
            )
            _early_dispatch.start()
            try:
                outcome = self._engine.prompt(
                    session_ref, prompt_text, cancel_event, on_delta=_on_coordinator_delta,
                    on_tool_call=self._engine_tool_call_emitter(
                        emit_tool_call, language=language, project_root=self._task_project_root(task_id),
                    ),
                    on_display_update=emit_display_update,
                )
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
            finally:
                # However this turn ends -- success, engine failure, or a
                # raise -- the early-dispatch watcher stops with it; it
                # exists only for the window while the turn is in flight.
                _turn_done.set()
            self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
            if claimed:
                delegation_events.mark_delivered(self._store, task_id, continuation_id)
            post_turn_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
            with self._lock:
                authorized = list(self._authorized_project_revisions.get(task_id, ()))
            # Compared with the same function the check itself uses -- a
            # revision is a structure, not a value to match by identity.
            authorized_now = any(
                not delegation_revision.workspace_changed(revision, post_turn_revision)
                for revision in authorized
            )
            with self._lock:
                if authorized_now:
                    # Consumed: a *further* change, by anyone, is once again
                    # a revision nobody authorized.
                    self._authorized_project_revisions.pop(task_id, None)
                    pre_turn_revision = post_turn_revision
            if not authorized_now and delegation_revision.workspace_changed(pre_turn_revision, post_turn_revision):
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
            # T035 visibility fix: outcome.text was already streamed chunk
            # by chunk via ``_on_coordinator_delta``/``on_delta`` above as
            # the engine produced it -- emitting it again here in full would
            # just duplicate everything the client already saw.
            if outcome.stop_reason == StopReason.REFUSAL:
                # Never re-route to another engine/connection/model on
                # refusal -- persist and hand the refusal straight to the host.
                self._set_state(
                    task_id, "refused",
                    wait_reason="refused", wait_detail=dict(explanation=outcome.text or "Coordinator refused this turn."),
                )
                return StopReason.REFUSAL
            self._process_new_events(task_id, channel, cancel_event, emit_tool_call)
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
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
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
                emit_display_update=emit_display_update,
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

    def _explicit_language(self, task_id: str) -> str | None:
        """The language the user actually chose, or ``None`` for "auto".

        Distinct from :meth:`_resolve_language`, which always answers with
        a concrete language (falling back to the script of what the user
        last wrote). Only an explicit choice is ever imposed on the
        coordinator itself -- on "auto" the model keeps following the user,
        which is what "auto" means.
        """
        explicit = acp_steering.get_language_override(self._store, task_id)
        return explicit if explicit in acp_messages.SUPPORTED_LANGUAGES else None

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

    def _delegation_route_label(self, task_id: str, assignment_ref: str) -> str | None:
        """Which engine, model and effort this worker is actually running
        on, read from its own configuration revision -- never from the
        task default, which a fallback or a per-role route may not match.

        ``None`` when provisioning has not recorded a revision for this
        assignment yet, or the revision has since gone: a card without the
        line is better than a card asserting a route that was not used.
        """
        import json

        try:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT config_revision FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if row is None or not row["config_revision"]:
                    return None
                revision = db.execute(
                    "SELECT route_engine, parameters FROM configuration_revisions WHERE id=?",
                    (row["config_revision"],),
                ).fetchone()
            if revision is None:
                return None
            parameters = json.loads(revision["parameters"] or "{}")
        except Exception:  # noqa: BLE001 - a label is never worth failing a card over
            return None
        parts = [catalog_labels.engine_short_name(revision["route_engine"])]
        model = parameters.get("model")
        if model:
            parts.append(str(model))
        # Each engine names its own effort knob; whichever one this route
        # carries is the one to show (see configuration.policy).
        for key in ("effort", "reasoning_effort", "thought_level"):
            value = parameters.get(key)
            if value:
                parts.append(str(value))
                break
        return " · ".join(parts)

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
                self._emit_worker_card(
                    emit_tool_call, assignment_ref, only_if_new=True,
                    title=f"Worker: {_display_role(role_hint)} — {work_title}",
                    kind=_WORKER_TOOL_CALL_KIND, status="pending",
                    text=scrub_text(f"Queued: {event.payload.get('brief', '')}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            elif event.event_type == "assignment_ready" and event.assignment_ref:
                assignment_ref = event.assignment_ref
                fields: dict[str, Any] = dict(status="in_progress")
                role_id = self._delegation_provisioning_role(task_id, assignment_ref)
                if role_id:
                    requested = self._delegation_requested_payload(task_id, assignment_ref)
                    if requested is not None:
                        # Requirement: the title names the *resolved* role
                        # id, not the raw hint the coordinator typed --
                        # this is the first point a resolved id exists.
                        base_title = f"Worker: {_display_role(role_id)} — {requested.get('title', '')}"
                        fields["title"] = base_title
                        with self._lock:
                            self._worker_card_title[assignment_ref] = base_title
                route = self._delegation_route_label(task_id, assignment_ref)
                if route:
                    fields["text"] = route
                    with self._lock:
                        self._worker_card_route[assignment_ref] = route
                self._emit_worker_card(emit_tool_call, assignment_ref, **fields)
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
                self._emit_worker_card(
                    emit_tool_call, assignment_ref, status="completed",
                    text=self._worker_card_body(assignment_ref, lines),
                )
            elif event.event_type == "assignment_failed" and event.assignment_ref:
                text = f"Not delegated/failed: {event.payload.get('error', 'unknown error')}"
                detail = event.payload.get("detail")
                if detail:
                    text += f" {detail}"
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref, status="failed",
                    text=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            elif event.event_type == "assignment_cancelled" and event.assignment_ref:
                text = f"Cancelled: {event.payload.get('reason', '')}"
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref,
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    text=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )

    def _worker_card_body(self, assignment_ref: str, lines) -> str:
        """A finished worker's card body: what it reports, and under it
        everything it actually did.

        The action log is consumed here -- this is the last thing that
        needs it -- so a card is never rebuilt from a stale history."""
        with self._lock:
            route = self._worker_card_route.pop(assignment_ref, None)
            actions = self._worker_action_log.pop(assignment_ref, None)
        body = list(lines)
        if route:
            body.append(route)
        if actions:
            body.extend(actions)
        return scrub_text("\n".join(part for part in body if part), max_chars=_WORKER_ACTION_LOG_MAX_CHARS)

    def _emit_worker_card(self, emit_tool_call, assignment_ref: str, *, only_if_new: bool = False, **fields) -> None:
        """Every update to a worker's assignment card goes through here.

        A card's create used to be emitted by the event-claim path (which
        only runs between coordinator turns) while its updates came from
        the wait/provisioning path -- so once a delegation started while
        the turn that asked for it was still running, an update could
        reach the client before the create. A client answers an update for
        a card it never saw with "Tool call not found", and the card -- the
        delegation itself -- never appears at all.

        So no caller decides ``new`` any more: this does, from whether this
        card has actually been emitted for this task yet. The default title
        is filled in for a create that arrives update-shaped, because a
        card must be named when it is created.
        """
        if emit_tool_call is None or not assignment_ref:
            return
        card_id = _worker_tool_call_id(assignment_ref)
        with self._lock:
            first = card_id not in self._emitted_worker_cards
            if first:
                self._emitted_worker_cards.add(card_id)
        if not first and only_if_new:
            # The create already happened -- at dispatch, which is where a
            # delegation's card is owned. Re-sending the queued state from
            # the event-claim path would be the same card twice.
            return
        payload = dict(fields)
        payload["tool_call_id"] = card_id
        payload["new"] = first
        if first:
            payload.setdefault("kind", _WORKER_TOOL_CALL_KIND)
            payload.setdefault("status", "pending")
            if not payload.get("title"):
                payload["title"] = "Worker assignment"
        emit_tool_call(payload)

    def _engine_tool_call_emitter(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
        *, prefix: str = "", id_scope: str = "coordinator", language: str = "en",
        project_root: str | None = None,
    ):
        """A callback for ``SessionRuntime.prompt``'s ``on_tool_call`` that
        renders each of that session's own engine tool calls as an ACP
        ToolCall card -- what is being read, edited, searched or run, while
        it happens, which is otherwise visible only in durable evidence
        nobody watching the session can see.

        Carries exactly the portable fields ``EngineEvent`` has for a tool
        call: ref, kind, title, status. Never arguments, output or result
        -- those are deliberately not portable (see ``EngineEvent``), so a
        card here is an accurate "it is reading calc.py", never a diff.

        ``prefix`` labels whose call it is once more than one session is
        working (a worker's role), and ``id_scope`` keeps two sessions'
        cards from colliding when both engines happen to use the same
        wire tool-call id. Returns ``None`` when no card channel is wired,
        which every pre-existing caller relies on.
        """
        if emit_tool_call is None:
            return None
        # Per call ref, the last title the engine actually gave for it.
        # Both engines name a call when they create it and then send
        # status-only updates with no title at all, so an update must
        # never overwrite the name with a placeholder -- that is exactly
        # how a feed of real actions turned into a column of "tool call".
        titles: dict[str, str] = {}

        def on_tool_call(
            ref: str, kind: str | None, title: str | None, status: str | None,
            locations=None, content=None,
        ) -> None:
            card_id = f"call:{id_scope}:{ref}"
            known = titles.get(ref)
            if isinstance(title, str) and title.strip():
                known = scrub_display_text(
                    _project_relative(title.strip(), project_root), max_chars=_TOOL_CALL_TITLE_MAX_CHARS,
                )
                titles[ref] = known
            if status is None:
                status = pending_status.pop(ref, None)
            else:
                pending_status.pop(ref, None)
            fields: dict[str, Any] = dict(
                tool_call_id=card_id, status=_acp_tool_call_status(status),
            )
            if _INTERNAL_TOOL_CALL.match(known or title or "") or _touches_store(known or title, locations):
                # This runtime's own bookkeeping -- create_task,
                # set_governance, list_events and the rest. A reader asked
                # for their session, not for its plumbing, and one card per
                # call buried the actual work. They collapse into a single
                # card whose body says what was done.
                name = _INTERNAL_TOOL_CALL.sub("", known or title or "") or "tool call"
                if internal_group[1] is None:
                    # A run that was interrupted by real work is finished;
                    # this call opens a new group rather than reopening a
                    # block the reader has already scrolled past.
                    internal_group[0] += 1
                    internal_group[1] = []
                entries = internal_group[1]
                if name and name not in entries:
                    entries.append(name)
                    del entries[:-_INTERNAL_OPERATIONS_MAX_ENTRIES]
                card_id = f"call:{id_scope}:internal:{internal_group[0]}"
                first = card_id not in seen
                seen.add(card_id)
                emit_tool_call(dict(
                    new=first, tool_call_id=card_id, status="completed",
                    title=acp_messages.render("internal_operations_card", language),
                    **(dict(kind="other") if first else {}),
                    text="\n".join(entries),
                ))
                return
            internal_group[1] = None
            if known is None:
                # Nothing to show yet. A card reading "tool call" tells a
                # reader nothing at all, which is the complaint this whole
                # emitter exists to answer -- so hold it until the engine
                # names the call, and remember the status it had meanwhile
                # so nothing is lost when it does.
                pending_status[ref] = status
                pending_kind.setdefault(ref, kind)
                if locations:
                    pending_locations[ref] = locations
                if content:
                    pending_content[ref] = content
                return
            if ref not in seen:
                seen.add(ref)
                fields["new"] = True
                fields["kind"] = _acp_tool_call_kind(kind if kind is not None else pending_kind.get(ref))
            else:
                fields["new"] = False
            fields["title"] = f"{prefix}{known}" if prefix else known
            # Carried verbatim: these are the schema's own display shapes,
            # and a client turns them into the clickable path and the diff
            # a reader actually wants. Anything the engine did not send
            # stays absent rather than being invented.
            locations = locations if locations else pending_locations.pop(ref, None)
            content = content if content else pending_content.pop(ref, None)
            if locations:
                fields["locations"] = list(locations)
            if content:
                fields["content"] = list(content)
            emit_tool_call(fields)

        seen: set[str] = set()
        # Status/kind seen for a call the engine has not named yet.
        pending_status: dict[str, str | None] = {}
        pending_kind: dict[str, str | None] = {}
        # [sequence number, entries of the run in progress or None]. Only
        # consecutive internal calls share a card: one block per run, not
        # one block for the whole session.
        internal_group: list = [0, None]
        pending_locations: dict[str, object] = {}
        pending_content: dict[str, object] = {}
        return on_tool_call

    def _emit_worker_activity_tool_calls(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
    ) -> None:
        """Show what a running worker is actually doing.

        Between ``assignment_ready`` and its result, a worker's card said
        only "in_progress" -- the reported complaint was "I can see the
        worker started, but not what it is doing". This updates the SAME
        per-assignment card (never a second one, exactly like
        :meth:`_emit_provisioning_tool_calls`) with the tail of that
        worker's own turn, scrubbed through the one redaction choke point.

        Card text only: a worker's text never enters the client's message
        stream, which stays the coordinator's own (see ``PromptOutcome``).
        A no-op when no card channel is wired, and when the worker has not
        produced anything yet (nothing to say is not worth an update).
        """
        if emit_tool_call is None:
            return
        with self._lock:
            activity = dict(self._worker_activity)
            logs = {ref: list(entries) for ref, entries in self._worker_action_log.items()}
            titles = dict(self._worker_card_title)
            routes = dict(self._worker_card_route)
        for assignment_ref, latest in activity.items():
            # The sentinel marks text the worker spoke (kept as a rolling
            # tail) rather than a tool call's own one-line title; it never
            # reaches the client.
            latest = latest[1:] if latest.startswith("\u0000") else latest
            latest = " ".join(latest.split())
            if not latest:
                continue
            fields: dict[str, Any] = dict(status="in_progress")
            base = titles.get(assignment_ref)
            if base:
                fields["title"] = scrub_text(f"{base} · {latest}", max_chars=_WORKER_CARD_TITLE_MAX_CHARS)
            entries = logs.get(assignment_ref)
            body_lines = []
            route = routes.get(assignment_ref)
            if route:
                body_lines.append(route)
            if entries:
                body_lines.extend(entries)
            if body_lines:
                fields["text"] = scrub_text("\n".join(body_lines), max_chars=_WORKER_ACTION_LOG_MAX_CHARS)
            # This runs on every wait tick, but a worker is not doing
            # something new on every tick: re-sending an identical card is
            # pure wire noise (and a client redrawing it, flicker).
            signature = (fields.get("title"), fields.get("text"))
            with self._lock:
                if self._worker_card_sent.get(assignment_ref) == signature:
                    continue
                self._worker_card_sent[assignment_ref] = signature
            self._emit_worker_card(emit_tool_call, assignment_ref, **fields)

    def _emit_provisioning_tool_calls(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
        provisioning_sig: tuple[tuple[str, str, str], ...],
    ) -> None:
        """T035 visibility fix: a worker still provisioning (opening its
        engine session, not yet ready -- see
        ``delegation_provisioning.in_progress_snapshot``) is otherwise
        completely silent from ``delegation_requested`` (pending, T035)
        until ``assignment_ready`` (in_progress) -- exactly the gap the real
        Zed session this fixes sat in for over a minute with nothing shown.
        Updates the SAME per-assignment card ``_emit_assignment_tool_calls``
        already created (never a second channel/card), so its title/history
        stay one continuous timeline. A no-op when ``emit_tool_call`` was
        never wired, same convention as ``_emit_assignment_tool_calls``."""
        if emit_tool_call is None:
            return
        for assignment_ref, step, _updated_at in provisioning_sig:
            self._emit_worker_card(
                emit_tool_call, assignment_ref, status="in_progress",
                text=scrub_text(f"Provisioning: {step}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
            )

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

    def _process_new_events(
        self, task_id: str, coordinator_channel: ChannelContext, cancel_event: threading.Event,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> None:
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

        # P1 #7 fix: the dedup bookkeeping below (which needs the shared
        # lock, to stay race-free against a concurrent _process_new_events
        # call for the same task) is now the *only* thing done under
        # self._lock. Every external/blocking effect -- executor.submit's
        # own dispatch, _cancel_assignment, _integrate_assignment -- runs
        # after the lock is released (P1 #7's second point: no blocking
        # external operation under the shared runtime lock).
        #
        # For a submitted future specifically: this also closes the
        # deadlock itself. future.add_done_callback() invokes the callback
        # *immediately, on the calling thread* when the future is already
        # done (documented concurrent.futures behavior) -- so registering
        # it while still holding self._lock let an already-finished future
        # re-enter _on_worker_done's self._cond (built on that same
        # non-reentrant lock) on the same thread and deadlock. Ordering is
        # preserved without that risk: the future is added to self._running
        # (under a short, separate lock acquisition) strictly *before*
        # add_done_callback is ever called, so a callback -- whether fired
        # synchronously right here or later from a worker thread -- always
        # finds the future already present to discard; it can never be
        # missed, and it is never observable as still "running" once its
        # own callback has had a chance to run.
        to_delegate = []
        to_cancel = []
        to_clarify = []
        to_integrate = []
        with self._lock:
            for row in requests:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_delegations:
                    continue
                self._dispatched_delegations.add(assignment_ref)
                # The ref is the event's own column, carried alongside the
                # payload rather than merged into it: the payload is what
                # the delegate tool wrote, and it stays that.
                to_delegate.append((assignment_ref, json.loads(row["payload_json"])))

            for row in cancels:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_cancels:
                    continue
                self._dispatched_cancels.add(assignment_ref)
                to_cancel.append((assignment_ref, json.loads(row["payload_json"]).get("reason", "")))

            for row in clarifications:
                assignment_ref = row["assignment_ref"]
                key = (task_id, assignment_ref)
                session_ref = self._worker_sessions.get(key)
                if session_ref is None or row["id"] in self._delivered_clarification_ids:
                    continue
                self._delivered_clarification_ids.add(row["id"])
                message = json.loads(row["payload_json"]).get("message", "")
                to_clarify.append((key, session_ref, message, row["id"]))

            for row in integrations:
                if row["id"] in self._dispatched_integrations:
                    continue
                self._dispatched_integrations.add(row["id"])
                payload = json.loads(row["payload_json"])
                to_integrate.append((payload["assignment_ref"], payload["request_key"]))

        for assignment_ref, payload in to_delegate:
            # The card appears when the worker is dispatched, not when the
            # coordinator's turn later ends. Delegation now starts while
            # that turn is still running, and until this ran here the
            # coordinator could truthfully say "I delegated it" with
            # nothing to show for minutes -- reported as "the worker is
            # nowhere to be seen".
            self._emit_worker_card(
                emit_tool_call, assignment_ref,
                title=f"Worker: {_display_role(payload.get('role') or '?')} — {payload.get('title', '')}",
                kind=_WORKER_TOOL_CALL_KIND, status="pending",
                text=scrub_text(f"Queued: {payload.get('brief', '')}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
            )
            future = self._executor.submit(
                self._provision_and_prompt, task_id, coordinator_channel, payload, cancel_event,
            )
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for assignment_ref, reason in to_cancel:
            self._cancel_assignment(task_id, assignment_ref, reason)

        for key, session_ref, message, row_id in to_clarify:
            future = self._executor.submit(self._deliver_clarification, task_id, key, session_ref, message, cancel_event, row_id)
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for assignment_ref, request_key in to_integrate:
            self._integrate_assignment(task_id, assignment_ref, request_key)

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
                with self._lock:
                    self._worker_turns_in_flight.add(assignment_ref)

                # A worker's card shows the one thing it is working on now,
                # never its whole feed: whichever of these two happened
                # last wins, because that is what "what it is doing" means.
                # Card content only -- never the client's message stream
                # (PromptOutcome owns what a worker's text means to the
                # coordinator; see SessionRuntime.prompt).
                def _on_worker_delta(chunk_text: str, _ref: str = assignment_ref) -> bool:
                    with self._lock:
                        previous = self._worker_activity.get(_ref, "")
                        if not previous.startswith("\u0000"):
                            previous = ""
                        self._worker_activity[_ref] = "\u0000" + (previous[1:] + chunk_text)[-_WORKER_ACTIVITY_MAX_CHARS:]
                    return True

                def _on_worker_tool_call(ref, kind, title, status, *rest, _ref=assignment_ref) -> None:
                    # Keyword-only ``_ref``: this callback is invoked
                    # positionally by the adapter, which grew ``locations``
                    # and ``content`` arguments -- as a positional default
                    # ``_ref`` silently took ``locations`` instead, the call
                    # then raised TypeError on the sixth argument, and the
                    # adapter's own best-effort ``except`` swallowed it. The
                    # visible result was a worker card that never showed
                    # anything the worker did, which is the one thing it is
                    # for. ``*rest`` accepts whatever the adapter carries
                    # without this needing to know about it.
                    action = (title or "tool call")[:_WORKER_ACTIVITY_MAX_CHARS]
                    with self._lock:
                        self._worker_activity[_ref] = action
                        log = self._worker_action_log.setdefault(_ref, [])
                        if not log or log[-1] != action:
                            log.append(action)
                            del log[:-_WORKER_ACTION_LOG_MAX_ENTRIES]

                try:
                    outcome = self._engine.prompt(
                        result.engine_session_ref, brief, cancel_event,
                        on_delta=_on_worker_delta, on_tool_call=_on_worker_tool_call,
                    )
                finally:
                    with self._lock:
                        self._worker_turns_in_flight.discard(assignment_ref)
                        # The *current* action is over; the log of what
                        # this worker did is not. A worker that finishes
                        # between two wait ticks used to have its whole
                        # history discarded before anything could show it,
                        # so its card said only that it had finished. The
                        # log now survives to the result card, where a
                        # reader can open it and see what was done.
                        self._worker_activity.pop(assignment_ref, None)
                        self._worker_card_sent.pop(assignment_ref, None)
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
        # Not self._notify() here: a waiting coordinator observes the new
        # event via its own bounded DB poll instead (see
        # _wait_for_progress) -- no _running entry is added/removed by this
        # call, so there is nothing for _cond to usefully notify here.

    def _make_host_fs_checker(self, project_root: str, task_id: str):
        """T025 (lead follow-up) / EXT-01 #5: builds the per-path unsaved-
        host-buffer check ``delegation.integration.integrate`` consults,
        reusing the existing T014 ``engine.fs_bridge.HostFsForwarder``/
        ``RevisionLog`` abstraction (never a bespoke second wire protocol)
        rather than calling a host fs requester directly.

        The requester consulted is this exact call's own registered
        ``self._task_fs_requesters[task_id]`` (set by ``start_prompt`` for
        the duration of the call that is running this integration -- see
        that method's own docstring) if one was registered, else this
        runtime's constructor-time ``self._host_fs_requester`` default (kept
        only for callers/tests that never opt into the per-task mechanism).

        Returns ``None`` -- capability genuinely absent -- when neither is
        set (host never negotiated fs, or no live connection is wired for
        this task right now); the caller then durably records
        ``host_buffer_state: 'unknown'`` instead of silently treating
        "unknown" the same as "checked, no conflict".

        The returned ``checker`` *raises* (never silently returns ``None``)
        when the host round-trip for one specific path could not be trusted
        (timeout/closed transport/malformed reply) -- ``integrate()`` itself
        is responsible for turning that into a per-path conflict and an
        overall ``host_buffer_state: 'check_failed'`` (see its own
        docstring); a clean "the host has no buffer for this exact path"
        answer stays a plain ``None`` return, never a conflict by itself."""
        requester = self._task_fs_requesters.get(task_id)
        if requester is None:
            requester = self._host_fs_requester
        if requester is None:
            return None
        from ..engine.fs_bridge import HostFsForwarder

        forwarder = HostFsForwarder(host_fs_requester=requester)
        binding_stub = _IntegrationHostBinding(f"integration:{task_id}")

        def checker(relpath: str):
            absolute_path = os.path.join(project_root, relpath)
            result = forwarder.read_text_file(binding_stub, dict(path=absolute_path))
            content = result.get("content") if isinstance(result, dict) else None
            if not isinstance(content, str):
                raise ValueError(f"host fs checker: malformed/missing content for {relpath!r}.")
            return content.encode("utf-8", "surrogateescape")

        return checker

    def permission_requester_for_task(self, task_id: str):
        """EXT-01 #5: the exact connection-bound
        ``(engine_session_id, params, timeout) -> dict`` callable currently
        registered for ``task_id`` (see ``start_prompt``'s own docstring),
        or ``None`` if no ``start_prompt`` call for this task is in flight
        right now. Called by
        ``delegation.live_adapters.EngineSessionManagerAdapter`` (see
        ``set_runtime``) after it has already resolved
        ``engine_session_id -> task_id`` itself -- this method never sees
        or needs an ``engine_session_id``, only the ``task_id`` that lookup
        already produced."""
        with self._lock:
            return self._task_permission_requesters.get(task_id)

    def _identical_integration_conflicts(self, task_id: str, assignment_ref: str, conflict_paths) -> int:
        """How many integration conflicts this assignment has already had
        with this exact path/reason signature. Signature, not count: a
        conflict that moved to a different path or a different reason is
        progress, and must not be counted against a retry."""
        import json

        signature = sorted(
            (path, (detail or {}).get("reason")) for path, detail in (conflict_paths or {}).items()
        )
        seen = 0
        with self._store.connection() as db:
            rows = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND assignment_ref=? AND dedup_key LIKE 'integration_conflict:%'",
                (task_id, assignment_ref),
            ).fetchall()
        for row in rows:
            try:
                previous = json.loads(row["payload_json"] or "{}").get("conflict_paths") or {}
            except (TypeError, ValueError):
                continue
            if sorted((path, (detail or {}).get("reason")) for path, detail in previous.items()) == signature:
                seen += 1
        return seen

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
            payload = dict(
                kind="integration_conflict", assignment_ref=assignment_ref, request_key=request_key,
                conflict_paths=result["conflict_paths"], host_buffer_state=result["host_buffer_state"],
            )
            # A conflict is a legitimate "try again": the host had an
            # unsaved buffer, main moved. The identical conflict over and
            # over is not -- watched live, a worker finished, integration
            # conflicted, the coordinator sent it back to redo work that
            # was already done, and that ran in a circle. Count how many
            # times this exact signature has already happened for this
            # assignment, so the coordinator is told when retrying has
            # stopped being a strategy. Advisory, never a runtime gate: the
            # coordinator still owns what to do about it.
            repeats = self._identical_integration_conflicts(task_id, assignment_ref, result["conflict_paths"])
            if repeats + 1 >= _IDENTICAL_INTEGRATION_CONFLICT_LIMIT:
                payload["repeated"] = repeats + 1
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_conflict:{request_key}",
                payload, origin="runtime", assignment_ref=assignment_ref,
            )
        else:
            # The project just changed, and this runtime changed it, on the
            # worker's own request. Record the resulting revision as
            # authorized: the coordinator-turn digest backstop compares the
            # project before and after a turn and cannot otherwise tell this
            # apart from the coordinator writing to the project itself --
            # observed live blocking a turn that had done everything right
            # (evidence/T035/RUN-08).
            try:
                with self._lock:
                    self._authorized_project_revisions.setdefault(task_id, []).append(
                        delegation_revision.capture_revision(project_root),
                    )
            except Exception:  # noqa: BLE001 - an unrecorded baseline only costs a false block
                pass
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
        # above.
