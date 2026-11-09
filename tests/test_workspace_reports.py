"""Public worker reports: real publication, bounded reads, no private state."""
import pytest

from cortex_runtime.delegation.events import append_event
from cortex_runtime.workspace_api import WorkspaceError
from test_workspace_decisions import asking
from test_workspace_api import query


def published(api, execution, *, expose=True):
    with api.store.connection() as db:
        runtime = db.execute("SELECT runtime_task_id FROM workspace_executions WHERE execution_id=?", (execution,)).fetchone()[0]
        thread = db.execute("SELECT thread_id FROM thread_bindings WHERE task_id=? AND parent_thread_id IS NULL", (runtime,)).fetchone()[0]
    draft = api.store.call("create_draft", dict(template="general", request_key="draft"), thread, trusted=True)
    body = draft["required_first_line"] + "\n\n# Worker report\n\n" + "fixture prose\n" * 700
    from pathlib import Path
    Path(draft["draft_path"]).write_text(body)
    report = api.store.call("write_report", dict(draft_id=draft["draft_id"], title="Worker report",
        summary="Fixture", author="worker", request_key="publish"), thread, trusted=True)["report_id"]
    if expose:
        append_event(api.store, runtime, "worker_result", "fixture-result", dict(report_ref=report, summary="Fixture"), assignment_ref="fixture-worker", origin="fixture")
    return report, body


def test_public_report_pages_identity_and_reopen(tmp_path):
    api, execution, _ = asking(tmp_path)
    report, body = published(api, execution)
    first = api.handle(query("reports.read", report, limit=4000))
    assert first["reportId"] == report
    assert first["partial"] and first["nextCursor"]
    assert len(first["markdown"]) == 4000
    assert first["totalCharacters"] == len(body)
    text, cursor = first["markdown"], first["nextCursor"]
    while cursor:
        page = api.handle(query("reports.read", report, cursor=cursor, limit=4000))
        assert page["sha256"] == first["sha256"]
        text += page["markdown"]
        cursor = page["nextCursor"]
    assert text == body
    assert not page["partial"]
    assert set(first) == {"reportId", "title", "sha256", "markdown", "totalCharacters", "nextCursor", "partial"}
    from cortex_runtime.workspace_api import WorkspaceAPI
    reopened = WorkspaceAPI(api.store, project_id=api.project_id, actor_id=api.actor_id)
    assert reopened.handle(query("reports.read", report, limit=4000)) == first
    with pytest.raises(Exception):
        api.handle(query("reports.read", "other-report", cursor=first["nextCursor"]))


def test_unpublished_reference_and_inactive_project_refused(tmp_path):
    api, execution, _ = asking(tmp_path)
    report, _ = published(api, execution, expose=False)
    with pytest.raises(WorkspaceError, match="report_unavailable"):
        api.handle(query("reports.read", report))
