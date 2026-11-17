//! SQLite UDF adapter. This is the ONLY raw/reentrant SQLite code in Core.
//! The callback never constructs/borrows a rusqlite Connection. Each nested statement
//! is prepared/bound/stepped/finalized via SQLite's documented same-connection UDF API.
//! No commit, rollback, hooked-table write, global pointer or escaped SQLite buffer.
//! SQLite owns callback lifetime; Store serializes its connection on a blocking executor.
use crate::{error::refuse, CoreResult};
use rusqlite::{ffi, functions::FunctionFlags, Connection};
use std::{ffi::CString, ptr};

const WRITERS: &[(&str, &str, &str)] = &[
    ("tasks", "runtime", "id"),
    ("runtime_tasks", "runtime", "task_id"),
    ("runtime_nodes", "runtime", "task_id"),
    ("runtime_events", "runtime", "task_id"),
    ("task_events", "runtime", "task_id"),
    ("task_orchestration", "runtime", "task_id"),
    ("task_owners", "runtime", "task_id"),
    ("pending_questions", "runtime", "task_id"),
    ("pending_question_late_answers", "runtime", "task_id"),
    ("task_language", "runtime", "task_id"),
    ("task_stopped", "runtime", "task_id"),
    ("acp_host_sessions", "session", "session_id"),
    ("acp_session_access_defaults", "session", "session_id"),
    ("host_transcript", "session", "session_id"),
    ("host_updates", "session", "session_id"),
];
pub fn install(db: &Connection) -> CoreResult<()> {
    let tables = super::schema::tables(db)?;
    let required = [
        "workspace_projects",
        "workspace_tasks",
        "workspace_executions",
        "workspace_requests",
        "workspace_starts",
        "workspace_outbox",
        "workspace_outbox_state",
    ];
    if !required.iter().all(|s| tables.contains(*s)) {
        return Ok(());
    }
    // SAFETY: handle stays valid until Connection destroys its registered UDF. The
    // closure is never called independently of SQLite, and owns no Connection ref.
    let handle = unsafe { db.handle() } as usize;
    db.create_scalar_function(
        "cortex_workspace_invalidate",
        2,
        FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            let scope = ctx.get::<String>(0)?;
            let identity = ctx.get::<String>(1)?;
            // SAFETY: invoked synchronously by this exact live SQLite connection.
            unsafe { invalidate(handle as *mut ffi::sqlite3, &scope, &identity) }.map_err(
                |_| {
                    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                        "outbox_append_failed",
                    )))
                },
            )?;
            Ok(0)
        },
    )?;
    let existing=db.prepare("SELECT name FROM main.sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'")?
        .query_map([],|r|r.get::<_,String>(0))?.collect::<Result<std::collections::BTreeSet<_>,_>>()?;
    for (table, scope, column) in WRITERS {
        if !tables.contains(*table) {
            continue;
        }
        let binding = if *scope == "runtime" {
            "runtime_task_id"
        } else {
            "acp_session_id"
        };
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let name = format!("workspace_outbox_{table}_{}", action.to_ascii_lowercase());
            if existing.contains(&name) {
                continue;
            }
            let row = if action == "DELETE" { "OLD" } else { "NEW" };
            let old = if action == "UPDATE" {
                format!("SELECT cortex_workspace_invalidate('{scope}',OLD.{column}) WHERE OLD.{column} IS NOT NEW.{column}
                    AND EXISTS (SELECT 1 FROM workspace_executions WHERE {binding}=OLD.{column});")
            } else {
                String::new()
            };
            db.execute_batch(&format!(
                "CREATE TEMP TRIGGER IF NOT EXISTS {name} AFTER {action} ON main.{table} BEGIN
                SELECT cortex_workspace_invalidate('{scope}',{row}.{column}) WHERE EXISTS
                (SELECT 1 FROM workspace_executions WHERE {binding}={row}.{column}); {old} END;"
            ))?;
        }
    }
    Ok(())
}

/// Raw statement owns exactly one sqlite3_stmt; column buffers are copied before step/finalize.
struct Statement(*mut ffi::sqlite3_stmt);
impl Drop for Statement {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_finalize(self.0);
        }
    }
}
impl Statement {
    unsafe fn new(db: *mut ffi::sqlite3, sql: &str, args: &[&str]) -> Result<Self, ()> {
        let sql = CString::new(sql).map_err(|_| ())?;
        let mut stmt = ptr::null_mut();
        let code = ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
        let value = Self(stmt);
        if code != ffi::SQLITE_OK {
            return Err(());
        }
        for (i, arg) in args.iter().enumerate() {
            let len = i32::try_from(arg.len()).map_err(|_| ())?;
            if ffi::sqlite3_bind_text(
                value.0,
                i as i32 + 1,
                arg.as_ptr().cast(),
                len,
                ffi::SQLITE_TRANSIENT(),
            ) != ffi::SQLITE_OK
            {
                return Err(());
            }
        }
        Ok(value)
    }
    unsafe fn text(&self, index: i32) -> Result<String, ()> {
        let value = ffi::sqlite3_column_text(self.0, index);
        if value.is_null() {
            return Err(());
        }
        // SQLite text is copied while this statement is still on its current row.
        let count = ffi::sqlite3_column_bytes(self.0, index) as usize;
        std::str::from_utf8(std::slice::from_raw_parts(value, count))
            .map(str::to_owned)
            .map_err(|_| ())
    }
}
unsafe fn exec(db: *mut ffi::sqlite3, sql: &str, args: &[&str]) -> Result<(), ()> {
    let s = Statement::new(db, sql, args)?;
    if ffi::sqlite3_step(s.0) != ffi::SQLITE_DONE {
        return Err(());
    }
    Ok(())
}
unsafe fn scalar(db: *mut ffi::sqlite3, sql: &str, args: &[&str]) -> Result<i64, ()> {
    let s = Statement::new(db, sql, args)?;
    if ffi::sqlite3_step(s.0) != ffi::SQLITE_ROW {
        return Err(());
    }
    Ok(ffi::sqlite3_column_int64(s.0, 0))
}
unsafe fn invalidate(db: *mut ffi::sqlite3, scope: &str, identity: &str) -> Result<(), ()> {
    let binding = match scope {
        "runtime" => "runtime_task_id",
        "session" => "acp_session_id",
        _ => return Err(()),
    };
    let mut rows = Vec::new();
    {
        let stmt = Statement::new(
            db,
            &format!(
                "SELECT e.execution_id,e.version,t.task_id,t.version,t.project_id
            FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE e.{binding}=?"
            ),
            &[identity],
        )?;
        loop {
            match ffi::sqlite3_step(stmt.0) {
                ffi::SQLITE_DONE => break,
                ffi::SQLITE_ROW => rows.push((
                    stmt.text(0)?,
                    ffi::sqlite3_column_int64(stmt.0, 1),
                    stmt.text(2)?,
                    ffi::sqlite3_column_int64(stmt.0, 3),
                    stmt.text(4)?,
                )),
                _ => return Err(()),
            }
        }
    }
    for (execution, ev, task, tv, project) in rows {
        let ev = ev.checked_add(1).ok_or(())?;
        let tv = tv.checked_add(1).ok_or(())?;
        exec(
            db,
            "UPDATE workspace_executions SET version=version+1 WHERE execution_id=?",
            &[&execution],
        )?;
        exec(
            db,
            "UPDATE workspace_tasks SET version=version+1 WHERE task_id=?",
            &[&task],
        )?;
        exec(
            db,
            "UPDATE workspace_projects SET version=version+1 WHERE project_id=?",
            &[&project],
        )?;
        append(db, &project, &task, "task", tv)?;
        append(db, &project, &execution, "execution", ev)?;
    }
    Ok(())
}
unsafe fn append(
    db: *mut ffi::sqlite3,
    project: &str,
    entity: &str,
    kind: &str,
    version: i64,
) -> Result<(), ()> {
    if version <= 0
        || project.is_empty()
        || project.chars().count() > 128
        || entity.is_empty()
        || entity.chars().count() > 128
    {
        return Err(());
    }
    exec(
        db,
        "INSERT OR IGNORE INTO workspace_outbox_state(project_id) VALUES (?)",
        &[project],
    )?;
    exec(
        db,
        "UPDATE workspace_outbox_state SET high_watermark=high_watermark+1 WHERE project_id=?",
        &[project],
    )?;
    let seq = scalar(
        db,
        "SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?",
        &[project],
    )?;
    if seq <= 0 {
        return Err(());
    }
    let payload = serde_json::json!({"projectId":project,"entityId":entity,"kind":kind,"entityVersion":version.to_string(),
        "type":"invalidated","schemaVersion":"1.0","mappingVersion":"1"});
    let digest = crate::digest(&serde_json::to_vec(&payload).map_err(|_| ())?);
    let event = format!("wev_{}", uuid::Uuid::new_v4().simple());
    exec(
        db,
        "INSERT INTO workspace_outbox VALUES (?,?,?,?,?,?,?,?)",
        &[
            project,
            &seq.to_string(),
            &event,
            entity,
            kind,
            &version.to_string(),
            &crate::now(),
            &digest,
        ],
    )?;
    let floor = (seq - 10_000).max(0).to_string();
    exec(
        db,
        "DELETE FROM workspace_outbox WHERE project_id=? AND sequence<=?",
        &[project, &floor],
    )?;
    exec(
        db,
        "UPDATE workspace_outbox_state SET retained_floor=max(retained_floor,?) WHERE project_id=?",
        &[&floor, project],
    )?;
    Ok(())
}
/// Public explicit append requires an explicit transaction; only metadata is persisted.
pub fn append_tx(
    db: &rusqlite::Transaction<'_>,
    project: &str,
    entity: &str,
    kind: &str,
    version: i64,
) -> CoreResult<()> {
    if !["task", "execution", "decision", "result", "unknown"].contains(&kind) || version <= 0 {
        return refuse("invalid_outbox_metadata");
    }
    // SAFETY: transaction holds Connection exclusively for the call. No rusqlite
    // statement/borrow is live while this raw statement adapter executes.
    unsafe { append(db.handle(), project, entity, kind, version) }
        .map_err(|_| crate::CoreError::Refused("outbox_append_failed"))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn bound() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(super::super::schema::SQL).unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        install(&db).unwrap();
        db.execute_batch("INSERT INTO tasks VALUES ('runtime','/fixture','now');
            INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('runtime','CREATED','goal','now','now');
            INSERT INTO workspace_projects VALUES ('p','/fixture','owner','now',1);
            INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('task','p','description','now');
            INSERT INTO workspace_executions VALUES ('execution','task','runtime','session',1,1,'now');").unwrap();
        db
    }
    #[test]
    fn reentrant_callback_works_in_autocommit_and_aborts_entire_statement_on_error() {
        let db = bound();
        assert!(db.is_autocommit());
        db.execute("UPDATE runtime_tasks SET goal='committed'", [])
            .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        db.execute_batch(
            "CREATE TRIGGER fail_second BEFORE INSERT ON workspace_outbox WHEN NEW.sequence%2=0
            BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(db
            .execute("UPDATE runtime_tasks SET goal='must-rollback'", [])
            .is_err());
        assert!(db.is_autocommit());
        assert_eq!(
            db.query_row("SELECT goal FROM runtime_tasks", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "committed"
        );
        assert_eq!(
            db.query_row(
                "SELECT high_watermark FROM workspace_outbox_state",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row("SELECT version FROM workspace_executions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn updates_moving_binding_invalidate_both_old_and_new_targets() {
        let db = bound();
        db.execute_batch("INSERT INTO tasks VALUES ('other','/fixture','now');
            INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('other','CREATED','goal','now','now');
            INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('other-task','p','description','now');
            INSERT INTO workspace_executions VALUES ('other-execution','other-task','other','other-session',1,1,'now');").unwrap();
        // Move a private session's binding column; use task_language's task_id
        // because it can move without breaking runtime_tasks' foreign keys.
        db.execute("INSERT INTO task_language VALUES ('runtime','en')", [])
            .unwrap();
        db.execute("UPDATE task_language SET task_id='other'", [])
            .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            6
        );
    }
    #[test]
    fn callback_registered_per_connection_cannot_publish_on_independent_writer() {
        let db = bound();
        let hooks:i64=db.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'",[],|r|r.get(0)).unwrap();
        assert_eq!(hooks, 0);
    }
}
