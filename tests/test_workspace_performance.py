"""No-paid action hot paths on disposable stores; timings are printed, not release gates."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import statistics
import time

from cortex_runtime.delegation.events_store import TASK_EVENTS_SCHEMA
from cortex_runtime.store import Store
from test_workspace_machine_tools import machine, ok

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("performance_helper", ROOT / "crates/workspace-service/machine_tools.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)

INDEXES = ("task_events_by_activity_ref", "runtime_requests_by_operation")


def indexes(db):
    return {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='index'")}


def test_additive_indexes_only_on_writable_initialization(tmp_path):
    directory = tmp_path / ".cortex"
    store = Store(directory, project_root=tmp_path)
    with store.connection() as db:
        assert set(INDEXES) <= indexes(db)
        for name in INDEXES:
            db.execute("DROP INDEX " + name)
        db.commit()
    before = hashlib.sha256(store.path.read_bytes()).hexdigest()
    Store(directory, initialize=False, project_root=tmp_path)
    assert hashlib.sha256(store.path.read_bytes()).hexdigest() == before
    live = Store(directory, initialize=False, project_root=tmp_path, runtime_access=True)
    with live.connection() as db:
        assert not set(INDEXES) & indexes(db)
    Store(directory, project_root=tmp_path)
    with store.connection() as db:
        assert set(INDEXES) <= indexes(db)
        assert db.execute("PRAGMA user_version").fetchone()[0] == 12
        plan = db.execute("EXPLAIN QUERY PLAN SELECT result_json FROM runtime_requests "
                          "WHERE task_id=? AND actor=? AND operation=? AND instr(result_json,?)>0",
                          ("task", "actor", "command", "ref")).fetchall()
        assert any("runtime_requests_by_operation" in row[3] for row in plan)


def test_worker_lookup_plan_and_identical_projection_without_analyze():
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("CREATE TABLE tasks(id TEXT PRIMARY KEY);"
                     "CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id);")
    db.executescript(TASK_EVENTS_SCHEMA)
    db.execute("DROP INDEX task_events_by_activity_ref")
    db.executemany("INSERT INTO task_events(task_id,assignment_ref,event_type,dedup_key,payload_json,origin,created_at) "
                   "VALUES('runtime','attempt','progress',?,?, 'engine','2026-11-11T00:00:00Z')",
                   ((str(n), json.dumps(dict(kind="worker_activity",activity_ref=f"tool:{n // 2}",
                                            tool_kind="read",title=f"Tool {n // 2}",status="completed")))
                    for n in range(10000)))
    traced = []
    db.set_trace_callback(traced.append)
    before = helper.worker_history(db, "runtime", ["attempt"], None, "task", "worker", "working", "binding", None, 65536)
    db.set_trace_callback(None)
    queries = [s for s in traced if "json_extract(payload_json,'$.activity_ref') IN" in s]
    assert queries and all("ORDER BY +id" in s for s in queries)
    query = queries[0]
    def measure(sql):
        samples = []
        for _ in range(21):
            start = time.perf_counter()
            db.execute(sql).fetchall()
            samples.append((time.perf_counter()-start)*1000)
        return statistics.median(samples)
    old_ms = measure(query.replace("ORDER BY +id", "ORDER BY id").replace("AND +assignment_ref", "AND assignment_ref"))
    db.executescript(TASK_EVENTS_SCHEMA)
    plan = [row[3] for row in db.execute("EXPLAIN QUERY PLAN " + query)]
    assert any("task_events_by_activity_ref" in s for s in plan)
    new_ms = measure(query)
    after = helper.worker_history(db, "runtime", ["attempt"], None, "task", "worker", "working", "binding", None, 65536)
    assert after == before
    assert not db.execute("SELECT 1 FROM sqlite_master WHERE name='sqlite_stat1'").fetchone()
    print(json.dumps(dict(lookup_before_ms=old_ms,lookup_after_ms=new_ms,plan=plan,events=10000)))
    db.close()


def test_product_read_git_queries_are_constant_per_project(tmp_path, monkeypatch):
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("CREATE TABLE workspace_tasks(task_id,project_id);"
                     "CREATE TABLE workspace_task_options(task_id,options_json);")
    for n in range(40):
        db.execute("INSERT INTO workspace_tasks VALUES(?, 'project')", (str(n),))
        db.execute("INSERT INTO workspace_task_options VALUES(?,?)",
                   (str(n),json.dumps(dict(checkout=dict(kind="main" if n % 2 else "worktree")))))
    monkeypatch.setattr(helper,"store_reader",lambda root:db)
    monkeypatch.setattr(helper,"product_of",lambda *args:dict(tasks={},sprints=[]))
    calls = []
    def is_repo(root):
        calls.append("is_repo")
        return True
    def out(root,*args):
        calls.append(args)
        return "main" if args[0] == "branch" else str(tmp_path / ".git")
    monkeypatch.setattr(helper,"is_repo",is_repo)
    monkeypatch.setattr(helper,"out",out)
    # Avoid checkout_leaf lookups unrelated to measuring Git query counts.
    import cortex_runtime.workspace_api.task_options as options
    monkeypatch.setattr(options,"checkout_leaf",lambda db,task:task)
    result = helper.product_read(tmp_path,tmp_path / "state","project",review=False)
    assert len(result["tasks"]) == 40
    assert calls == ["is_repo",("rev-parse","--path-format=absolute","--git-common-dir"),("branch","--show-current")]


def test_real_bundled_helper_matches_component_readers(machine):
    bundle = ok(machine("product.facts", dict(taskIds=[])))
    assert bundle == dict(product=ok(machine("product.read")), workers=[])


def test_bundled_product_facts_uses_both_readers_once(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(helper,"product_read",lambda *args: calls.append("product") or dict(tasks={},sprints=[]))
    monkeypatch.setattr(helper,"workers_of",lambda root,project,ids: calls.append(tuple(ids)) or [])
    result = helper.run(dict(op="product.facts",root=str(tmp_path),projectId="project",
                             stateDir=str(tmp_path/"state"),params=dict(taskIds=["task"]),projects=[]))
    assert result == dict(product=dict(tasks={},sprints=[]),workers=[])
    assert calls == ["product",("task",)]
