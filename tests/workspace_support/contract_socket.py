"""Test-only transport; real API/helper/control, no native models or owner settings."""
import json
import os
import socketserver
import subprocess
import tempfile
import threading
from pathlib import Path

from test_workspace_task_git import helper
from test_workspace_review_recovery import settled_agent
from cortex_runtime.workspace_api.control import handle

ROOT = Path(__file__).resolve().parents[2]


def run_ui(api, state, fixture, filename, extra=None):
    host = settled_agent(api)
    counts, errors = {}, {}
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            message = json.loads(self.rfile.readline())
            op, body = message["operation"], message["message"]
            counts[op] = counts.get(op, 0) + 1
            try:
                if extra and op == "fixture.tick":
                    value = extra()
                elif op == "fixture":
                    value = fixture
                elif op == "service.capabilities":
                    value = dict(methods=["machine.call", "projects.list", "executions.prompt", "executions.control"], machine=True)
                elif op == "projects.list":
                    value = [dict(projectId=api.project_id, displayName="Private fixture", canonicalRoot=api.store.project_root, activation={})]
                elif op in ("executions.prompt", "executions.control"):
                    value = handle(host, dict(body, action="prompt") if op == "executions.prompt" else body)
                elif op == "machine.call":
                    if body.get("op") == "config.read" and extra:
                        value = dict(ok=True, result=dict(config=dict(catalogue={engine:dict(models=[dict(id="fixture", label="Fixture")] if engine == "codex-acp" else []) for engine in ("codex-acp", "claude-agent-acp", "opencode")})))
                    elif body.get("projectId") != api.project_id:
                        value = dict(ok=False, code="project_required", message="Project-only fixture.")
                    else:
                        try:
                            value = dict(ok=True, result=helper.run(dict(body, root=api.store.project_root, stateDir=str(state))))
                        except helper.Refused as error:
                            value = dict(ok=False, code=error.code, message=error.message)
                else:
                    value = api.handle(dict(body, operation=op, projectId=api.project_id))
                reply = dict(result=value)
            except Exception as error:
                # Never dump copied private source/report content on failure.
                errors[op] = type(error).__name__
                reply = dict(error=type(error).__name__)
            self.wfile.write((json.dumps(reply) + "\n").encode())
    with tempfile.TemporaryDirectory(prefix="cw-contract-") as sockets:
        server = socketserver.UnixStreamServer(sockets + "/rpc", Handler)
        thread = threading.Thread(target=server.serve_forever, kwargs=dict(poll_interval=.02))
        thread.start()
        try:
            env = dict(os.environ, WORKSPACE_CONTRACT_SOCKET=sockets + "/rpc")
            result = subprocess.run(["timeout", "90", "npm", "--prefix", "packages/workspace-ui", "test", "--",
                                     filename, "--testTimeout=20000", "--hookTimeout=20000"],
                                    cwd=ROOT, env=env, capture_output=True, text=True, timeout=100)
            assert result.returncode == 0, str(dict(counts=counts, errors=errors)) + result.stdout + result.stderr
        finally:
            server.shutdown(); server.server_close(); thread.join(5)
            assert not thread.is_alive()
