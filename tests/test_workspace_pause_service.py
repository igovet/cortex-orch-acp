"""Stop/resume via real Rust service, helper and runtime; engines are fakes only."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def agent_child():
    sys.path[:0] = [str(ROOT / "src"), str(ROOT / "tests")]
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource
    import cortex_runtime.engine.session_manager as native_manager
    def forbidden(*a, **kw):
        raise AssertionError("native engines forbidden")
    native_manager.EngineSessionManager = forbidden
    spec = importlib.util.spec_from_file_location("pause_agent", ROOT / "crates/workspace-service/agent_launcher.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    root = Path.cwd().resolve()
    assert root != ROOT and ROOT not in root.parents
    from cortex_runtime.project_storage import project_store_directory
    store = Store(project_store_directory(root), project_root=root)
    from cortex_runtime.orchestration import sandbox
    from sandbox_fakes import unjailed_gateway_argv
    sandbox.compact_argv = unjailed_gateway_argv
    def behavior(store, channel, ref, message, index, cancel_event=None, **kw):
        from cortex_runtime.orchestration.state import TaskMachine
        import json
        state = TaskMachine(store).context(channel.task_id)
        if state["goal"].startswith("worker-"):
            if channel.role == "coordinator":
                if state["state"] == "PLANNING":
                    tasks = [dict(id="W", role="backend-dev", objective=state["goal"], dependencies=[], scope=["out.txt"], model="auto")]
                    if state["goal"] == "worker-access":
                        tasks.insert(0, dict(id="H", role="backend-dev", objective="Hold scope", dependencies=[], scope=["held.txt"], model="auto"))
                    mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(tasks=tasks,
                        verification=[["python3", "-B", "-c", "pass"]]), request_key="plan"))
                else:
                    mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Continued the saved worker wait."))
                return PromptOutcome(StopReason.END_TURN, "Plan")
            with store.connection() as db:
                node = db.execute("SELECT n.* FROM runtime_nodes n JOIN delegation_provisioning p ON n.assignment_ref=p.idempotency_key WHERE p.attempt_id=?",
                                  (channel.attempt_id,)).fetchone()
            if node["node_id"] == "H" and not (root / "release").exists():
                while not cancel_event.is_set():
                    time.sleep(.02)
                return PromptOutcome(StopReason.CANCELLED, "Held")
            if node["stage"] != "review" and node["node_id"] != "H":
                if not (root / "worker-wait").exists():
                    if state["goal"] in ("worker-question", "worker-history"):
                        if state["goal"] == "worker-history":
                            from cortex_runtime.delegation.events import append_event
                            from cortex_runtime.orchestration.state import event
                            def history(db):
                                for n in range(215):
                                    event(db, channel.task_id, "COMMAND_PROCESS_STARTED",
                                          dict(tool_call_ref=f"history-command-{n}"), channel.channel_id)
                                    event(db, channel.task_id, "WORKER_GUIDANCE",
                                          dict(channelId=channel.channel_id, requestId=f"guidance-{n}",
                                               text=f"Guidance {n} " + "界😀\\\"" * (20000 if n == 100 else 20),
                                               attachmentNames=[dict(name=f"attachment-{n}.txt")]))
                            store.immediate_write(history)
                            for n in range(215):
                                append_event(store, channel.task_id, "progress", f"history-{n}",
                                    dict(kind="worker_activity", text=f"Activity {n}"), assignment_ref=channel.attempt_id, origin="fixture")
                        receipt = mcp_tools.dispatch(store, channel, "ask_question",
                            dict(question="Keep this worker question?", options=["Continue", "Later"], request_key="worker-ask"))
                    else:
                        receipt = mcp_tools.dispatch(store, channel, "access_request",
                            dict(paths=["held.txt"], reason="Need the held path", request_key="worker-access"))
                        assert receipt["status"] == "pending", receipt
                    (root / "worker-wait").write_text(json.dumps(dict(receipt=receipt, attempt=channel.attempt_id, session=ref)))
                    return PromptOutcome(StopReason.END_TURN, "Wait")
                assert ("Runtime recorded wait continuation" if index == 0 else "Runtime access decision continuation") in message
                if state["goal"] in ("worker-question", "worker-history"):
                    assert "Continue" in message and "Keep this worker question?" in message
                else:
                    if index == 0:
                        assert "access_decision" in message and "held.txt" in message
                        receipt = mcp_tools.dispatch(store, channel, "access_request",
                            dict(paths=["held.txt"], reason="Revalidate replacement scope", request_key="replacement-access"))
                        if receipt["status"] == "pending":
                            return PromptOutcome(StopReason.END_TURN, "Revalidate")
                        assert receipt["status"] in ("granted", "already_covered"), receipt
                    else:
                        assert '"status":"granted"' in message or '"status":"already_covered"' in message
                    mcp_tools.dispatch(store, channel, "project_write",
                        dict(path="held.txt", content="revalidated", expected_sha256="absent", request_key="held"))
                (root / "worker-continued").write_text(json.dumps(dict(attempt=channel.attempt_id, session=ref)))
                mcp_tools.dispatch(store, channel, "project_write",
                    dict(path="out.txt", content="continued", expected_sha256="absent", request_key="out"))
            result = dict(report="# Continued\n\nThe recorded worker wait continued.", report_title="Continued",
                          summary="Continued", checks=[], request_key="result")
            if node["stage"] == "review":
                result["verdict"] = "APPROVE"
            mcp_tools.dispatch(store, channel, "result_submit", result)
            return PromptOutcome(StopReason.END_TURN, "Done")
        context = mcp_tools.dispatch(store, channel, "task_get_context", {})
        import json
        with (root / "fixture-turns").open("a") as output:
            output.write(json.dumps(dict(state=context["state"], answers=len(context["user_answers"]))) + "\n")
        (root / "prompt-entered").touch()
        if context["goal"] in ("waiting", "batch") and not context["user_answers"]:
            form = dict(question="Preserve this exact question?", options=["Continue", "Later"])
            payload = dict(questions=[form, dict(question="Choose format?", options=["JSON","Text"])],request_key="ask") if context["goal"] == "batch" else dict(**form,request_key="ask")
            mcp_tools.dispatch(store, channel, "user_question_request", payload)
            return PromptOutcome(StopReason.END_TURN, "Waiting")
        if not (root / "release").exists():
            deadline = time.monotonic() + 25
            while not cancel_event.is_set() and time.monotonic() < deadline:
                time.sleep(.02)
            return PromptOutcome(StopReason.CANCELLED, "Interrupted")
        mcp_tools.dispatch(store, channel, "answer_submit", dict(text="Resumed the recorded task.", request_key="answer"))
        return PromptOutcome(StopReason.END_TURN, "Done")
    def traced_behavior(store, channel, ref, message, index, cancel_event=None):
        try:
            return behavior(store, channel, ref, message, index, cancel_event=cancel_event)
        except Exception as error:
            import traceback
            with (root / "fixture-errors").open("a") as out:
                out.write(f"{channel.role}:{type(error).__name__}:line={traceback.extract_tb(error.__traceback__)[-1].lineno}\n")
            raise
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, lambda *a: traced_behavior), FakeWorkspace(),
        FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    class Agent(module.ServiceAgent):
        def _default_selection_from_user_defaults(self): return None
        def _config_options_for_session(self, *a): return []
        def _post_session_new_notifications(self, *a): pass
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
    production = ProductionOrchestratingRuntime(environ={})
    production._runtimes[str(root)] = runtime
    module.run(runtime=production, agent_type=Agent)


if __name__ == "__main__":
    agent_child()
    raise SystemExit(0)

import pytest

@pytest.mark.parametrize("mode,restart", [(m, True) for m in ("running", "waiting", "between", "batch")] + [(m, r) for m in ("worker-question", "worker-access", "worker-history") for r in (False, True)])
def test_stop_restart_resume(mode, restart, tmp_path):
    import json
    def json_bytes(value): return json.dumps(value, ensure_ascii=False).encode("utf-8")
    from test_workspace_service_integration import Client, service_binary
    binary = service_binary()
    projects = tmp_path / "projects"
    project = projects / "fixture"
    project.mkdir(parents=True)
    home = tmp_path / "home"
    home.mkdir()
    for name in ("config", "state", "cache"):
        (home / name).mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix="cw-pause-") as scratch:
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
        env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
            XDG_CACHE_HOME=str(home / "cache"), XDG_RUNTIME_DIR=scratch, PYTHONDONTWRITEBYTECODE="1")
        endpoint = Path(scratch) / "cortex-workspace-dev/service.sock"
        process = None
        def launch():
            return subprocess.Popen([str(binary), str(projects), sys.executable, str(ROOT),
                "--fixture-launcher", str(Path(__file__).resolve())], env=env, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def call(method, params):
            c = Client(endpoint)
            try: return c.call(method, params)
            finally: c.close()
        def wait(fn):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                try:
                    value = fn()
                except (ConnectionRefusedError, FileNotFoundError):
                    value = None
                if value: return value
                time.sleep(.03)
            latest = call("workspace", dict(protocol=dict(major=1, minor=5), requestId="diagnostic", projectId=pid, operation="tasks.list")) if "pid" in locals() else {}
            pytest.fail("fixture state did not arrive; projection=" + str(latest) + "; turns=" + ((project / "fixture-turns").read_text() if (project / "fixture-turns").exists() else "none") + "; fixture errors=" + ((project / "fixture-errors").read_text() if (project / "fixture-errors").exists() else "none"))
        try:
            process = launch()
            wait(endpoint.exists)
            pid = call("projects.register", dict(root=str(project), displayName="Fixture"))["projectId"]
            call("projects.activate", dict(projectId=pid))
            def q(op, target=None):
                return call("workspace", dict(protocol=dict(major=1, minor=5), requestId="read",
                    projectId=pid, operation=op, target=target))
            create = dict(protocol=dict(major=1, minor=5), requestId="create", projectId=pid,
                actorId="ignored", operation="tasks.create", target=pid, expectedVersion="0",
                ownerEpoch=None, payload=dict(description=mode if mode != "between" else "running"))
            task = call("workspace", create)["target"]
            call("workspace", dict(create, requestId="start", operation="tasks.start", target=task,
                expectedVersion=q("tasks.get", task)["version"], payload={}))
            wait(lambda: (project / ("worker-wait" if mode.startswith("worker-") else "prompt-entered")).exists())
            row = wait(lambda: q("tasks.get", task) if q("tasks.get", task)["executionId"] else None)
            execution = row["executionId"]
            asked = None
            if mode in ("waiting", "batch", "worker-question", "worker-history"):
                asked = wait(lambda: (q("decisions.list", execution)["items"] or [None])[0])
            elif mode == "between":
                call("executions.cancel", dict(projectId=pid, executionId=execution))
                wait(lambda: q("tasks.get", task)["executionState"] == "recovering")
                time.sleep(.2)
            def control(action, key):
                return call("executions.control", dict(projectId=pid, executionId=execution,
                    action=action, requestId=key, expectedVersion=q("tasks.get", task)["version"]))
            if mode == "worker-history":
                def history(cursor=None):
                    reply = call("machine.call", dict(op="workers", projectId=pid,
                        params=dict(taskIds=[task], **(dict(workerId=worker_id, cursor=cursor) if cursor else {}))))
                    assert reply["ok"], reply
                    return reply["result"][0]
                first = history()
                worker_id = first["id"]
                seen, guidance, cursor = {}, {}, None
                for page_no in range(100):
                    page = first if page_no == 0 else history(cursor)
                    assert len(json_bytes(page)) <= 65536
                    assert page["activityPartial"] == bool(page["activityCursor"])
                    for item in page["activity"]:
                        assert item["sequence"] not in seen
                        seen[item["sequence"]] = item
                    for message in page["messages"]:
                        assert message["id"] not in guidance
                        guidance[message["id"]] = message
                    cursor = page["activityCursor"]
                    if cursor is None:
                        break
                else:
                    pytest.fail("Worker history did not terminate")
                assert len([a for a in seen.values() if a.get("toolCallRef")]) == 215
                assert len([a for a in seen.values() if a["text"].startswith("Activity ")]) == 215
                assert len(guidance) == 215
                assert guidance["guidance-0"]["attachments"] == ["attachment-0.txt"]
                assert "[Content clipped:" in guidance["guidance-100"]["text"]
                # Page reads do not depend on the helper process or drop its runtime namespace.
                assert history(first["activityCursor"])["activity"]
            time.sleep(.2)  # Let the initial start/owner observation finish before taking its CAS version.
            assert control("pause", "pause")["outcome"] == "recorded"
            assert q("tasks.get", task)["executionState"] == "paused"
            if asked:
                kept = q("decisions.list", execution)["items"][0]
                assert kept["questionId"] == asked["questionId"] and kept["status"] == "pending"
                assert kept["allowedOperations"] == []
            if restart:
                call("service.stop", {})
                assert process.wait(timeout=10) == 0
                process = launch()
                wait(lambda: call("service.status", {}))
                wait(lambda: call("projects.status", dict(projectId=pid)).get("ready"))
            assert q("tasks.get", task)["executionState"] == "paused"
            (project / "release").touch()
            def resume():
                try:
                    return control("resume", "resume")
                except AssertionError as error:
                    if "pause_settling" not in str(error):
                        raise
            assert wait(resume)["outcome"] == "recorded"
            if asked:
                all_restored = q("decisions.list", execution)["items"]
                restored = all_restored[0]
                assert len(all_restored) == (2 if mode == "batch" else 1)
                assert restored["questionId"] == asked["questionId"]
                assert restored["status"] == "pending" and restored["allowedOperations"] == ["decisions.answer"]
                answer = dict(create, requestId="answer", operation="decisions.answer",
                    target=restored["batchId"] or restored["decisionId"], expectedVersion=restored["version"], ownerEpoch=restored["ownerEpoch"],
                    payload=dict(executionId=execution, acpSessionId=restored["acpSessionId"], batchId=restored["batchId"],
                        answers=[dict(decisionId=d["decisionId"], questionId=d["questionId"],
                            expectedVersion=d["version"], optionId=None, text="Continue") for d in all_restored]))
                assert call("workspace", answer)["outcome"] == "recorded"
            if mode.startswith("worker-"):
                wait(lambda: (project / "worker-continued").exists())
                import json
                old = json.loads((project / "worker-wait").read_text())
                new = json.loads((project / "worker-continued").read_text())
                assert new["attempt"] != old["attempt"]
                wait(lambda: (project / "out.txt").exists())
                assert (project / "out.txt").read_text() == "continued"
                if mode == "worker-access":
                    assert (project / "held.txt").read_text() == "revalidated"
                wait(lambda: q("tasks.get", task)["executionState"] == "result")
                import sqlite3
                from cortex_runtime.orchestration.worker_waits import projection
                with sqlite3.connect("file:" + str(project / ".cortex-dev/cortex.sqlite3") + "?mode=ro", uri=True) as db:
                    db.row_factory = sqlite3.Row
                    runtime_id = db.execute("SELECT runtime_task_id FROM workspace_executions WHERE execution_id=?", (execution,)).fetchone()[0]
                    bound = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_WAIT_CONTINUATION_BOUND'", (runtime_id,)).fetchall()
                    graph_status = db.execute("SELECT a.status FROM work_assignment_attempts a JOIN task_events e "
                        "ON a.attempt_id=json_extract(e.payload_json,'$.work_attempt_ref') JOIN delegation_provisioning p "
                        "ON p.idempotency_key=e.assignment_ref WHERE p.attempt_id=? AND e.event_type='delegation_requested'",
                        (old["attempt"],)).fetchone()[0]
                    assert graph_status not in ("running", "dispatched"), graph_status
                assert len(bound) == 1, [(d["previous_attempt"], d["attempt_id"], d["wait_id"]) for b in bound for d in [json.loads(b[0])]]
                data = json.loads(bound[0][0])
                assert data["previous_attempt"] == old["attempt"] and data["attempt_id"] == new["attempt"]
                with sqlite3.connect("file:" + str(project / ".cortex-dev/cortex.sqlite3") + "?mode=ro", uri=True) as db:
                    db.row_factory = sqlite3.Row
                    assert projection(db, runtime_id)[0]["status"] == "continued"
            else:
                wait(lambda: q("tasks.get", task)["executionState"] == "result")
                assert q("results.get", "wr_" + execution)["text"] == "Resumed the recorded task."
            call("service.stop", {})
            assert process.wait(timeout=10) == 0
        finally:
            if process and process.poll() is None:
                process.terminate()
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def test_resume_uses_the_version_shown_after_stop(tmp_path):
    """A window reads the task once after Stop answers, shows Resume, and sends that version.

    The interrupted turn finishing in the background must not make that Resume stale.
    """
    from test_workspace_service_integration import Client, service_binary
    binary = service_binary()
    projects = tmp_path / "projects"
    project = projects / "fixture"
    project.mkdir(parents=True)
    home = tmp_path / "home"
    home.mkdir()
    for name in ("config", "state", "cache"):
        (home / name).mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix="cw-pause-") as scratch:
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
        env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
            XDG_CACHE_HOME=str(home / "cache"), XDG_RUNTIME_DIR=scratch, PYTHONDONTWRITEBYTECODE="1")
        endpoint = Path(scratch) / "cortex-workspace-dev/service.sock"
        process = subprocess.Popen([str(binary), str(projects), sys.executable, str(ROOT),
            "--fixture-launcher", str(Path(__file__).resolve())], env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def call(method, params):
            c = Client(endpoint)
            try: return c.call(method, params)
            finally: c.close()
        def wait(fn, seconds=15):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                try:
                    value = fn()
                except (ConnectionRefusedError, FileNotFoundError):
                    value = None
                if value: return value
                time.sleep(.03)
            pytest.fail("fixture state did not arrive")
        try:
            wait(endpoint.exists)
            pid = call("projects.register", dict(root=str(project), displayName="Fixture"))["projectId"]
            call("projects.activate", dict(projectId=pid))
            def q(op, target=None):
                return call("workspace", dict(protocol=dict(major=1, minor=5), requestId="read",
                    projectId=pid, operation=op, target=target))
            create = dict(protocol=dict(major=1, minor=5), requestId="create", projectId=pid,
                actorId="ignored", operation="tasks.create", target=pid, expectedVersion="0",
                ownerEpoch=None, payload=dict(description="running"))
            task = call("workspace", create)["target"]
            call("workspace", dict(create, requestId="start", operation="tasks.start", target=task,
                expectedVersion=q("tasks.get", task)["version"], payload={}))
            wait(lambda: (project / "prompt-entered").exists())
            execution = wait(lambda: q("tasks.get", task)["executionId"])
            time.sleep(.2)  # Let the initial start/owner observation finish before taking its CAS version.
            assert call("executions.control", dict(projectId=pid, executionId=execution, action="pause",
                requestId="pause", expectedVersion=q("tasks.get", task)["version"]))["outcome"] == "recorded"
            shown = q("tasks.get", task)
            assert shown["executionState"] == "paused"
            time.sleep(1.5)  # The owner reads the screen; nothing they can see has changed.
            assert q("tasks.get", task)["executionState"] == "paused"
            (project / "release").touch()
            reply = call("executions.control", dict(projectId=pid, executionId=execution, action="resume",
                requestId="resume", expectedVersion=shown["version"]))
            assert reply["outcome"] == "recorded", reply
            wait(lambda: q("tasks.get", task)["executionState"] == "result")
            call("service.stop", {})
            assert process.wait(timeout=10) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == "__main__":
    agent_child()
