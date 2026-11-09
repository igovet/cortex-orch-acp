"""Public immutable report pages, authorized by retained worker result references."""
from ..workspace_contract import validate
from .storage import WorkspaceError


def page_tx(db, store, project_id, report_id, *, cursor=None, limit=4000):
    # A caller cannot name a draft, pipeline or arbitrary private document.
    row = db.execute(
        "SELECT r.task_id FROM reports r JOIN workspace_executions e ON e.runtime_task_id=r.task_id "
        "JOIN workspace_tasks t ON t.task_id=e.task_id WHERE t.project_id=? AND r.id=? "
        "AND EXISTS (SELECT 1 FROM task_events v WHERE v.task_id=r.task_id "
        "AND v.event_type='worker_result' AND json_valid(v.payload_json) "
        "AND json_extract(v.payload_json,'$.report_ref')=r.id)",
        (project_id, report_id)).fetchone()
    if row is None:
        raise WorkspaceError("report_unavailable")
    args = dict(task_id=row["task_id"], report_id=report_id, limit=limit)
    if cursor is not None:
        args["cursor"] = cursor
    # Same public reader used by read_report: verifies immutable digest and cursor binding.
    result = store._read(db, args)
    return validate("ReportPage", dict(reportId=report_id, title=result["title"],
        sha256=result["sha256"], markdown=result["markdown"],
        totalCharacters=result["total_characters"], nextCursor=result["next_cursor"],
        partial=result["next_cursor"] is not None))
