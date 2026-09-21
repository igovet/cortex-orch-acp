"""Explicit offline format-12 upgrade/rollback safety for one project store.

Three operations, all offline (stopped-access) and never touching Markdown:

- ``plan()``:    read-only; reports the exact intended change, or a no-op.
- ``migrate()``: upgrades old format 10/11 stores to ACP-only format 12;
  verified backup first, then one atomic SQLite transaction, then a
  ``format_history`` row. Format 12 removes retired Codex hook tables.
- ``rollback()``: restores a verified, format-compatible backup file exactly.
  Never recreates a second storage root and never deletes
  ``.cortex``; refuses an unverifiable or incompatible backup.

See docs/acp/format-migration.md for the compatibility and backup procedure.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import stat
import time
import uuid

from . import _fault_injection
from .acp.sessions import ACP_SESSION_SCHEMA
from .catalog.store import CATALOG_SCHEMA
from .configuration.store import CONFIGURATION_SCHEMA
from .contracts import StoreError
from .delegation.store import DELEGATION_SCHEMA
from .delegation.work_graph import WORK_GRAPH_SCHEMA
from .delegation.events_store import TASK_EVENTS_SCHEMA
from .orchestration.state import SCHEMA as RUNTIME_SCHEMA
from .delegation.capacity_store import CAPACITY_SCHEMA
from .delegation.evidence_store import EVIDENCE_SCHEMA
from .routing.store import ROUTING_SCHEMA
from .store import APPLICATION_ID, SCHEMA, private_directory, regular, sync_directory
from .trusted_channel import IDENTITY_SCHEMA

# Formats this exact runtime build knows how to recognize at all: 10 (the
# pre-T005 layout), 11 (the pre-ACP-hook-removal layout), and 12 (the only
# format any current store.py-created database uses). Anything else -- a
# backup or live file from an unrelated/newer/older application_id or
# user_version -- is never opened for rollback or reported by plan().
_KNOWN_VERSIONS = (10, 11, 12)
_LEGACY_HOOK_TABLES = (
    "hook_pending_sources", "hook_events", "hook_hints",
    "hook_agent_bindings", "hook_command_sessions",
)

# A normal Store opener may race another opener while SQLite is switching the
# journal/WAL state during initialization.  The format probe must wait for a
# short, bounded interval in that case; a zero timeout turns an otherwise
# harmless opener race into an uncaught ``sqlite3.OperationalError``.
_LIVE_HEADER_PROBE_ATTEMPTS = 8
_LIVE_HEADER_PROBE_DELAY = 0.02


def _read_live_header(source):
    """Read the committed live header, retrying only transient lock errors.

    This probe is intentionally read-only.  A persistent lock is reported as
    ``storage_busy`` and an invalid/ambiguous header remains the caller's
    responsibility, so no migration is attempted on uncertain state.
    """
    uri = source.resolve().as_uri() + "?mode=ro"
    for attempt in range(_LIVE_HEADER_PROBE_ATTEMPTS):
        db = None
        try:
            # Keep each individual wait bounded as well as the overall retry
            # window.  This also handles a lock that appears after connect.
            db = sqlite3.connect(uri, uri=True, timeout=_LIVE_HEADER_PROBE_DELAY,
                                 isolation_level=None)
            app = db.execute("PRAGMA application_id").fetchone()[0]
            version = db.execute("PRAGMA user_version").fetchone()[0]
            return app, version
        except sqlite3.OperationalError as error:
            if "locked" not in str(error).lower() and "busy" not in str(error).lower():
                raise
            if attempt == _LIVE_HEADER_PROBE_ATTEMPTS - 1:
                raise StoreError("storage_busy") from error
            time.sleep(_LIVE_HEADER_PROBE_DELAY * (attempt + 1))
        finally:
            if db is not None:
                db.close()


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def plan(directory):
    """Read-only; never mutates. Returns the exact intended change, or a no-op."""
    directory = private_directory(directory)
    source = directory / "cortex.sqlite3"
    if not (source.exists() or source.is_symlink()):
        return dict(action="none", note="no store exists yet; the next Store() open initializes format 12 directly")
    regular(source)
    # ``mode=ro`` alone still lets SQLite create the ``-shm``/``-wal``
    # sidecars on open when neither already exists (it must be ready to
    # read a WAL that could appear) -- a real state mutation a pure
    # dry-run must never perform, and one that would additionally fail
    # outright against a read-only parent directory. ``immutable=1`` tells
    # SQLite this file will not change for the connection's lifetime, so
    # it skips the WAL/locking machinery entirely and never touches the
    # sidecars, whether or not they already exist (verified: opening
    # immutable never creates, deletes, or modifies an existing -wal/-shm
    # byte-for-byte -- the same mode ``split.py`` already uses for its own
    # inert reads). application_id/user_version are only ever written by
    # migrate()'s own exclusive-lock transaction, never by live traffic,
    # so a stale main-file header is never a concern for those two
    # pragmas; only the purely informational ``tables_present`` count
    # below can under-report an uncheckpointed concurrent writer's very
    # latest table.
    # ponytail: informational table count can be stale during concurrent
    # WAL activity; not used for the action decision, so left as-is.
    db = sqlite3.connect(source.resolve().as_uri()+"?mode=ro&immutable=1", uri=True, timeout=0, isolation_level=None)
    try:
        app = db.execute("PRAGMA application_id").fetchone()[0]
        version = db.execute("PRAGMA user_version").fetchone()[0]
        tables = db.execute("SELECT COUNT(*) FROM sqlite_master WHERE type='table'").fetchone()[0]
    finally:
        db.close()
    if app != APPLICATION_ID or version not in _KNOWN_VERSIONS:
        raise StoreError("unsupported_storage")
    if version == 10:
        return dict(action="migrate", from_version=10, to_version=12, tables_present=tables,
                    note="run migrate(directory, backup, access_stopped=True) with a fresh, unused backup path")
    if version == 11:
        return dict(action="migrate", from_version=11, to_version=12, tables_present=tables,
                    note="run migrate(directory, backup, access_stopped=True) to remove legacy hook metadata")
    return dict(action="none", current_version=12, tables_present=tables,
                note="already at the only format this runtime writes; any table group this store predates "
                     "self-heals (CREATE TABLE IF NOT EXISTS) on the next Store() open -- no migration needed")


def auto_migrate_format11(directory):
    """Upgrade a legacy format-11 store before a live ``Store`` opens it.

    This is deliberately a narrow wrapper around :func:`migrate`: it only
    recognizes the exact legacy format, allocates a fresh backup in the
    project-local private directory, and lets the existing exclusive-lock,
    verified-backup, transactional migration enforce all safety guarantees.
    A concurrent opener either observes the still-legacy store and receives
    ``storage_busy`` or observes the exact v12 header committed by the other
    opener and continues normally.
    """
    directory = private_directory(directory)
    source = directory / "cortex.sqlite3"
    if not source.exists() and not source.is_symlink():
        return None
    regular(source)
    # Unlike ``plan()``, this probe must observe a committed user_version in
    # an existing WAL.  Use a normal read-only connection (not immutable=1),
    # then close it before the exclusive migration lock is requested.
    app, version = _read_live_header(source)
    if app != APPLICATION_ID or version != 11:
        return None

    # Keep automatic backups beside the database, under a private directory.
    # A UUID and O_EXCL in migrate() ensure this path is never reused or
    # overwritten, including after a crashed opener.
    backup_dir = private_directory(directory / ".migration-backups")
    backup = backup_dir / ("cortex.sqlite3.format11-" + uuid.uuid4().hex + ".bak")
    try:
        return migrate(directory, backup, access_stopped=True)
    except StoreError as error:
        # A second opener can finish its initial v11 probe while the first
        # opener migrates and releases the lock.  In that case migrate()
        # acquires the lock, observes the now-current v12 header, and
        # reports unsupported_storage rather than storage_busy.  Re-read
        # after either race outcome and only treat an exact committed v12
        # header as the other opener having completed successfully.
        if str(error) not in ("storage_busy", "unsupported_storage"):
            raise
        # Another opener may have completed the migration between our probe
        # and the exclusive-lock attempt.  Re-read the live WAL-aware header
        # once after a short scheduling window; only retain storage_busy when
        # the file is still format 11 (for example, a genuinely live legacy
        # runtime still owns the shared lock).
        time.sleep(0.05)
        current_app, current_version = _read_live_header(source)
        if current_app == APPLICATION_ID and current_version == 12:
            return None
        raise


def migrate(directory, backup, *, access_stopped=False):
    """Require a fresh verified backup and exclusive access; never open Markdown."""
    if not access_stopped: raise StoreError("migration_requires_stopped_access")
    directory=private_directory(directory); source=directory/"cortex.sqlite3"
    regular(source)
    backup=Path(backup).expanduser().absolute()
    if backup.exists() or backup.is_symlink(): raise StoreError("migration_backup_exists")
    if backup.parent.resolve()!=backup.parent or not backup.parent.is_dir(): raise StoreError("unsafe_storage")
    lock=directory/".access.lock"
    lock_fd=os.open(lock,os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    db=None; target=None
    try:
        regular(lock)
        try: fcntl.flock(lock_fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError: raise StoreError("storage_busy") from None
        _fault_injection.checkpoint("migrate_before_backup")
        db=sqlite3.connect(source,timeout=0,isolation_level=None)
        version = db.execute("PRAGMA user_version").fetchone()[0]
        if db.execute("PRAGMA application_id").fetchone()[0]!=APPLICATION_ID or version not in (10,11):
            raise StoreError("unsupported_storage")
        # Build the recovery artifact from SQLite's logical snapshot.  This
        # includes committed pages still residing in a WAL; copying only the
        # main file here could silently omit those rows.
        fd=os.open(backup,os.O_CREAT|os.O_EXCL|os.O_RDWR|os.O_NOFOLLOW,0o600)
        os.close(fd)
        target=sqlite3.connect(backup)
        db.backup(target)
        if target.execute("PRAGMA integrity_check").fetchone()[0]!="ok":raise StoreError("migration_backup_invalid")
        target.close();target=None
        with backup.open("rb") as stream:os.fsync(stream.fileno())
        sync_directory(backup.parent)
        backup_digest=hashlib.sha256(backup.read_bytes()).hexdigest()
        _fault_injection.checkpoint("migrate_after_backup_before_schema")
        # Existing v10 tables and immutable Markdown are untouched. Add metadata,
        # initialize known source order and mark older authored provenance unknown (0).
        db.execute("PRAGMA foreign_keys=ON")
        db.executescript("BEGIN EXCLUSIVE;\n"+SCHEMA+IDENTITY_SCHEMA+CATALOG_SCHEMA+CONFIGURATION_SCHEMA+ACP_SESSION_SCHEMA+DELEGATION_SCHEMA+WORK_GRAPH_SCHEMA+TASK_EVENTS_SCHEMA+RUNTIME_SCHEMA+CAPACITY_SCHEMA+EVIDENCE_SCHEMA+ROUTING_SCHEMA)
        # The graph schema is additive inside this explicit migration
        # transaction.  Rows from the pre-structured layout retain an
        # unclassified marker; they are never silently reinterpreted as
        # ordinary/general requirements.
        requirement_columns = {row[1] for row in db.execute("PRAGMA table_info(work_requirement_revisions)")}
        if "requirement_type" not in requirement_columns:
            db.execute(
                "ALTER TABLE work_requirement_revisions ADD COLUMN requirement_type "
                "TEXT NOT NULL DEFAULT 'legacy_unclassified'"
            )
        assignment_columns = {row[1] for row in db.execute("PRAGMA table_info(work_assignments)")}
        if "requirement_state" not in assignment_columns:
            db.execute(
                "ALTER TABLE work_assignments ADD COLUMN requirement_state "
                "TEXT NOT NULL DEFAULT 'legacy_unclassified'"
            )
        if version == 10:
            rows=db.execute("SELECT thread_id FROM thread_bindings").fetchall()
            for (thread,) in rows:
                db.execute("INSERT INTO binding_receipts VALUES (?,?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",(thread,"b_"+uuid.uuid4().hex,"cortex"))
            for (task,) in db.execute("SELECT id FROM tasks").fetchall():
                messages=db.execute("""SELECT s.thread_id,s.message_id,s.report_id FROM source_messages s
                    WHERE task_id=? ORDER BY (SELECT MIN(sequence) FROM editions WHERE report_id=s.report_id),s.message_id""",(task,)).fetchall()
                for revision,(thread,message,report) in enumerate(messages,1):
                    db.execute("INSERT INTO source_revisions VALUES (?,?,?,?,?,?)",(task,revision,thread,message,report,"[]"))
            db.execute("INSERT INTO report_provenance SELECT sequence,0,'[]' FROM editions")
            db.execute("INSERT INTO draft_revisions SELECT id,0 FROM drafts")
            for scope,operation,key,response in db.execute("SELECT scope,operation,delivery_key,response FROM deliveries WHERE operation='write_report'").fetchall():
                receipt=json.loads(response)
                receipt.update(source_revision=0,artifacts=[])
                db.execute("UPDATE deliveries SET response=? WHERE scope=? AND operation=? AND delivery_key=?",(json.dumps(receipt,ensure_ascii=False,sort_keys=True,separators=(",",":")),scope,operation,key))
        for table in _LEGACY_HOOK_TABLES:
            db.execute("DROP TABLE IF EXISTS " + table)
        if db.execute("PRAGMA foreign_key_check").fetchone() is not None:raise StoreError("migration_integrity_failed")
        db.execute("INSERT INTO format_history VALUES (NULL,?,?,?,?,?,?)",
                   ("migrate",version,12,str(backup),backup_digest,_now()))
        _fault_injection.checkpoint("migrate_after_schema_before_commit")
        db.commit()
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        return dict(from_version=version,to_version=12,backup=str(backup),backup_digest=backup_digest,markdown_rewritten=False)
    except BaseException:
        if db is not None:db.rollback()
        raise
    finally:
        if target is not None:target.close()
        if db is not None:db.close()
        fcntl.flock(lock_fd,fcntl.LOCK_UN);os.close(lock_fd)


def rollback(directory, backup, *, access_stopped=False, confirm=False):
    """Restore one verified, format-compatible backup file exactly.

    Never recreates a second storage root and never deletes ``.cortex`` --
    only ``cortex.sqlite3`` (and its WAL/SHM/journal sidecars) inside the
    given storage directory is replaced. Markdown task/draft trees, the
    lock file, and every other entry in the directory are untouched.
    Refuses a backup that fails integrity verification or whose
    application_id/user_version is not one this runtime recognizes (never
    reads an incompatible format with this runtime instead of refusing).
    """
    if not access_stopped: raise StoreError("migration_requires_stopped_access")
    if not confirm: raise StoreError("rollback_requires_confirmation")
    directory=private_directory(directory); target=directory/"cortex.sqlite3"
    backup=Path(backup).expanduser().absolute()
    regular(backup)
    # The backup is an input to a destructive restore.  Keep the containing
    # directory owner-only as well as the file itself, so an attacker cannot
    # replace or observe the backup through a shared directory.
    backup_parent=backup.parent
    parent_info=backup_parent.lstat()
    if (backup_parent.resolve()!=backup_parent or
            stat.S_ISLNK(parent_info.st_mode) or
            not stat.S_ISDIR(parent_info.st_mode) or
            parent_info.st_uid!=os.getuid() or
            stat.S_IMODE(parent_info.st_mode)&0o077):
        raise StoreError("unsafe_storage")
    lock=directory/".access.lock"
    lock_fd=os.open(lock,os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    temp=None
    try:
        regular(lock)
        try: fcntl.flock(lock_fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError: raise StoreError("storage_busy") from None
        # A previous rollback attempt that crashed between writing its own
        # temp copy and the atomic replace leaves only this exact prefix
        # behind (never touches ``target`` before the replace); clean it up
        # before this fresh attempt, matching the existing ``.pending_*``
        # convention for unconfirmed writes elsewhere in this store.
        for stale in directory.glob(".rollback-*.tmp"):
            regular(stale); stale.unlink()
        _fault_injection.checkpoint("rollback_before_restore")
        temp=directory/(".rollback-"+uuid.uuid4().hex+".tmp")
        fd=os.open(temp,os.O_CREAT|os.O_EXCL|os.O_RDWR|os.O_NOFOLLOW,0o600)
        backup_fd=None
        try:
            backup_fd=os.open(backup,os.O_RDONLY|os.O_NOFOLLOW)
            source_info=os.fstat(backup_fd)
            if (not stat.S_ISREG(source_info.st_mode) or
                    source_info.st_uid!=os.getuid() or source_info.st_nlink!=1 or
                    stat.S_IMODE(source_info.st_mode)&0o077):
                raise StoreError("unsafe_storage")
            while True:
                chunk=os.read(backup_fd,1024*1024)
                if not chunk: break
                view=memoryview(chunk)
                while view:
                    written=os.write(fd,view)
                    view=view[written:]
            os.fsync(fd)
        finally:
            os.close(fd)
            if backup_fd is not None:
                os.close(backup_fd)
        _fault_injection.checkpoint("rollback_after_temp_write")
        backup_digest=hashlib.sha256(temp.read_bytes()).hexdigest()
        restored_check=sqlite3.connect(temp)
        try:
            app=restored_check.execute("PRAGMA application_id").fetchone()[0]
            version=restored_check.execute("PRAGMA user_version").fetchone()[0]
            if app!=APPLICATION_ID or version not in _KNOWN_VERSIONS:
                raise StoreError("rollback_backup_incompatible")
            if restored_check.execute("PRAGMA integrity_check").fetchone()[0]!="ok":
                raise StoreError("rollback_backup_invalid")
        finally:
            restored_check.close()
        for suffix in ("-wal","-shm","-journal"):
            sidecar=Path(str(target)+suffix)
            try: info=sidecar.lstat()
            except FileNotFoundError: continue
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid():
                raise StoreError("unsafe_storage")
            sidecar.unlink()
        os.replace(temp,target); temp=None
        sync_directory(directory)
        _fault_injection.checkpoint("rollback_after_replace_before_confirm")
        log=directory/"rollback.log"
        log_fd=os.open(log,os.O_CREAT|os.O_APPEND|os.O_WRONLY|os.O_NOFOLLOW,0o600)
        try:
            entry=dict(action="rollback",restored_from=str(backup),restored_version=version,
                       backup_digest=backup_digest,at=_now())
            os.write(log_fd,(json.dumps(entry,sort_keys=True)+"\n").encode())
            os.fsync(log_fd)
        finally:
            os.close(log_fd)
        return dict(action="rollback",restored_from=str(backup),restored_version=version,backup_digest=backup_digest)
    finally:
        if temp is not None:
            try: temp.unlink()
            except OSError: pass
        fcntl.flock(lock_fd,fcntl.LOCK_UN);os.close(lock_fd)


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__,formatter_class=argparse.RawDescriptionHelpFormatter)
    sub=parser.add_subparsers(dest="command",required=True)

    plan_parser=sub.add_parser("plan",help="Read-only: report the exact intended change or a no-op. Never mutates.")
    plan_parser.add_argument("--storage-dir",required=True)

    migrate_parser=sub.add_parser("migrate",help="Apply the only versioned migration this runtime has (format 10->11).")
    migrate_parser.add_argument("--storage-dir",required=True)
    migrate_parser.add_argument("--backup",required=True,help="New owner-private SQLite backup file; never overwritten.")
    migrate_parser.add_argument("--access-stopped",action="store_true",help="Confirm all ACP client and Cortex MCP access has been stopped; back up project task directories before running.")

    rollback_parser=sub.add_parser("rollback",help="Restore one verified backup exactly. Never recreates a second storage root.")
    rollback_parser.add_argument("--storage-dir",required=True)
    rollback_parser.add_argument("--backup",required=True,help="An existing, previously verified backup file (e.g. from a prior migrate --backup).")
    rollback_parser.add_argument("--access-stopped",action="store_true",help="Confirm all ACP client and Cortex MCP access has been stopped.")
    rollback_parser.add_argument("--confirm",action="store_true",help="Required explicit confirmation; refused without it.")

    args=parser.parse_args(argv)
    try:
        if args.command=="plan":
            result=plan(args.storage_dir)
        elif args.command=="migrate":
            result=migrate(args.storage_dir,args.backup,access_stopped=args.access_stopped)
        else:
            result=rollback(args.storage_dir,args.backup,access_stopped=args.access_stopped,confirm=args.confirm)
    except (StoreError,OSError,sqlite3.Error) as error:
        parser.exit(1,f"{args.command} failed: {type(error).__name__}: {error}\n")
    print(json.dumps(result,sort_keys=True))
    return 0
