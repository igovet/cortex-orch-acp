"""T025 (lead follow-up item 1 + 3): the coordinator-facing ``integrate``
MCP operation, wired through ``OrchestratingRuntime`` end to end against
real ``IsolatedSnapshotWorkspaceProvisioner`` snapshots and the
``FakeSessionRuntime``/``mcp_tools.dispatch`` harness already used by
``test_orchestrating_runtime.py`` -- no real engine subprocess.
"""
from __future__ import annotations

import json
import os
import threading
import uuid

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime


def _assignment_title(store, channel):
    with store.connection() as db:
        row = db.execute(
            "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (channel.attempt_id,),
        ).fetchone()
        if row is None:
            return None
        event = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND assignment_ref=?",
            (channel.task_id, row["idempotency_key"]),
        ).fetchone()
    return json.loads(event["payload_json"])["title"] if event is not None else None


def _publish(store, channel, label, call_index):
    report_id = "r_" + uuid.uuid4().hex[:12]
    filename = report_id + ".md"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        path = task_dir / filename
        path.write_text(f"# {label}\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", filename, store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], channel.task_id, channel.principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
        )
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}-{call_index}"))


def _task(tmp_path):
    (tmp_path / "shared.txt").write_text("line1\nline2\nline3\n")
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return store, result.task_id


def _runtime(store, router, project_root, *, host_fs_requester=None):
    engine = FakeSessionRuntime(store, router)
    workspace = IsolatedSnapshotWorkspaceProvisioner(store, str(project_root))
    runtime = OrchestratingRuntime(
        store, engine, workspace, FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=30.0,
        host_fs_requester=host_fs_requester,
    )
    return runtime, engine, workspace


def _integration_events(store, task_id):
    """T025: integration outcomes are carried as ``progress`` events (see
    ``delegation.events.INTEGRATION_PROGRESS_KINDS``); returns
    ``[(kind, payload), ...]`` for the outcome rows only (never the
    ``integration_requested`` request row itself)."""
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND (dedup_key LIKE 'integration_applied:%' OR dedup_key LIKE 'integration_conflict:%') "
            "ORDER BY id",
            (task_id,),
        ).fetchall()
    payloads = [json.loads(row["payload_json"]) for row in rows]
    return [(payload["kind"], payload) for payload in payloads]


def _wait_for_both_snapshots(store, task_id, *, timeout):
    import time

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with store.connection() as db:
            count = db.execute(
                "SELECT COUNT(*) FROM workspace_snapshots WHERE task_id=?", (task_id,),
            ).fetchone()[0]
        if count >= 2:
            return
        time.sleep(0.01)
    raise AssertionError("both workers' snapshots were never both created in time")


def test_two_workers_same_file_first_integrates_second_conflicts_no_overwrite(tmp_path):
    store, task_id = _task(tmp_path)
    assignment_refs = {}
    integrated = set()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result_a = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="edit shared.txt", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            result_b = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker B", brief="edit shared.txt", role="backend-dev", acceptance="done", request_key="reqB",
            ))
            assignment_refs["Worker A"] = result_a["assignment_ref"]
            assignment_refs["Worker B"] = result_b["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated A and B.")
        for name in ("Worker A", "Worker B"):
            if f"{name} finished" in message and name not in integrated:
                integrated.add(name)
                mcp_tools.dispatch(store, channel, "integrate", dict(
                    assignment_ref=assignment_refs[name], request_key=f"int-{name}",
                ))
        if len(integrated) >= 2:
            return PromptOutcome(StopReason.END_TURN, "Both processed.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        # Both workers' own base snapshots must exist *before* either one
        # edits/publishes -- otherwise a worker provisioned late (a real
        # thread-scheduling race, reproduced under CPU load) would capture
        # the *other* worker's already-integrated content as its own base,
        # which is a different (and separately correct) scenario, not the
        # "two workers race from the same starting point" this test checks.
        _wait_for_both_snapshots(store, task_id, timeout=10.0)
        root = workspace.root_for(channel.workspace_binding_id)
        content = "line1-BY-A\nline2\nline3\n" if title == "Worker A" else "line1-BY-B\nline2\nline3\n"
        with open(os.path.join(root, "shared.txt"), "w") as f:
            f.write(content)
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path)
    cancel_event = threading.Event()
    updates = []
    runtime.start_prompt(task_id, "s1", "Build A and B.", updates.append, cancel_event)

    events = _integration_events(store, task_id)
    statuses = {payload["assignment_ref"]: (etype, payload) for etype, payload in events}
    assert len(statuses) == 2
    outcomes = [etype for etype, _ in statuses.values()]
    # Exactly one applied, one conflicted -- order depends on scheduling,
    # but never both applied (that would mean a silent overwrite) and
    # never both conflicted (the first one always has a clean base).
    assert sorted(outcomes) == ["integration_applied", "integration_conflict"]
    conflict_payload = next(payload for etype, payload in statuses.values() if etype == "integration_conflict")
    assert "shared.txt" in conflict_payload["conflict_paths"]

    # Whichever worker applied first, the file on disk matches exactly
    # that worker's content -- never a merge of both, never last-write-wins.
    applied_ref = next(ref for ref, (etype, _) in statuses.items() if etype == "integration_applied")
    applied_name = next(name for name, ref in assignment_refs.items() if ref == applied_ref)
    expected = "line1-BY-A\nline2\nline3\n" if applied_name == "Worker A" else "line1-BY-B\nline2\nline3\n"
    assert (tmp_path / "shared.txt").read_text() == expected


def test_integrate_mcp_operation_is_coordinator_only(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        result = mcp_tools.dispatch(store, channel, "delegate", dict(
            title="Worker A", brief="edit", role="backend-dev", acceptance="done", request_key="reqA",
        ))
        try:
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=result["assignment_ref"], request_key="badcall",
            ))
        except Exception:
            pass
        return PromptOutcome(StopReason.END_TURN, "done")

    captured = {}

    def worker_behavior(store, channel, session_ref, message, call_index):
        from cortex_runtime.contracts import StoreError
        try:
            mcp_tools.dispatch(store, channel, "integrate", dict(assignment_ref="whatever", request_key="rk"))
            captured["refused"] = False
        except StoreError as error:
            captured["refused"] = str(error) == "unknown_operation"
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, cancel_event)
    assert captured.get("refused") is True


def test_host_buffer_unknown_when_no_host_fs_requester_wired(tmp_path):
    store, task_id = _task(tmp_path)
    assignment_ref_holder = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="edit", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            assignment_ref_holder["ref"] = result["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "Worker A finished" in message and "done" not in assignment_ref_holder:
            assignment_ref_holder["done"] = True
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=assignment_ref_holder["ref"], request_key="int1",
            ))
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        root = workspace.root_for(channel.workspace_binding_id)
        with open(os.path.join(root, "shared.txt"), "a") as f:
            f.write("worker edit\n")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path, host_fs_requester=None)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, cancel_event)

    events = _integration_events(store, task_id)
    assert len(events) == 1
    etype, payload = events[0]
    assert etype == "integration_applied"
    assert payload["host_buffer_state"] == "unknown"


def test_host_buffer_checked_and_blocks_via_fake_host_forwarder(tmp_path):
    """The same production seam (``host_fs_requester`` on
    ``OrchestratingRuntime``) that a real ACP host connection would supply
    once it negotiates fs, exercised here with a fake forwarder reporting a
    newer, unsaved buffer for the exact path the worker changed."""
    store, task_id = _task(tmp_path)
    assignment_ref_holder = {}

    def fake_host_fs_requester(method, params):
        assert method == "fs/read_text_file"
        if params["path"].endswith("shared.txt"):
            return {"content": "line1\nline2\nHOST-HAS-UNSAVED-EDIT\n"}
        raise RuntimeError("no buffer for this path")

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="edit", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            assignment_ref_holder["ref"] = result["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "Worker A finished" in message and "done" not in assignment_ref_holder:
            assignment_ref_holder["done"] = True
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=assignment_ref_holder["ref"], request_key="int1",
            ))
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        root = workspace.root_for(channel.workspace_binding_id)
        with open(os.path.join(root, "shared.txt"), "w") as f:
            f.write("line1\nline2\nWORKER-EDIT\n")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path, host_fs_requester=fake_host_fs_requester)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, cancel_event)

    events = _integration_events(store, task_id)
    assert len(events) == 1
    etype, payload = events[0]
    assert etype == "integration_conflict"
    assert payload["conflict_paths"]["shared.txt"]["reason"] == "unsaved_host_buffer"
    assert payload["host_buffer_state"] == "checked"
    # Never overwritten by the worker's stale-relative-to-host result.
    assert (tmp_path / "shared.txt").read_text() == "line1\nline2\nline3\n"


def test_host_buffer_check_failed_blocks_overwrite_never_silently_applies(tmp_path):
    """EXT-01 #5 (three-state fix): a requester *was* supplied (fs is
    negotiated) but the one round-trip for the worker's own touched path
    fails (here: the host's transport is simply gone -- a
    ``HostRequestClosed``-shaped failure in production) -- this must never
    be treated the same as "host confirms no buffer here" (which stays a
    clean pass). Recorded as its own third state, ``check_failed``, and the
    path is refused exactly like a positively-observed conflict: the
    worker's result is never applied uncertain."""
    store, task_id = _task(tmp_path)
    assignment_ref_holder = {}

    def fake_host_fs_requester(method, params):
        assert method == "fs/read_text_file"
        if params["path"].endswith("shared.txt"):
            raise RuntimeError("transport closed mid-request")
        raise AssertionError("only the worker's own touched path is ever consulted")

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="edit", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            assignment_ref_holder["ref"] = result["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "Worker A finished" in message and "done" not in assignment_ref_holder:
            assignment_ref_holder["done"] = True
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=assignment_ref_holder["ref"], request_key="int1",
            ))
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        root = workspace.root_for(channel.workspace_binding_id)
        with open(os.path.join(root, "shared.txt"), "w") as f:
            f.write("line1\nline2\nWORKER-EDIT\n")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path, host_fs_requester=fake_host_fs_requester)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, cancel_event)

    events = _integration_events(store, task_id)
    assert len(events) == 1
    etype, payload = events[0]
    assert etype == "integration_conflict"
    assert payload["conflict_paths"]["shared.txt"]["reason"] == "host_buffer_check_failed"
    assert payload["host_buffer_state"] == "check_failed"
    # Never overwritten by the worker's own result while the check was
    # uncertain -- exactly the same protection a positively-observed
    # unsaved buffer gets, never weaker.
    assert (tmp_path / "shared.txt").read_text() == "line1\nline2\nline3\n"


def test_per_call_host_fs_requester_overrides_constructor_default_and_is_scoped_to_the_call(tmp_path):
    """EXT-01 #5: ``OrchestratingRuntime`` is one long-lived instance per
    project, shared across every ACP connection -- so a real caller
    (``acp.agent.Agent``) registers this exact call's own connection-bound
    requester via ``start_prompt(host_fs_requester=...)``, never only the
    constructor-time default. Proven here: the constructor default is never
    consulted at all when a per-call one is given, and
    ``self._task_fs_requesters`` (this runtime's own per-task registry) is
    empty again once the call returns -- never leaked across tasks/calls."""
    store, task_id = _task(tmp_path)
    assignment_ref_holder = {}

    def constructor_default_requester(method, params):
        raise AssertionError("the per-call requester must win; the constructor default must never be consulted")

    def per_call_requester(method, params):
        assert method == "fs/read_text_file"
        return {"content": "line1\nline2\nHOST-HAS-UNSAVED-EDIT\n"}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            result = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Worker A", brief="edit", role="backend-dev", acceptance="done", request_key="reqA",
            ))
            assignment_ref_holder["ref"] = result["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        if "Worker A finished" in message and "done" not in assignment_ref_holder:
            assignment_ref_holder["done"] = True
            mcp_tools.dispatch(store, channel, "integrate", dict(
                assignment_ref=assignment_ref_holder["ref"], request_key="int1",
            ))
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        root = workspace.root_for(channel.workspace_binding_id)
        with open(os.path.join(root, "shared.txt"), "w") as f:
            f.write("line1\nline2\nWORKER-EDIT\n")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine, workspace = _runtime(store, router, tmp_path, host_fs_requester=constructor_default_requester)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Go.", lambda t: True, cancel_event, host_fs_requester=per_call_requester)

    assert runtime._task_fs_requesters == {}  # deregistered once the call returned
    events = _integration_events(store, task_id)
    assert len(events) == 1
    etype, payload = events[0]
    assert etype == "integration_conflict"
    assert payload["conflict_paths"]["shared.txt"]["reason"] == "unsaved_host_buffer"
    assert payload["host_buffer_state"] == "checked"
