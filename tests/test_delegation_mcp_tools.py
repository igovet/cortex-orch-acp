"""T016: delegation.mcp_tools -- role-filtered delegation operation handlers.

Exercises the handler layer directly (in-process), the same way the T016
fake-model tests in test_orchestrating_runtime.py call it: never a real MCP
subprocess, never a real engine. Every Store is rooted at tmp_path.
"""
from __future__ import annotations

import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    coordinator = resolve_channel(store, result.channel_id, result.secret)
    return store, result.task_id, coordinator


def _worker_channel(store, task_id, coordinator, attempt_id="xa_test1"):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", epoch, parent_channel_id=coordinator.channel_id,
            assignment_id="a_test1", attempt_id=attempt_id,
        )
        db.commit()
    return resolve_channel(store, channel_id, secret)


def _report(store, task_id, report_id="r_" + "a" * 12):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO reports VALUES (?,?,?,?,?)",
            (report_id, task_id, "report", "reports/x.md", store._now()),
        )
        db.commit()
    return report_id


def _worker_report(store, worker, body="Worker's own result.", request_key=None):
    """T019: a report published the real way, from this exact worker
    channel's own draft (trusted path -- see server._dispatch_channeled),
    so ``drafts.owner_thread_id`` binds it to ``worker.principal_thread_id``."""
    from pathlib import Path

    key = request_key or uuid.uuid4().hex
    draft = store.call(
        "create_draft", dict(template="general", request_key="draft-" + key),
        worker.principal_thread_id, worker.parent_principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + body.encode())
    result = store.call(
        "write_report",
        dict(title="Work", summary="A concise result.", author="worker",
             draft_id=draft["draft_id"], request_key=key),
        worker.principal_thread_id, worker.parent_principal_thread_id, trusted=True,
    )
    return result["report_id"]


def test_delegate_returns_deterministic_assignment_ref_and_is_idempotent(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    args = dict(title="T", brief="B", role="backend-dev", acceptance="A", request_key="k1")
    first = mcp_tools.dispatch(store, coordinator, "delegate", args)
    second = mcp_tools.dispatch(store, coordinator, "delegate", args)
    assert first["assignment_ref"] == second["assignment_ref"]
    assert first["status"] == "provisioning"
    events = delegation_events.list_since(store, task_id)
    assert len([e for e in events if e.event_type == "delegation_requested"]) == 1
    with store.connection() as db:
        assignment = db.execute(
            "SELECT assignment_id,node_key,status FROM work_assignments WHERE task_id=?",
            (task_id,),
        ).fetchone()
        attempt = db.execute(
            "SELECT assignment_id,attempt_no,status FROM work_assignment_attempts WHERE task_id=?",
            (task_id,),
        ).fetchone()
    assert tuple(assignment) == (first["assignment_ref"], "delegate:k1", "planned")
    assert tuple(attempt) == (first["assignment_ref"], 1, "planned")
    # A retry must not create a second graph node or attempt.
    with store.connection() as db:
        assert db.execute(
            "SELECT COUNT(*) FROM work_assignments WHERE task_id=?", (task_id,)
        ).fetchone()[0] == 1
        assert db.execute(
            "SELECT COUNT(*) FROM work_assignment_attempts WHERE task_id=?", (task_id,)
        ).fetchone()[0] == 1


def test_delegate_empty_optional_plan_reference_uses_implicit_assignment(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    result = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Implement", brief="Do the work", role="backend-dev", acceptance="It works",
        plan_assignment_ref="", request_key="empty-plan-reference",
    ))
    assert result["assignment_ref"]
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? "
            "AND event_type='delegation_requested' AND assignment_ref=?",
            (task_id, result["assignment_ref"]),
        ).fetchone()
    assert json.loads(row["payload_json"])["plan_assignment_ref"] is None


def test_delegate_requires_explicit_handoff_choice_after_worker_result(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="qa-source")
    report_ref = _worker_report(store, worker)
    mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=report_ref, summary="Source worker result.", request_key="publish-qa-source",
    ))
    with pytest.raises(StoreError, match="handoff_choice_required"):
        mcp_tools.dispatch(store, coordinator, "delegate", dict(
            title="Review", brief="Review the implementation.", role="qa-engineer",
            acceptance="Publish findings.", request_key="qa-missing-handoff",
        ))
    independent = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Independent QA", brief="Test the current project.", role="qa-engineer",
        acceptance="Publish findings.", handoff_mode="independent",
        request_key="qa-independent-handoff",
    ))
    assert independent["assignment_ref"]
    with pytest.raises(StoreError, match="handoff_report_unexpected"):
        mcp_tools.dispatch(store, coordinator, "delegate", dict(
            title="Independent QA", brief="Test the current project.", role="qa-engineer",
            acceptance="Publish findings.", handoff_mode="independent",
            predecessor_report_ref=report_ref, request_key="qa-independent-with-report",
        ))


def test_delegate_accepts_exact_worker_report_handoff_and_persists_it(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="frontend-source")
    report_ref = _worker_report(store, worker)
    mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=report_ref, summary="Source worker result.", request_key="publish-frontend-source",
    ))
    delegated = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Review implementation", brief="Review the predecessor result.", role="qa-engineer",
        acceptance="Publish an independent review.", handoff_mode="report_review",
        predecessor_report_ref=report_ref, request_key="qa-exact-report",
    ))
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='delegation_requested'", (task_id, delegated["assignment_ref"]),
        ).fetchone()
    payload = json.loads(row["payload_json"])
    assert payload["handoff_mode"] == "report_review"
    assert payload["predecessor_report_ref"] == report_ref
    # Retrying the exact request remains idempotent after the handoff was recorded.
    assert mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Review implementation", brief="Review the predecessor result.", role="qa-engineer",
        acceptance="Publish an independent review.", handoff_mode="report_review",
        predecessor_report_ref=report_ref, request_key="qa-exact-report",
    )) == delegated


def test_report_review_handoff_is_event_bound_not_candidate_graph_predecessor(tmp_path):
    """A report handoff and a candidate graph edge are different relations.

    ``report_review`` must retain the exact published report reference on the
    delegation event.  It must not synthesize a work-graph predecessor: that
    table is reserved for exact candidate references selected by
    ``update_plan``.  This keeps a delegation made before planning truthful.
    """
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="source-for-review")
    report_ref = _worker_report(store, worker)
    mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=report_ref, summary="Source worker result.", request_key="publish-source-for-review",
    ))
    delegated = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Review implementation", brief="Review the predecessor result.", role="qa-engineer",
        acceptance="Publish an independent review.", handoff_mode="report_review",
        predecessor_report_ref=report_ref, request_key="qa-report-edge-semantics",
    ))
    with store.connection() as db:
        event = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='delegation_requested'", (task_id, delegated["assignment_ref"]),
        ).fetchone()
        graph_edges = db.execute(
            "SELECT COUNT(*) FROM work_assignment_predecessors "
            "WHERE task_id=? AND assignment_id=?", (task_id, delegated["assignment_ref"]),
        ).fetchone()[0]
    assert json.loads(event["payload_json"])["predecessor_report_ref"] == report_ref
    assert graph_edges == 0


def test_delegation_before_plan_does_not_retroactively_bind_later_plan_node(tmp_path):
    """Implicit delegation remains an unbound simple node until the model plans.

    ``delegate`` is intentionally usable for a simple one-worker task.  A
    later ``update_plan`` call creates a separate model-chosen node; the
    runtime must not infer that it is the already-running worker or copy
    requirements into it after the fact.
    """
    store, task_id, coordinator = _task(tmp_path)
    delegated = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Implement", brief="Implement the page.", role="frontend-dev",
        acceptance="The page works.", request_key="delegate-before-plan",
    ))
    requirement = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="create_requirement", statement="The page works.", criteria=["The page renders"],
        request_key="requirement-after-delegation",
    ))
    planned = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="add_assignment", node_key="implementation",
        requirement_bindings=[dict(requirement_ref=requirement["requirement_id"], revision="1")],
        request_key="plan-after-delegation",
    ))
    with store.connection() as db:
        rows = db.execute(
            "SELECT assignment_id,requirement_bindings_json FROM work_assignments "
            "WHERE task_id=? ORDER BY created_at,assignment_id", (task_id,),
        ).fetchall()
        orchestration = db.execute(
            "SELECT requirement_revision FROM task_orchestration WHERE task_id=?", (task_id,)
        ).fetchone()
    by_id = {row[0]: json.loads(row[1]) for row in rows}
    assert by_id[delegated["assignment_ref"]] == {}
    assert by_id[planned["assignment_id"]] == {
        requirement["requirement_id"]: {"revision": 1}
    }
    # This counter tracks user steering, not work-graph revisions.  A fresh
    # task may not have provisioned its lifecycle row yet; capsule recovery
    # treats that absent row as revision zero.
    assert orchestration is None or orchestration["requirement_revision"] == 0


def test_delegate_rejects_unpublished_or_wrong_task_report_handoff(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="source-worker")
    source_report = _worker_report(store, worker)
    mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=source_report, summary="Source worker result.", request_key="publish-source-worker",
    ))
    unpublished = _report(store, task_id, "r_" + "b" * 12)
    with pytest.raises(StoreError, match="report_not_worker_result"):
        mcp_tools.dispatch(store, coordinator, "delegate", dict(
            title="Review", brief="Review the result.", role="qa-engineer",
            acceptance="Publish findings.", handoff_mode="report_review",
            predecessor_report_ref=unpublished, request_key="qa-unpublished-report",
        ))

    other_root = tmp_path / "other"
    other_root.mkdir()
    other_store, other_task_id, other_coordinator = _task(other_root)
    other_worker = _worker_channel(other_store, other_task_id, other_coordinator, attempt_id="other-worker")
    other_report = _worker_report(other_store, other_worker)
    mcp_tools.dispatch(other_store, other_worker, "publish_result", dict(
        report_ref=other_report, summary="Other worker result.", request_key="publish-other-worker",
    ))
    with pytest.raises(StoreError, match="unknown_predecessor_report"):
        mcp_tools.dispatch(store, coordinator, "delegate", dict(
            title="Review", brief="Review the result.", role="qa-engineer",
            acceptance="Publish findings.", handoff_mode="report_review",
            predecessor_report_ref=other_report, request_key="qa-wrong-task-report",
        ))


def test_plan_operations_are_role_scoped_and_hide_task_identity(tmp_path):
    store, _task_id, coordinator = _task(tmp_path)
    requirement = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="create_requirement", statement="Change the page",
        criteria=["Page renders"], request_key="requirement-1",
    ))
    assert requirement["revision"] == 1
    assert "task_id" not in requirement
    planned = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="add_assignment", node_key="implementation",
        requirement_bindings=[dict(requirement_ref=requirement["requirement_id"], revision="1")],
        request_key="assignment-1",
    ))
    assert "task_id" not in planned
    snapshot = mcp_tools.dispatch(store, coordinator, "read_plan", {})
    assert "task_id" not in snapshot
    assert snapshot["assignments"][0]["assignment_id"] == planned["assignment_id"]
    assert "task_id" not in snapshot["assignments"][0]
    delegated = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Implement", brief="Change the page", role="backend-dev", acceptance="Page renders",
        plan_assignment_ref=planned["assignment_id"], request_key="planned-delegate-1",
    ))
    assert mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Implement", brief="Change the page", role="backend-dev", acceptance="Page renders",
        plan_assignment_ref=planned["assignment_id"], request_key="planned-delegate-1",
    )) == delegated
    with store.connection() as db:
        requested = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='delegation_requested'", (coordinator.task_id, delegated["assignment_ref"]),
        ).fetchone()
        work_attempt_id = json.loads(requested["payload_json"])["work_attempt_ref"]
        attempt = db.execute(
            "SELECT assignment_id,attempt_no,status FROM work_assignment_attempts WHERE attempt_id=?",
            (work_attempt_id,),
        ).fetchone()
    assert tuple(attempt) == (planned["assignment_id"], 1, "planned")
    revised = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="revise_requirement", requirement_ref=requirement["requirement_id"],
        statement="Change the page and document it", criteria=["Page renders", "Docs updated"],
        request_key="requirement-2",
    ))
    assert revised["revision"] == 2
    assert revised["affected_assignments"][0]["assignment_id"] == planned["assignment_id"]
    worker = _worker_channel(store, coordinator.task_id, coordinator)
    with pytest.raises(StoreError, match="unknown_operation"):
        mcp_tools.dispatch(store, worker, "read_plan", {})


def test_bind_candidate_explains_plan_assignment_vs_delegate_reference(tmp_path):
    store, _task_id, coordinator = _task(tmp_path)
    requirement = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="create_requirement", statement="Change the page", request_key="bind-requirement",
    ))
    planned = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="add_assignment", node_key="implementation",
        requirement_bindings=[dict(requirement_ref=requirement["requirement_id"], revision="1")],
        request_key="bind-assignment",
    ))
    delegated = mcp_tools.dispatch(store, coordinator, "delegate", dict(
        title="Implement", brief="Change the page", role="backend-dev", acceptance="Page renders",
        plan_assignment_ref=planned["assignment_id"], request_key="delegate-for-bind-guidance",
    ))
    with pytest.raises(StoreError, match="unknown_plan_assignment") as excinfo:
        mcp_tools.dispatch(store, coordinator, "update_plan", dict(
            action="bind_candidate", assignment_ref=delegated["assignment_ref"],
            candidate_ref="cand_example", request_key="bind-guidance",
        ))
    error = excinfo.value
    assert error.field == "assignment_ref"
    assert "assignment_id returned by update_plan(action='add_assignment')" in error.expected
    assert "do not use the assignment_ref returned by delegate()" in error.correction


def test_check_request_binds_current_requirement_and_revision_invalidates_only_affected_evidence(tmp_path):
    from cortex_runtime.delegation import candidates, check_evidence

    store, task_id, coordinator = _task(tmp_path)
    requirements = []
    for index in (1, 2):
        requirements.append(mcp_tools.dispatch(store, coordinator, "update_plan", dict(
            action="create_requirement", statement=f"Requirement {index}",
            request_key=f"requirement-{index}",
        ))["requirement_id"])
    candidate_id = "cand_test"
    digest = "a" * 64
    candidates._ensure_schema(store)
    with store.connection() as db:
        db.execute(
            "INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,attempt_ids_json,"
            "source_bindings_json,base_identity_json,project_base_manifest_json,project_base_digest,"
            "content_digest,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            (candidate_id, task_id, str(tmp_path), str(tmp_path / "staged"), "[]", "[]", "[]", "{}",
             "b" * 64, digest, "ready", store._now()),
        )
    request = mcp_tools.dispatch(store, coordinator, "request_check", dict(
        candidate_ref=candidate_id, candidate_digest=digest, argv=["/usr/bin/true"],
        requirement_bindings=[dict(requirement_ref=requirements[0], revision="1")],
        checked_files=["app.py"], request_key="check-request",
    ))
    check = mcp_tools.dispatch(store, coordinator, "read_check", dict(
        check_ref=request["check_ref"],
    ))
    assert check["requirement_bindings"] == {requirements[0]: {"revision": 1}}
    assert check["checked_files"] == ["app.py"]
    ids = []
    for index, bindings in enumerate(({requirements[0]: {"revision": 1}},
                                      {requirements[1]: {"revision": 1}}, {})):
        evidence = check_evidence.record_check_evidence(
            store, task_id, f"attempt-{index}", candidate_id, digest, bindings, {},
            {"argv": ["/usr/bin/true"]}, {"status": "completed", "exit_code": 0},
            {"status": "pass"}, idempotency_key=f"evidence-{index}",
        )
        ids.append(evidence["evidence_id"])
    revision = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="revise_requirement", requirement_ref=requirements[0],
        statement="Requirement 1 revised", request_key="requirement-revision",
    ))
    assert set(revision["invalidated_checks"]) == {ids[0], ids[2]}
    assert check_evidence.get_check_evidence(store, ids[1])["applicable"] == 1
    stale_check = mcp_tools.dispatch(store, coordinator, "read_check", dict(
        check_ref=request["check_ref"],
    ))
    assert stale_check["applicable"] is False
    assert stale_check["invalidated_reason"] == f"stale_requirement:{requirements[0]}"
    with store.connection() as db:
        db.execute("UPDATE check_requests SET status='completed',result_json=? WHERE check_id=?", (
            json.dumps({"evidence_id": "observed", "observed_result": {
                "status": "completed", "exit_code": 0, "output_truncated": False,
            }}), request["check_ref"],
        ))
    with pytest.raises(StoreError, match="check_requirement_stale"):
        mcp_tools.dispatch(store, coordinator, "assess_check", dict(
            check_ref=request["check_ref"], assessment="pass", rationale="Pass",
            request_key="stale-assessment",
        ))
    unscoped = mcp_tools.dispatch(store, coordinator, "request_check", dict(
        candidate_ref=candidate_id, candidate_digest=digest, argv=["/usr/bin/true"],
        request_key="unscoped-check-request",
    ))
    mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="revise_requirement", requirement_ref=requirements[1],
        statement="Requirement 2 revised", request_key="requirement-revision-2",
    ))
    unscoped_read = mcp_tools.dispatch(store, coordinator, "read_check", dict(
        check_ref=unscoped["check_ref"],
    ))
    assert unscoped_read["applicable"] is False
    assert unscoped_read["invalidated_reason"] == "plan_changed_after_check_request"


def test_delegate_carries_optional_engine_model_effort_into_the_event_payload(tmp_path):
    # D-T035-04: the coordinator's own explicit route choice for this
    # delegation must reach the durable event unchanged -- resolved later
    # by delegation.provisioning/live_adapters, never defaulted here.
    store, task_id, coordinator = _task(tmp_path)
    args = dict(title="T", brief="B", role="backend-dev", acceptance="A", request_key="k1",
                engine="codex-acp", model="gpt-5.6-terra", effort="medium")
    mcp_tools.dispatch(store, coordinator, "delegate", args)
    events = delegation_events.list_since(store, task_id)
    requested = next(e for e in events if e.event_type == "delegation_requested")
    assert requested.payload["engine"] == "codex-acp"
    assert requested.payload["model"] == "gpt-5.6-terra"
    assert requested.payload["effort"] == "medium"


def test_delegate_without_engine_carries_none_into_the_event_payload(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    args = dict(title="T", brief="B", role="backend-dev", acceptance="A", request_key="k2")
    mcp_tools.dispatch(store, coordinator, "delegate", args)
    events = delegation_events.list_since(store, task_id)
    requested = next(e for e in events if e.event_type == "delegation_requested")
    assert requested.payload["engine"] is None
    assert requested.payload["model"] is None
    assert requested.payload["effort"] is None


def test_worker_channel_cannot_call_coordinator_operations(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "delegate", dict(title="T", brief="B", role="backend-dev", acceptance="A", request_key="k"))
    assert str(excinfo.value) == "unknown_operation"


def test_coordinator_channel_cannot_call_worker_operations(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "ask_question", dict(question="huh?", request_key="k"))
    assert str(excinfo.value) == "unknown_operation"


def test_decide_accept_requires_an_existing_worker_result(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "decide", dict(assignment_ref="ax", decision="accept", rationale="looks done", request_key="k"))
    assert str(excinfo.value) == "no_result_to_accept"


def test_decide_accept_succeeds_after_worker_result(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "worker_result", "wr1", dict(report_ref="r_x", summary="done"),
        origin="worker-ch", assignment_ref="ax",
    )
    result = mcp_tools.dispatch(store, coordinator, "decide", dict(assignment_ref="ax", decision="accept", rationale="looks done", request_key="k"))
    assert result["recorded"] is True


@pytest.mark.parametrize(
    "checks,accepted",
    [
        ([], False),
        ([{"name": "rendered page", "outcome": "failed", "evidence": "observed"}], False),
        ([{"name": "rendered page", "outcome": "unverified", "evidence": "claimed"}], False),
        ([{"name": "rendered page", "outcome": "passed", "evidence": "claimed"}], False),
        ([{"name": "rendered page", "outcome": "passed", "evidence": "observed"}], True),
    ],
)
def test_assignment_accept_requires_observed_structured_rendered_check(tmp_path, checks, accepted):
    """A rendered requirement cannot be certified by report prose.

    This reproduces the twentieth live failure: the worker's structured
    publish_result checks were empty while its report said that rendering was
    unverified, yet the coordinator accepted the assignment.  Informational
    assignments remain covered by the existing no-check acceptance tests.
    """
    store, task_id, coordinator = _task(tmp_path)
    ref = "frontend-rendered"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-rendered",
        dict(
            title="Implement landing page",
            brief="Build the page and run the supplied rendered checks.",
            acceptance="The page must pass the rendered browser check and have a visible focus ring.",
        ),
        origin=coordinator.channel_id, assignment_ref=ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "worker-rendered-result",
        dict(
            report_ref="r_rendered",
            summary="The report says the rendered check is unverified.",
            checks=checks,
        ),
        origin="worker-channel", assignment_ref=ref,
    )
    call = dict(
        assignment_ref=ref, decision="accept",
        rationale="The report says the page is complete.", request_key="accept-rendered",
    )
    if accepted:
        assert mcp_tools.dispatch(store, coordinator, "decide", call)["recorded"]
    else:
        with pytest.raises(StoreError, match="acceptance_refused"):
            mcp_tools.dispatch(store, coordinator, "decide", call)


def test_assignment_accept_uses_exact_assessed_candidate_check_when_worker_checks_empty(tmp_path):
    """Candidate review evidence can satisfy the rendered gate.

    A worker report may legitimately contain no publish_result checks when the
    coordinator performs the exact candidate check after staging.  Acceptance
    must bind that check to this attempt and the current candidate digest; a
    passing prose claim or an unrelated candidate must remain insufficient.
    """
    from cortex_runtime.delegation import candidates, check_requests

    store, task_id, coordinator = _task(tmp_path)
    ref = "frontend-rendered-candidate"
    requirement = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="create_requirement", statement="The page must pass a rendered browser check.",
        criteria=["Rendered browser check passes"], request_key="candidate-render-requirement",
    ))["requirement_id"]
    planned = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="add_assignment", node_key="rendered-candidate",
        requirement_bindings=[dict(requirement_ref=requirement, revision="1")],
        request_key="candidate-render-assignment",
    ))
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-rendered-candidate",
        dict(
            title="Implement landing page",
            brief="Build the page and run the supplied rendered checks.",
            acceptance="The page must pass the rendered browser check.",
            plan_assignment_ref=planned["assignment_id"],
        ), origin=coordinator.channel_id, assignment_ref=ref,
    )
    stage = tmp_path / ".cortex" / "candidates" / "cand_exact" / "stage"
    stage.mkdir(parents=True)
    (stage / "index.html").write_text("<main>ready</main>\n", encoding="utf-8")
    digest = candidates._digest_manifest(candidates._manifest(str(stage)))
    candidates._ensure_schema(store)
    with store.connection() as db:
        db.execute(
            "INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,"
            "attempt_ids_json,source_bindings_json,base_identity_json,project_base_manifest_json,"
            "project_base_digest,content_digest,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            ("cand_exact", task_id, str(tmp_path), str(stage), json.dumps([ref]), "[]", "[]", "{}",
             "base", digest, "ready", store._now()),
        )
    static_request = check_requests.enqueue_check(
        store, task_id=task_id, candidate_id="cand_exact", candidate_digest=digest,
        argv=["python3", "tests/check_static.py"], request_key="static-check",
    )
    check_requests.claim_check(store, static_request["check_id"])
    check_requests.complete_check(
        store, static_request["check_id"],
        {"evidence_id": "e_static", "observed_result": {
            "status": "completed", "exit_code": 0, "output_truncated": False,
        }},
    )
    delegation_events.append_event(
        store, task_id, "progress", "assess_check:static-check",
        dict(kind="candidate_check_assessed", check_ref=static_request["check_id"],
             candidate_ref="cand_exact", candidate_digest=digest,
             evidence_ref="e_static", assessment="pass"),
        origin=coordinator.channel_id,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "worker-rendered-candidate-result",
        dict(report_ref="r_candidate", summary="The initial browser sandbox was unavailable.",
             checks=[{"name": "rendered browser page", "outcome": "unverified", "evidence": "observed"}]),
        origin="worker-channel", assignment_ref=ref,
    )
    with pytest.raises(StoreError, match="acceptance_refused"):
        mcp_tools.dispatch(store, coordinator, "decide", dict(
            assignment_ref=ref, decision="accept", rationale="A static check passed.",
            request_key="reject-static-only",
        ))
    # A historical ACP turn may have delegated before it created the graph
    # node, so the delegation event has no plan_assignment_ref.  Once the
    # exact candidate is bound to that node, the candidate itself must bridge
    # the requirement scope without broadening it to unrelated candidates.
    with store.connection() as db:
        row = db.execute(
            "SELECT id,payload_json FROM task_events WHERE task_id=? "
            "AND event_type='delegation_requested' AND assignment_ref=?",
            (task_id, ref),
        ).fetchone()
        payload = json.loads(row["payload_json"])
        payload.pop("plan_assignment_ref", None)
        db.execute("UPDATE task_events SET payload_json=? WHERE id=?", (json.dumps(payload), row["id"]))
        db.execute(
            "UPDATE work_assignments SET candidate_ref=? WHERE task_id=? AND assignment_id=?",
            ("candidate:cand_exact", task_id, planned["assignment_id"]),
        )
    request = check_requests.enqueue_check(
        store, task_id=task_id, candidate_id="cand_exact", candidate_digest=digest,
        argv=["python3", "tests/check_render.py"], request_key="exact-render-check",
        requirement_bindings={requirement: {"revision": 1}},
    )
    check_requests.claim_check(store, request["check_id"])
    check_requests.complete_check(
        store, request["check_id"],
        {"evidence_id": "e_exact", "observed_result": {
            "status": "completed", "exit_code": 0, "output_truncated": False,
        }},
    )
    delegation_events.append_event(
        store, task_id, "progress", "assess_check:exact-render-check",
        dict(kind="candidate_check_assessed", check_ref=request["check_id"],
             candidate_ref="cand_exact", candidate_digest=digest,
             evidence_ref="wrong-evidence", assessment="pass"),
        origin=coordinator.channel_id,
    )
    with pytest.raises(StoreError, match="acceptance_refused"):
        mcp_tools.dispatch(store, coordinator, "decide", dict(
            assignment_ref=ref, decision="accept", rationale="A mismatched assessment was supplied.",
            request_key="reject-mismatched-evidence",
        ))
    delegation_events.append_event(
        store, task_id, "progress", "assess_check:exact-render-check-correct",
        dict(kind="candidate_check_assessed", check_ref=request["check_id"],
             candidate_ref="cand_exact", candidate_digest=digest,
             evidence_ref="e_exact", assessment="pass"),
        origin=coordinator.channel_id,
    )
    assert mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=ref, decision="accept", rationale="The exact staged candidate check passed.",
        request_key="accept-rendered-candidate",
    ))["recorded"]
    # The task-level gate must apply the same candidate supersession rule as
    # assignment acceptance: the worker's unverified native check is no
    # longer a blocker once this exact requirement-bound candidate pass is
    # durable.  Other unrelated reasons (such as the synthetic report path
    # in this unit fixture) may still keep the whole task closed.
    from cortex_runtime.delegation import completion as delegation_completion
    _ok, reasons = delegation_completion.gate(store, task_id)
    assert not any("is unverified" in reason for reason in reasons)


def test_decide_ask_user_requires_a_question(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "decide", dict(assignment_ref="task", decision="ask_user", rationale="need budget approval", request_key="k"))
    assert str(excinfo.value) == "missing_question"


def test_worker_publish_result_requires_a_real_report_of_this_task(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref="r_" + "b" * 12, summary="done", request_key="k"))
    assert str(excinfo.value) == "unknown_report_ref"


def test_worker_publish_result_rejects_empty_checks_for_rendered_assignment(tmp_path):
    """A rendered/UI promise must survive publication as structured data.

    The rejection is deliberately at ``publish_result`` time, before an
    immutable ``worker_result`` event is appended.  Informational assignments
    continue to use the existing empty-checks path below.
    """
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="rendered-publish")
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-rendered-publish",
        dict(
            title="Implement landing page",
            brief="Build the page and verify it in the rendered browser check.",
            role="frontend-dev",
            acceptance="The rendered page must pass with a visible focus ring.",
        ), origin=coordinator.channel_id, assignment_ref=worker.attempt_id,
    )
    report_ref = _worker_report(store, worker)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "publish_result", dict(
            report_ref=report_ref, summary="The report describes the page.", request_key="rendered-empty",
        ))
    error = excinfo.value
    assert str(error) == "checks_required"
    assert error.field == "checks"
    assert "structured check name" in error.expected
    assert "outcome" in error.expected
    assert "exact tool_call_ref" in error.expected
    assert "prose alone" in error.correction
    with store.connection() as db:
        assert db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task_id,),
        ).fetchone() is None


def test_worker_publish_result_allows_empty_checks_for_report_only_graph_criteria(tmp_path):
    """Graph criteria alone do not turn a report-only assignment into UI proof."""
    store, task_id, coordinator = _task(tmp_path)
    requirement = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="create_requirement", statement="The landing page is verified.",
        criteria=["Automated verification passes"], request_key="publish-criteria-requirement",
    ))
    planned = mcp_tools.dispatch(store, coordinator, "update_plan", dict(
        action="add_assignment", node_key="implementation",
        requirement_bindings=[dict(requirement_ref=requirement["requirement_id"], revision="1")],
        request_key="publish-criteria-assignment",
    ))
    worker = _worker_channel(store, task_id, coordinator, attempt_id="criteria-publish")
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-criteria-publish",
        dict(
            title="Implement landing page", brief="Build the page.", role="frontend-dev",
            acceptance="Deliver the page.", plan_assignment_ref=planned["assignment_id"],
        ), origin=coordinator.channel_id, assignment_ref=worker.attempt_id,
    )
    report_ref = _worker_report(store, worker)
    result = mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=report_ref, summary="The page was delivered.", request_key="criteria-empty",
    ))
    assert result["recorded"] is True


@pytest.mark.parametrize("outcome", ["failed", "unverified"])
def test_worker_publish_result_preserves_honest_nonpassing_rendered_check(tmp_path, outcome):
    """Fail-fast only rejects missing evidence, never honest outcomes."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id=f"rendered-{outcome}")
    delegation_events.append_event(
        store, task_id, "delegation_requested", f"delegate-rendered-{outcome}",
        dict(
            title="Implement landing page",
            brief="Build the page and run the rendered browser check.",
            role="frontend-dev",
            acceptance="The rendered page must pass.",
        ), origin=coordinator.channel_id, assignment_ref=worker.attempt_id,
    )
    report_ref = _worker_report(store, worker)
    result = mcp_tools.dispatch(store, worker, "publish_result", dict(
        report_ref=report_ref, summary=f"The rendered check is {outcome}.",
        checks=[{"name": "rendered browser page", "outcome": outcome}],
        request_key=f"rendered-{outcome}",
    ))
    assert result["recorded"] is True


def test_worker_publish_result_and_ask_question_carry_no_technical_ids(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    report_id = _worker_report(store, worker)
    result = mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=report_id, summary="done", request_key="k1"))
    assert set(result) == {"recorded", "cursor", "superseded"}
    question = mcp_tools.dispatch(store, worker, "ask_question", dict(question="Which env?", request_key="k2"))
    assert set(question) == {"recorded", "cursor"}


def test_worker_result_after_cancellation_is_recorded_superseded(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="xa_cancelled")
    delegation_events.append_event(
        store, task_id, "assignment_cancelled", "cancel1", dict(reason="pipeline revised"),
        origin="runtime", assignment_ref=worker.attempt_id,
    )
    report_id = _worker_report(store, worker)
    result = mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=report_id, summary="late", request_key="k"))
    assert result["superseded"] is True
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont")
    assert not any(e.event_type == "worker_result" for e in claimed)


def test_worker_cannot_publish_result_for_a_report_it_did_not_author(tmp_path):
    """T019: report_ref binding -- a worker can only publish_result for a
    report published from its own draft, never a coordinator report or
    another worker's report."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="xa_self")
    other_worker = _worker_channel(store, task_id, coordinator, attempt_id="xa_other")
    coordinator_report = _report(store, task_id)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=coordinator_report, summary="done", request_key="k1"))
    assert str(excinfo.value) == "report_not_own"
    other_report = _worker_report(store, other_worker)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=other_report, summary="done", request_key="k2"))
    assert str(excinfo.value) == "report_not_own"


def test_worker_publish_result_is_idempotent_and_rejects_changed_payload(tmp_path):
    """T019: same attempt + same report_ref + identical payload returns the
    same canonical event exactly once; a changed payload for the same
    report_ref is a delivery_conflict, never a silent second publication."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    report_id = _worker_report(store, worker)
    args = dict(report_ref=report_id, summary="done", request_key="k1")
    first = mcp_tools.dispatch(store, worker, "publish_result", args)
    second = mcp_tools.dispatch(store, worker, "publish_result", dict(args))
    assert first["cursor"] == second["cursor"]
    events = [e for e in delegation_events.list_since(store, task_id) if e.event_type == "worker_result"]
    assert len(events) == 1
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=report_id, summary="changed", request_key="k2"))
    assert str(excinfo.value) == "delivery_conflict"
    events = [e for e in delegation_events.list_since(store, task_id) if e.event_type == "worker_result"]
    assert len(events) == 1


def test_a_different_attempt_cannot_claim_a_report_refs_publication(tmp_path):
    """T019: even if a second attempt somehow reused the identical
    report_ref (for example a replayed channel), a different assignment
    attempt is refused rather than silently taking over the first attempt's
    canonical publication."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator, attempt_id="xa_first")
    report_id = _worker_report(store, worker)
    mcp_tools.dispatch(store, worker, "publish_result", dict(report_ref=report_id, summary="done", request_key="k1"))
    impersonator = _worker_channel(store, task_id, coordinator, attempt_id="xa_second")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("UPDATE drafts SET owner_thread_id=? WHERE published_report_id=?",
                   (impersonator.principal_thread_id, report_id))
        db.commit()
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, impersonator, "publish_result", dict(report_ref=report_id, summary="done", request_key="k2"))
    assert str(excinfo.value) == "report_ref_already_published"


def test_dispatch_refuses_a_handler_result_that_would_leak_technical_metadata(tmp_path, monkeypatch):
    """T019 entrypoint audit: mcp_tools.dispatch's own defensive
    NO_TECHNICAL_ID_FIELDS check (never exercised in normal operation,
    since no real handler sets these fields) actually fires if a handler
    ever regresses -- a delegation MCP tool result can never carry
    task_id/attempt_id/channel_id/etc, even accidentally."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)

    def leaking_handler(store, channel, args):
        return dict(recorded=True, cursor=1, task_id=channel.task_id)

    monkeypatch.setitem(mcp_tools._WORKER_HANDLERS, "ask_question", leaking_handler)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "ask_question", dict(question="leak?", request_key="k"))
    assert str(excinfo.value) == "internal_leak"


def test_decide_accept_takes_the_delegation_ref_as_well_as_the_attempt_id(tmp_path):
    """Live defect (2026-09-19): a ``worker_result`` is keyed by the
    worker's own attempt id, but every coordinator-facing surface speaks the
    delegation ref ``delegate`` returned. Accepting one assignment must work
    with either one, or accepting becomes a guessing game about which id the
    coordinator happens to be holding."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_ref = "d" * 64
    attempt_id = "xa_0123456789ab"
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
            "owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
            (delegation_ref, task_id, attempt_id, "worker-backend-dev", "ready", "ready", 1,
             store._now(), store._now()),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-both", dict(report_ref="r_x", summary="done"),
        origin="worker-ch", assignment_ref=attempt_id,
    )

    result = mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=delegation_ref, decision="accept", rationale="Verified.", request_key="k-deleg"))
    assert result is not None
    assert "separate task-level accept decision" in result["next_step"]

    # The attempt id still works too -- neither id was taken away.
    result = mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=attempt_id, decision="accept", rationale="Verified.", request_key="k-attempt"))
    assert result is not None


def test_assignment_accept_emits_one_canonical_task_followup(tmp_path):
    """An assignment accept must wake the coordinator toward task accept.

    The live mixed-engine run repeatedly accepted the same worker under the
    delegation hash and then under its ``xa_`` attempt id.  The two aliases
    represent one assignment, so the correction marker must be durable and
    idempotent across both forms.
    """
    store, task_id, coordinator = _task(tmp_path)
    delegation_ref = "e" * 64
    attempt_id = "xa_0123456789ab"
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
            "owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
            (delegation_ref, task_id, attempt_id, "worker-backend-dev", "ready", "ready", 1,
             store._now(), store._now()),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-followup", dict(report_ref="r_x", summary="done"),
        origin="worker-ch", assignment_ref=attempt_id,
    )

    mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=delegation_ref, decision="accept", rationale="Verified.", request_key="k-hash"))
    mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=attempt_id, decision="accept", rationale="Verified again.", request_key="k-attempt"))

    decisions = [
        event for event in delegation_events.list_since(store, task_id)
        if event.event_type == "coordinator_decision"
        and event.payload.get("decision") == "accept"
    ]
    assert len(decisions) == 1

    followups = [
        event for event in delegation_events.list_since(store, task_id)
        if event.event_type == "progress"
        and event.payload.get("kind") == "assignment_accept_requires_task"
    ]
    assert len(followups) == 1
    assert followups[0].assignment_ref == delegation_ref
    assert delegation_events.is_meaningful(followups[0])


@pytest.mark.parametrize("new_worker_event", [False, True])
def test_task_accept_discharges_only_its_own_assignment_notice(tmp_path, new_worker_event):
    """A same-turn explicit task decision answers the coordinator's own
    reminder; a new worker event still blocks it without losing the notice."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    report_id = _worker_report(store, worker)
    ref = "assignment-for-task-decision"
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate-for-task", dict(title="Fix"),
        origin=coordinator.channel_id, assignment_ref=ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "result-for-task", dict(report_ref=report_id, summary="Fixed"),
        origin=worker.channel_id, assignment_ref=ref,
    )
    delegation_events.claim_unconsumed(store, task_id, "cont-reviewed")
    delegation_events.mark_delivered(store, task_id, "cont-reviewed")
    assignment = mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=ref, decision="accept", rationale="Reviewed the report.", request_key="assignment-accept"))
    assert assignment["recorded"]

    draft = store.call(
        "create_draft", dict(template="pipeline", request_key="pipeline-task-decision"),
        coordinator.principal_thread_id, coordinator.parent_principal_thread_id, trusted=True,
    )
    from pathlib import Path
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + b"# Current pipeline\n\nThe result and report were reviewed.\n")
    published = store.call(
        "write_report", dict(title="Current pipeline", summary="Reviewed result.", author="coordinator",
                             draft_id=draft["draft_id"], request_key="publish-current-pipeline"),
        coordinator.principal_thread_id, coordinator.parent_principal_thread_id, trusted=True,
    )
    assert published["report_id"]

    if new_worker_event:
        delegation_events.append_event(
            store, task_id, "worker_question", "new-worker-question",
            dict(question="A genuine new question"), origin=worker.channel_id, assignment_ref=ref,
        )
        with pytest.raises(StoreError, match="acceptance_refused"):
            mcp_tools.dispatch(store, coordinator, "decide", dict(
                assignment_ref="task", decision="accept", rationale="Complete.", request_key="task-accept"))
    else:
        accepted = mcp_tools.dispatch(store, coordinator, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Complete.", request_key="task-accept"))
        assert accepted["recorded"]
        assert mcp_tools.dispatch(store, coordinator, "decide", dict(
            assignment_ref="task", decision="accept", rationale="Complete.", request_key="task-accept"))["replayed"]

    with store.connection() as db:
        reminder = db.execute(
            "SELECT superseded,consumed_by FROM task_events WHERE task_id=? "
            "AND dedup_key='assignment-accept-followup:'||?", (task_id, ref),
        ).fetchone()
        decisions = db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='coordinator_decision' "
            "AND assignment_ref='task'", (task_id,),
        ).fetchone()[0]
    assert reminder["superseded"] == (0 if new_worker_event else 1)
    assert reminder["consumed_by"] is None
    assert decisions == (0 if new_worker_event else 1)


def test_clarifying_a_finished_assignment_is_refused_not_silently_recorded(tmp_path):
    """Live (2026-09-19): after an integration conflict the coordinator
    clarified an assignment whose worker had already published its result.
    The call answered "recorded", the coordinator read that as delivered,
    and the task then waited for a reply that could never come. A
    clarification nobody can read must say so."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "delegation_requested", "req-done", dict(title="T"),
        origin="coordinator-ch", assignment_ref="finished-one",
    )
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-done", dict(report_ref="r_y", summary="done"),
        origin="worker-ch", assignment_ref="finished-one",
    )

    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "clarify", dict(
            assignment_ref="finished-one", message="please redo it", request_key="k-late"))
    assert str(excinfo.value) == "assignment_already_finished"


def test_clarifying_a_running_assignment_still_works(tmp_path):
    """The other half: an assignment still running takes clarifications as
    it always did."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "delegation_requested", "req-live", dict(title="T"),
        origin="coordinator-ch", assignment_ref="running-one",
    )

    result = mcp_tools.dispatch(store, coordinator, "clarify", dict(
        assignment_ref="running-one", message="use tabs", request_key="k-live"))
    assert result["recorded"] is True


def test_an_assignment_whose_integration_conflicted_cannot_be_accepted(tmp_path):
    """Live (2026-09-19, L3): the editor held an unsaved buffer for the file,
    so the integration was refused and nothing was written -- and the
    coordinator accepted the assignment anyway and told the user the change
    "was integrated into the project and accepted". It was not in the
    project at all."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-c", dict(report_ref="r_z", summary="done"),
        origin="worker-ch", assignment_ref="conflicted-one",
    )
    delegation_events.append_event(
        store, task_id, "progress", "integration_conflict:one",
        dict(kind="integration_conflict", conflict_paths={"calc.py": {"reason": "unsaved_host_buffer"}}),
        origin="runtime", assignment_ref="conflicted-one",
    )

    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "decide", dict(
            assignment_ref="conflicted-one", decision="accept", rationale="looks good", request_key="k-c"))
    assert str(excinfo.value) == "integration_conflicted"


def test_a_later_successful_integration_clears_the_earlier_conflict(tmp_path):
    """The conflict is only the *last* word while it is the last word: once
    the work actually reaches the project, accepting is right again."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-d", dict(report_ref="r_w", summary="done"),
        origin="worker-ch", assignment_ref="retried-one",
    )
    delegation_events.append_event(
        store, task_id, "progress", "integration_conflict:two",
        dict(kind="integration_conflict", conflict_paths={"calc.py": {"reason": "unsaved_host_buffer"}}),
        origin="runtime", assignment_ref="retried-one",
    )
    delegation_events.append_event(
        store, task_id, "progress", "integration_applied:two",
        dict(kind="integration_applied", applied_paths=["calc.py"]),
        origin="runtime", assignment_ref="retried-one",
    )

    assert mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref="retried-one", decision="accept", rationale="in the project now",
        request_key="k-d")) is not None


def test_an_assignment_that_never_integrated_anything_is_unaffected(tmp_path):
    """A review or an investigation changes no files and asks for no
    integration; it must still be acceptable."""
    store, task_id, coordinator = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "worker_result", "wr-e", dict(report_ref="r_v", summary="reviewed"),
        origin="worker-ch", assignment_ref="review-one",
    )

    assert mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref="review-one", decision="accept", rationale="review is the deliverable",
        request_key="k-e")) is not None


def test_a_worker_can_read_the_references_its_own_profile_names(tmp_path):
    """A worker's working directory is a copy of the user's project, not of
    this installation, so a relative markdown link in its own instructions
    resolves to nothing. Live (2026-09-19) the step that link explains --
    publishing a report and handing back its id -- was the step three
    workers failed at in one day: one asked the coordinator to send it a
    report reference, another published and never said so."""
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)

    for reference in ("report-publication", "code-and-evidence", "interactive-resources"):
        result = mcp_tools.dispatch(store, worker, "read_skill",
                                    dict(skill=reference))
        assert result["skill"] == reference
        assert result["body"].strip(), reference
    # The name it is written with in the profile works too.
    assert mcp_tools.dispatch(
        store, worker, "read_skill",
        dict(skill="report-publication.md"))["body"].strip()
    # And the publication flow is genuinely in there.
    body = mcp_tools.dispatch(store, worker, "read_skill",
                              dict(skill="report-publication"))["body"]
    assert "draft" in body.lower() and "publi" in body.lower()


def test_an_unknown_skill_still_names_what_is_readable(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "read_skill",
                           dict(skill="nothing-like-this"))
    assert str(excinfo.value) == "unknown_skill"
    assert "report-publication" in excinfo.value.expected
