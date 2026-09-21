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
import sqlite3
from typing import Any, Mapping

from ..acp import replay_store
from ..contracts import StoreError
from ..trusted_channel import ChannelContext
from . import completion as delegation_completion
from . import contracts as delegation_contracts
from . import events as delegation_events
from . import evidence as delegation_evidence
from . import revision as delegation_revision

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


def _validate_delegate_handoff(store, channel: ChannelContext, args: Mapping[str, Any]) -> None:
    """Validate an explicit predecessor-report handoff for a new delegation.

    The free-form brief cannot reliably distinguish an independent QA run from
    a review of a previous worker.  Once this task has a worker result, a new
    delegation must therefore state that choice explicitly.  A retry of an
    already-recorded request keeps the original idempotency behavior.
    """
    role_hint = str(args.get("role", "")).strip().lower().replace("_", "-")
    # This contract is specifically for QA/review handoffs.  Other workers
    # may legitimately be delegated after an earlier result without consuming
    # or reviewing that result.
    if role_hint not in {"qa-engineer", "worker-qa-engineer"}:
        return
    request_key = args["request_key"]
    dedup_key = f"delegate:{request_key}"
    with store.connection() as db:
        existing = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key=? LIMIT 1",
            (channel.task_id, dedup_key),
        ).fetchone()
        if existing is not None:
            return
        rows = db.execute(
            "SELECT payload_json, superseded, assignment_ref FROM task_events "
            "WHERE task_id=? AND event_type='worker_result' ORDER BY id",
            (channel.task_id,),
        ).fetchall()
    worker_results = []
    for row in rows:
        if row[1]:
            continue
        try:
            payload = json.loads(row[0])
        except (TypeError, ValueError):
            continue
        report_ref = payload.get("report_ref")
        if isinstance(report_ref, str):
            worker_results.append((report_ref, row[2]))

    mode = args.get("handoff_mode")
    report_ref = args.get("predecessor_report_ref")
    if worker_results and mode is None:
        raise StoreError(
            "handoff_choice_required", "handoff_mode", "missing",
            "independent or report_review",
            "This task already has a worker result. Set handoff_mode to 'independent', "
            "or set it to 'report_review' and copy the exact predecessor_report_ref "
            "from that worker_result event.",
        )
    if mode == "independent":
        if report_ref is not None:
            raise StoreError(
                "handoff_report_unexpected", "predecessor_report_ref", "provided",
                "omitted with handoff_mode='independent'",
                "Remove predecessor_report_ref, or choose report_review.",
            )
        return
    if mode != "report_review":
        return
    if report_ref is None:
        raise StoreError(
            "predecessor_report_required", "predecessor_report_ref", "missing",
            "the exact published worker report reference",
            "Copy predecessor_report_ref unchanged from the predecessor worker_result event.",
        )
    if not any(ref == report_ref for ref, _assignment_ref in worker_results):
        with store.connection() as db:
            report = db.execute(
                "SELECT task_id FROM reports WHERE id=?", (report_ref,)
            ).fetchone()
        if report is None:
            raise StoreError(
                "unknown_predecessor_report", "predecessor_report_ref", report_ref,
                "a published worker report from this task",
                "Use the exact report_ref from a same-task worker_result event; do not guess it.",
            )
        raise StoreError(
            "report_not_worker_result", "predecessor_report_ref", report_ref,
            "a report_ref recorded by a same-task worker_result event",
            "Use the exact report_ref from the predecessor worker_result event.",
        )


def _coordinator_delegate(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    request_key = args["request_key"]
    assignment_ref = _assignment_ref(channel.task_id, channel.channel_id, request_key)
    # Some ACP clients serialize an omitted optional string as ``""``.  An
    # empty value has no assignment identity and is therefore equivalent to
    # omission; normalize it before graph lookup and durable event creation.
    # Non-empty values still go through the exact task/binding validation below.
    plan_assignment_ref = args.get("plan_assignment_ref") or None
    work_attempt_ref = None
    if plan_assignment_ref:
        from .work_graph import WorkGraph

        planned = WorkGraph(store).get_assignment(plan_assignment_ref)
        if planned is None or planned["task_id"] != channel.task_id:
            raise StoreError(
                "unknown_plan_assignment", "plan_assignment_ref", plan_assignment_ref,
                "assignment_id returned by update_plan(action='add_assignment')",
                "Use the plan node's assignment_id from update_plan/add_assignment, not the "
                "assignment_ref returned by delegate(); those references are different.",
            )
        predecessors = planned["predecessors"]
        if len(predecessors) > 1:
            raise StoreError("multiple_candidate_predecessors_not_supported")
        if predecessors:
            predecessor_ref = predecessors[0]["candidate_ref"]
            if not isinstance(predecessor_ref, str) or not predecessor_ref.startswith("candidate:"):
                raise StoreError("invalid_candidate_predecessor")
            from .workspace_isolation import IsolatedSnapshotWorkspaceProvisioner

            IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)).register_candidate_seed(
                assignment_ref, predecessor_ref.removeprefix("candidate:"), task_id=channel.task_id,
            )
        work_attempt_ref = WorkGraph(store).create_attempt(
            plan_assignment_ref,
            input_candidate_ref=predecessors[0]["candidate_ref"] if predecessors else None,
            idempotency_key=f"delegate-attempt:{assignment_ref}",
        )["attempt_id"]
    else:
        # A normal delegation is still one executable node in the durable
        # graph.  ``update_plan`` is optional for the simple one-worker
        # workflow, so create the implicit assignment here instead of
        # leaving the provisioning/event records as the only source of
        # assignment state.  Reusing the public assignment ref keeps the
        # graph and legacy delegation identities exactly aligned; both graph
        # writes are idempotent on a retried delegate request.
        from .work_graph import WorkGraph

        graph = WorkGraph(store)
        graph.add_assignment(
            channel.task_id,
            f"delegate:{request_key}",
            assignment_id=assignment_ref,
            idempotency_key=f"delegate-assignment:{assignment_ref}",
        )
        work_attempt_ref = graph.create_attempt(
            assignment_ref,
            idempotency_key=f"delegate-attempt:{assignment_ref}",
        )["attempt_id"]
    dedup_key = f"delegate:{request_key}"
    event = delegation_events.append_event(
        store, channel.task_id, "delegation_requested", dedup_key,
        dict(request_key=request_key, title=args["title"], brief=args["brief"],
             role=args["role"], acceptance=args["acceptance"],
             # D-T035-04: the coordinator's own optional route choice for
             # this delegation, carried verbatim (never defaulted here --
             # ``delegation.provisioning``/``live_adapters`` resolve an
             # omitted value against the route policy).
             engine=args.get("engine"), model=args.get("model"), effort=args.get("effort"),
             handoff_mode=args.get("handoff_mode"),
             predecessor_report_ref=args.get("predecessor_report_ref"),
             plan_assignment_ref=plan_assignment_ref, work_attempt_ref=work_attempt_ref,
             input_candidate_ref=predecessors[0]["candidate_ref"] if plan_assignment_ref and predecessors else None,
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


def _coordinator_update_plan(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from .work_graph import WorkGraph

    graph = WorkGraph(store)
    action = args["action"]
    key = f"{channel.channel_id}:{args['request_key']}"
    if action == "create_requirement":
        if "statement" not in args:
            raise StoreError("statement_required")
        result = graph.create_requirement(
            channel.task_id, args["statement"], criteria=args.get("criteria", []), idempotency_key=key,
        )
        result.pop("task_id", None)
        return result
    if action == "revise_requirement":
        if "statement" not in args or "requirement_ref" not in args:
            raise StoreError("requirement_revision_fields_required")
        with store.connection() as db:
            row = db.execute("SELECT task_id FROM work_requirements WHERE requirement_id=?",
                             (args["requirement_ref"],)).fetchone()
        if row is None or row["task_id"] != channel.task_id:
            raise StoreError("unknown_requirement")
        result = graph.revise_requirement(
            args["requirement_ref"], args["statement"], criteria=args.get("criteria", []),
            idempotency_key=key,
        )
        result.pop("task_id", None)
        result["affected_assignments"] = graph.requirement_impact(
            channel.task_id, args["requirement_ref"], revision=result["revision"],
        )
        from .check_evidence import invalidate_requirement_revision

        result["invalidated_checks"] = invalidate_requirement_revision(
            store, channel.task_id, args["requirement_ref"], result["revision"],
        )
        return result
    if action == "add_assignment":
        if "node_key" not in args:
            raise StoreError("node_key_required")
        bindings = {}
        for item in args.get("requirement_bindings", []):
            ref = item["requirement_ref"]
            if ref in bindings:
                raise StoreError("duplicate_requirement_binding")
            bindings[ref] = {"revision": int(item["revision"])}
        predecessors = [
            {"assignment_id": item["assignment_ref"], "candidate_ref": item["candidate_ref"]}
            for item in args.get("predecessors", [])
        ]
        result = graph.add_assignment(
            channel.task_id, args["node_key"], requirement_bindings=bindings,
            predecessors=predecessors, idempotency_key=key,
        )
        result.pop("task_id", None)
        return result
    if action == "bind_candidate":
        if "assignment_ref" not in args or "candidate_ref" not in args:
            raise StoreError("candidate_binding_fields_required")
        planned = graph.get_assignment(args["assignment_ref"])
        if planned is None or planned["task_id"] != channel.task_id:
            raise StoreError(
                "unknown_plan_assignment", "assignment_ref", args["assignment_ref"],
                "assignment_id returned by update_plan(action='add_assignment')",
                "bind_candidate targets the plan node. Copy assignment_id from the "
                "earlier update_plan/add_assignment result; do not use the assignment_ref "
                "returned by delegate(). The candidate_ref is the bare candidate id from "
                "prepare_candidate, without the 'candidate:' display prefix.",
            )
        result = graph.bind_candidate(args["assignment_ref"], args["candidate_ref"], idempotency_key=key)
        result.pop("task_id", None)
        return result
    raise StoreError("unknown_plan_action")


def _coordinator_read_plan(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from .work_graph import WorkGraph

    result = WorkGraph(store).snapshot(channel.task_id, limit=int(args.get("limit", 100)))
    result.pop("task_id", None)
    for assignment in result.get("assignments", []):
        assignment.pop("task_id", None)
    return result


def _coordinator_prepare_candidate(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    refs = args["assignment_refs"]
    if not refs or len(set(refs)) != len(refs):
        raise StoreError("candidate_assignments_invalid")
    # The main runtime performs filesystem staging.  The channel-verified
    # MCP dispatch persists only this request and its exact source refs.
    with store.connection() as db:
        for ref in refs:
            row = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (channel.task_id, ref),
            ).fetchone()
            if row is None or not row["attempt_id"]:
                raise StoreError("candidate_assignment_unavailable")
    candidate_id = "cand_" + hashlib.sha256(
        f"{channel.task_id}|{channel.channel_id}|{args['request_key']}".encode("utf-8")
    ).hexdigest()[:32]
    event = delegation_events.append_event(
        store, channel.task_id, "progress", f"prepare_candidate:{args['request_key']}",
        dict(kind="candidate_requested", candidate_id=candidate_id, assignment_refs=refs,
             request_key=args["request_key"]),
        origin=channel.channel_id,
    )
    return dict(candidate_id=candidate_id, status="preparing", cursor=event.id)


def _coordinator_request_check(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """Persist a request; only the main runtime executes the candidate check."""
    from .candidate_checks import _validate_argv
    from .candidates import get_candidate
    from .check_requests import enqueue_check
    from .check_evidence import _files

    candidate = get_candidate(store, args["candidate_ref"])
    if candidate is None or candidate["task_id"] != channel.task_id or candidate["status"] != "ready":
        raise StoreError("candidate_not_ready")
    if candidate["content_digest"] != args["candidate_digest"]:
        raise StoreError("candidate_digest_mismatch")
    argv = list(_validate_argv(args["argv"]))
    bindings = {}
    with store.connection() as db:
        for item in args.get("requirement_bindings", []):
            ref = item["requirement_ref"]
            revision = int(item["revision"])
            if ref in bindings:
                raise StoreError("duplicate_requirement_binding")
            row = db.execute(
                "SELECT current_revision FROM work_requirements WHERE requirement_id=? AND task_id=?",
                (ref, channel.task_id),
            ).fetchone()
            if row is None or row["current_revision"] != revision:
                raise StoreError("stale_requirement_binding")
            bindings[ref] = {"revision": revision}
    checked_files = _files(args.get("checked_files", []))
    request_key = f"{channel.task_id}:{channel.channel_id}:{args['request_key']}"
    check = enqueue_check(
        store, task_id=channel.task_id, candidate_id=candidate["candidate_id"],
        candidate_digest=args["candidate_digest"], argv=argv, request_key=request_key,
        requirement_bindings=bindings, checked_files=checked_files,
    )
    event = delegation_events.append_event(
        store, channel.task_id, "progress", f"request_check:{request_key}",
        dict(kind="candidate_check_requested", check_id=check["check_id"],
             request_key=args["request_key"]), origin=channel.channel_id,
    )
    return dict(check_ref=check["check_id"], status=check["status"], cursor=event.id)


def _check_requirement_currency(db, check: Mapping[str, Any]) -> tuple[bool, str | None]:
    """Reconcile a check with the durable plan, including interrupted invalidation.

    Requirement revision and evidence invalidation are separate writes. A
    crash between them must not let a stale observation pass assessment.
    """
    bindings = check["requirement_bindings"]
    graph_exists = db.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='work_requirements'"
    ).fetchone()
    if graph_exists is None:
        return (False, "missing_requirement_plan") if bindings else (True, None)
    if bindings:
        for ref, binding in bindings.items():
            row = db.execute(
                "SELECT current_revision FROM work_requirements WHERE requirement_id=? AND task_id=?",
                (ref, check["task_id"]),
            ).fetchone()
            if row is None or row["current_revision"] != binding.get("revision"):
                return False, f"stale_requirement:{ref}"
    else:
        # An unscoped check covers the plan as it stood when requested.
        # A new or revised requirement makes that scope ambiguous.
        changed = db.execute(
            "SELECT 1 FROM work_requirements r JOIN work_requirement_revisions v "
            "ON v.requirement_id=r.requirement_id WHERE r.task_id=? "
            "AND v.created_at>? LIMIT 1",
            (check["task_id"], check["created_at"]),
        ).fetchone()
        if changed is not None:
            return False, "plan_changed_after_check_request"
    return True, None


def _coordinator_read_check(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from .check_requests import get_check_request

    check = get_check_request(store, args["check_ref"])
    if check is None or check["task_id"] != channel.task_id:
        raise StoreError("unknown_check_ref")
    with store.connection() as db:
        applicable, invalidated_reason = _check_requirement_currency(db, check)
        assessment = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND dedup_key LIKE ? ORDER BY id DESC LIMIT 1",
            (channel.task_id, f"assess_check:{check['check_id']}:%"),
        ).fetchone()
    return dict(check_ref=check["check_id"], candidate_ref=check["candidate_id"],
                candidate_digest=check["candidate_digest"], argv=check["argv"],
                requirement_bindings=check["requirement_bindings"], checked_files=check["checked_files"],
                status=check["status"], result=check["result"], error=check["error"],
                applicable=applicable, invalidated_reason=invalidated_reason,
                assessment=json.loads(assessment["payload_json"]) if assessment else None)


def _coordinator_assess_check(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    from .check_requests import get_check_request
    from .candidates import _digest_manifest, _manifest, get_candidate

    check = get_check_request(store, args["check_ref"])
    if check is None or check["task_id"] != channel.task_id or check["status"] != "completed":
        raise StoreError("check_not_completed")
    observed = (check["result"] or {}).get("observed_result") or {}
    if args["assessment"] == "pass":
        with store.connection() as db:
            applicable, _ = _check_requirement_currency(db, check)
        if not applicable:
            raise StoreError("check_requirement_stale")
        if (observed.get("status") != "completed" or observed.get("exit_code") != 0
                or observed.get("output_truncated")):
            raise StoreError("check_observation_not_successful")
        candidate = get_candidate(store, check["candidate_id"])
        try:
            stage_digest = (_digest_manifest(_manifest(candidate["stage_path"]))
                            if candidate is not None and candidate["status"] == "ready" else None)
        except (OSError, ValueError, StoreError):
            stage_digest = None
        if (candidate is None or candidate["status"] != "ready"
                or candidate["content_digest"] != check["candidate_digest"]
                or stage_digest != check["candidate_digest"]):
            raise StoreError("check_candidate_stale")
    dedup_key = f"assess_check:{check['check_id']}:{args['request_key']}"
    payload = dict(kind="candidate_check_assessed", check_ref=check["check_id"],
                   evidence_ref=check["result"]["evidence_id"],
                   candidate_ref=check["candidate_id"], candidate_digest=check["candidate_digest"],
                   assessment=args["assessment"], rationale=args["rationale"])
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        if args["assessment"] == "pass":
            applicable, _ = _check_requirement_currency(db, check)
            if not applicable:
                raise StoreError("check_requirement_stale")
        prior = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
                           (channel.task_id, dedup_key)).fetchone()
        if prior is not None and json.loads(prior["payload_json"]) != payload:
            raise StoreError("check_assessment_delivery_conflict")
        event = delegation_events.append_event_tx(db, channel.task_id, "progress", dedup_key,
                                                  payload, origin=channel.channel_id)
        db.commit()
    return dict(check_ref=check["check_id"], assessment=args["assessment"], cursor=event.id)


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
    # A clarification to an assignment that has already finished can never
    # be read: the worker published its result and its session is done.
    # Recording one anyway and answering "recorded" reads as "delivered",
    # and live (2026-09-19) the coordinator then waited for a reply that
    # could not come -- after a conflicted integration it clarified a
    # finished assignment and the task simply stopped. Say so instead;
    # redoing the work needs a new delegation.
    with store.connection() as db:
        if delegation_completion.assignment_is_terminal(db, channel.task_id, args["assignment_ref"]):
            raise StoreError(
                "assignment_already_finished", "assignment_ref", args["assignment_ref"],
                "an assignment that has not yet produced a result, failure or cancellation",
                "This assignment has already finished, so nothing will read a clarification for it. "
                "Delegate the remaining work as a new assignment instead.",
            )
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
    revision_source = None
    pipeline_binding = None
    if decision == "accept" and assignment_ref == "task":
        # Idempotent transport retries must return the original decision even
        # if the pipeline has changed since the first call.  The runtime's
        # recheck will then reject that stale decision before host success;
        # re-running validation here must not turn a retry into a new error.
        with store.connection() as db:
            prior = db.execute(
                "SELECT id, payload_json FROM task_events "
                "WHERE task_id=? AND event_type='coordinator_decision' "
                "AND dedup_key=? LIMIT 1",
                (channel.task_id, f"decide:{args['request_key']}"),
            ).fetchone()
        if prior is not None:
            prior_payload = json.loads(prior["payload_json"])
            return {
                "assignment_ref": "task", "decision": prior_payload.get("decision", "accept"),
                "recorded": True, "replayed": True, "cursor": prior["id"],
            }
    if decision == "revise" and assignment_ref != "task":
        with store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' "
                "AND assignment_ref=? ORDER BY id DESC LIMIT 1",
                (channel.task_id, assignment_ref),
            ).fetchone()
            if row is None:
                raise StoreError("revision_assignment_unknown")
            revision_source = json.loads(row["payload_json"])
            attempt = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (channel.task_id, assignment_ref),
            ).fetchone()
            result = None if attempt is None else db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result' "
                "AND assignment_ref=? AND superseded=0 ORDER BY id DESC LIMIT 1",
                (channel.task_id, attempt["attempt_id"]),
            ).fetchone()
            if result is None:
                raise StoreError("revision_requires_published_result")
            revision_source["prior_report_ref"] = json.loads(result["payload_json"]).get("report_ref")
    if decision == "ask_user" and not args.get("question"):
        raise StoreError(
            "missing_question", "question", None, "non-empty string",
            "decision='ask_user' requires a concrete 'question' field.",
        )
    if decision == "accept" and assignment_ref != "task":
        with store.connection() as db:
            # Both ids are accepted here on purpose. A ``worker_result``
            # event is keyed by the worker's own attempt id, while every
            # coordinator-facing surface (``delegate``'s own answer,
            # ``clarify``, ``cancel``) speaks the delegation ref. Live
            # (2026-09-19) the coordinator reached for whichever one it had
            # just seen; refusing the other would only teach it that
            # accepting an assignment is a guessing game.
            has_result = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='worker_result' AND superseded=0 LIMIT 1",
                (channel.task_id, assignment_ref),
            ).fetchone()
            if has_result is None:
                has_result = db.execute(
                    "SELECT 1 FROM task_events e "
                    "JOIN delegation_provisioning p ON p.attempt_id = e.assignment_ref "
                    "WHERE e.task_id=? AND p.task_id=? AND p.idempotency_key=? "
                    "AND e.event_type='worker_result' AND e.superseded=0 LIMIT 1",
                    (channel.task_id, channel.task_id, assignment_ref),
                ).fetchone()
        if has_result is None:
            raise StoreError(
                "no_result_to_accept", "assignment_ref", assignment_ref,
                "an assignment_ref with a retained worker_result event",
                "Only accept an assignment after a (non-superseded) worker_result event exists for it.",
            )
        # A rendered/UI requirement is accepted only from the worker's
        # structured publish_result checks.  Report prose and coordinator
        # rationale are not evidence, while informational/review assignments
        # with no such requirement remain eligible for the normal assignment
        # acceptance path.
        evidence_blockers = delegation_completion.assignment_acceptance_evidence(
            store, channel.task_id, assignment_ref,
        )
        if evidence_blockers:
            raise StoreError(
                "acceptance_refused", "assignment_ref", assignment_ref,
                "a structured observed passing check for every explicit rendered/UI requirement",
                "Assignment accept refused: " + " ".join(evidence_blockers),
            )
        # Work that is not in the project cannot be accepted. Live
        # (2026-09-19) an integration was refused because the editor held
        # an unsaved buffer for that file -- nothing was written -- and the
        # coordinator accepted the assignment anyway and told the user the
        # change "was integrated into the project and accepted". It was
        # not. The refusal is already in its own continuation; this makes
        # ignoring it impossible rather than merely discouraged.
        with store.connection() as db:
            conflict = delegation_completion.latest_integration_conflicted(
                db, channel.task_id, assignment_ref)
        if conflict is not None:
            paths = ", ".join(sorted((conflict.get("conflict_paths") or {}))) or "the project"
            raise StoreError(
                "integration_conflicted", "assignment_ref", assignment_ref,
                "an assignment whose last integration actually reached the project",
                f"This assignment's last integration conflicted and wrote nothing ({paths}), so its "
                "work is not in the project. Resolve the conflict and integrate again before accepting; "
                "accepting now would report work that does not exist.",
            )
    if decision == "accept" and assignment_ref == "task":
        # T017 completion gating: refuse to even record a task-level accept
        # when it is visibly ungrounded at call time (the runtime rechecks
        # this same gate defensively before honoring an already-recorded
        # decision -- see delegation.completion module docstring).
        ok, reasons = delegation_completion.gate(
            store, channel.task_id, completing_channel_id=channel.channel_id)
        if not ok:
            raise StoreError(
                "acceptance_refused", "assignment_ref", "task",
                "every non-superseded assignment terminal, no unconsumed meaningful "
                "events, and every accepted assignment's report_ref resolved to a "
                "stored report file",
                "Task-level accept refused: " + " ".join(reasons),
            )
        # Bind the durable decision to the exact coordinator-authored
        # pipeline edition that was current when the model accepted.  A
        # later pipeline publication must force a fresh decision rather than
        # allowing the old decision to finalize against changed intent.
        with store.connection() as db:
            pipeline_binding = delegation_completion.current_pipeline_binding(db, channel.task_id)
        if pipeline_binding is None:
            raise StoreError(
                "acceptance_refused", "assignment_ref", "task",
                "a current coordinator-authored pipeline edition",
                "Task-level accept refused: publish a coordinator-authored current pipeline edition "
                "before accepting; the runtime bootstrap or a missing pipeline cannot be the acceptance basis.",
            )
    # Resolve the two public names for one worker attempt before recording an
    # assignment-level accept.  Models can see the delegation hash in one
    # response and the attempt id in another; both must address one logical
    # decision.  Once that decision exists, return it instead of appending a
    # fresh event for every retry/request_key.  This prevents a same-turn
    # model loop while retaining distinct decisions for other assignments and
    # for task-level accept/revise/ask_user.
    canonical_ref = assignment_ref
    if decision == "accept" and assignment_ref != "task":
        with store.connection() as db:
            alias = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning "
                "WHERE task_id=? AND attempt_id=? LIMIT 1",
                (channel.task_id, assignment_ref),
            ).fetchone()
            if alias is not None:
                canonical_ref = alias["idempotency_key"]
            refs = (canonical_ref,) if canonical_ref == assignment_ref else (canonical_ref, assignment_ref)
            # SQLite stores the decision reference in payload_json rather than
            # a dedicated decision_ref column.  Keep the lookup bounded to
            # this task's coordinator decisions and parse only their small
            # payloads, avoiding a schema migration for an idempotency fix.
            rows = db.execute(
                "SELECT id, assignment_ref, payload_json FROM task_events "
                "WHERE task_id=? AND event_type='coordinator_decision' ORDER BY id DESC LIMIT 200",
                (channel.task_id,),
            ).fetchall()
        for row in rows:
            try:
                prior = json.loads(row["payload_json"])
            except (TypeError, ValueError):
                continue
            if prior.get("decision") == "accept" and prior.get("assignment_ref") in refs:
                return {
                    "assignment_ref": assignment_ref,
                    "decision": "accept",
                    "recorded": True,
                    "replayed": True,
                    "cursor": row["id"],
                    "next_step": (
                        "Assignment decision already recorded. If the overall task is complete, "
                        "record a separate task-level accept decision after reconciling all results."
                    ),
                }
    dedup_key = f"decide:{args['request_key']}"
    payload = dict(assignment_ref=assignment_ref, decision=decision, rationale=args["rationale"],
                   question=args.get("question"), options=args.get("options"),
                   pipeline_binding=pipeline_binding)
    if decision == "accept" and assignment_ref == "task":
        # The assignment-level accept's own reminder is already answered by
        # this explicit task decision. Discharge only exact, linked reminders
        # in the same transaction that records the decision. A genuine new
        # worker/user event committed after the initial gate must still win.
        with store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                notices = delegation_completion.own_assignment_accept_notices(
                    db, channel.task_id, channel.channel_id)
                pending = db.execute(
                    "SELECT * FROM task_events WHERE task_id=? AND consumed_by IS NULL "
                    "AND superseded=0", (channel.task_id,),
                ).fetchall()
                if any(row["id"] not in notices and delegation_events.is_meaningful(
                        delegation_events.TaskEvent._from_row(row)) for row in pending):
                    raise StoreError(
                        "acceptance_refused", "assignment_ref", "task",
                        "no unconsumed meaningful worker or user events",
                        "Task-level accept refused: a new meaningful event arrived before the decision was recorded.",
                    )
                if notices:
                    db.executemany(
                        "UPDATE task_events SET superseded=1 WHERE task_id=? AND id=? "
                        "AND consumed_by IS NULL AND superseded=0",
                        [(channel.task_id, notice_id) for notice_id in notices],
                    )
                event = delegation_events.append_event_tx(
                    db, channel.task_id, "coordinator_decision", dedup_key, payload,
                    origin=channel.channel_id, assignment_ref=assignment_ref,
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
    else:
        event = delegation_events.append_event(
            store, channel.task_id, "coordinator_decision", dedup_key, payload,
            origin=channel.channel_id, assignment_ref=assignment_ref,
        )
    response = dict(assignment_ref=assignment_ref, decision=decision, recorded=True, cursor=event.id)
    if decision == "accept" and assignment_ref != "task":
        # Assignment acceptance is evidence about one worker only; it is not
        # the task-level completion decision.  Persist a canonical,
        # idempotent continuation marker so a model that chose the narrower
        # decision receives one durable correction on its next continuation.
        # The marker is keyed by the canonical assignment reference rather
        # than the model's request_key, preventing repeated retries (or a
        # delegation-vs-attempt alias) from creating a decision loop.
        delegation_events.append_event(
            store, channel.task_id, "progress",
            f"assignment-accept-followup:{canonical_ref}",
            dict(
                kind="assignment_accept_requires_task",
                assignment_ref=canonical_ref,
                decision_event_id=event.id,
            ),
            origin="runtime",
            assignment_ref=canonical_ref,
        )
        response["next_step"] = (
            "Assignment decision recorded. If the overall task is complete, "
            "record a separate task-level accept decision after reconciling all results."
        )
    if revision_source is not None:
        revision_request_key = f"revise:{args['request_key']}"
        next_ref = _assignment_ref(channel.task_id, channel.channel_id, revision_request_key)
        try:
            with store.connection() as db:
                seed_row = db.execute(
                    "SELECT candidate_id FROM workspace_candidate_seeds WHERE assignment_ref=? AND task_id=?",
                    (assignment_ref, channel.task_id),
                ).fetchone()
        except sqlite3.OperationalError:
            seed_row = None
        if seed_row is not None:
            from .workspace_isolation import IsolatedSnapshotWorkspaceProvisioner

            IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)).register_candidate_seed(
                next_ref, seed_row["candidate_id"], task_id=channel.task_id,
            )
        prior_report = revision_source.get("prior_report_ref")
        next_work_attempt_ref = None
        if revision_source.get("plan_assignment_ref"):
            from .work_graph import WorkGraph

            graph = WorkGraph(store)
            previous_work_attempt_ref = revision_source.get("work_attempt_ref")
            if previous_work_attempt_ref is None:
                raise StoreError("revision_work_attempt_missing")
            graph.fail_attempt(
                previous_work_attempt_ref, args.get("revision_brief") or args["rationale"],
                status="needs_revision", idempotency_key=f"revise-needs-work:{args['request_key']}",
            )
            next_work_attempt_ref = graph.create_attempt(
                revision_source["plan_assignment_ref"],
                predecessor_attempt_id=previous_work_attempt_ref,
                input_candidate_ref=revision_source.get("input_candidate_ref"),
                idempotency_key=f"revise-work-attempt:{args['request_key']}",
            )["attempt_id"]
        original_brief = revision_source.get("original_brief", revision_source["brief"])
        brief = (f"Continue assignment {assignment_ref}. Review previous report {prior_report}. "
                 f"Coordinator feedback: {args.get('revision_brief') or args['rationale']}\n"
                 f"Original brief: {original_brief}")
        delegation_events.append_event(
            store, channel.task_id, "delegation_requested", f"delegate:{revision_request_key}",
            dict(request_key=revision_request_key, title=revision_source["title"], brief=brief,
                 role=revision_source["role"], acceptance=revision_source["acceptance"],
                 engine=revision_source.get("engine"), model=revision_source.get("model"),
                 effort=revision_source.get("effort"),
                 plan_assignment_ref=revision_source.get("plan_assignment_ref"),
                 work_attempt_ref=next_work_attempt_ref,
                 original_brief=original_brief,
                 supersedes_assignment_ref=assignment_ref, prior_report_ref=prior_report,
                 assignment_ref=next_ref, coordinator_channel_id=channel.channel_id),
            origin=channel.channel_id, assignment_ref=next_ref,
        )
        response["continuation_ref"] = next_ref
    return response


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
    candidate_ref = args.get("candidate_ref")
    expected_base_digest = args.get("expected_base_digest")
    if bool(candidate_ref) != bool(expected_base_digest):
        raise StoreError("candidate_base_binding_required")
    dedup_key = f"integrate:{args['request_key']}"
    event = delegation_events.append_event(
        store, channel.task_id, "progress", dedup_key,
        dict(kind="integration_requested", assignment_ref=assignment_ref, request_key=args["request_key"],
             candidate_ref=candidate_ref, expected_base_digest=expected_base_digest),
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


def _assignment_requires_publish_checks(store, task_id: str, assignment_ref: str) -> bool:
    """Return whether this assignment's durable brief requires UI evidence.

    ``completion`` owns the conservative criterion classifier used by the
    acceptance gate.  Keep publication on the same classifier so a worker
    cannot publish a result that the coordinator would later be unable to
    assess, while ordinary informational assignments remain unchanged.
    """
    with store.connection() as db:
        if delegation_completion._assignment_requires_rendered_check(
            db, task_id, assignment_ref,
        ):
            return True
        return False


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
    # Preserve the raw presence of ``checks`` for the fail-fast criterion
    # below.  Once normalized, omitted and explicitly empty arrays are both
    # ``[]``; the field-level error should still explain which input was
    # missing to the model.
    raw_checks = args.get("checks")
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
    # A worker may publish an informational/review report without structured
    # checks.  An assignment whose durable brief explicitly promises rendered
    # or UI evidence is different: accepting prose as a substitute would
    # allow the immutable worker_result to lose the only machine-checkable
    # proof.  Reject before appending that event, while retaining the normal
    # idempotent replay/conflict behavior for a report that was already
    # published successfully.
    if existing is None and not checks and _assignment_requires_publish_checks(
        store, channel.task_id, assignment_ref,
    ):
        raise StoreError(
            "checks_required", "checks",
            "missing" if raw_checks is None else "empty list",
            "a non-empty array with a structured check name, outcome, and the exact tool_call_ref when a tool call produced the evidence",
            "This assignment explicitly requires rendered/UI evidence. Add one checks item naming the exact criterion, set outcome to passed, failed, not_run, or unverified, and copy the exact tool_call_ref from this attempt when available; report prose alone is insufficient.",
        )
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


def _shared_read_skill(store, channel: ChannelContext, args: Mapping[str, Any]) -> dict:
    """Read a shared skill or a declared reference for this channel's role."""
    from ..portable.roles import (
        COORDINATOR_REFERENCE_IDS, EXAMPLE_REFERENCE_IDS, SHARED_ROLE_IDS,
        WORKER_REFERENCE_IDS, load_role, read_reference,
    )

    requested = args.get("skill")
    skill_id = requested if isinstance(requested, str) else ""
    skill_id = skill_id.strip().removeprefix("cortex:").removesuffix(".md")
    role_kind = "coordinator" if channel is not None and channel.role == "coordinator" else "worker"
    references = (COORDINATOR_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS if role_kind == "coordinator"
                  else WORKER_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS)
    if skill_id in references:
        return dict(skill=skill_id, title=skill_id.replace("-", " ").capitalize(),
                    body=read_reference(skill_id, role_kind=role_kind))
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
    delegate=_coordinator_delegate, list_events=_coordinator_list_events,
    update_plan=_coordinator_update_plan, read_plan=_coordinator_read_plan,
    prepare_candidate=_coordinator_prepare_candidate,
    request_check=_coordinator_request_check, read_check=_coordinator_read_check,
    assess_check=_coordinator_assess_check,
    clarify=_coordinator_clarify, cancel=_coordinator_cancel, decide=_coordinator_decide,
    integrate=_coordinator_integrate,
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule, read_large_output=_shared_read_large_output,
    read_skill=_shared_read_skill,
    read_diagnostics=_coordinator_read_diagnostics,
)
_WORKER_HANDLERS = dict(
    ask_question=_worker_ask_question, publish_result=_worker_publish_result,
    list_sources=_shared_list_sources, read_source=_shared_read_source,
    read_capsule=_shared_read_capsule, read_large_output=_shared_read_large_output,
    read_skill=_shared_read_skill,
)
_ROLE_HANDLERS = dict(coordinator=_COORDINATOR_HANDLERS, worker=_WORKER_HANDLERS)


_OPERATION_DESCRIPTIONS = dict(
    update_plan=(
        "Create or revise this task's requirements and assignment graph, or bind an exact ready "
        "candidate. For bind_candidate, assignment_ref is the assignment_id returned by "
        "update_plan/action='add_assignment', not the assignment_ref returned by delegate(); "
        "candidate_ref is the bare candidate id returned by prepare_candidate, without the "
        "'candidate:' display prefix. The first call must contain action='create_requirement', a statement, and "
        "the required request_key; criteria is optional. Later calls select one of "
        "revise_requirement, add_assignment, or bind_candidate and provide the exact fields "
        "described by that action. Do not send title, summary, body, or other legacy aliases: "
        "all fields are closed by inputSchema.additionalProperties=false."
    ),
    read_plan="Read the bounded current requirement and assignment graph for this task.",
    prepare_candidate="Stage selected finished assignment attempts into an isolated candidate for review and checks.",
    request_check=(
        "Request one bounded read-only check against an exact ready candidate. Before this "
        "first call, read_plan and include every applicable current requirement revision in "
        "requirement_bindings; an unbound check followed by a bound repeat is unnecessary. "
        "The runtime may reuse an already assessed pass only when candidate digest, argv, "
        "and checked-file scope match exactly, and marks that result as reused in read_check."
    ),
    delegate="Delegate one bounded unit of work to a Cortex-managed worker; "
             "never construct a technical id, the runtime fills every binding.",
    list_events="List durable delegation events visible to this coordinator, cursor-paginated.",
    clarify="Send a clarification/steering message to a previously delegated worker.",
    cancel="Cancel a previously delegated assignment; late results are recorded superseded.",
    decide=(
        "Record an acceptance/revise/ask_user decision grounded in retained results. "
        "Every call must include assignment_ref: copy it exactly from the relevant delegate "
        "result for an assignment decision, or set assignment_ref='task' only for the final "
        "task-level decision. Do not omit the target. "
        "When the user explicitly requests a tool question, use ask_user with its "
        "question and optional choices; supported hosts may render a form, while "
        "others receive one ordinary text question and wait for the reply."
    ),
    integrate="Request technical application of a delegated worker's isolated workspace changes onto "
              "the main project root, against its known base; idempotent, conflict-checked, never "
              "last-write-wins -- the outcome (applied/conflict/noop) appears as a later durable event.",
    ask_question="Ask the delegating coordinator one bounded question; never ask the user directly.",
    publish_result=(
        "Publish this assignment's result via an already-acknowledged report reference. "
        "The write_report receipt calls that reference report_id; pass its exact value "
        "as publish_result's report_ref. Include the `checks` array in this call: add one "
        "item for every acceptance check you actually attempted, with the exact check "
        "name, outcome, and the matching tool_call_ref copied from this worker attempt "
        "when a tool result is the evidence. Use an empty array only when the assignment "
        "has no evidence-checkable acceptance criterion; report prose alone never "
        "establishes a check."
    ),
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
    read_diagnostics="Read this task's own observability/usage diagnostics summary: resource caps, recovery "
                     "latency, repeat calls, schema/validation errors, manual interventions, duplicate "
                     "publications, tokens, and concurrency -- each with its declared unit and availability.",
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
            annotations=dict(readOnlyHint=name in ("read_plan", "list_events", "list_sources", "read_source", "read_capsule", "read_check", "read_large_output", "read_diagnostics", "read_skill"), destructiveHint=False,
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
    try:
        validated = delegation_contracts.validate(role_kind, operation, dict(args))
        if role_kind == "coordinator" and operation == "delegate":
            _validate_delegate_handoff(store, channel, validated)
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
