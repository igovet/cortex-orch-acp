"""No-paid walking slice through the Rust UDS service and real ACP Agent.

The child mode injects the real OrchestratingRuntime with fake sessions. Neither
mode constructs ProductionOrchestratingRuntime or a native engine.
"""
import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
import socket
import struct
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("WORKSPACE_TEST_SERVICE_BINARY", str(ROOT / "target/debug/workspace-service")))
LAUNCHER = ROOT / "crates/workspace-service/agent_launcher.py"


def service_binary(binary=BINARY):
    """The service build to launch, refused when older than the sources in it.

    These tests run the real binary, so an old build tests old code and fails far
    from the cause. Cargo's dep-info beside the binary lists its exact inputs.
    """
    build = "cargo build --locked -p workspace-service"
    assert binary.is_file(), f"run {build} first"
    record = binary.with_name(binary.name + ".d")
    if not record.is_file():
        # A packaged binary chosen by the caller has no cargo record to compare.
        assert "WORKSPACE_TEST_SERVICE_BINARY" in os.environ, f"run {build} first"
        return binary
    inputs = [re.sub(r"\\(.)", r"\1", path) for path in
              re.findall(r"(?:\\.|[^\s\\])+", record.read_text().split(": ", 1)[1])]
    built = binary.stat().st_mtime
    stale = [path for path in inputs if not Path(path).is_file() or Path(path).stat().st_mtime > built]
    assert not stale, f"run {build}: {binary} is older than {stale[0]}"
    return binary


def _agent_child():
    sys.path.insert(0, str(ROOT / "src"))
    sys.path.insert(0, str(ROOT / "tests"))
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from orchestration_fakes import FakeSessionRuntime, FakeConfigSnapshotSource, FakeWorkspace

    spec = importlib.util.spec_from_file_location("workspace_service_agent", LAUNCHER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    project = Path.cwd().resolve()
    assert project != ROOT and ROOT not in project.parents, "child project must be disposable"
    from cortex_runtime.project_storage import project_store_directory
    store = Store(project_store_directory(project), project_root=project)

    def router(store, channel, ref, launch, instructions):
        def behavior(store, channel, ref, message, index):
            # Real channel dispatch, real durable runtime result, no paid process.
            with (Path.cwd() / "fake-runtime-calls").open("a") as calls:
                calls.write(channel.task_id + "\n")
            context = mcp_tools.dispatch(store, channel, "task_get_context", {})
            if context["goal"] == "Explain this disposable project":
                from cortex_runtime.task_workspace import store_task_root
                task_root = Path(store_task_root(store, channel.task_id))
                assert task_root != project
                (task_root / "task-proof.txt").write_text("Task-owned file.\\n")
            if context["goal"] == "Wait for owner":
                mcp_tools.dispatch(store, channel, "ask_question", dict(
                    question="Choose a direction?", options=["A", "B"], request_key="ask"))
                return PromptOutcome(StopReason.END_TURN, "Waiting is not completion.")
            mcp_tools.dispatch(store, channel, "answer_submit",
                              dict(text="Fake-backed authoritative answer.", request_key="answer"))
            return PromptOutcome(StopReason.END_TURN, "Chat is not the result.")
        return behavior

    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    # This fixture deliberately has no production manager escape hatch.
    from dataclasses import dataclass
    from types import SimpleNamespace
    import threading
    import task_titles
    import cortex_runtime.connections.onboarding as onboarding
    import cortex_runtime.engine.session_manager as native_manager
    def forbidden_native_manager(*args, **kwargs):
        raise AssertionError("native engine construction is forbidden in this fixture")
    native_manager.EngineSessionManager = forbidden_native_manager
    from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind
    @dataclass
    class TitleRoute:
        id: str = "fake-title-route"
        model: str = "fake-title-model"
        supported_parameters: tuple = ()
    entry = SimpleNamespace(connection_id="fake-title-connection", route=TitleRoute())
    onboarding.resolve_connection_for_route = lambda connection_id, **kw: SimpleNamespace(engine="codex-acp")

    class FakeTitleEngine:
        def __init__(self, record, cwd, **kwargs):
            assert record.engine == "codex-acp"
            assert Path(cwd) != project and not list(Path(cwd).iterdir())
            self.closed = threading.Event()
        def negotiate(self, capabilities):
            assert capabilities == ()
        def create_session(self, task, route, **kwargs):
            assert route == "fake-title-route"
            assert "same language" in kwargs["session_new_meta"]["systemPrompt"]
            return "fake-title-session"
        def configure(self, session, revision):
            assert revision.route.model == "fake-title-model"
            assert dict(revision.route.supported_parameters)["mode"] == "read-only"
        def prompt_turn(self, session, prompt):
            with (project / "fake-title-calls").open("a") as calls:
                calls.write("title\n")
            if "Поздний" in prompt:
                (project / "title-waiting").touch()
                deadline = time.monotonic() + 10
                while not (project / "release-title").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
            yield EngineEvent(EngineEventKind.TURN_DELTA, delta_text="Исправить вход")
            yield EngineEvent(EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN)
        def shutdown(self, **kwargs):
            self.closed.set()

    class TestAgent(module.ServiceAgent):
        def _generate_title(self, description, options, expires):
            return task_titles.generate(self, self._service_template, description, options, expires,
                                        manager_factory=FakeTitleEngine)
        def _selectable_entries_and_connections(self, project_root=None):
            return [entry], {}
        def _default_selection_from_user_defaults(self):
            return None
        def _config_options_for_session(self, *args):
            return []
        def _post_session_new_notifications(self, session_id):
            pass
    module.run(runtime=runtime, agent_type=TestAgent)


def _safety_agent_child():
    def git(path, *args, check=True):
        result = subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                                 *args], cwd=path, capture_output=True, timeout=10)
        if check:
            assert result.returncode == 0, "disposable Git operation failed"
        return result

    sys.path[:0] = [str(ROOT / "src"), str(ROOT / "tests")]
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from cortex_runtime.orchestration import controller, artifacts, sandbox
    from cortex_runtime.delegation import revision
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource
    from sandbox_fakes import verification_command, project_git_command, sandboxed_git, unjailed_gateway_argv

    # Opt-in lifecycle launch fixtures, not a fallback after a sandbox refusal.
    controller._verification_command = verification_command
    artifacts._project_git_command = project_git_command
    revision.sandboxed_git = sandboxed_git
    sandbox.compact_argv = unjailed_gateway_argv
    import cortex_runtime.engine.session_manager as native_manager
    def forbidden(*args, **kwargs):
        raise AssertionError("native engines are forbidden")
    native_manager.EngineSessionManager = forbidden

    spec = importlib.util.spec_from_file_location("safety_agent", ROOT / "crates/workspace-service/agent_launcher.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    project = Path.cwd().resolve()
    assert project != ROOT and ROOT not in project.parents
    from cortex_runtime.project_storage import project_store_directory
    store = Store(project_store_directory(project), project_root=project)

    def behavior(store, channel, ref, message, index, on_delta=None, on_tool_call=None):
        from cortex_runtime.orchestration.state import TaskMachine
        context = (mcp_tools.dispatch(store, channel, "task_get_context", {})
                   if channel.role == "coordinator" else TaskMachine(store).context(channel.task_id))
        from cortex_runtime.task_workspace import store_task_root
        path = Path(store_task_root(store, channel.task_id))
        goal = context["goal"]
        if "Please clarify this completed result" in str(message):
            (project / ".cortex-dev/review-message-observed").touch()
        walkthrough = (project / ".git/owner-walkthrough-fixture").is_file()
        if walkthrough and channel.role == "coordinator" and goal.startswith("Plan this sprint."):
            detail = " Preserve existing work, explain failures, and retain evidence before continuing." * 45
            if not context["user_answers"]:
                mcp_tools.dispatch(store, channel, "user_question_request", dict(
                    questions=[dict(question=f"Planning decision {i}: choose the recovery approach." + detail,
                                    options=["Keep existing work", "Start a separate checkout"]) for i in range(1, 4)],
                    request_key="walkthrough-planning-questions"))
            else:
                tasks = [dict(key=f"step-{i}", title=f"Delivery stage {i}",
                              description=f"walkthrough delivery stage {i}." + detail,
                              dependsOn=[f"step-{i-1}"] if i > 1 else []) for i in range(1, 7)]
                mcp_tools.dispatch(store, channel, "answer_submit", dict(
                    text="Six dependent delivery stages.\n```json\n" + json.dumps(dict(tasks=tasks)) + "\n```",
                    request_key="walkthrough-plan"))
            return PromptOutcome(StopReason.END_TURN, "Planning decision recorded.")
        if channel.role == "coordinator" and goal.startswith("Plan this sprint."):
            if not context["user_answers"]:
                mcp_tools.dispatch(store, channel, "user_question_request", dict(
                    question="Plan two dependent tasks?", options=["Continue", "Wait"], request_key="planning-question"))
            else:
                mcp_tools.dispatch(store, channel, "answer_submit", dict(text='Plan ready.\n```json\n{"tasks":[{"key":"first","description":"question-first","dependsOn":[]},{"key":"second","description":"dependent-second","dependsOn":["first"]}]}\n```',
                                   request_key="planning-answer"))
            return PromptOutcome(StopReason.END_TURN, "Sprint planning turn.")
        if channel.role == "coordinator":
            if "Rerun the check" in str(message):
                (project / ".cortex-dev/rerun-observed").touch()
                mcp_tools.dispatch(store, channel, "answer_submit", dict(
                    text="Request observed, no check executed.", request_key="rerun-observed"))
                return PromptOutcome(StopReason.END_TURN, "Request observed.")
            if context["state"] == "PLANNING":
                if goal == "batched-questions" and not context["user_answers"]:
                    mcp_tools.dispatch(store, channel, "user_question_request", dict(
                        questions=[dict(question="Question "+str(i), options=["Choice "+str(n) for n in range(1,9)]) for i in range(1,9)],
                        request_key="batched-questions"))
                elif goal == "question-first" and not context["user_answers"]:
                    mcp_tools.dispatch(store, channel, "user_question_request", dict(
                        question="Continue the first task?", options=["Continue", "Wait"], request_key="question"))
                else:
                    mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(
                        tasks=([dict(id=f"work-{i}", role="backend-dev", objective=goal + f" Inspect stage {i}.",
                                     dependencies=[f"work-{i-1}"] if i else [], scope=["a.txt"], model="auto")
                                for i in range(8)] if walkthrough and goal.startswith("walkthrough delivery stage 1.") else
                               [dict(id="work", role="backend-dev", objective=goal,
                                     dependencies=[], scope=["a.txt"], model="auto")]),
                        verification=[["python3", "-B", "-c",
                                       "from pathlib import Path; assert Path('a.txt').read_text() in ('task\\n','resolved\\n')"]]),
                        request_key="plan-" + str(context["revision"])))
            elif context["state"] == "RECONCILING":
                mcp_tools.dispatch(store, channel, "synthesis_submit",
                                   dict(text="Reviewed disposable change.", not_verified=[
                                       dict(item="Required skipped check", blocker="The worker did not execute it.")
                                   ] if goal in ("skipped-check", "technical-check") else []))
            elif context["state"] == "COMPLETED":
                # Deliberately does not obey a prose-only rerun request: the control
                # must establish a real execution, not merely trust another answer.
                mcp_tools.dispatch(store, channel, "answer_submit",
                                   dict(text="Request observed, no check executed.", request_key="no-rerun"))
            return PromptOutcome(StopReason.END_TURN, "Coordinator turn.")

        with store.connection() as db:
            role = db.execute("SELECT role_id FROM delegation_provisioning WHERE attempt_id=?",
                              (channel.attempt_id,)).fetchone()[0]
        checks = []
        if walkthrough and on_tool_call and goal.startswith("walkthrough delivery stage 1."):
            for number in range(240):
                on_tool_call(f"inspection-{number}", "read",
                             f"Inspect recovery boundary {number}: " + "retain existing work and bounded evidence; " * 20,
                             "completed")
        if walkthrough and on_delta and goal.startswith("walkthrough delivery stage 1."):
            on_delta("Checked the recorded recovery contract. " * 250)
        if walkthrough and role != "worker-code-reviewer" and goal.startswith("walkthrough delivery stage 1."):
            deadline = time.monotonic() + 180
            while not (project / ".git/walkthrough-release").exists() and time.monotonic() < deadline:
                time.sleep(.05)
            assert (project / ".git/walkthrough-release").exists(), "walkthrough barrier not released"
        if role != "worker-code-reviewer" and goal == "sprint-delete-running":
            mcp_tools.dispatch(store, channel, "command_run", dict(
                argv=["python3", "-B", "-c", "import os,time; from pathlib import Path; Path('a.txt').write_text(str(os.getpid())); time.sleep(40)"],
                timeout_seconds=50, request_key="delete-running-command"))
            return PromptOutcome(StopReason.CANCELLED, "Stopped disposable command.")
        if role != "worker-code-reviewer":
            if (path / ".git").is_file() and git(path, "rev-parse", "--verify", "MERGE_HEAD", check=False).returncode == 0:
                (path / "a.txt").write_text("resolved\n")
                git(path, "add", "a.txt")
                git(path, "commit", "--no-edit")
            else:
                # Retain the real file receipt: shared-branch commits require task-owned evidence.
                mcp_tools.dispatch(store, channel, "project_read", dict(path="a.txt"))
                mcp_tools.dispatch(store, channel, "project_write", dict(path="a.txt", content="task\n",
                    expected_sha256=__import__("hashlib").sha256((path / "a.txt").read_bytes()).hexdigest(),
                    request_key="write-task-file"))
            if (project / ".git/human-walkthrough-fixture").is_file():
                if on_tool_call:
                    on_tool_call("human-read", "read", "Read a.txt", "completed",
                                 None, None, dict(file_path="a.txt"))
                    on_tool_call("human-edit", "edit", "Edit a.txt", "completed",
                                 None, None, dict(file_path="a.txt", old_string="Disposable notes.\n", new_string="task\n"))
                observed = mcp_tools.dispatch(store, channel, "command_run", dict(
                    argv=["python3", "-B", "-c", "print('Fixture command complete')"], request_key="human-command"))
                checks.append(dict(name="Fixture command", outcome="passed", command=observed["argv"], tool_call_ref=observed["tool_call_ref"]))
                if on_delta:
                    on_delta("Recorded the local change. Checking the result before continuing.")
                deadline = time.monotonic() + 300
                while (project / ".git/human-hold").exists():
                    assert time.monotonic() < deadline, "human fixture barrier deadline"
                    time.sleep(.05)
            if goal == "failed-check":
                observed = mcp_tools.dispatch(store, channel, "command_run", dict(
                    argv=["python3", "-B", "-c", "raise SystemExit(7)"], request_key="failed-check"))
                assert observed["exit_code"] == 7 and observed["complete"]
                checks = [dict(name="Required failed check", outcome="failed",
                               command=observed["argv"], tool_call_ref=observed["tool_call_ref"])]
            if goal in ("skipped-check", "technical-check"):
                checks = [dict(name="Required skipped check", outcome="not_run",
                               blocker="Visual and screen-reader judgement needs a person.")]
                if goal != "technical-check":
                    checks[0]["command"] = ["python3", "-B", "-c", "print('skipped-check-executed')"]
                    checks[0]["blocker"] = "Worker omitted a required executable check."
        draft = store.call("create_draft", dict(template="general", request_key="draft-" + channel.attempt_id),
                           channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
        mcp_tools.dispatch(store, channel, "draft_write", dict(
            draft_id=draft["draft_id"], content=draft["required_first_line"] + "\n\n# Result\n\nDisposable change inspected.\n",
            request_key="body-" + channel.attempt_id))
        report = store.call("write_report", dict(draft_id=draft["draft_id"], title="Disposable result",
                            summary="Inspected disposable change", author="worker", request_key="report-" + channel.attempt_id),
                            channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
        args = dict(report=report["report_id"], summary=("Disposable_change inspected." if (project / ".git/human-walkthrough-fixture").is_file() else "Disposable change inspected."), checks=checks, request_key="result")
        if role == "worker-code-reviewer":
            args["verdict"] = "APPROVE"
            if goal == "withdrawn-check":
                probe = mcp_tools.dispatch(store, channel, "command_run", dict(
                    argv=["python3", "-B", "-c", "raise SystemExit(1)"], request_key="review-probe"))
                args["checks"] = [dict(name="Withdrawn reviewer probe", outcome="failed",
                                       tool_call_ref=probe["tool_call_ref"],
                                       probe_withdrawn="Expected nonzero exploratory probe, not a project requirement.")]
        elif goal == "failed-check":
            args["assignment_outcome"] = "failed"
        mcp_tools.dispatch(store, channel, "result_submit", args)
        return PromptOutcome(StopReason.END_TURN, "Worker turn.")

    def router(*args):
        def guarded(*args, on_delta=None, on_tool_call=None, **kwargs):
            try:
                return behavior(*args, on_delta=on_delta, on_tool_call=on_tool_call, **kwargs)
            except Exception as error:
                import traceback
                with (project / ".cortex-dev/fake-errors").open("a") as out:
                    out.write(type(error).__name__ + ":" + str(error)[:120] + ":" +
                              str(traceback.extract_tb(error.__traceback__)[-1].lineno) + "\n")
                raise
        return guarded

    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    finish = (project / ".git/workspace-finish-fixture").is_file()
    if finish:
        from workspace_support.finish_fixture import catalogue
        catalogue()

    class TestAgent(module.ServiceAgent):
        def _maintenance_cleanup(self, state, api):
            if finish:
                from workspace_support.finish_fixture import cleanup
                return cleanup(state, api)
            return super()._maintenance_cleanup(state, api)
        def _maintenance_interval(self):
            return 0 if finish else super()._maintenance_interval()
        def _workspace_select(self, session, chosen):
            if finish:
                return super()._workspace_select(session, chosen)
            # This fixture has no native connection registry. Persisted task settings
            # still round-trip; fake-engine routing is supplied by FakeSessionRuntime.
            assert chosen["model"] == "fixture-route"
            session["pending_language"] = chosen["language"]
        def _generate_title(self, *args):
            return None
        def _generate_plan(self, description, options, expires, cancel):
            # Only the native model output is fake. Planning jobs, validation,
            # service scheduling, product writes and draft materialization are real.
            value = json.loads(description)
            if not any(e["kind"] == "answer" for e in value["conversation"]):
                return json.dumps(dict(question="Plan two dependent tasks?", options=["Continue", "Wait"]))
            return json.dumps(dict(tasks=[
                dict(key="first", description="question-first", dependsOn=[]),
                dict(key="second", description="dependent-second", dependsOn=["first"])]))
        def _default_selection_from_user_defaults(self):
            return None
        def _config_options_for_session(self, *args):
            return []
        def _post_session_new_notifications(self, *args):
            pass
    module.run(runtime=runtime, agent_type=TestAgent)



def _recv_exact(stream, n):
    value = bytearray()
    while len(value) < n:
        part = stream.recv(n - len(value))
        assert part, "service disconnected"
        value.extend(part)
    return value


class Client:
    def __init__(self, path):
        self.stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.stream.settimeout(15)
        self.stream.connect(str(path))
        pid, uid, gid = struct.unpack("3i", self.stream.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        assert uid == os.geteuid() and pid > 0
        self.number = 0

    def call(self, method, params):
        self.number += 1
        data = json.dumps(dict(jsonrpc="2.0", id=self.number, method=method, params=params)).encode()
        self.stream.sendall(struct.pack(">I", len(data)) + data)
        while True:
            n = struct.unpack(">I", _recv_exact(self.stream, 4))[0]
            assert 0 < n <= 1048576
            result = json.loads(_recv_exact(self.stream, n))
            if result.get("method"):
                continue
            assert result["id"] == self.number
            assert "error" not in result, result.get("error")
            return result["result"]

    def close(self):
        self.stream.close()


def _assert_no_tcp_sockets(pid):
    # Match only this owned process's socket descriptors, not unrelated host sockets.
    inodes = set()
    for fd in Path(f"/proc/{pid}/fd").iterdir():
        try:
            target = fd.readlink().as_posix()
        except FileNotFoundError:
            continue
        if target.startswith("socket:["):
            inodes.add(target[8:-1])
    for name in ("tcp", "tcp6"):
        rows = Path(f"/proc/{pid}/net/{name}").read_text().splitlines()[1:]
        assert all(row.split()[9] not in inodes for row in rows)


def _children(pid):
    found = set()
    for thread in Path(f"/proc/{pid}/task").iterdir():
        try:
            found.update(int(value) for value in (thread / "children").read_text().split())
        except FileNotFoundError:
            pass
    return found


def _alive_non_zombie(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def test_real_agent_fake_runtime_detach_result_and_restart_without_replay(tmp_path):
    binary = service_binary()
    projects = tmp_path / "projects"
    project = projects / "one"
    project.mkdir(parents=True)
    home = tmp_path / "home"
    home.mkdir()
    for name in ["config", "state", "cache", "runtime"]:
        (home / name).mkdir(mode=0o700)
    socket_home = tempfile.TemporaryDirectory(prefix="ws-")
    runtime = Path(socket_home.name) / "cortex-workspace-dev"
    state = tmp_path / "service-state"
    # Only test-owned XDG paths reach the child. Production accounts are not read.
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
               XDG_STATE_HOME=str(home / "state"), XDG_CACHE_HOME=str(home / "cache"),
               XDG_RUNTIME_DIR=socket_home.name, PYTHONDONTWRITEBYTECODE="1")
    argv = [str(binary), str(projects), sys.executable, str(ROOT), "--fixture-launcher", str(Path(__file__).resolve())]
    process = None
    client = None

    def launch():
        p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert p.poll() is None, "service exited during startup"
            if (runtime / "service.sock").exists():
                try:
                    return p, Client(runtime / "service.sock")
                except (ConnectionRefusedError, FileNotFoundError):
                    pass
            time.sleep(0.02)
        p.kill()
        p.wait()
        raise AssertionError("service did not bind")

    def query(operation, target=None, **extra):
        return dict(protocol=dict(major=1, minor=0), requestId="read", projectId=project_id,
                    operation=operation, target=target, **extra)

    def command(operation, key, target, version, payload):
        return dict(protocol=dict(major=1, minor=0), requestId=key, projectId=project_id,
                    actorId="client-cannot-select-this", operation=operation, target=target,
                    expectedVersion=version, ownerEpoch=None, payload=payload)

    try:
        process, client = launch()
        registration = client.call("projects.register", dict(root=str(project), displayName="Disposable"))
        project_id = registration["projectId"]
        assert registration["activation"] is None
        assert not (project / ".cortex").exists()
        assert client.call("service.status", {})["children"] == 0
        assert client.call("service.capabilities", {})["machine"] is True
        assert client.call("machine.call", dict(op="overview"))["result"]["retentionDays"] == 30
        folder_snapshot = client.call("machine.call", dict(op="repo.snapshot", projectId=project_id))
        assert folder_snapshot["ok"] is True
        folder = folder_snapshot["result"]
        assert folder["projectId"] == project_id and folder["hasGit"] is False
        assert folder["tree"] == [] and folder["branches"] == [] and folder["commits"] == []
        assert len(folder["checkouts"]) == 1
        assert folder["checkouts"][0]["kind"] == "main"
        assert folder["checkouts"][0]["path"] == str(project)
        assert not (project / ".git").exists()
        assert client.call("machine.call", dict(op="product.read", projectId=project_id))["result"]["tasks"] == {}
        for refused in (dict(op="shell"), dict(op="overview", root="/etc")):
            try:
                client.call("machine.call", refused)
            except AssertionError:
                continue
            raise AssertionError("the service answered a call it must refuse")
        diagnostics = home / "config/cortex-workspace-dev/cortex"
        diagnostics.parent.mkdir(mode=0o700, exist_ok=True)
        diagnostics.mkdir(mode=0o700)
        status_file = diagnostics / "first-run-codex-acp.json"
        status_file.write_text(json.dumps({"phase": "login_required", "private": "must-not-leak"}))
        status_file.chmod(0o600)
        engine_status = client.call("service.status", {})["engines"]
        assert engine_status == {"codex-acp": "login_required", "claude-agent-acp": "not_started"}
        status_file.write_text(json.dumps({"phase": "must-not-leak"}))
        assert client.call("service.status", {})["engines"]["codex-acp"] == "not_started"
        status_file.write_text(json.dumps({"phase": "ready"}))
        elsewhere = diagnostics.parent / "other"
        diagnostics.rename(elsewhere)
        diagnostics.symlink_to(elsewhere, target_is_directory=True)
        assert client.call("service.status", {})["engines"]["codex-acp"] == "not_started"
        diagnostics.unlink()
        elsewhere.rename(diagnostics)
        duplicate = subprocess.run(argv, env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        assert duplicate.returncode == 1
        assert client.call("service.status", {})["children"] == 0
        for args in (["init","-q","-b","main"],["-c","user.name=Owner","-c","user.email=owner@example.invalid","commit","-q","--allow-empty","-m","initial"]):
            subprocess.run(["git",*args],cwd=project,check=True,capture_output=True,env=env)
        client.call("projects.activate", dict(projectId=project_id))
        _assert_no_tcp_sockets(process.pid)
        children = _children(process.pid)
        assert len(children) == 1
        for child in children:
            _assert_no_tcp_sockets(child)
        assert (project / ".cortex-dev/cortex.sqlite3").is_file()
        # Save a sprint task through the real service before any execution.
        title_settings = dict(model="fake-title-route", effort="medium", mode="guarded", language="en")
        title_create = command("tasks.create", "title-draft", project_id, "0",
                               dict(description="Исправить вход\nСохранить текущую сессию", settings=title_settings))
        titled = client.call("workspace", title_create)["target"]
        sprint_reply = client.call("machine.call", dict(op="product.sprint", projectId=project_id,
                                  params=dict(action="create", description="Title sprint")))
        assert sprint_reply["ok"]
        title_sprint = sprint_reply["result"]["sprintId"]
        linked = client.call("machine.call", dict(op="product.task", projectId=project_id,
                                        params=dict(action="sprint", taskId=titled, sprintId=title_sprint, dependencies=[])))
        assert linked["ok"]
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            title_product = client.call("machine.call", dict(op="product.read", projectId=project_id))["result"]
            if title_product["tasks"].get(titled, {}).get("titleState") == "generated":
                break
            time.sleep(0.02)
        assert title_product["tasks"][titled]["title"] == "Исправить вход"
        assert title_product["tasks"][titled]["sprintId"] == title_sprint
        assert client.call("workspace", query("tasks.get", titled))["executionId"] is None
        assert not (project / "fake-runtime-calls").exists()
        assert (project / "fake-title-calls").read_text().splitlines() == ["title"]
        client.call("workspace", title_create)  # receipt replay never calls again

        late = client.call("workspace", command("tasks.create", "late-title", project_id, "0",
                           dict(description="Поздний ответ модели", settings=title_settings)))["target"]
        deadline = time.monotonic() + 8
        while not (project / "title-waiting").exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        assert (project / "title-waiting").exists()  # save returned while the fake is waiting
        assert client.call("workspace", query("tasks.get", late))["executionId"] is None
        client.call("machine.call", dict(op="product.task", projectId=project_id,
                                        params=dict(action="rename", taskId=late, title="Owner wins")))
        (project / "release-title").touch()
        time.sleep(0.15)
        title_product = client.call("machine.call", dict(op="product.read", projectId=project_id))["result"]
        assert title_product["tasks"][late]["title"] == "Owner wins"
        assert title_product["tasks"][late]["titleState"] == "user"
        assert (project / "fake-title-calls").read_text().splitlines() == ["title", "title"]
        assert not (project / "fake-runtime-calls").exists()

        draft = client.call("workspace", command("tasks.create", "draft", project_id, "0",
                                                dict(description="Explain this disposable project", checkout={"kind":"worktree"})))
        task = draft["target"]
        assert client.call("workspace", query("tasks.get", task))["executionState"] == "not_started"
        start_command = command("tasks.start", "explicit-start", task, "1", {})
        started = client.call("workspace", start_command)
        assert started["outcome"] == "recorded"
        client.close()  # No cancel, no child EOF.
        client = Client(runtime / "service.sock")
        assert client.call("service.status", {})["children"] == 1
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            execution = client.call("workspace", query("executions.get", started["executionId"]))
            if execution["state"] == "result":
                break
            time.sleep(0.03)
        assert execution["state"] == "result"
        result = client.call("workspace", query("results.get", execution["resultId"]))
        assert result["text"] == "Fake-backed authoritative answer."
        repo = client.call("machine.call",dict(op="repo.snapshot",projectId=project_id))["result"]
        checkout = next(c for c in repo["checkouts"] if c["taskId"] == task)
        assert checkout["kind"] == "worktree"
        assert (Path(checkout["path"]) / "task-proof.txt").is_file()
        assert not (project / "task-proof.txt").exists()
        assert not (Path(checkout["path"]) / ".cortex").exists()
        assert [f["path"] for f in repo["taskDiffs"][task]["files"]] == ["task-proof.txt"]
        # Current result evidence permits owner acceptance; integration is separate.
        assert result["acceptanceAvailable"] is True
        assert result["integrationAvailable"] is False
        assert client.call("workspace", start_command) == started
        waiting_task = client.call("workspace", command("tasks.create", "waiting-draft", project_id, "0",
                                                       dict(description="Wait for owner")))["target"]
        waiting_command = command("tasks.start", "waiting-start", waiting_task, "1", {})
        waiting = client.call("workspace", waiting_command)
        assert waiting["outcome"] == "recorded"
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            pending = client.call("workspace", query("executions.get", waiting["executionId"]))
            if pending["state"] == "waiting_user":
                break
            time.sleep(0.03)
        assert pending["state"] == "waiting_user"
        before_calls = (project / "fake-runtime-calls").read_text().splitlines()
        assert len(before_calls) == 2
        # A real service crash releases the flock and terminates the directly owned
        # child through Linux parent-death supervision. No prompt is replayed.
        client.close()
        client = None
        process.kill()
        process.wait(timeout=15)
        deadline = time.monotonic() + 10
        while any(_alive_non_zombie(pid) for pid in children) and time.monotonic() < deadline:
            time.sleep(0.02)
        assert all(not _alive_non_zombie(pid) for pid in children)
        process, client = launch()
        # Poll because the socket exists before the restored child's handshake completes.
        deadline = time.monotonic() + 15
        while client.call("projects.status", dict(projectId=project_id))["ready"] is not True and time.monotonic() < deadline:
            time.sleep(0.05)
        assert client.call("projects.status", dict(projectId=project_id))["recovery"] == "unknown"
        assert client.call("workspace", query("requests.get", "explicit-start")) == started
        assert client.call("workspace", start_command) == started
        assert len(client.call("workspace", query("tasks.list"))["items"]) == 4
        reloaded = client.call("machine.call", dict(op="product.read", projectId=project_id))["result"]
        assert reloaded["tasks"][late]["title"] == "Owner wins"
        assert reloaded["tasks"][late]["titleState"] == "user"
        assert reloaded["tasks"][titled]["title"] == "Исправить вход"
        assert (project / "fake-title-calls").read_text().splitlines() == ["title", "title"]
        assert client.call("workspace", query("executions.get", waiting["executionId"]))["state"] == "unknown"
        assert client.call("workspace", waiting_command) == waiting
        assert (project / "fake-runtime-calls").read_text().splitlines() == before_calls
        assert client.call("workspace", query("results.get", execution["resultId"]))["text"] == result["text"]
        children = _children(process.pid)
        assert len(children) == 1
        client.call("service.stop", {})
        client.close()
        client = None
        assert process.wait(timeout=15) == 0
        assert all(not _alive_non_zombie(pid) for pid in children)
    finally:
        if client is not None:
            client.close()
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=15)
        socket_home.cleanup()


if __name__ == "__main__":
    if (Path.cwd() / ".git" / "workspace-safety-fixture").is_file():
        try:
            _safety_agent_child()
        except Exception as error:
            import traceback
            location = traceback.extract_tb(error.__traceback__)[-1]
            (Path.cwd() / ".git/safety-child-error").write_text(
                type(error).__name__ + ":" + Path(location.filename).name + ":" + str(location.lineno))
            raise
    else:
        _agent_child()
