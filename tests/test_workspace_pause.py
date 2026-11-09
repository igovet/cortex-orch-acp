"""Owner-only stop/resume state; fake engines are covered by the service journey."""
import pytest

from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
from cortex_runtime.store import Store
from test_workspace_decisions import asking, command, decisions
from test_workspace_api import query, create_command
from test_worker_wait_continuation import bound


@pytest.mark.parametrize("kind", ["question", "access"])
def test_paused_worker_decision_cannot_be_claimed_answered_or_cancelled_by_polling(bound, kind):
    from cortex_runtime.acp import questions_store, sessions
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.orchestration import access, task_pause, worker_waits
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    store, machine, coordinator, workers = bound
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner", dispatch_answer=lambda _: None)
    api.activate(request_id="activate")
    task = api.handle(create_command())["target"]
    sessions.create_session(store, "incoming", str(store.project_root), {}, {}, [])
    sessions.bind_task(store, "incoming", coordinator.task_id)
    execution = store.immediate_write(lambda db: bind_execution_tx(db, task, coordinator.task_id, "incoming"))
    if kind == "question":
        receipt = mcp_tools.dispatch(store, workers[0], "ask_question",
            dict(question="Keep this decision?", options=["Yes", "No"], request_key="ask"))
    else:
        receipt = mcp_tools.dispatch(store, workers[0], "access_request",
            dict(paths=["part1.txt"], reason="Overlapping sibling", request_key="access"))
        assert receipt["status"] == "pending"
    version = api.handle(query("tasks.get", task))["version"]
    task_pause.apply(api, execution, "pause", "pause", version)
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait["status"] == "open" and wait["continuation_required"]
    with store.connection() as db:
        assert worker_waits.valid(db, wait)
        assert db.execute("SELECT revoked_at FROM trusted_channels WHERE channel_id=?", (wait["channel_id"],)).fetchone()[0]
    if kind == "question":
        assert not questions_store.answer(store, receipt["question_id"], "late while paused", "text")
        assert questions_store.get(store, receipt["question_id"]).status == "open"
    access.reconcile(store, coordinator.task_id)
    assert worker_waits.claim(store, coordinator.task_id, wait["assignment_ref"], wait["engine_session_ref"]) is None
    assert worker_waits.list_waits(store, coordinator.task_id)[0]["status"] == "open"
    task_pause.apply(api, execution, "resume", "resume", api.handle(query("tasks.get", task))["version"])
    # Stop may still be settling its old process after the owner has resumed.
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, lambda *a: None), FakeWorkspace(), FakeConfigSnapshotSource())
    try:
        runtime._cancel_assignment(coordinator.task_id, wait["assignment_ref"], "owner_paused")
        assert worker_waits.list_waits(store, coordinator.task_id)[0]["status"] == "open"
    finally:
        runtime.shutdown()
    if kind == "question":
        assert questions_store.answer(store, receipt["question_id"], "Yes", "text")
        assert worker_waits.list_waits(store, coordinator.task_id)[0]["status"] == "ready"
        assert worker_waits.claim(store, coordinator.task_id, wait["assignment_ref"], wait["engine_session_ref"]) is None
    machine.cancel(coordinator.task_id)
    if kind == "question":
        assert not questions_store.answer(store, receipt["question_id"], "after close", "text")
    else:
        access.reconcile(store, coordinator.task_id)
        assert worker_waits.list_waits(store, coordinator.task_id)[0]["status"] == "cancelled"


@pytest.mark.parametrize("before", ["PLANNING", "WAITING_USER", "RECOVERING"])
def test_pause_resume_survives_store_restart_and_preserves_question(tmp_path, before):
    from cortex_runtime.orchestration.task_pause import apply
    api, execution, _ = asking(tmp_path)
    with api.store.connection() as db:
        task = db.execute("SELECT runtime_task_id FROM workspace_executions WHERE execution_id=?", (execution,)).fetchone()[0]
    if before != "WAITING_USER":
        api.store.immediate_write(lambda db: db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (before, task)))
    old = decisions(api, execution)
    stale_answer = command(old)
    version = api.handle(query("tasks.list"))["items"][0]["version"]
    assert apply(api, execution, "pause", "stop", version)["changed"]
    assert not apply(api, execution, "pause", "stop", version)["changed"]
    assert api.handle(query("tasks.list"))["items"][0]["phase"] == "paused"
    paused = decisions(api, execution)
    if before == "WAITING_USER":
        assert paused[0]["questionId"] == old[0]["questionId"]
        assert paused[0]["status"] == "pending"
        assert not paused[0]["allowedOperations"]
    assert api.handle(stale_answer)["outcome"] != "recorded"
    reopened_store = Store(tmp_path / ".cortex", project_root=tmp_path)
    reopened = WorkspaceAPI(reopened_store, project_id=api.project_id, actor_id=api.actor_id, dispatch_answer=lambda answer: None)
    reopened.activate(request_id="restart")
    assert reopened.handle(query("executions.get", execution))["state"] == "paused"
    version = reopened.handle(query("tasks.list"))["items"][0]["version"]
    assert apply(reopened, execution, "resume", "resume", version)["changed"]
    with reopened_store.connection() as db:
        assert db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (task,)).fetchone()[0] == before
    if before == "WAITING_USER":
        restored = decisions(reopened, execution)
        assert restored[0]["questionId"] == old[0]["questionId"]
        assert restored[0]["allowedOperations"] == ["decisions.answer"]
    with pytest.raises(WorkspaceError, match="stale_version"):
        apply(reopened, execution, "resume", "stale-resume", "0")
