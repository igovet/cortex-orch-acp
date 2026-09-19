"""T016: delegation.mcp_tools -- role-filtered delegation operation handlers.

Exercises the handler layer directly (in-process), the same way the T016
fake-model tests in test_orchestrating_runtime.py call it: never a real MCP
subprocess, never a real engine. Every Store is rooted at tmp_path.
"""
from __future__ import annotations

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

    # The attempt id still works too -- neither id was taken away.
    result = mcp_tools.dispatch(store, coordinator, "decide", dict(
        assignment_ref=attempt_id, decision="accept", rationale="Verified.", request_key="k-attempt"))
    assert result is not None


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
