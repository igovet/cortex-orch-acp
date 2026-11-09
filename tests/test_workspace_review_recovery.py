"""Settled review recovery and planning contracts; no real engines or owner data."""
import json
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from test_workspace_check_execution import prepared
from test_workspace_safety_journeys import journey

def settled_agent(api):
    """Only the model continuation is replaced; durable capture/control stay real."""
    from cortex_runtime.acp.agent import Agent
    runtime = SimpleNamespace(start_prompt=lambda *a, **k: None, _store=api.store,
                              _task_workspace_root=lambda _: api.store.project_root)
    host = Agent(runtime, environ={})
    host._workspace_apis = {api.project_id: api}
    host._store_for = lambda _: api.store
    host._config_options_for_session = lambda *args: []
    host._make_emit_tool_call = lambda *args: None
    host._start_queued_message_continuation = lambda *args: None
    host.transport = SimpleNamespace(send_notification=lambda *args: True)
    return host


@pytest.mark.parametrize("missing", [False, True])
def test_completed_session_is_reopened_for_durable_messages(prepared, missing):
    from test_workspace_api import create_command
    from cortex_runtime.workspace_api import WorkspaceAPI
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from cortex_runtime.workspace_api.control import handle
    runtime, task_id, _, _ = prepared
    api = WorkspaceAPI(runtime._store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    public = api.handle(create_command())["target"]
    def finish(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('session-1',?,?,'{}','{}','[]','now',NULL)", (api.store.project_root, task_id))
        execution = bind_execution_tx(db, public, task_id, "session-1")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?", (task_id,))
        return execution
    execution = api.store.immediate_write(finish)
    host = settled_agent(api)
    if not missing:
        host.sessions["session-1"] = dict(session_id="session-1", project=api.store.project_root,
            workspace=api.store.project_root, task_id=task_id, owner_epoch=None, active=None,
            channel=None, lock=threading.Lock())
    reply = handle(host, dict(projectId=api.project_id, executionId=execution, action="prompt",
                              requestId="owner-message", text="Please clarify the result"))
    assert reply["outcome"] == "recorded"
    # The same delivery identity must not duplicate captured input.
    handle(host, dict(projectId=api.project_id, executionId=execution, action="prompt",
                     requestId="owner-message", text="Please clarify the result"))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM source_events WHERE event_id='owner-message'").fetchone()[0] == 1


def test_completed_review_message_through_real_service_and_runtime(journey):
    j = journey
    task = j.create("dependent-second")
    j.start(task)
    j.wait(lambda: j.ready(task), "task did not finish")
    original = j.result(task)
    previous = original["version"]
    assert j.prompt(task, "Please clarify this completed result")["outcome"] == "recorded"
    j.wait(lambda: (j.project / ".cortex-dev/review-message-observed").exists(), "captured message was not delivered to the runtime")
    result = j.wait(lambda: (r if r and r["version"] != previous else None)
                    if (r := j.result(task)) else None, "message did not produce a fresh recorded result")
    assert result["runtimeState"] == "COMPLETED"
    with __import__("sqlite3").connect(j.project / ".cortex-dev/cortex.sqlite3") as db:
        db.row_factory = __import__("sqlite3").Row
        row = db.execute("SELECT r.* FROM runtime_tasks r JOIN workspace_executions e ON e.runtime_task_id=r.task_id WHERE e.task_id=?", (task,)).fetchone()
        baseline = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKSPACE_REVIEW_MESSAGE' ORDER BY id DESC LIMIT 1", (row["task_id"],)).fetchone()
        assert baseline is not None, "Review baseline was not captured"
        retained = json.loads(baseline[0])
        # This fake coordinator creates a new plan; unchanged files still retain the implementation summary.
        assert retained["digest"] == result["observedWorkspaceDigest"], "Message changed workspace binding"
    assert result["text"] == original["text"]


@pytest.mark.parametrize("checkout", ["main", "worktree"])
def test_service_accepts_and_merges_ignored_helper_checkout(journey, checkout):
    from test_workspace_safety_journeys import git
    j = journey
    task = j.command("tasks.create", j.project_id, "0",
        dict(description="dependent-second", checkout=dict(kind=checkout)))["target"]
    before = git(j.project, "rev-parse", "HEAD").stdout
    j.start(task)
    j.wait(lambda: j.ready(task), "task did not reach Review")
    result = j.result(task)
    assert result["acceptanceAvailable"] is True
    j.accept(task, result)
    if checkout == "main":
        refused = j.call("machine.call", dict(op="product.integrate", projectId=j.project_id,
            params=dict(taskId=task, method="squash", targetBranch="main",
                        message="Reviewed task change", confirmed=True, digest=result["observedWorkspaceDigest"])))
        assert refused["code"] == "same_branch"
        assert git(j.project, "rev-parse", "HEAD").stdout == before
    j.machine("product.integrate", taskId=task, method="commit" if checkout == "main" else "squash", targetBranch="main",
              message="Reviewed task change", confirmed=True, digest=result["observedWorkspaceDigest"])
    assert git(j.project, "rev-parse", "HEAD").stdout != before
    assert git(j.project, "show", "HEAD:a.txt").stdout == b"task\n"
    assert j.product()["tasks"][task]["status"] == "done"
    assert not any(name.startswith((".cortex/", ".cortex-dev/"))
                   for name in git(j.project, "ls-files").stdout.decode().splitlines())
    assert not git(j.project, "diff", "--cached", "--name-only").stdout


@pytest.mark.parametrize("changed", [False, True])
@pytest.mark.parametrize("prepared", ["missing"], indirect=True)
def test_review_acknowledgement_keeps_implementation_and_same_technical_blocker(prepared, changed):
    from test_workspace_api import create_command, query
    from cortex_runtime.workspace_api import WorkspaceAPI
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from cortex_runtime.workspace_api.control import handle
    from cortex_runtime.orchestration import worker_checks
    runtime, task_id, target, _ = prepared
    api = WorkspaceAPI(runtime._store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    public = api.handle(create_command())["target"]
    worker_checks.execute(runtime, task_id, target, "record-technical-blocker")
    # Compatibility: an older runtime stored an unavailable-program technical gap.
    def legacy_gap(db):
        row = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type=\'WORKER_CHECK_FINISHED\' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
        payload = json.loads(row["payload_json"])
        payload["result"]["blocker"] = "verification_executable_unavailable"
        db.execute("UPDATE runtime_events SET payload_json=? WHERE id=?", (json.dumps(payload), row["id"]))
    api.store.immediate_write(legacy_gap)
    def finish(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('session-1',?,?,'{}','{}','[]','now',NULL)",
                   (api.store.project_root, task_id))
        execution = bind_execution_tx(db, public, task_id, "session-1")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Implemented subtraction with tests.',blocker=NULL WHERE task_id=?", (task_id,))
        return execution
    execution = api.store.immediate_write(finish)
    before = api.handle(query("results.get", "wr_" + execution))
    assert before["acceptanceAvailable"] is True
    host = settled_agent(api)
    assert handle(host, dict(projectId=api.project_id, executionId=execution, action="prompt",
        requestId="ack-message", text="Acknowledge only. Do not change files."))["outcome"] == "recorded"
    # Real session restoration advances the owner epoch, not the check/source identity.
    def acknowledge(db):
        db.execute("UPDATE task_owners SET epoch=epoch+1 WHERE task_id=?", (task_id,))
        db.execute("UPDATE workspace_executions SET owner_epoch=owner_epoch+1,version=version+1 WHERE execution_id=?", (execution,))
        db.execute("UPDATE runtime_tasks SET synthesis='Acknowledged. No changes.' WHERE task_id=?", (task_id,))
    api.store.immediate_write(acknowledge)
    if changed:
        (Path(api.store.project_root) / "feature.py").write_text("changed after message\n")
    after = api.handle(query("results.get", "wr_" + execution))
    if changed:
        assert after["acceptanceAvailable"] is False
        assert after["text"] == "Acknowledged. No changes."
    else:
        assert after["text"] == before["text"]
        assert after["checks"] == before["checks"]
        assert after["acceptanceAvailable"] is True


def test_live_session_cannot_adopt_database_owner(tmp_path):
    from test_workspace_api import api_for, create_command, runtime_binding
    from cortex_runtime.workspace_api.control import handle
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path); public = api.handle(create_command())["target"]
    execution = runtime_binding(api, public)
    host = settled_agent(api)
    host.sessions["session-1"] = dict(project=api.store.project_root, task_id="runtime-1",
        owner_epoch=None, active=None, lock=threading.Lock())
    with pytest.raises(WorkspaceError, match="execution_session_unavailable"):
        handle(host, dict(projectId=api.project_id, executionId=execution, action="prompt",
                          requestId="must-refuse", text="Not an owning session"))


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))


def test_planning_is_not_execution_and_invalid_output_is_actionable(tmp_path):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project", store=SimpleNamespace(project_root=str(tmp_path)))
    sprint = machine.sprint_action("project", state, dict(action="create", description="Implement arithmetic", runMode="automatic"))["sprintId"]
    output = json.dumps(dict(tasks=[dict(key="first", title="Arithmetic", description="Implement arithmetic", dependsOn=[])]))
    jobs = planning.PlanningJobs(state, lambda *args: output, threading.Event())
    jobs.tick(api)
    jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    assert saved["planning"] == "ready"
    assert saved["plan"]["items"][0]["description"].startswith("Implement arithmetic")
    assert "Original owner requirements" in saved["plan"]["items"][0]["description"]
    assert saved["taskIds"] == [] and not saved.get("planningTaskId")
    assert not (tmp_path / "arithmetic.py").exists()
    assert not machine.product_of(state, "project")["tasks"]
    machine.sprint_action("project", state, dict(action="patch", sprintId=sprint, patch=dict(planning="planning", plan=None)))
    jobs.generate = lambda *args: "I implemented it."
    jobs.tick(api); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    assert saved["planning"] == "cancelled" and saved["plan"] is None
    assert "required task-plan JSON" in saved["conversation"][-1]["text"]


def test_planning_keeps_explicit_owner_question_even_when_model_weakens_it(tmp_path):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project")
    original = "Implement subtraction. Ask the owner whether negative results are allowed before implementation."
    machine.sprint_action("project", state, dict(action="create", description=original, runMode="automatic"))
    # Exact observed failure shape: a valid plan silently substitutes model choice.
    output = json.dumps(dict(tasks=[dict(key="first", title="Subtraction",
        description="Decide whether negative results are allowed before implementation.", dependsOn=[])]))
    jobs = planning.PlanningJobs(state, lambda *args: output, threading.Event())
    jobs.tick(api); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    description = saved["plan"]["items"][0]["description"]
    assert original in description
    assert "ask the owner" in planning.INSTRUCTIONS.lower()
    assert "never decide" in description.lower()


@pytest.mark.parametrize("answered", [False, True])
def test_sprint_item_descriptions_keep_only_their_own_scope(tmp_path, answered):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project")
    subtraction = ("Implement subtract.py and test_subtract.py. " +
        ("Negative results are allowed." if answered else
         "Ask the owner whether negative results are allowed before implementation."))
    multiplication = "Implement multiply.py and test_multiply.py. Cover zero and negative operands."
    machine.sprint_action("project", state, dict(action="create",
        description=subtraction + "\n\n" + multiplication, runMode="automatic"))
    product = machine.product_of(state, "project")
    if answered:
        product["sprints"][0]["conversation"] = [
            dict(kind="question", text="Allow negative results?", at=1700000000000),
            dict(kind="answer", text="Negative results are allowed.", at=1700000000001),
            dict(kind="message", text="Keep the multiplication tests small.", at=1700000000002)]
        machine.write_json(machine.product_path(state, "project"), product)
    output = json.dumps(dict(tasks=[
        dict(key="subtract", title="Subtraction", description=subtraction, dependsOn=[]),
        dict(key="multiply", title="Multiplication", description=multiplication, dependsOn=[])]))
    jobs = planning.PlanningJobs(state, lambda *args: output, threading.Event())
    jobs.tick(api); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    first, second = saved["plan"]["items"]
    assert "multiply.py" not in first["description"]
    assert "subtract.py" not in second["description"]
    assert subtraction in first["description"] and multiplication in second["description"]
    assert ("Ask the owner" in first["description"]) is not answered
    assert "negative results are allowed" not in second["description"]
    for item in (first, second):
        assert "Owner follow-up" not in item["description"]
        assert '"kind"' not in item["description"] and '"at"' not in item["description"]
        assert "170000000000" not in item["description"]
    assert saved["taskIds"] == []


def test_single_item_owner_followup_is_plain_text(tmp_path):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project")
    machine.sprint_action("project", state, dict(action="create",
        description="Implement subtraction.", runMode="automatic"))
    product = machine.product_of(state, "project")
    product["sprints"][0]["conversation"] = [
        dict(kind="answer", text="Negative results are allowed.", at=1700000000001)]
    machine.write_json(machine.product_path(state, "project"), product)
    output = json.dumps(dict(tasks=[dict(key="subtract", description="Implement subtraction.", dependsOn=[])]))
    jobs = planning.PlanningJobs(state, lambda *args: output, threading.Event())
    jobs.tick(api); jobs.join(3)
    description = machine.product_of(state, "project")["sprints"][0]["plan"]["items"][0]["description"]
    assert "Negative results are allowed." in description
    assert '"kind"' not in description and '"at"' not in description
    assert "1700000000001" not in description


def test_planning_materializes_drafts_through_real_ui_port(tmp_path):
    from test_workspace_api import api_for
    from workspace_support.contract_socket import run_ui
    import planning_jobs, machine_tools
    api = api_for(tmp_path)
    state = tmp_path / "state"
    output = json.dumps(dict(tasks=[
        dict(key="first", title="Implement subtraction", description="Implement subtract.py. Negative results are not allowed.", dependsOn=[]),
        dict(key="test", title="Implement multiplication", description="Implement multiply.py with unit tests.", dependsOn=["first"])]))
    def generate(prompt, *_):
        value = json.loads(prompt)
        if not any(c.get("kind") == "answer" for c in value["conversation"]):
            return json.dumps(dict(question="Are negative subtraction results allowed?", options=["Yes", "No"]))
        return output
    jobs = planning_jobs.PlanningJobs(state, generate, threading.Event())
    def tick():
        jobs.tick(api); jobs.join(3)
        return {}
    run_ui(api, state, dict(projectId=api.project_id), "test/planning-contract.test.ts", extra=tick)
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_executions").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM runtime_tasks").fetchone()[0] == 0
    assert not (tmp_path / "arithmetic.py").exists()


@pytest.mark.parametrize("response", ["plan", "tool", "cancel", "error", "config", "oversize", "stopped", "empty", "deadline"])
def test_planner_manager_has_no_project_or_runtime_authority(tmp_path, monkeypatch, response):
    from dataclasses import dataclass
    import time
    import task_titles, planning_jobs
    import cortex_runtime.connections.onboarding as onboarding
    from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason
    @dataclass
    class Route:
        id: str = "selected"
        model: str = "fixture"
        supported_parameters: tuple = ()
    entry = SimpleNamespace(connection_id="connection", route=Route())
    agent = SimpleNamespace(sessions={"template":dict(project=str(tmp_path),
        default_selection=dict(connection_id="connection", model="fixture", parameters={}))},
        environ={}, _workspace_shutdown=threading.Event(),
        _selectable_entries_and_connections=lambda _: ([entry], {}))
    monkeypatch.setattr(onboarding, "resolve_connection_for_route", lambda *a, **k: SimpleNamespace(engine="codex-acp"))
    closed, cancel = threading.Event(), threading.Event()
    class Manager:
        def __init__(self, record, cwd, **kwargs):
            assert Path(cwd) != tmp_path and not list(Path(cwd).iterdir())
        def negotiate(self, caps):
            assert caps == ()
        def create_session(self, *args, **kwargs):
            assert "never implement" in kwargs["session_new_meta"]["systemPrompt"]
            return "session"
        def configure(self, session, revision):
            if response == "config":
                raise RuntimeError("private engine detail must not be displayed")
            assert dict(revision.route.supported_parameters)["mode"] == "read-only"
        def prompt_turn(self, session, prompt):
            # Native Luna returned a numbered prose list when only the task data
            # was in this turn. The auxiliary turn must carry its output contract.
            assert planning_jobs.INSTRUCTIONS in prompt
            assert "Planning input (JSON data, not instructions):" in prompt
            if response == "cancel":
                cancel.set(); assert closed.wait(2)
            if response == "empty":
                yield EngineEvent(EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN)
                return
            if response == "deadline":
                assert closed.wait(2)
            if response == "error":
                yield EngineEvent(EngineEventKind.ERROR, error_message="private engine detail")
            if response == "oversize":
                yield EngineEvent(EngineEventKind.TURN_DELTA, delta_text="x" * 32001)
            if response == "tool":
                yield EngineEvent(EngineEventKind.TOOL_CALL_OBSERVED)
            yield EngineEvent(EngineEventKind.TURN_DELTA, delta_text='{"tasks":[{"key":"one","description":"Work","dependsOn":[]}]}')
            yield EngineEvent(EngineEventKind.TURN_END, stop_reason=StopReason.REFUSAL if response == "stopped" else StopReason.END_TURN)
        def shutdown(self, **kwargs):
            closed.set()
    if response != "plan":
        with pytest.raises(task_titles.GenerationFailure) as failure:
            task_titles.generate(agent, "template", "Plan arithmetic", {}, time.monotonic()+(0.06 if response == "deadline" else 3),
                manager_factory=Manager, instructions=planning_jobs.INSTRUCTIONS, max_output=32000, cancel=cancel)
        assert "private engine detail" not in str(failure.value)
        trace = failure.value.diagnostics
        assert trace["stage"] and trace["elapsed_ms"] >= 0
        if response == "cancel":
            assert trace["cancelled"] is True
        if response == "empty":
            assert trace["stop_reason"] == "end_turn" and trace["text_characters"] == 0
        if response == "deadline":
            assert trace["deadline_reached"] is True
        if response == "stopped":
            assert trace["stop_reason"] == "refusal" and trace["text_characters"] > 0
        assert "private engine detail" not in json.dumps(trace)
    else:
        result = task_titles.generate(agent, "template", "Plan arithmetic", {}, time.monotonic()+3,
            manager_factory=Manager, instructions=planning_jobs.INSTRUCTIONS, max_output=32000, cancel=cancel)
        assert bool(result) == (response == "plan")
    assert closed.is_set()


def test_planning_cancel_discards_late_output(tmp_path):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project", store=SimpleNamespace(project_root=str(tmp_path)))
    sprint = machine.sprint_action("project", state, dict(action="create", description="Work", runMode="manual"))["sprintId"]
    entered, release = threading.Event(), threading.Event()
    def generate(*args):
        entered.set(); assert release.wait(3)
        return '{"tasks":[{"key":"one","description":"Work","dependsOn":[]}]}'
    jobs = planning.PlanningJobs(state, generate, threading.Event())
    jobs.tick(api); assert entered.wait(2)
    machine.sprint_action("project", state, dict(action="patch", sprintId=sprint, patch=dict(planning="cancelled")))
    jobs.tick(api)
    release.set(); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    assert saved["planning"] == "cancelled" and saved["plan"] is None


@pytest.mark.parametrize("output, reason", [
    ("1. Add subtract(a, b).\n2. Add its unittest.", "required task-plan JSON"),
    ('{"tasks":[{"key":"a","description":"Work","dependsOn":["missing"]}]}', "dependencies"),
    (None, "without a complete plan"),
])
def test_native_planning_failure_is_readable_without_leaking_output(tmp_path, output, reason):
    import planning_jobs as planning
    import machine_tools as machine
    state = tmp_path / "state"
    api = SimpleNamespace(project_id="project")
    machine.sprint_action("project", state, dict(action="create", description="Work", runMode="manual"))
    jobs = planning.PlanningJobs(state, lambda *args: output, threading.Event())
    jobs.tick(api); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    assert saved["planningDiagnostics"][-1]["attempt"]
    assert saved["planningDiagnostics"][-1]["failure_type"]
    assert saved["planning"] == "cancelled"
    assert reason in saved["conversation"][-1]["text"]
    assert saved.get("planningFailure") == saved["conversation"][-1]["text"]
    assert saved["plan"] is None and saved["taskIds"] == []
    machine.sprint_action("project", state, dict(action="patch", sprintId=saved["id"], patch=dict(planning="planning")))
    assert "planningFailure" not in machine.product_of(state, "project")["sprints"][0]
    jobs.generate = lambda *args: '{"tasks":[{"key":"one","description":"Work","dependsOn":[]}]}'
    jobs.tick(api); jobs.join(3)
    saved = machine.product_of(state, "project")["sprints"][0]
    assert saved["planning"] == "ready" and "planningFailure" not in saved


@pytest.mark.parametrize("prepared", ["missing"], indirect=True)
def test_rerun_explains_real_missing_tool(prepared):
    from cortex_runtime.orchestration import worker_checks
    runtime, task, target, _ = prepared
    reply = worker_checks.execute(runtime, task, target, "owner-rerun")
    assert reply["outcome"] == "recorded"
    with runtime._store.connection() as db:
        row, payload = worker_checks.latest_attempts(db, task, 1)[(target["assignment"], target["index"])]
        assert payload["result"]["checks"][0]["exit_code"] == 127
        assert payload["result"]["checks"][0]["output_tail"]
        assert worker_checks.unresolved(db, task, 1)[(target["assignment"], target["index"])]["outcome"] == "failed"


def test_no_command_is_an_explicit_manual_item_not_a_fake_execution(prepared):
    from cortex_runtime.orchestration import worker_checks
    runtime, task, target, _ = prepared
    def remove(db):
        row = db.execute("SELECT result_json FROM runtime_nodes WHERE assignment_ref=?", (target["assignment"],)).fetchone()
        result = json.loads(row[0])
        result["checks"][0].pop("command")
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?", (json.dumps(result), target["assignment"]))
    runtime._store.immediate_write(remove)
    worker_checks.automatic(runtime, task, threading.Event(), None)
    with runtime._store.connection() as db:
        reasons = worker_checks.unresolved(db, task, 1)
        mapped = worker_checks.targets(db, task, 1)[0]
    assert mapped["kind"] == "manual" and not mapped["argv"]
    assert (target["assignment"], target["index"]) not in reasons
