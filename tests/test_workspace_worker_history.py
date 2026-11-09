"""Combined helper continuation: independent event clocks, UTF-8 and frozen tails."""
import importlib.util
import json
from pathlib import Path
import sqlite3

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("worker_history_helper", ROOT / "crates/workspace-service/machine_tools.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


@pytest.fixture
def history():
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("""
        CREATE TABLE task_events(id INTEGER PRIMARY KEY,task_id,assignment_ref,event_type,created_at,payload_json);
        CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id,type,actor,created_at,payload_json);
    """)
    def add(n):
        # Timestamp capture may precede lock acquisition, so timestamp order
        # is deliberately not id order, even within a namespace.
        stamp = f"2026-11-06T00:00:{n % 7:02d}+00:00"
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)", (n, "runtime", "attempt", "progress", stamp,
                   json.dumps(dict(kind="worker_activity", text=f"note-{n}" + "界😀" * 300))))
        db.execute("INSERT INTO runtime_events VALUES(?,?,?,?,?,?)", (n * 3, "runtime", "WORKER_GUIDANCE", "owner", stamp,
                   json.dumps(dict(channelId="channel", requestId=f"message-{n}", text=f"guidance-{n}",
                                   attachmentNames=[dict(name=f"file-{n}.txt")]))))
    for n in range(1, 32):
        add(n)
    yield db, add
    db.close()


def page(db, cursor=None, binding="binding", budget=7000):
    return helper.worker_history(db, "runtime", ["attempt"], "channel", "task", "worker", "working", binding, cursor, budget)


def test_one_cursor_covers_both_namespaces_without_gaps_or_repeats(history):
    db, add = history
    current = page(db)
    assert current["activityPartial"] and current["activityCursor"]
    expected = {n * 2 for n in range(1, 32)} | {n * 6 + 1 for n in range(1, 32)}
    add(100)  # A new live tail must not leak into an older page.
    seen, messages = set(), set()
    for _ in range(100):
        assert helper.history_bytes(current) <= 7000
        for item in current["activity"]:
            assert item["sequence"] not in seen
            seen.add(item["sequence"])
        for message in current["messages"]:
            assert message["id"] not in messages
            messages.add(message["id"])
        cursor = current["activityCursor"]
        if not cursor:
            break
        current = page(db, cursor)
    else:
        pytest.fail("Non-progressing history cursor")
    assert seen == expected
    assert messages == {f"message-{n}" for n in range(1, 32)}


def test_many_worker_snapshot_defers_history_without_losing_its_continuation(history):
    db, _ = history
    db.executescript("""
        DELETE FROM task_events;
        DELETE FROM runtime_events;
        CREATE TABLE workspace_executions(task_id,execution_id,runtime_task_id);
        CREATE TABLE work_assignments(assignment_id,task_id,node_key,idempotency_key,created_at,status);
        CREATE TABLE work_assignment_attempts(assignment_id,attempt_id,attempt_no,status);
        CREATE TABLE delegation_provisioning(task_id,idempotency_key,attempt_id,channel_id,created_at);
        INSERT INTO workspace_executions VALUES('task','execution','runtime');
    """)
    number = 0
    for w in range(30):
        db.execute("INSERT INTO work_assignments VALUES(?,?,?,?,?,?)",
                   (f"worker-{w}", "runtime", "worker", f"request-{w}", "now", "running"))
        db.execute("INSERT INTO work_assignment_attempts VALUES(?,?,?,?)",
                   (f"worker-{w}", f"graph-{w}", 1, "running"))
        db.execute("INSERT INTO delegation_provisioning VALUES(?,?,?,?,?)",
                   ("runtime", f"request-{w}", f"engine-{w}", f"channel-{w}", "now"))
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
                   (w + 1, "runtime", f"request-{w}", "delegation_requested", "now",
                    json.dumps(dict(work_attempt_ref=f"graph-{w}", role="worker-backend-dev"))))
        for n in range(50):
            number += 1
            db.execute("INSERT INTO runtime_events VALUES(?,?,?,?,?,?)", (number, "runtime", "WORKER_GUIDANCE", "owner", "now",
                       json.dumps(dict(channelId=f"channel-{w}", requestId=f"guidance-{w}-{n}", text="Content " * 100))))
    rows = helper._workers_of_db(db, "project", ["task"])
    assert len(rows) == 30
    assert helper.history_bytes(dict(ok=True, result=rows)) < 786432
    assert all(helper.history_bytes(w) <= 65536 for w in rows)
    deferred = rows[-1]
    assert deferred["activityPartial"] and deferred["activityCursor"] and not deferred["activity"]
    count, cursor = 0, deferred["activityCursor"]
    while cursor:
        item = helper._workers_of_db(db, "project", ["task"], worker_id=deferred["id"], cursor=cursor)[0]
        count += len(item["messages"])
        cursor = item["activityCursor"]
    assert count == 50


@pytest.mark.parametrize("cursor", ["wa1.binding.1", "wa2.foreign.10.10", "wa2.binding.01.10",
                                  "wa2.binding.0.1", "wa2.binding.1.999999999999999999999"])
def test_foreign_legacy_and_malformed_cursors_fail_closed(history, cursor):
    with pytest.raises(helper.Refused, match="invalid_cursor"):
        page(history[0], cursor)


@pytest.mark.parametrize("text", ["界😀" * 3000, "\\x00\\n" * 4000])
def test_clipping_is_explicit_and_utf8_safe(text):
    clipped = helper.history_text(text)
    assert "\ufffd" not in clipped
    prefix, label = clipped.rsplit("\n", 1)
    assert label == f"[Content clipped: first {len(prefix.encode())} of {len(text.encode())} UTF-8 bytes]"
    assert helper.history_bytes(clipped) <= 8192


def test_real_tool_updates_keep_inputs_and_fold_by_call_identity(history):
    db, _ = history
    db.execute("DELETE FROM task_events")
    payloads = [
        dict(kind="worker_activity", activity_ref="tool:claude-1", tool_kind="execute",
             title="Bash", status="in_progress", raw_input={"command": "printf hello"}),
        dict(kind="worker_activity", activity_ref="tool:claude-1", tool_kind="execute",
             title="Bash", status="completed", raw_input={}, raw_output=[{"type": "text", "text": "hello"}]),
        dict(kind="worker_activity", activity_ref="tool:codex-1", tool_kind="read",
             title="Read", status="completed", raw_input={"input": {"path": "src/main.ts"}}),
        dict(kind="worker_activity", activity_ref="tool:edit-1", tool_kind="edit", title="Edit",
             raw_input={"file_path": "src/main.ts", "old_string": "before", "new_string": "after"}),
    ]
    for n, payload in enumerate(payloads, 100):
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
                   (n, "runtime", "attempt", "progress", "2026-11-08T00:00:00Z", json.dumps(payload)))
    activity = page(db, budget=20000)["activity"]
    calls = [a for a in activity if a.get("toolCallRef")]
    assert len(calls) == 3
    shell = next(a for a in calls if a["toolCallRef"] == "tool:claude-1")
    assert shell["text"] == "printf hello"
    assert shell["command"]["tail"] == "hello"
    assert shell["command"]["state"] == "native"  # no invented native exit code
    read = next(a for a in calls if a["toolCallRef"] == "tool:codex-1")
    assert json.loads(read["args"])["path"] == "src/main.ts"
    edit = next(a for a in calls if a["kind"] == "edit")
    assert edit["diff"]["lines"] == ["-before", "+after"]


def test_long_native_call_keeps_initial_command_and_stable_identity(history):
    db, _ = history
    db.execute("DELETE FROM task_events")
    db.execute("DELETE FROM runtime_events")
    for n in range(1, 206):
        payload = dict(kind="worker_activity", activity_ref="tool:long-call", tool_kind="execute",
                       title="Bash", status="in_progress", raw_input={"command": "printf hello"} if n == 1 else {})
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
                   (n, "runtime", "attempt", "progress", "2026-11-08T00:00:00Z", json.dumps(payload)))
    current = page(db, budget=20000)
    assert current["activityPartial"]
    for _ in range(4):
        assert len(current["activity"]) == 1
        assert current["activity"][0]["sequence"] == 2
        assert current["activity"][0]["text"] == "printf hello"
        if not current["activityCursor"]:
            break
        current = page(db, current["activityCursor"], budget=20000)
    else:
        pytest.fail("Call updates did not advance the history cursor")


def test_gateway_command_has_argv_while_still_running(history):
    db, _ = history
    db.execute("DELETE FROM runtime_events")
    db.execute("INSERT INTO runtime_events VALUES(?,?,?,?,?,?)",
               (1, "runtime", "COMMAND_PROCESS_STARTED", "channel", "2026-11-08T00:00:00Z",
                json.dumps(dict(tool_call_ref="gateway:1", argv=["python3", "-B", "check.py"]))))
    row = db.execute("SELECT * FROM runtime_events").fetchone()
    activity, _ = helper.worker_controls(db, "runtime", "channel", "task", "worker", "working", [row])
    assert activity[0]["text"] == "python3 -B check.py"
    assert activity[0]["command"]["tail"] == ""
    assert activity[0]["command"]["state"] == "running"


def test_selected_details_are_valid_bounded_json_not_an_envelope():
    from cortex_runtime.workspace_api.history import tool_detail
    detail = tool_detail(dict(title="Write", tool_kind="edit",
        raw_input={"path": "index.html", "content": "\\\\" * 40000, "credential": "do-not-project"}))
    assert len(json.dumps(detail, ensure_ascii=False).encode()) <= 18000
    assert "do-not-project" not in str(detail)
    assert "[Content clipped]" in detail["newText"]
    detail = tool_detail(dict(title="Read", tool_kind="execute", raw_input={"path": "notes.md"},
                              raw_output={"text": "private read body"}))
    assert "output" not in detail


def test_worker_routes_use_each_engine_attempt_confirmation_not_graph_id_or_dispatch(history):
    db, _ = history
    db.executescript("""
        DELETE FROM task_events;
        DELETE FROM runtime_events;
        CREATE TABLE workspace_executions(task_id,execution_id,runtime_task_id);
        CREATE TABLE work_assignments(assignment_id,task_id,node_key,idempotency_key,created_at,status);
        CREATE TABLE work_assignment_attempts(assignment_id,attempt_id,attempt_no,status);
        CREATE TABLE delegation_provisioning(task_id,idempotency_key,attempt_id,channel_id,created_at);
        CREATE TABLE attempt_route_audit(task_id,attempt_id,engine,model,decision_detail_json,recorded_at);
        INSERT INTO workspace_executions VALUES('task','execution','runtime');
        INSERT INTO work_assignments VALUES('assignment','runtime','worker','request-1','now','running');
    """)
    for n in (1, 2):
        db.execute("INSERT INTO work_assignment_attempts VALUES(?,?,?,?)",
                   ("assignment", f"graph-{n}", n, "running"))
        db.execute("INSERT INTO delegation_provisioning VALUES(?,?,?,?,?)",
                   ("runtime", f"request-{n}", f"engine-{n}", f"channel-{n}", "now"))
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
                   (n, "runtime", f"request-{n}", "delegation_requested", "now",
                    json.dumps(dict(work_attempt_ref=f"graph-{n}", role="worker-backend-dev"))))
        db.execute("INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?)",
                   ("runtime", f"engine-{n}", "codex-acp", f"model-{n}",
                    json.dumps(dict(model_name=f"Observed Model {n}", effort="xhigh")), "now"))
        db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
                   (10 + n, "runtime", f"request-{n}", "progress", "now",
                    json.dumps(dict(kind="worker_configuration_confirmed", attempt_id=f"engine-{n}",
                        model=f"model-{n}", parameters={"thought_level": "medium" if n == 1 else "high"},
                        confirmation_evidence_ref="native-confirmed"))))
    workers = helper._workers_of_db(db, "project", ["task"])
    assert len(workers) == 2
    assert {w["attempt"]["n"]: w["route"] for w in workers} == {
        1: {"model": "Observed Model 1", "effort": "Medium"},
        2: {"model": "Observed Model 2", "effort": "High"}}
    for worker in workers:
        assert helper._workers_of_db(db, "project", ["task"], worker_id=worker["id"])[0]["route"] == worker["route"]
    db.execute("CREATE TABLE runtime_nodes(task_id,assignment_ref,result_json)")
    db.execute("INSERT INTO runtime_nodes VALUES(?,?,?)", ("runtime", "request-2", json.dumps({"checks": []})))
    db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
        (20, "runtime", "request-2", "worker_result", "now", json.dumps({"summary": "Finished", "checks": []})))
    completed = helper._workers_of_db(db, "project", ["task"], worker_id="assignment")[0]
    assert completed["id"] == "assignment" and completed["result"]["summary"] == "Finished"
    db.execute("ALTER TABLE attempt_route_audit ADD COLUMN predecessor_attempt_id")
    db.execute("INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?)",
        ("runtime", "engine-3", "claude-agent-acp", "model-3",
         json.dumps(dict(model_name="Observed Model 3", effort="xhigh")), "later", "engine-2"))
    db.execute("UPDATE delegation_provisioning SET attempt_id='engine-3' WHERE attempt_id='engine-2'")
    db.execute("INSERT INTO task_events VALUES(?,?,?,?,?,?)",
        (13, "runtime", None, "progress", "later", json.dumps(dict(
            kind="worker_configuration_confirmed", attempt_id="engine-3", model="model-3",
            parameters={"effort": "low"}, confirmation_evidence_ref="native-fallback-confirmed"))))
    fallback = helper._workers_of_db(db, "project", ["task"], worker_id="assignment")[0]
    assert fallback["route"] == {"model": "Observed Model 3", "effort": "Low"}
    assert fallback["routeAttempts"] == [
        {"n": 1, "route": {"model": "Observed Model 2", "effort": "High"}},
        {"n": 2, "route": {"model": "Observed Model 3", "effort": "Low"}}]
    db.execute("DELETE FROM task_events WHERE id=13")
    assert helper._workers_of_db(db, "project", ["task"], worker_id="assignment")[0]["route"] is None


@pytest.mark.parametrize("drop,model,effort", [((), "default", "High"), (("effort",), "haiku", "")])
def test_observed_routes_from_real_configuration_with_fake_engine_processes(tmp_path, drop, model, effort):
    from test_delegation_worker_mode_floor import (
        _bootstrap, _claude_haiku_connection, _claude_coordinator_revision)
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.workspace_api.routes import observed_route
    store, runtime, epoch = _bootstrap(tmp_path)
    environ, connection = _claude_haiku_connection(tmp_path, tmp_path / "requests.jsonl", drop_on_model_switch=drop)
    revision = _claude_coordinator_revision(store, runtime, epoch, connection)
    adapter = EngineSessionManagerAdapter(lambda _: connection, lambda _: str(tmp_path),
        environ=environ, call_timeout=10.0, store=store)
    with store.connection() as db:
        coordinator = observed_route(db, runtime)
        assert coordinator == {"model": "default (alias)", "effort": "Medium"}
    attempt, request = "xa_observed", "request-observed"
    try:
        adapter.open("shared_project_root", dict(task_id=runtime, attempt_id=attempt,
            assignment_id="worker-observed", idempotency_key=request, config_revision=revision["id"],
            resolved_model=model, parameters={"effort": "high"}), "instructions")
        with store.connection() as db:
            route = observed_route(db, runtime, attempt_id=attempt, assignment_ref=request)
            assert route == {"model": model + " (alias)", "effort": effort}
            assert observed_route(db, runtime, attempt_id=attempt, assignment_ref="foreign") is None
    finally:
        adapter.terminate_owned(attempt)

