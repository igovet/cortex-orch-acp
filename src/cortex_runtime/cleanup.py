"""Explicit owner retention maintenance; not a model-callable MCP operation."""
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
        _delete_if_present("runtime_requests","task_id=?",(task,))
        _delete_if_present("runtime_events","task_id=?",(task,))
        _delete_if_present("runtime_nodes","task_id=?",(task,))
        _delete_if_present("runtime_tasks","task_id=?",(task,))
        # pending_questions/pending_question_late_answers (T020): the late-
        # answers table has no declared FK of its own (a deliberately inert
        # audit row -- see acp.questions_store) but carries its own
        # task_id column; delete both directly.
        _delete_if_present("pending_question_late_answers","task_id=?",(task,))
        _delete_if_present("pending_questions","task_id=?",(task,))
        _delete_if_present("task_language","task_id=?",(task,))
        _delete_if_present("task_stopped","task_id=?",(task,))
        # T022 context_generations/capsule_deliveries/compaction_owners/
        # compaction_events_seen: capsule_deliveries references
        # context_generations(id), so it must go first.
        _delete_if_present("capsule_deliveries","task_id=?",(task,))
        _delete_if_present("context_generations","task_id=?",(task,))
        _delete_if_present("compaction_owners","task_id=?",(task,))
        _delete_if_present("compaction_events_seen","task_id=?",(task,))
        # T023 attempt_process_registry/attempt_effective_context: no other
        # table references either by id, so plain task_id deletes suffice.
        _delete_if_present("attempt_process_registry","task_id=?",(task,))
        _delete_if_present("attempt_effective_context","task_id=?",(task,))
        # T027 attempt_route_audit/fallback_consent_records: no other table
        # references either by id, so plain task_id deletes suffice.
        _delete_if_present("attempt_route_audit","task_id=?",(task,))
        _delete_if_present("fallback_consent_records","task_id=?",(task,))
        # T029 observability_events/usage_records: no other table references
        # either by id, so plain task_id deletes suffice.
        _delete_if_present("observability_events","task_id=?",(task,))
        _delete_if_present("usage_records","task_id=?",(task,))
        # T024 large_outputs/compaction_recovery_events: no other table
        # references either by id, so plain task_id deletes suffice.
        # large_outputs also owns files under this task's storage directory
        # (see recovery.large_output_store) -- the caller deletes those
        # files as part of the same task-directory teardown this cleanup
        # already performs; this only removes the metadata rows.
        _delete_if_present("large_outputs","task_id=?",(task,))
        _delete_if_present("compaction_recovery_events","task_id=?",(task,))
        # T025 workspace_snapshots/workspace_integrations: the actual
        # snapshot/base directories on disk (Cortex-owned, under this
        # project's own .cortex/workspaces/<attempt_id>/) are removed
        # first -- a plain DB delete alone would leak them -- then both
        # rows. workspace_integrations has no other table referencing it.
        if "workspace_snapshots" in existing_tables:
            from .delegation.workspace_isolation import _remove_owned_tree
            for snapshot_row in db.execute(
                "SELECT root_path, base_path FROM workspace_snapshots WHERE task_id=? AND cleaned_up_at IS NULL",
                (task,),
            ).fetchall():
                for path in (snapshot_row["root_path"], snapshot_row["base_path"]):
                    if path is not None:
                        try: _remove_owned_tree(path)
                        except StoreError: pass
        # Candidate stages live beside workspaces under the project store,
        # rather than under the per-task directory.  Remove their owned trees
        # only while replaying an explicit, durable task deletion.  In
        # particular, conflict/failed candidates are never reclaimed by a
        # background sweep merely because they are non-ready.
        if "candidate_staging" in existing_tables:
            from .delegation.workspace_isolation import _remove_owned_tree
            for candidate_row in db.execute(
                "SELECT stage_path FROM candidate_staging WHERE task_id=?", (task,)
            ).fetchall():
                stage_path = candidate_row["stage_path"]
                if stage_path:
                    _remove_owned_tree(Path(stage_path).parent)
        _delete_if_present("candidate_staging", "task_id=?", (task,))
        # Verification requests are immutable execution identity records for
        # the task.  They are retained while the task exists, including
        # unknown outcomes, and removed only with the task's explicit
        # retention deletion.
        _delete_if_present("check_requests", "task_id=?", (task,))
        # T035 check evidence is bound to candidates and attempts, but its
        # rows intentionally remain immutable until task retention. Delete
        # the operation journal first, then the evidence rows, before the
        # task itself is removed. The evidence files are retained only inside
        # the task/project store and are covered by the task teardown above.
        _delete_if_present("check_evidence_operations", "task_id=?", (task,))
        _delete_if_present("check_evidence", "task_id=?", (task,))
        _delete_if_present("instruction_bundles", "task_id=?", (task,))
        # Lineage and seed bindings are auxiliary rows around T025 workspace
        # records.  The lineage table references workspace_snapshots and must
        # be deleted first; candidate IDs are intentionally historical strings
        # without a foreign key to allow candidate inspection until retention.
        _delete_if_present(
            "workspace_snapshot_lineage",
            "workspace_binding_id IN (SELECT workspace_binding_id FROM workspace_snapshots WHERE task_id=?)",
            (task,),
        )
        _delete_if_present("workspace_candidate_seeds", "task_id=?", (task,))
        _delete_if_present("workspace_integrations","task_id=?",(task,))
        _delete_if_present("workspace_snapshots","task_id=?",(task,))
        # T034 work graph: attempts reference assignments (and each other),
        # predecessors reference assignments, and requirement revisions
        # reference requirements. Delete children first so retention leaves
        # no graph rows behind and SQLite foreign-key checks remain clean.
        _delete_if_present("work_assignment_attempts","task_id=?",(task,))
        _delete_if_present("work_assignment_predecessors","task_id=?",(task,))
        _delete_if_present("work_assignments","task_id=?",(task,))
        _delete_if_present("work_requirement_revisions","requirement_id IN (SELECT requirement_id FROM work_requirements WHERE task_id=?)",(task,))
        _delete_if_present("work_requirements","task_id=?",(task,))
        _delete_if_present("work_graph_operations","task_id=?",(task,))
        db.execute("DELETE FROM task_events WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_orchestration WHERE task_id=?",(task,))
        db.execute("DELETE FROM capacity_reservations WHERE task_id=?",(task,))
        db.execute("DELETE FROM tool_result_observations WHERE task_id=?",(task,))
        _delete_if_present("report_read_observations","task_id=?",(task,))
        _delete_if_present("permission_receipts","task_id=?",(task,))
        # host_transcript (T018) references acp_host_sessions(session_id),
        # not tasks(id) directly -- must go before acp_host_sessions.
        _delete_if_present("host_transcript","session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",(task,))
        _delete_if_present("host_updates","session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",(task,))
        _delete_if_present("acp_session_workspaces","session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",(task,))
        db.execute("DELETE FROM acp_host_sessions WHERE task_id=?",(task,))
        _delete_if_present("task_workspaces","task_id=?",(task,))
        db.execute("DELETE FROM task_route_selections WHERE task_id=?",(task,))
        # T027 configuration_route_extras references configuration_revisions(id)
        # -- must go before that table's own delete.
        _delete_if_present("configuration_route_extras","task_id=?",(task,))
        db.execute("DELETE FROM configuration_revisions WHERE task_id=?",(task,))
        # T005 identity/ownership group -- previously never deleted at all
        # (pre-existing gap, T015 report; closed here).
        db.execute("DELETE FROM trusted_channels WHERE task_id=?",(task,))
        db.execute("DELETE FROM source_events WHERE task_id=?",(task,))
        db.execute("DELETE FROM task_owners WHERE task_id=?",(task,))
        db.execute("DELETE FROM report_provenance WHERE edition_sequence IN (SELECT e.sequence FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=?)",(task,))
        for table in ("source_turns","source_resets"):
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


def protected_active_tasks(db):
    """Fail closed for non-terminal, live or ambiguously live task activity."""
    from .orchestration.state import TERMINAL

    tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    keep = set()
    if "runtime_tasks" in tables:
        keep.update(row["task_id"] for row in db.execute("SELECT task_id,state FROM runtime_tasks")
                    if row["state"] not in TERMINAL)
    if "capacity_reservations" in tables:
        keep.update(row[0] for row in db.execute(
            "SELECT task_id FROM capacity_reservations WHERE state!='released'"))
    if "attempt_process_registry" in tables:
        keep.update(row[0] for row in db.execute(
            "SELECT task_id FROM attempt_process_registry WHERE terminated_at IS NULL"))
    return keep


def clear_tasks(store,project_root,days=None,keep_threads=(),now=None,*,
                cutoff=None,dry_run=False,protected_task_ids=(),protect_active=False):
    """Select and journal retention under an exclusive, nonblocking store lock.

    Chat callers require active protection; the manual CLI retains its existing
    retention/keep-thread contract. Dry runs never replay pending deletions.
    """
    current = now or datetime.now(timezone.utc)
    if cutoff is None:
        if type(days) is not int or not 0<=days<=36500:
            raise StoreError("invalid_retention","days",days,"integer from 0 through 36500")
        cutoff = current - timedelta(days=days)
    elif days is not None or not isinstance(cutoff, datetime) or cutoff.tzinfo is None or cutoff > current:
        raise StoreError("invalid_retention","cutoff",None,"a non-future timezone-aware cutoff, without days")
    cutoff_text = cutoff.astimezone(timezone.utc).isoformat(timespec="microseconds")
    project=Path(project_root)
    if not project.is_absolute() or not project.is_dir() or str(project.resolve())!=str(project_root):
        raise StoreError("invalid_project","project_root",str(project_root),"absolute existing canonical directory")
    # Keep the lock through every intent commit and filesystem teardown: a
    # concurrently resumed task cannot cross the selection/deletion boundary.
    with store.connection(exclusive=True) as db:
        db.execute("BEGIN IMMEDIATE")
        rows=db.execute('''SELECT id,activity FROM (
            SELECT t.id,t.project_root,MAX(t.created_at,
                COALESCE((SELECT MAX(e.updated_at) FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=t.id),t.created_at)) AS activity
            FROM tasks t) WHERE project_root=? AND activity<?''',(str(project),cutoff_text)).fetchall()
        keep={entry["task_id"] for thread in keep_threads for entry in db.execute("SELECT task_id FROM thread_bindings WHERE thread_id=?",(thread,))}
        keep.update(protected_task_ids)
        if protect_active:
            keep.update(protected_active_tasks(db))
        selected=[row["id"] for row in rows if row["id"] not in keep]
        pending=[row[0] for row in db.execute("SELECT d.task_id FROM pending_deletions d JOIN tasks t ON t.id=d.task_id WHERE t.project_root=?",(str(project),))]
        if set(pending) & keep:
            raise StoreError("retention_active_conflict")
        result = dict(deleted_tasks=0 if dry_run else len(set(pending) | set(selected)),retention_days=days,
                      skipped_protected=sum(row["id"] in keep for row in rows),cutoff=cutoff_text)
        if dry_run:
            result["would_delete_tasks"] = len(set(pending) | set(selected))
            db.rollback()
            return result
        for task in selected: db.execute("INSERT OR IGNORE INTO pending_deletions VALUES (?)",(task,))
        db.commit()
        for task in set(pending) | set(selected):
            db.execute("BEGIN IMMEDIATE"); finish_deletions(store,db,task); db.commit()
    return result
