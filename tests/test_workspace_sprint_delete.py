"""Destructive sprint deletion: disposable stores and fake engine sessions only."""
import pytest

from test_workspace_api import api_for, create_command, query, runtime_binding
from test_workspace_machine_tools import machine, ok
from test_workspace_safety_journeys import journey


def setup(machine):
    api = api_for(machine.repo)
    tasks = [api.handle(create_command("new-" + str(i)))["target"] for i in range(3)]
    call = lambda **params: machine("product.sprint", params, project="project-1")
    sprint = ok(call(action="create", description="Delete only this sprint"))["sprintId"]
    for task in tasks[:2]:
        ok(machine("product.task", dict(action="sprint", taskId=task, sprintId=sprint), project="project-1"))
    return api, tasks, sprint, call


@pytest.mark.parametrize("archived", [False, True])
def test_delete_is_confirmed_durable_idempotent_and_scoped(machine, archived):
    api, tasks, sprint, call = setup(machine)
    if archived:
        ok(call(action="close", sprintId=sprint, confirmed=True))
    assert not call(action="delete_prepare", sprintId=sprint, confirmed=False)["ok"]
    assert len(api.handle(query("tasks.list"))["items"]) == 3
    before = (machine.repo / "a.txt").read_bytes()
    ok(call(action="delete_prepare", sprintId=sprint, confirmed=True))
    result = ok(call(action="delete_finish", sprintId=sprint, confirmed=True))
    assert result["done"] and set(result["taskIds"]) == set(tasks[:2])
    assert [t["taskId"] for t in api.handle(query("tasks.list"))["items"]] == tasks[2:]
    assert ok(machine("product.read", project="project-1"))["sprints"] == []
    assert ok(call(action="delete_prepare", sprintId=sprint, confirmed=True)) == result
    assert (machine.repo / "a.txt").read_bytes() == before


def test_live_or_unknown_execution_is_not_purged_and_start_is_fenced(machine):
    api, tasks, sprint, call = setup(machine)
    runtime_binding(api, tasks[0])
    prepared = ok(call(action="delete_prepare", sprintId=sprint, confirmed=True))
    assert prepared["executionIds"]
    api.dispatch_start = lambda start: pytest.fail("deleting task was dispatched")
    command = dict(create_command("start-deleting"), operation="tasks.start", target=tasks[1], expectedVersion="1", payload={})
    assert api.handle(command)["outcome"] == "busy"
    partial = ok(call(action="delete_finish", sprintId=sprint, confirmed=True))
    assert not partial["done"] and "active_execution" in partial["reason"]
    assert api.handle(query("tasks.get", tasks[0]))["taskId"] == tasks[0]
    assert ok(machine("product.read", project="project-1"))["sprints"][0]["queueState"] == "paused"


@pytest.mark.parametrize("dirty", [False, True])
def test_only_clean_app_worktree_is_removed_and_branches_are_preserved(machine, tmp_path, dirty):
    from cortex_runtime.workspace_api import task_options
    api, tasks, sprint, call = setup(machine)
    api.store.immediate_write(lambda db: task_options.save(db, tasks[0], {"checkout": {"kind": "worktree"}}))
    checkout = task_options.prepare(api.store, tasks[0], {"HOME": str(tmp_path), "CORTEX_WORKSPACE_STATE_DIR": str(machine.repo.parent / "state")})
    from pathlib import Path
    if dirty:
        (Path(checkout) / "owner.txt").write_text("owner work")
    with api.store.connection() as db:
        branch = task_options.read(db, tasks[0])["checkout"]["branch"]
    ok(call(action="delete_prepare", sprintId=sprint, confirmed=True))
    result = ok(call(action="delete_finish", sprintId=sprint, confirmed=True))
    assert result["done"]
    assert Path(checkout).exists() is dirty
    assert (checkout in result["keptPaths"]) is dirty
    machine.git("show-ref", "--verify", "refs/heads/" + branch)


def test_delete_waits_for_planner_cancellation_and_fences_late_output(machine, monkeypatch):
    import threading
    from pathlib import Path
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[1] / "crates/workspace-service"))
    from planning_jobs import PlanningJobs
    api, tasks, sprint, call = setup(machine)
    entered = threading.Event()
    ended = threading.Event()
    def generate(prompt, options, expires, cancel):
        entered.set()
        assert cancel.wait(5), "planner cancellation was not delivered"
        ended.set()
        return '{"tasks":[{"key":"late","description":"Late output","dependsOn":[]}]}'
    jobs = PlanningJobs(machine.repo.parent / "state", generate, threading.Event())
    jobs.tick(api)
    assert entered.wait(2)
    ok(call(action="delete_prepare", sprintId=sprint, confirmed=True))
    partial = ok(call(action="delete_finish", sprintId=sprint, confirmed=True))
    assert not partial["done"] and "planner" in partial["reason"]
    jobs.tick(api)
    jobs.join(2)
    assert ended.is_set()
    jobs.tick(api)
    assert ok(call(action="delete_finish", sprintId=sprint, confirmed=True))["done"]
    assert ok(machine("product.read", project="project-1"))["sprints"] == []


def test_real_service_reaps_a_running_command_before_deleting_task(journey):
    import os
    from pathlib import Path
    j = journey
    task = j.create("sprint-delete-running")
    sprint = j.machine("product.sprint", action="create", description="Running command")["sprintId"]
    j.machine("product.task", action="sprint", taskId=task, sprintId=sprint)
    j.start(task)
    def process_id():
        checkout = j.product()["tasks"].get(task, {}).get("checkout", {}).get("path")
        text = (Path(checkout) / "a.txt").read_text() if checkout and Path(checkout).exists() else ""
        return int(text) if text.isdigit() else None
    pid = j.wait(process_id, "command did not start")
    os.kill(pid, 0)
    result = j.machine("product.sprint", action="delete", sprintId=sprint, confirmed=True)
    assert result["done"], result
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    assert j.query("tasks.list")["items"] == []


def test_real_service_stops_execution_and_deletes_queue_without_touching_other_tasks(journey):
    j = journey
    first, pending, other = j.create("question-first"), j.create("queued-second"), j.create("other sprint")
    sprint = j.machine("product.sprint", action="create", description="Delete running queue", runMode="automatic")["sprintId"]
    for task in (first, pending):
        j.machine("product.task", action="sprint", taskId=task, sprintId=sprint, dependencies=[])
    j.start(first)
    j.wait(lambda: j.query("tasks.get", first)["phase"] == "waiting", "task did not ask")
    result = j.machine("product.sprint", action="delete", sprintId=sprint, confirmed=True)
    assert result["done"], result
    assert set(result["taskIds"]) == {first, pending}
    assert [t["taskId"] for t in j.query("tasks.list")["items"]] == [other]
    assert j.product()["sprints"] == []
    assert j.project.is_dir()
    assert j.machine("product.sprint", action="delete", sprintId=sprint, confirmed=True) == result
