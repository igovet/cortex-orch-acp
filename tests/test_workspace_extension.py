"""Boundary and consistency checks for the unadvertised Workspace extension."""
import json
import pytest

from cortex_runtime.workspace_contract import validate
from test_workspace_api import api_for, create_command, query, runtime_binding


def test_snapshot_uses_one_read_transaction(tmp_path, monkeypatch):
    from cortex_runtime.workspace_api import projections
    api = api_for(tmp_path)
    api.handle(create_command())
    original = projections.task_page
    def interleave(db, project, **kwargs):
        assert db.in_transaction
        api.handle(create_command("concurrent-write", "Added during snapshot"))
        return original(db, project, **kwargs)
    monkeypatch.setattr(projections, "task_page", interleave)
    snapshot = api.handle(query("project.snapshot"))
    assert snapshot["version"] == "2"
    assert len(snapshot["tasks"]["items"]) == 1
    monkeypatch.setattr(projections, "task_page", original)
    assert len(api.handle(query("tasks.list"))["items"]) == 2


def test_capabilities_and_unsupported_operations_never_start(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    caps = api.handle(query("capabilities.get"))
    validate("Capabilities", caps)
    assert caps["mappingVersion"] == "1"
    assert "history.page" in caps["operations"]
    for op in ("tasks.start", "decisions.answer"):
        assert op not in caps["operations"]
    command = dict(create_command(), operation="tasks.start", expectedVersion="1", payload={})
    assert api.handle(command)["outcome"] == "unsupported"
    assert "results.get" in caps["operations"]
    with pytest.raises(WorkspaceError, match="result_unavailable"):
        api.handle(query("results.get", "not-a-result"))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM runtime_tasks").fetchone()[0] == 0


def test_cursor_rejects_other_query_stale_and_malformed(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    for n in range(3):
        api.handle(create_command(str(n)))
    first = api.handle(query("tasks.list", limit=1))
    for cursor in ("broken", first["nextCursor"] + ".extra", "wt1.wrong.1"):
        with pytest.raises(WorkspaceError, match="invalid_cursor"):
            api.handle(query("tasks.list", limit=1, cursor=cursor))
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("tasks.list", limit=2, cursor=first["nextCursor"]))
    api.handle(create_command("new"))
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("tasks.list", limit=1, cursor=first["nextCursor"]))


def test_large_description_unavailable_not_truncated_and_pages_fit(tmp_path):
    api = api_for(tmp_path)
    description = "😀" * 32000
    task_id = api.handle(create_command(description=description))["target"]
    task = api.handle(query("tasks.get", task_id))
    validate("Task", task)
    assert task["descriptionAvailability"] == "unavailable"
    assert task["description"] == "" and task["warnings"]
    snapshot = api.handle(query("project.snapshot"))
    validate("ProjectSnapshot", snapshot)
    with api.store.connection() as db:
        assert db.execute("SELECT description FROM workspace_tasks").fetchone()[0] == description


def test_identity_binding_is_idempotent(tmp_path):
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    execution_id = runtime_binding(api, task_id)
    assert api.store.immediate_write(lambda db: bind_execution_tx(
        db, task_id, "runtime-1", "session-1")) == execution_id
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_executions").fetchone()[0] == 1
        assert db.execute("SELECT version FROM workspace_tasks").fetchone()[0] == 2


def test_extension_dispatch_requires_trusted_activation_and_stays_unadvertised(tmp_path):
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp.protocol import initialize_result
    from cortex_runtime.acp.runtime import UnavailableRuntime
    from cortex_runtime.workspace_api import WorkspaceError

    class Transport:
        def __init__(self):
            self.responses = []
        def send_response(self, response):
            self.responses.append(response)

    agent = Agent(UnavailableRuntime(), environ={})
    agent.initialized = True
    agent.transport = Transport()
    agent.sessions["session-host"] = {"project": str(tmp_path)}
    request = create_command()
    message = dict(jsonrpc="2.0", id=1, method="_cortex/workspace", params=request)
    agent.handle_message(message, None)
    assert agent.transport.responses[-1]["error"]["message"] == "workspace_not_activated"
    agent.activate_workspace("session-host", project_id="project-1", actor_id="local-owner",
                             request_id="activate-host")
    agent.handle_message(dict(message, id=2), None)
    receipt = agent.transport.responses[-1]["result"]
    validate("Receipt", receipt)
    assert receipt["target"] != "session-host"
    agent.handle_message(dict(message, id=3, params=query("tasks.get", receipt["target"])), None)
    assert agent.transport.responses[-1]["result"]["phase"] == "not_started"
    agent.handle_message(dict(message, id=4, params=dict(request, actorId="forged")), None)
    assert agent.transport.responses[-1]["error"]["message"] == "unauthorized"
    agent.handle_message(dict(message, id=5, params=dict(request, _meta={"taskId": "session-host"})), None)
    assert agent.transport.responses[-1]["error"]["message"] == "invalid_workspace_request"
    for params in (None, [], {"projectId": []}):
        agent.handle_message(dict(message, id=6, params=params), None)
        assert "error" in agent.transport.responses[-1]
    before = len(agent.transport.responses)
    agent.handle_message(dict(method="_cortex/workspace", params=create_command("notify")), None)
    assert len(agent.transport.responses) == before
    with agent._store_for(str(tmp_path)).connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 1
    assert "_cortex/workspace" not in json.dumps(initialize_result("test"))

    # A different project cannot reuse the trusted connection binding.
    agent.handle_message(dict(message, id=7, params=dict(request, projectId="project-2")), None)
    assert agent.transport.responses[-1]["error"]["message"] == "workspace_not_activated"
    with pytest.raises(WorkspaceError):
        agent.activate_workspace("missing-session", project_id="project-1", actor_id="local-owner",
                                 request_id="bad-activation")


def test_page_count_and_byte_boundaries(tmp_path):
    api = api_for(tmp_path)
    for n in range(51):
        api.handle(create_command(str(n), "Description " + str(n)))
    first = api.handle(query("tasks.list"))
    assert len(first["items"]) == 50 and first["nextCursor"]
    last = api.handle(query("tasks.list", cursor=first["nextCursor"]))
    assert len(last["items"]) == 1 and last["nextCursor"] is None
    for n in range(3):
        api.handle(create_command("large-" + str(n), "x" * 32000))
    seen = set()
    cursor = None
    while True:
        page = api.handle(query("tasks.list", cursor=cursor))
        validate("TaskPage", page)
        for item in page["items"]:
            assert item["taskId"] not in seen
            seen.add(item["taskId"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
    assert len(seen) == 54


def test_queries_do_not_repair_missing_storage(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceError
    api = api_for(tmp_path)
    with api.store.connection() as db:
        db.execute("DROP TABLE workspace_requests")
    with pytest.raises(WorkspaceError, match="workspace_storage_unavailable"):
        api.handle(query("tasks.list"))
    with api.store.connection() as db:
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='workspace_requests'").fetchone() is None


def test_untrusted_actor_cannot_activate_or_inspect_existing_project(tmp_path):
    from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
    api = api_for(tmp_path)
    other = WorkspaceAPI(api.store, project_id="project-1", actor_id="other")
    with pytest.raises(WorkspaceError, match="unauthorized"):
        other.activate(request_id="other-activation")
    with pytest.raises(WorkspaceError, match="unauthorized"):
        other.handle(query("tasks.list"))


def test_binding_rolls_back_with_callers_transaction(tmp_path):
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    api = api_for(tmp_path)
    task_id = api.handle(create_command())["target"]
    # First seed coherent runtime facts, then roll back only the public binding.
    runtime_binding(api, task_id)
    with api.store.connection() as db:
        db.execute("DELETE FROM workspace_executions")
        db.execute("UPDATE workspace_tasks SET execution_id=NULL,version=1")
    def fail(db):
        bind_execution_tx(db, task_id, "runtime-1", "session-1")
        raise RuntimeError("injected failure")
    with pytest.raises(RuntimeError, match="injected failure"):
        api.store.immediate_write(fail)
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_executions").fetchone()[0] == 0
        task = db.execute("SELECT * FROM workspace_tasks").fetchone()
        assert task["execution_id"] is None and task["version"] == 1


def test_split_validates_workspace_optional_group(tmp_path):
    from cortex_runtime.split import _validate_format
    from cortex_runtime.contracts import StoreError
    api = api_for(tmp_path)
    with api.store.connection() as db:
        _validate_format(db)
        db.execute("DROP TABLE workspace_requests")
        with pytest.raises(StoreError, match="unsupported_storage"):
            _validate_format(db)
        for row in db.execute("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall():
            db.execute('DROP TRIGGER "' + row[0] + '"')
        for table in ("workspace_starts", "workspace_outbox", "workspace_outbox_state", "workspace_executions", "workspace_tasks", "workspace_projects"):
            db.execute("DROP TABLE " + table)
        _validate_format(db)


def test_split_carries_only_selected_workspace_project(tmp_path):
    from cortex_runtime.store import Store
    from cortex_runtime.split import split_project
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    source = Store(tmp_path / "legacy")
    selected = tmp_path / "selected"
    other = tmp_path / "other"
    selected.mkdir()
    (selected / ".cortex").mkdir(mode=0o700)
    other.mkdir()
    with source.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        for name, root in (("selected", selected), ("other", other)):
            db.execute("INSERT INTO tasks VALUES (?,?,?)", ("r-" + name, str(root), "now"))
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) "
                       "VALUES (?,'EXECUTING','Goal','now','now')", ("r-" + name,))
            db.execute("INSERT INTO task_owners VALUES (?,'owner',1,'now')", ("r-" + name,))
            db.execute("INSERT INTO acp_host_sessions VALUES (?,?,?,'{}','{}','[]','now',NULL)",
                       ("s-" + name, str(root), "r-" + name))
            db.execute("INSERT INTO workspace_projects VALUES (?,?,?,'now',1)",
                       (name, str(root), "owner"))
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) "
                       "VALUES (?,?,?,'now')", ("w-" + name, name, "Description"))
            execution_id = bind_execution_tx(db, "w-" + name, "r-" + name, "s-" + name)
            db.execute("INSERT INTO workspace_starts VALUES (?,?,?,?,?,?,?,'dispatched',1,'now')",
                       (execution_id, "w-" + name, name, "owner", "start-" + name,
                        "workspace:" + name, "source-" + name))
            db.execute("INSERT INTO workspace_requests VALUES (?,'owner','create','tasks.create','digest','{}')", (name,))
        db.commit()
        expected_outbox = [tuple(r) for r in db.execute("SELECT * FROM workspace_outbox WHERE project_id='selected' ORDER BY sequence")]
        expected_position = [tuple(r) for r in db.execute("SELECT * FROM workspace_outbox_state WHERE project_id='selected'")]
    split_project(str(source.directory), str(selected), str(tmp_path / "backup.sqlite3"), access_stopped=True)
    target = Store(selected / ".cortex", initialize=False, project_root=selected)
    with target.connection(read_only=True) as db:
        assert [r[0] for r in db.execute("SELECT project_id FROM workspace_projects")] == ["selected"]
        assert [r[0] for r in db.execute("SELECT task_id FROM workspace_tasks")] == ["w-selected"]
        assert [r[0] for r in db.execute("SELECT runtime_task_id FROM workspace_executions")] == ["r-selected"]
        assert db.execute("SELECT count(*) FROM workspace_requests").fetchone()[0] == 1
        assert [r[0] for r in db.execute("SELECT project_id FROM workspace_starts")] == ["selected"]
        assert [tuple(r) for r in db.execute("SELECT * FROM workspace_outbox ORDER BY sequence")] == expected_outbox
        assert [tuple(r) for r in db.execute("SELECT * FROM workspace_outbox_state")] == expected_position
        assert not db.execute("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall()
        assert len(db.execute("SELECT name FROM sqlite_temp_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall()) == 45
    with source.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_projects").fetchone()[0] == 2


