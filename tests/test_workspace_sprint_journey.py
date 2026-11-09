"""Empty-HOME sprint journeys; real runtime and service, fake sessions only."""
import os
import pytest
import subprocess
from pathlib import Path

from test_workspace_safety_journeys import journey

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("state", ["intact", "edited_plan", "dependency_mismatch", "purged", "existing"])
def test_older_build_sprint_members_return_on_activation_without_reading_release(tmp_path, monkeypatch, state):
    import sys
    from types import SimpleNamespace
    sys.path.insert(0, str(ROOT / "crates/workspace-service"))
    import agent_launcher
    import machine_tools
    import task_titles
    from cortex_runtime.acp.agent import Agent
    from test_workspace_api import api_for, query

    api = api_for(tmp_path)
    # Exact observed upgrade shape: product sprint/plan survive, selected store has no tasks.
    # The old .cortex is not a data source and must not even be opened.
    (tmp_path / ".cortex-dev").mkdir(mode=0o700)
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    from cortex_runtime.store import Store
    from cortex_runtime.workspace_api import WorkspaceAPI
    store = Store(tmp_path / ".cortex-dev", project_root=tmp_path)
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    old = tmp_path / ".cortex/cortex.sqlite3"
    before = old.read_bytes()
    ids = ["old-first", "old-second"]
    plan = dict(version=1, items=[
        dict(key="second", title="Second", description="Do second", dependsOn=["first"]),
        dict(key="first", title="First", description="Do first", dependsOn=[])])
    product = machine_tools.product_of(tmp_path / "state", "project-1")
    product["sprints"] = [dict(id="sprint", projectId="project-1", planning="created",
        materializedVersion=1, plan=plan, taskIds=ids, queueState="running", runMode="automatic")]
    product["tasks"] = {ids[0]:dict(sprintId="sprint", dependencies=[], checkout=dict(kind="main")),
                        ids[1]:dict(sprintId="sprint", dependencies=[ids[0]], checkout=dict(kind="main"))}
    if state == "edited_plan":
        product["sprints"][0]["materializedVersion"] = 2
    elif state == "dependency_mismatch":
        product["tasks"][ids[1]]["dependencies"] = []
    elif state == "purged":
        for record in product["tasks"].values():
            record["purged"] = True
    elif state == "existing":
        api.store.immediate_write(lambda db: db.execute(
            "INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,?,'now')",
            (ids[0], "project-1", "Owner changed this description")))
    machine_tools.write_json(machine_tools.product_path(tmp_path / "state", "project-1"), product)

    host = object.__new__(agent_launcher.ServiceAgent)
    host._service_template = None
    host._service_binding = SimpleNamespace(project_id="project-1", actor_id="local-owner")
    host._workspace_apis = {"project-1": api}
    host.environ = {"CORTEX_WORKSPACE_STATE_DIR": str(tmp_path / "state")}
    host._start_preflight = False
    host._workspace_shutdown = None
    host.activate_workspace = lambda *args, **kwargs: {}
    monkeypatch.setattr(Agent, "_session_new", lambda *_: {"sessionId":"template"})
    monkeypatch.setattr(task_titles, "TitleJobs", lambda *_: None)
    monkeypatch.setattr(agent_launcher.threading, "Thread", lambda **_: SimpleNamespace(start=lambda:None))
    for _ in range(2):
        host._service_template = None
        host._session_new({"cwd":str(tmp_path)})
    tasks = api.handle(query("tasks.list"))["items"]
    if state in ("edited_plan", "dependency_mismatch", "purged"):
        assert tasks == []
    else:
        assert [(t["taskId"], t["description"], t["executionState"]) for t in tasks] == [
            (ids[0], "Owner changed this description" if state == "existing" else "Do first", "not_started"),
            (ids[1], "Do second", "not_started")]
    saved = machine_tools.product_of(tmp_path / "state", "project-1")
    if state != "purged":
        assert saved["sprints"][0]["queueState"] == "paused"
        assert saved["sprints"][0]["pauseReason"]
    assert old.read_bytes() == before


def test_first_sprints_through_real_port(journey):
    j = journey
    env = {key: value for key, value in os.environ.items()
           if key in ("PATH", "LANG", "LC_ALL", "TMPDIR")}
    env.update(WORKSPACE_SPRINT_SOCKET=str(j.endpoint), WORKSPACE_SPRINT_PROJECT=j.project_id)
    result = subprocess.run(["npm", "--prefix", "packages/workspace-ui", "test", "--",
                             "test/sprint-journey.test.ts"], cwd=ROOT, env=env, timeout=60)
    errors = j.project / ".cortex-dev/fake-errors"
    assert not errors.exists(), errors.read_text() if errors.exists() else ""
    assert result.returncode == 0
    assert len(j.product()["sprints"]) == 2
