//! Workspace retention, fenced across journal commits and filesystem teardown.
use super::{
    locks::{self, AccessLock},
    outbox, schema, OpenMode, Store,
};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    time::Duration,
};

const DELETIONS: &[&str]=&[
"DELETE FROM attempt_revisions WHERE attempt_id IN (SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND attempt_id IS NOT NULL)",
"DELETE FROM attempt_revisions WHERE attempt_id='baseline:'||?",
"DELETE FROM delegation_effects WHERE idempotency_key IN (SELECT idempotency_key FROM delegation_provisioning WHERE task_id=?)",
"DELETE FROM delegation_provisioning WHERE task_id=?",
"DELETE FROM runtime_requests WHERE task_id=?",
"DELETE FROM runtime_events WHERE task_id=?",
"DELETE FROM runtime_nodes WHERE task_id=?",
"DELETE FROM runtime_tasks WHERE task_id=?",
"DELETE FROM pending_question_late_answers WHERE task_id=?",
"DELETE FROM pending_questions WHERE task_id=?",
"DELETE FROM task_language WHERE task_id=?",
"DELETE FROM task_stopped WHERE task_id=?",
"DELETE FROM capsule_deliveries WHERE task_id=?",
"DELETE FROM compaction_recovery_events WHERE task_id=?",
"DELETE FROM context_generations WHERE task_id=?",
"DELETE FROM compaction_owners WHERE task_id=?",
"DELETE FROM compaction_events_seen WHERE task_id=?",
"DELETE FROM attempt_process_registry WHERE task_id=?",
"DELETE FROM attempt_effective_context WHERE task_id=?",
"DELETE FROM attempt_route_audit WHERE task_id=?",
"DELETE FROM fallback_consent_records WHERE task_id=?",
"DELETE FROM observability_events WHERE task_id=?",
"DELETE FROM usage_records WHERE task_id=?",
"DELETE FROM large_outputs WHERE task_id=?",
"DELETE FROM candidate_staging WHERE task_id=?",
"DELETE FROM check_requests WHERE task_id=?",
"DELETE FROM check_evidence_operations WHERE task_id=?",
"DELETE FROM check_evidence WHERE task_id=?",
"DELETE FROM instruction_bundle_deliveries WHERE bundle_id IN (SELECT bundle_id FROM instruction_bundles WHERE task_id=?)",
"DELETE FROM instruction_bundles WHERE task_id=?",
"DELETE FROM workspace_snapshot_lineage WHERE workspace_binding_id IN (SELECT workspace_binding_id FROM workspace_snapshots WHERE task_id=?)",
"DELETE FROM workspace_candidate_seeds WHERE task_id=?",
"DELETE FROM workspace_integrations WHERE task_id=?",
"DELETE FROM workspace_snapshots WHERE task_id=?",
"DELETE FROM work_assignment_attempts WHERE task_id=?",
"DELETE FROM work_assignment_predecessors WHERE task_id=?",
"DELETE FROM work_assignments WHERE task_id=?",
"DELETE FROM work_requirement_revisions WHERE requirement_id IN (SELECT requirement_id FROM work_requirements WHERE task_id=?)",
"DELETE FROM work_requirements WHERE task_id=?",
"DELETE FROM work_graph_operations WHERE task_id=?",
"DELETE FROM task_events WHERE task_id=?",
"DELETE FROM task_orchestration WHERE task_id=?",
"DELETE FROM capacity_reservations WHERE task_id=?",
"DELETE FROM tool_result_observations WHERE task_id=?",
"DELETE FROM report_read_observations WHERE task_id=?",
"DELETE FROM permission_receipts WHERE task_id=?",
"DELETE FROM host_transcript WHERE session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",
"DELETE FROM host_updates WHERE session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",
"DELETE FROM acp_session_workspaces WHERE session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",
"DELETE FROM acp_session_access_defaults WHERE session_id IN (SELECT session_id FROM acp_host_sessions WHERE task_id=?)",
"DELETE FROM acp_host_sessions WHERE task_id=?",
"DELETE FROM task_workspaces WHERE task_id=?",
"DELETE FROM task_route_selections WHERE task_id=?",
"DELETE FROM configuration_route_extras WHERE task_id=?",
"DELETE FROM configuration_revisions WHERE task_id=?",
"DELETE FROM trusted_channels WHERE task_id=?",
"DELETE FROM source_events WHERE task_id=?",
"DELETE FROM task_owners WHERE task_id=?",
"DELETE FROM report_provenance WHERE edition_sequence IN (SELECT e.sequence FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=?)",
"DELETE FROM source_turns WHERE task_id=?",
"DELETE FROM source_resets WHERE task_id=?",
"DELETE FROM source_revisions WHERE task_id=?",
"DELETE FROM task_changes WHERE task_id=?",
"DELETE FROM binding_receipts WHERE thread_id IN (SELECT thread_id FROM thread_bindings WHERE task_id=?)",
"DELETE FROM pending_source_deletions WHERE task_id=?",
"DELETE FROM draft_revisions WHERE draft_id IN (SELECT id FROM drafts WHERE task_id=?)",
"DELETE FROM drafts WHERE task_id=?",
"DELETE FROM governance WHERE task_id=?",
"DELETE FROM source_messages WHERE task_id=?",
"DELETE FROM source_cursors WHERE task_id=?",
"DELETE FROM source_block_reads WHERE report_id IN (SELECT id FROM reports WHERE task_id=?)",
"DELETE FROM source_blocks WHERE task_id=?",
"DELETE FROM editions WHERE report_id IN (SELECT id FROM reports WHERE task_id=?)",
"DELETE FROM reports WHERE task_id=?",
"DELETE FROM deliveries WHERE scope=? OR (operation=? AND json_extract(response,?)=?)",
"DELETE FROM thread_bindings WHERE task_id=?",
"DELETE FROM pending_deletions WHERE task_id=?",
"DELETE FROM tasks WHERE id=?",
];
fn safe_id(id: &str) -> CoreResult<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        || [".", ".."].contains(&id)
    {
        return refuse("unsafe_storage");
    }
    Ok(())
}
pub(crate) fn quiet(db: &Connection, task: &str) -> CoreResult<()> {
    let row: Option<String> = db
        .query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    if !matches!(row.as_deref(), Some("COMPLETED" | "FAILED" | "CANCELLED")) {
        return refuse("active_execution");
    }
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND status='running')",
    ] {
        if db.query_row(sql, [task], |r| r.get::<_, bool>(0))? {
            return refuse("active_execution");
        }
    }
    // A process recorded without an end time is live only within the boot that
    // recorded it: no process survives a reboot. An unreadable boot identity keeps
    // every such record active (fail-closed).
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|b| b.trim().to_owned())
        .unwrap_or_default();
    let open: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM attempt_process_registry WHERE task_id=? AND terminated_at IS NULL
         AND (?2='' OR boot_id=?2))",
        (task, &boot),
        |r| r.get(0),
    )?;
    if open {
        return refuse("active_execution");
    }
    Ok(())
}
fn remove_tree(store: &Store, path: &Path, sub: &str) -> CoreResult<()> {
    if !path.starts_with(store.key.directory().join(sub))
        || path == store.key.directory().join(sub)
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return refuse("unsafe_storage");
    }
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let parent = path.parent().ok_or(CoreError::Refused("unsafe_storage"))?;
    if parent.canonicalize()? != parent {
        return refuse("unsafe_storage");
    }
    locks::private_directory(path, false)?;
    fs::remove_dir_all(path)?;
    locks::sync_directory(parent)?;
    Ok(())
}
fn remove_source(
    root: &Path,
    path: &Path,
    device: i64,
    inode: i64,
    digest: Option<&str>,
) -> CoreResult<()> {
    // A stored source identity never authorizes following a new symlink or an outside path.
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return refuse("unsafe_storage");
    }
    let parent = path.parent().ok_or(CoreError::Refused("unsafe_storage"))?;
    if parent.canonicalize()? != parent {
        return refuse("unsafe_storage");
    }
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    let dir = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)?;
    let name = CString::new(
        path.file_name()
            .ok_or(CoreError::Refused("unsafe_storage"))?
            .as_encoded_bytes(),
    )
    .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(e.into())
        };
    }
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.nlink() != 1
        || (m.dev() as i64, m.ino() as i64) != (device, inode)
    {
        return refuse("draft_conflict");
    }
    if let Some(wanted) = digest {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if format!("{:x}", hash.finalize()) != wanted {
            return refuse("draft_conflict");
        }
    }
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    dir.sync_all()?;
    Ok(())
}
fn adapter(db: &rusqlite::Transaction<'_>, project: &str, task: &str) -> CoreResult<()> {
    let v: Option<i64> = db
        .query_row(
            "SELECT version FROM workspace_tasks WHERE task_id=? AND project_id=?",
            (task, project),
            |r| r.get(0),
        )
        .optional()?;
    if let Some(v) = v {
        for table in [
            "workspace_task_options",
            "workspace_starts",
            "workspace_executions",
            "workspace_tasks",
        ] {
            db.execute(&format!("DELETE FROM {table} WHERE task_id=?"), [task])?;
        }
        db.execute(
            "UPDATE workspace_projects SET version=version+1 WHERE project_id=?",
            [project],
        )?;
        outbox::append_tx(db, project, task, "task", v + 1)?;
    }
    Ok(())
}
/// Check the Workspace disposition and archive clock. The scheduler holds the
/// machine lock from reading this record until recording the purge outcome.
pub fn expired_closed_archive(record: &Value, now_ms: u64, retention_days: u64) -> bool {
    (1..=3650).contains(&retention_days)
        && record["purged"] != true
        && record["status"] == "done"
        && record["archive"]["previousStatus"] == "done"
        && (matches!(record["closedAs"].as_str(), Some("as_is" | "cancel" | "external"))
            || record["closedAs"].is_null() && record["work"] == "merged")
        && record["doneAt"].as_u64().zip(record["archive"]["at"].as_u64())
            .is_some_and(|(done, archived)| {
                if done > archived {
                    return false;
                }
                let clock = match record["archive"].get("retentionAt") {
                    None => Some(archived),
                    // Sprint close stores the completion plus the ordinary
                    // auto-archive delay, independently of the close time.
                    Some(clock) => clock.as_u64().filter(|clock| *clock >= done),
                };
                clock.is_some_and(|clock| {
                    now_ms.saturating_sub(clock) >= retention_days * 86_400_000
                })
            })
}

fn scheduled_binding(
    db: &Connection,
    root: &str,
    project: &str,
    task: &str,
    runtime: &str,
) -> CoreResult<()> {
    let projected = crate::projections::task_records(db, project, root, task)?
        .ok_or(CoreError::Refused("not_found"))?;
    let Some(execution) = projected.execution_id.as_deref() else {
        return refuse("active_execution");
    };
    if projected.warnings.iter().any(|w| w == crate::projections::CONTRADICTORY_EVIDENCE)
        || matches!(projected.phase.as_str(), "unknown" | "paused")
    {
        return refuse("active_execution");
    }
    let linked: bool = db.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
           JOIN workspace_projects p USING(project_id)
           WHERE p.project_id=?1 AND p.project_root=?2 AND t.task_id=?3
           AND e.execution_id=t.execution_id AND e.execution_id=?4 AND e.runtime_task_id=?5
           AND (SELECT count(*) FROM workspace_executions x WHERE x.task_id=t.task_id)=1
           AND NOT EXISTS(SELECT 1 FROM workspace_starts s WHERE s.task_id=t.task_id
               AND (s.state!='dispatched' OR s.execution_id!=e.execution_id
                    OR s.project_id!=p.project_id))
         )",
        (project, root, task, execution, runtime),
        |r| r.get(0),
    )?;
    if !linked {
        return refuse("active_execution");
    }
    // The bound Workspace display journal is a logical, replayable ID,
    // not a loadable ACP session. We cannot establish whether another
    // retained host session is loaded (even an old one can be loaded).
    let protected: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM acp_host_sessions h WHERE h.task_id=?1
            AND NOT (h.session_id=(
                SELECT acp_session_id FROM workspace_executions WHERE execution_id=?2
            ) AND h.client_info='{\"name\":\"cortex-native\"}'))
         OR EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=?1 AND status!='terminated')
         OR EXISTS(SELECT 1 FROM work_assignment_attempts WHERE task_id=?1 AND status IN ('planned','running'))
         OR EXISTS(SELECT 1 FROM pending_questions WHERE task_id=?1 AND status='open')",
        (runtime, execution),
        |r| r.get(0),
    )?;
    if protected {
        return refuse("active_execution");
    }
    Ok(())
}

impl Store {
    /// Fails closed on active, missing or ambiguous execution evidence. Retry finishes only
    /// this task's committed deletion intent. It never age-selects or adopts another owner.
    pub fn purge_workspace_task(
        &self,
        project: &str,
        task: &str,
        reclaim: bool,
    ) -> CoreResult<Value> {
        self.purge_workspace_task_inner(project, task, reclaim, None)
    }

    /// Background callers have no adapter-only fallback and do not VACUUM
    /// the whole project database for each expired task.
    pub fn purge_expired_workspace_archive(
        &self,
        project: &str,
        task: &str,
        locked_record: &Value,
        now_ms: u64,
        retention_days: u64,
    ) -> CoreResult<Value> {
        self.purge_workspace_task_inner(project, task, false, Some((locked_record, now_ms, retention_days)))
    }

    fn purge_workspace_task_inner(
        &self,
        project: &str,
        task: &str,
        reclaim: bool,
        scheduled: Option<(&Value, u64, u64)>,
    ) -> CoreResult<Value> {
        if matches!(
            self.mode,
            OpenMode::LiveReadOnly | OpenMode::InspectImmutable
        ) {
            return refuse("inspection_read_only");
        }
        let _access = AccessLock::acquire(
            &self.key.directory().join(".access.lock"),
            false,
            true,
            Duration::ZERO,
        )?;
        self.key.validate_sidecars()?;
        let metadata = self.key.database().symlink_metadata()?;
        if (metadata.dev(), metadata.ino()) != self.identity {
            return refuse("storage_replaced");
        }
        let mut db = self
            .connection
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE project_id=? AND task_id=?)",
            (project, task),
            |r| r.get(0),
        )?;
        if !exists {
            return refuse("not_found");
        }
        let runtime: Option<String> = tx
            .query_row(
                "SELECT runtime_task_id FROM workspace_executions WHERE task_id=?",
                [task],
                |r| r.get(0),
            )
            .optional()?;
        let start: Option<String> = tx
            .query_row(
                "SELECT state FROM workspace_starts WHERE task_id=?",
                [task],
                |r| r.get(0),
            )
            .optional()?;
        if start.as_deref() == Some("unknown") || start.is_some() && runtime.is_none() {
            return refuse("active_execution");
        }
        if let Some((record, now_ms, retention_days)) = scheduled {
            if !expired_closed_archive(record, now_ms, retention_days) {
                return refuse("archive_not_expired");
            }
            let runtime = runtime.as_deref().ok_or(CoreError::Refused("active_execution"))?;
            // Native Workspace task directories use the t_<simple UUID>
            // namespace. An arbitrary ID must never name a private store area.
            if !runtime.strip_prefix("t_").is_some_and(|id| {
                id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return refuse("unsafe_storage");
            }
            scheduled_binding(&tx, self.key.root().to_str().ok_or(CoreError::Refused("invalid_project"))?, project, task, runtime)?;
        }
        if let Some(runtime) = &runtime {
            safe_id(runtime)?;
            let root: Option<String> = tx
                .query_row(
                    "SELECT project_root FROM tasks WHERE id=?",
                    [runtime],
                    |r| r.get(0),
                )
                .optional()?;
            if root.as_deref() != self.key.root().to_str() {
                return refuse("unauthorized");
            }
            quiet(&tx, runtime)?;
            tx.execute(
                "INSERT OR IGNORE INTO pending_deletions VALUES (?)",
                [runtime],
            )?;
        } else {
            adapter(&tx, project, task)?;
            tx.commit()?;
            return Ok(reclamation(self, &db, reclaim));
        }
        tx.commit()?;
        let runtime = runtime.unwrap();
        // Intent is durable before the first file removal. The exclusive access fence stays
        // held over both transactions; other cooperative stores cannot resume this task.
        let sources = db
            .prepare("SELECT path,device,inode,published_digest,id,kind FROM drafts WHERE task_id=?")?
            .query_map([&runtime], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if scheduled.is_some() {
            for (path, _, _, _, id, kind) in &sources {
                safe_id(id)?;
                let leaf = if kind == "pipeline" { "pipeline-drafts" } else { "draft-reports" };
                if Path::new(path) != self.key.directory().join(leaf).join(format!("{id}.md")) {
                    return refuse("unsafe_storage");
                }
            }
        }
        for (path, device, inode, sha, _, _) in sources {
            remove_source(
                self.key.root(),
                Path::new(&path),
                device,
                inode,
                sha.as_deref(),
            )?;
        }
        let snapshots=db.prepare("SELECT root_path,base_path FROM workspace_snapshots WHERE task_id=? AND cleaned_up_at IS NULL")?
            .query_map([&runtime],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        for (a, b) in snapshots {
            for path in [a, b].into_iter().flatten() {
                remove_tree(self, Path::new(&path), "workspaces")?;
            }
        }
        let stages = if schema::tables(&db)?.contains("candidate_staging") {
            db.prepare("SELECT stage_path FROM candidate_staging WHERE task_id=?")?
                .query_map([&runtime], |r| r.get::<_, Option<String>>(0))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        for path in stages.into_iter().flatten() {
            let parent = Path::new(&path)
                .parent()
                .ok_or(CoreError::Refused("unsafe_storage"))?;
            remove_tree(self, parent, "candidates")?;
        }
        let dir = self.key.directory().join(&runtime);
        if dir.symlink_metadata().is_ok() {
            locks::private_directory(&dir, false)?;
            fs::remove_dir_all(&dir)?;
            locks::sync_directory(&self.key.directory())?;
        }
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        quiet(&tx, &runtime)?;
        crate::workspace::worktrees::queue_purge_tx(&tx, project, task, &runtime)?;
        adapter(&tx, project, task)?;
        let tables = schema::tables(&tx)?;
        for sql in DELETIONS {
            let table = sql.split_whitespace().nth(2).unwrap();
            if !tables.contains(table) {
                continue;
            }
            if table == "deliveries" {
                tx.execute(sql, (&runtime, "create_task", "$.task_id", &runtime))?;
            } else {
                tx.execute(sql, [&runtime])?;
            }
        }
        tx.commit()?;
        Ok(reclamation(self, &db, reclaim))
    }
}
// Deletion is already committed. Reclamation failure must not invite replay.
fn reclamation(store: &Store, db: &Connection, reclaim: bool) -> Value {
    let mut result = json!({"deleted":true,"vacuum_status":"not_needed","reclaimed_bytes":0});
    if reclaim {
        let reclaimed = (|| -> CoreResult<u64> {
            let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
            if busy != 0 {
                return refuse("busy");
            }
            let before = store.key.database().metadata()?.len();
            db.execute_batch("VACUUM")?;
            let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
            if busy != 0 {
                return refuse("busy");
            }
            Ok(before.saturating_sub(store.key.database().metadata()?.len()))
        })();
        match reclaimed {
            Ok(n) => {
                result["vacuum_status"] = json!("completed");
                result["reclaimed_bytes"] = json!(n);
            }
            Err(_) => {
                result["vacuum_status"] = json!("failed");
                result["vacuum_error"] =
                    json!("space reclamation failed; deletion remains committed");
            }
        }
    }
    result
}
