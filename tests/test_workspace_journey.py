"""Owner screens through the Rust socket service, real Python runtime and fake engine only."""
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

    spec = importlib.util.spec_from_file_location("journey_agent", ROOT / "crates/workspace-service/agent_launcher.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    project = Path.cwd().resolve()
    assert project != ROOT and ROOT not in project.parents, "child project must be disposable"
    from cortex_runtime.project_storage import project_store_directory
    store = Store(project_store_directory(project), project_root=project)
    # Receipt/control fixture only: a real disposable sleep process, not a containment test.
    # Production never imports this opt-in fake launch preparation.
    from cortex_runtime.orchestration import sandbox
    from sandbox_fakes import unjailed_gateway_argv, verification_command
    from cortex_runtime.orchestration import controller
    sandbox.compact_argv = unjailed_gateway_argv
    # The availability probe uses the same disposable, real-subprocess test seam.
    # This journey exercises owner controls, not nested namespace containment.
    controller._verification_command = verification_command

    def _worker_scenario(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
        with store.connection() as db:
            context = db.execute("SELECT state,revision FROM runtime_tasks WHERE task_id=?", (channel.task_id,)).fetchone()
        if channel.role == "coordinator":
            if context["state"] == "PLANNING" and index == 0:
                if on_delta:
                    on_delta("Planning two real fake-engine workers.")
                mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(tasks=[
                    dict(id="W1",role="backend-dev",objective="Receive owner guidance",dependencies=[],scope=["a.txt"],model="auto"),
                    dict(id="W2",role="backend-dev",objective="Wait for command control",dependencies=[],scope=["b.txt"],model="auto")],
                    verification=[["python3","-B","-c","pass"]]),request_key="worker-plan"))
            else:
                mcp_tools.dispatch(store, channel, "user_question_request", dict(question="Continue after the stopped worker?", options=["Later","Finish"],request_key="worker-stopped"))
            return PromptOutcome(StopReason.END_TURN, "Plan observed.")
        with store.connection() as db:
            spec = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND assignment_ref=(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)", (channel.task_id,channel.attempt_id)).fetchone()
        import json
        spec = json.loads(spec[0])
        if "Receive owner guidance" in str(spec):
            (project / "worker-one").touch()
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                reply = mcp_tools.dispatch(store, channel, "project_read", dict(path="a.txt"))
                if reply.get("owner_guidance"):
                    if (project / "expect-worker-resources").exists():
                        from cortex_runtime.acp.replay_store import read_source
                        guidance = reply["owner_guidance"][0]
                        assert len(guidance["attachments"]) == 3
                        for source in guidance["attachments"]:
                            if source["kind"] == "resource":
                                assert read_source(store, channel.task_id, source["source_ref"])["chunk"] in ("worker fixture", "worker picker")
                        (project / "worker-resources-received").touch()
                    assert reply["owner_guidance"][0]["text"] == "Keep the worker message verbatim\nsecond line"
                    (project / "worker-guidance-delivered").touch()
                    break
                time.sleep(.05)
            else:
                raise AssertionError("guidance was not delivered")
            if on_delta:
                on_delta("Worker received owner guidance and inspected the project.")
            if on_tool_call:
                if (project / "expect-older-history").exists():
                    for number in range(205):
                        on_tool_call("history-" + str(number), "read", "Retained fixture event " + str(number), "completed",
                                     None, None, {"path": "fixture-history/" + str(number) + ".txt"})
                on_tool_call("worker-read","read","Read project notes","completed", None, None, {"path": "a.txt"})
            (project / "worker-progress").write_text("draft")
            draft = store.call("create_draft",dict(template="general",request_key="worker-draft"),channel.principal_thread_id,channel.parent_principal_thread_id,trusted=True)
            mcp_tools.dispatch(store,channel,"draft_write",dict(draft_id=draft["draft_id"],content=draft["required_first_line"]+"\n\n# Guidance result\n\nReceived the owner guidance at a Cortex call. No files changed.\n",request_key="worker-body"))
            report = store.call("write_report",dict(draft_id=draft["draft_id"],title="Guidance received",summary="Observed guidance delivery",author="worker",request_key="worker-report"),channel.principal_thread_id,channel.parent_principal_thread_id,trusted=True)
            (project / "worker-progress").write_text("submit")
            mcp_tools.dispatch(store,channel,"result_submit",dict(report=report["report_id"],summary="Worker guidance completed.",checks=[],request_key="worker-result"))
            return PromptOutcome(StopReason.END_TURN, "Worker guidance completed.")
        (project / "worker-two").touch()
        reply = mcp_tools.dispatch(store,channel,"command_run",dict(argv=["python3","-B","-c","import time; time.sleep(110)"],request_key="held-command",timeout_seconds=120))
        if reply["failure"] != "stopped_by_user":
            raise AssertionError("command failure code: " + str(reply["failure"]) + "; exit=" + str(reply["exit_code"]))
        (project / "worker-command-stopped").touch()
        deadline = time.monotonic()+30
        while time.monotonic()<deadline:
            with store.connection() as db:
                stopped = db.execute("SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?", (channel.task_id, spec.get("assignment_ref", ""))).fetchone()
                revoked = db.execute("SELECT revoked_at FROM trusted_channels WHERE channel_id=?", (channel.channel_id,)).fetchone()
            if not engine.lookup_owned(channel.attempt_id) == "alive" or revoked[0]:
                break
            time.sleep(.05)
        return PromptOutcome(StopReason.END_TURN, "Stopped worker.")

    def worker_scenario(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
        try:
            return _worker_scenario(store,channel,ref,message,index,on_delta,on_tool_call,cancel_event)
        except Exception as error:
            import json, traceback
            with (project / "worker-errors").open("a") as out:
                json.dump(dict(type=type(error).__name__,code=str(error) if type(error).__name__ in ("StoreError","AssertionError") else None,line=traceback.extract_tb(error.__traceback__)[-1].lineno),out)
                out.write("\n")
            raise

    def router(store, channel, ref, launch, instructions):
        if (project / "start-worker-scenario").exists():
            return worker_scenario
        def behavior(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
            with (Path.cwd() / "turns").open("a") as stream:
                stream.write(str(index) + "\n")
            if "Owner review feedback" in str(message):
                (project / "review-returned").touch()
                mcp_tools.dispatch(store, channel, "answer_submit", {"text": "Runtime-owned journey result.\nReview feedback returned to Cortex.", "request_key": "reviewed-result"})
                return PromptOutcome(StopReason.END_TURN, "Review feedback observed.")
            if (project / "expect-native-answer-queue").exists() and index >= 1:
                from cortex_runtime.acp.replay_store import list_sources, read_source
                import hashlib
                expected = ["answer fixture", "answer picker"] + (["queue fixture", "queue picker"] if index >= 2 else [])
                sources = list_sources(store, channel.task_id)["entries"]
                for text in expected:
                    digest = hashlib.sha256(text.encode()).hexdigest()
                    source = next(s for s in sources if s["kind"] == "resource" and s["sha256"] == digest)
                    assert source["availability"] == "stored"
                    assert read_source(store, channel.task_id, source["source_ref"])["chunk"] == text
                assert any(s["kind"] == "image" and s["availability"] == "stored" for s in sources)
                (project / ("native-steer-resources-received" if index >= 2 else "native-answer-resources-received")).touch()
            if index >= 2:
                if (project / "expect-attachments").exists():
                    from cortex_runtime.acp.replay_store import list_sources, read_source
                    import hashlib
                    expected = {hashlib.sha256(b"answer fixture").hexdigest(), hashlib.sha256(b"queue fixture").hexdigest()}
                    sources = list_sources(store, channel.task_id)["entries"]
                    resources = [s for s in sources if s["kind"] == "resource"]
                    assert expected.issubset({s["sha256"] for s in resources})
                    assert "attachment" in str(message)
                    for source in resources:
                        assert source["availability"] == "stored"
                        assert read_source(store, channel.task_id, source["source_ref"])["chunk"] in ("answer fixture", "queue fixture")
                    (project / "resources-received").touch()
                assert "Focus on durable conversation" in str(message)
                (Path.cwd() / "steer-applied").touch()
                mcp_tools.dispatch(store, channel, "answer_submit", {"text": "Runtime-owned journey result.", "request_key": "steered-result"})
                return PromptOutcome(StopReason.END_TURN, "Steered outcome.")
            if index == 0:
                if (project / "expect-create-resources").exists():
                    from cortex_runtime.acp.replay_store import list_sources, read_source
                    resources = [s for s in list_sources(store, channel.task_id)["entries"] if s["kind"] in ("resource", "image")]
                    assert len(resources) == 3 and all(s["availability"] == "stored" for s in resources)
                    for source in resources:
                        if source["kind"] == "resource":
                            assert read_source(store, channel.task_id, source["source_ref"])["chunk"] in ("creation fixture", "creation picker")
                    (project / "creation-resources-received").touch()
                mcp_tools.dispatch(store, channel, "user_question_request", {
                    "question": "Which direction for this project?", "options": ["A", "B"], "request_key": "ask"})
                return PromptOutcome(StopReason.END_TURN, "Waiting, not complete.")
            if (project / "expect-attachments").exists():
                from cortex_runtime.acp.replay_store import list_sources, read_source
                resources = [s for s in list_sources(store, channel.task_id)["entries"] if s["kind"] == "resource"]
                assert len(resources) == 1 and resources[0]["availability"] == "stored"
                assert read_source(store, channel.task_id, resources[0]["source_ref"])["chunk"] == "answer fixture"
                assert "notes.txt" in str(message)
                (project / "answer-resource-received").touch()
            if on_delta:
                on_delta("Streaming from the fake engine.")
            if on_tool_call:
                on_tool_call("journey-read", "read", "Inspect project notes", "completed")
            (Path.cwd() / "streaming").touch()
            deadline = time.monotonic() + 120
            while not (Path.cwd() / "release-stream").exists() and time.monotonic() < deadline and not cancel_event.is_set():
                time.sleep(0.02)
            assert (Path.cwd() / "release-stream").exists(), "stream was not released"
            with store.connection() as db:
                answers = db.execute("SELECT answer_text FROM pending_questions WHERE task_id=?", (channel.task_id,)).fetchall()
            assert [row[0] for row in answers] == ["first line\nsecond line"]
            with store.connection() as db:
                steered = db.execute("SELECT 1 FROM source_events WHERE task_id=? AND origin LIKE \'paseo-steer:%\'", (channel.task_id,)).fetchone()
            if steered:
                return PromptOutcome(StopReason.END_TURN, "Applying your new guidance at the boundary.")
            mcp_tools.dispatch(store, channel, "answer_submit", {
                "text": "Runtime-owned journey result.", "request_key": "result"})
            return PromptOutcome(StopReason.END_TURN, "Not the result text.")
        def guarded(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
            try:
                return behavior(store, channel, ref, message, index, on_delta, on_tool_call, cancel_event)
            except Exception as error:
                import traceback
                (project / "worker-errors").write_text(type(error).__name__ + ":" + str(traceback.extract_tb(error.__traceback__)[-1].lineno))
                raise
        return guarded

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)

    class TestAgent(module.ServiceAgent):
        def _default_selection_from_user_defaults(self):
            return None
        def _config_options_for_session(self, *args):
            return []
        def _post_session_new_notifications(self, session_id):
            pass

    module.run(runtime=runtime, agent_type=TestAgent)


def test_owner_screens_real_runtime_question_and_stream_reopen(tmp_path):
    from test_workspace_service_integration import Client, service_binary
    binary = service_binary()
    projects = tmp_path / "projects"
    project = projects / "journey"
    project.mkdir(parents=True)
    (project / "a.txt").write_text("Disposable project notes.\n")
    identity = ["-c", "user.name=Owner", "-c", "user.email=owner@example.invalid"]
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=project, check=True)
    subprocess.run(["git", *identity, "commit", "-q", "--allow-empty", "-m", "Journey history"], cwd=project, check=True)
    home = tmp_path / "home"
    home.mkdir()
    for name in ("config", "state", "cache", "runtime"):
        (home / name).mkdir(mode=0o700)
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
               XDG_CACHE_HOME=str(home / "cache"), XDG_RUNTIME_DIR=str(home / "runtime"),
               PYTHONDONTWRITEBYTECODE="1")
    with tempfile.TemporaryDirectory(prefix="cw-journey-") as scratch:
        runtime = Path(scratch) / "cortex-workspace-dev"
        env["XDG_RUNTIME_DIR"] = scratch
        process = subprocess.Popen([str(binary), str(projects), sys.executable, str(ROOT),
                                    "--fixture-launcher", str(Path(__file__).resolve())],
                                   env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 15
            while not (runtime / "service.sock").exists() and time.monotonic() < deadline:
                assert process.poll() is None, "service exited"
                time.sleep(0.02)
            assert (runtime / "service.sock").exists()
            # Only disposable, non-secret test endpoints are passed; none are printed.
            node_env = dict(os.environ, WORKSPACE_JOURNEY_SOCKET=str(runtime / "service.sock"),
                            WORKSPACE_JOURNEY_PROJECT=str(project))
            result = subprocess.run(["npm", "--prefix", "packages/workspace-ui", "test", "--",
                                     "test/runtime-journey.test.tsx"], cwd=ROOT, env=node_env, timeout=70)
            assert result.returncode == 0
            assert (project / "turns").read_text().splitlines()[:3] == ["0", "1", "2"]
            assert (project / "review-returned").is_file()
            client = Client(runtime / "service.sock")
            try:
                assert client.call("service.status", {})["children"] == 1
                client.call("service.stop", {})
            finally:
                client.close()
            assert process.wait(timeout=15) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=15)


if __name__ == "__main__":
    agent_child()
