"""Transactional public invalidations; no engine, network or model calls."""
import ast
import json
import re
from pathlib import Path
import sqlite3

import pytest

from cortex_runtime.store import Store
from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
from cortex_runtime.workspace_api import projections, storage
from cortex_runtime.workspace_contract import validate
from test_workspace_api import api_for, create_command, query, runtime_binding


def events(api, cursor=None, **kwargs):
    return api.handle(query("events.page", cursor=cursor, **kwargs))


def bound(api):
    task = api.handle(create_command())["target"]
    execution = runtime_binding(api, task)
    return task, execution


@pytest.mark.parametrize("missing_workspace", [None, "workspace_executions", "all"])
def test_other_sqlite_writers_do_not_depend_on_workspace(tmp_path, missing_workspace):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with sqlite3.connect(store.path) as raw:
        if missing_workspace == "all":
            for row in raw.execute("SELECT name FROM sqlite_master WHERE type='table' "
                                   "AND name LIKE 'workspace_%'").fetchall():
                raw.execute('DROP TABLE "' + row[0] + '"')
        elif missing_workspace:
            raw.execute("DROP TABLE " + missing_workspace)
        raw.execute("INSERT INTO tasks VALUES ('external',?,'now')", (str(tmp_path),))
    with store.connection() as db:
        db.execute("UPDATE tasks SET created_at='later' WHERE id='external'")
        assert db.execute("SELECT created_at FROM tasks WHERE id='external'").fetchone()[0] == "later"


def test_legacy_hooks_inspection_is_immutable_and_writable_open_retires_them(tmp_path):
    api = api_for(tmp_path)
    bound(api)
    with sqlite3.connect(api.store.path) as raw:
        for statement in storage.outbox_trigger_statements():
            raw.execute(statement.replace("CREATE TEMP TRIGGER", "CREATE TRIGGER"))
        raw.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = api.store.path.read_bytes()
    inspected = Store(api.store.directory, project_root=tmp_path, initialize=False)
    with inspected.connection(read_only=True) as db:
        assert len(db.execute("SELECT name FROM sqlite_master WHERE type='trigger' "
                              "AND name LIKE 'workspace_outbox_%'").fetchall()) == 45
        assert not db.execute("SELECT name FROM sqlite_temp_master WHERE type='trigger'").fetchall()
        assert db.total_changes == 0
    assert api.store.path.read_bytes() == before
    # A live non-initializing access must not duplicate old and TEMP callbacks.
    live = Store(api.store.directory, project_root=tmp_path, initialize=False, runtime_access=True)
    with live.connection() as db:
        high = storage.outbox_position(db, api.project_id)[0]
        db.execute("UPDATE runtime_tasks SET state='FAILED'")
        assert storage.outbox_position(db, api.project_id)[0] == high + 2
    repaired = Store(api.store.directory, project_root=tmp_path)
    with repaired.connection() as db:
        assert not db.execute("SELECT name FROM sqlite_master WHERE type='trigger' "
                              "AND name LIKE 'workspace_outbox_%'").fetchall()
        assert len(db.execute("SELECT name FROM sqlite_temp_master WHERE type='trigger'").fetchall()) == 45
        high = storage.outbox_position(db, api.project_id)[0]
        db.execute("UPDATE runtime_tasks SET state='CANCELLED'")
        assert storage.outbox_position(db, api.project_id)[0] == high + 2
    # The writable repair also restores independent/older writers.
    with sqlite3.connect(repaired.path) as raw:
        raw.execute("UPDATE tasks SET created_at='later'")


def test_inspection_with_hooks_does_not_change_main_database(tmp_path):
    api = api_for(tmp_path)
    bound(api)
    with api.store.connection() as db:
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = api.store.path.read_bytes()
    inspected = Store(api.store.directory, project_root=tmp_path, initialize=False)
    with inspected.connection(read_only=True) as db:
        storage.require_schema(db)
        assert db.total_changes == 0
        assert not db.execute("SELECT name FROM sqlite_master WHERE type='trigger' "
                              "AND name LIKE 'workspace_outbox_%'").fetchall()
        assert len(db.execute("SELECT name FROM sqlite_temp_master WHERE type='trigger'").fetchall()) == 45
    assert api.store.path.read_bytes() == before


def test_draft_and_receipt_are_atomic_public_invalidations(tmp_path):
    api = api_for(tmp_path)
    before = api.handle(query("project.snapshot"))
    receipt = api.handle(create_command(description="PRIVATE prompt\nPRIVATE transcript"))
    page = events(api, before["eventCursor"])
    assert page["items"]
    assert any(e["entityId"] == receipt["target"] and e["kind"] == "task" for e in page["items"])
    assert "PRIVATE" not in json.dumps(page)
    assert api.handle(create_command(description="PRIVATE prompt\nPRIVATE transcript")) == receipt
    assert events(api)["highWatermark"] == page["highWatermark"]
    assert "events.page" in api.handle(query("capabilities.get"))["operations"]


@pytest.mark.parametrize("mutation", ["runtime", "node", "delegation", "orchestration", "question", "history", "adapter"])
def test_append_failure_rolls_back_original_write_and_versions(tmp_path, monkeypatch, mutation):
    api = api_for(tmp_path)
    task, execution = bound(api)
    before = api.handle(query("project.snapshot"))
    before_execution = api.handle(query("executions.get", execution))
    checks = {
        "runtime": "SELECT count(*) FROM runtime_tasks WHERE state='FAILED'",
        "node": "SELECT count(*) FROM runtime_nodes",
        "delegation": "SELECT count(*) FROM task_events",
        "orchestration": "SELECT count(*) FROM task_orchestration",
        "question": "SELECT count(*) FROM pending_questions",
        "history": "SELECT count(*) FROM host_transcript",
        "adapter": "SELECT count(*) FROM workspace_tasks",
    }
    def facts(db):
        return tuple(tuple(row) for table in (
            "runtime_tasks", "runtime_nodes", "task_events", "task_orchestration",
            "pending_questions", "host_transcript", "workspace_tasks", "workspace_requests",
            "workspace_outbox_state", "workspace_outbox")
            for row in db.execute("SELECT * FROM " + table))
    with api.store.connection() as db:
        before_facts = facts(db)
    seen = []
    def fail(db, **kwargs):
        # We are inside the original writer's transaction, AFTER the state write.
        seen.append(db.in_transaction)
        assert db.execute(checks[mutation]).fetchone()[0] == (2 if mutation == "adapter" else 1)
        raise RuntimeError("injected public append failure")
    monkeypatch.setattr(storage, "append_public_tx", fail)
    statements = {
        "runtime": ("UPDATE runtime_tasks SET state='FAILED' WHERE task_id='runtime-1'", ()),
        "node": ("INSERT INTO runtime_nodes VALUES ('runtime-1',1,'n','backend','private','[]','[]','execution','running',NULL,NULL)", ()),
        "delegation": ("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at) VALUES('runtime-1','progress','key','{}','runtime','now')", ()),
        "orchestration": ("INSERT INTO task_orchestration(task_id,state,updated_at) VALUES('runtime-1','running','now')", ()),
        "question": ("INSERT INTO pending_questions(id,task_id,dedup_key,text,created_at) VALUES('q','runtime-1','q','private','now')", ()),
        "history": ("INSERT INTO host_transcript(session_id,seq,kind,text,created_at) VALUES('session-1',1,'agent_message','private','now')", ()),
    }
    with pytest.raises((RuntimeError, sqlite3.Error)):
        if mutation == "adapter":
            api.handle(create_command("another"))
        else:
            api.store.immediate_write(lambda db: db.execute(*statements[mutation]))
    assert seen and all(seen)
    assert api.handle(query("project.snapshot")) == before
    assert api.handle(query("executions.get", execution)) == before_execution
    with api.store.connection() as db:
        assert facts(db) == before_facts


def test_runtime_changes_advance_versions_and_sequence_not_private_ids(tmp_path):
    api = api_for(tmp_path)
    task, execution = bound(api)
    before = api.handle(query("project.snapshot"))
    def change(db):
        db.execute("INSERT INTO runtime_events(id,task_id,type,actor,payload_json,created_at) "
                   "VALUES(900000,'runtime-1','test','runtime','{\"text\":\"PRIVATE\"}','now')")
        db.execute("UPDATE runtime_tasks SET state='FAILED' WHERE task_id='runtime-1'")
    api.store.immediate_write(change)
    page = events(api, before["eventCursor"])
    seqs = [int(e["sequence"]) for e in page["items"]]
    assert seqs == list(range(int(before["highWatermark"]) + 1, int(page["highWatermark"]) + 1))
    assert max(seqs) < 900000
    assert int(api.handle(query("tasks.get", task))["version"]) > int(before["tasks"]["items"][0]["version"])
    assert int(api.handle(query("executions.get", execution))["version"]) > 1
    for event in page["items"]:
        validate("Event", event)
        assert event["projectId"] == api.project_id
        assert event["schemaVersion"] == "1.0" and event["mappingVersion"] == "1"
        assert len(event["payloadDigest"]) == 64
    assert "PRIVATE" not in json.dumps(page)


def test_snapshot_and_typed_reads_share_read_transaction(tmp_path, monkeypatch):
    api = api_for(tmp_path)
    task, execution = bound(api)
    before = api.handle(query("project.snapshot"))
    original = projections.task_page
    def interleave(db, *args, **kwargs):
        assert db.in_transaction
        api.store.immediate_write(lambda writer: writer.execute(
            "UPDATE runtime_tasks SET state='FAILED' WHERE task_id='runtime-1'"))
        return original(db, *args, **kwargs)
    monkeypatch.setattr(projections, "task_page", interleave)
    assert api.handle(query("project.snapshot")) == before
    monkeypatch.setattr(projections, "task_page", original)
    after = api.handle(query("project.snapshot"))
    assert after["tasks"]["items"][0]["phase"] == "failed"
    assert int(after["highWatermark"]) > int(before["highWatermark"])


def test_500_summaries_page_with_one_stable_high_watermark(tmp_path):
    api = api_for(tmp_path)
    def populate(db):
        for i in range(500):
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES(?,?,?,?)",
                       (f"t-{i:04}", api.project_id, "description", "now"))
            storage.append_public_tx(db, project_id=api.project_id, entity_id=f"t-{i:04}", kind="task", version=1)
    api.store.immediate_write(populate)
    snapshot = api.handle(query("project.snapshot"))
    cursor, seen, watermarks = None, [], set()
    while True:
        page = api.handle(query("tasks.list", limit=50, cursor=cursor))
        assert len(page["items"]) <= 50
        assert len(json.dumps(page).encode()) <= 65536
        seen.extend(t["taskId"] for t in page["items"])
        watermarks.add(page["highWatermark"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
    assert len(seen) == len(set(seen)) == 500
    assert watermarks == {snapshot["highWatermark"]}


def test_retention_floor_paging_and_reopen_never_reset_sequence(tmp_path):
    api = api_for(tmp_path)
    first = api.handle(query("project.snapshot"))["eventCursor"]
    def populate(db):
        for i in range(10005):
            storage.append_public_tx(db, project_id=api.project_id, entity_id="task", kind="task", version=i + 1)
    api.store.immediate_write(populate)
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_outbox").fetchone()[0] == 10000
        state = db.execute("SELECT * FROM workspace_outbox_state WHERE project_id=?", (api.project_id,)).fetchone()
        floor = state["retained_floor"]
        assert floor > 0
    before = api.handle(query("project.snapshot"))
    with api.store.connection() as db:
        db.execute("CREATE TRIGGER inject_prune_failure BEFORE DELETE ON workspace_outbox "
                   "BEGIN SELECT RAISE(ABORT,'injected prune failure'); END")
    with pytest.raises(sqlite3.IntegrityError, match="injected prune failure"):
        api.handle(create_command())
    assert api.handle(query("project.snapshot")) == before
    with api.store.connection() as db:
        assert db.execute("SELECT retained_floor FROM workspace_outbox_state").fetchone()[0] == floor
        assert db.execute("SELECT count(*) FROM workspace_outbox").fetchone()[0] == 10000
        db.execute("DROP TRIGGER inject_prune_failure")
    with pytest.raises(WorkspaceError, match="resync_required"):
        events(api, first)
    cursor = storage.event_cursor(api.project_id, floor)
    seen = []
    while True:
        page = events(api, cursor, limit=100)
        assert 0 < len(page["items"]) <= 100
        assert len(json.dumps(page).encode()) <= 65536
        seen.extend(int(e["sequence"]) for e in page["items"])
        if page["nextCursor"] is None:
            break
        cursor = page["nextCursor"]
    assert seen == list(range(floor + 1, int(page["highWatermark"]) + 1))
    reopened = WorkspaceAPI(Store(tmp_path / ".cortex", project_root=tmp_path),
                            project_id=api.project_id, actor_id=api.actor_id)
    assert events(reopened, storage.event_cursor(api.project_id, seen[-1]))["items"] == []
    reopened.handle(create_command())
    assert int(events(reopened, storage.event_cursor(api.project_id, seen[-1]))["highWatermark"]) > seen[-1]


def test_event_gaps_future_and_wrong_project_cursor_fail_closed(tmp_path):
    api = api_for(tmp_path)
    start = api.handle(query("project.snapshot"))["eventCursor"]
    api.handle(create_command())
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        events(api, storage.event_cursor("other-project", 0))
    with pytest.raises(WorkspaceError, match="resync_required"):
        events(api, storage.event_cursor(api.project_id, 900000))
    api.store.immediate_write(lambda db: db.execute("DELETE FROM workspace_outbox WHERE sequence="
                                                    "(SELECT max(sequence) FROM workspace_outbox)"))
    with pytest.raises(WorkspaceError, match="resync_required"):
        events(api, start)


def test_runtime_mutation_expires_summary_page_cursor(tmp_path):
    api = api_for(tmp_path)
    bound(api)
    api.handle(create_command("second"))
    page = api.handle(query("tasks.list", limit=1))
    api.store.immediate_write(lambda db: db.execute("UPDATE runtime_tasks SET state='FAILED'"))
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("tasks.list", limit=1, cursor=page["nextCursor"]))


def test_inspection_open_does_not_install_outbox(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        for row in db.execute("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'").fetchall():
            db.execute('DROP TRIGGER "' + row[0] + '"')
        db.execute("DROP TABLE workspace_outbox")
        db.execute("DROP TABLE workspace_outbox_state")
    Store(tmp_path / ".cortex", project_root=tmp_path, initialize=False)
    with store.connection() as db:
        assert not db.execute("SELECT 1 FROM sqlite_master WHERE name='workspace_outbox'").fetchone()
    Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='workspace_outbox'").fetchone()


def test_reconnect_duplicate_gap_and_backwards_versions():
    reconnect = projections.ReconnectState("p", "5", {"task": "3"})
    event = dict(eventId="e6", projectId="p", sequence="6", entityId="task", entityVersion="4", kind="task", recordedAt="now")
    assert reconnect.accept(event) is True
    assert reconnect.accept(event) is False
    assert reconnect.sequence == 6 and reconnect.versions["task"] == 4
    with pytest.raises(WorkspaceError, match="resync_required"):
        reconnect.accept(dict(event, eventId="e8", sequence="8"))
    assert reconnect.sequence == 6
    with pytest.raises(WorkspaceError, match="resync_required"):
        reconnect.accept(dict(event, eventId="e7", sequence="7", entityVersion="2"))
    assert reconnect.sequence == 6


def test_event_page_count_limit_is_validated(tmp_path):
    api = api_for(tmp_path)
    for limit in (0, 101, True):
        with pytest.raises(ValueError):
            events(api, limit=limit)


def test_every_inventory_table_has_transactional_hooks(tmp_path):
    api = api_for(tmp_path)
    assert {"runtime_tasks", "runtime_nodes", "runtime_events", "task_events", "task_orchestration",
            "pending_questions", "pending_question_late_answers", "host_transcript", "host_updates"} <= set(storage.OUTBOX_WRITERS)
    with api.store.connection() as db:
        for table in storage.OUTBOX_WRITERS:
            for action in ("insert", "update", "delete"):
                row = db.execute("SELECT sql FROM sqlite_temp_master WHERE type='trigger' AND name=?",
                                 (f"workspace_outbox_{table}_{action}",)).fetchone()
                assert row is not None, (table, action)
                assert "cortex_workspace_invalidate" in row[0]
    # The hook is on the table, not a list of callers: even a new direct writer
    # must append; a removed hook is detected above, not excused as a new path.
    task, _ = bound(api)
    before = events(api)["highWatermark"]
    api.store.immediate_write(lambda db: db.execute("UPDATE runtime_tasks SET synthesis='private'"))
    assert int(events(api)["highWatermark"]) > int(before)


# The concrete adapter writers are deliberately explicit. Private writers are
# covered for ANY caller by the table hooks checked above, not just this scan.
ADAPTER_WRITERS = {
    # Owner pause and resume invalidate the execution and task in the same write.
    ("orchestration/task_pause.py", "apply.write"): "append_public_tx",
    ("workspace_api/__init__.py", "WorkspaceAPI.activate.write"): "record",
    ("workspace_api/__init__.py", "WorkspaceAPI._command"): "append_public_tx",
    ("workspace_api/__init__.py", "WorkspaceAPI._start.prepare"): "start_state_tx",
    # Owner rebinding appends execution and task invalidations in the same write.
    ("workspace_api/control.py", "observe_owner.write"): "append_public_tx",
    # Deletion invalidates the removed task after advancing its project version.
    ("workspace_api/purge.py", "remove_adapter_task"): "append_public_tx",
    ("workspace_api/storage.py", "start_state_tx"): "append_public_tx",
    ("workspace_api/storage.py", "record"): "append_public_tx",
    ("workspace_api/storage.py", "bind_execution_tx"): "append_public_tx",
    ("workspace_api/storage.py", "register_outbox_function.invalidate"): "append_public_tx",
}
NON_SLICE_TABLES = {
    "workspace_task_options": "Service checkout/settings metadata, not read by local slice projections; task create/delete invalidate separately.",
    "runtime_requests": "Private request ledger; observable question/domain writes have their own hooks.",
    "workspace_snapshots": "Git candidate/integration feature, not the local read-only slice.",
    "workspace_snapshot_lineage": "Git candidate/integration feature.",
    "workspace_candidate_seeds": "Git candidate/integration feature.",
    "workspace_integrations": "Git integration feature.",
    "workspace_integration_journal": "Git integration feature.",
    "workspace_outbox": "Owned only by append_public_tx (or verified split copy).",
    "workspace_outbox_state": "Owned only by append_public_tx (or verified split copy).",
}


def writer_inventory(source):
    """Inventory SQL literals at their exact lexical function, including f-strings."""
    class Writers(ast.NodeVisitor):
        def __init__(self):
            self.stack, self.rows = [], []
        def visit_ClassDef(self, node):
            self.stack.append(node.name)
            self.generic_visit(node)
            self.stack.pop()
        def visit_FunctionDef(self, node):
            self.stack.append(node.name)
            # Record called helpers without treating a nested function as this
            # function's direct helper coverage.
            direct = []
            def walk(value):
                if value is not node and isinstance(value, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    return
                direct.append(value)
                for child in ast.iter_child_nodes(value):
                    walk(child)
            walk(node)
            helpers = {v.func.id if isinstance(v.func, ast.Name) else v.func.attr
                       for v in direct if isinstance(v, ast.Call) and isinstance(v.func, (ast.Name, ast.Attribute))}
            tables = set()
            for value in direct:
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    tables.update(m[1].lower() for m in re.finditer(
                        r"(?i)\b(?:INSERT\s+(?:OR\s+\w+\s+)?INTO|UPDATE|DELETE\s+FROM)\s+([a-z_]+)",
                        value.value))
            self.rows.append((".".join(self.stack), tables, helpers))
            self.generic_visit(node)
            self.stack.pop()
    visitor = Writers()
    visitor.visit(ast.parse(source))
    return visitor.rows


def assert_writer_coverage(path, rows):
    for function, tables, helpers in rows:
        for table in tables:
            if table in storage.OUTBOX_WRITERS:
                continue  # all callers share the table's transactional hooks
            if table in storage.WORKSPACE_API_TABLES | storage.WORKSPACE_START_TABLES:
                expected = ADAPTER_WRITERS.get((path, function))
                assert expected and expected in helpers, (path, function, table)
            elif table.startswith(("runtime_", "pending_question", "host_", "workspace_")):
                assert table in NON_SLICE_TABLES, (path, function, table)


def test_source_writer_inventory_and_uncovered_writer_regression():
    root = Path(__file__).resolve().parents[1] / "src/cortex_runtime"
    found = set()
    for path in root.rglob("*.py"):
        relative = path.relative_to(root).as_posix()
        # Split/migrate copy verified stored history without manufacturing new
        # events; the split regression below verifies exact sequence/floor copy.
        if relative in {"split.py", "migrate.py"}:
            continue
        rows = writer_inventory(path.read_text())
        assert_writer_coverage(relative, rows)
        found.update((relative, function) for function, tables, _ in rows
                     if tables & (storage.WORKSPACE_API_TABLES | storage.WORKSPACE_START_TABLES))
    assert found == set(ADAPTER_WRITERS)
    with pytest.raises(AssertionError):
        assert_writer_coverage("new_writer.py", writer_inventory(
            "def uncovered(db):\n    db.execute('INSERT INTO workspace_tasks VALUES (?)')"))
    with pytest.raises(AssertionError):
        assert_writer_coverage("orchestration/state.py", writer_inventory(
            "def uncovered(db):\n    db.execute('UPDATE runtime_new_public_fact SET value=1')"))


def test_owner_pause_and_resume_invalidate_execution_and_task_atomically(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import task_pause
    from test_workspace_decisions import asking
    api, execution, _ = asking(tmp_path)
    task = api.handle(query("tasks.list"))["items"][0]["taskId"]
    def transition(action):
        cursor = api.handle(query("project.snapshot"))["eventCursor"]
        version = api.handle(query("tasks.get", task))["version"]
        assert task_pause.apply(api, execution, action, action, version)["changed"]
        return {(e["kind"], e["entityId"]) for e in events(api, cursor)["items"]}
    expected = {("execution", execution), ("task", task)}
    assert expected <= transition("pause")
    assert expected <= transition("resume")
    state = api.handle(query("executions.get", execution))["state"]
    def refused(*_args, **_kwargs):
        raise RuntimeError("outbox failure")
    monkeypatch.setattr(task_pause, "append_public_tx", refused)
    version = api.handle(query("tasks.get", task))["version"]
    with pytest.raises(RuntimeError, match="outbox failure"):
        task_pause.apply(api, execution, "pause", "failed-pause", version)
    assert api.handle(query("tasks.get", task))["version"] == version
    assert api.handle(query("executions.get", execution))["state"] == state


def test_real_question_history_and_event_writers(tmp_path):
    from cortex_runtime.acp import questions_store, replay_store
    from cortex_runtime.delegation import events as delegation
    from cortex_runtime.orchestration import question_batches
    from cortex_runtime.orchestration.state import event
    api = api_for(tmp_path)
    bound(api)
    def observed(call):
        before = events(api)["highWatermark"]
        result = call()
        assert int(events(api)["highWatermark"]) > int(before)
        assert "PRIVATE" not in json.dumps(events(api))
        return result
    q = observed(lambda: questions_store.create_if_absent(api.store, "runtime-1", "single", "PRIVATE question"))
    observed(lambda: questions_store.answer(api.store, q.id, "PRIVATE answer", "text"))
    q = observed(lambda: questions_store.create_if_absent(api.store, "runtime-1", "cancel", "PRIVATE", assignment_ref="worker"))
    observed(lambda: questions_store.cancel_for_assignment(api.store, "runtime-1", "worker", "PRIVATE"))
    observed(lambda: questions_store.create_if_absent(api.store, "runtime-1", "supersede", "PRIVATE"))
    observed(lambda: questions_store.supersede_open(api.store, "runtime-1", "PRIVATE"))
    observed(lambda: questions_store.record_late_answer(api.store, q.id, "runtime-1", "PRIVATE", "text"))
    observed(lambda: replay_store.append_host_transcript(api.store, "session-1", "agent_message", "PRIVATE"))
    observed(lambda: replay_store.append_host_update(api.store, "session-1", {"sessionUpdate": "agent_message_chunk", "text": "PRIVATE"}))
    observed(lambda: delegation.append_event(api.store, "runtime-1", "progress", "progress-1", {"text": "PRIVATE"}, "runtime"))
    def create_batch(db):
        db.execute("UPDATE runtime_tasks SET state='WAITING_USER'")
        batch = question_batches.create_tx(db, "runtime-1", "channel", "batch", [
            {"question": "PRIVATE one", "options": ["A", "B"]}, {"question": "PRIVATE two", "options": ["C", "D"]}])
        event(db, "runtime-1", "USER_QUESTION_REQUESTED", batch)
        return batch
    batch = observed(lambda: api.store.immediate_write(create_batch))
    answers = [dict(questionId=q["question_id"], answer="PRIVATE answer") for q in batch["questions"]]
    observed(lambda: question_batches.answer(api.store, "runtime-1", batch["batch_id"], answers,
                                            "answer", session_id="session-1", owner_epoch=1))
    high = events(api)["highWatermark"]
    assert question_batches.answer(api.store, "runtime-1", batch["batch_id"], answers,
                                   "answer", session_id="session-1", owner_epoch=1)["replayed"]
    assert events(api)["highWatermark"] == high


def test_autocommit_and_missing_function_fail_atomically(tmp_path, monkeypatch):
    api = api_for(tmp_path)
    bound(api)
    with api.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='FAILED'")
    before = api.handle(query("project.snapshot"))
    def fail(*args, **kwargs):
        raise RuntimeError("append unavailable")
    monkeypatch.setattr(storage, "append_public_tx", fail)
    with api.store.connection() as db:
        with pytest.raises(sqlite3.OperationalError):
            db.execute("UPDATE runtime_tasks SET state='CANCELLED'")
    assert api.handle(query("project.snapshot")) == before
    # A broken runtime connection still fails closed: hooks and callback are
    # connection-local, not a dependency imposed on unrelated/older writers.
    with api.store.connection() as db:
        db.create_function("cortex_workspace_invalidate", 2, None)
        with pytest.raises(sqlite3.OperationalError):
            db.execute("UPDATE runtime_tasks SET state='CANCELLED'")
    assert api.handle(query("project.snapshot")) == before
    assert api.handle(query("executions.get", before["tasks"]["items"][0]["executionId"]))["state"] == "failed"


def test_bound_history_delete_invalidates_and_no_hook_is_refused(tmp_path):
    from cortex_runtime.acp.replay_store import append_host_update
    api = api_for(tmp_path)
    bound(api)
    append_host_update(api.store, "session-1", {"sessionUpdate": "plan"})
    before = events(api)["highWatermark"]
    api.store.immediate_write(lambda db: db.execute("DELETE FROM host_updates"))
    assert int(events(api)["highWatermark"]) > int(before)
    with api.store.connection() as db:
        db.execute("DROP TRIGGER temp.workspace_outbox_runtime_tasks_update")
        with pytest.raises(WorkspaceError, match="workspace_storage_unavailable"):
            storage.require_schema(db)
    # Opening another connection must reinstall its complete private hooks.
    api.handle(query("project.snapshot"))


def test_event_page_byte_budget_and_project_sequence_domains(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    project = "п" * 128
    api = WorkspaceAPI(store, project_id=project, actor_id="owner")
    api.activate(request_id="activate")
    def populate(db):
        db.execute("INSERT INTO workspace_projects VALUES ('other','other-root','owner','now',1)")
        for i in range(200):
            storage.append_public_tx(db, project_id=project, entity_id="я" * 128, kind="task", version=i + 1)
        storage.append_public_tx(db, project_id="other", entity_id="other-task", kind="task", version=1)
    store.immediate_write(populate)
    with store.connection() as db:
        assert storage.outbox_position(db, "other") == (1, 0)
    cursor, sequences = None, []
    while True:
        page = api.handle(dict(query("events.page", limit=100, cursor=cursor), projectId=project))
        assert len(storage.encode(page).encode("utf-8")) <= 65536
        assert len(page["items"]) < 100
        sequences.extend(int(e["sequence"]) for e in page["items"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
    assert sequences == list(range(1, 202))
    with pytest.raises(WorkspaceError, match="invalid_request"):
        api.handle(dict(query("events.page", target="entity-filter"), projectId=project))


def test_concurrent_creates_and_duplicate_deliveries_have_no_sequence_gaps(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    api = api_for(tmp_path)
    start = api.handle(query("project.snapshot"))
    with ThreadPoolExecutor(max_workers=6) as pool:
        receipts = list(pool.map(lambda i: api.handle(create_command(f"parallel-{i}")), range(18)))
        replays = list(pool.map(lambda _: api.handle(create_command("parallel-0")), range(6)))
    assert all(r == receipts[0] for r in replays)
    page = events(api, start["eventCursor"])
    seqs = [int(e["sequence"]) for e in page["items"]]
    assert seqs == list(range(int(start["highWatermark"]) + 1, int(page["highWatermark"]) + 1))
    assert len([e for e in page["items"] if e["kind"] == "task"]) == 18


def test_batch_append_failure_cannot_commit_partial_answers(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import question_batches
    from cortex_runtime.orchestration.state import event
    api = api_for(tmp_path)
    bound(api)
    def create(db):
        db.execute("UPDATE runtime_tasks SET state='WAITING_USER'")
        batch = question_batches.create_tx(db, "runtime-1", "channel", "batch", [
            {"question": "one", "options": ["A", "B"]}, {"question": "two", "options": ["C", "D"]}])
        event(db, "runtime-1", "USER_QUESTION_REQUESTED", batch)
        return batch
    batch = api.store.immediate_write(create)
    before = api.handle(query("project.snapshot"))
    original = storage.append_public_tx
    def fail_second_answer(db, **kwargs):
        answered = db.execute("SELECT count(*) FROM pending_questions WHERE status='answered'").fetchone()[0]
        if answered == 2:
            raise RuntimeError("injected second answer failure")
        return original(db, **kwargs)
    monkeypatch.setattr(storage, "append_public_tx", fail_second_answer)
    answers = [dict(questionId=q["question_id"], answer="answer") for q in batch["questions"]]
    with pytest.raises(sqlite3.OperationalError):
        question_batches.answer(api.store, "runtime-1", batch["batch_id"], answers,
                                "answer", session_id="session-1", owner_epoch=1)
    assert api.handle(query("project.snapshot")) == before
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM pending_questions WHERE status='open'").fetchone()[0] == 2
        assert db.execute("SELECT count(*) FROM runtime_requests").fetchone()[0] == 0


def test_extension_returns_visible_resynchronization_error(tmp_path):
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp.protocol import RpcError
    from cortex_runtime.acp.runtime import UnavailableRuntime
    api = api_for(tmp_path)
    agent = Agent(UnavailableRuntime(), environ={})
    agent._workspace_apis[api.project_id] = api
    snapshot = agent._workspace_request(query("project.snapshot"))
    assert snapshot["eventCursor"]
    with pytest.raises(RpcError, match="resync_required"):
        agent._workspace_request(query("events.page", cursor=storage.event_cursor(api.project_id, 100000)))
