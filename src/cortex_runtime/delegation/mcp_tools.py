"""Channel-scoped proposal and evidence dispatch.

This MCP child process records validated model proposals. Only the main
runtime creates assignments and starts engines; no model-facing route exposes
runtime scheduling, model selection or completion decisions.
"""
from __future__ import annotations

import json
import sqlite3
from typing import Any, Mapping

from ..acp import replay_store
from ..contracts import StoreError
from ..trusted_channel import ChannelContext
from . import contracts as delegation_contracts
from . import events as delegation_events
from . import evidence as delegation_evidence
from . import revision as delegation_revision

NO_TECHNICAL_ID_FIELDS = frozenset({
    "task_id", "attempt_id", "channel_id", "workspace_binding_id",
    "context_snapshot_ref", "engine_session_ref", "config_revision",
})


def _worker_ask_question(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from ..orchestration.questions import request
    return request(store,channel,dict(args))


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
    dedup_key = f"result_submit:{report_ref}"
    # Each report_ref carries exactly one canonical publication
    # identity. A re-delivery from the *same* attempt with identical
    # summary/checks reuses the existing event (below, via
    # ``delegation_events.append_event``'s own dedup-by-key). A re-delivery
    # from the same attempt with a *different* summary/checks is a
    # delivery_conflict (never silently overwritten -- ``worker_result``
    # events are append-only). A *different* attempt referencing the same
    # report_ref is refused outright: report_ref binding already restricts
    # it to this worker's own draft, so a second attempt can never
    # legitimately own the first attempt's already-claimed report_ref.
    # Preserve the raw presence of ``checks`` for the fail-fast criterion
    # below.  Once normalized, omitted and explicitly empty arrays are both
    # ``[]``; the field-level error should still explain which input was
    # missing to the model.
    checks = []
    for item in args.get("checks") or []:
        label = delegation_evidence.resolve_check(store, assignment_ref, item.get("tool_call_ref"))
        checks.append(dict(
            name=item["name"], outcome=item["outcome"],
            tool_call_ref=item.get("tool_call_ref"), evidence=label,
        ))
    with store.connection() as db:
        existing = db.execute(
            "SELECT dedup_key, assignment_ref, payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
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
                "identical summary and checks as this report_ref's first result_submit",
                "Retry result_submit with the exact original summary/checks, or publish "
                "a new report via write_report for a changed result.",
            )
    with store.connection() as db:
        cancelled = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='assignment_cancelled' LIMIT 1",
            (channel.task_id, assignment_ref),
        ).fetchone() is not None
        direct_workspace = db.execute(
            "SELECT p.workspace_binding_id, t.project_root FROM delegation_provisioning p "
            "JOIN tasks t ON t.id=p.task_id WHERE p.task_id=? AND p.attempt_id=?",
            (channel.task_id, assignment_ref),
        ).fetchone()
    # A direct worker can change the project while the coordinator's prompt
    # is still running. Observe the tree *before* making its result event
    # visible, so the coordinator can distinguish that completed worker
    # publication from a later, unaccounted-for change. An idempotent retry
    # must use the original observation, never certify newer bytes.
    if not cancelled and direct_workspace is not None and direct_workspace["workspace_binding_id"] in {
        "shared_project_root", "serialized_shared_root",
    }:
        phase = f"worker_publish:{report_ref}"
        with store.connection() as db:
            captured = db.execute(
                "SELECT 1 FROM attempt_revisions WHERE attempt_id=? AND phase=? LIMIT 1",
                (assignment_ref, phase),
            ).fetchone()
        if captured is None:
            delegation_revision.record_attempt_revision(
                store, assignment_ref, phase, direct_workspace["project_root"],
            )
    # T017: each claimed check is stored as claimed; 'observed'/'contradicted'
    # is resolved strictly against this exact attempt's own durable
    # tool_result_observations (delegation.evidence) -- a claim never
    # becomes confirmed without that link (see delegation.evidence docstring).
    from ..orchestration.gateway import _authority
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        _authority(db,channel)
        event = delegation_events.append_event_tx(
            db, channel.task_id, "worker_result", dedup_key,
            dict(report_ref=report_ref, summary=args["summary"], checks=checks),
            origin=channel.channel_id, assignment_ref=assignment_ref,
        )
        db.commit()
    if cancelled and not event.superseded:
        delegation_events.mark_superseded(store, channel.task_id, assignment_ref)
        # mark_superseded flags every event for this assignment_ref,
        # including the assignment_cancelled row itself; that is fine, it
        # remains readable via list_events and is only excluded from a
        # continuation prompt (see acp.orchestrating_runtime).
    if not cancelled:
        with store.connection() as db:
            provisioning = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?",
                (channel.task_id, assignment_ref),
            ).fetchone()
            request = None if provisioning is None else db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id DESC LIMIT 1",
                (channel.task_id, provisioning["idempotency_key"]),
            ).fetchone()
        work_attempt_ref = None if request is None else json.loads(request["payload_json"]).get("work_attempt_ref")
        if work_attempt_ref:
            from .work_graph import WorkGraph

            WorkGraph(store).transition_attempt(
                work_attempt_ref, "reported", idempotency_key=f"work-reported:{event.id}",
            )
    return dict(recorded=True, cursor=event.id, superseded=cancelled)


def _shared_list_sources(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T018: task-scoped list of durable source references (text + resolved
    attachment blocks) for this channel's own task. Never accepts a task id
    -- always ``channel.task_id`` -- so a channel can never enumerate
    another task's sources."""
    after = int(args.get("after") or 0)
    limit = int(args.get("limit") or 50)
    return replay_store.list_sources(store, channel.task_id, after, limit)


def _own_role_id(store, channel: ChannelContext | None) -> str | None:
    """The role this channel was issued for: the coordinator role, or the
    worker role its attempt was provisioned with."""
    if channel is None:
        return None
    if channel.role == "coordinator":
        from ..acp.orchestrating_runtime import COORDINATOR_ROLE_ID

        return COORDINATOR_ROLE_ID
    if not channel.attempt_id:
        return None
    with store.connection() as db:
        row = db.execute(
            "SELECT role_id FROM delegation_provisioning WHERE attempt_id=?", (channel.attempt_id,),
        ).fetchone()
    return row["role_id"] if row is not None else None


def _shared_read_skill(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """Read a shared skill or a declared reference for this channel's role."""
    from ..portable.roles import (
        COORDINATOR_REFERENCE_IDS, EXAMPLE_REFERENCE_IDS, SHARED_ROLE_IDS,
        WORKER_REFERENCE_IDS, load_role, read_reference,
    )

    requested = args.get("skill")
    skill_id = requested if isinstance(requested, str) else ""
    # Skills link their references as `references/<id>.md`; accept the link too.
    skill_id = skill_id.strip().removeprefix("cortex:").removeprefix("references/").removesuffix(".md")
    role_kind = "coordinator" if channel is not None and channel.role == "coordinator" else "worker"
    references = (COORDINATOR_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS if role_kind == "coordinator"
                  else WORKER_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS)
    if skill_id in references:
        return dict(skill=skill_id, title=skill_id.replace("-", " ").capitalize(),
                    body=read_reference(skill_id, role_kind=role_kind))
    if skill_id and skill_id == _own_role_id(store, channel):
        # A channel may always re-read its own role. A coordinator asking
        # for `orchestrator` -- the role it is -- used to fail as
        # `unknown_skill`, a red, misleading failure on the first step of
        # every such session (live, 2026-09-22).
        role_set = load_role(skill_id)
        return dict(skill=skill_id, title=role_set.title, body=role_set.body_portable)
    if skill_id not in SHARED_ROLE_IDS:
        raise StoreError(
            "unknown_skill", "skill", str(requested),
            ", ".join(sorted(SHARED_ROLE_IDS | references)),
            "Read a shared skill or a reference declared for this role.",
        )
    role_set = load_role(skill_id)
    return dict(skill=skill_id, title=role_set.title, body=role_set.body_portable)


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


def _shared_read_large_output(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T024: bounded, owner-scoped read of one persisted large output.

    Never accepts an owner selector -- the owner scope is always derived
    from the channel's own resolved identity (``"coordinator"`` or this
    worker's own ``assignment_ref``, same resolution as
    ``_shared_read_capsule``), so a channel can never read another
    assignment's or the other role's persisted terminal/tool output."""
    from ..recovery.large_output_store import LargeOutputError, read_output

    if channel.role == "coordinator":
        owner_scope = "coordinator"
    else:
        with store.connection() as db:
            row = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE assignment_id=? AND task_id=?",
                (channel.assignment_id, channel.task_id),
            ).fetchone()
        if row is None:
            raise StoreError(
                "large_output_unavailable", "assignment_id", channel.assignment_id,
                "a delegation_provisioning row for this worker's own assignment",
                "This worker's own delegation has not finished being recorded yet; retry shortly.",
            )
        owner_scope = row["idempotency_key"]
    offset = int(args.get("offset") or 0)
    limit = int(args.get("limit") or 65_536)
    try:
        return read_output(
            store, task_id=channel.task_id, owner_scope=owner_scope,
            record_id=args["large_output_id"], offset=offset, chunk_size=limit,
        )
    except LargeOutputError as error:
        raise StoreError(
            "unknown_large_output_ref", "large_output_id", args["large_output_id"],
            "a large_output_id this channel's own owner scope actually persisted",
            "Copy the large_output_id exactly from the terminal/tool-result reference that reported it.",
        ) from error


def _coordinator_read_diagnostics(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """T029: this channel's own task's observability diagnostics summary
    and resource caps (see ``observability.metrics.diagnostics_summary``).
    Never accepts a task id -- always ``channel.task_id``, same pattern as
    ``_shared_read_capsule`` -- so a channel can never read another task's
    usage/attribution data."""
    from ..observability import metrics as observability_metrics

    return observability_metrics.diagnostics_summary(store, channel.task_id)


_COORDINATOR_HANDLERS = dict(
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule, read_large_output=_shared_read_large_output,
    read_skill=_shared_read_skill, read_diagnostics=_coordinator_read_diagnostics,
)
_WORKER_HANDLERS = dict(
    ask_question=_worker_ask_question, list_sources=_shared_list_sources,
    read_source=_shared_read_source, read_capsule=_shared_read_capsule,
    read_large_output=_shared_read_large_output, read_skill=_shared_read_skill,
)
_ROLE_HANDLERS = dict(coordinator=_COORDINATOR_HANDLERS, worker=_WORKER_HANDLERS)


_OPERATION_DESCRIPTIONS = dict(
    task_get_context="Read the current runtime task, original goal, validated plan, results and user answers.",
    answer_submit="Submit a direct answer to an informational question before any plan or workers exist, or a final summary after the user chose to finish when answering a runtime stop question or a question asked after verification. The runtime records and delivers the answer; this does not certify code changes or tests.",
    plan_submit="For changes or substantial delegated work, submit a dependency plan for runtime validation. Accepted only while the task is planning. The runtime chooses routes, schedules assignments, requires review and runs verification.",
    verification_submit="After the runtime verification failed and the work itself is fine: replace only the verification commands and verify again, without redoing the reviewed work.",
    synthesis_submit="Submit a final synthesis grounded in the retained reports and verification. Accepted only after every task, the review and the verification finished. The runtime then completes the task, so use it only when nothing is left for the user to decide; otherwise ask with user_question_request.",
    user_question_request="Request user input while planning or reconciling. Write the details in your message first; the question itself is one short line with short answer options. The runtime persists the question before elicitation and resumes only after a real answer. A question asked while reconciling (approve a proposed plan, choose how to continue) keeps the task open, and the answer returns it to planning: submit a plan for the chosen work, or answer_submit a short close when the user chose to finish.",
    result_submit="Submit this worker attempt's exact immutable report reference, summary and observed checks. Reviewers must include a verdict. This cannot accept the task.",
    ask_question="Ask the user for information this assignment cannot finish without. It stops every running worker and returns the task to planning; after the answer the coordinator re-plans with it.",
    read_skill=(
        "Read one shared skill or role-scoped reference. The input object has exactly one "
        "accepted field, `skill`; pass the exact skill/reference id there (for example "
        "skill='documentation-sync' or skill='knowledge-harvest'). Do not send `topic`, "
        "`name`, or `id`; those fields are invalid."
    ),
    list_sources="List durable source references (user text and resolved attachments) for this task, cursor-paginated.",
    read_source="Bounded, addressable read of one source_ref (text or attachment) with provenance and trust label.",
    read_capsule="Read this channel's own deterministic recovery capsule: identity, applied config, exact "
                 "constraints with origins, own assignments/pipeline, confirmed publications/checks, open/"
                 "uncertain actions, newer events, role/tool access, and continuation intent.",
    read_large_output="Page through an output too large to return inline (a command_run or project_git result), by its large_output_id, byte offset and limit.",
    project_read="Read a project file (UTF-8 text, paged by offset/limit; returns its sha256 for a later write or delete) or list a directory. Read-only and always available.",
    project_git="Read-only git evidence for the project: `status`, or the full working-tree `diff` against HEAD including edits that were already there. Large output is retained; page it with read_large_output.",
    project_write="Create or replace one project file with complete UTF-8 content. Pass the file's current sha256 from project_read, or \"absent\" for a new file; a mismatch changes nothing. Only for a worker whose assignment may write; paths the user protected are refused.",
    project_mkdir="Create one new project directory; its parent must already exist. Only for a worker whose assignment may write.",
    project_delete="Delete one existing regular project file (never a directory), identified by its current sha256. The user confirms every deletion first. Only for a worker whose assignment may write.",
    command_run="Run one command (argv, no shell) in the project root inside the sandbox, as the owner, with their toolchain. Returns exit_code, a tool_call_ref that result_submit accepts as check evidence, and the retained output (read_large_output). Not available to coordinators or consultants.",
    outside_command="Run one command (argv, no shell) outside the sandbox as the user, for work the sandbox cannot do (a global install, a change in HOME). The user confirms it first on a permission card; a refusal means it did not run. Only for a worker whose assignment may write.",
    outside_write="Write one file OUTSIDE the project as the user (inside the project use project_write or your own tools). The user confirms it first; the parent directory must exist. Only for a worker whose assignment may write.",
    draft_write="Replace the whole content of one of your unpublished drafts (from create_draft), keeping the first line create_draft wrote. Publish it with write_report.",
    replan_request="Ask the runtime for a new plan when evidence shows the current decomposition cannot reach the goal. It stops every running worker and returns the task to planning; at most three such requests per task, after which the user decides.",
    scope_request="Report that the assignment's scope prevents completion and name the paths it needs. It stops every running worker and returns the task to planning so the coordinator can re-plan; it never widens the scope by itself. Counts toward the three re-plans per task.",
    escalation_request="Ask for an independent senior consultation on a problem this attempt cannot resolve. It stops every running worker and returns the task to planning with a consultation; counts toward the three re-plans per task.",
    read_diagnostics="Read this task's own observability/usage diagnostics summary: resource caps, recovery "
                     "latency, repeat calls, schema/validation errors, manual interventions, duplicate "
                     "publications, tokens, and concurrency -- each with its declared unit and availability.",
)


# Tools that change the project or act outside it. A channel with no write
# grant -- the coordinator, a read-only worker role, a review stage -- is
# refused every one of them by the gateway, so they are not offered to it.
_WRITE_TOOLS = frozenset({"project_write", "project_mkdir", "project_delete", "outside_write", "outside_command"})


def assignment_profile(store, channel: ChannelContext) -> tuple[str | None, str | None]:
    """``(role_id, stage)`` of the assignment this worker channel runs, or
    ``(None, None)`` when it cannot be resolved (then nothing is narrowed)."""
    if channel is None or channel.role != "worker" or not channel.attempt_id:
        return None, None
    with store.connection() as db:
        row = db.execute(
            "SELECT n.role, n.stage FROM delegation_provisioning p JOIN runtime_nodes n "
            "ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
            "WHERE p.task_id=? AND p.attempt_id=? ORDER BY n.revision DESC LIMIT 1",
            (channel.task_id, channel.attempt_id),
        ).fetchone()
    return (row["role"], row["stage"]) if row is not None else (None, None)


def _offered(name: str, role_kind: str, role_id: str | None, stage: str | None) -> bool:
    """Whether the gateway would ever accept ``name`` from this channel.

    Advertising a tool the gateway always refuses costs a failed call to learn
    it: a coordinator was offered project_write, command_run and the outside
    tools although it has no write grant and may not execute anything.
    """
    from ..orchestration.gateway import READ_ONLY

    if role_kind == "coordinator":
        return name not in _WRITE_TOOLS and name != "command_run"
    if role_id is None:
        return True
    if name in _WRITE_TOOLS and (stage == "review" or role_id in READ_ONLY):
        return False
    if name == "command_run" and "consultant" in role_id:
        return False
    return True


def _narrow_result_submit(schema: dict, stage: str | None) -> dict:
    """``verdict`` is required on a review stage and refused on every other:
    offer it only where it is accepted. Workers that were shown it as an
    optional field sent it anyway and failed with reviewer_only_verdict."""
    if stage is None:
        return schema
    properties = dict(schema["properties"])
    required = [name for name in schema["required"] if name != "verdict"]
    if stage == "review":
        required.append("verdict")
    else:
        properties.pop("verdict", None)
    return dict(schema, properties=properties, required=required)


def _enumerate_skills(schema: dict, role_kind: str, role_id: str | None) -> dict:
    """Offer exactly the ids read_skill accepts from this role, so a guessed
    id is no longer possible (a coordinator once asked for `orchestrator`
    before its own role became readable)."""
    from ..acp.orchestrating_runtime import COORDINATOR_ROLE_ID
    from ..portable.roles import (
        COORDINATOR_REFERENCE_IDS, EXAMPLE_REFERENCE_IDS, SHARED_ROLE_IDS, WORKER_REFERENCE_IDS,
    )

    ids = set(SHARED_ROLE_IDS) | EXAMPLE_REFERENCE_IDS
    ids |= COORDINATOR_REFERENCE_IDS if role_kind == "coordinator" else WORKER_REFERENCE_IDS
    own = COORDINATOR_ROLE_ID if role_kind == "coordinator" else role_id
    if own:
        ids.add(own)
    properties = dict(schema["properties"])
    properties["skill"] = dict(properties["skill"], enum=sorted(ids))
    return dict(schema, properties=properties)


def tool_definitions(role_kind: str, *, role_id: str | None = None, stage: str | None = None) -> list[dict]:
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
        if not _offered(name, role_kind, role_id, stage):
            continue
        schema = schemas[name]
        if name == "result_submit":
            schema = _narrow_result_submit(schema, stage)
        if name == "read_skill":
            schema = _enumerate_skills(schema, role_kind, role_id)
        tools.append(dict(
            name=name, title=name.replace("_", " ").capitalize(),
            description=_OPERATION_DESCRIPTIONS.get(name, f"Delegation operation '{name}'."),
            inputSchema=dict(type="object", properties=schema["properties"],
                              required=schema["required"], additionalProperties=False),
            annotations=dict(readOnlyHint=name in ("project_read", "project_git", "task_get_context", "list_sources", "read_source", "read_capsule", "read_large_output", "read_diagnostics", "read_skill"), destructiveHint=name=="project_delete",
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
    from ..orchestration import gateway
    if operation in gateway.SCHEMAS:
        return gateway.dispatch(store, channel, operation, dict(args))
    from ..orchestration import mcp as runtime_mcp
    if operation in runtime_mcp.schemas(channel.role):
        return runtime_mcp.dispatch(store, channel, operation, dict(args))
    if operation not in delegation_contracts.schemas_for(channel.role):
        raise StoreError("unknown_operation")
    role_kind = channel.role
    handlers = _ROLE_HANDLERS.get(role_kind)
    if handlers is None or operation not in handlers:
        raise StoreError(
            "unknown_operation", "operation", operation,
            ", ".join(sorted(_ROLE_HANDLERS.get(role_kind, {}))),
            f"Use one of this channel's own delegation operations.",
        )
    try:
        validated = delegation_contracts.validate(role_kind, operation, dict(args))
    except StoreError as error:
        # T029: a schema/contract-validation rejection of one channel's own
        # tool call arguments -- recorded for the "schema/validation
        # errors" metric (see observability.metrics). Never blocks the
        # rejection itself; recording is best-effort only.
        try:
            from ..observability.events import append_event as _append_observability_event

            _append_observability_event(
                store, channel.task_id, "validation", str(error), "observed",
                attempt_id=channel.attempt_id, detail=dict(operation=operation),
            )
        except Exception:
            pass
        raise
    result = handlers[operation](store, channel, validated)
    leaked = NO_TECHNICAL_ID_FIELDS & set(result)
    if leaked:  # pragma: no cover - defensive; handlers above never set these
        raise StoreError("internal_leak", "fields", ", ".join(sorted(leaked)), "no technical ids", "internal error")
    return result
