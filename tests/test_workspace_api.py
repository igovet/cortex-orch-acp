"""Workspace adapter tests: no engine, model, network or prompt dispatch."""
from concurrent.futures import ThreadPoolExecutor

import pytest

from cortex_runtime.store import Store
from cortex_runtime.workspace_contract import ContractError, validate


def api_for(tmp_path, *, activate=True):
    from cortex_runtime.workspace_api import WorkspaceAPI
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    if activate:
        api.activate(request_id="activate-1")
    return api


def create_command(key="create-1", description="Build a thing"):
    return dict(protocol=dict(major=1, minor=0), requestId=key,
                projectId="project-1", actorId="local-owner", operation="tasks.create",
                target="project-1", expectedVersion="0", ownerEpoch=None,
                payload=dict(description=description))


def query(operation, target=None, **extra):
    return dict(protocol=dict(major=1, minor=0), requestId="read-1",
                projectId="project-1", operation=operation, target=target, **extra)


def runtime_binding(api, task_id, state="EXECUTING"):
    """Real persisted identity facts, with no runtime or model invoked."""
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    def write(db):
        db.execute("INSERT INTO tasks VALUES ('runtime-1',?,?)",
                   (api.store.project_root, "2026-11-05T00:00:00+00:00"))
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) "
                   "VALUES ('runtime-1',?,'Goal','now','now')", (state,))
        db.execute("INSERT INTO task_owners VALUES ('runtime-1','owner',1,'now')")
        db.execute("INSERT INTO acp_host_sessions VALUES ('session-1',?,'runtime-1','{}','{}','[]','now',NULL)",
                   (api.store.project_root,))
        return bind_execution_tx(db, task_id, "runtime-1", "session-1")
    return api.store.immediate_write(write)


def test_draft_only_no_runtime_calls_and_no_title(tmp_path, monkeypatch):
    import cortex_runtime.bootstrap as bootstrap
    def forbidden(*args, **kwargs):
        pytest.fail("draft called runtime bootstrap")
    monkeypatch.setattr(bootstrap, "bootstrap_task", forbidden)
    api = api_for(tmp_path)
    receipt = api.handle(create_command(description="Line one\nLine two"))
    validate("Receipt", receipt)
    task = api.handle(query("tasks.get", receipt["target"]))
    validate("Task", task)
    assert task["description"] == "Line one\nLine two"
    assert task["title"] is None and task["titleAvailability"] == "not_connected"
    assert task["phase"] == task["executionState"] == "not_started"
    assert task["executionId"] is None
    with api.store.connection() as db:
        for table in ("tasks", "runtime_tasks", "source_events", "acp_host_sessions", "workspace_executions"):
            assert db.execute("SELECT count(*) FROM " + table).fetchone()[0] == 0


def test_activation_separate_recorded_and_idempotent(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path, activate=False)
    with pytest.raises(WorkspaceError, match="project_inactive"):
        api.handle(create_command())
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_projects").fetchone()[0] == 0
    first = api.activate(request_id="activate-1")
    assert api.activate(request_id="activate-1") == first
    assert first["outcome"] == "recorded"
    with api.store.connection() as db:
        row = db.execute("SELECT * FROM workspace_projects").fetchone()
        assert row["activated_by"] == "local-owner" and row["activated_at"]
    assert api.activate(request_id="activate-2")["outcome"] == "stale_version"


def test_identity_idempotency_concurrency_and_changed_payload(tmp_path):
    api = api_for(tmp_path)
    command = create_command()
    with ThreadPoolExecutor(max_workers=4) as pool:
        receipts = list(pool.map(lambda _: api.handle(command), range(8)))
    assert all(r == receipts[0] for r in receipts)
    assert receipts[0]["target"] != command["target"]
    assert api.handle(dict(command, payload=dict(description="Changed")))["outcome"] == "payload_conflict"
    second = api.handle(create_command("create-2"))
    assert second["target"] != receipts[0]["target"]  # no text-based dedup
    assert api.handle(query("requests.get", "create-1")) == receipts[0]
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 2


@pytest.mark.parametrize("change", [
    {"actorId": "someone-else"}, {"projectId": "another-project"},
    {"ownerEpoch": "1"}, {"target": "another-project"},
])
def test_authorization_and_preconditions_refused(tmp_path, change):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    command = dict(create_command(), **change)
    try:
        result = api.handle(command)
    except WorkspaceError:
        pass
    else:
        assert result["outcome"] in ("unauthorized", "invalid_request")
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 0


@pytest.mark.parametrize("patch", [
    {"payload": {"description": ""}}, {"payload": {"description": " \n "}},
    {"payload": {"description": "x" * 32001}},
    {"payload": {"description": "ok", "title": "Not allowed"}},
    {"_meta": {"taskId": "session-1"}},
    {"expectedVersion": "1"}, {"protocol": {"major": 2, "minor": 0}},
])
def test_invalid_command_does_not_write(tmp_path, patch):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    with pytest.raises((ContractError, WorkspaceError)):
        api.handle(dict(create_command(), **patch))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 0


MAPPING = {
    "CREATED": ("planning", "starting"),
    "PLANNING": ("planning", "running"), "PLAN_READY": ("planning", "running"),
    "EXECUTING": ("execution", "running"), "REVIEWING": ("review", "running"),
    "VERIFYING": ("verification", "running"), "RECONCILING": ("finalizing", "running"),
    "ANSWER_READY": ("finalizing", "running"), "WAITING_USER": ("waiting", "waiting_user"),
    "RECOVERING": ("recovery", "recovering"), "BLOCKED": ("blocked", "blocked"),
    "FAILED": ("failed", "failed"), "CANCELLED": ("stopped", "stopped"),
    "COMPLETED": ("result", "result"),
}


@pytest.mark.parametrize("state,expected", MAPPING.items())
def test_all_lifecycle_states(tmp_path, state, expected):
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    execution_id = runtime_binding(api, task_id, state)
    task = api.handle(query("tasks.get", task_id))
    execution = api.handle(query("executions.get", execution_id))
    validate("Task", task)
    validate("Execution", execution)
    assert (task["phase"], task["executionState"]) == expected
    assert not task["warnings"] and not execution["warnings"]
    assert len({task_id, execution_id, execution["runtimeTaskId"], execution["acpSessionId"]}) == 4
    assert execution["runtimePlanRevision"] == "0"
    assert execution["version"] == "1"  # not a runtime plan revision
    with api.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET revision=27 WHERE task_id='runtime-1'")
    assert api.handle(query("executions.get", execution_id))["version"] == "2"  # public invalidation, not revision 27


@pytest.mark.parametrize("mutation", [
    "UPDATE runtime_tasks SET state='NEW_STATE'",
    "UPDATE runtime_tasks SET state=''",
    "UPDATE acp_host_sessions SET project_root='elsewhere'",
    "DELETE FROM task_owners",
    "UPDATE task_owners SET epoch=2",
    "UPDATE runtime_tasks SET blocker='unresolved',state='COMPLETED'",
    "DELETE FROM runtime_tasks",
])
def test_missing_or_contradictory_evidence_unknown(tmp_path, mutation):
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    execution_id = runtime_binding(api, task_id)
    with api.store.connection() as db:
        db.execute(mutation)
    task = api.handle(query("tasks.get", task_id))
    execution = api.handle(query("executions.get", execution_id))
    assert task["phase"] == task["executionState"] == execution["state"] == "unknown"
    assert task["warnings"] and execution["warnings"]


def test_wait_reason_and_session_not_a_task(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    runtime_binding(api, task_id, "WAITING_USER")
    with api.store.connection() as db:
        db.execute("INSERT INTO task_orchestration(task_id,state,updated_at,wait_reason) "
                   "VALUES ('runtime-1','waiting','now','question')")
    assert api.handle(query("tasks.get", task_id))["waitReason"] == "question"
    for value in ("session-1", "runtime-1"):
        with pytest.raises(WorkspaceError, match="not_found"):
            api.handle(query("tasks.get", value))


def test_binding_validates_and_rolls_back(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    with pytest.raises(WorkspaceError, match="binding"):
        api.store.immediate_write(lambda db: bind_execution_tx(db, task_id, "absent", "generic-session"))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_executions").fetchone()[0] == 0


def test_typed_bounded_pages_and_snapshot(tmp_path):
    api = api_for(tmp_path)
    for n in range(4):
        api.handle(create_command("create-" + str(n), "Task " + str(n)))
    first = api.handle(query("tasks.list", limit=2))
    validate("TaskPage", first)
    second = api.handle(query("tasks.list", limit=2, cursor=first["nextCursor"]))
    assert second["nextCursor"] is None
    assert len({t["taskId"] for t in first["items"] + second["items"]}) == 4
    snapshot = api.handle(query("project.snapshot"))
    validate("ProjectSnapshot", snapshot)
    assert snapshot["tasks"]["highWatermark"] == snapshot["highWatermark"]


def test_inspection_installs_nothing_and_initializing_open_repairs(tmp_path):
    from cortex_runtime.workspace_api.storage import WORKSPACE_API_TABLES
    api = api_for(tmp_path)
    with api.store.connection() as db:
        for row in db.execute("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall():
            db.execute('DROP TRIGGER "' + row[0] + '"')
        for table in ("workspace_starts", "workspace_outbox", "workspace_outbox_state", "workspace_requests", "workspace_executions", "workspace_tasks", "workspace_projects"):
            db.execute("DROP TABLE " + table)
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = api.store.path.read_bytes()
    Store(api.store.directory, initialize=False, project_root=tmp_path)
    assert before == api.store.path.read_bytes()
    with api.store.connection() as db:
        names = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert not names & WORKSPACE_API_TABLES
    Store(api.store.directory, project_root=tmp_path)
    with api.store.connection() as db:
        names = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert WORKSPACE_API_TABLES <= names


def test_format11_inspection_refuses_then_normal_open_migrates(tmp_path):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.workspace_api.storage import WORKSPACE_API_TABLES
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        for row in db.execute("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall():
            db.execute('DROP TRIGGER "' + row[0] + '"')
        for table in ("workspace_starts", "workspace_outbox", "workspace_outbox_state", "workspace_requests", "workspace_executions", "workspace_tasks", "workspace_projects"):
            db.execute("DROP TABLE " + table)
        db.execute("PRAGMA user_version=11")
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = store.path.read_bytes()
    with pytest.raises(StoreError, match="legacy_storage_migration_required"):
        Store(store.directory, initialize=False, project_root=tmp_path)
    assert store.path.read_bytes() == before
    Store(store.directory, project_root=tmp_path)
    with store.connection() as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == 12
        assert WORKSPACE_API_TABLES <= {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
