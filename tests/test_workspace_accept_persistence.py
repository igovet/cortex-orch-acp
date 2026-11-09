"""Persisted review gaps and shared-checkout drift, through the real helper/UI port."""
import json
import threading
from pathlib import Path

import pytest

from test_workspace_check_execution import prepared
from test_workspace_api import create_command, query
from test_workspace_task_git import helper

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")


@pytest.mark.parametrize("prepared", [("review-search", True), ("gap", True), ("display", True)], indirect=True)
def test_persisted_acceptance_has_a_working_next_action(prepared, tmp_path, request):
    from cortex_runtime.workspace_api import WorkspaceAPI, task_options
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from cortex_runtime.orchestration import worker_checks
    from workspace_support.contract_socket import run_ui
    runtime, runtime_task, _, _ = prepared
    store = runtime._store
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    task = api.handle(create_command())["target"]
    task_options.prepare(store, task, {})
    def finish(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('accept-session',?,?,'{}','{}','[]','now',NULL)",
                   (store.project_root, runtime_task))
        execution = bind_execution_tx(db, task, runtime_task, "accept-session")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Implemented own item',blocker=NULL WHERE task_id=?",
                   (runtime_task,))
        return execution
    execution = store.immediate_write(finish)
    with store.connection() as db:
        nodes = [json.loads(r[0]) for r in db.execute(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND stage='review'", (runtime_task,))]
    search = any(n["checks"] for n in nodes)
    display = request.node.callspec.params["prepared"][0] == "display"
    if search:
        check = nodes[0]["checks"][0]
        # Actual result_submit normalization, not a made-up display-only DTO.
        assert check["outcome"] == "unverified" and check["claimed_outcome"] == "passed"
        assert check["evidence_gap"] == "trivial_command" and check.get("tool_call_ref")
        assert not check.get("command")
    worker_checks.automatic(runtime, runtime_task, threading.Event(), None)
    state = tmp_path / "owner-state"
    if not search and not display:
        # A sibling main-checkout task changes files after the first became ready.
        helper.task_action(store.project_root, api.project_id, state, dict(action="auto_ready", taskId=task))
        (Path(store.project_root) / "sibling.py").write_text("pass\n")
    result = api.handle(query("results.get", "wr_" + execution))
    if search:
        assert result["acceptanceAvailable"], result["checks"]
        manual = [c for c in result["checks"] if c.get("kind") == "manual"]
        assert len(manual) == 1
        assert manual[0]["outcome"] == "unverified"
        assert manual[0]["rerunnable"] is False and not manual[0].get("command")
        assert not manual[0].get("technicalReason")
        # A second open runtime connection retains committed WAL frames. The live
        # product read must agree with results.get, without an inspection checkpoint.
        with store.connection() as live:
            live.execute("UPDATE runtime_tasks SET synthesis=synthesis WHERE task_id=?", (runtime_task,))
            assert Path(str(store.path) + "-wal").stat().st_size > 0
            current = helper.product_read(store.project_root, state, api.project_id)["tasks"][task]
            assert current["reviewActions"]["accept"] is True, current["reviewActions"]
        assert helper.product_read(store.project_root, state, api.project_id)["tasks"][task]["reviewActions"]["acknowledgeOpenChecks"] is False
    elif not display:
        assert not result["acceptanceAvailable"] and "workspace_drift" in result["warnings"]
    def display_event():
        before = api.handle(query("results.get", "wr_" + execution))
        def write(db):
            db.execute("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at) "
                       "VALUES (?,'progress','late-display','{}','runtime','now')", (runtime_task,))
        store.immediate_write(write)
        after = api.handle(query("results.get", "wr_" + execution))
        assert before["version"] == after["version"]
        return {}
    run_ui(api, state, dict(projectId=api.project_id, taskId=task, drift=not search and not display, display=display),
           "test/accept-persistence.test.tsx", extra=display_event)
    saved = helper.product_read(store.project_root, state, api.project_id)["tasks"][task]
    if search or display:
        assert saved["acceptance"]["digest"] == result["observedWorkspaceDigest"]
        assert saved["acceptance"]["open"] == dict(failed=0, not_run=0, unverified=0)
    else:
        assert not saved.get("acceptance")
    if display:
        from cortex_runtime.orchestration.state import event
        store.immediate_write(lambda db: event(db, runtime_task, "SYNTHESIS_SUBMITTED", dict(text="A new result")))
        fresh = api.handle(query("results.get", "wr_" + execution))
        assert fresh["version"] != result["version"]
        with pytest.raises(helper.Refused, match="stale_result"):
            helper.task_action(store.project_root, api.project_id, state,
                dict(action="accept", taskId=task, digest=result["observedWorkspaceDigest"], version=result["version"]))
