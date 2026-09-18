"""Explicit offline format-11 upgrade/rollback safety for one project store.

Three operations, all offline (stopped-access) and never touching Markdown:

- ``plan()``:    read-only; reports the exact intended change, or a no-op.
- ``migrate()``: the only versioned schema change this runtime has ever
  needed (format 10 -> 11, T005); verified backup first, then one atomic
  SQLite transaction, then a ``format_history`` row.
- ``rollback()``: restores a verified, format-compatible backup file
  byte-for-byte. Never recreates a second storage root and never deletes
  ``.cortex``; refuses an unverifiable or incompatible backup.

See docs/acp/format-migration.md for the full compatibility conclusion:
every schema addition since format 11 (T005 onward) is one independent,
self-healing, all-or-nothing optional table group created by
``CREATE TABLE IF NOT EXISTS`` on the next ``Store()`` open (see
``store.py``, and the group inventory in ``split.py``) -- no further
version bump has ever been required, and none is invented here.
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
import uuid

from . import _fault_injection
from .acp.sessions import ACP_SESSION_SCHEMA
from .catalog.store import CATALOG_SCHEMA
from .configuration.store import CONFIGURATION_SCHEMA
from .contracts import StoreError
from .delegation.store import DELEGATION_SCHEMA
from .delegation.events_store import TASK_EVENTS_SCHEMA
from .delegation.capacity_store import CAPACITY_SCHEMA
from .delegation.evidence_store import EVIDENCE_SCHEMA
from .routing.store import ROUTING_SCHEMA
from .hook_storage import HOOK_SCHEMA
from .store import APPLICATION_ID, SCHEMA, private_directory, regular, sync_directory
from .trusted_channel import IDENTITY_SCHEMA

# Formats this exact runtime build knows how to recognize at all: 10 (the
# pre-T005 layout migrate() upgrades from) and 11 (the only format any
# store.py-created database has ever used since). Anything else -- a
# backup or live file from an unrelated/newer/older application_id or
# user_version -- is never opened for rollback or reported by plan().
_KNOWN_VERSIONS = (10, 11)


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def plan(directory):
    """Read-only; never mutates. Returns the exact intended change, or a no-op."""
    directory = private_directory(directory)
    source = directory / "cortex.sqlite3"
    if not (source.exists() or source.is_symlink()):
        return dict(action="none", note="no store exists yet; the next Store() open initializes format 11 directly")
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
        return dict(action="migrate", from_version=10, to_version=11, tables_present=tables,
                    note="run migrate(directory, backup, access_stopped=True) with a fresh, unused backup path")
    return dict(action="none", current_version=11, tables_present=tables,
                note="already at the only format this runtime writes; any table group this store predates "
                     "self-heals (CREATE TABLE IF NOT EXISTS) on the next Store() open -- no migration needed")


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
        if db.execute("PRAGMA application_id").fetchone()[0]!=APPLICATION_ID or db.execute("PRAGMA user_version").fetchone()[0]!=10:
            raise StoreError("unsupported_storage")
        db.execute("PRAGMA locking_mode=EXCLUSIVE")
        db.execute("BEGIN EXCLUSIVE"); db.commit()
        fd=os.open(backup,os.O_CREAT|os.O_EXCL|os.O_RDWR|os.O_NOFOLLOW,0o600);os.close(fd)
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
        db.executescript("BEGIN EXCLUSIVE;\n"+SCHEMA+HOOK_SCHEMA+IDENTITY_SCHEMA+CATALOG_SCHEMA+CONFIGURATION_SCHEMA+ACP_SESSION_SCHEMA+DELEGATION_SCHEMA+TASK_EVENTS_SCHEMA+CAPACITY_SCHEMA+EVIDENCE_SCHEMA+ROUTING_SCHEMA)
        db.execute("PRAGMA foreign_keys=ON")
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
        if db.execute("PRAGMA foreign_key_check").fetchone() is not None:raise StoreError("migration_integrity_failed")
        db.execute("INSERT INTO format_history VALUES (NULL,?,?,?,?,?,?)",
                   ("migrate",10,11,str(backup),backup_digest,_now()))
        _fault_injection.checkpoint("migrate_after_schema_before_commit")
        db.commit()
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        return dict(from_version=10,to_version=11,backup=str(backup),backup_digest=backup_digest,markdown_rewritten=False)
    except BaseException:
        if db is not None:db.rollback()
        raise
    finally:
        if target is not None:target.close()
        if db is not None:db.close()
        fcntl.flock(lock_fd,fcntl.LOCK_UN);os.close(lock_fd)


def rollback(directory, backup, *, access_stopped=False, confirm=False):
    """Restore one verified, format-compatible backup file byte-for-byte.

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
        verify=sqlite3.connect(backup.resolve().as_uri()+"?mode=ro",uri=True,timeout=0,isolation_level=None)
        try:
            app=verify.execute("PRAGMA application_id").fetchone()[0]
            version=verify.execute("PRAGMA user_version").fetchone()[0]
            if app!=APPLICATION_ID or version not in _KNOWN_VERSIONS:
                raise StoreError("rollback_backup_incompatible")
            if verify.execute("PRAGMA integrity_check").fetchone()[0]!="ok":
                raise StoreError("rollback_backup_invalid")
        finally:
            verify.close()
        backup_digest=hashlib.sha256(backup.read_bytes()).hexdigest()
        _fault_injection.checkpoint("rollback_before_restore")
        temp=directory/(".rollback-"+uuid.uuid4().hex+".tmp")
        fd=os.open(temp,os.O_CREAT|os.O_EXCL|os.O_RDWR|os.O_NOFOLLOW,0o600)
        try:
            with backup.open("rb") as source_stream:
                while True:
                    chunk=source_stream.read(1024*1024)
                    if not chunk: break
                    os.write(fd,chunk)
            os.fsync(fd)
        finally:
            os.close(fd)
        _fault_injection.checkpoint("rollback_after_temp_write")
        restored_check=sqlite3.connect(temp)
        try:
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
    migrate_parser.add_argument("--access-stopped",action="store_true",help="Confirm all CLI/Desktop/MCP/hook access has been stopped; back up project task directories before running.")

    rollback_parser=sub.add_parser("rollback",help="Restore one verified backup byte-for-byte. Never recreates a second storage root.")
    rollback_parser.add_argument("--storage-dir",required=True)
    rollback_parser.add_argument("--backup",required=True,help="An existing, previously verified backup file (e.g. from a prior migrate --backup).")
    rollback_parser.add_argument("--access-stopped",action="store_true",help="Confirm all CLI/Desktop/MCP/hook access has been stopped.")
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
