"""Saved-task editing and sprint closure; disposable stores, no paid engines."""
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import subprocess

import pytest

from cortex_runtime.workspace_api import WorkspaceError, task_options
from test_workspace_api import api_for, create_command, query
from test_workspace_machine_tools import machine, ok
from test_workspace_safety_journeys import journey

FILE = dict(name="scope.txt", mimeType="text/plain", data="aGk=")
CHOICES = dict(title="Owner title", description="Revised prompt", attachments=[FILE],
               settings=dict(model="fixture-route", effort="", mode="auto", language="en"),
               checkout=dict(kind="main"))


def test_edit_is_atomic_versioned_and_replaces_saved_attachments(tmp_path):
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    before = api.handle(query("project.snapshot"))
    assert api.edit_draft(task, expected_version="1", payload=CHOICES) == "2"
    saved = api.handle(query("tasks.get", task))
    assert saved["description"] == CHOICES["description"] and saved["executionId"] is None
    assert saved["version"] == "2"
    assert api.handle(query("project.snapshot"))["version"] != before["version"]
    with api.store.connection() as db:
        options = task_options.read(db, task)
        assert options["attachments"] == [FILE] and options["draftTitle"] == "Owner title"
        assert db.execute("SELECT count(*) FROM workspace_starts").fetchone()[0] == 0
    with pytest.raises(WorkspaceError, match="stale_version"):
        api.edit_draft(task, expected_version="1", payload=dict(CHOICES, description="Lost overwrite"))
    invalid = dict(CHOICES, attachments=[dict(FILE, data="bad")])
    with pytest.raises(ValueError):
        api.edit_draft(task, expected_version="2", payload=invalid)
    assert api.handle(query("tasks.get", task))["description"] == CHOICES["description"]
    api.edit_draft(task, expected_version="2", payload=dict(CHOICES, attachments=[]))
    with api.store.connection() as db:
        assert task_options.read(db, task)["attachments"] == []
    starts = []
    api.dispatch_start = starts.append
    command = dict(create_command("start-edited"), operation="tasks.start", target=task, expectedVersion="3", payload={})
    assert api.handle(command)["outcome"] == "recorded"
    assert starts[0]["description"] == CHOICES["description"]
    with api.store.connection() as db:
        assert task_options.read(db, task)["settings"] == CHOICES["settings"]


def test_competing_edit_and_start_never_dispatch_a_mixed_or_overwritten_prompt(tmp_path):
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    starts = []
    api.dispatch_start = lambda start: starts.append(start)
    command = dict(create_command("start"), operation="tasks.start", target=task, expectedVersion="1", payload={})
    def edit():
        try:
            return api.edit_draft(task, expected_version="1", payload=CHOICES)
        except WorkspaceError as error:
            return str(error)
    with ThreadPoolExecutor(max_workers=2) as pool:
        edit_result, start_result = list(pool.map(lambda f: f(), [edit, lambda: api.handle(command)]))
    if edit_result == "2":
        assert start_result["outcome"] == "stale_version" and starts == []
    else:
        assert edit_result == "task_started" and start_result["outcome"] == "recorded"
        assert starts[0]["description"] == "Build a thing"
    # Even a failed/unconfirmed start owns an execution identity; it is never editable again.
    if starts:
        with pytest.raises(WorkspaceError, match="task_started"):
            api.edit_draft(task, expected_version=api.handle(query("tasks.get", task))["version"], payload=CHOICES)


def test_helper_edit_survives_process_restart_and_does_not_lose_title(machine):
    api = api_for(machine.repo)
    task = api.handle(create_command())["target"]
    call = lambda op, **params: machine(op, params, project="project-1")
    ok(call("product.task", action="edit", taskId=task, expectedVersion="1", payload=CHOICES))
    product = ok(call("product.read"))
    assert product["tasks"][task]["title"] == CHOICES["title"]
    assert product["tasks"][task]["settings"] == CHOICES["settings"]
    assert ok(call("product.read", taskId=task, attachments=True)) == [FILE]
    assert call("product.task", action="edit", taskId=task, expectedVersion="1", payload=CHOICES)["ok"] is False


def test_close_sprint_keeps_tasks_and_files_and_refuses_reopening_or_joining(machine):
    api = api_for(machine.repo)
    tasks = [api.handle(create_command("new-" + str(i)))["target"] for i in range(2)]
    call = lambda op, **params: machine(op, params, project="project-1")
    sprint = ok(call("product.sprint", action="create", description="Owner sprint", runMode="automatic"))["sprintId"]
    for task in tasks:
        ok(call("product.task", action="sprint", taskId=task, sprintId=sprint, dependencies=[]))
    ok(call("product.sprint", action="patch", sprintId=sprint, patch=dict(planning="created", taskIds=tasks, queueState="running")))
    assert not call("product.sprint", action="close", sprintId=sprint, confirmed=False)["ok"]
    before = (machine.repo / "a.txt").read_bytes()
    ok(call("product.sprint", action="close", sprintId=sprint, confirmed=True))
    saved = ok(call("product.read"))["sprints"][0]
    assert saved["archivedAt"] and saved["queueState"] == "paused"
    ok(call("product.sprint", action="close", sprintId=sprint, confirmed=True))
    assert ok(call("product.read"))["sprints"][0] == saved
    assert (machine.repo / "a.txt").read_bytes() == before
    assert len(api.handle(query("tasks.list"))["items"]) == 2
    assert not call("product.sprint", action="patch", sprintId=sprint, patch=dict(queueState="running"))["ok"]
    assert not call("product.task", action="sprint", taskId=tasks[0], sprintId=sprint)["ok"]


def test_real_port_editing_and_archive_projection(journey):
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
    env.update(WORKSPACE_EDIT_SOCKET=str(journey.endpoint), WORKSPACE_EDIT_PROJECT=journey.project_id)
    result = subprocess.run(["npm", "--prefix", "packages/workspace-ui", "test", "--",
                             "test/editing-journey.test.ts"], cwd=Path(__file__).resolve().parents[1],
                            env=env, timeout=55)
    assert result.returncode == 0


def test_running_sprint_accepts_edits_and_new_tasks_then_closes_with_fake_engine(journey):
    j = journey
    first, second = j.create("question-first"), j.create("queued-second")
    sprint = j.machine("product.sprint", action="create", description="Editable queue", runMode="automatic")["sprintId"]
    j.machine("product.task", action="sprint", taskId=first, sprintId=sprint, dependencies=[])
    j.machine("product.task", action="sprint", taskId=second, sprintId=sprint, dependencies=[first])
    j.machine("product.sprint", action="patch", sprintId=sprint,
              patch=dict(planning="created", taskIds=[first, second], queueState="running"))
    row = j.wait(lambda: (t if (t := j.query("tasks.get", first))["phase"] == "waiting" else None), "first did not ask")
    pending = j.query("tasks.get", second)
    j.machine("product.task", action="edit", taskId=second, expectedVersion=pending["version"], payload=CHOICES)
    assert j.query("tasks.get", second)["description"] == CHOICES["description"]
    refused = j.call("machine.call", dict(op="product.task", projectId=j.project_id,
                      params=dict(action="edit", taskId=first, expectedVersion=row["version"], payload=CHOICES)))
    assert refused["ok"] is False and refused["code"] == "task_started"
    third = j.create("late task")
    j.machine("product.task", action="sprint", taskId=third, sprintId=sprint, dependencies=[second])
    saved = j.product()["sprints"][0]
    assert saved["taskIds"] == [first, second, third]
    assert j.product()["tasks"][second]["dependencies"] == [first]
    bad = j.call("machine.call", dict(op="product.task", projectId=j.project_id,
                 params=dict(action="sprint", taskId=third, sprintId=sprint, dependencies=[third])))
    assert bad["ok"] is False
    j.machine("product.sprint", action="close", sprintId=sprint, confirmed=True)
    assert j.product()["sprints"][0]["archivedAt"]
    decision = j.query("decisions.list", row["executionId"])["items"][0]
    j.command("decisions.answer", decision["decisionId"], decision["version"],
              dict(executionId=row["executionId"], acpSessionId=decision["acpSessionId"], batchId=None,
                   answers=[dict(decisionId=decision["decisionId"],expectedVersion=decision["version"],questionId=decision["questionId"],optionId=decision["options"][0]["optionId"],text="Continue")]), decision["ownerEpoch"])
    j.wait(lambda: j.ready(first), "running task did not finish after sprint closure")
    assert j.query("tasks.get", second)["executionId"] is None
    assert j.query("tasks.get", third)["executionId"] is None
    assert j.query("tasks.get", second)["description"] == CHOICES["description"]
