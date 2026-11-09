"""Real UI port + WorkspaceAPI + machine helper; no model, build or owner data.

The private test socket replaces only native transport/registry discovery. All project
queries and actions use the production adapter/helper against disposable receipt-backed
stores. The ordinary test sandbox launch fixtures apply; this is not containment or
compiled desktop/UDS qualification.
"""
import json
import os
from pathlib import Path
import socketserver
import subprocess
import threading

import pytest

from cortex_runtime.workspace_api import WorkspaceAPI, task_options
from cortex_runtime.workspace_api.storage import bind_execution_tx
from test_workspace_api import create_command, query
from test_workspace_check_execution import prepared
from test_workspace_task_git import helper

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("prepared", [("withdrawn", False), ("withdrawn", True), ("missing", False)], indirect=True)
def test_ui_port_helper_contract(prepared, tmp_path, request):
    runtime, runtime_task, _, _ = prepared
    request_mode = request.node.callspec.params["prepared"][0]
    store = runtime._store
    project = "project-1"
    api = WorkspaceAPI(store, project_id=project, actor_id="local-owner")
    api.activate(request_id="activate-ui")
    task = api.handle(create_command())["target"]
    task_options.prepare(store, task, {})

    def finish(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('ui-session',?,?,'{}','{}','[]','now',NULL)",
                   (store.project_root, runtime_task))
        execution = bind_execution_tx(db, task, runtime_task, "ui-session")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Receipt-backed UI result',blocker=NULL WHERE task_id=?",
                   (runtime_task,))
        return execution

    execution = store.immediate_write(finish)
    result = api.handle(query("results.get", "wr_" + execution))
    from cortex_runtime.orchestration import worker_checks
    worker_checks.automatic(runtime, runtime_task, threading.Event(), None)
    result = api.handle(query("results.get", "wr_" + execution))
    technical = any(c.get("technicalReason") for c in result["checks"])
    blocked = any(c["outcome"] == "failed" for c in result["checks"])
    assert blocked == (request_mode == "missing")
    assert result["acceptanceAvailable"] is not blocked
    assert result["version"] != "1"  # Deliberately different from the display revision.
    state = tmp_path / "owner-state"
    has_git = helper.is_repo(store.project_root)
    # Mirror the real automatically-ready journey without confusing it with acceptance.
    if not technical and not blocked:
        helper.task_action(store.project_root, project, state, dict(action="auto_ready", taskId=task))
    assert not helper.product_read(store.project_root, state, project)["tasks"][task].get("acceptance")

    from test_workspace_review_recovery import settled_agent
    from cortex_runtime.workspace_api.control import handle as control
    host = settled_agent(api)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            message = json.loads(self.rfile.readline())
            operation, body = message["operation"], message["message"]
            try:
                if operation == "fixture":
                    value = dict(projectId=project, taskId=task, root=store.project_root, hasGit=has_git, technical=technical, blocked=blocked)
                elif operation == "fixture.result":
                    value = api.handle(query("results.get", "wr_" + execution))
                elif operation == "service.capabilities":
                    value = dict(methods=["machine.call", "projects.list", "executions.control"], machine=True)
                elif operation == "projects.list":
                    value = [dict(projectId=project, displayName="Contract project",
                                  canonicalRoot=store.project_root, activation={})]
                elif operation == "executions.control":
                    value = control(host, body)
                elif operation == "machine.call":
                    # Fail closed for unrelated machine/account discovery: this fixture only
                    # grants project facts/actions. No account/settings/usage reads are needed.
                    if body.get("projectId") != project:
                        value = dict(ok=False, code="project_required", message="Project calls only in this fixture.")
                    else:
                        try:
                            value = dict(ok=True, result=helper.run(dict(
                                body, root=store.project_root, stateDir=str(state))))
                        except helper.Refused as error:
                            value = dict(ok=False, code=error.code, message=error.message)
                else:
                    value = api.handle(dict(body, operation=operation, projectId=project))
                reply = dict(result=value)
            except Exception as error:
                reply = dict(error=type(error).__name__ + ": " + str(error))
            self.wfile.write((json.dumps(reply) + "\n").encode())

    # A short pathname stays within AF_UNIX's bound even under a long checkout root.
    import tempfile
    with tempfile.TemporaryDirectory(prefix="cw-ui-") as sockets:
        server = socketserver.UnixStreamServer(sockets + "/rpc", Handler)
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.02})
        thread.start()
        try:
            env = dict(os.environ, WORKSPACE_CONTRACT_SOCKET=sockets + "/rpc")
            done = subprocess.run(
                ["timeout", "90", "npm", "--prefix", "packages/workspace-ui", "test", "--",
                 "test/helper-contract.test.tsx", "--testTimeout=20000", "--hookTimeout=20000"],
                cwd=ROOT, env=env, text=True, capture_output=True, timeout=100)
            assert done.returncode == 0, done.stdout + done.stderr
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
            assert not thread.is_alive()

    result = api.handle(query("results.get", "wr_" + execution))
    saved = helper.product_read(store.project_root, state, project)["tasks"][task]
    if blocked:
        assert not saved.get("acceptance")
        assert saved["reviewActions"]["accept"] is False
        assert saved["reviewActions"]["unblockAction"] == "rerun_checks"
    else:
        assert saved["status"] == "ready_to_merge"
        assert saved["acceptance"]["version"] == result["version"]
        assert saved["acceptance"]["digest"] == result["observedWorkspaceDigest"]
    # The UI's explicit init creates only Git metadata, not a commit or a remote.
    assert helper.is_repo(store.project_root)
    if not has_git:
        assert helper.git(store.project_root, "rev-parse", "--verify", "HEAD", check=False).returncode != 0
        assert not helper.out(store.project_root, "remote")
    assert (Path(store.project_root) / "feature.py").read_text() == "pass\n"


@pytest.mark.parametrize("prepared", [("failed", False)], indirect=True)
def test_worker_overview_keeps_real_failure_and_unbound_withdrawal(prepared, tmp_path):
    runtime, runtime_task, _, _ = prepared
    store = runtime._store
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate-failed")
    task = api.handle(create_command())["target"]

    def bind(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('failed-session',?,?,'{}','{}','[]','now',NULL)",
                   (store.project_root, runtime_task))
        bind_execution_tx(db, task, runtime_task, "failed-session")

    store.immediate_write(bind)
    def checks():
        rows = helper.workers_of(store.project_root, "project-1", [task])
        return next(w for w in rows if w["lane"] == "execution")["result"]["checks"]

    assert checks()[0]["outcome"] == "failed"
    # Unbound presentation metadata is not runtime withdrawal evidence.
    with store.connection() as db:
        row = db.execute("SELECT assignment_ref,result_json FROM runtime_nodes WHERE task_id=? AND stage='execution'",
                         (runtime_task,)).fetchone()
        result = json.loads(row["result_json"])
        result["checks"][0].update(retracted_by="probe_withdrawn", current_outcome="retracted")
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?",
                   (json.dumps(result), row["assignment_ref"]))
        db.commit()
    assert checks()[0]["outcome"] == "failed"
