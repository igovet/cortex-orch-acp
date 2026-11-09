"""Service-owned queue policy, without a window or paid engine."""
import importlib.util
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))
import machine_tools
import scheduler


class API:
    project_id = "project"
    actor_id = "owner"

    def __init__(self, tasks, checks):
        self.tasks, self.checks, self.starts = tasks, checks, []

    def handle(self, request):
        op = request["operation"]
        if op == "project.snapshot":
            return {"tasks": {"items": self.tasks, "nextCursor": None}}
        if op == "executions.get":
            return {"resultId": "result"}
        if op == "results.get":
            return {"taskId": "first", "executionId": "execution", "checks": self.checks, "text": "Done.", "version":"1", "verificationStatus":"passed", "workspaceBindingValid":True}
        if op == "tasks.start":
            self.starts.append(request["target"])
            return {"outcome": "recorded"}
        raise AssertionError(op)


def setup(tmp_path):
    product = machine_tools.product_of(tmp_path, "project")
    product["sprints"] = [{"id": "s", "runMode":"automatic", "planning": "created", "queueState": "running", "taskIds": ["first", "next"]}]
    product["tasks"]["next"] = {"dependencies": ["first"]}
    machine_tools.write_json(machine_tools.product_path(tmp_path, "project"), product)
    tasks = [{"taskId": "first", "phase": "result", "executionId": "execution", "version": "3"},
             {"taskId": "next", "phase": "not_started", "executionId": None, "version": "1"}]
    return tasks


def test_service_advances_after_verified_result_without_screen(tmp_path):
    api = API(setup(tmp_path), [{"outcome": "passed"}])
    scheduler.tick(api, tmp_path)
    assert api.starts == ["next"]
    assert machine_tools.product_of(tmp_path, "project")["tasks"]["first"]["status"] == "ready_to_merge"


def test_finished_work_is_not_reopened_when_cleanup_removes_its_checkout(tmp_path):
    tasks = setup(tmp_path)
    product = machine_tools.product_of(tmp_path, "project")
    product["tasks"]["first"] = {"status": "done", "automaticReady": True, "readyVersion": "1",
                                  "integration": {"method": "squash", "worktreeCleanup": "removed"}}
    machine_tools.write_json(machine_tools.product_path(tmp_path, "project"), product)
    # An absent worktree makes old result evidence unobservable, not unfinished.
    scheduler.tick(API(tasks, []), tmp_path)
    record = machine_tools.product_of(tmp_path, "project")["tasks"]["first"]
    assert record["status"] == "done"
    assert record["integration"]["worktreeCleanup"] == "removed"


def test_old_result_cannot_certify_manually_finished_conflict(tmp_path):
    api = API(setup(tmp_path), [{"outcome":"passed"}])
    product = machine_tools.product_of(tmp_path, "project")
    product["tasks"]["first"] = {"requiresFreshResult": {"resultId":None,"version":"1"}}
    machine_tools.write_json(machine_tools.product_path(tmp_path, "project"), product)
    scheduler.tick(api, tmp_path)
    assert not api.starts
    assert machine_tools.product_of(tmp_path, "project")["tasks"]["first"].get("status") != "ready_to_merge"


def test_question_pause_resumes_but_owner_pause_and_failed_check_do_not(tmp_path):
    tasks = setup(tmp_path)
    tasks[0]["phase"] = "waiting"
    scheduler.tick(API(tasks, []), tmp_path)
    paused = machine_tools.product_of(tmp_path, "project")["sprints"][0]
    assert paused["pauseKind"] == "question"
    tasks[0]["phase"] = "recovery"
    scheduler.tick(API(tasks, []), tmp_path)
    assert machine_tools.product_of(tmp_path, "project")["sprints"][0]["pauseKind"] == "question"
    tasks[0]["phase"] = "result"
    api = API(tasks, [{"outcome": "passed"}])
    scheduler.tick(api, tmp_path)
    assert api.starts == ["next"]

    setup(tmp_path)
    api = API(tasks, [{"outcome": "failed"}])
    scheduler.tick(api, tmp_path)
    api.checks = [{"outcome": "passed"}]
    scheduler.tick(api, tmp_path)
    assert not api.starts

    setup(tmp_path)
    tasks[0]["phase"] = "waiting"
    scheduler.tick(API(tasks, []), tmp_path)
    machine_tools.sprint_action("project", tmp_path, dict(action="patch", sprintId="s",
                                                        patch=dict(queueState="paused", pauseReason="Paused by you")))
    tasks[0]["phase"] = "result"
    api = API(tasks, [{"outcome": "passed"}])
    scheduler.tick(api, tmp_path)
    assert not api.starts


def test_service_pauses_for_missing_checks_and_questions(tmp_path):
    tasks = setup(tmp_path)
    api = API(tasks, [])
    scheduler.tick(api, tmp_path)
    assert not api.starts
    assert machine_tools.product_of(tmp_path, "project")["sprints"][0]["queueState"] == "paused"
    setup(tmp_path)
    tasks[0]["phase"] = "waiting"
    scheduler.tick(API(tasks, []), tmp_path)
    assert machine_tools.product_of(tmp_path, "project")["sprints"][0]["queueState"] == "paused"
