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
from .. import task_workspace
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


# The template whose placeholder comments an inline report must not contain;
# publication rejects any it finds, exactly as for a draft filled by hand.
INLINE_REPORT_TEMPLATE = "general"


def inline_report_keys(channel_id: str, request_key: str) -> tuple[str, str]:
    """The store delivery keys of one result_submit's inline report.

    Derived only from the channel and the result's own request_key, so a
    retry of the same result -- after any crash -- replays the same draft and
    the same publication instead of creating a second report. The prefix
    keeps them apart from model-chosen keys, which the server hashes as
    ``channel_id + NUL + key`` (a channel id never equals this prefix).
    """
    import hashlib

    base = "result_submit.report\0" + channel_id + "\0" + request_key
    return (hashlib.sha256((base + "\0draft").encode()).hexdigest(),
            hashlib.sha256((base + "\0publish").encode()).hexdigest())


def inline_report_form(args: Mapping[str, Any]) -> dict:
    """``args`` as retained in runtime_requests: an inline report's Markdown
    is replaced by its SHA-256. The report file keeps the body; the request
    digest still binds it (a changed body is a request_key conflict)."""
    import hashlib

    if "report" not in args:
        return dict(args)
    report = args["report"]
    return dict(args, report=dict(
        title=report["title"], markdown_sha256=hashlib.sha256(report["markdown"].encode("utf-8")).hexdigest()))


def publish_inline_report(store, channel: ChannelContext, request_key: str,
                          report: Mapping[str, Any], summary: str) -> str:
    """Publish a result_submit inline report through the ordinary
    create_draft/write_report path as this channel's own draft; idempotent.

    Each step is its own durable, keyed delivery, so a crash at any point
    leaves at most an unpublished draft or a published report without a
    result, and the retry of the same result continues from there: the
    draft and the publication replay, and a published draft is never
    rewritten. The file system and SQLite are never assumed to commit
    together.
    """
    from ..contracts import clip_summary_preview
    from ..orchestration.gateway import _authority, write_draft

    def authorize(db):
        _authority(db, channel)

    draft_key, publish_key = inline_report_keys(channel.channel_id, request_key)
    draft = store.call("create_draft", dict(template=INLINE_REPORT_TEMPLATE, request_key=draft_key),
                       channel.principal_thread_id, channel.parent_principal_thread_id,
                       trusted=True, authorization=authorize)
    draft_id = draft["draft_id"]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        authorize(db)
        row = db.execute("SELECT published_report_id, role_id FROM drafts d LEFT JOIN delegation_provisioning p "
                         "ON p.task_id=d.task_id AND p.attempt_id=? WHERE d.id=? AND d.task_id=?",
                         (channel.attempt_id, draft_id, channel.task_id)).fetchone()
        if row is None:
            raise StoreError("draft_not_found", "draft_id", draft_id)
        if row["published_report_id"] is None:
            write_draft(store, db, channel, draft_id,
                        f"Cortex draft ID: `{draft_id}`\n\n" + report["markdown"])
        db.commit()
    author = (row["role_id"] or "worker")[:120]
    published = store.call("write_report", dict(
        draft_id=draft_id, title=report["title"], author=author, request_key=publish_key,
        summary=clip_summary_preview("write_report", dict(summary=summary))["summary"]),
        channel.principal_thread_id, channel.parent_principal_thread_id,
        trusted=True, authorization=authorize)
    return published["report_id"]


def inline_report_ref(db, task_id: str, actor: str, request_key: str) -> str | None:
    """The report the inline publication of one result_submit produced, from
    its durable write_report delivery; None before that delivery committed."""
    _draft_key, publish_key = inline_report_keys(actor, request_key)
    row = db.execute("SELECT response FROM deliveries WHERE scope=? AND operation='write_report' AND delivery_key=?",
                     (task_id, publish_key)).fetchone()
    return None if row is None else json.loads(row["response"]).get("report_id")


def _worker_publish_result(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    report_ref = args["report_ref"]
    with store.connection() as db:
        report = db.execute("SELECT task_id FROM reports WHERE id=?", (report_ref,)).fetchone()
    if report is None or report["task_id"] != channel.task_id:
        raise StoreError(
            "unknown_report_ref", "arguments.report.report_ref", "well-formed id not published in this task",
            "the report_id returned by this worker's write_report in this task",
            "Publish this worker's filled draft with write_report, then send its exact report_id as "
            "result_submit report={report_ref: <report_id>}, or send the report inline as report={title, markdown}.",
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
            "report_not_own", "arguments.report.report_ref", "published report owned by another thread or attempt",
            "a report published from this worker's own draft in this attempt",
            "Publish the result via write_report using this worker's own draft, then send that call's exact "
            "report_id as result_submit report={report_ref: <report_id>}, or send the report inline as report={title, markdown}.",
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
        label = delegation_evidence.resolve_check(
            store, channel.task_id, assignment_ref, item.get("tool_call_ref"), item["outcome"],
        )
        entry = dict(
            name=item["name"], outcome=item["outcome"],
            tool_call_ref=item.get("tool_call_ref"), evidence=label,
        )
        # A pass the runtime did not accept keeps the claim and its gap; a
        # failure an exact later rerun closed keeps the closing ref.
        for key in ("blocker", "claimed_outcome", "evidence_gap", "closed_by"):
            if item.get(key):
                entry[key] = item[key]
        checks.append(entry)
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
            "SELECT p.workspace_binding_id FROM delegation_provisioning p "
            "WHERE p.task_id=? AND p.attempt_id=?",
            (channel.task_id, assignment_ref),
        ).fetchone()
        direct_root = task_workspace.task_root(db, channel.task_id) if direct_workspace is not None else None
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
                store, assignment_ref, phase, direct_root,
            )
    # T017: each check carries an audit label; 'observed'/'contradicted' is
    # resolved strictly against this exact attempt's own command receipts
    # (delegation.evidence.assess_check) -- a claim never becomes confirmed
    # without that link (see delegation.evidence docstring).
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
    """Read a shared skill or a declared reference for this channel's role.

    A worker's successful read is retained for its attempt: the runtime's
    situational-skill gate at result_submit (orchestration.skill_gate)
    checks it."""
    result = _read_skill_body(store, channel, args)
    from ..orchestration.skill_gate import record_skill_read
    if "/" not in result["skill"]:
        record_skill_read(store, channel, result["skill"])
    return result


def _read_skill_body(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from ..portable.roles import (
        COORDINATOR_REFERENCE_IDS, EXAMPLE_REFERENCE_IDS,
        WORKER_REFERENCE_IDS, load_role, read_reference, shared_skill_ids,
    )

    requested = args.get("skill")
    if (isinstance(requested, str) and "/" in requested
            and not (requested.startswith("references/") and requested.endswith(".md"))):
        from .session_skills import read_session_skill

        return read_session_skill(store, channel, requested)
    skill_id = requested if isinstance(requested, str) else ""
    # Skills link their references as `references/<id>.md`; accept that link form.
    skill_id = skill_id.strip().removeprefix("references/").removesuffix(".md")
    role_kind = "coordinator" if channel is not None and channel.role == "coordinator" else "worker"
    references = (COORDINATOR_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS if role_kind == "coordinator"
                  else WORKER_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS)
    allowed_shared = shared_skill_ids(role_kind, _own_role_id(store, channel))
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
    if skill_id not in allowed_shared:
        raise StoreError(
            "unknown_skill", "skill", str(requested),
            ", ".join(sorted(allowed_shared | references)),
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


def _coordinator_route_options(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from ..routing import auto_route, availability
    from ..portable.errors import PortableError

    try:
        return auto_route.route_menu(store.project_root, unavailable=availability.active(store))
    except PortableError as error:
        # Policy validation is an actionable read refusal, not an
        # indeterminate storage write in the MCP server's generic catch.
        raise StoreError(error.code, error.field or "routing",
                         correction=error.remediation) from error


_COORDINATOR_HANDLERS = dict(
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule, read_large_output=_shared_read_large_output,
    read_skill=_shared_read_skill, read_diagnostics=_coordinator_read_diagnostics,
    list_route_options=_coordinator_route_options,
)
_WORKER_HANDLERS = dict(
    ask_question=_worker_ask_question, list_sources=_shared_list_sources,
    read_source=_shared_read_source, read_capsule=_shared_read_capsule,
    read_large_output=_shared_read_large_output, read_skill=_shared_read_skill,
)
_ROLE_HANDLERS = dict(coordinator=_COORDINATOR_HANDLERS, worker=_WORKER_HANDLERS)


_OPERATION_DESCRIPTIONS = dict(
    task_get_context="Read the current runtime task, original goal, validated plan, results and user answers.",
    list_route_options="Read the live engine-confirmed model routes the owner configured, each with its tier and tier source, permitted efforts, per-route policy, automatic eligibility, availability and quota band; role tiers at low/normal/high complexity and each role's kind of work; engine affinity per kind of work (only engines with an automatically eligible route); the models the owner prefers for each kind of work, which dispatch ranks ahead of unconfirmed plan hints; under preferred_when, models the owner prefers only when a node meets a free-text condition, which you judge per node; stretch models (light models also used for some standard work) and flagship routes with their extra request names. Use exact route labels in plan_submit model. Informational: dispatch revalidates every route. Explicit user model requests, confirmed only from recorded user messages/answers, precede pins, preferences, usage ranking and automatic tier selection on covered exact-route nodes in every tier. Hard policy or unavailable routes refuse visibly without substitution. Usage switching engages only at observed usage >=95%; unknown usage is unavailable, never inferred hot. Without user consent flagship remains escalation-only.",
    answer_submit="Submit a direct answer to an informational question before any plan or workers exist, a final summary after the user chose to finish when answering a runtime stop question or a question asked after verification, or an answer to a follow-up question that reopened a completed task before any new plan. The runtime records and delivers the answer; this does not certify code changes or tests.",
    plan_submit="For changes or substantial delegated work, submit a dependency plan for runtime validation. Accepted only while the task is planning. The runtime chooses routes, schedules assignments, requires review and runs verification. plan.tasks and plan.verification each need at least one item; [] is rejected for either.",
    verification_submit="After the runtime verification failed and the work itself is fine: replace only the verification commands and verify again, without redoing the reviewed work.",
    synthesis_submit="Submit a final synthesis grounded in the retained reports and verification. Accepted only after every task, the review and the verification finished. Every retained not_run or unverified worker check must be verified first or listed in not_verified with its concrete blocker. The runtime then completes the task, so use it only when nothing is left for the user to decide; otherwise ask with user_question_request.",
    user_question_request="Always use this structured tool for user questions while planning or reconciling, never prose-only or native engine questions. Supply meaningful selectable options; group related questions with questions, instead of question/options. All batch answers are required; custom free text remains available. Write the details once in your message before this call; the question itself is one short line with short answer options. After the call end the turn without writing anything more. The runtime persists the question before elicitation and resumes only after a real answer. A question asked while reconciling (approve a proposed plan, choose how to continue) keeps the task open, and the answer returns it to planning: submit a plan for the chosen work, or answer_submit a short close when the user chose to finish. With skill_proposal it proposes one reusable project skill backed by retained report evidence. Use exactly one accepted action-matched pair, approval first: create: [Create the skill, Not now], [Создать навык, Не сейчас], [Approve creating the skill, Decline creating the skill], [Одобрить создание навыка, Отклонить создание навыка]; update: [Update the skill, Not now], [Обновить навык, Не сейчас], [Approve updating the skill, Decline updating the skill], [Одобрить обновление навыка, Отклонить обновление навыка]. Only an exact pair is accepted; 1 selects its approval label, 2 its decline label, and any other answer is unclear. An arbitrary first option never approves.",
    result_submit="Submit this worker attempt's report, summary and observed checks. report is required in every call, a review included; summary, verdict and findings never replace it. Send report in exactly one form: inline report={title, markdown}, which the runtime publishes; or, after create_draft, draft_write and write_report, report={report_ref: <that write_report's report_id>}. Pass a structured argument object; do not hand-concatenate Markdown into JSON. After a JSON parse error, correct serialization before retrying; for repeated inline serialization failures use create_draft, draft_write and write_report, then send report={report_ref: <the acknowledged report_id>}. This does not relax publication or receipt checks. Use checks=[] only if no automated command ran. Each checks item has name (a stable check name) and outcome (passed|failed|not_run|unverified; there is no status field); passed or failed needs tool_call_ref, not_run or unverified needs blocker, and command is optional as an argv array of strings. For a verified passed check, run command_run first, inspect its complete exit-0 result, and copy its tool_call_ref into one check. Engine-native shell calls and report ids are not command evidence. A checks[].command supplied with passed or failed is descriptive only and cannot verify the check; do not send closed_by, which the runtime derives. Every not_run or unverified check needs a concrete blocker. Use a fresh request_key for a new result; retry an uncertain call only with the identical key and payload. Reviewers must include verdict=APPROVE, REQUEST_CHANGES or BLOCK (lowercase forms normalize); other workers omit verdict. This cannot accept the task. Top-level write_report-only fields such as artifacts are ignored. "
                  "The runtime compares the project with how it was when this attempt started: when the changes match a "
                  "situational skill's trigger ({gates}) and this attempt has not read that skill with read_skill, the "
                  "result is refused with situational_skill_required and nothing is recorded; read the skill, re-check "
                  "the edits against it, then submit again.",
    ask_question="Only for a genuine user decision not answerable from assignment/context/runtime tools. Missing project access, permissions and work paths go to access_request for a runtime grant/refusal, never to a user question; changed decomposition goes to scope_request/replan_request. Unavailable evidence remains a report limitation. Write each question and all options in the current turn's user_question_language (en = English, ru = Russian); this is the exception to English worker communication. The runtime forwards strings verbatim, without translation. Supply meaningful selectable options or a related-question batch using questions instead of question/options. All batch answers are required; custom free text remains available. The runtime persists an assignment-local wait and returns promptly. End your current turn without publishing an incomplete result. The runtime delivers the answer as the next prompt in this same healthy engine session; siblings continue. A recovered channel without retained session metadata uses task-global question recovery instead.",
    access_request="Request additional declared work paths in flight instead of asking the user for access. Safe, policy-compliant, conflict-free requests reserve them transactionally without restarting this worker. Ordinary writers already have whole-project gateway/jail access: this does not remount or widen an OS sandbox. On pending, end the turn without an incomplete result; the runtime retries writer conflicts with bounded delays, then grants or refuses without a user question. Policy/path/role refusals are runtime decisions without retries. The runtime delivers the decision in this same healthy session. A denial does not fail the node or override private paths, owner policy, role restrictions or live writers. Re-read content/hash before a new write; never replay a refused action. Keep scope_request/replan as fallback for replacement/decomposition.",
    read_skill=(
        "Read one shared skill, role-scoped reference, or session-listed project/user SKILL.md. "
        "The input object has exactly one accepted field, `skill`; pass the exact shared id "
        "(for example skill='documentation-sync') or the exact path shown in the session's "
        "Project and user skills list (for example skill='.agents/skills/review/SKILL.md' "
        "or skill='/home/user/.codex/skills/review/SKILL.md'). No globs, sibling files, "
        "or unlisted paths are accepted. Do not send `topic`, "
        "`name`, or `id`; those fields are invalid. A worker's successful read is retained for its attempt; "
        "result_submit refuses a result whose observed changes match an unread situational skill's trigger "
        "({gates})."
    ),
    list_sources="List durable source references (user text and resolved attachments) for this task, cursor-paginated. "
                 "Every page also carries answered_questions: each question a real user answer resolved, with its "
                 "question_id, question as asked, the user's verbatim answer, timestamps and answering session.",
    read_source="Bounded, addressable read of one source_ref (text, attachment, or an answered question's "
                "<question_id>:answer) with provenance and trust label. A stored image attachment is "
                "always returned whole as a real MCP image content block (offset/limit are ignored for it); "
                "every other source stays a bounded, char-paged text/base64 read.",
    read_capsule="Read this channel's own deterministic recovery capsule: identity, applied config, exact "
                 "constraints with origins (including answered user questions), own assignments/pipeline, confirmed publications/checks, open/"
                 "uncertain actions, newer events, role/tool access, and continuation intent.",
    read_large_output="Page through an output too large to return inline (a command_run or project_git result), by its large_output_id, byte offset and limit.",
    project_read="Read a project file (UTF-8 text, paged by offset/limit; returns its sha256 for a later write or delete) "
                 "or list a directory. A supported image (PNG, JPEG, GIF, WEBP; sniffed by content, not extension) "
                 "returns a real MCP image content block plus sha256/size_bytes/mime/path as text, up to a 10 MiB cap. "
                 "Read-only.",
    project_git="Read-only git evidence for the project: `status`, or the full working-tree `diff` against HEAD including edits that were already there. Large output is retained; page it with read_large_output.",
    project_write="Create or replace one project file with complete UTF-8 content. Pass the file's current sha256 from project_read, or \"absent\" for a new file; a mismatch changes nothing. Only for a worker whose assignment may write; paths the user protected are refused.",
    project_mkdir="Create one new project directory; its parent must already exist. Only for a worker whose assignment may write.",
    project_skill_link="Expose a project skill to Claude: link makes .claude/skills/<name> the relative symlink ../../.agents/skills/<name> (Git keeps it as a symlink); unlink removes only that exact link. The only write .claude accepts; the returned git field says whether the project's .gitignore would drop the link or skill. Replacing a different existing link needs its exact text and the user's confirmation. Every worker whose assignment may write can use it; no particular declared scope is required, and a path the user protected refuses it.",
    project_delete="Delete one existing regular project file (never a directory), identified by its current sha256. The user confirms every deletion first. Only for a worker whose assignment may write.",
    command_run="Run one command (argv, no shell) in the project root inside the sandbox, as the owner, with their toolchain. Returns argv, exit_code, a tool_call_ref (the evidence a passed result_submit check must cite) and the retained output (read_large_output). Not available to coordinators or consultants.",
    outside_command="Run one command (argv, no shell) outside the sandbox as the user, for work the sandbox cannot do (a global install, a change in HOME). The user confirms it first on a permission card; a refusal means it did not run. Only for a worker whose assignment may write.",
    outside_write="Write one file OUTSIDE the project as the user (inside the project use project_write or your own tools). The user confirms it first; the parent directory must exist. Only for a worker whose assignment may write.",
    draft_write="Replace the whole content of one of your unpublished drafts (from create_draft), keeping the first line create_draft wrote. Publish it with write_report.",
    replan_request="Ask the runtime for a new plan when evidence shows the current decomposition cannot reach the goal. The task enters recovery: end this turn now and do not call plan_submit in the same turn. The runtime stops workers, then requests the new plan. At most three such requests since the user last spoke (a message or an answer), after which the user decides.",
    scope_request="Report that the assignment's scope prevents completion and name the paths it needs. The task enters recovery: end this turn now and do not call plan_submit in the same turn. The runtime stops workers, then requests the new plan; it never widens scope by itself. Counts toward the three re-plans allowed since the user last spoke.",
    escalation_request="Ask for an independent senior consultation on a problem this attempt cannot resolve. The task enters recovery: end this turn now and do not call plan_submit in the same turn. The runtime stops workers, then requests the new plan with a consultation. Counts toward the three re-plans allowed since the user last spoke.",
    read_diagnostics="Read this task's own observability/usage diagnostics summary: resource caps, recovery "
                     "latency, repeat calls, schema/validation errors, manual interventions, duplicate "
                     "publications, tokens, and concurrency -- each with its declared unit and availability.",
)


# Tools that change the project or act outside it. A channel with no write
# grant -- the coordinator, a read-only worker role, a review stage -- is
# refused every one of them by the gateway, so they are not offered to it.
_WRITE_TOOLS = frozenset({"project_write", "project_mkdir", "project_delete", "project_skill_link",
                          "outside_write", "outside_command"})


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
    # The senior consultant works only from its consultation packet and
    # retained reports: no commands and no project source reads.
    if "consultant" in role_id and name in {"command_run", "project_read", "project_git"}:
        return False
    return True


def _narrow_result_submit(schema: dict, stage: str | None) -> dict:
    """``verdict`` is required on a review stage and refused on every other:
    offer it only where it is accepted. Workers that were shown it as an
    optional field sent it anyway and failed with reviewer_only_verdict.
    ``findings`` is likewise review-only (optional there)."""
    if stage is None:
        return schema
    properties = dict(schema["properties"])
    required = [name for name in schema["required"] if name != "verdict"]
    if stage == "review":
        required.append("verdict")
    else:
        properties.pop("verdict", None)
        properties.pop("findings", None)
    return dict(schema, properties=properties, required=required)


def _enumerate_skills(schema: dict, role_kind: str, role_id: str | None) -> dict:
    """Describe role-scoped ids; path grants are checked per session at read."""
    from ..acp.orchestrating_runtime import COORDINATOR_ROLE_ID
    from ..portable.roles import (
        COORDINATOR_REFERENCE_IDS, EXAMPLE_REFERENCE_IDS,
        WORKER_REFERENCE_IDS, shared_skill_ids, situational_skill_triggers,
    )

    ids = set(shared_skill_ids(role_kind, role_id)) | EXAMPLE_REFERENCE_IDS
    ids |= COORDINATOR_REFERENCE_IDS if role_kind == "coordinator" else WORKER_REFERENCE_IDS
    own = COORDINATOR_ROLE_ID if role_kind == "coordinator" else role_id
    if own:
        ids.add(own)
    properties = dict(schema["properties"])
    triggers = situational_skill_triggers(role_kind, role_id)
    properties["skill"] = dict(
        properties["skill"],
        anyOf=[
            dict(type="string", enum=sorted(ids), description="Named Cortex shared or reference skill id for this role."),
            # Typed, or Codex code mode renders this branch as `unknown`.
            dict(type="string", description="Exact project/user SKILL.md path listed in this session; the runtime checks its session grant."),
        ],
        description=properties["skill"].get("description", "Exact skill id or listed path.")
        + " Shared/reference ids for this role: " + ", ".join(sorted(ids))
        + ". The enumerated named-Cortex branch covers the built-in shared/reference choices; the other branch "
        "accepts exact session-listed project/user SKILL.md paths and checks them against the session grant."
        + ". Situational ids and when to read them: "
        + "; ".join(f"{skill_id}: {trigger}" for skill_id, trigger in triggers)
        + ". Read only a matching skill; the always-needed skills are already included in the role instructions.",
    )
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
        description = _OPERATION_DESCRIPTIONS.get(name, f"Delegation operation '{name}'.")
        if "{gates}" in description:
            from ..orchestration.skill_gate import gate_summary
            description = description.replace("{gates}", gate_summary())
        if name == "read_skill":
            from ..portable.roles import situational_skill_triggers
            description += (
                " When one of these conditions arises during work, call read_skill with that exact id before "
                "the action and follow the returned body; do not read unrelated or already-included skills. "
                "Situational skills: "
                + "; ".join(f"{skill_id}: {trigger}" for skill_id, trigger in
                            situational_skill_triggers(role_kind, role_id)) + "."
            )
        tools.append(dict(
            name=name, title=name.replace("_", " ").capitalize(),
            description=description,
            inputSchema=dict(schema),
            annotations=dict(readOnlyHint=name in ("project_read", "project_git", "task_get_context", "list_sources", "read_source", "read_capsule", "read_large_output", "read_diagnostics", "read_skill", "list_route_options"), destructiveHint=name=="project_delete",
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
    if channel.role == 'worker' and operation not in {
            'ask_question', 'access_request', 'list_sources', 'read_source', 'read_capsule', 'read_large_output',
            'read_skill', 'read_report', 'list_reports', 'read_draft', 'project_read', 'project_git'}:
        from ..orchestration.worker_waits import check_mutation
        with store.connection() as db:
            check_mutation(db, channel)
    if operation in gateway.SCHEMAS:
        return gateway.dispatch(store, channel, operation, dict(args))
    from ..orchestration import mcp as runtime_mcp
    if operation in runtime_mcp.schemas(channel.role):
        try:
            return runtime_mcp.dispatch(store, channel, operation, dict(args))
        except StoreError as error:
            if operation == "result_submit":
                report_arg = args.get("report") if isinstance(args, Mapping) else None
                if (error.field == "arguments.report"
                        and not (isinstance(report_arg, Mapping) and "report_ref" in report_arg)):
                    with store.connection() as db:
                        rows = db.execute(
                            "SELECT r.id FROM reports r JOIN drafts d ON d.published_report_id=r.id "
                            "WHERE r.task_id=? AND d.owner_thread_id=?",
                            (channel.task_id, channel.principal_thread_id),
                        ).fetchall()
                        claimed = {
                            json.loads(row["payload_json"]).get("report_ref")
                            for row in db.execute(
                                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
                                (channel.task_id,),
                            ).fetchall()
                        }
                    candidates = [row["id"] for row in rows if row["id"] not in claimed]
                    if len(candidates) == 1:
                        error.correction += (f" This attempt published {candidates[0]}; send it as "
                                             f"report={{report_ref: {candidates[0]}}}.")
                # Retain only a code and safe field path, never rejected
                # arguments, report bodies, receipts or raw error messages.
                try:
                    from ..observability.events import append_event as _append_observability_event
                    from ..orchestration.contracts import MAX_VIOLATIONS

                    fields = [
                        item.get("field") for item in (error.violations or ())
                        if isinstance(item, dict) and isinstance(item.get("field"), str)
                        and item["field"].startswith("arguments.")
                    ]
                    if not fields:
                        fields = [error.field] if isinstance(error.field, str) and (
                            error.field.startswith("arguments.") or error.field in {"report_ref", "request_key"}
                        ) else ["arguments"]
                    _append_observability_event(
                        store, channel.task_id, "submission_rejection", str(error), "observed",
                        attempt_id=channel.attempt_id,
                        detail=dict(operation=operation, field=fields[0], fields=fields[:MAX_VIOLATIONS]),
                    )
                except Exception:
                    pass
            raise
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
