"""External completion and content containment; disposable repositories, no engines."""
import pytest
from test_workspace_task_git import git_machine as machine, task_tree, helper, task_git
from test_workspace_machine_tools import machine as base_machine
from test_workspace_machine_tools import ok
from test_workspace_scheduler import API, setup
import scheduler
from test_workspace_check_execution import prepared


@pytest.mark.parametrize("checkout", ["main", "worktree"])
@pytest.mark.parametrize("remote", [False, True])
def test_content_containment_is_separate_from_push(machine, tmp_path, checkout, remote):
    path = task_tree(machine, tmp_path)
    if checkout == "main":
        path = machine.repo
        product = helper.product_of(tmp_path / "state", "wp_1")
        product["tasks"]["task"]["checkout"] = {"kind": "main"}
        helper.write_json(helper.product_path(tmp_path / "state", "wp_1"), product)
    (path / "a.txt").write_text("task change\n")
    helper.git(path, "add", "a.txt")
    helper.git(path, "commit", "-m", "Task implementation")
    if checkout == "worktree":
        # Same content, different commit identity: a real squash, not ancestry.
        machine.git("merge", "--squash", "cw/task")
        machine.git("commit", "-m", "Owner squash")
    if remote:
        machine.git("remote", "add", "origin", str(tmp_path / "not-contacted.git"))
    facts = ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]
    # A baseline alone is not task attribution in a shared checkout. The
    # receipt-backed main-checkout cases live in test_workspace_same_branch.
    assert facts["alreadyInTarget"] is (checkout == "worktree")
    assert facts["targetBranch"] == "main"
    assert facts["pushStatus"] == ("unknown" if remote else None)
    assert ok(machine("product.read"))["tasks"]["task"]["status"] == "ready_to_merge"
    reply = machine("product.integrate", dict(taskId="task", method="squash", targetBranch="main",
                    message="Do not merge again", confirmed=True, digest="reviewed"))
    assert reply["code"] == ("already_in_target" if checkout == "worktree" else "same_branch")


def test_untracked_addition_containment_does_not_mutate_git(machine, tmp_path):
    import hashlib
    path = task_tree(machine, tmp_path)
    (path / "new.txt").write_text("implemented outside\n")
    (machine.repo / "new.txt").write_text("implemented outside\n")
    machine.git("add", "new.txt")
    machine.git("commit", "-m", "Owner committed task")
    index = __import__("pathlib").Path(helper.out(path, "rev-parse", "--path-format=absolute", "--git-path", "index").strip())
    before = hashlib.sha256(index.read_bytes()).hexdigest()
    facts = ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]
    assert facts["alreadyInTarget"] is True
    assert hashlib.sha256(index.read_bytes()).hexdigest() == before
    ok(machine("product.task", dict(action="close", taskId="task", choice="already_in_target", confirmed=True)))
    saved = ok(machine("product.read"))["tasks"]["task"]
    assert saved["closedAs"] == "external" and saved.get("integration") is None
    assert (path / "new.txt").read_text() == "implemented outside\n"


def test_push_is_last_fetched_information_only(machine, tmp_path):
    task_tree(machine, tmp_path)
    machine.git("remote", "add", "origin", str(tmp_path / "not-contacted.git"))
    machine.git("config", "branch.main.remote", "origin")
    machine.git("config", "branch.main.merge", "refs/heads/main")
    machine.git("update-ref", "refs/remotes/origin/main", "HEAD")
    assert ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]["pushStatus"] == "pushed"
    (machine.repo / "owner.txt").write_text("other work")
    machine.git("add", "owner.txt")
    machine.git("commit", "-m", "Unpushed owner work")
    assert ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]["pushStatus"] == "not_pushed"


def test_containment_changed_refuses_confirmed_close(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (machine.repo / "a.txt").write_text("task change\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Owner committed")
    assert ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]["alreadyInTarget"] is True
    (machine.repo / "a.txt").write_text("different owner work\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "New work")
    assert machine("product.task", dict(action="close", taskId="task", choice="already_in_target", confirmed=True))["code"] == "containment_changed"


def test_pending_changes_and_missing_base_never_claim_containment(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "new.txt").write_text("new content\n")
    assert ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]["alreadyInTarget"] is False
    product = helper.product_of(tmp_path / "state", "wp_1")
    product["tasks"]["task"].pop("baseRevision")
    helper.write_json(helper.product_path(tmp_path / "state", "wp_1"), product)
    assert ok(machine("product.read"))["tasks"]["task"]["integrationFacts"]["alreadyInTarget"] is False


def test_external_close_is_confirmed_durable_and_not_acceptance(machine):
    assert machine("product.task", dict(action="close", taskId="task", choice="external", confirmed=False))["code"] == "confirmation_required"
    ok(machine("product.task", dict(action="close", taskId="task", choice="external", confirmed=True)))
    record = ok(machine("product.read"))["tasks"]["task"]
    assert record["status"] == "done" and record["closedAs"] == "external"
    assert record["notVerifiedByCortex"] is True
    assert record.get("acceptance") is None and record.get("integration") is None
    assert record["work"] == "unmerged"
    assert machine("product.task", dict(action="close", taskId="task", choice="external", confirmed=True))["code"] == "already_closed"


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("prepared", [("display", False)], indirect=True)
def test_real_port_external_close_refuses_live_then_survives_remount(prepared, tmp_path):
    from cortex_runtime.workspace_api import WorkspaceAPI, task_options
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from test_workspace_api import create_command
    from workspace_support.contract_socket import run_ui
    runtime, runtime_task, _, _ = prepared
    store = runtime._store
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    task = api.handle(create_command())["target"]
    task_options.prepare(store, task, {})
    def bind(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('external-session',?,?,'{}','{}','[]','now',NULL)", (store.project_root, runtime_task))
        bind_execution_tx(db, task, runtime_task, "external-session")
        db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id=?", (runtime_task,))
    store.immediate_write(bind)
    state = tmp_path / "owner-state"
    params = dict(action="close", taskId=task, choice="external", confirmed=True)
    with pytest.raises(helper.Refused, match="stop_required"):
        helper.task_action(store.project_root, api.project_id, state, params)
    assert not helper.product_of(state, api.project_id)["tasks"]
    store.immediate_write(lambda db: db.execute("UPDATE runtime_tasks SET state='PAUSED' WHERE task_id=?", (runtime_task,)))
    run_ui(api, state, dict(projectId=api.project_id, taskId=task), "test/external-done.test.tsx")
    saved = helper.product_read(store.project_root, state, api.project_id)["tasks"][task]
    assert saved["closedAs"] == "external" and saved["status"] == "done"
    assert saved.get("acceptance") is None and saved.get("integration") is None


@pytest.mark.parametrize("phase", ["stopped", "failed", "not_started"])
def test_external_done_unblocks_dependency_without_certifying_checks(tmp_path, phase):
    tasks = setup(tmp_path)
    tasks[0].update(phase=phase, executionId=None)
    product = helper.product_of(tmp_path, "project")
    product["tasks"]["first"] = dict(status="done", closedAs="external", notVerifiedByCortex=True)
    helper.write_json(helper.product_path(tmp_path, "project"), product)
    api = API(tasks, [])
    scheduler.tick(api, tmp_path)
    assert api.starts == ["next"]
    assert helper.product_of(tmp_path, "project")["tasks"]["first"].get("acceptance") is None
