"""T016 delegation MCP tool dispatch: role-filtered operations over trusted channels.

This module is the handler layer behind the T015 operation schemas
(:mod:`contracts`, ``schemas_for(role_kind)``): given an already-resolved
:class:`trusted_channel.ChannelContext` and validated arguments, it performs
exactly the durable write(s) the operation names, inside one transaction,
and returns a plain-dict result carrying no technical id the model did not
already hand back unchanged (``assignment_ref``).

Process boundary (see ``docs/acp/orchestration.md``): this module runs
*inside the Cortex MCP subprocess* an engine launches as its own child
(``cortex.py`` -> ``server.py`` -> here), authenticated by the channel's
trusted env. It must never launch engines or provision workers itself --
``delegate`` only records a ``delegation_requested`` event and returns a
deterministic ``assignment_ref``; the actual :class:`ProvisioningService`
call, worker engine session, and any host-visible progress happen in the
*main* process's :class:`acp.orchestrating_runtime.OrchestratingRuntime`,
which reacts to this event asynchronously.
"""
from __future__ import annotations

import hashlib
import json
from typing import Any, Mapping

from ..acp import replay_store
from ..contracts import StoreError
from ..trusted_channel import ChannelContext
from . import completion as delegation_completion
from . import contracts as delegation_contracts
from . import events as delegation_events
from . import evidence as delegation_evidence

NO_TECHNICAL_ID_FIELDS = frozenset({
    "task_id", "attempt_id", "channel_id", "workspace_binding_id",
    "context_snapshot_ref", "engine_session_ref", "config_revision",
})


def _assignment_ref(task_id: str, coordinator_channel_id: str, request_key: str) -> str:
    """The same deterministic derivation ``ProvisioningService`` uses for its
    ``idempotency_key`` (see ``delegation.provisioning._idempotency_key``),
    kept here as an independent, dependency-free computation so this
    MCP-subprocess-side handler never needs a live ``ProvisioningService``
    (and therefore never launches engines -- see module docstring)."""
    payload = f"{task_id}|{coordinator_channel_id}|{request_key}".encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _require_role(channel: ChannelContext, role: str) -> None:
    if channel.role != role:
        raise StoreError(
            "wrong_channel_role", "role", channel.role, role,
            f"This operation is only available on a {role} channel.",
        )


def _coordinator_delegate(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    request_key = args["request_key"]
    assignment_ref = _assignment_ref(channel.task_id, channel.channel_id, request_key)
    dedup_key = f"delegate:{request_key}"
    event = delegation_events.append_event(
        store, channel.task_id, "delegation_requested", dedup_key,
        dict(request_key=request_key, title=args["title"], brief=args["brief"],
             role=args["role"], acceptance=args["acceptance"],
             assignment_ref=assignment_ref, coordinator_channel_id=channel.channel_id),
        origin=channel.channel_id, assignment_ref=assignment_ref,
    )
    # Idempotent on retry: same request_key -> same assignment_ref and the
    # provisioning row's *current* status (read directly; "provisioning" is
    # only the pre-provisioning default when no row exists yet).
    with store.connection() as db:
        row = db.execute(
            "SELECT status FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
        ).fetchone()
    status = row["status"] if row is not None else "provisioning"
    return dict(assignment_ref=assignment_ref, status=status)


def _coordinator_list_events(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    after = int(args.get("after") or 0)
    limit = int(args.get("limit") or 50)
    events = delegation_events.list_since(store, channel.task_id, after, limit)
    return dict(
        events=[
            dict(cursor=e.id, event_type=e.event_type, assignment_ref=e.assignment_ref,
                 payload=e.payload, superseded=e.superseded, created_at=e.created_at)
            for e in events
        ],
        next_cursor=events[-1].id if events else after,
    )


def _coordinator_clarify(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    dedup_key = f"clarify:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "coordinator_clarification", dedup_key,
        dict(assignment_ref=args["assignment_ref"], message=args["message"]),
        origin=channel.channel_id, assignment_ref=args["assignment_ref"],
    )
    return dict(assignment_ref=args["assignment_ref"], recorded=True, cursor=event.id)


def _coordinator_cancel(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    dedup_key = f"cancel:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "assignment_cancel_requested", dedup_key,
        dict(assignment_ref=args["assignment_ref"], reason=args["reason"]),
        origin=channel.channel_id, assignment_ref=args["assignment_ref"],
    )
    return dict(assignment_ref=args["assignment_ref"], recorded=True, cursor=event.id)


def _coordinator_decide(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    decision = args["decision"]
    assignment_ref = args["assignment_ref"]
    if decision == "ask_user" and not args.get("question"):
        raise StoreError(
            "missing_question", "question", None, "non-empty string",
            "decision='ask_user' requires a concrete 'question' field.",
        )
    if decision == "accept" and assignment_ref != "task":
        with store.connection() as db:
            has_result = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='worker_result' AND superseded=0 LIMIT 1",
                (channel.task_id, assignment_ref),
            ).fetchone()
        if has_result is None:
            raise StoreError(
                "no_result_to_accept", "assignment_ref", assignment_ref,
                "an assignment_ref with a retained worker_result event",
                "Only accept an assignment after a (non-superseded) worker_result event exists for it.",
            )
    if decision == "accept" and assignment_ref == "task":
        # T017 completion gating: refuse to even record a task-level accept
        # when it is visibly ungrounded at call time (the runtime rechecks
        # this same gate defensively before honoring an already-recorded
        # decision -- see delegation.completion module docstring).
        ok, reasons = delegation_completion.gate(store, channel.task_id)
        if not ok:
            raise StoreError(
                "acceptance_refused", "assignment_ref", "task",
                "every non-superseded assignment terminal, no unconsumed meaningful "
                "events, and every accepted assignment's report_ref resolved to a "
                "stored report file",
                "Task-level accept refused: " + " ".join(reasons),
            )
    dedup_key = f"decide:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "coordinator_decision", dedup_key,
        dict(assignment_ref=assignment_ref, decision=decision, rationale=args["rationale"],
             question=args.get("question"), options=args.get("options")),
        origin=channel.channel_id, assignment_ref=assignment_ref,
    )
    return dict(assignment_ref=assignment_ref, decision=decision, recorded=True, cursor=event.id)


def _coordinator_integrate(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T025: records a ``progress`` event (``payload["kind"] ==
    'integration_requested'`` -- see ``delegation.events
    .INTEGRATION_PROGRESS_KINDS``) only; this module runs inside the MCP
    subprocess and must never itself touch the project tree (see module
    docstring). The *main* process's
    ``acp.orchestrating_runtime.OrchestratingRuntime`` reacts to this event
    asynchronously (see its ``_integrate_assignment``), performs the
    actual technical application via ``delegation.integration.integrate``,
    and appends the coordinator-visible ``integration_applied``/
    ``integration_conflict`` outcome (also a ``progress`` event) this same
    ``request_key`` resolves to (idempotent: a retry with the same key
    returns the exact same recorded outcome, never re-applies)."""
    assignment_ref = args["assignment_ref"]
    dedup_key = f"integrate:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "progress", dedup_key,
        dict(kind="integration_requested", assignment_ref=assignment_ref, request_key=args["request_key"]),
        origin=channel.channel_id, assignment_ref=assignment_ref,
    )
    return dict(assignment_ref=assignment_ref, recorded=True, cursor=event.id)


def _worker_ask_question(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    assignment_ref = channel.attempt_id
    dedup_key = f"ask_question:{channel.channel_id}:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "worker_question", dedup_key,
        dict(question=args["question"]),
        origin=channel.channel_id, assignment_ref=assignment_ref,
    )
    return dict(recorded=True, cursor=event.id)


def _worker_publish_result(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    report_ref = args["report_ref"]
    with store.connection() as db:
        report = db.execute("SELECT task_id FROM reports WHERE id=?", (report_ref,)).fetchone()
    if report is None or report["task_id"] != channel.task_id:
        raise StoreError(
            "unknown_report_ref", "report_ref", report_ref,
            "an existing published report id for this task",
            "Publish via write_report first and pass the exact receipt's report_ref.",
        )
    # T019: report_ref must be a report this worker itself published from its
    # own draft (drafts.owner_thread_id == channel.principal_thread_id) --
    # never a coordinator's report or another worker's report. Otherwise a
    # worker could claim any report visible in the task as its own result.
    with store.connection() as db:
        draft = db.execute(
            "SELECT owner_thread_id FROM drafts WHERE published_report_id=?", (report_ref,),
        ).fetchone()
    if draft is None or draft["owner_thread_id"] != channel.principal_thread_id:
        raise StoreError(
            "report_not_own", "report_ref", report_ref,
            "a report published from this worker's own draft in this attempt",
            "Publish the result via write_report using this worker's own draft, "
            "then pass that exact receipt's report_ref.",
        )
    assignment_ref = channel.attempt_id
    dedup_key = f"publish_result:{report_ref}"
    # T019: each report_ref carries exactly one canonical publication
    # identity. A re-delivery from the *same* attempt with identical
    # summary/checks reuses the existing event (below, via
    # ``delegation_events.append_event``'s own dedup-by-key). A re-delivery
    # from the same attempt with a *different* summary/checks is a
    # delivery_conflict (never silently overwritten -- ``worker_result``
    # events are append-only). A *different* attempt referencing the same
    # report_ref is refused outright: report_ref binding already restricts
    # it to this worker's own draft, so a second attempt can never
    # legitimately own the first attempt's already-claimed report_ref.
    checks = []
    for item in args.get("checks") or []:
        label = delegation_evidence.resolve_check(store, assignment_ref, item.get("tool_call_ref"))
        checks.append(dict(
            name=item["name"], outcome=item["outcome"],
            tool_call_ref=item.get("tool_call_ref"), evidence=label,
        ))
    with store.connection() as db:
        existing = db.execute(
            "SELECT assignment_ref, payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
            (channel.task_id, dedup_key),
        ).fetchone()
    if existing is not None:
        if existing["assignment_ref"] != assignment_ref:
            raise StoreError(
                "report_ref_already_published", "report_ref", report_ref,
                "a report_ref not already published by a different attempt",
                "This report_ref's result was already published by a different attempt; "
                "publish a new report for this attempt's own result.",
            )
        prior = json.loads(existing["payload_json"])
        if prior.get("summary") != args["summary"] or prior.get("checks") != checks:
            raise StoreError(
                "delivery_conflict", "report_ref", report_ref,
                "identical summary and checks as this report_ref's first publish_result",
                "Retry publish_result with the exact original summary/checks, or publish "
                "a new report via write_report for a changed result.",
            )
    with store.connection() as db:
        cancelled = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='assignment_cancelled' LIMIT 1",
            (channel.task_id, assignment_ref),
        ).fetchone() is not None
    # T017: each claimed check is stored as claimed; 'observed'/'contradicted'
    # is resolved strictly against this exact attempt's own durable
    # tool_result_observations (delegation.evidence) -- a claim never
    # becomes confirmed without that link (see delegation.evidence docstring).
    event = delegation_events.append_event(
        store, channel.task_id, "worker_result", dedup_key,
        dict(report_ref=report_ref, summary=args["summary"], checks=checks),
        origin=channel.channel_id, assignment_ref=assignment_ref,
    )
    if cancelled and not event.superseded:
        delegation_events.mark_superseded(store, channel.task_id, assignment_ref)
        # mark_superseded flags every event for this assignment_ref,
        # including the assignment_cancelled row itself; that is fine, it
        # remains readable via list_events and is only excluded from a
        # continuation prompt (see acp.orchestrating_runtime).
    return dict(recorded=True, cursor=event.id, superseded=cancelled)


def _shared_list_sources(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T018: task-scoped list of durable source references (text + resolved
    attachment blocks) for this channel's own task. Never accepts a task id
    -- always ``channel.task_id`` -- so a channel can never enumerate
    another task's sources."""
    after = int(args.get("after") or 0)
    limit = int(args.get("limit") or 50)
    return replay_store.list_sources(store, channel.task_id, after, limit)


def _shared_read_capsule(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T021: this channel's own deterministic recovery capsule.

    Never accepts a task id or a recipient selector -- the recipient is
    always derived from the channel's own resolved identity, so a channel
    can never read another task's or another role's/assignment's capsule
    (see ``trusted_channel.resolve_channel``, which is the only source of
    ``channel.task_id``/``channel.role``/``channel.assignment_id``/
    ``channel.attempt_id`` here)."""
    from ..recovery.capsule import build_capsule

    at_revision = args.get("at_revision")
    if channel.role == "coordinator":
        recipient = dict(role="coordinator")
    else:
        with store.connection() as db:
            row = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE assignment_id=? AND task_id=?",
                (channel.assignment_id, channel.task_id),
            ).fetchone()
        if row is None:
            raise StoreError(
                "capsule_unavailable", "assignment_id", channel.assignment_id,
                "a delegation_provisioning row for this worker's own assignment",
                "This worker's own delegation has not finished being recorded yet; retry shortly.",
            )
        recipient = dict(role="worker", assignment_ref=row["idempotency_key"], attempt_id=channel.attempt_id)
    return build_capsule(store, channel.task_id, recipient, at_revision=at_revision)


def _shared_read_source(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T018: bounded read of one source_ref, refused for a ref outside this
    channel's own task (``replay_store.read_source`` scopes every lookup by
    ``channel.task_id``, so a cross-task ref simply resolves to
    ``unknown_source_ref`` -- never leaking whether it exists elsewhere)."""
    offset = int(args.get("offset") or 0)
    limit = int(args.get("limit") or replay_store.READ_CHUNK_LIMIT)
    return replay_store.read_source(store, channel.task_id, args["source_ref"], offset, limit)


_COORDINATOR_HANDLERS = dict(
    delegate=_coordinator_delegate, list_events=_coordinator_list_events,
    clarify=_coordinator_clarify, cancel=_coordinator_cancel, decide=_coordinator_decide,
    integrate=_coordinator_integrate,
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule,
)
_WORKER_HANDLERS = dict(
    ask_question=_worker_ask_question, publish_result=_worker_publish_result,
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule,
)
_ROLE_HANDLERS = dict(coordinator=_COORDINATOR_HANDLERS, worker=_WORKER_HANDLERS)


_OPERATION_DESCRIPTIONS = dict(
    delegate="Delegate one bounded unit of work to a Cortex-managed worker; "
             "never construct a technical id, the runtime fills every binding.",
    list_events="List durable delegation events visible to this coordinator, cursor-paginated.",
    clarify="Send a clarification/steering message to a previously delegated worker.",
    cancel="Cancel a previously delegated assignment; late results are recorded superseded.",
    decide="Record an acceptance/revise/ask_user decision grounded in retained results.",
    integrate="Request technical application of a delegated worker's isolated workspace changes onto "
              "the main project root, against its known base; idempotent, conflict-checked, never "
              "last-write-wins -- the outcome (applied/conflict/noop) appears as a later durable event.",
    ask_question="Ask the delegating coordinator one bounded question; never ask the user directly.",
    publish_result="Publish this assignment's result via an already-acknowledged report reference.",
    list_sources="List durable source references (user text and resolved attachments) for this task, cursor-paginated.",
    read_source="Bounded, addressable read of one source_ref (text or attachment) with provenance and trust label.",
    read_capsule="Read this channel's own deterministic recovery capsule: identity, applied config, exact "
                 "constraints with origins, own assignments/pipeline, confirmed publications/checks, open/"
                 "uncertain actions, newer events, role/tool access, and continuation intent.",
)


def tool_definitions(role_kind: str) -> list[dict]:
    """MCP ``tools/list`` entries for ``role_kind``'s delegation operations.

    Shape matches ``contracts.tool()``'s advertised entries exactly (name,
    title, description, inputSchema, annotations) so a channeled
    ``tools/list`` response is homogeneous with the 7 always-advertised
    tools. Derived fresh from :func:`contracts.schemas_for` on every call
    (never cached) for the same reason that function itself is never
    cached.
    """
    schemas = delegation_contracts.schemas_for(role_kind)
    tools = []
    for name in sorted(schemas):
        schema = schemas[name]
        tools.append(dict(
            name=name, title=name.replace("_", " ").capitalize(),
            description=_OPERATION_DESCRIPTIONS.get(name, f"Delegation operation '{name}'."),
            inputSchema=dict(type="object", properties=schema["properties"],
                              required=schema["required"], additionalProperties=False),
            annotations=dict(readOnlyHint=name in ("list_events", "list_sources", "read_source", "read_capsule"), destructiveHint=False,
                              idempotentHint=name != "ask_question", openWorldHint=False),
        ))
    return tools


def dispatch(store, channel: ChannelContext, operation: str, args: Mapping[str, Any]) -> dict:
    """Validate and perform one delegation operation for ``channel``'s role.

    Raises ``StoreError('wrong_channel_role', ...)`` if ``operation`` is not
    among the channel's own role_kind's operations (a coordinator channel
    can never reach a worker-only handler and vice versa) -- checked before
    schema validation so a worker channel probing a coordinator operation
    name gets a role error, not a validation error that would leak the
    coordinator schema's shape.
    """
    role_kind = channel.role
    handlers = _ROLE_HANDLERS.get(role_kind)
    if handlers is None or operation not in handlers:
        raise StoreError(
            "unknown_operation", "operation", operation,
            ", ".join(sorted(_ROLE_HANDLERS.get(role_kind, {}))),
            f"Use one of this channel's own delegation operations.",
        )
    validated = delegation_contracts.validate(role_kind, operation, dict(args))
    result = handlers[operation](store, channel, validated)
    leaked = NO_TECHNICAL_ID_FIELDS & set(result)
    if leaked:  # pragma: no cover - defensive; handlers above never set these
        raise StoreError("internal_leak", "fields", ", ".join(sorted(leaked)), "no technical ids", "internal error")
    return result
