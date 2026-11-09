"""Bounded durable owner conversation: real persisted rows, no paid engines."""
import pytest

from cortex_runtime.acp.replay_store import append_host_transcript, append_host_update
from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
import json
from test_workspace_api import api_for, create_command, query, runtime_binding


def conversation(tmp_path):
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task)
    return api, task


def test_history_order_snapshot_cursor_reopen_and_binding(tmp_path):
    api, task = conversation(tmp_path)
    append_host_transcript(api.store, "session-1", "user_message", "Owner first")
    append_host_transcript(api.store, "session-1", "agent_message", "Cortex first")
    append_host_update(api.store, "session-1", dict(sessionUpdate="plan", entries=[dict(content="Inspect", status="pending")]))
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call", toolCallId="worker:a", title="Worker started", status="in_progress", rawInput={"secret":"must not escape"}))
    first = api.handle(query("history.page", task, limit=2))
    assert [r["text"] for r in first["items"]] == ["Owner first", "Cortex first"]
    assert first["items"][0]["kind"] == "user"
    append_host_transcript(api.store, "session-1", "agent_message", "Later live update")
    second = api.handle(query("history.page", task, limit=2, cursor=first["nextCursor"]))
    assert [r["text"] for r in second["items"]] == ["Plan\nInspect · pending", "Worker started · in_progress"]
    assert second["nextCursor"] is None
    assert "must not escape" not in str(second)
    reopened = WorkspaceAPI(api.store, project_id=api.project_id, actor_id=api.actor_id)
    assert reopened.handle(query("history.page", task))["items"][-1]["text"] == "Later live update"
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("history.page", task, limit=3, cursor=first["nextCursor"]))
    with pytest.raises(WorkspaceError, match="not_found"):
        api.handle(query("history.page", "other-task", cursor=first["nextCursor"]))


def test_split_native_calls_retain_input_across_history_pages(tmp_path):
    api, task = conversation(tmp_path)
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call", toolCallId="claude:bash",
        title="Bash", kind="execute", status="in_progress", rawInput={"command": "printf hello"}))
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call_update", toolCallId="claude:bash",
        status="completed", rawInput={}, rawOutput=[{"type": "text", "text": "hello"}]))
    first = api.handle(query("history.page", task, limit=1))
    last = api.handle(query("history.page", task, limit=1, cursor=first["nextCursor"]))
    detail = json.loads(last["items"][0]["text"].removeprefix("Workspace step: "))
    assert detail["command"] == "printf hello" and detail["output"] == "hello"
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call", toolCallId="codex:read",
        title="Read", kind="read", status="completed", rawInput={"input": {"path": "index.html"}},
        rawOutput={"text": "READ BODY MUST NOT BE PROJECTED"}))
    newest = api.handle(query("history.page", task))
    assert "READ BODY MUST NOT BE PROJECTED" not in str(newest)
    assert "index.html" in newest["items"][-1]["text"]


@pytest.mark.parametrize("empty", ["", None, {}])
def test_sparse_tool_placeholders_do_not_erase_input(empty):
    from cortex_runtime.workspace_api.history import merge_tool_update, tool_detail
    merged = merge_tool_update({"rawInput": {"command": "printf hello"}},
                               {"rawInput": empty, "status": "completed"})
    assert tool_detail(merged)["command"] == "printf hello"


def test_bookkeeping_input_is_not_projected_as_an_edit():
    from cortex_runtime.workspace_api.history import tool_detail
    for title in ("mcp__cortex__draft_write", "mcp__cortex__write_report", "mcp__cortex__result_submit"):
        detail = tool_detail(dict(title=title, kind="other",
                                  rawInput={"content": "PRIVATE REPORT BODY", "report": "PRIVATE REPORT BODY"}))
        assert "PRIVATE REPORT BODY" not in json.dumps(detail)


def test_worker_guidance_exact_channel_order_and_no_redelivery(tmp_path):
    from cortex_runtime.orchestration.worker_messages import capture, deliver
    from cortex_runtime.trusted_channel import ChannelContext
    api, task = conversation(tmp_path)
    def setup(db):
        db.execute("INSERT INTO trusted_channels VALUES ('ch','unused','runtime-1','thread',NULL,'worker','a','attempt',NULL,NULL,1,'now',NULL,NULL)")
    api.store.immediate_write(setup)
    from orchestration_fakes import channel_context_by_id
    channel = channel_context_by_id(api.store, "ch")
    capture(api.store, "runtime-1", "ch", "one", "First\nunchanged")
    capture(api.store, "runtime-1", "ch", "two", "Second")
    capture(api.store, "runtime-1", "ch", "one", "First\nunchanged")
    with pytest.raises(WorkspaceError, match="payload_conflict"):
        capture(api.store, "runtime-1", "ch", "one", "Changed")
    assert [m["text"] for m in deliver(api.store, channel)] == ["First\nunchanged", "Second"]
    assert deliver(api.store, channel) == []
    capture(api.store, "runtime-1", "ch", "large-one", "😀" * 32000)
    capture(api.store, "runtime-1", "ch", "large-two", "😀" * 32000)
    assert len(deliver(api.store, channel)) == 1
    assert len(deliver(api.store, channel)) == 1
    api.store.immediate_write(lambda db: db.execute("UPDATE trusted_channels SET revoked_at='now' WHERE channel_id='ch'"))
    with pytest.raises(WorkspaceError, match="worker_unavailable"):
        capture(api.store, "runtime-1", "ch", "three", "Too late")


def test_worker_guidance_resources_are_private_and_not_duplicated(tmp_path):
    from cortex_runtime.orchestration.worker_messages import capture, deliver
    from cortex_runtime.acp.replay_store import list_sources, read_source
    from orchestration_fakes import channel_context_by_id
    from test_workspace_attachments import attachment
    from test_workspace_decisions import asking
    api, execution, _ = asking(tmp_path)
    with api.store.connection() as db:
        task = db.execute("SELECT runtime_task_id FROM workspace_executions WHERE execution_id=?", (execution,)).fetchone()[0]
    api.store.immediate_write(lambda db: db.execute(
        "INSERT INTO trusted_channels VALUES ('ch','unused',?,'thread',NULL,'worker','a','attempt',NULL,NULL,1,'now',NULL,NULL)", (task,)))
    files = [attachment()]
    capture(api.store, task, "ch", "file", "Use the file", attachments=files)
    capture(api.store, task, "ch", "file", "Use the file", attachments=files)
    with pytest.raises(WorkspaceError, match="payload_conflict"):
        capture(api.store, task, "ch", "file", "Use the file", attachments=[attachment(b"changed")])
    items = deliver(api.store, channel_context_by_id(api.store, "ch"))
    assert len(items) == 1 and len(items[0]["attachments"]) == 1
    source = items[0]["attachments"][0]
    assert source["availability"] == "stored"
    assert read_source(api.store, task, source["source_ref"])["chunk"] == "fixture resource"
    assert deliver(api.store, channel_context_by_id(api.store, "ch")) == []
    assert len([s for s in list_sources(api.store, task)["entries"] if s["kind"] == "resource"]) == 1


def test_history_byte_budget_and_missing_content_are_explicit(tmp_path):
    api, task = conversation(tmp_path)
    for _ in range(5):
        append_host_transcript(api.store, "session-1", "agent_message", "😀" * 10000)
    append_host_transcript(api.store, "session-1", "agent_message", "x" * 32001)
    rows, cursor = [], None
    while True:
        page = api.handle(query("history.page", task, cursor=cursor))
        rows.extend(page["items"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
    assert len(rows) == 6
    assert len({r["historyId"] for r in rows}) == 6
    assert all(r["availability"] == "available" for r in rows[:5])
    assert all("[Content clipped:" in r["activity"]["text"] for r in rows[:5])
    assert rows[-1]["availability"] == "unavailable"
    assert rows[-1]["text"] is None


@pytest.mark.parametrize("state", ["EXECUTING", "COMPLETED"])
def test_coordinator_activity_is_durable_paged_and_shared_with_helper(tmp_path, state):
    import importlib.util
    from pathlib import Path
    from cortex_runtime.store import Store
    from cortex_runtime.workspace_api import WorkspaceAPI
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task, state)
    append_host_transcript(api.store, "session-1", "user_message", "Please inspect")
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call",
        toolCallId="bookkeeping", title="mcp__cortex__result_submit", kind="other",
        rawInput={"report": "PRIVATE REPORT"}))
    append_host_transcript(api.store, "session-1", "agent_message", "Inspecting the source")
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call",
        toolCallId="shell", title="Bash", kind="execute", rawInput={"command": "printf hello"}))
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call_update",
        toolCallId="shell", status="completed", rawOutput=[{"type": "text", "text": "hello"}]))
    append_host_update(api.store, "session-1", dict(sessionUpdate="config_option_update", configOptions=[]))
    page = api.handle(query("history.page", task, limit=2))
    append_host_transcript(api.store, "session-1", "agent_message", "New tail")
    items = list(page["items"])
    while page["nextCursor"]:
        page = api.handle(query("history.page", task, limit=2, cursor=page["nextCursor"]))
        items.extend(page["items"])
    assert [i["kind"] for i in items] == ["user", "assistant", "tool", "tool"]
    assert "PRIVATE REPORT" not in str(items) and "result_submit" not in str(items)
    assert items[0]["activity"]["kind"] == "message"
    assert items[1]["activity"]["text"] == "Inspecting the source"
    assert items[-1]["activity"]["command"]["tail"] == "hello"
    assert items[-1]["activity"]["text"] == "printf hello"
    reopened = WorkspaceAPI(Store(tmp_path / ".cortex", project_root=tmp_path),
                            project_id=api.project_id, actor_id=api.actor_id)
    assert reopened.handle(query("history.page", task))["items"][-1]["text"] == "New tail"
    spec = importlib.util.spec_from_file_location("coordinator_helper",
        Path(__file__).resolve().parents[1] / "crates/workspace-service/machine_tools.py")
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    worker = helper.workers_of(tmp_path, api.project_id, [task], worker_id="coordinator:" + task)[0]
    assert worker["role"] == "coordinator" and worker["route"] is None
    assert worker["status"] == ("working" if state == "EXECUTING" else "completed")
    assert [a["text"] for a in worker["activity"]][-1] == "New tail"
    assert any(a.get("command", {}).get("tail") == "hello" for a in worker["activity"])


def test_task_route_is_absent_until_confirmed_and_never_a_selected_default(tmp_path):
    api, task = conversation(tmp_path)
    assert api.handle(query("tasks.get", task))["coordinatorRoute"] is None
    def record(db):
        db.execute("INSERT INTO configuration_revisions VALUES "
            "('cfg','runtime-1',1,'internal-route','codex-acp','internal-connection',"
            "'{\"thought_level\":\"high\"}','{}','active','observed-confirmation',NULL,'now','now')")
        db.execute("INSERT INTO configuration_route_extras VALUES "
            "('cfg','runtime-1','gpt-observed','{}','{}','now')")
    api.store.immediate_write(record)
    route = api.handle(query("tasks.get", task))["coordinatorRoute"]
    assert route == {"model": "gpt-observed (alias)", "effort": "High"}
    assert "internal-" not in str(route)
    assert api.handle(query("tasks.list"))["items"][0]["coordinatorRoute"] == route
    def audit(db):
        db.execute("INSERT INTO task_orchestration(task_id,state,coordinator_attempt_ref,updated_at) "
                   "VALUES('runtime-1','running','coordinator-current','now')")
        for number, attempt in enumerate(("coordinator-current", "unrelated-attempt")):
            db.execute("INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (f"audit-{number}", attempt, "runtime-1", "coordinator", "codex-acp",
                 "internal-connection", "gpt-observed", "fingerprint", "cfg", "authorized",
                 json.dumps({"model_name": "Observed Coordinator" if number == 0 else "Wrong attempt"}),
                 None, f"stamp-{number}"))
    api.store.immediate_write(audit)
    assert api.handle(query("tasks.get", task))["coordinatorRoute"] == {
        "model": "Observed Coordinator", "effort": "High"}
    api.store.immediate_write(lambda db: db.execute(
        "UPDATE task_orchestration SET coordinator_attempt_ref='not-observed-yet' WHERE task_id='runtime-1'"))
    assert api.handle(query("tasks.get", task))["coordinatorRoute"] is None
    api.store.immediate_write(lambda db: db.execute(
        "UPDATE task_orchestration SET coordinator_attempt_ref='coordinator-current' WHERE task_id='runtime-1'"))
    api.store.immediate_write(lambda db: db.execute(
        "INSERT INTO configuration_revisions VALUES "
        "('pending','runtime-1',2,'new-route','codex-acp','internal-connection','{}','{}',"
        "'indeterminate',NULL,NULL,'now','now')"))
    assert api.handle(query("tasks.get", task))["coordinatorRoute"] is None


def test_hidden_only_history_pages_advance_and_sparse_updates_stay_hidden(tmp_path):
    api, task = conversation(tmp_path)
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call",
        toolCallId="bookkeeping", title="mcp__cortex__result_submit", kind="other"))
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call_update",
        toolCallId="bookkeeping", status="completed"))
    append_host_transcript(api.store, "session-1", "agent_message", "Visible reply")
    cursor, seen = None, []
    for _ in range(3):
        page = api.handle(query("history.page", task, limit=1, cursor=cursor))
        seen.extend(page["items"])
        cursor = page["nextCursor"]
    assert cursor is None and [i["text"] for i in seen] == ["Visible reply"]
