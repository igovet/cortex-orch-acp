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
from cortex_runtime.delegation.work_graph import WorkGraph
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




























def test_coordinator_channel_cannot_call_worker_operations(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "ask_question", dict(question="huh?", request_key="k"))
    assert str(excinfo.value) == "unknown_operation"
































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


def test_coordinator_read_diagnostics_omits_technical_ids(tmp_path):
    """The read-only diagnostics handler must pass the gateway's metadata guard."""
    store, task_id, coordinator = _task(tmp_path)

    result = mcp_tools.dispatch(store, coordinator, "read_diagnostics", {})

    assert result["resource_caps"]["availability"] == "unavailable_reason"
    assert result["route_acceptance_latency"]["availability"] == "unavailable_reason"
    assert result["route_fix_cycles"]["total_review_fix_cycles"] == 0
    assert "task_id" not in result
    assert "task_id" not in result["resource_caps"]
    assert "config_revision_id" not in result["resource_caps"]


def test_coordinator_list_route_options_reaches_its_handler_not_unknown_operation(tmp_path, tmp_path_factory, monkeypatch):
    """`list_route_options` is advertised (contracts.schemas_for and
    tool_definitions) and has a real handler (_COORDINATOR_HANDLERS), but the
    name also used to sit in orchestration.contracts.COORDINATOR, which
    orchestration.mcp.schemas() reuses for the runtime state-machine's own
    operation set. dispatch() checks that runtime-state-machine set before
    the delegation handler table, so a name present in both routed here and
    hit orchestration.mcp.dispatch's fall-through `unknown_operation` instead
    of ever calling the real handler. Keep the name out of COORDINATOR so this
    read stays reachable."""
    from cortex_runtime.orchestration import mcp as runtime_mcp

    monkeypatch.setenv('XDG_CONFIG_HOME', str(tmp_path_factory.mktemp('xdg-config')))
    store, task_id, coordinator = _task(tmp_path)

    assert "list_route_options" not in runtime_mcp.schemas("coordinator")
    result = mcp_tools.dispatch(store, coordinator, "list_route_options", {})
    assert set(result["tiers"]) == {"flagship", "deep", "standard", "light"}
    assert "roles" in result


















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


def test_a_coordinator_can_read_its_own_role_but_not_another_role(tmp_path):
    """Asking for `orchestrator` -- the role the coordinator is -- failed as
    `unknown_skill`, a red failure on the first step of the session."""
    store, task_id, coordinator = _task(tmp_path)
    result = mcp_tools.dispatch(store, coordinator, "read_skill", dict(skill="orchestrator"))
    assert result["skill"] == "orchestrator"
    assert result["body"].strip()
    # Only its own role: a worker's role is still not readable this way.
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, coordinator, "read_skill", dict(skill="worker-debugger"))
    assert str(excinfo.value) == "unknown_skill"


def test_a_worker_cannot_read_the_coordinator_role(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    worker = _worker_channel(store, task_id, coordinator)
    with pytest.raises(StoreError) as excinfo:
        mcp_tools.dispatch(store, worker, "read_skill", dict(skill="orchestrator"))
    assert str(excinfo.value) == "unknown_skill"


def _unfinished_worker_draft(store, worker, body="First section"):
    key = uuid.uuid4().hex
    draft = store.call(
        "create_draft", dict(template="general", request_key="draft-" + key),
        worker.principal_thread_id, worker.parent_principal_thread_id, trusted=True,
    )
    from pathlib import Path

    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + body.encode("utf-8"))
    return draft, path










def test_committed_worker_result_repairs_running_graph_without_mcp_replay(tmp_path):
    """A crash gap cannot leave a durable worker card stuck in running."""
    store, task_id, coordinator = _task(tmp_path)
    graph = WorkGraph(store)
    assignment = graph.add_assignment(task_id, "implementation", idempotency_key="repair-assignment")
    attempt = graph.create_attempt(assignment["assignment_id"], idempotency_key="repair-attempt")
    graph.transition_attempt(attempt["attempt_id"], "running", idempotency_key="repair-running")
    delegation_events.append_event(
        store, task_id, "worker_result", "crash-committed-result",
        {"report_ref": "r_" + "b" * 12, "summary": "Committed before projection."},
        origin="worker-channel", assignment_ref=attempt["attempt_id"],
    )
    assert graph.get_assignment(assignment["assignment_id"])["status"] == "running"

    # Simulate the next store operation after a process died. No replay of
    # the worker MCP call is made; Store recovery reconciles from the event.
    store.call(
        "list_reports", {}, coordinator.principal_thread_id,
        coordinator.parent_principal_thread_id, trusted=True,
    )
    assert graph.get_assignment(assignment["assignment_id"])["status"] == "reported"
