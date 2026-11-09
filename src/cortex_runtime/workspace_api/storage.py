"""Additive project-local Workspace metadata; never installs schema on reads."""
from datetime import datetime, timezone
import hashlib
import json
import uuid

from ..workspace_contract import validate


class WorkspaceError(ValueError):
    """Value-free public refusal; never includes a private row or client input."""


WORKSPACE_API_TABLES = frozenset({
    "workspace_projects", "workspace_tasks", "workspace_executions", "workspace_requests",
})

WORKSPACE_API_SCHEMA = '''
CREATE TABLE IF NOT EXISTS workspace_projects (
 project_id TEXT PRIMARY KEY, project_root TEXT NOT NULL UNIQUE,
 activated_by TEXT NOT NULL, activated_at TEXT NOT NULL,
 version INTEGER NOT NULL DEFAULT 1 CHECK(version>0)
);
CREATE TABLE IF NOT EXISTS workspace_tasks (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL UNIQUE, project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 description TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),
 execution_id TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workspace_tasks_project ON workspace_tasks(project_id,sequence);
CREATE TABLE IF NOT EXISTS workspace_task_options (
 task_id TEXT PRIMARY KEY REFERENCES workspace_tasks(task_id), options_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_executions (
 execution_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES workspace_tasks(task_id),
 runtime_task_id TEXT NOT NULL UNIQUE, acp_session_id TEXT NOT NULL UNIQUE,
 owner_epoch INTEGER NOT NULL CHECK(owner_epoch>0),
 version INTEGER NOT NULL DEFAULT 1 CHECK(version>0), bound_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_starts (
 execution_id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE REFERENCES workspace_tasks(task_id),
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 actor_id TEXT NOT NULL, request_id TEXT NOT NULL,
 source_origin TEXT NOT NULL, source_event_id TEXT NOT NULL UNIQUE,
 state TEXT NOT NULL CHECK(state IN ('prepare','claimed','dispatched','unknown')),
 version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
 UNIQUE(project_id,actor_id,request_id)
);
CREATE TABLE IF NOT EXISTS workspace_requests (
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 actor_id TEXT NOT NULL, request_id TEXT NOT NULL, operation TEXT NOT NULL,
 digest TEXT NOT NULL, receipt_json TEXT NOT NULL,
 PRIMARY KEY(project_id,actor_id,request_id)
);
'''


def now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def encode(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


def digest(value):
    return hashlib.sha256(encode(value).encode("utf-8")).hexdigest()


def new_identity(prefix):
    return prefix + uuid.uuid4().hex


def receipt(request_id, outcome, target, version="0", **extra):
    return validate("Receipt", dict(requestId=request_id, outcome=outcome, target=target,
                                   entityVersion=str(version), recordedAt=now(), **extra))


def require_schema(db):
    names = {row[0] for row in db.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'workspace_%'")}
    if not (WORKSPACE_API_TABLES | WORKSPACE_OUTBOX_TABLES | WORKSPACE_START_TABLES) <= names:
        raise WorkspaceError("workspace_storage_unavailable")
    hooks = {row[0] for row in db.execute(
        "SELECT name FROM sqlite_temp_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%' "
        "UNION SELECT name FROM main.sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'")}
    if any(f"workspace_outbox_{table}_{action}" not in hooks
           for table in OUTBOX_WRITERS for action in ("insert", "update", "delete")):
        raise WorkspaceError("workspace_storage_unavailable")


def replay(db, project_id, actor_id, request_id, command_digest):
    row = db.execute(
        "SELECT digest,receipt_json FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=?",
        (project_id, actor_id, request_id)).fetchone()
    if row is None:
        return None
    original = json.loads(row["receipt_json"])
    if row["digest"] != command_digest:
        return receipt(request_id, "payload_conflict", original["target"], original["entityVersion"])
    return original


def record(db, project_id, actor_id, request_id, operation, command_digest, result):
    db.execute("INSERT INTO workspace_requests VALUES (?,?,?,?,?,?)",
               (project_id, actor_id, request_id, operation, command_digest, encode(result)))
    version = db.execute("SELECT version FROM workspace_projects WHERE project_id=?", (project_id,)).fetchone()[0]
    append_public_tx(db, project_id=project_id, entity_id=project_id, kind="unknown", version=version)
    return result


def bind_execution_tx(db, task_id, runtime_task_id, acp_session_id, *, start=None):
    """Trusted runtime hook, not a command: caller owns the write transaction.

    Validates already persisted runtime/session/owner facts. Does not bootstrap,
    dispatch, commit, infer identity from _meta, or start an engine. The explicit
    start implementation can call this inside its capture transaction.
    """
    if not db.in_transaction:
        raise WorkspaceError("binding_transaction_required")
    task = db.execute(
        "SELECT t.*,p.project_root FROM workspace_tasks t JOIN workspace_projects p USING(project_id) "
        "WHERE t.task_id=?", (task_id,)).fetchone()
    runtime = db.execute(
        "SELECT t.project_root,r.task_id FROM tasks t JOIN runtime_tasks r ON r.task_id=t.id "
        "WHERE t.id=?", (runtime_task_id,)).fetchone()
    base = db.execute("SELECT project_root FROM tasks WHERE id=?", (runtime_task_id,)).fetchone()
    session = db.execute("SELECT * FROM acp_host_sessions WHERE session_id=?", (acp_session_id,)).fetchone()
    owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (runtime_task_id,)).fetchone()
    if (task is None or base is None or base["project_root"] != task["project_root"]
            or (runtime is None and start is None) or session is None or owner is None or owner["epoch"] <= 0
            or (runtime is not None and runtime["project_root"] != task["project_root"])
            or session["project_root"] != task["project_root"]
            or session["task_id"] != runtime_task_id
            or len({task_id, runtime_task_id, acp_session_id}) != 3):
        raise WorkspaceError("binding_evidence_invalid")
    existing = db.execute("SELECT * FROM workspace_executions WHERE runtime_task_id=? OR acp_session_id=?",
                          (runtime_task_id, acp_session_id)).fetchone()
    if existing is not None:
        if (existing["task_id"] == task_id and existing["runtime_task_id"] == runtime_task_id
                and existing["acp_session_id"] == acp_session_id
                and task["execution_id"] == existing["execution_id"]):
            return existing["execution_id"]
        raise WorkspaceError("binding_conflict")
    bound_version = 1
    if start is not None:
        claimed = db.execute("SELECT * FROM workspace_starts WHERE execution_id=? AND task_id=?",
                             (start["execution_id"], task_id)).fetchone()
        if (claimed is None or claimed["state"] not in ("claimed", "dispatched")
                or task["execution_id"] != claimed["execution_id"]
                or claimed["source_origin"] != start["source_origin"]
                or claimed["source_event_id"] != start["source_event_id"]):
            raise WorkspaceError("binding_conflict")
        source = db.execute("SELECT task_id FROM source_events WHERE origin=? AND event_id=?",
                            (start["source_origin"], start["source_event_id"])).fetchone()
        if source is None or source["task_id"] != runtime_task_id:
            raise WorkspaceError("binding_evidence_invalid")
        execution_id = claimed["execution_id"]
        bound_version = claimed["version"] + 1
    else:
        if task["execution_id"] is not None:
            raise WorkspaceError("binding_conflict")
        execution_id = new_identity("we_")
    db.execute("INSERT INTO workspace_executions VALUES (?,?,?,?,?,?,?)",
               (execution_id, task_id, runtime_task_id, acp_session_id, owner["epoch"], bound_version, now()))
    db.execute("UPDATE workspace_tasks SET execution_id=?,version=version+1 WHERE task_id=?",
               (execution_id, task_id))
    db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (task["project_id"],))
    append_public_tx(db, project_id=task["project_id"], entity_id=execution_id, kind="execution", version=bound_version)
    append_public_tx(db, project_id=task["project_id"], entity_id=task_id, kind="task", version=task["version"] + 1)
    return execution_id


WORKSPACE_START_TABLES = frozenset({"workspace_starts"})


def start_state_tx(db, execution_id, state):
    row = db.execute("SELECT * FROM workspace_starts WHERE execution_id=?", (execution_id,)).fetchone()
    if row is None:
        raise WorkspaceError("not_found")
    db.execute("UPDATE workspace_starts SET state=?,version=version+1 WHERE execution_id=?", (state, execution_id))
    db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", (row["task_id"],))
    db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (row["project_id"],))
    version = db.execute("SELECT version FROM workspace_tasks WHERE task_id=?", (row["task_id"],)).fetchone()[0]
    append_public_tx(db, project_id=row["project_id"], entity_id=row["task_id"], kind="task", version=version)
    bound = db.execute("SELECT version FROM workspace_executions WHERE execution_id=?", (execution_id,)).fetchone()
    if bound:
        db.execute("UPDATE workspace_executions SET version=version+1 WHERE execution_id=?", (execution_id,))
    append_public_tx(db, project_id=row["project_id"], entity_id=execution_id, kind="execution",
                     version=bound["version"] + 1 if bound else row["version"] + 1)


def claim_start(store, execution_id):
    """Only the original preparing call can claim. A receipt retry never calls here."""
    def write(db):
        row = db.execute("SELECT * FROM workspace_starts WHERE execution_id=?", (execution_id,)).fetchone()
        if row is None or row["state"] != "prepare":
            raise WorkspaceError("start_not_prepared")
        start_state_tx(db, execution_id, "claimed")
        return dict(db.execute("SELECT s.*,t.description FROM workspace_starts s "
                               "JOIN workspace_tasks t USING(task_id) WHERE s.execution_id=?",
                               (execution_id,)).fetchone())
    return store.immediate_write(write)


# A separate additive schema group: old format-12 stores may have the adapter
# metadata but not this journal. Inspection never installs either group.
WORKSPACE_OUTBOX_TABLES = frozenset({"workspace_outbox_state", "workspace_outbox"})
OUTBOX_RETENTION = 10_000
WORKSPACE_OUTBOX_SCHEMA = '''
CREATE TABLE IF NOT EXISTS workspace_outbox_state (
 project_id TEXT PRIMARY KEY REFERENCES workspace_projects(project_id),
 high_watermark INTEGER NOT NULL DEFAULT 0 CHECK(high_watermark>=0),
 retained_floor INTEGER NOT NULL DEFAULT 0 CHECK(retained_floor>=0 AND retained_floor<=high_watermark)
);
CREATE TABLE IF NOT EXISTS workspace_outbox (
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 sequence INTEGER NOT NULL CHECK(sequence>0),
 event_id TEXT NOT NULL UNIQUE, entity_id TEXT NOT NULL,
 kind TEXT NOT NULL, entity_version INTEGER NOT NULL CHECK(entity_version>0),
 recorded_at TEXT NOT NULL, payload_digest TEXT NOT NULL,
 PRIMARY KEY(project_id,sequence)
);
'''

# Writer coverage is table-based, so a new caller or direct SQL path cannot
# bypass invalidation. These are all private facts in this slice's public
# task/execution/history/question/result projections. Questions and results
# invalidate their enclosing task/execution until their own DTOs are available.
# Adapter metadata writers call append_public_tx explicitly (tested inventory).
OUTBOX_WRITERS = {
    "tasks": ("runtime", "id"),
    "runtime_tasks": ("runtime", "task_id"),
    "runtime_nodes": ("runtime", "task_id"),
    "runtime_events": ("runtime", "task_id"),
    "task_events": ("runtime", "task_id"),
    "task_orchestration": ("runtime", "task_id"),
    "task_owners": ("runtime", "task_id"),
    "pending_questions": ("runtime", "task_id"),
    "pending_question_late_answers": ("runtime", "task_id"),
    "task_language": ("runtime", "task_id"),
    "task_stopped": ("runtime", "task_id"),
    "acp_host_sessions": ("session", "session_id"),
    "acp_session_access_defaults": ("session", "session_id"),
    "host_transcript": ("session", "session_id"),
    "host_updates": ("session", "session_id"),
}


def outbox_trigger_statements(tables=None, *, existing=()):
    """AFTER hooks run within the original SQL statement, including autocommit.

    Both hooks and callback belong to the same Store connection. Persisting
    hooks would require unrelated/older writers to load Workspace code, even
    for unbound rows. There is no publisher, second transaction, private event-ID
    conversion or internal-event recursion. Version-only writes below touch no
    hooked private table.
    """
    for table, (scope, column) in OUTBOX_WRITERS.items():
        if tables is not None and table not in tables:
            continue
        binding = "runtime_task_id" if scope == "runtime" else "acp_session_id"
        for action in ("INSERT", "UPDATE", "DELETE"):
            name = f"workspace_outbox_{table}_{action.lower()}"
            if name in existing:
                continue
            row = "OLD" if action == "DELETE" else "NEW"
            identity = f"{row}.{column}"
            # An UPDATE that moves a binding invalidates both its old and new
            # target. All other updates invalidate just once per changed row.
            old = ""
            if action == "UPDATE":
                old = (f"SELECT cortex_workspace_invalidate('{scope}',OLD.{column}) "
                       f"WHERE OLD.{column} IS NOT NEW.{column} AND EXISTS "
                       f"(SELECT 1 FROM workspace_executions WHERE {binding}=OLD.{column});")
            yield (
                f"CREATE TEMP TRIGGER IF NOT EXISTS {name} "
                f"AFTER {action} ON main.{table} BEGIN "
                f"SELECT cortex_workspace_invalidate('{scope}',{identity}) WHERE EXISTS "
                f"(SELECT 1 FROM workspace_executions WHERE {binding}={identity});"
                + old + " END"
            )


# Writable initialization retires the old durable hooks before any schema
# repair writes. Inspection never repairs main, and TEMP hooks never survive
# closing a connection or impose dependencies on an independent SQLite writer.
OUTBOX_LEGACY_CLEANUP_SCHEMA = "".join(
    f"DROP TRIGGER IF EXISTS main.workspace_outbox_{table}_{action};\n"
    for table in OUTBOX_WRITERS for action in ("insert", "update", "delete")
)


def install_outbox_hooks(db):
    tables = {row[0] for row in db.execute("SELECT name FROM main.sqlite_master WHERE type='table'")}
    if not (WORKSPACE_API_TABLES | WORKSPACE_OUTBOX_TABLES | WORKSPACE_START_TABLES) <= tables:
        return
    legacy = {row[0] for row in db.execute(
        "SELECT name FROM main.sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'")}
    # Non-initializing opens leave legacy main hooks untouched. Avoid publishing
    # twice until writable initialization retires them and installs TEMP hooks.
    for statement in outbox_trigger_statements(tables, existing=legacy):
        db.execute(statement)


def register_outbox_function(db):
    def invalidate(scope, identity):
        binding = "runtime_task_id" if scope == "runtime" else "acp_session_id"
        rows = db.execute(
            f"SELECT e.execution_id,e.version AS execution_version,t.task_id,t.version AS task_version,t.project_id "
            f"FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE e.{binding}=?",
            (identity,)).fetchall()
        for row in rows:
            db.execute("UPDATE workspace_executions SET version=version+1 WHERE execution_id=?", (row["execution_id"],))
            db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", (row["task_id"],))
            db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (row["project_id"],))
            append_public_tx(db, project_id=row["project_id"], entity_id=row["task_id"],
                             kind="task", version=row["task_version"] + 1, _from_trigger=True)
            append_public_tx(db, project_id=row["project_id"], entity_id=row["execution_id"],
                             kind="execution", version=row["execution_version"] + 1, _from_trigger=True)
        return 0
    db.create_function("cortex_workspace_invalidate", 2, invalidate)


def append_public_tx(db, *, project_id, entity_id, kind, version, _from_trigger=False):
    """The sole append helper; caller owns state write and transaction.

    Only identifiers, versions and a fixed invalidation kind enter the outbox.
    No prompt, transcript, question, result text, private payload or file path is
    accepted. The counter/floor survive pruning and never use private sequences.
    """
    if not db.in_transaction and not _from_trigger:
        # Trigger calls in SQLite autocommit still belong to the outer statement
        # transaction, although sqlite3.in_transaction reports False there.
        # Public callers must use an explicit transaction; the function itself
        # has no commit/rollback or connection-opening side effects.
        raise WorkspaceError("outbox_transaction_required")
    validate("Identity", project_id)
    validate("Identity", entity_id)
    validate("PositiveDecimal", str(version))
    if kind not in {"task", "execution", "decision", "result", "unknown"}:
        raise WorkspaceError("outbox_kind_invalid")
    db.execute("INSERT OR IGNORE INTO workspace_outbox_state(project_id) VALUES (?)", (project_id,))
    db.execute("UPDATE workspace_outbox_state SET high_watermark=high_watermark+1 WHERE project_id=?", (project_id,))
    sequence = db.execute("SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?", (project_id,)).fetchone()[0]
    payload = dict(projectId=project_id, entityId=entity_id, kind=kind, entityVersion=str(version),
                   type="invalidated", schemaVersion="1.0", mappingVersion="1")
    db.execute("INSERT INTO workspace_outbox VALUES (?,?,?,?,?,?,?,?)",
               (project_id, sequence, new_identity("wev_"), entity_id, kind, version, now(), digest(payload)))
    floor = max(0, sequence - OUTBOX_RETENTION)
    db.execute("DELETE FROM workspace_outbox WHERE project_id=? AND sequence<=?", (project_id, floor))
    db.execute("UPDATE workspace_outbox_state SET retained_floor=max(retained_floor,?) WHERE project_id=?", (floor, project_id))
    return sequence


def outbox_position(db, project_id):
    row = db.execute("SELECT high_watermark,retained_floor FROM workspace_outbox_state WHERE project_id=?", (project_id,)).fetchone()
    return (row[0], row[1]) if row is not None else (0, 0)


def event_cursor(project_id, sequence):
    return "we1." + digest(project_id)[:32] + "." + str(sequence)


def parse_event_cursor(project_id, cursor):
    parts = cursor.split(".")
    if (len(parts) != 3 or parts[:2] != ["we1", digest(project_id)[:32]]
            or len(parts[2]) > 19 or not parts[2].isascii() or not parts[2].isdecimal()
            or str(int(parts[2])) != parts[2] or int(parts[2]) > 9223372036854775807):
        raise WorkspaceError("invalid_cursor")
    return int(parts[2])


def event_page(db, project_id, *, cursor=None, limit=100):
    from ..workspace_contract import LIMITS
    if type(limit) is not int or not 1 <= limit <= LIMITS["eventsPerPage"]:
        raise WorkspaceError("invalid_request")
    after = 0 if cursor is None else parse_event_cursor(project_id, cursor)
    high, floor = outbox_position(db, project_id)
    if after < floor:
        raise WorkspaceError("resync_required:floor_expired")
    if after > high:
        raise WorkspaceError("resync_required:future_cursor")
    rows = db.execute("SELECT * FROM workspace_outbox WHERE project_id=? AND sequence>? ORDER BY sequence LIMIT ?",
                      (project_id, after, limit + 1)).fetchall()
    page = dict(items=[], nextCursor=None, highWatermark=str(high), retainedFloor=str(floor),
                eventCursor=event_cursor(project_id, after))
    expected = after + 1
    for row in rows:
        if row["sequence"] != expected:
            raise WorkspaceError("resync_required:sequence_gap")
        expected += 1
        if len(page["items"]) == limit:
            break
        event = dict(eventId=row["event_id"], projectId=project_id, sequence=str(row["sequence"]),
                     entityId=row["entity_id"], entityVersion=str(row["entity_version"]), kind=row["kind"],
                     recordedAt=row["recorded_at"], type="invalidated", schemaVersion="1.0",
                     mappingVersion="1", payloadDigest=row["payload_digest"])
        # Reserve space for both opaque cursors and the response wrapper.
        if len(encode(dict(page, items=page["items"] + [event])).encode("utf-8")) > LIMITS["pageContentBytes"] - 1024:
            break
        page["items"].append(event)
    last = int(page["items"][-1]["sequence"]) if page["items"] else after
    if len(rows) <= limit and ((not rows and after < high) or (rows and rows[-1]["sequence"] < high)):
        raise WorkspaceError("resync_required:sequence_gap")
    page["eventCursor"] = event_cursor(project_id, last)
    if last < high:
        page["nextCursor"] = page["eventCursor"]
    return validate("EventPage", page)
