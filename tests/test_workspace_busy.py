"""Read latency and owner closure regressions; disposable state, no engines."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))
import machine_tools
import scheduler
from test_workspace_scheduler import API, setup


def test_ui_busy_regressions():
    done = subprocess.run(["timeout", "35", str(ROOT / "packages/workspace-ui/node_modules/.bin/vitest"),
                           "run", "--config", "tests/workspace-busy.vitest.config.ts"],
                          cwd=ROOT, capture_output=True, text=True, timeout=40)
    assert done.returncode == 0, done.stdout + done.stderr


def test_helper_read_does_not_wait_for_product_writer(tmp_path):
    state = tmp_path / "state"
    request = dict(op="product.read", root=str(tmp_path), projectId="p", stateDir=str(state))
    with machine_tools.Locked(state):
        try:
            done = subprocess.run(
                [sys.executable, "-I", "-B", str(ROOT / "crates/workspace-service/machine_tools.py")],
                input=json.dumps(request), capture_output=True, text=True, timeout=1)
        except subprocess.TimeoutExpired:
            pytest.fail("A completed read waited on the product writer just to log its reply")
    assert json.loads(done.stdout)["ok"] is True


@pytest.mark.parametrize("choice", ["as_is", "external", "cancel"])
@pytest.mark.parametrize("legacy_pause", [False, True])
def test_owner_closure_queue_and_continue(tmp_path, choice, legacy_pause):
    tasks = setup(tmp_path)
    tasks[0]["phase"] = "stopped"
    product = machine_tools.product_of(tmp_path, "project")
    product["tasks"]["first"] = dict(status="done", closedAs=choice, notVerifiedByCortex=True)
    if legacy_pause:
        product["sprints"][0].update(queueState="paused", pauseKind="attention",
                                    pauseReason="A task needs attention or has an unknown outcome.")
    machine_tools.write_json(machine_tools.product_path(tmp_path, "project"), product)
    if legacy_pause:
        # Continue queue is an owner operation, not a database repair.
        machine_tools.sprint_action("project", tmp_path, dict(action="patch", sprintId="s",
                                   patch=dict(queueState="running", pauseReason=None)))
    api = API(tasks, [])
    scheduler.tick(api, tmp_path)
    assert api.starts == ([] if choice == "cancel" else ["next"])
    saved = machine_tools.product_of(tmp_path, "project")
    assert saved["tasks"]["first"]["notVerifiedByCortex"] is True
    assert not saved["tasks"]["first"].get("automaticReady")


@pytest.mark.parametrize("phase", ["working", "unknown", "recovery", "waiting"])
def test_owner_closure_does_not_hide_a_live_execution(tmp_path, phase):
    tasks = setup(tmp_path)
    tasks[0]["phase"] = phase
    product = machine_tools.product_of(tmp_path, "project")
    product["tasks"]["first"] = dict(status="done", closedAs="as_is")
    machine_tools.write_json(machine_tools.product_path(tmp_path, "project"), product)
    api = API(tasks, [])
    scheduler.tick(api, tmp_path)
    assert api.starts == []


@pytest.mark.parametrize("mode", ["automatic", "continue", "manual"])
def test_real_api_starts_dependent_after_owner_closed_task(tmp_path, mode):
    from test_workspace_api import api_for, create_command, query, runtime_binding
    api = api_for(tmp_path)
    first = api.handle(create_command("first"))["target"]
    second = api.handle(create_command("next"))["target"]
    runtime_binding(api, first, "CANCELLED")
    state = tmp_path / "machine"
    machine_tools.task_action(tmp_path, api.project_id, state,
                             dict(action="close", taskId=first, choice="as_is", confirmed=True))
    product = machine_tools.product_of(state, api.project_id)
    product["tasks"][second] = {"dependencies": [first]}
    product["sprints"] = [dict(id="s", runMode="automatic", planning="created",
                              queueState="paused" if mode == "continue" else "running",
                              taskIds=[first, second])]
    machine_tools.write_json(machine_tools.product_path(state, api.project_id), product)
    starts = []
    api.dispatch_start = starts.append  # No native or fake paid engine is constructed.
    if mode == "continue":
        machine_tools.sprint_action(api.project_id, state, dict(action="patch", sprintId="s",
                                   patch=dict(queueState="running", pauseReason=None)))
    if mode == "manual":
        task = api.handle(query("tasks.get", second))
        result = api.handle(dict(protocol=dict(major=1,minor=2), requestId="manual-start",
                                 projectId=api.project_id, actorId=api.actor_id, operation="tasks.start",
                                 target=second, expectedVersion=task["version"], ownerEpoch=None, payload={}))
        assert result["outcome"] == "recorded"
    else:
        scheduler.tick(api, state)
    assert len(starts) == 1
    assert starts[0]["task_id"] == second
    assert machine_tools.product_of(state, api.project_id)["tasks"][first]["notVerifiedByCortex"] is True
