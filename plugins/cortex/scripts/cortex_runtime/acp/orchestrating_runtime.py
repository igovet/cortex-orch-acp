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
import threading
import time
from typing import Any, Callable, Mapping, Protocol

from ..delegation import events as delegation_events
from ..delegation.provisioning import ProvisioningService
from ..engine.instruction_delivery_registry import delivery_for
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..portable.instruction_delivery import qualify_delivery
from ..portable.roles import load_role
from ..trusted_channel import ChannelContext, issue_channel_tx

COORDINATOR_ROLE_ID = "orchestrator"
DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS = 3
DEFAULT_MAX_TOTAL_CONTINUATIONS = 50
DEFAULT_WORKER_POOL_SIZE = 4
DEFAULT_WAIT_TIMEOUT_SECONDS = 5.0
_WAIT_BACKOFF_START = 0.05
_WAIT_BACKOFF_MAX = 1.0
_UNSET = object()


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


def _coalesce(events: list[delegation_events.TaskEvent]) -> str:
    """Coalesce a batch of claimed events into ONE continuation prompt.

    Every claimed event contributes a line (informative-only event types
    included) so nothing silently vanishes from the record even when it
    does not by itself gate continuation; see
    ``delegation.events.MEANINGFUL_EVENT_TYPES`` for what *does* gate it.
    """
    lines = []
    for event in events:
        payload = event.payload
        ref = event.assignment_ref or ""
        if event.event_type == "user_message":
            lines.append(f"User message: {payload.get('content', '')}")
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
        elif event.event_type == "progress":
            lines.append(f"Progress: {payload.get('note', '')}")
        # delegation_requested / coordinator_decision are the coordinator's
        # own prior tool calls; they need no echo back to the coordinator.
    return "\n".join(lines)


class OrchestratingRuntime:
    """Model-owned coordination loop; implements ``acp.runtime.IncomingRuntime``."""

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
    ) -> None:
        self._store = store
        self._engine = session_runtime
        self._workspace = workspace_provisioner
        self._config = config_snapshot_source
        self._allow_unqualified_delivery = bool(allow_unqualified_delivery)
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
        self._delivered_clarification_ids: set[int] = set()
        # T017: attempt_ids whose owning process has already been asked to
        # terminate after reaching a terminal state (worker_result or
        # assignment_failed) -- so a confirmed/ambiguous capacity release
        # actually happens once terminal, and this is only ever requested
        # once per attempt.
        self._terminated_attempts: set[str] = set()
        self._executor = ThreadPoolExecutor(max_workers=worker_pool_size, thread_name_prefix="cortex-worker")

    # -- IncomingRuntime -----------------------------------------------

    def start_prompt(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
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
        thread a source event through (every existing direct-call test)."""
        with self._lock:
            if task_id in self._active_tasks:
                raise SecondCoordinatorRefused()
            self._active_tasks.add(task_id)
        try:
            return self._run(task_id, session_id, content, emit_update, cancel_event,
                              source_origin=source_origin, source_event_id=source_event_id)
        finally:
            with self._lock:
                self._active_tasks.discard(task_id)

    def cancel(self, session_id: str) -> None:
        # cancel_event (passed per start_prompt call) is the primary
        # cancellation path the loop below observes promptly; this method
        # is a best-effort hook for a session with no in-flight start_prompt
        # call in this process. Full out-of-band cancel plumbing (stopping
        # a coordinator/worker turn from a *different* process) is T020/T028
        # scope; documented, not fabricated here.
        return None

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
                    channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner_epoch)
                else:
                    attempt_ref = existing["coordinator_attempt_ref"]
                    row = db.execute(
                        "SELECT channel_id FROM trusted_channels WHERE task_id=? AND role='coordinator' AND revoked_at IS NULL",
                        (task_id,),
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
            # Reusing a channel issued by an earlier call in *this* process
            # (cached above) never reaches here; a genuinely different
            # process/restart cannot recover the never-persisted secret.
            raise PortableError(
                category=PortableErrorCategory.INDETERMINATE_WRITE, code="coordinator_session_not_cached",
                remediation="This coordinator's channel secret is not held by this runtime instance (a process restart lost it); T017 owns resumable coordinator recovery.",
                retry_eligible=False,
            )
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

    def _open_coordinator_session(self, task_id: str, attempt_ref: str, channel_id: str, secret: str, role_set) -> str:
        return self._engine.open(
            "shared_project_root",
            dict(task_id=task_id, attempt_id=attempt_ref, channel_id=channel_id,
                 channel_secret=secret),
            role_set.body_portable,
        )

    # -- main loop ----------------------------------------------------

    def _run(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
    ) -> StopReason:
        channel, session_ref = self._ensure_coordinator(task_id)
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
        no_progress = 0
        total = 0
        first = True
        while True:
            if cancel_event.is_set():
                # T017 (fresh review, MEDIUM): a queued-for-capacity open
                # must observe task cancellation promptly, not poll forever.
                cancel_task = getattr(self._engine, "cancel_task", None)
                if cancel_task is not None:
                    cancel_task(task_id)
                self._set_state(task_id, "cancelled")
                return StopReason.CANCELLED
            total += 1
            if total > self._max_total:
                self._emit_and_record(task_id, emit_update, "Stopping: exceeded the maximum number of coordination continuations for one prompt.")
                self._set_state(task_id, "blocked", wait_reason="limit:max_total_continuations")
                return StopReason.END_TURN
            continuation_id = new_id("cont_")
            claimed = delegation_events.claim_unconsumed(self._store, task_id, continuation_id)
            meaningful = [e for e in claimed if e.event_type in delegation_events.MEANINGFUL_EVENT_TYPES]
            if not meaningful and not first:
                progressed = self._wait_for_progress(task_id, cancel_event)
                if not progressed:
                    no_progress += 1
                    self._set_state(task_id, "blocked", wait_reason="workers_running", no_progress_count=no_progress)
                    if no_progress >= self._max_no_progress:
                        explanation = "Blocked: no new worker result, question, or user input arrived after repeated waiting."
                        self._emit_and_record(task_id, emit_update, explanation)
                        self._set_state(
                            task_id, "blocked", wait_reason="blocked",
                            wait_detail=dict(explanation=explanation), no_progress_count=no_progress,
                        )
                        return StopReason.END_TURN
                continue
            first = False
            no_progress = 0
            prompt_text = _coalesce(claimed) if claimed else content
            try:
                outcome = self._engine.prompt(session_ref, prompt_text, cancel_event)
            except EngineTurnFailed as failure:
                # T017: an engine failure on the coordinator's own turn is
                # never silently treated as end_turn, and never rerouted to
                # a second engine/connection -- record it durably and stop
                # with an honest blocker instead of a fabricated success.
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                explanation = f"Coordinator engine turn failed ({failure.kind})."
                self._emit_and_record(task_id, emit_update, explanation)
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
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                raise
            if claimed:
                delegation_events.mark_delivered(self._store, task_id, continuation_id)
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
            if terminal is not None and terminal.startswith("waiting_for_user:"):
                question = terminal.split(":", 1)[1]
                emit_update(question)
                self._set_state(task_id, "waiting_for_user", wait_reason="user_input_required", wait_detail=dict(question=question))
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
                self._emit_and_record(task_id, emit_update, f"Coordinator turn ended: {kind} limit reached.")
                if not has_unconsumed and not running:
                    explanation = f"Blocked on a {kind} limit with no unfinished action or new event."
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
        if row is None or not row["wait_reason"] or row["wait_reason"] in ("workers_running", "idle_awaiting_user"):
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
        emit_update(text)
        delegation_events.append_event(
            self._store, task_id, "progress", f"progress:{new_id('p_')}", dict(note=text), origin="runtime",
        )

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
            return "waiting_for_user:" + (payload.get("question") or "")
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
        if newly_terminal:
            self._notify()

    def _deliver_clarification(self, task_id, key, session_ref, message, cancel_event, event_row_id) -> None:
        try:
            self._engine.prompt(session_ref, message, cancel_event)
        except EngineTurnFailed as failure:
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{key[1]}",
                dict(error=f"engine_failure:{failure.kind}", kind=failure.kind),
                origin="runtime", assignment_ref=key[1],
            )
            return
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
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                dict(error=error.code), origin="runtime", assignment_ref=assignment_ref,
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
        try:
            outcome = self._engine.prompt(result.engine_session_ref, brief, cancel_event)
        except EngineTurnFailed as failure:
            # T017: a worker's own engine turn failing (transport closed,
            # timed out, poisoned session, ...) must produce an
            # assignment_failed event -- never silently drop the assignment
            # and never re-route it to a second engine/connection.
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                dict(error=f"engine_failure:{failure.kind}", kind=failure.kind),
                origin="runtime", assignment_ref=assignment_ref,
            )
            return
        except BaseException as error:
            # T017 fix: this call was previously unwrapped -- any other
            # exception from a worker's first prompt silently lost the
            # assignment (no durable record at all). Record it instead.
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                dict(error=type(error).__name__), origin="runtime", assignment_ref=assignment_ref,
            )
            return
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
