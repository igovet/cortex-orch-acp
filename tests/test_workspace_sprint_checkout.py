"""Sprint-wide checkout choice persistence and existing task preparation, no engines."""
import pytest

from cortex_runtime.workspace_api import task_options
from test_workspace_api import api_for, create_command
from test_workspace_machine_tools import machine, ok


@pytest.mark.parametrize("kind", ["main", "worktree"])
def test_sprint_checkout_is_durable_and_task_preparation_uses_it(machine, tmp_path, kind):
    sprint = ok(machine("product.sprint", dict(action="create", description="Two changes",
                                             runMode="manual", checkout=dict(kind=kind))))["sprintId"]
    saved = ok(machine("product.read"))["sprints"][0]
    assert saved["id"] == sprint and saved["checkout"] == dict(kind=kind)
    # The helper runs in a new process for each read/write.
    ok(machine("product.sprint", dict(action="patch", sprintId=sprint, patch=dict(planning="ready"))))
    assert ok(machine("product.read"))["sprints"][0]["checkout"] == dict(kind=kind)
    api = api_for(machine.repo)
    env = dict(HOME=str(tmp_path), CORTEX_WORKSPACE_STATE_DIR=str(tmp_path / "state"),
               XDG_DATA_HOME=str(tmp_path / "data"))
    paths = []
    for i in range(2):
        command = create_command("sprint-task-" + str(i))
        command["payload"]["checkout"] = saved["checkout"]
        task = api.handle(command)["target"]
        paths.append(task_options.prepare(api.store, task, env))
        with api.store.connection() as db:
            assert task_options.read(db, task)["checkout"]["kind"] == kind
    if kind == "main":
        assert paths == [str(machine.repo)] * 2
    else:
        assert len(set(paths)) == 2 and str(machine.repo) not in paths
        for path in paths:
            assert task_options.git(path, "branch", "--show-current").startswith("cw/")
            assert task_options.git(path, "rev-parse", "--git-common-dir")


@pytest.mark.parametrize("checkout", [None, "main", {}, {"kind": "remote"}, {"kind": "main", "path": "/arbitrary"}])
def test_invalid_sprint_checkout_refuses_without_saving(machine, checkout):
    result = machine("product.sprint", dict(action="create", description="Do not save", checkout=checkout))
    assert result["ok"] is False and result["code"] == "invalid_checkout"
    assert ok(machine("product.read"))["sprints"] == []


def test_older_sprint_without_checkout_keeps_its_original_shape(machine):
    ok(machine("product.sprint", dict(action="create", description="Older client")))
    assert "checkout" not in ok(machine("product.read"))["sprints"][0]
