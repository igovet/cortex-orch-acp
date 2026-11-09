"""Archive deletion reuses the runtime's locked, journaled retention machinery."""
import pytest

from test_workspace_api import api_for, create_command, query, runtime_binding


def test_purge_refuses_live_runtime_and_keeps_all_rows(tmp_path):
    from cortex_runtime.workspace_api.purge import purge_task
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task)
    with pytest.raises(Exception, match="active"):
        purge_task(api.store, "project-1", task)
    assert api.handle(query("tasks.get", task))["taskId"] == task


def test_purge_deletes_runtime_and_adapter_rows_and_reclaims_space(tmp_path):
    from cortex_runtime.workspace_api.purge import purge_task
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task, state="COMPLETED")
    outcome = purge_task(api.store, "project-1", task)
    assert outcome["deleted"] is True
    assert outcome["vacuum_status"] == "completed"
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM tasks").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM workspace_tasks").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM workspace_task_options").fetchone()[0] == 0
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []


def test_purge_never_deletes_another_project_or_unknown_start(tmp_path):
    from cortex_runtime.workspace_api.purge import purge_task
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    with pytest.raises(Exception, match="not_found"):
        purge_task(api.store, "another-project", task)
    api.store.immediate_write(lambda db: db.execute("INSERT INTO workspace_starts VALUES ('e',?,'project-1','local-owner','s','o','event','unknown',1,'now')", (task,)))
    with pytest.raises(Exception, match="active"):
        purge_task(api.store, "project-1", task)
