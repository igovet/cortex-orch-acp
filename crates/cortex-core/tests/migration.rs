mod support;
use cortex_core::{lifecycle::TaskMachine, OpenMode, Store};
use rusqlite::Connection;
use support::*;
#[test]
fn format11_requires_migration_on_reads_and_writes_verified_private_backup() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    drop(s);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("PRAGMA user_version=11; CREATE TABLE hook_events(id TEXT); INSERT INTO hook_events VALUES ('retired');").unwrap();
    drop(db);
    let before = std::fs::read(p.key().database()).unwrap();
    for mode in [OpenMode::RuntimeValidate, OpenMode::InspectImmutable] {
        assert_eq!(
            Store::open(p.key(), mode).err().unwrap().code(),
            "legacy_storage_migration_required"
        );
    }
    assert_eq!(before, std::fs::read(p.key().database()).unwrap());
    let s = p.open();
    assert_eq!(count(&s, "tasks"), 1);
    let (backup, digest) = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT backup_path,backup_digest FROM format_history",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?)
        })
        .unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(&backup).unwrap())),
        digest
    );
    let b =
        Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        b.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM hook_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    s.read(|db| {
        assert!(!cortex_core::store::schema::tables(db)?.contains("hook_events"));
        Ok(())
    })
    .unwrap();
}
#[test]
fn migration_failure_rolls_back_schema_and_retains_backup() {
    let p = Project::new();
    drop(p.open());
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("PRAGMA user_version=11; CREATE TRIGGER fail_migration BEFORE INSERT ON format_history BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    drop(db);
    assert!(Store::open(p.key(), OpenMode::Initialize).is_err());
    let db = Connection::open(p.key().database()).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
    assert_eq!(
        std::fs::read_dir(p.key().directory().join(".migration-backups"))
            .unwrap()
            .count(),
        1
    );
}
#[test]
fn additive_legacy_columns_do_not_reorder_or_reinterpret_backfilled_values() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    drop(s);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("DROP TABLE work_assignments; DROP TABLE work_graph_operations;
        CREATE TABLE work_assignments (
            assignment_id TEXT PRIMARY KEY,task_id TEXT NOT NULL REFERENCES tasks(id),
            node_key TEXT NOT NULL,requirement_bindings_json TEXT NOT NULL,candidate_ref TEXT,
            status TEXT NOT NULL,idempotency_key TEXT NOT NULL,created_at TEXT NOT NULL,
            UNIQUE(task_id,idempotency_key));
        CREATE TABLE work_graph_operations (
            task_id TEXT NOT NULL REFERENCES tasks(id),idempotency_key TEXT NOT NULL,
            operation TEXT NOT NULL,response_json TEXT NOT NULL,created_at TEXT NOT NULL,
            PRIMARY KEY(task_id,idempotency_key));
        INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
            VALUES ('t','delegation_requested','legacy','legacy','{}','runtime','original-time');").unwrap();
    drop(db);
    let s = p.open();
    s.read(|db| {
        let row: (String, String, String, String) = db.query_row(
            "SELECT status,requirement_state,idempotency_key,created_at FROM work_assignments",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        assert_eq!(
            row,
            (
                "planned".into(),
                "legacy_unclassified".into(),
                "legacy-assignment:legacy".into(),
                "original-time".into()
            )
        );
        let row: (String, String, String) = db.query_row(
            "SELECT operation,request_json,response_json FROM work_graph_operations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(
            row,
            (
                "legacy_requirement_state_migration".into(),
                "{}".into(),
                "{}".into()
            )
        );
        Ok(())
    })
    .unwrap();
}
