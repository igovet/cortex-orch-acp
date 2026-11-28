use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeSet;
pub const APPLICATION_ID: i64 = 1129467218;
pub const FORMAT: i64 = 12;
pub const SQL: &str = include_str!("schema.sql");
/// Read-path indexes of format 12, created by `install` after every column they cover exists (a
/// format-11 `host_transcript` gains `message_id` only there). They are additive: each initializing
/// open creates a missing one inside its install transaction, so an existing store gains it on its next
/// open without a format change, and an older build reading a store that has it sees one more index.
/// - `host_transcript_by_message`: one worker's narration (`{attempt}:` message identities) of a session.
pub const READ_INDEXES: &str = "CREATE INDEX IF NOT EXISTS host_transcript_by_message ON host_transcript(session_id, kind, message_id);";
pub fn tables(db: &Connection) -> CoreResult<BTreeSet<String>> {
    Ok(db
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}
/// Stores whose table groups and columns this process already validated, by file identity and
/// SQLite schema cookie (which every schema change increments). A reopen of an unchanged schema
/// skips the per-table column checks; any change of the cookie or of the file runs them again.
static SHAPE_VALIDATED: std::sync::Mutex<BTreeSet<(String, u64, u64, i64)>> =
    std::sync::Mutex::new(BTreeSet::new());

pub fn validate_groups(db: &Connection) -> CoreResult<()> {
    use std::os::unix::fs::MetadataExt;
    let fingerprint = db.path().filter(|p| !p.is_empty()).and_then(|path| {
        let meta = std::fs::metadata(path).ok()?;
        let cookie = db.pragma_query_value(None, "schema_version", |r| r.get::<_, i64>(0)).ok()?;
        Some((path.to_owned(), meta.dev(), meta.ino(), cookie))
    });
    if let Some(known) = &fingerprint {
        if SHAPE_VALIDATED.lock().is_ok_and(|set| set.contains(known)) {
            return Ok(());
        }
    }
    validate_shape(db, false)?;
    if let (Some(known), Ok(mut set)) = (fingerprint, SHAPE_VALIDATED.lock()) {
        if set.len() >= 256 {
            set.clear();
        }
        set.insert(known);
    }
    Ok(())
}

/// Unlike inspection, an upgrade has just installed every known group.
pub(super) fn validate_installed(db: &Connection) -> CoreResult<()> {
    validate_shape(db, true)
}

fn validate_shape(db: &Connection, installed: bool) -> CoreResult<()> {
    let names = tables(db)?;
    let groups: Vec<Vec<String>> = serde_json::from_str(include_str!("groups.json"))?;
    for group in groups {
        let n = group.iter().filter(|s| names.contains(*s)).count();
        if (installed || n != 0) && n != group.len() {
            return refuse("partial_storage_schema");
        }
    }
    if !names.contains("tasks") {
        return refuse("unsupported_storage");
    }
    let columns: std::collections::BTreeMap<String, Vec<String>> =
        serde_json::from_str(include_str!("columns.json"))?;
    for (table, expected) in columns {
        if !names.contains(&table) {
            if installed {
                return refuse("partial_storage_schema");
            }
            continue;
        }
        let actual = db
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        if expected.iter().any(|c| !actual.contains(c)) {
            return refuse("incomplete_storage_schema");
        }
    }
    Ok(())
}
pub fn validate_header(db: &Connection, initialize: bool) -> CoreResult<i64> {
    let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if initialize && app == 0 && version == 0 && tables(db)?.is_empty() {
        return Ok(0);
    }
    if app != APPLICATION_ID {
        return refuse("unsupported_storage");
    }
    if version == 10 {
        return Err(CoreError::UnsupportedStorageFormat10);
    }
    if version == 11 {
        return Ok(11);
    }
    if version != FORMAT {
        return refuse("unsupported_storage");
    }
    Ok(version)
}
/// An already initialized store is empty only if it has the expected shape and
/// none of its tables contain records (including settings, history or upgrades).
pub(super) fn is_empty_store(db: &Connection) -> CoreResult<bool> {
    // The common populated case does not need to enumerate SQLite's schema.
    if !tables(db)?.contains("tasks")
        || db
            .query_row("SELECT 1 FROM tasks LIMIT 1", [], |_| Ok(()))
            .optional()?
            .is_some()
    {
        return Ok(false);
    }
    let mut statement = db.prepare(
        "SELECT name,type FROM pragma_table_list
         WHERE schema='main' AND name NOT GLOB 'sqlite_*' AND name<>'tasks'",
    )?;
    for table in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (name, kind) = table?;
        // Do not execute views or virtual tables just to classify an empty store.
        if kind != "table" {
            return Ok(false);
        }
        let identifier = name.replace('"', "\"\"");
        if db
            .query_row(
                &format!("SELECT 1 FROM main.\"{identifier}\" LIMIT 1"),
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Ok(false);
        }
    }
    validate_groups(db)?;
    Ok(true)
}
pub fn validate_project(db: &Connection, root: &str) -> CoreResult<()> {
    validate_project_has_tasks(db, root).map(|_| ())
}

/// Validate path binding and tell the opener if the project already has tasks.
pub(super) fn validate_project_has_tasks(db: &Connection, root: &str) -> CoreResult<bool> {
    if !tables(db)?.contains("tasks") {
        return Ok(false);
    }
    let bounds: (Option<String>, Option<String>) = db.query_row(
        "SELECT min(project_root),max(project_root) FROM tasks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if bounds.0.iter().chain(bounds.1.iter()).any(|s| s != root) {
        return refuse("project_storage_mismatch");
    }
    Ok(bounds.0.is_some())
}
fn add(db: &Connection, table: &str, column: &str, decl: &str) -> CoreResult<bool> {
    let exists = db
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|s| s == column);
    if !exists {
        db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    }
    Ok(!exists)
}
/// Called only under BEGIN IMMEDIATE / exclusive migration. No nested commits.
pub fn install(db: &Connection) -> CoreResult<()> {
    let old_triggers=db.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'")?
        .query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for name in old_triggers {
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return refuse("unsupported_storage");
        }
        db.execute_batch(&format!("DROP TRIGGER {name};"))?;
    }
    db.execute_batch(SQL)?;
    super::open_commands::install(db)?;
    db.execute_batch(crate::workspace::worktrees::SCHEMA)?;
    db.execute_batch(crate::workspace::worktrees::assignments::SCHEMA)?;
    db.execute_batch(super::owner_file_leases::SCHEMA)?;
    for (t, c, d) in [
        (
            "work_requirement_revisions",
            "requirement_type",
            "TEXT NOT NULL DEFAULT 'legacy_unclassified'",
        ),
        (
            "work_assignments",
            "requirement_state",
            "TEXT NOT NULL DEFAULT 'legacy_unclassified'",
        ),
        (
            "work_graph_operations",
            "request_json",
            "TEXT NOT NULL DEFAULT '{}'",
        ),
        ("task_events", "delivered_at", "TEXT"),
        ("task_orchestration", "wait_reason", "TEXT"),
        ("task_orchestration", "wait_detail_json", "TEXT"),
        (
            "task_orchestration",
            "no_progress_count",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "task_orchestration",
            "requirement_revision",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("task_orchestration", "stopped_at", "TEXT"),
        ("task_orchestration", "language", "TEXT"),
        ("source_events", "meta_json", "TEXT"),
        ("attempt_revisions", "manifest_json", "TEXT"),
        // Python format 11 did not record a message identity. Preserve NULL
        // for those rows rather than grouping distinct chunks under a made-up ID.
        ("host_transcript", "message_id", "TEXT"),
    ] {
        add(db, t, c, d)?;
    }
    if add(db, "source_events", "ledger_state", "TEXT")? {
        db.execute(
            "UPDATE source_events SET ledger_state='legacy_recorded' WHERE ledger_state IS NULL",
            [],
        )?;
    }
    db.execute("UPDATE attempt_revisions SET manifest_json=NULL WHERE manifest_json IS NOT NULL AND
        (phase LIKE 'worker_publish:%' OR (phase LIKE 'worker_start:%' AND attempt_id IN
        (SELECT attempt_id FROM attempt_process_registry WHERE terminated_at IS NOT NULL AND termination_reason IS NOT 'timeout_alive')))",[])?;
    legacy_requirements(db)?;
    db.execute_batch(READ_INDEXES)?;
    // Setting a pragma rewrites the database header even to the same value, which turns every
    // initializing open of an installed store into a synced write transaction: write only a change.
    let current = |name: &str| db.pragma_query_value(None, name, |r| r.get::<_, i64>(0));
    if current("application_id")? != APPLICATION_ID {
        db.pragma_update(None, "application_id", APPLICATION_ID)?;
    }
    if current("user_version")? != FORMAT {
        db.pragma_update(None, "user_version", FORMAT)?;
    }
    Ok(())
}
fn legacy_requirements(db: &Connection) -> CoreResult<()> {
    let key = "__migration__:legacy-requirement-state:v1";
    let tasks=db.prepare("SELECT id FROM tasks WHERE id NOT IN (SELECT task_id FROM work_graph_operations WHERE idempotency_key=?)")?
        .query_map([key],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for task in tasks {
        let events=db.prepare("SELECT assignment_ref,payload_json,created_at FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND superseded=0 AND assignment_ref IS NOT NULL")?
            .query_map([&task],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (assignment, payload, at) in events {
            let value: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            if value["structured_requirement_state"] == "structured" {
                continue;
            }
            let old:Option<String>=db.query_row("SELECT requirement_state FROM work_assignments WHERE task_id=? AND assignment_id=?",(&task,&assignment),|r|r.get(0)).optional()?;
            if old.is_some() {
                db.execute("UPDATE work_assignments SET requirement_state='legacy_unclassified' WHERE task_id=? AND assignment_id=? AND requirement_state='structured'",(&task,&assignment))?;
            } else {
                db.execute("INSERT INTO work_assignments(assignment_id,task_id,node_key,requirement_bindings_json,candidate_ref,status,requirement_state,idempotency_key,created_at) VALUES (?,?,?,'{}',NULL,'planned','legacy_unclassified',?,?)",
                    (&assignment,&task,format!("legacy:{assignment}"),format!("legacy-assignment:{assignment}"),&at))?;
            }
        }
        db.execute("INSERT INTO work_graph_operations(task_id,idempotency_key,operation,request_json,response_json,created_at) VALUES (?,?,'legacy_requirement_state_migration','{}','{}',?)",(&task,key,crate::now()))?;
    }
    Ok(())
}
