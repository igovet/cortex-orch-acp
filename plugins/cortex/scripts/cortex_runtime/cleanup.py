"""Explicit host-side retention maintenance; not an MCP operation."""
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import shutil

from .contracts import StoreError
from .project_storage import project_store_directory
from .store import file_digest, private_directory, sync_directory


def finish_deletions(store,db,task):
    """Replay committed deletion intents without deleting unrelated project drafts."""
    rows=db.execute("SELECT d.task_id,t.project_root FROM pending_deletions d JOIN tasks t ON t.id=d.task_id WHERE d.task_id=?",(task,)).fetchall()
    for row in rows:
        task=row["task_id"]
        for source in db.execute("SELECT path,published_digest FROM drafts WHERE task_id=?",(task,)).fetchall():
            try: candidate=store._source(db,task,source["path"],must_exist=False)
            except StoreError: candidate=None
            if candidate is not None:
                # Every server-created draft row is an unambiguous task link. Published
                # drafts additionally retain their accepted digest for tamper detection.
                if source["published_digest"] is not None and file_digest(candidate,private=False,validate_utf8=True)!=source["published_digest"]:
                    raise StoreError("draft_conflict","draft_id","retention candidate","bytes matching its accepted SHA-256")
                candidate.unlink(); sync_directory(candidate.parent)
        task_root=private_directory(project_store_directory(row["project_root"]))
        directory=task_root/task
        if directory.exists() or directory.is_symlink():
            info=directory.lstat()
            if directory.is_symlink() or not directory.is_dir() or info.st_uid!=os.getuid():
                raise StoreError("unsafe_storage","task_directory",str(directory),"owned project task directory")
            if not shutil.rmtree.avoids_symlink_attacks: raise StoreError("unsafe_storage")
            shutil.rmtree(directory); sync_directory(task_root)
        # T020 fresh-review fix (MEDIUM, reproduced): every optional-group
        # table that references tasks(id) directly or indirectly (via a
        # table that itself references tasks(id)) must be deleted here, in
        # dependency order (children before the row they reference), only
        # when that group's table(s) actually exist for this store's
        # schema version -- see tests/test_retention.py's
        # test_finish_deletions_covers_every_fk_to_tasks_table, which
        # derives the authoritative table list from PRAGMA
        # foreign_key_list/sqlite_master at runtime and fails closed if
        # any such table is ever added without a matching delete here.
        # Earlier gap (T015 report, also closed here): trusted_channels/
        # source_events/task_owners (identity/ownership group) were never
        # deleted at all -- a bootstrap-created task's owner epoch/channel
        # rows outlived the task itself.
        existing_tables={row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")}
        def _delete_if_present(table,where,params):
            if table in existing_tables:
                db.execute(f"DELETE FROM {table} WHERE {where}",params)
        # attempt_revisions (T019) has no declared FK (attempt_id, not
        # task_id) but is logically scoped to this task's own delegation
        # attempts -- must be removed before delegation_provisioning
        # itself (the subquery needs it still present).
        _delete_if_present("attempt_revisions","attempt_id IN (SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND attempt_id IS NOT NULL)",(task,))
        db.execute("DELETE FROM delegation_effects WHERE idempotency_key IN (SELECT idempotency_key FROM delegation_provisioning WHERE task_id=?)",(task,))
        db.execute("DELETE FROM delegation_provisioning WHERE task_id=?",(task,))
        # pending_questions/pending_question_late_answers (T020): the late-
        # answers table has no declared FK of its own (a deliberately inert
        # audit row -- see acp.questions_store) but carries its own
        # task_id column; delete both directly.
        _delete_if_present("pending_question_late_answers","task_id=?",(task,))
        _delete_if_present("pending_questions","task_id=?",(task,))
        _delete_if_present("task_language","task_id=?",(task,))
        _delete_if_present("task_stopped","task_id=?",(task,))
        db.execute("DELETE FROM task_events WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_orchestration WHERE task_id=?",(task,))
        db.execute("DELETE FROM capacity_reservations WHERE task_id=?",(task,))
        db.execute("DELETE FROM tool_result_observations WHERE task_id=?",(task,))
        # host_transcript (T018) references acp_host_sessions(session_id),
        # not tasks(id) directly -- must go before acp_host_sessions.
        _delete_if_present("host_transcript","session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",(task,))
        db.execute("DELETE FROM acp_host_sessions WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_route_selections WHERE task_id=?",(task,))
        db.execute("DELETE FROM configuration_revisions WHERE task_id=?",(task,))
        # T005 identity/ownership group -- previously never deleted at all
        # (pre-existing gap, T015 report; closed here).
        db.execute("DELETE FROM trusted_channels WHERE task_id=?",(task,))
        db.execute("DELETE FROM source_events WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_owners WHERE task_id=?",(task,))
        db.execute("DELETE FROM report_provenance WHERE edition_sequence IN (SELECT e.sequence FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=?)",(task,))
        for table in ("source_turns","source_resets","hook_events","hook_hints","hook_agent_bindings","hook_pending_sources","hook_command_sessions"):
            db.execute(f"DELETE FROM {table} WHERE task_id=?",(task,))
        db.execute("DELETE FROM source_revisions WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_changes WHERE task_id=?",(task,))
        db.execute("DELETE FROM binding_receipts WHERE thread_id IN (SELECT thread_id FROM thread_bindings WHERE task_id=?)",(task,))
        db.execute("DELETE FROM pending_source_deletions WHERE task_id=?",(task,))
        db.execute("DELETE FROM draft_revisions WHERE draft_id IN (SELECT id FROM drafts WHERE task_id=?)",(task,))
        db.execute("DELETE FROM drafts WHERE task_id=?",(task,))
        db.execute("DELETE FROM governance WHERE task_id=?",(task,))
        db.execute("DELETE FROM source_messages WHERE task_id=?",(task,))
        db.execute("DELETE FROM source_cursors WHERE task_id=?",(task,))
        # source_block_reads (T018) references reports(report_id), not
        # tasks(id) directly -- must go before reports/source_blocks.
        _delete_if_present("source_block_reads","report_id IN (SELECT id FROM reports WHERE task_id=?)",(task,))
        _delete_if_present("source_blocks","task_id=?",(task,))
        db.execute("DELETE FROM editions WHERE report_id IN (SELECT id FROM reports WHERE task_id=?)",(task,))
        db.execute("DELETE FROM reports WHERE task_id=?",(task,))
        db.execute("DELETE FROM deliveries WHERE scope=? OR (operation=? AND json_extract(response,?)=?)",(task,"create_task","$.task_id",task))
        db.execute("DELETE FROM thread_bindings WHERE task_id=?",(task,))
        db.execute("DELETE FROM pending_deletions WHERE task_id=?",(task,))
        db.execute("DELETE FROM tasks WHERE id=?",(task,))


def clear_tasks(store,project_root,days,keep_threads=(),now=None):
    if type(days) is not int or not 0<=days<=36500:
        raise StoreError("invalid_retention","days",days,"integer from 0 through 36500","Use clear N days with a nonnegative integer N.")
    project=Path(project_root)
    if not project.is_absolute() or not project.is_dir() or str(project.resolve())!=str(project_root):
        raise StoreError("invalid_project","project_root",str(project_root),"absolute existing canonical directory")
    cutoff=((now or datetime.now(timezone.utc))-timedelta(days=days)).isoformat(timespec="microseconds")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        rows=db.execute('''SELECT id,activity FROM (
            SELECT t.id,t.project_root,MAX(t.created_at,
                COALESCE((SELECT MAX(e.updated_at) FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=t.id),t.created_at)) AS activity
            FROM tasks t) WHERE project_root=? AND activity<?''',(str(project),cutoff)).fetchall()
        keep={entry["task_id"] for thread in keep_threads for entry in db.execute("SELECT task_id FROM thread_bindings WHERE thread_id=?",(thread,))}
        selected=[row["id"] for row in rows if row["id"] not in keep]
        for task in selected: db.execute("INSERT OR IGNORE INTO pending_deletions VALUES (?)",(task,))
        db.commit()
        pending=[row[0] for row in db.execute("SELECT d.task_id FROM pending_deletions d JOIN tasks t ON t.id=d.task_id WHERE t.project_root=?",(str(project),))]
        for task in pending:
            db.execute("BEGIN IMMEDIATE"); finish_deletions(store,db,task); db.commit()
    return dict(deleted_tasks=len(selected),retention_days=days,
                skipped_protected=sum(row["id"] in keep for row in rows))
