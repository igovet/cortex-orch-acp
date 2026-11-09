"""Explicit task deletion; no age-wide selection, force flag or model entry point."""
from ..cleanup import finish_deletions, protected_active_tasks, _reclaim_space
from .storage import WorkspaceError, append_public_tx


def purge_task(store, project_id, task_id):
    with store.connection(exclusive=True) as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT * FROM workspace_tasks WHERE project_id=? AND task_id=?", (project_id, task_id)).fetchone()
        if row is None:
            raise WorkspaceError("not_found")
        execution = db.execute("SELECT * FROM workspace_executions WHERE task_id=?", (task_id,)).fetchone()
        start = db.execute("SELECT * FROM workspace_starts WHERE task_id=?", (task_id,)).fetchone()
        runtime_id = execution["runtime_task_id"] if execution else None
        if start and (start["state"] == "unknown" or not execution):
            raise WorkspaceError("active_execution")
        if runtime_id:
            runtime = db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (runtime_id,)).fetchone()
            if runtime is None or runtime[0] not in {"COMPLETED", "FAILED", "CANCELLED"} or runtime_id in protected_active_tasks(db):
                raise WorkspaceError("active_execution")
            db.execute("INSERT OR IGNORE INTO pending_deletions VALUES (?)", (runtime_id,))
            db.commit()
            db.execute("BEGIN IMMEDIATE")
            finish_deletions(store, db, runtime_id)
        else:
            remove_adapter_task(db, project_id, task_id)
        db.commit()
        result = {"deleted": True}
        result.update(_reclaim_space(store, db))
        return result


def remove_adapter_task(db, project_id, task_id):
    db.execute("DELETE FROM workspace_task_options WHERE task_id=?", (task_id,))
    db.execute("DELETE FROM workspace_starts WHERE task_id=?", (task_id,))
    db.execute("DELETE FROM workspace_executions WHERE task_id=?", (task_id,))
    version = db.execute("SELECT version FROM workspace_tasks WHERE task_id=?", (task_id,)).fetchone()
    db.execute("DELETE FROM workspace_tasks WHERE task_id=? AND project_id=?", (task_id, project_id))
    db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (project_id,))
    append_public_tx(db, project_id=project_id, entity_id=task_id, kind="task", version=version[0]+1)
