"""SQLite metadata with project-local, atomic Markdown publication."""
from __future__ import annotations

import bisect
import codecs
from collections import OrderedDict
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import stat
import threading
import time
import uuid

from . import _fault_injection


class NativeSource(str):
    """Text plus private provenance, never serialized into a tool response.

    Relocated verbatim from the deleted (T033) Codex-plugin-harness-era
    ``cortex_runtime.host_source``. ``Store.call``'s ``original_request``
    parameter (see below) expects exactly this shape -- ``.cursor``,
    ``.messages``, ``.completeness`` and ``.reason`` -- from any caller
    that supplies real captured source text, native or ACP.

    T033 follow-up (recorded, not fixed here): ``server.py``'s only live
    ``Store.call`` site passes ``original_request=None`` (bootstrap_task
    captures the ACP source text up front instead, before any task-scoped
    call). So on the current ACP path nothing ever constructs a
    ``NativeSource`` in production, and the ``original_request`` capture
    branch in ``Store._call``/``_capture_status`` (``original_request_sha256``)
    is reachable only from tests. Left in place deliberately -- removing a
    still-plumbed-through Store parameter is a larger, riskier change than
    this task's scope.
    """
    def __new__(cls, text, cursor, messages, completeness='complete', reason=None):
        value = super().__new__(cls, text)
        value.cursor = cursor
        value.messages = messages
        value.completeness = completeness
        value.reason = reason
        return value
from .contracts import StoreError, validate
from .governance import governance_snapshot

APPLICATION_ID = 1129467218
# How long a shared Store.connection() waits for a transient exclusive holder
# of .access.lock (for example a metadata inspection page) before storage_busy.
SHARED_ACCESS_WAIT_SECONDS = 2.0
SCHEMA = '''
CREATE TABLE IF NOT EXISTS tasks (
 id TEXT PRIMARY KEY, project_root TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_by_project ON tasks(project_root);
CREATE TABLE IF NOT EXISTS thread_bindings (
 thread_id TEXT PRIMARY KEY, parent_thread_id TEXT,
 task_id TEXT NOT NULL REFERENCES tasks(id)
);
CREATE TABLE IF NOT EXISTS reports (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 kind TEXT NOT NULL, filename TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_cursors (
 thread_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), cursor TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_messages (
 thread_id TEXT NOT NULL, message_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
 report_id TEXT NOT NULL REFERENCES reports(id), digest TEXT NOT NULL,
 PRIMARY KEY(thread_id,message_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS one_pipeline ON reports(task_id) WHERE kind='pipeline';
CREATE TABLE IF NOT EXISTS editions (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 report_id TEXT NOT NULL REFERENCES reports(id),
 title TEXT NOT NULL, summary TEXT NOT NULL, author TEXT NOT NULL,
 updated_at TEXT NOT NULL, digest TEXT NOT NULL, size_bytes INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS report_editions ON editions(report_id,sequence);
CREATE TABLE IF NOT EXISTS governance (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), mode TEXT NOT NULL,
 report_id TEXT NOT NULL REFERENCES reports(id)
);
CREATE TABLE IF NOT EXISTS deliveries (
 scope TEXT NOT NULL, operation TEXT NOT NULL, delivery_key TEXT NOT NULL,
 digest TEXT NOT NULL, content_digest TEXT NOT NULL, response TEXT NOT NULL,
 PRIMARY KEY(scope,operation,delivery_key)
);
CREATE TABLE IF NOT EXISTS drafts (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 owner_thread_id TEXT NOT NULL, kind TEXT NOT NULL, template TEXT NOT NULL,
 path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL,
 device INTEGER NOT NULL, inode INTEGER NOT NULL,
 published_report_id TEXT REFERENCES reports(id), published_digest TEXT
);
CREATE TABLE IF NOT EXISTS pending_source_deletions (
 task_id TEXT NOT NULL REFERENCES tasks(id), draft_id TEXT NOT NULL REFERENCES drafts(id),
 path TEXT NOT NULL, digest TEXT NOT NULL, PRIMARY KEY(task_id,draft_id)
);
CREATE TABLE IF NOT EXISTS pending_deletions (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id)
);
CREATE TRIGGER IF NOT EXISTS editions_no_update BEFORE UPDATE ON editions BEGIN
 SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS editions_no_delete BEFORE DELETE ON editions
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d JOIN reports r ON r.task_id=d.task_id
 WHERE r.id=OLD.report_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS binding_receipts (
 thread_id TEXT PRIMARY KEY REFERENCES thread_bindings(thread_id), receipt TEXT NOT NULL UNIQUE,
 state TEXT NOT NULL DEFAULT 'cortex', updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_resets (
 thread_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), pause_turn TEXT
);
CREATE TABLE IF NOT EXISTS source_turns (
 task_id TEXT NOT NULL REFERENCES tasks(id), thread_id TEXT NOT NULL,
 turn_id TEXT NOT NULL, report_id TEXT NOT NULL REFERENCES reports(id), digest TEXT NOT NULL,
 PRIMARY KEY(thread_id,turn_id,report_id)
);
CREATE TABLE IF NOT EXISTS source_revisions (
 task_id TEXT NOT NULL REFERENCES tasks(id), revision INTEGER NOT NULL,
 thread_id TEXT NOT NULL, message_id TEXT NOT NULL, report_id TEXT NOT NULL REFERENCES reports(id),
 attachments TEXT NOT NULL DEFAULT '[]', PRIMARY KEY(task_id,revision), UNIQUE(thread_id,message_id)
);
CREATE TABLE IF NOT EXISTS report_provenance (
 edition_sequence INTEGER PRIMARY KEY REFERENCES editions(sequence), source_revision INTEGER NOT NULL,
 artifacts TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS draft_revisions (
 draft_id TEXT PRIMARY KEY REFERENCES drafts(id), source_revision INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS task_changes (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL REFERENCES tasks(id),
 kind TEXT NOT NULL, reference TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS changes_by_task ON task_changes(task_id,sequence);
-- T019: additive optional group. Never claims what an attempt did; only
-- what a read-only, isolated git inspection of the (shared) workspace root
-- actually observed at one instant, bound to a worker_result event or a
-- coordinator-acceptance gate check -- see delegation.revision.
CREATE TABLE IF NOT EXISTS attempt_revisions (
 attempt_id TEXT NOT NULL, captured_at TEXT NOT NULL, phase TEXT NOT NULL,
 kind TEXT NOT NULL, head TEXT, dirty_digest TEXT, detail TEXT, manifest_json TEXT
);
CREATE INDEX IF NOT EXISTS attempt_revisions_by_attempt ON attempt_revisions(attempt_id,captured_at);
-- T032: durable record of every offline format migration/rollback ever
-- applied to this exact database file (never mutated, only appended).
-- Optional, self-healing like attempt_revisions above -- a pre-T032 v11
-- store simply has zero rows until its next migration/rollback.
CREATE TABLE IF NOT EXISTS format_history (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL,
 from_version INTEGER NOT NULL, to_version INTEGER NOT NULL,
 backup_path TEXT NOT NULL, backup_digest TEXT NOT NULL, at TEXT NOT NULL
);
PRAGMA user_version=12;
'''

PIPELINE_PLACEHOLDERS = (
    b"{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}",
    b"{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}",
    b"{{CURRENT_WORK_GRAPH}}",
    b"{{CURRENT_ASSIGNMENTS_AND_ROUTING}}",
    b"{{CURRENT_EVIDENCE_AND_VERIFICATION}}",
    b"{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}",
)


def identifier(prefix):
    return prefix + uuid.uuid4().hex[:12]


def unique_identifier(db, prefix):
    table={"t_":"tasks","r_":"reports","d_":"drafts"}[prefix]
    for _ in range(32):
        value=identifier(prefix)
        if db.execute(f"SELECT 1 FROM {table} WHERE id=?",(value,)).fetchone() is None:
            return value
    raise StoreError("identifier_unavailable")


def encode(value):
    return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(",",":"))


def digest(body):
    return hashlib.sha256(body).hexdigest()


def _base36(value):
    alphabet="0123456789abcdefghijklmnopqrstuvwxyz"; result=""
    while value:
        value,remainder=divmod(value,36); result=alphabet[remainder]+result
    return result


def private_directory(path):
    """Create a private storage directory without traversing symlinks."""
    path=Path(path).expanduser().absolute()
    if ".." in path.parts: raise StoreError("unsafe_storage","storage_path",str(path),"canonical private directory")
    current=Path(path.anchor)
    for part in path.parts[1:]:
        current/=part
        try: current.mkdir(mode=0o700)
        except FileExistsError: pass
        info=current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise StoreError("unsafe_storage","storage_path",str(current),"owned directory without symlinks")
    info=path.stat()
    if info.st_uid!=os.getuid() or stat.S_IMODE(info.st_mode)&0o077:
        raise StoreError(
            "unsafe_storage", "storage_path", str(path), "owner-only directory mode 0700",
            f"run chmod 700 {shlex.quote(str(path))}",
        )
    return path


def project_draft_directory(project_root,kind="report"):
    """Create the source directory for one file-backed Markdown write."""
    project=Path(project_root)
    if not project.is_absolute() or not project.is_dir() or str(project.resolve())!=str(project):
        raise StoreError("invalid_project","project_root",str(project_root),"absolute existing canonical directory")
    current=project
    leaf={"report":"draft-reports","pipeline":"pipeline-drafts"}.get(kind)
    if leaf is None: raise StoreError("invalid_draft_path","kind",kind,"report or pipeline")
    for part in (".cortex",leaf):
        current/=part
        try: current.mkdir(mode=0o700)
        except FileExistsError: pass
        info=current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid!=os.getuid():
            raise StoreError("unsafe_storage","draft_path",str(current),"owned directory without symlinks")
        os.chmod(current,0o700,follow_symlinks=False)
    return current


def project_task_directory(project_root):
    """Create the MCP-owned published-document root below project .cortex."""
    project=Path(project_root)
    if not project.is_absolute() or not project.is_dir() or str(project.resolve())!=str(project):
        raise StoreError("invalid_project","project_root",str(project_root),"absolute existing canonical directory")
    current=project
    for part in (".cortex",):
        current/=part
        try: current.mkdir(mode=0o700)
        except FileExistsError: pass
        info=current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid!=os.getuid():
            raise StoreError("unsafe_storage","storage_path",str(current),"owned directory without symlinks")
        os.chmod(current,0o700,follow_symlinks=False)
    return current


def draft_template(template, draft_id):
    """Load one packaged draft template and add its durable short identifier."""
    root=Path(__file__).resolve().parents[2]/"report-templates"
    path=(root/(template+".md")).resolve()
    if path.parent!=root.resolve() or not path.is_file():
        raise StoreError("storage_error","template",template,"packaged draft template")
    body=path.read_bytes()
    try: body.decode("utf-8")
    except UnicodeDecodeError:
        raise StoreError("storage_error","template",template,"valid UTF-8 packaged draft template") from None
    return f"Cortex draft ID: `{draft_id}`\n\n".encode()+body


def regular(path, private=True):
    info=Path(path).lstat()
    # A concurrent unlink() of this exact path (for example SQLite deleting
    # then recreating its -wal/-shm sidecar as the last/first connection
    # closes/opens -- see Store.connection()) has a well-known benign race:
    # another process's lstat() can observe the just-unlinked inode (still
    # owned, still regular, still correct mode) with st_nlink==0 for a brief
    # window before the directory entry is gone from this process's view.
    # This can never be a symlink or a foreign/attacker hardlink -- both of
    # those always show st_nlink>=1 (a hardlink attack increases nlink, it
    # never produces zero) -- so treating it as "does not exist right now"
    # preserves the safety property while tolerating the transient churn.
    if info.st_nlink==0:
        raise FileNotFoundError(2,"No such file or directory",str(path))
    if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_nlink!=1:
        raise StoreError("unsafe_storage","path",str(path),"owned regular file with one link")
    if private and stat.S_IMODE(info.st_mode)&0o077:
        raise StoreError("unsafe_storage","path",str(path),"owner-only regular file mode 0600")
    return info


def file_blocks(path, private=True):
    path=Path(path); before=regular(path,private)
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,"rb") as stream:
        info=os.fstat(stream.fileno())
        signature=(info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns,info.st_ctime_ns)
        expected=(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns)
        if signature!=expected: raise StoreError("file_conflict","path",str(path),"unchanged regular file")
        while block:=stream.read(64*1024): yield block
        info=os.fstat(stream.fileno())
        after=(info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns,info.st_ctime_ns)
        if after!=signature: raise StoreError("file_conflict","path",str(path),"unchanged regular file")


def file_digest(path, private=True, validate_utf8=False,error_field="path",error_value=None):
    hashed=hashlib.sha256(); decoder=codecs.getincrementaldecoder("utf-8")() if validate_utf8 else None
    try:
        for block in file_blocks(path,private):
            hashed.update(block)
            if decoder: decoder.decode(block)
        if decoder: decoder.decode(b"",final=True)
    except UnicodeDecodeError:
        raise StoreError("invalid_utf8",error_field,error_value if error_value is not None else str(path),"complete valid UTF-8 Markdown file") from None
    return hashed.hexdigest()


def sync_directory(path):
    fd=os.open(path,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)


class Store:
    def __init__(self,directory,initialize=True,*,project_root=None):
        from .trusted_channel import IDENTITY_SCHEMA
        from .catalog.store import CATALOG_SCHEMA
        from .configuration.store import CONFIGURATION_SCHEMA
        from .acp.sessions import ACP_SESSION_SCHEMA
        from .delegation.store import DELEGATION_SCHEMA
        from .delegation.work_graph import WORK_GRAPH_SCHEMA
        from .delegation.events_store import TASK_EVENTS_SCHEMA
        from .orchestration.state import SCHEMA as RUNTIME_SCHEMA
        from .delegation.capacity_store import CAPACITY_SCHEMA
        from .delegation.evidence_store import EVIDENCE_SCHEMA
        from .delegation.generations_store import CONTEXT_GENERATIONS_SCHEMA
        from .recovery.budget_store import LARGE_OUTPUT_SCHEMA
        from .recovery.registry import PROCESS_REGISTRY_SCHEMA
        from .acp.replay_store import REPLAY_SCHEMA
        from .acp.questions_store import PENDING_QUESTIONS_SCHEMA
        from .acp.steering import TASK_LANGUAGE_SCHEMA, TASK_STOPPED_SCHEMA
        from .delegation.workspace_isolation import WORKSPACE_SCHEMA
        from .delegation.integration import INTEGRATION_SCHEMA
        from .routing.store import ROUTING_SCHEMA
        from .observability.store import OBSERVABILITY_SCHEMA
        from .project_storage import canonical_project, project_store_directory
        self.project_root=canonical_project(project_root) if project_root is not None else None
        if self.project_root is not None and Path(directory)!=project_store_directory(self.project_root):
            raise StoreError('project_storage_mismatch')
        self._file_cache=OrderedDict()
        self._cache_lock=threading.RLock()
        self.directory=private_directory(directory)
        self.path=self.directory/"cortex.sqlite3"
        if initialize:
            fd=os.open(self.path,os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600); os.close(fd)
        regular(self.path)
        # Format 11 is upgraded on a normal writable runtime open.  A
        # non-initializing open is also used by inspection/maintenance paths;
        # keep those paths read-only with respect to format migration so they
        # retain the explicit migration-required error below.
        if initialize:
            from .migrate import auto_migrate_format11
            auto_migrate_format11(self.directory)
        with self.connection() as db:
            # A rejected archive must retain its original bytes and journal
            # mode. Validate under the write lock before any persistent pragma.
            if initialize:db.execute("BEGIN IMMEDIATE")
            app=db.execute("PRAGMA application_id").fetchone()[0]
            version=db.execute("PRAGMA user_version").fetchone()[0]
            tables=db.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
            if app!=APPLICATION_ID and (app!=0 or tables or version!=0): raise StoreError("unsupported_storage")
            if app==APPLICATION_ID and version != 12:
                raise StoreError("legacy_storage_migration_required" if version==11 else "unsupported_storage")
            if not initialize and (app!=APPLICATION_ID or version!=12):raise StoreError("legacy_storage_migration_required" if version==11 else "unsupported_storage")
            if app==APPLICATION_ID:self._check_project(db)
            db.commit()
            if initialize:
                db.execute("PRAGMA journal_mode=WAL")
                db.executescript("BEGIN IMMEDIATE;\n"+SCHEMA+IDENTITY_SCHEMA+CATALOG_SCHEMA+CONFIGURATION_SCHEMA+ACP_SESSION_SCHEMA+DELEGATION_SCHEMA+WORK_GRAPH_SCHEMA+TASK_EVENTS_SCHEMA+RUNTIME_SCHEMA+CAPACITY_SCHEMA+EVIDENCE_SCHEMA+REPLAY_SCHEMA+PENDING_QUESTIONS_SCHEMA+TASK_LANGUAGE_SCHEMA+TASK_STOPPED_SCHEMA+CONTEXT_GENERATIONS_SCHEMA+LARGE_OUTPUT_SCHEMA+PROCESS_REGISTRY_SCHEMA+WORKSPACE_SCHEMA+INTEGRATION_SCHEMA+ROUTING_SCHEMA+OBSERVABILITY_SCHEMA+f"PRAGMA application_id={APPLICATION_ID};\nCOMMIT;")
            if app==APPLICATION_ID:
                # T036: add the model-selected requirement type and the
                # fail-closed legacy assignment state to pre-existing graph
                # rows.  WorkGraph also performs this check on direct graph
                # access, but opening a store must make the migration durable
                # before completion reads it.
                from .delegation.work_graph import WorkGraph
                WorkGraph._ensure_schema(db)
                # T017: additive columns for a pre-T017 store that already has
                # task_events/task_orchestration (CREATE TABLE IF NOT EXISTS
                # above never adds columns to an existing table).
                from .delegation.events_store import ensure_task_lifecycle_columns
                ensure_task_lifecycle_columns(db)
                # T018: additive source_events.meta_json for a pre-T018 store
                # that already has source_events (CREATE TABLE IF NOT EXISTS
                # above never adds columns to an existing table).
                from .trusted_channel import ensure_source_event_meta_column, ensure_source_event_ledger_column
                ensure_source_event_meta_column(db)
                ensure_source_event_ledger_column(db)
                # T019: additive attempt_revisions table for a pre-T019 store
                # (CREATE TABLE IF NOT EXISTS in SCHEMA only runs for a brand
                # new database -- see the ``if initialize:`` branch above).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS attempt_revisions ("
                    " attempt_id TEXT NOT NULL, captured_at TEXT NOT NULL, phase TEXT NOT NULL,"
                    " kind TEXT NOT NULL, head TEXT, dirty_digest TEXT, detail TEXT, manifest_json TEXT)"
                )
                revision_columns = {row["name"] for row in db.execute("PRAGMA table_info(attempt_revisions)")}
                if "manifest_json" not in revision_columns:
                    db.execute("ALTER TABLE attempt_revisions ADD COLUMN manifest_json TEXT")
                db.execute("CREATE INDEX IF NOT EXISTS attempt_revisions_by_attempt ON attempt_revisions(attempt_id,captured_at)")
                # Listings nothing reads any more: publication captures and
                # the start captures of attempts whose process exited. An
                # inspection open stays non-mutating.
                if initialize:
                    db.execute(
                        "UPDATE attempt_revisions SET manifest_json=NULL WHERE manifest_json IS NOT NULL AND "
                        "(phase LIKE 'worker_publish:%' OR (phase LIKE 'worker_start:%' AND attempt_id IN "
                        "(SELECT attempt_id FROM attempt_process_registry WHERE terminated_at IS NOT NULL "
                        "AND termination_reason IS NOT 'timeout_alive')))"
                    )
                # T020: additive pending_questions/pending_question_late_answers
                # tables for a pre-T020 store (same precedent as
                # attempt_revisions above -- plain db.execute per statement,
                # not executescript, so this never opens an implicit nested
                # transaction here).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS pending_questions ("
                    " id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " assignment_ref TEXT, dedup_key TEXT NOT NULL, text TEXT NOT NULL,"
                    " options_json TEXT, status TEXT NOT NULL DEFAULT 'open',"
                    " answer_text TEXT, answer_source TEXT, created_at TEXT NOT NULL,"
                    " answered_at TEXT, UNIQUE(task_id, dedup_key))"
                )
                db.execute("CREATE INDEX IF NOT EXISTS pending_questions_by_task ON pending_questions(task_id, status, id)")
                db.execute(
                    "CREATE TABLE IF NOT EXISTS pending_question_late_answers ("
                    " id INTEGER PRIMARY KEY AUTOINCREMENT, question_id TEXT NOT NULL,"
                    " task_id TEXT NOT NULL, answer_text TEXT NOT NULL, answer_source TEXT NOT NULL,"
                    " received_at TEXT NOT NULL)"
                )
                # T020: additive task_language table for a pre-T020 store
                # (same precedent as pending_questions above).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS task_language ("
                    " task_id TEXT PRIMARY KEY REFERENCES tasks(id), language TEXT)"
                )
                # T020 fresh-review fix: additive task_stopped table for a
                # pre-fix store (same precedent).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS task_stopped ("
                    " task_id TEXT PRIMARY KEY REFERENCES tasks(id), stopped_at TEXT NOT NULL)"
                )
                # T023: additive attempt_process_registry/attempt_effective_context
                # tables for a pre-T023 store (same precedent as pending_questions
                # above -- CREATE TABLE IF NOT EXISTS in SCHEMA only runs for a
                # brand new database).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS attempt_process_registry ("
                    " attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " role TEXT NOT NULL, engine_session_ref TEXT, connection_id TEXT,"
                    " pid INTEGER NOT NULL, pgid INTEGER NOT NULL,"
                    " start_ticks TEXT NOT NULL, boot_id TEXT NOT NULL,"
                    " launch_command_digest TEXT NOT NULL,"
                    " recorded_at TEXT NOT NULL, terminated_at TEXT, termination_reason TEXT)"
                )
                db.execute(
                    "CREATE TABLE IF NOT EXISTS attempt_effective_context ("
                    " attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " role TEXT NOT NULL,"
                    " selected_config_revision_id TEXT, effective_config_revision_id TEXT,"
                    " route_engine TEXT NOT NULL, route_connection_id TEXT,"
                    " resource_policy_json TEXT NOT NULL, recorded_at TEXT NOT NULL)"
                )
                # T025: additive workspace_snapshots/workspace_integrations
                # tables for a pre-T025 store (same precedent as
                # attempt_process_registry above -- CREATE TABLE IF NOT
                # EXISTS in SCHEMA only runs for a brand new database).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS workspace_snapshots ("
                    " workspace_binding_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " attempt_id TEXT NOT NULL, project_root TEXT NOT NULL,"
                    " root_path TEXT, base_path TEXT, mode TEXT NOT NULL, manifest_json TEXT NOT NULL,"
                    " base_kind TEXT NOT NULL, base_head TEXT, base_dirty_digest TEXT,"
                    " created_at TEXT NOT NULL, cleaned_up_at TEXT, cleanup_reason TEXT)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS workspace_snapshots_by_attempt ON workspace_snapshots(attempt_id, created_at)")
                db.execute(
                    "CREATE TABLE IF NOT EXISTS workspace_integrations ("
                    " id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " assignment_ref TEXT NOT NULL, request_key TEXT NOT NULL, attempt_id TEXT NOT NULL,"
                    " workspace_binding_id TEXT, status TEXT NOT NULL,"
                    " applied_paths_json TEXT NOT NULL, conflict_paths_json TEXT NOT NULL,"
                    " integrated_head TEXT, integrated_dirty_digest TEXT,"
                    " host_buffer_state TEXT NOT NULL DEFAULT 'unknown', created_at TEXT NOT NULL,"
                    " UNIQUE(assignment_ref, request_key))"
                )
                # T027: additive configuration_route_extras/attempt_route_audit/
                # fallback_consent_records tables for a pre-T027 store (same
                # precedent as attempt_process_registry above -- CREATE TABLE
                # IF NOT EXISTS in SCHEMA only runs for a brand new database).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS configuration_route_extras ("
                    " revision_id TEXT PRIMARY KEY REFERENCES configuration_revisions(id),"
                    " task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " route_model TEXT NOT NULL, route_profile_json TEXT NOT NULL,"
                    " route_details_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS configuration_route_extras_by_task ON configuration_route_extras(task_id)")
                db.execute(
                    "CREATE TABLE IF NOT EXISTS attempt_route_audit ("
                    " id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " role TEXT NOT NULL, engine TEXT NOT NULL, connection_id TEXT NOT NULL,"
                    " model TEXT NOT NULL, params_fingerprint TEXT NOT NULL, config_revision_id TEXT,"
                    " decision TEXT NOT NULL CHECK(decision IN ('authorized','fallback')),"
                    " decision_detail_json TEXT NOT NULL, predecessor_attempt_id TEXT, recorded_at TEXT NOT NULL)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS attempt_route_audit_by_task ON attempt_route_audit(task_id, recorded_at)")
                db.execute("CREATE INDEX IF NOT EXISTS attempt_route_audit_by_attempt ON attempt_route_audit(attempt_id)")
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_update BEFORE UPDATE ON attempt_route_audit "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_delete BEFORE DELETE ON attempt_route_audit "
                    "WHEN NOT EXISTS (SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id) "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )
                db.execute(
                    "CREATE TABLE IF NOT EXISTS fallback_consent_records ("
                    " id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " target_connection_id TEXT NOT NULL, target_provider TEXT NOT NULL, data_scope TEXT NOT NULL,"
                    " granted_by TEXT NOT NULL, granted_at TEXT NOT NULL, revoked_at TEXT)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS fallback_consent_by_task ON fallback_consent_records(task_id, target_connection_id)")
                # T029: additive observability_events/usage_records tables for
                # a pre-T029 store (same precedent as attempt_route_audit
                # above -- CREATE TABLE IF NOT EXISTS in SCHEMA only runs for
                # a brand new database).
                db.execute(
                    "CREATE TABLE IF NOT EXISTS observability_events ("
                    " id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " category TEXT NOT NULL, outcome TEXT NOT NULL,"
                    " basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),"
                    " assignment_ref TEXT, attempt_id TEXT, engine_session_ref TEXT,"
                    " context_generation_id TEXT, config_revision_id TEXT, connection_id TEXT,"
                    " route_engine TEXT, source_event_ref TEXT,"
                    " detail_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS observability_events_by_task ON observability_events(task_id, created_at)")
                db.execute("CREATE INDEX IF NOT EXISTS observability_events_by_attempt ON observability_events(attempt_id)")
                # T036: structured host-visible ACP updates (tool cards,
                # plans, and thoughts) need durable replay alongside the
                # text-only host_transcript.  The table is additive so an
                # existing project keeps its prior session history.
                db.execute(
                    "CREATE TABLE IF NOT EXISTS host_updates ("
                    " session_id TEXT NOT NULL REFERENCES acp_host_sessions(session_id),"
                    " seq INTEGER NOT NULL, update_json TEXT NOT NULL, created_at TEXT NOT NULL,"
                    " UNIQUE(session_id, seq))"
                )
                db.execute("CREATE INDEX IF NOT EXISTS host_updates_by_session ON host_updates(session_id, seq)")
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS observability_events_no_update BEFORE UPDATE ON observability_events "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS observability_events_no_delete BEFORE DELETE ON observability_events "
                    "WHEN NOT EXISTS (SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id) "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )
                db.execute(
                    "CREATE TABLE IF NOT EXISTS usage_records ("
                    " id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),"
                    " attempt_id TEXT, engine_session_ref TEXT NOT NULL, call_sequence INTEGER NOT NULL,"
                    " basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),"
                    " tokens_json TEXT NOT NULL DEFAULT '{}', stop_reason TEXT,"
                    " failed INTEGER NOT NULL DEFAULT 0 CHECK(failed IN (0,1)), created_at TEXT NOT NULL)"
                )
                db.execute("CREATE INDEX IF NOT EXISTS usage_records_by_task ON usage_records(task_id, created_at)")
                db.execute("CREATE INDEX IF NOT EXISTS usage_records_by_attempt ON usage_records(attempt_id, call_sequence)")
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS usage_records_no_update BEFORE UPDATE ON usage_records "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )
                db.execute(
                    "CREATE TRIGGER IF NOT EXISTS usage_records_no_delete BEFORE DELETE ON usage_records "
                    "WHEN NOT EXISTS (SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id) "
                    "BEGIN SELECT RAISE(ABORT, 'immutable'); END"
                )

    def _check_project(self,db,task=None):
        if self.project_root is None:return
        if task is not None:
            rows=db.execute('SELECT project_root FROM tasks WHERE id=?',(task,)).fetchall()
        else:
            # Indexed endpoints establish a single project without scanning all
            # task bodies or walking every metadata row on each hook process.
            rows=db.execute('SELECT project_root FROM tasks ORDER BY project_root LIMIT 1').fetchall()
            rows+=db.execute('SELECT project_root FROM tasks ORDER BY project_root DESC LIMIT 1').fetchall()
        if any(row[0]!=self.project_root for row in rows):raise StoreError('project_storage_mismatch')

    @contextmanager
    def connection(self, *, exclusive=False):
        """Use exclusive access only for maintenance that fences other connections."""
        for suffix in ("","-wal","-shm"):
            try: regular(Path(str(self.path)+suffix))
            except FileNotFoundError:
                if not suffix: raise
        lock_path=self.directory/".access.lock"
        lock_fd=os.open(lock_path,os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
        try:
            regular(lock_path)
            # Exclusive maintenance stays fail-fast. A shared connection waits
            # a bounded time: a short exclusive holder (the bound-task
            # inspector freezes the store for one page) must not fail a live
            # runtime operation; a long one still ends in storage_busy.
            deadline=time.monotonic()+(0 if exclusive else SHARED_ACCESS_WAIT_SECONDS)
            while True:
                try:
                    fcntl.flock(lock_fd,(fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)|fcntl.LOCK_NB); break
                except BlockingIOError:
                    if time.monotonic()>=deadline:raise
                    time.sleep(0.02)
        except BaseException:
            os.close(lock_fd); raise StoreError("storage_busy") from None
        db=None
        try:
            db=sqlite3.connect(self.path,timeout=15,isolation_level=None)
            db.row_factory=sqlite3.Row
            db.execute("PRAGMA foreign_keys=ON"); db.execute("PRAGMA synchronous=FULL")
            yield db
        finally:
            if db is not None:db.close()
            fcntl.flock(lock_fd,fcntl.LOCK_UN); os.close(lock_fd)

    def immediate_write(self, body, *, attempts=8, base_delay=0.02, max_delay=0.4):
        """Run ``body(db)`` in a write transaction whose FIRST table-touching
        statement is ``BEGIN IMMEDIATE``, retrying a contended lock.

        A read (SELECT) before ``BEGIN IMMEDIATE`` on the same connection is a
        read->write upgrade: under concurrency SQLite refuses to wait on it
        (SQLITE_BUSY_SNAPSHOT) and fails *instantly*, bypassing the 15s busy
        timeout -- the source of spurious "database is locked" failures that
        surfaced to workers as an opaque server error. Taking the write lock
        up front lets the busy timeout apply, and a still-contended lock is
        then retried with exponential backoff before giving up. ``body`` must
        be replayable (it may run more than once on retry) and must not commit
        or begin its own transaction -- this owns BEGIN/COMMIT/ROLLBACK."""
        delay = base_delay
        for attempt in range(attempts):
            try:
                with self.connection() as db:
                    db.execute("BEGIN IMMEDIATE")
                    try:
                        result = body(db)
                        db.commit()
                        return result
                    except BaseException:
                        db.rollback()
                        raise
            except sqlite3.OperationalError as error:
                message = str(error).lower()
                if attempt == attempts - 1 or ("lock" not in message and "busy" not in message):
                    raise
                time.sleep(min(delay, max_delay))
                delay *= 2

    def call(self,operation,args,thread_id,parent_thread_id=None,*,original_request=None,steering_source=None,trusted=False,authorization=None):
        """``trusted`` is set only by ``server._dispatch_channeled`` (never
        from a model argument or ``_meta``) after a channel secret has been
        verified. A task once owned through ``task_owners``/``trusted_channels``
        (see ``trusted_channel``/``ownership``) becomes channel-managed and
        rejects every untrusted (legacy Codex ``_meta``) call — reads and
        mutations alike, including a legacy child-thread registering under a
        leaked channel principal as its parent — with ``channel_managed_task``.
        A legacy-created task (no ``task_owners`` row) is unaffected.
        """
        validate(operation,args)
        # Pipeline editions are coordinator-owned durable task state. Reject a
        # worker request before entering the transaction so no draft file,
        # delivery receipt, or other task state is created for the attempt.
        if operation=="create_draft" and args.get("template")=="pipeline" and parent_thread_id is not None:
            raise StoreError("coordinator_pipeline_only")
        # T030 (security audit): governance is a coordinator-owned advisory
        # decision for the whole task, not a per-worker one. Without this
        # check any worker channel could call set_governance and overwrite
        # the task's governance mode/rationale -- reject before entering the
        # transaction so no report/governance row is created for the attempt.
        if operation=="set_governance" and parent_thread_id is not None:
            raise StoreError("coordinator_governance_only")
        if operation=="create_draft" and "request_key" not in args:
            args=dict(args,request_key=str(uuid.uuid4()))
        if operation=="create_task" and parent_thread_id is not None:raise StoreError("child_creation")
        return self._transaction(operation,args,(thread_id,parent_thread_id),original_request,steering_source,trusted,authorization)

    def _transaction(self,operation,args,context,original_request=None,steering_source=None,trusted=False,authorization=None):
        rollbacks=[]; staged=[]
        try:
            with self.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    if authorization is not None:
                        authorization(db)
                    bound=dict(args); thread,parent=context
                    capture=dict(status="not_attempted",reason=None)
                    if operation=="create_task":
                        if self.project_root is not None and args['project_root']!=self.project_root:
                            raise StoreError('project_storage_mismatch')
                        self._check_project(db)
                        if parent is not None: raise StoreError("child_creation")
                        existing=db.execute("SELECT * FROM thread_bindings WHERE thread_id=?",(thread,)).fetchone()
                        if existing is not None:
                            if existing["parent_thread_id"] is not None: raise StoreError("thread_conflict")
                            if not trusted and self._channel_managed(db,existing["task_id"]):
                                raise StoreError("channel_managed_task")
                            receipt=db.execute("SELECT 1 FROM deliveries WHERE scope=? AND operation=? AND delivery_key=?",("new-task:"+thread,operation,args["request_key"])).fetchone()
                            if receipt is None: raise StoreError("task_already_bound")
                    else:
                        bound["task_id"]=self._resolve_thread(db,thread,parent,trusted)
                        self._check_project(db,bound['task_id'])
                        self._recover(db,bound["task_id"])
                        delivery=db.execute("SELECT 1 FROM deliveries WHERE scope=? AND operation=? AND delivery_key=?",(bound["task_id"],operation,args.get("request_key"))).fetchone()
                        current_state=self._binding(db,thread,trusted)["state"]
                        capture_state=args.get("state",current_state)
                        # An accepted delivery recovers its receipt only. Its
                        # historical redactions/state cannot govern later input.
                        if parent is None and steering_source is not None and not delivery and capture_state=="cortex":
                            db.execute("SAVEPOINT source_capture")
                            try:
                                capture=self._capture_steering(db,bound["task_id"],thread,steering_source,
                                                               args.get("redact_values",[]),rollbacks,staged)
                            except StoreError as error:
                                if str(error)!="host_request_unavailable":raise
                                db.execute("ROLLBACK TO source_capture")
                                capture=dict(status="unavailable",reason="host_request_unavailable")
                            db.execute("RELEASE source_capture")
                    if operation=="read_report" and "report_id" not in bound:
                        if "cursor" in bound:
                            decoded=self._decode(bound["cursor"])
                            if not isinstance(decoded,list) or len(decoded)!=5 or decoded[0]!="read" or decoded[1]!=self._cursor_binding(bound["task_id"]) or not isinstance(decoded[2],str):
                                raise StoreError("invalid_cursor","cursor",bound["cursor"],"cursor for this task and document")
                            bound["report_id"]=decoded[2]
                        else:
                            row=db.execute("SELECT id FROM reports WHERE task_id=? AND kind='pipeline'",(bound["task_id"],)).fetchone()
                            if row is None: raise StoreError("pipeline_missing")
                            bound["report_id"]=row["id"]
                    result=self._call(db,operation,bound,context[0],rollbacks,staged,original_request)
                    if operation=="create_task":
                        task=result.pop("task_id")
                        db.execute("INSERT OR IGNORE INTO thread_bindings VALUES (?,NULL,?)",(thread,task))
                        self._binding(db,thread)
                        capture=dict(status=result.pop("_capture_status","complete"),reason=None)
                    else: task=bound["task_id"]
                    if "state" in args and not result.get("replayed",False):
                        if parent is not None: raise StoreError("coordinator_only")
                        if args["state"]=="normal":
                            pause_turn=None
                            if steering_source is not None:
                                project=db.execute("SELECT project_root FROM tasks WHERE id=?",(task,)).fetchone()[0]
                                try:
                                    boundary=steering_source(project,None)
                                    if getattr(boundary,"messages",[]):pause_turn=boundary.messages[-1].get("turn")
                                except StoreError as error:
                                    if str(error)!="host_request_unavailable":raise
                            db.execute("INSERT OR REPLACE INTO source_resets VALUES (?,?,?)",(thread,task,pause_turn))
                        db.execute("UPDATE binding_receipts SET state=?,updated_at=? WHERE thread_id IN (SELECT thread_id FROM thread_bindings WHERE task_id=?)",(args["state"],self._now(),task))
                    pending_thread=parent or thread
                    result.update(binding=self._binding(db,thread,trusted),source_capture=dict(capture,revision=self._revision(db,task),pending_turns=0),
                                  governance=governance_snapshot(db,task))
                    if operation=="list_reports":
                        result.update(self._discovery(db,task,thread,args,trusted))
                    db.commit()
                except BaseException:
                    db.rollback(); self._rollback_files(rollbacks)
                    for path in staged: path.unlink(missing_ok=True)
                    raise
                _fault_injection.checkpoint("after_commit_before_cleanup")
                self._finalize_files(rollbacks)
                db.execute("BEGIN IMMEDIATE"); self._finish_source_deletions(db,task); db.commit()
            return result
        except StoreError: raise
        except (sqlite3.Error,OSError,ValueError,UnicodeError):
            raise StoreError("storage_error") from None

    def bootstrap_source(self,db,task,thread,message_id,text,rollbacks,staged):
        """Publish initial bootstrap input and record its source revision.

        A minimal internal seam for trusted-channel bootstrap (see
        cortex_runtime.bootstrap.bootstrap_task), reusing the same
        publish/record path as create_task's plain-text branch rather than
        duplicating it.
        """
        rid,digest_hex,size=self._publish_text(db,task,"Original user request","Original task context.",
                                                "user request",text,"report",rollbacks,staged)
        self._record_source(db,task,thread,message_id,rid,[])
        return rid,digest_hex,size

    def _source_receipt(self,db,task,thread,source,report):
        if not hasattr(source,"cursor"):
            self._record_source(db,task,thread,"original:"+report,report,[])
            return
        for message in source.messages:
            db.execute("INSERT INTO source_messages VALUES (?,?,?,?,?)",
                       (thread,message["id"],task,report,digest(message["text"].encode())))
            self._record_source(db,task,thread,message["id"],report,message.get("attachments",[]))
            if message.get("turn"):
                db.execute("INSERT OR IGNORE INTO source_turns VALUES (?,?,?,?,?)",(task,thread,message["turn"],report,digest(message["text"].encode())))
        db.execute("INSERT OR REPLACE INTO source_cursors VALUES (?,?,?)",(thread,task,encode(source.cursor)))

    def _capture_steering(self,db,task,thread,reader,redactions,rollbacks,staged):
        row=db.execute("SELECT cursor FROM source_cursors WHERE thread_id=?",(thread,)).fetchone()
        project=db.execute("SELECT project_root FROM tasks WHERE id=?",(task,)).fetchone()[0]
        reset=db.execute("SELECT pause_turn FROM source_resets WHERE thread_id=?",(thread,)).fetchone()
        try: source=reader(project,None if reset else json.loads(row[0]) if row else None)
        except StoreError as error:
            if str(error)!="host_request_unavailable": raise
            return dict(status="unavailable",reason="resume_boundary_unavailable" if reset else "host_request_unavailable")
        completeness=getattr(source,"completeness","complete")
        if completeness=="unavailable": return dict(status="unavailable",reason="host_request_unavailable")
        messages=source.messages
        resume_reason=None
        if reset:
            # The last native message is the only unambiguous current triggering
            # source. Never consume the backlog from before an inactive interval.
            if not messages or reset["pause_turn"] is None or messages[-1].get("turn")==reset["pause_turn"]:
                db.execute("INSERT OR REPLACE INTO source_cursors VALUES (?,?,?)",(thread,task,encode(source.cursor)))
                db.execute("DELETE FROM source_resets WHERE thread_id=?",(thread,))
                return dict(status="partial",reason="inactive_interval_skipped_current_source_unconfirmed")
            messages=messages[-1:]
            resume_reason="inactive_interval_skipped_current_message_only"
        pending=[];seen={}
        for message in messages:
            fingerprint=digest(message["text"].encode())
            prior=db.execute("SELECT digest FROM source_messages WHERE thread_id=? AND message_id=?",
                             (thread,message["id"])).fetchone()
            accepted=prior[0] if prior else seen.get(message["id"])
            if accepted is not None:
                if accepted!=fingerprint:raise StoreError("host_request_unavailable")
                continue
            seen[message["id"]]=fingerprint;pending.append(message)
        for value in redactions:
            if pending and not any(value in message["text"] for message in pending):
                raise StoreError("invalid_redaction")
        for message in pending:
            text=message["text"]
            for value in sorted(redactions,key=len,reverse=True):text=text.replace(value,"[REDACTED]")
            report,_,_=self._publish_text(db,task,"User steering","Exact native user message; chronological source of requirements.",
                                          "user request",text,"report",rollbacks,staged)
            db.execute("INSERT INTO source_messages VALUES (?,?,?,?,?)",
                       (thread,message["id"],task,report,seen[message["id"]]))
            self._record_source(db,task,thread,message["id"],report,message.get("attachments",[]))
            if message.get("turn"):
                db.execute("INSERT OR IGNORE INTO source_turns VALUES (?,?,?,?,?)",(task,thread,message["turn"],report,digest(message["text"].encode())))
        db.execute("INSERT OR REPLACE INTO source_cursors VALUES (?,?,?)",(thread,task,encode(source.cursor)))
        if reset:db.execute("DELETE FROM source_resets WHERE thread_id=?",(thread,))
        return dict(status="partial" if reset else completeness,reason=resume_reason)

    @staticmethod
    def _channel_managed(db,task_id):
        """True once a task has an owner (see ownership.acquire_task_owner_tx).

        Every task bootstrapped through ``bootstrap.bootstrap_task`` acquires
        a ``task_owners`` row in the same transaction that issues its
        coordinator channel, so this is equivalent to "this task is
        channel-managed" without needing to also check ``trusted_channels``.
        """
        return db.execute("SELECT 1 FROM task_owners WHERE task_id=? LIMIT 1",(task_id,)).fetchone() is not None

    @staticmethod
    def _resolve_thread(db,thread,parent,trusted=False):
        existing=db.execute("SELECT * FROM thread_bindings WHERE thread_id=?",(thread,)).fetchone()
        if existing is not None:
            if existing["parent_thread_id"]!=parent: raise StoreError("thread_conflict")
            if not trusted and Store._channel_managed(db,existing["task_id"]):
                raise StoreError("channel_managed_task")
            return existing["task_id"]
        if parent is None: raise StoreError("task_not_bound")
        ancestor=db.execute("SELECT task_id FROM thread_bindings WHERE thread_id=?",(parent,)).fetchone()
        if ancestor is None: raise StoreError("parent_not_bound")
        # Reject a legacy child thread registering under a channel-managed
        # parent (for example a leaked/guessed principal thread id) before
        # any row is written; the surrounding transaction still rolls back
        # on this exception regardless.
        if not trusted and Store._channel_managed(db,ancestor["task_id"]):
            raise StoreError("channel_managed_task")
        db.execute("INSERT INTO thread_bindings VALUES (?,?,?)",(thread,parent,ancestor["task_id"]))
        return ancestor["task_id"]

    def _task_directory(self,db,task):
        row=db.execute("SELECT project_root FROM tasks WHERE id=?",(task,)).fetchone()
        if row is None: raise StoreError("task_not_bound")
        cortex_root=project_task_directory(row["project_root"])
        return private_directory(cortex_root/task)

    def _reports_directory(self,db,task):
        return self._task_directory(db,task)

    def _location(self,db,report):
        row=db.execute("SELECT task_id,kind,filename FROM reports WHERE id=?",(report,)).fetchone()
        if row is None: raise StoreError("not_found","report_id",report,"report in the current task")
        return self._task_directory(db,row["task_id"])/row["filename"]

    def _source(self,db,task,value,must_exist=True,kind=None,error_field="draft_path",error_value=None):
        shown=error_value if error_value is not None else value
        project=db.execute("SELECT project_root FROM tasks WHERE id=?",(task,)).fetchone()[0]
        roots={entry:project_draft_directory(project,entry).resolve() for entry in ("report","pipeline")}
        allowed=[roots[kind]] if kind in roots else list(roots.values())
        raw=Path(value).expanduser()
        if not raw.is_absolute():
            raise StoreError("invalid_draft_path",error_field,shown,"absolute .md path inside the matching project draft directory")
        try:
            info=raw.lstat()
        except FileNotFoundError:
            if must_exist: raise StoreError("draft_missing",error_field,shown,"existing server-created regular .md file") from None
            return None
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_nlink!=1:
            raise StoreError("invalid_draft_path",error_field,shown,"owned regular non-symlink .md file with one link")
        resolved=raw.resolve(strict=True)
        if resolved.parent not in allowed or resolved.suffix.lower()!=".md":
            expected=str(allowed[0]) if len(allowed)==1 else "one advertised project draft directory"
            raise StoreError("invalid_draft_path",error_field,shown,".md file directly inside "+expected)
        os.chmod(resolved,0o600,follow_symlinks=False)
        return resolved

    def _draft(self,db,task,thread,draft_id,published_ok=False):
        row=db.execute("SELECT * FROM drafts WHERE id=?",(draft_id,)).fetchone()
        if row is None:
            raise StoreError("draft_not_found","draft_id",draft_id,"draft_id returned by create_draft to this thread")
        if row["task_id"]!=task or row["owner_thread_id"]!=thread:
            raise StoreError("draft_not_owned","draft_id",draft_id,"draft_id returned by create_draft to this exact native thread")
        if row["published_report_id"] is not None and not published_ok:
            raise StoreError("draft_published","draft_id",draft_id,"new unpublished draft_id")
        return row

    def _draft_source(self,db,task,thread,draft_id):
        draft=self._draft(db,task,thread,draft_id)
        source=self._source(db,task,draft["path"],kind=draft["kind"],error_field="draft_id",error_value=draft_id)
        identity=source.lstat()
        if identity.st_dev!=draft["device"] or identity.st_ino!=draft["inode"]:
            # A different file object than create_draft made, at the same
            # path. Every property that bounds *who* could have put it
            # there still holds, and was just re-checked by _source above:
            # this exact server-issued path, directly inside the owner-only
            # draft directory, a regular file, not a symlink, owned by this
            # uid, with exactly one link. What an inode additionally says is
            # only *how* the bytes arrived -- edited in place, or written
            # elsewhere and moved over -- and moving a new file over the old
            # one is how most write tools write anything. It is not a
            # principal boundary: both require this same uid, which is the
            # worker itself.
            #
            # Found live, and it stopped a task dead: every publish attempt
            # was refused, the worker created a fresh draft and tried again
            # five times in five minutes, then ran an experiment of its own
            # to work out whether shell redirection preserves an inode. The
            # remediation this used to print -- "without deleting,
            # replacing, renaming, or recreating the file" -- is advice an
            # atomic writer cannot follow.
            #
            # The new identity is recorded, so publication, idempotent
            # retry and the reader all keep comparing against the file that
            # is actually there.
            db.execute(
                "UPDATE drafts SET device=?,inode=? WHERE id=?",
                (identity.st_dev,identity.st_ino,draft["id"]),
            )
            draft=self._draft(db,task,thread,draft_id,published_ok=True)
        return draft,source

    def _recover(self,db,task,outcome=None):
        from .cleanup import finish_deletions
        # A committed worker_result is terminal evidence even if an older
        # process died before updating the optional work-graph projection.
        # Repair that projection under this task's existing transaction so a
        # subsequent list/snapshot cannot report a finished worker as running.
        from .delegation.work_graph import WorkGraph
        WorkGraph(self).reconcile_reported_tx(db, task)
        if db.execute("SELECT 1 FROM pending_deletions WHERE task_id=?",(task,)).fetchone():
            finish_deletions(self,db,task)
            # The committed tombstone owns this recovery independently of the
            # rejected operation; do not roll its metadata deletion back.
            db.commit()
            raise StoreError("task_not_bound")
        self._recover_task_files(db,task,outcome)
        self._finish_source_deletions(db,task)

    def _quarantine_directory(self,task_dir):
        """T019: a private per-task home for confirmed-orphan document
        bodies (see ``_quarantine_orphan``). Never listed by any query
        path; created the same owned/no-symlink way as every other
        storage directory."""
        current=task_dir/".orphaned"
        try: current.mkdir(mode=0o700)
        except FileExistsError: pass
        info=current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid!=os.getuid():
            raise StoreError("unsafe_storage","storage_path",str(current),"owned directory without symlinks")
        os.chmod(current,0o700,follow_symlinks=False)
        return current

    def _quarantine_orphan(self,db,task,task_dir,path,outcome=None):
        """T019: move one confirmed-orphan ``.md`` body (file present, no
        ``reports``/``editions`` row referencing it -- for example a report
        or pipeline file whose commit crashed before its metadata row was
        written) into the private quarantine directory instead of deleting
        it, and record a ``task_changes`` row. Quarantine content is never
        read by any query path and never auto-published; recovering it is
        an out-of-band operator action, not a store operation. An
        incomplete ``.pending_*.tmp`` write is unconfirmed content and is
        still deleted, never quarantined (see ``_recover_task_files``)."""
        content=file_digest(path,private=True)
        quarantine=self._quarantine_directory(task_dir)
        target=quarantine/f"{path.name}.{content[:12]}"
        try:
            target.lstat()
        except FileNotFoundError:
            os.rename(path,target); sync_directory(quarantine); sync_directory(task_dir)
        else:
            # Same name+digest already quarantined: this duplicate carries
            # no new content and is discarded without ever overwriting the
            # existing quarantine entry.
            path.unlink(); sync_directory(task_dir)
        reference=path.name+"."+content[:12]
        self._change(db,task,"orphan_quarantined",reference)
        if outcome is not None: outcome["quarantined"].append(dict(task_id=task,name=path.name,digest=content))

    def _recover_task_files(self,db,task,outcome=None):
        for row in db.execute("SELECT id FROM tasks WHERE id=?",(task,)).fetchall():
            task=row["id"]; task_dir=self._task_directory(db,task)
            indexed={r["filename"] for r in db.execute("SELECT filename FROM reports WHERE task_id=? AND kind='report'",(task,))}
            for path in task_dir.iterdir():
                if path.name in (".orphaned",".access.lock"):continue
                if path.name.startswith(".pending_"):
                    regular(path); path.unlink(); sync_directory(task_dir)
                    if outcome is not None: outcome["deleted_temps"].append(str(path))
                    continue
                if path.suffix==".md" and path.name not in indexed and path.name!="pipeline.md" and not path.name.startswith(".backup_pipeline_"):
                    self._quarantine_orphan(db,task,task_dir,path,outcome)
            pipeline=db.execute("SELECT id,filename FROM reports WHERE task_id=? AND kind='pipeline'",(task,)).fetchone()
            backups=list(task_dir.glob(".backup_pipeline_*.md"))
            if pipeline:
                latest=db.execute("SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1",(pipeline["id"],)).fetchone()[0]
                target=task_dir/pipeline["filename"]
                try: matches=target.exists() and self._checked_file(target)["digest"]==latest
                except StoreError as error:
                    if str(error)!="invalid_utf8":raise
                    matches=False
                if not matches:
                    match=next((p for p in backups if file_digest(p)==latest),None)
                    if match is None: raise StoreError("file_conflict","pipeline",str(target),"bytes matching committed SHA-256")
                    os.replace(match,target); sync_directory(task_dir)
                    if outcome is not None: outcome["confirmed_restored"].append(dict(task_id=task,report_id=pipeline["id"]))
                for backup in backups: backup.unlink(missing_ok=True)
            else:
                orphan=task_dir/"pipeline.md"
                if orphan.exists():self._quarantine_orphan(db,task,task_dir,orphan,outcome)
                for backup in backups: regular(backup); backup.unlink()
            for temp in task_dir.glob(".pending_*"):
                regular(temp); temp.unlink()
                if outcome is not None: outcome["deleted_temps"].append(str(temp))

    def reconcile(self,task_id=None):
        """T019: run crash recovery for one task (or, with ``task_id=None``,
        every task in this store) and return a structured report --
        ``confirmed_restored`` (pipeline views repaired from a matching
        backup), ``quarantined`` (orphan document bodies moved aside),
        ``deleted_temps`` (unconfirmed ``.pending_*`` writes removed), and
        ``indeterminate`` (a task left as a raised conflict rather than
        silently resolved, for example a pipeline digest mismatch with no
        matching backup). This is the same recovery every store call
        already performs for its own task before acting (see ``_recover``
        in ``_transaction``); callers such as
        ``acp.production_runtime.ProductionOrchestratingRuntime`` invoke it
        once, best-effort, at first use per project so crash leftovers are
        surfaced/quarantined before any request needs them, not only lazily
        on the next call touching that exact task."""
        outcome=dict(confirmed_restored=[],quarantined=[],deleted_temps=[],indeterminate=[])
        with self.connection() as db:
            if task_id is not None:
                tasks=[r["id"] for r in db.execute("SELECT id FROM tasks WHERE id=?",(task_id,)).fetchall()]
            else:
                tasks=[r["id"] for r in db.execute("SELECT id FROM tasks").fetchall()]
        for tid in tasks:
            with self.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    self._recover(db,tid,outcome)
                    db.commit()
                except StoreError as error:
                    db.rollback()
                    if str(error)!="task_not_bound":
                        outcome["indeterminate"].append(dict(task_id=tid,reason=str(error)))
        return outcome

    def _finish_source_deletions(self,db,task):
        for row in db.execute("SELECT * FROM pending_source_deletions WHERE task_id=?",(task,)).fetchall():
            source=self._source(db,row["task_id"],row["path"],must_exist=False,error_field="draft_id",error_value=row["draft_id"])
            if source is not None:
                if file_digest(source,private=False,validate_utf8=True)!=row["digest"]:
                    raise StoreError("draft_conflict","draft_id",row["draft_id"],"original accepted SHA-256 "+row["digest"])
                source.unlink(); sync_directory(source.parent)
            db.execute("DELETE FROM pending_source_deletions WHERE task_id=? AND draft_id=?",(row["task_id"],row["draft_id"]))

    @staticmethod
    def _rollback_files(items):
        for target,backup in reversed(items):
            target.unlink(missing_ok=True)
            if backup is not None and backup.exists(): os.replace(backup,target)
            sync_directory(target.parent)

    @staticmethod
    def _finalize_files(items):
        for target,backup in items:
            if backup is not None: backup.unlink(missing_ok=True)
            sync_directory(target.parent)

    def _call(self,db,operation,args,creation_thread,rollbacks,staged,original_request=None):
        task=args.get("task_id")
        if task and db.execute("SELECT 1 FROM tasks WHERE id=?",(task,)).fetchone() is None: raise StoreError("task_not_bound")
        if operation=="list_reports": return self._list(db,args)
        if operation=="read_report": return self._read(db,args)
        if operation=="read_draft": return self._read_draft(db,args,creation_thread)
        scope=task or "new-task:"+creation_thread
        arg_hash=digest(encode(args).encode())
        previous=db.execute("SELECT digest,content_digest,response FROM deliveries WHERE scope=? AND operation=? AND delivery_key=?",(scope,operation,args["request_key"])).fetchone()
        if previous:
            if previous["digest"]!=arg_hash: raise StoreError("delivery_conflict","request_key",args["request_key"],"unused key or exact prior arguments")
            if task:
                # T029: this is an idempotent replay of an already-confirmed
                # delivery, not a new mutation -- but it is a genuine
                # *duplicate publication attempt* for the metric (see
                # observability.metrics._duplicate_publications). Recorded
                # in this same transaction; never for the pre-task-creation
                # scope ("new-task:"+thread has no tasks(id) row yet).
                from .observability.events import record_duplicate_publication_tx
                record_duplicate_publication_tx(db,task,operation,args["request_key"],created_at=self._now())
            if operation=="write_report":
                draft=self._draft(db,task,creation_thread,args["draft_id"],published_ok=True)
                source=self._source(db,task,draft["path"],must_exist=False,kind=draft["kind"],error_field="draft_id",error_value=args["draft_id"])
                if source is not None:
                    if file_digest(source,private=False,validate_utf8=True,error_field="draft_id",error_value=args["draft_id"])!=previous["content_digest"]:
                        raise StoreError("draft_conflict","draft_id",args["draft_id"],"bytes matching accepted SHA-256 "+previous["content_digest"])
                    db.execute("INSERT OR IGNORE INTO pending_source_deletions VALUES (?,?,?,?)",(task,args["draft_id"],str(source),previous["content_digest"]))
            return dict(json.loads(previous["response"]),replayed=True)
        content_hash=""
        if operation=="create_task":
            source=original_request() if callable(original_request) else original_request
            provenance=source
            if not isinstance(source,str) or (not source and not getattr(source,"messages",[])):
                raise StoreError("host_request_unavailable")
            for value in args.get("redact_values",[]):
                if value not in source:raise StoreError("invalid_redaction")
            for value in sorted(args.get("redact_values",[]),key=len,reverse=True):
                source=source.replace(value,"[REDACTED]")
            project=Path(args["project_root"])
            if not project.is_absolute() or not project.is_dir() or str(project.resolve())!=args["project_root"]:
                raise StoreError("invalid_project","project_root",args["project_root"],"absolute existing canonical directory")
            project_draft_directory(project,"report"); project_draft_directory(project,"pipeline")
            task=unique_identifier(db,"t_"); db.execute("INSERT INTO tasks VALUES (?,?,?)",(task,str(project),self._now()))
            if getattr(provenance,"messages",[]):
                first=None
                for message in provenance.messages:
                    body=message.get("archive_text",message["text"])
                    for value in sorted(args.get("redact_values",[]),key=len,reverse=True):body=body.replace(value,"[REDACTED]")
                    ref,hashed,size=self._publish_text(db,task,"Original user request" if first is None else "Initial user message","Exact initial native source.","user request",body,"report",rollbacks,staged)
                    db.execute("INSERT INTO source_messages VALUES (?,?,?,?,?)",(creation_thread,message["id"],task,ref,digest(message["text"].encode())))
                    self._record_source(db,task,creation_thread,message["id"],ref,message.get("attachments",[]))
                    if message.get("turn"):db.execute("INSERT OR IGNORE INTO source_turns VALUES (?,?,?,?,?)",(task,creation_thread,message["turn"],ref,digest(message["text"].encode())))
                    if first is None:first=(ref,hashed)
                rid,content_hash=first
                db.execute("INSERT OR REPLACE INTO source_cursors VALUES (?,?,?)",(creation_thread,task,encode(provenance.cursor)))
            else:
                rid,content_hash,size=self._publish_text(db,task,"Original user request","Original task context.","user request",source,"report",rollbacks,staged)
                self._source_receipt(db,task,creation_thread,provenance,rid)
            result=dict(task_id=task,original_report_id=rid,original_request_sha256=content_hash,_capture_status=getattr(provenance,"completeness","complete"))
        elif operation=="set_governance":
            gid=identifier("g_")
            rid,content_hash,size=self._publish_text(db,task,"Governance: "+args["mode"],"Advisory choice and rationale.","coordinator",args["mode"]+"\n\n"+args["rationale"],"report",rollbacks,staged)
            db.execute("INSERT INTO governance VALUES (?,?,?,?)",(gid,task,args["mode"],rid))
            result=dict(governance_id=gid,report_id=rid)
        elif operation=="create_draft":
            draft_id=unique_identifier(db,"d_")
            kind="pipeline" if args["template"]=="pipeline" else "report"
            directory=project_draft_directory(db.execute("SELECT project_root FROM tasks WHERE id=?",(task,)).fetchone()[0],kind)
            path=directory/(draft_id+".md")
            body=draft_template(args["template"],draft_id)
            _fault_injection.checkpoint("draft_before_write")
            fd=os.open(path,os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_NOFOLLOW,0o600)
            staged.append(path)
            with os.fdopen(fd,"wb") as output:
                output.write(body); output.flush(); os.fsync(output.fileno())
            _fault_injection.checkpoint("draft_after_fsync")
            sync_directory(directory)
            identity=path.lstat()
            markdown=body.decode("utf-8")
            replaceable_markers=[line for line in markdown.splitlines()
                                 if (line.startswith("<!--") and line.endswith("-->")
                                     or line.encode("utf-8") in PIPELINE_PLACEHOLDERS)]
            db.execute("INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode) VALUES (?,?,?,?,?,?,?,?,?)",
                       (draft_id,task,creation_thread,kind,args["template"],str(path),self._now(),identity.st_dev,identity.st_ino))
            db.execute("INSERT INTO draft_revisions VALUES (?,?)",(draft_id,self._revision(db,task)))
            result=dict(draft_id=draft_id,draft_path=str(path),kind=kind,
                        template=args["template"],
                        required_first_line=f"Cortex draft ID: `{draft_id}`",
                        edit_instruction="Fill this owned draft with draft_write. Preserve required_first_line, replace every listed placeholder and retain complete UTF-8 Markdown. The runtime owns pipeline editions.",
                        replaceable_markers=replaceable_markers,
                        required_replacement_count=len(replaceable_markers),
                        markdown=markdown,total_characters=len(markdown),
                        sha256=digest(body))
            content_hash=digest(body)
        else:
            draft,source=self._draft_source(db,task,creation_thread,args["draft_id"])
            rid,stored_hash,size,content_hash=self._publish_source(db,task,args,draft,source,rollbacks,staged)
            revision=args.get("source_revision",db.execute("SELECT source_revision FROM draft_revisions WHERE draft_id=?",(draft["id"],)).fetchone()[0])
            if revision>self._revision(db,task): raise StoreError("invalid_source_revision")
            sequence=db.execute("SELECT MAX(sequence) FROM editions WHERE report_id=?",(rid,)).fetchone()[0]
            db.execute("INSERT INTO report_provenance VALUES (?,?,?)",(sequence,revision,encode(args.get("artifacts",[]))))
            self._change(db,task,"report",rid)
            result=dict(report_id=rid,summary=args["summary"],size_bytes=size,sha256=stored_hash,source_revision=revision,artifacts=args.get("artifacts",[]))
        db.execute("INSERT INTO deliveries VALUES (?,?,?,?,?,?)",(scope,operation,args["request_key"],arg_hash,content_hash,encode(result)))
        return dict(result,replayed=False)

    @staticmethod
    def _now(): return datetime.now(timezone.utc).isoformat(timespec="microseconds")

    def _publish_text(self,db,task,title,summary,author,markdown,kind,rollbacks,staged):
        return self._publish_blocks(db,task,title,summary,author,(markdown.encode("utf-8"),),kind,rollbacks,staged)

    def _publish_source(self,db,task,args,draft,source,rollbacks,staged):
        kind=draft["kind"]
        existing=db.execute("SELECT id FROM reports WHERE task_id=? AND kind='pipeline'",(task,)).fetchone() if kind=="pipeline" else None
        now=self._now(); previous=None
        if existing:
            rid=existing["id"]; target=self._location(db,rid)
            previous=db.execute("SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1",(rid,)).fetchone()[0]
        else:
            rid=unique_identifier(db,"r_"); target=self._task_directory(db,task)/("pipeline.md" if kind=="pipeline" else rid+".md")
        directory=target.parent; temporary=directory/(identifier(".pending_")+".tmp")
        staged.append(temporary); stored=hashlib.sha256(); content=hashlib.sha256(); size=0
        marker=f"Cortex draft ID: `{draft['id']}`\n\n".encode()
        observed_prefix=bytearray()
        template_lines=draft_template(draft["template"],draft["id"]).splitlines()
        placeholders=tuple(line for line in template_lines
                           if line in PIPELINE_PLACEHOLDERS
                           or line.startswith(b"<!--") and line.endswith(b"-->"))
        guidance_placeholder=None
        placeholder_tail=b""
        placeholder_window=max(map(len,placeholders),default=1)-1
        decoder=codecs.getincrementaldecoder("utf-8")()
        _fault_injection.checkpoint("before_temp_write")
        fd=os.open(temporary,os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_NOFOLLOW,0o600)
        try:
            with os.fdopen(fd,"wb") as output:
                try:
                    for block in file_blocks(source,private=False):
                        if len(observed_prefix)<len(marker):
                            observed_prefix.extend(block[:len(marker)-len(observed_prefix)])
                        decoder.decode(block); output.write(block); content.update(block); stored.update(block); size+=len(block)
                        if placeholders and guidance_placeholder is None:
                            candidate=placeholder_tail+block
                            guidance_placeholder=next((token for token in placeholders if token in candidate),None)
                            placeholder_tail=candidate[-placeholder_window:]
                    decoder.decode(b"",final=True)
                except UnicodeDecodeError:
                    raise StoreError("invalid_utf8","draft_id",draft["id"],"complete valid UTF-8 Markdown file") from None
                if bytes(observed_prefix)!=marker:
                    raise StoreError("draft_marker_missing","draft_id",draft["id"],"unchanged first-line marker "+marker.decode().strip())
                if guidance_placeholder is not None:
                    token=guidance_placeholder.decode()
                    raise StoreError("draft_guidance_remaining","draft_markdown",token,
                                     "every exact template placeholder replaced with complete Markdown",
                                     "Replace the exact received placeholder in the existing draft and retry the same write.")
                if existing:
                    separator=b"\n\n---\n\n"; output.write(separator); stored.update(separator); size+=len(separator)
                    old=hashlib.sha256()
                    for block in file_blocks(target): old.update(block); output.write(block); stored.update(block); size+=len(block)
                    if old.hexdigest()!=previous: raise StoreError("file_conflict","pipeline",str(target),"committed SHA-256 "+previous)
                output.flush(); os.fsync(output.fileno())
            _fault_injection.checkpoint("after_temp_fsync")
            sync_directory(directory)
            backup=None
            if existing:
                backup=directory/(identifier(".backup_pipeline_")+".md")
                with open(backup,"xb") as copy:
                    for block in file_blocks(target): copy.write(block)
                    copy.flush(); os.fsync(copy.fileno())
                os.chmod(backup,0o600); sync_directory(directory)
            _fault_injection.checkpoint("after_backup_copy")
            os.replace(temporary,target); staged.remove(temporary)
            rollbacks.append((target,backup)); sync_directory(directory)
            _fault_injection.checkpoint("after_replace_before_commit")
        except BaseException:
            temporary.unlink(missing_ok=True); raise
        if not existing: db.execute("INSERT INTO reports VALUES (?,?,?,?,?)",(rid,task,kind,target.name,now))
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,?,?,?,?,?,?)",(rid,args["title"],args["summary"],args["author"],now,stored.hexdigest(),size))
        db.execute("UPDATE drafts SET published_report_id=?,published_digest=? WHERE id=?",
                   (rid,content.hexdigest(),draft["id"]))
        db.execute("INSERT INTO pending_source_deletions VALUES (?,?,?,?)",
                   (task,draft["id"],str(source),content.hexdigest()))
        return rid,stored.hexdigest(),size,content.hexdigest()

    def _publish_blocks(self,db,task,title,summary,author,blocks,kind,rollbacks,staged):
        existing=db.execute("SELECT id FROM reports WHERE task_id=? AND kind='pipeline'",(task,)).fetchone() if kind=="pipeline" else None
        now=self._now(); previous=None
        if existing:
            rid=existing["id"]; target=self._location(db,rid)
            previous=db.execute("SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1",(rid,)).fetchone()[0]
        else:
            rid=unique_identifier(db,"r_"); filename="pipeline.md" if kind=="pipeline" else rid+".md"
            target=self._task_directory(db,task)/filename
            db.execute("INSERT INTO reports VALUES (?,?,?,?,?)",(rid,task,kind,filename,now))
        temporary=target.parent/(identifier(".pending_")+".tmp"); staged.append(temporary)
        hashed=hashlib.sha256(); size=0
        _fault_injection.checkpoint("before_temp_write")
        fd=os.open(temporary,os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_NOFOLLOW,0o600)
        try:
            with os.fdopen(fd,"wb") as output:
                for block in blocks: output.write(block); hashed.update(block); size+=len(block)
                if existing:
                    separator=b"\n\n---\n\n"; output.write(separator); hashed.update(separator); size+=len(separator)
                    old=hashlib.sha256()
                    for block in file_blocks(target): old.update(block); output.write(block); hashed.update(block); size+=len(block)
                    if old.hexdigest()!=previous: raise StoreError("file_conflict","pipeline",str(target),"committed SHA-256 "+previous)
                output.flush(); os.fsync(output.fileno())
            _fault_injection.checkpoint("after_temp_fsync")
            sync_directory(target.parent)
            backup=None
            if existing:
                backup=target.parent/(identifier(".backup_pipeline_")+".md")
                with open(backup,"xb") as copy:
                    for block in file_blocks(target): copy.write(block)
                    copy.flush(); os.fsync(copy.fileno())
                os.chmod(backup,0o600); sync_directory(target.parent)
            _fault_injection.checkpoint("after_backup_copy")
            os.replace(temporary,target); staged.remove(temporary)
            rollbacks.append((target,backup)); sync_directory(target.parent)
            _fault_injection.checkpoint("after_replace_before_commit")
        except BaseException:
            temporary.unlink(missing_ok=True); raise
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,?,?,?,?,?,?)",(rid,title,summary,author,now,hashed.hexdigest(),size))
        return rid,hashed.hexdigest(),size

    @staticmethod
    def _signature(info):
        return (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns,info.st_ctime_ns)

    def _checked_file(self,path,private=True):
        """Cache only metadata and sparse UTF-8 byte offsets, never document bodies."""
        key=str(path); signature=self._signature(regular(path,private))
        with self._cache_lock:
            cached=self._file_cache.get(key)
            if cached is not None and cached["identity"]==signature:
                self._file_cache.move_to_end(key); return cached
        hashed=hashlib.sha256(); decoder=codecs.getincrementaldecoder("utf-8")()
        characters=0; offset=0; points=[(0,0)]; stride=65536
        try:
            for block in file_blocks(path,private):
                hashed.update(block); characters+=len(decoder.decode(block)); offset+=len(block)
                if offset-points[-1][1]>=stride:
                    points.append((characters,offset-len(decoder.getstate()[0])))
                    if len(points)>1024:
                        points=points[::2];stride*=2
            characters+=len(decoder.decode(b"",final=True))
        except UnicodeDecodeError:
            raise StoreError("invalid_utf8","path",str(path),"complete valid UTF-8 Markdown") from None
        if self._signature(regular(path,private))!=signature: raise StoreError("file_conflict")
        checked=dict(identity=signature,digest=hashed.hexdigest(),characters=characters,points=points)
        with self._cache_lock:
            self._file_cache[key]=checked
            while len(self._file_cache)>128:self._file_cache.popitem(last=False)
        return checked

    def _file_page(self,path,checked,start,limit,private=True):
        if start>checked["characters"]: raise StoreError("invalid_cursor")
        point=checked["points"][bisect.bisect_right(checked["points"],(start,float("inf")))-1]
        if self._signature(regular(path,private))!=checked["identity"]: raise StoreError("file_conflict")
        fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
        with os.fdopen(fd,"rb") as stream:
            if self._signature(os.fstat(stream.fileno()))!=checked["identity"]: raise StoreError("file_conflict")
            stream.seek(point[1]); decoder=codecs.getincrementaldecoder("utf-8")(); pieces=[]; count=0
            skip=start-point[0];needed=skip+limit
            while count<needed:
                block=stream.read(min(65536,4*(needed-count)))
                if not block: break
                text=decoder.decode(block)
                left=max(0,skip-count);right=min(len(text),needed-count)
                if right>left:pieces.append(text[left:right])
                count+=len(text)
            if self._signature(os.fstat(stream.fileno()))!=checked["identity"]: raise StoreError("file_conflict")
        if self._signature(regular(path,private))!=checked["identity"]: raise StoreError("file_conflict")
        return "".join(pieces)

    @staticmethod
    def _revision(db,task):
        return db.execute("SELECT COALESCE(MAX(revision),0) FROM source_revisions WHERE task_id=?",(task,)).fetchone()[0]

    def _binding(self,db,thread,trusted=False):
        row=db.execute("SELECT * FROM thread_bindings WHERE thread_id=?",(thread,)).fetchone()
        if row is None:raise StoreError("task_not_bound")
        prior=db.execute("SELECT receipt,state FROM binding_receipts WHERE thread_id=?",(thread,)).fetchone()
        if prior is None:
            state="cortex"
            if row["parent_thread_id"]:
                parent=db.execute("SELECT state FROM binding_receipts WHERE thread_id=?",(row["parent_thread_id"],)).fetchone()
                if parent:state=parent[0]
            db.execute("INSERT INTO binding_receipts VALUES (?,?,?,?)",(thread,"b_"+uuid.uuid4().hex,state,self._now()))
            prior=db.execute("SELECT receipt,state FROM binding_receipts WHERE thread_id=?",(thread,)).fetchone()
        if trusted:
            # The channel path never discloses the principal thread id or its
            # durable receipt (either is usable as an impersonation
            # credential on the legacy path); only participation state is a
            # legitimate model-visible signal here.
            return dict(receipt="channel-managed",thread_id="channel-managed",parent_thread_id=None,state=prior["state"])
        return dict(receipt=prior["receipt"],thread_id=thread,parent_thread_id=row["parent_thread_id"],state=prior["state"])

    def _change(self,db,task,kind,reference=None):
        db.execute("INSERT INTO task_changes(task_id,kind,reference,created_at) VALUES (?,?,?,?)",(task,kind,reference,self._now()))

    def _record_source(self,db,task,thread,message_id,report,attachments,turn=None):
        revision=self._revision(db,task)+1
        db.execute("INSERT INTO source_revisions VALUES (?,?,?,?,?,?)",(task,revision,thread,message_id,report,encode(attachments)))
        self._change(db,task,"source",report)
        sequence=db.execute("SELECT MAX(sequence) FROM editions WHERE report_id=?",(report,)).fetchone()[0]
        db.execute("INSERT OR REPLACE INTO report_provenance VALUES (?,?,?)",(sequence,revision,"[]"))
        return revision

    def _discovery(self,db,task,thread,args,trusted=False):
        limit=args.get("limit",25)
        after=args.get("changes_after",0)
        changes=db.execute("SELECT c.sequence,c.kind,c.reference,c.created_at,NULL AS event_name,NULL AS hook_thread_id,NULL AS hook_metadata FROM task_changes c WHERE c.task_id=? AND c.sequence>? ORDER BY c.sequence LIMIT ?",(task,after,limit+1)).fetchall()
        page=changes[:limit]
        drafts=db.execute("SELECT id AS draft_id,path AS draft_path,kind,template,created_at FROM drafts WHERE task_id=? AND owner_thread_id=? AND published_report_id IS NULL AND id>? ORDER BY id LIMIT ?",(task,thread,args.get("drafts_after",""),limit+1)).fetchall()
        own=drafts[:limit]
        return dict(changes=[self._change_metadata(row,thread,trusted) for row in page],changes_next=page[-1]["sequence"] if len(changes)>limit else None,
                    own_drafts=[dict(row) for row in own],drafts_next=own[-1]["draft_id"] if len(drafts)>limit else None)

    @staticmethod
    def _change_metadata(row,thread=None,trusted=False):
        result={key:row[key] for key in ("sequence","kind","reference","created_at")}
        result["observation"]=None
        if row["hook_metadata"] is None:return result
        metadata=json.loads(row["hook_metadata"])
        if not isinstance(metadata,dict):return result
        def bounded(name,maximum=256):
            value=metadata.get(name)
            return value if isinstance(value,str) and len(value)<=maximum else None
        paths=metadata.get("changed_paths",[])
        if not isinstance(paths,list):paths=[]
        selected=[];characters=0
        for path in paths:
            if not isinstance(path,str) or not path or len(path)>4096:continue
            if len(selected)>=16 or characters+len(path)>4096:break
            selected.append(path);characters+=len(path)
        actor=bounded("actor_thread_id")
        if actor!=row["hook_thread_id"]:actor=None
        scope="actor" if metadata.get("actor_scope")=="actor" and actor else "session"
        exit_code=metadata.get("exit_code")
        status=metadata.get("status")
        if status not in {"failed","exited","running","unverified","completed"}:status="unverified"
        # T030 (security audit): a hook-recorded actor_thread_id is the same
        # internal principal thread id trusted_channel deliberately never
        # discloses to a channel-managed caller (see ``_binding`` above,
        # which masks it to "channel-managed" for exactly this reason -- a
        # leaked principal is a usable impersonation credential on the
        # unfenced legacy path). A channel-managed caller therefore only
        # ever learns whether an observation was its *own* action, never any
        # other thread's raw id; an untrusted/legacy caller keeps the prior
        # unmasked behavior (its own thread id is already directly visible
        # elsewhere on that path).
        if trusted:
            actor_thread_id=None
            actor_is_self=actor is not None and actor==thread
        else:
            actor_thread_id=actor
            actor_is_self=actor is not None and actor==thread
        result["observation"]=dict(source="hook",event_name=row["event_name"],actor_scope=scope,
            actor_thread_id=actor_thread_id,actor_is_self=actor_is_self,parent_session_id=bounded("parent_session_id"),
            binding_origin=bounded("binding_origin",32),tool_name=bounded("tool_name",128),
            exit_code=exit_code if type(exit_code) is int else None,
            command_session_id=bounded("command_session_id"),status=status,
            truncated=metadata.get("truncated") if type(metadata.get("truncated")) is bool else None,
            error=metadata.get("error") if type(metadata.get("error")) is bool else None,
            changed_paths=selected,changed_paths_total=len(paths),changed_paths_complete=len(selected)==len(paths))
        return result

    @staticmethod
    def _metadata(row):
        return dict(report_id=row["id"],title=row["title"],summary=row["summary"],author=row["author"],created_at=row["created_at"],updated_at=row["updated_at"],kind=row["kind"],size_bytes=row["size_bytes"],sha256=row["digest"],source_revision=row["source_revision"],artifacts=json.loads(row["artifacts"]),source_attachments=json.loads(row["source_attachments"]))

    @staticmethod
    def _cursor(value):
        tag=value[0]
        number=lambda item: ("0" if item==0 else _base36(item))
        if tag=="list": return ".".join(("cl",value[1],number(value[2]),number(value[3])))
        if tag=="read": return ".".join(("cr",value[1],value[2][2:],value[3][:12],number(value[4])))
        if tag=="draft": return ".".join(("cd",value[1],value[2][2:],value[3][:12],number(value[4])))
        raise StoreError("invalid_cursor","cursor_kind",tag,"list, read, or draft")

    @staticmethod
    def _cursor_binding(task):
        return hashlib.sha256(("cortex-cursor\0"+task).encode()).hexdigest()[:12]

    @staticmethod
    def _decode(value):
        try:
            parts=value.split(".")
            if len(parts)==4 and parts[0]=="cl":
                return ["list",parts[1],int(parts[2],36),int(parts[3],36)]
            if len(parts)==5 and parts[0] in {"cr","cd"}:
                prefix="r_" if parts[0]=="cr" else "d_"
                return ["read" if parts[0]=="cr" else "draft",parts[1],prefix+parts[2],parts[3],int(parts[4],36)]
            raise ValueError
        except (AttributeError,ValueError,TypeError):
            raise StoreError("invalid_cursor","cursor",value,"exact compact server-issued cursor") from None

    def _list(self,db,args):
        task=args["task_id"]
        binding=self._cursor_binding(task)
        ceiling=db.execute("SELECT COALESCE(MAX(e.sequence),0) FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=?",(task,)).fetchone()[0]
        before=ceiling+1
        if "cursor" in args:
            decoded=self._decode(args["cursor"])
            if not isinstance(decoded,list) or len(decoded)!=4: raise StoreError("invalid_cursor","cursor",args["cursor"],"catalogue cursor for this task")
            tag,bound,before,high=decoded
            if tag!="list" or bound!=binding or type(before) is not int or type(high) is not int or not 0<before<=high+1<=ceiling+1: raise StoreError("invalid_cursor","cursor",args["cursor"],"catalogue cursor for this task")
            ceiling=high
        rows=db.execute('''SELECT r.*,e.*,COALESCE(p.source_revision,0) AS source_revision,COALESCE(p.artifacts,'[]') AS artifacts,COALESCE((SELECT attachments FROM source_revisions s WHERE s.report_id=r.id ORDER BY revision DESC LIMIT 1),'[]') AS source_attachments FROM reports r JOIN editions e ON e.report_id=r.id LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence
            WHERE r.task_id=? AND e.sequence=(SELECT MAX(v.sequence) FROM editions v WHERE v.report_id=r.id AND v.sequence<=?)
            AND e.sequence<? ORDER BY e.sequence DESC LIMIT ?''',(task,ceiling,before,args.get("limit",25)+1)).fetchall()
        page=rows[:args.get("limit",25)]
        cursor=self._cursor(["list",binding,page[-1]["sequence"],ceiling]) if len(rows)>len(page) else None
        return dict(reports=[self._metadata(row) for row in page],next_cursor=cursor)

    def _read_draft(self,db,args,thread):
        draft,source=self._draft_source(db,args["task_id"],thread,args["draft_id"])
        start=0; expected=None
        if "cursor" in args:
            decoded=self._decode(args["cursor"])
            if not isinstance(decoded,list) or len(decoded)!=5:
                raise StoreError("invalid_cursor","cursor",args["cursor"],"cursor for this draft")
            tag,task,draft_id,expected,start=decoded
            if tag!="draft" or task!=self._cursor_binding(args["task_id"]) or draft_id!=draft["id"] or type(start) is not int or start<0:
                raise StoreError("invalid_cursor","cursor",args["cursor"],"cursor for this draft")
        checked=self._checked_file(source,private=False)
        total=checked["characters"]; current=checked["digest"]; end=start+args.get("limit",4000)
        if expected is not None and expected!=current[:12]:
            raise StoreError("draft_cursor_stale","cursor",args["cursor"],"cursor for the current draft digest")
        if start>total:raise StoreError("invalid_cursor","cursor",args.get("cursor"),"position within the draft")
        markdown=self._file_page(source,checked,start,args.get("limit",4000),private=False)
        cursor=self._cursor(["draft",self._cursor_binding(args["task_id"]),draft["id"],current,end]) if end<total else None
        return dict(draft_id=draft["id"],draft_path=str(source),kind=draft["kind"],template=draft["template"],
                    markdown=markdown,total_characters=total,sha256=current,next_cursor=cursor)

    def _read(self,db,args):
        row=db.execute("SELECT r.*,e.*,COALESCE(p.source_revision,0) AS source_revision,COALESCE(p.artifacts,'[]') AS artifacts,COALESCE((SELECT attachments FROM source_revisions s WHERE s.report_id=r.id ORDER BY revision DESC LIMIT 1),'[]') AS source_attachments FROM reports r JOIN editions e ON e.report_id=r.id LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence WHERE r.task_id=? AND r.id=? ORDER BY e.sequence DESC LIMIT 1",(args["task_id"],args["report_id"])).fetchone()
        if row is None: raise StoreError("not_found","report_id",args["report_id"],"report in the current task")
        start=0
        if "cursor" in args:
            decoded=self._decode(args["cursor"])
            if not isinstance(decoded,list) or len(decoded)!=5: raise StoreError("invalid_cursor","cursor",args["cursor"],"read cursor for this document")
            tag,task,rid,expected,start=decoded
            if tag!="read" or task!=self._cursor_binding(args["task_id"]) or rid!=row["id"] or type(start) is not int or start<0: raise StoreError("invalid_cursor","cursor",args["cursor"],"read cursor for this document")
            if expected!=row["digest"][:12]:
                if row["kind"]=="pipeline":
                    raise StoreError("cursor_stale","cursor",args["cursor"],"cursor for current pipeline digest")
                raise StoreError("invalid_cursor","cursor",args["cursor"],"exact cursor for this immutable report")
        path=self._location(db,row["id"])
        try: checked=self._checked_file(path)
        except StoreError as error:
            if str(error)!="invalid_utf8": raise
            raise StoreError("file_conflict","report_id",row["id"],"valid UTF-8 matching committed digest") from None
        if checked["digest"]!=row["digest"]: raise StoreError("file_conflict","report_id",row["id"],"committed SHA-256 "+row["digest"])
        total=checked["characters"]; end=start+args.get("limit",4000)
        markdown=self._file_page(path,checked,start,args.get("limit",4000))
        if start>total: raise StoreError("invalid_cursor","cursor",args.get("cursor"),"position within the document")
        cursor=self._cursor(["read",self._cursor_binding(args["task_id"]),row["id"],row["digest"],end]) if end<total else None
        return dict(self._metadata(row),markdown=markdown,total_characters=total,next_cursor=cursor)
