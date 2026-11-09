"""Acceptance journeys through the real socket service/helper/runtime, fake sessions only.

Launch preparation uses the established lifecycle test fakes. Commands, revision
capture, review, verification, result projection and acceptance are real. These
are not OS-containment or rendered-window checks.
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time

import pytest

ROOT = Path(__file__).resolve().parents[1]


def git(path, *args, check=True):
    result = subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                             *args], cwd=path, capture_output=True, timeout=10)
    if check:
        assert result.returncode == 0, "disposable Git operation failed"
    return result


class Journey:
    def __init__(self, project, endpoint, process):
        self.project, self.endpoint, self.process = project, endpoint, process
        self.project_id = None
        self.counter = 0

    def call(self, method, params):
        from test_workspace_service_integration import Client
        # Each RPC detaches immediately: no UI/window connection remains attached
        # while the service scheduler and runtime execute.
        client = Client(self.endpoint)
        try:
            return client.call(method, params)
        finally:
            client.close()

    def query(self, op, target=None):
        return self.call("workspace", dict(protocol=dict(major=1, minor=2), requestId="read",
                         projectId=self.project_id, operation=op, target=target))

    def command(self, op, target, version, payload, epoch=None):
        self.counter += 1
        return self.call("workspace", dict(protocol=dict(major=1, minor=2), requestId="c" + str(self.counter),
                         projectId=self.project_id, actorId="ignored", operation=op, target=target,
                         expectedVersion=version, ownerEpoch=epoch, payload=payload))

    def machine(self, op, **params):
        result = self.call("machine.call", dict(op=op, projectId=self.project_id, params=params))
        assert result["ok"], result.get("code")
        return result["result"]

    def product(self):
        return self.machine("product.read")

    def wait(self, predicate, description, seconds=18):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            assert self.process.poll() is None, "service exited"
            errors = self.project / ".cortex-dev/fake-errors"
            assert not errors.exists(), errors.read_text() if errors.exists() else ""
            value = predicate()
            if value:
                return value
            time.sleep(.05)
        pytest.fail(description)

    def create(self, description):
        return self.command("tasks.create", self.project_id, "0",
                            dict(description=description, checkout=dict(kind="worktree")))["target"]

    def start(self, task):
        result = self.command("tasks.start", task, self.query("tasks.get", task)["version"], {})
        assert result["outcome"] == "recorded"

    def result(self, task):
        row = self.query("tasks.get", task)
        if not row["executionId"]:
            return None
        execution = self.query("executions.get", row["executionId"])
        if execution["state"] != "result":
            return None
        return self.query("results.get", execution["resultId"])

    def checks(self, task):
        with sqlite3.connect(self.project / ".cortex-dev/cortex.sqlite3") as db:
            rows = db.execute("SELECT payload_json FROM runtime_events WHERE task_id="
                              "(SELECT runtime_task_id FROM workspace_executions WHERE task_id=?) "
                              "AND type IN ('CHECK_EXECUTION_FINISHED','TOOL_COMPLETED') ORDER BY id", (task,)).fetchall()
        receipts = []
        for row in rows:
            payload = json.loads(row[0])
            if payload.get("operation") == "command_run":
                observed = payload["result"]
                receipts.append(dict(observed, command_ref=observed["tool_call_ref"]))
            elif "command_ref" in payload:
                receipts.append(payload)
        return receipts

    def ready(self, task):
        return self.product()["tasks"].get(task, {}).get("status") == "ready_to_merge"

    def accept(self, task, result):
        return self.machine("product.task", action="accept", taskId=task,
                            digest=result["observedWorkspaceDigest"], version=result["version"])

    def prompt(self, task, text):
        execution = self.query("tasks.get", task)["executionId"]
        self.counter += 1
        return self.call("executions.control", dict(projectId=self.project_id, executionId=execution,
                         action="prompt", text=text, requestId="prompt" + str(self.counter)))


@pytest.fixture
def journey(tmp_path):
    from test_workspace_service_integration import service_binary
    binary = service_binary()
    projects = tmp_path / "projects"
    project = projects / "fixture"
    project.mkdir(parents=True)
    (project / "a.txt").write_text("base\n")
    git(project, "init", "-q", "-b", "main")
    git(project, "config", "user.name", "Fixture")
    git(project, "config", "user.email", "fixture@example.invalid")
    git(project, "add", "a.txt")
    git(project, "commit", "-qm", "base")
    (project / ".git/workspace-safety-fixture").touch()
    home = tmp_path / "home"
    home.mkdir()
    for name in ("config", "state", "cache", "data"):
        (home / name).mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix="cw-safe-") as scratch:
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
        env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
                   XDG_CACHE_HOME=str(home / "cache"), XDG_DATA_HOME=str(home / "data"),
                   XDG_RUNTIME_DIR=scratch, PYTHONDONTWRITEBYTECODE="1")
        process = subprocess.Popen([str(binary), str(projects), sys.executable, str(ROOT),
                                    "--fixture-launcher", str(ROOT / "tests/test_workspace_service_integration.py")],
                                   env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        j = Journey(project, Path(scratch) / "cortex-workspace-dev/service.sock", process)
        try:
            j.wait(lambda: j.endpoint.exists(), "service did not bind")
            j.project_id = j.call("projects.register", dict(root=str(project), displayName="Fixture"))["projectId"]
            try:
                j.call("projects.activate", dict(projectId=j.project_id))
            except AssertionError:
                error = project / ".git/safety-child-error"
                if error.exists():
                    pytest.fail("fixture child: " + error.read_text())
                raise
            yield j
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def test_windowless_automatic_sprint_continues_after_question(journey):
    j = journey
    first, second = j.create("question-first"), j.create("dependent-second")
    sprint = j.machine("product.sprint", action="create", description="Automatic fixture",
                       runMode="automatic")["sprintId"]
    for task, deps in ((first, []), (second, [first])):
        j.machine("product.task", action="sprint", taskId=task, sprintId=sprint, dependencies=deps)
    j.machine("product.sprint", action="patch", sprintId=sprint,
              patch=dict(planning="created", taskIds=[first, second], queueState="running"))
    def paused_question():
        task = j.query("tasks.get", first)
        return task if task["phase"] == "waiting" and j.product()["sprints"][0]["queueState"] == "paused" else None
    row = j.wait(paused_question, "automatic sprint did not pause on the question")
    assert j.query("tasks.get", second)["executionId"] is None
    decision = j.query("decisions.list", row["executionId"])["items"][0]
    result = j.command("decisions.answer", decision["decisionId"], decision["version"],
                       dict(executionId=decision["executionId"], acpSessionId=decision["acpSessionId"],
                            batchId=decision["batchId"], answers=[dict(decisionId=decision["decisionId"],
                            questionId=decision["questionId"], expectedVersion=decision["version"],
                            optionId=None, text="Continue")]), epoch=decision["ownerEpoch"])
    assert result["outcome"] == "recorded"
    j.wait(lambda: j.ready(first), "answered first task did not reach Ready to merge")
    j.wait(lambda: j.ready(second), "answered question did not resume dependent task")
    assert j.product()["sprints"][0]["queueState"] == "complete"
    assert j.checks(first) and j.checks(second)


def test_real_conflict_to_fresh_acceptance(journey):
    j = journey
    task = j.create("conflict")
    j.start(task)
    initial = j.wait(lambda: j.result(task), "initial work did not complete")
    j.wait(lambda: j.ready(task), "initial work did not reach Ready to merge")
    assert initial["kind"] == "work" and initial["verificationStatus"] == "passed"
    j.accept(task, initial)
    old_checks = j.checks(task)
    (j.project / "a.txt").write_text("owner\n")
    git(j.project, "add", "a.txt")
    git(j.project, "commit", "-qm", "owner")
    reply = j.call("machine.call", dict(op="product.integrate", projectId=j.project_id,
                   params=dict(taskId=task, method="squash", targetBranch="main", message="Task",
                               confirmed=True, digest=initial["observedWorkspaceDigest"])))
    assert not reply["ok"] and reply["code"] == "merge_conflict"
    record = j.product()["tasks"][task]
    path = Path(record["checkout"]["path"])
    assert git(path, "diff", "--name-only", "--diff-filter=U").stdout.strip() == b"a.txt"
    assert (j.project / "a.txt").read_text() == "owner\n"
    refused = j.call("machine.call", dict(op="product.task", projectId=j.project_id,
                     params=dict(action="accept", taskId=task, digest=initial["observedWorkspaceDigest"],
                                 version=initial["version"])))
    assert refused["ok"] is False
    assert j.product()["tasks"][task]["acceptance"] is None
    # The same two service calls as resolve_with_cortex in the real port.
    j.prompt(task, "Resolve the retained Git merge conflict in this task's own checkout. "
             "Preserve both intended changes, run the required checks, and return a fresh result for my review. "
             "Do not merge or push the target branch. Owner guidance: resolve to resolved.")
    j.machine("product.task", action="resolution_started", taskId=task, guidance="resolve to resolved")
    fresh = j.wait(lambda: (r if (r := j.result(task)) and r["version"] != initial["version"] and
                            r["verificationStatus"] == "passed" and r["workspaceBindingValid"] else None),
                   "conflict resolution did not produce fresh verified work")
    j.wait(lambda: j.ready(task) and j.product()["tasks"][task]["conflict"] == "none",
           "resolved task did not return to Ready to merge")
    assert not git(path, "diff", "--name-only", "--diff-filter=U").stdout.strip()
    assert git(path, "rev-parse", "--verify", "MERGE_HEAD", check=False).returncode != 0
    assert (path / "a.txt").read_text() == "resolved\n"
    assert (j.project / "a.txt").read_text() == "owner\n"
    new_checks = j.checks(task)
    assert set(c["command_ref"] for c in new_checks) > set(c["command_ref"] for c in old_checks)
    j.accept(task, fresh)
    assert j.product()["tasks"][task]["acceptance"]["digest"] == fresh["observedWorkspaceDigest"]


def test_windowless_two_task_dependency_positive_control(journey):
    j = journey
    first, second = j.create("first"), j.create("second")
    sprint = j.machine("product.sprint", action="create", description="Dependency control",
                       runMode="automatic")["sprintId"]
    for task, deps in ((first, []), (second, [first])):
        j.machine("product.task", action="sprint", taskId=task, sprintId=sprint, dependencies=deps)
    j.machine("product.sprint", action="patch", sprintId=sprint,
              patch=dict(planning="created", taskIds=[first, second], queueState="running"))
    j.wait(lambda: j.ready(first), "first task did not become ready")
    j.wait(lambda: j.ready(second), "dependent task did not become ready")
    j.wait(lambda: j.product()["sprints"][0]["queueState"] == "complete", "queue did not complete")
    assert j.checks(first) and j.checks(second)
    assert all(c["complete"] and c["exit_code"] == 0 for c in j.checks(first) + j.checks(second))
    assert max(c["started_event_id"] for c in j.checks(first)) < min(
        c["started_event_id"] for c in j.checks(second))


def test_failed_check_pauses_queue_and_owner_force_gets_fresh_receipt(journey):
    j = journey
    first, second = j.create("failed-check"), j.create("dependent")
    sprint = j.machine("product.sprint", action="create", description="Failure control",
                       runMode="automatic")["sprintId"]
    for task, deps in ((first, []), (second, [first])):
        j.machine("product.task", action="sprint", taskId=task, sprintId=sprint, dependencies=deps)
    j.machine("product.sprint", action="patch", sprintId=sprint,
              patch=dict(planning="created", taskIds=[first, second], queueState="running"))
    j.wait(lambda: j.checks(first) and j.product()["sprints"][0]["queueState"] == "paused",
           "failed check did not pause the automatic queue")
    assert j.query("tasks.get", second)["executionId"] is None
    before = j.checks(first)
    assert before[-1]["exit_code"] == 7 and before[-1]["complete"]
    request = j.prompt(first, 'Rerun the check "Required failed check" for this result. Do not skip it. '
             'Use the retained exact command and working directory, and report the new evidence. '
             'If it cannot run, report the concrete technical blocker; never mark it passed without evidence.')
    assert request["outcome"] == "recorded"
    refs = {c["command_ref"] for c in before}
    j.wait(lambda: any(c["command_ref"] not in refs and c["argv"] == before[-1]["argv"]
                      for c in j.checks(first)), "owner force received no fresh execution receipt", seconds=8)


def test_skipped_check_is_executed_automatically(journey):
    j = journey
    task = j.create("skipped-check")
    j.start(task)
    result = j.wait(lambda: j.result(task), "skipped-check fixture did not complete")
    assert any(c["name"] == "Required skipped check" for c in result["checks"])
    j.wait(lambda: any(c["argv"][-1] == "print('skipped-check-executed')" for c in j.checks(task)),
           "required skipped check never received an automatic execution receipt")

