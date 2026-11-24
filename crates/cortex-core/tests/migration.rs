mod support;
use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, Store, StoreChannel};
use rusqlite::{config::DbConfig, types::Value, Connection, OpenFlags};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
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
// Historical project identity is part of the data. Never rewrite the archive
// or reuse a path owned by another run.
struct PythonFixture(PathBuf);

impl Drop for PythonFixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if !std::thread::panicking() {
                panic!("cannot remove disposable Python fixture: {error}");
            }
        }
    }
}

fn copy_private_tree(source: &Path, destination: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(destination)
        .unwrap();
    for entry in fs::read_dir(source).unwrap().map(Result::unwrap) {
        let kind = entry.file_type().unwrap();
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_private_tree(&entry.path(), &target);
        } else {
            assert!(kind.is_file());
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(target)
                .unwrap()
                .write_all(&fs::read(entry.path()).unwrap())
                .unwrap();
        }
    }
}

fn columns(db: &Connection, table: &str) -> Vec<String> {
    db.prepare("SELECT name FROM pragma_table_info(?) ORDER BY cid")
        .unwrap()
        .query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn rows(db: &Connection, table: &str, columns: &[String]) -> Vec<Vec<Value>> {
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    let projection = columns.iter().map(|name| quote(name)).collect::<Vec<_>>();
    db.prepare(&format!(
        "SELECT {} FROM {} ORDER BY rowid",
        projection.join(","),
        quote(table)
    ))
    .unwrap()
    .query_map([], |row| {
        (0..columns.len()).map(|index| row.get(index)).collect()
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn genuine_python_format11_migrates_without_losing_data() {
    let archive = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/python-era/format-11/store"
    ));
    let root = PathBuf::from("/tmp/cortex-python-era-fixture-v1");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let fixture = PythonFixture(root);
    let project = fixture.0.join("format-11");
    fs::DirBuilder::new().mode(0o700).create(&project).unwrap();
    let key = ProjectKey::new(&project, StoreChannel::Release).unwrap();
    copy_private_tree(archive, &key.directory());

    let original = Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            archive.join("cortex.sqlite3").display()
        ),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let original_bytes = fs::read(key.database()).unwrap();
    for mode in [OpenMode::InspectImmutable, OpenMode::LiveReadOnly] {
        assert_eq!(
            Store::open(key.clone(), mode).err().unwrap().code(),
            "legacy_storage_migration_required"
        );
        assert_eq!(fs::read(key.database()).unwrap(), original_bytes);
        assert!(!key.directory().join(".migration-backups").exists());
    }
    let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
    let backup_path: String = store
        .read(|db| {
            use cortex_core::store::schema;
            assert_eq!(schema::validate_header(db, false)?, 12);
            let original_tables = schema::tables(&original)?;
            let migrated_tables = schema::tables(db)?;
            let retired = [
                "hook_agent_bindings",
                "hook_command_sessions",
                "hook_events",
                "hook_hints",
                "hook_pending_sources",
            ];
            for table in original_tables.iter().filter(|name| !name.starts_with("sqlite_")) {
                if retired.contains(&table.as_str()) {
                    assert!(!migrated_tables.contains(table), "retired table {table}");
                } else if table != "format_history" {
                    let columns = columns(&original, table);
                    assert_eq!(rows(db, table, &columns), rows(&original, table, &columns), "{table}");
                }
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM host_transcript WHERE message_id IS NULL", [], |row| row.get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM work_graph_operations WHERE operation='legacy_requirement_state_migration'", [], |row| row.get::<_, i64>(0))?,
                2
            );
            Ok(db.query_row(
                "SELECT backup_path FROM format_history WHERE action='migrate' AND from_version=11 AND to_version=12",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let backup =
        Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    use cortex_core::store::schema;
    assert_eq!(schema::validate_header(&backup, false).unwrap(), 11);
    let original_tables = schema::tables(&original).unwrap();
    assert_eq!(schema::tables(&backup).unwrap(), original_tables);
    for table in original_tables {
        let columns = columns(&original, &table);
        assert_eq!(
            rows(&backup, &table, &columns),
            rows(&original, &table, &columns),
            "{table}"
        );
    }

    fn assert_files(source: &Path, installed: &Path) {
        for entry in fs::read_dir(source).unwrap().map(Result::unwrap) {
            let path = installed.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                assert_files(&entry.path(), &path);
            } else if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("cortex.sqlite3")
            {
                // The database and its -wal/-shm sidecars are SQLite-owned
                // volatile state: the live store rewrites the shared-memory
                // index and checkpoints the WAL. Their data is asserted row
                // by row above and through the verified backup.
                assert_eq!(
                    fs::read(&path).unwrap(),
                    fs::read(entry.path()).unwrap(),
                    "{path:?}"
                );
            }
        }
    }
    assert_files(archive, &key.directory());
}

#[test]
fn unsupported_format11_shape_rolls_back_without_rewriting_main_database() {
    for pending_wal in [false, true] {
        let p = Project::new();
        drop(p.open());
        let key = p.key();
        let db = Connection::open(key.database()).unwrap();
        if pending_wal {
            db.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .unwrap();
            db.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        }
        // Missing unrelated column: the additive install can run, but its
        // format-12 shape is incomplete. The hook drop must roll back too.
        db.execute_batch(
            "PRAGMA user_version=11;
             ALTER TABLE project_configuration_defaults DROP COLUMN updated_at;
             CREATE TABLE hook_events(id TEXT);
             INSERT INTO hook_events VALUES ('retired');",
        )
        .unwrap();
        drop(db);
        let before = fs::read(key.database()).unwrap();
        // In the WAL case the old main header is still format 12; only
        // SQLite's committed WAL view contains the format-11 witness.
        assert_eq!(
            u32::from_be_bytes(before[60..64].try_into().unwrap()),
            if pending_wal { 12 } else { 11 }
        );
        let error = Store::open(key.clone(), OpenMode::Initialize)
            .err()
            .unwrap();
        assert_eq!(error.code(), "incomplete_storage_schema");
        let diagnostic = error.storage_diagnostic().unwrap();
        assert_eq!(diagnostic.reason, "incomplete_storage_schema");
        assert!(!diagnostic.retryable);
        assert!(
            fs::read(key.database()).unwrap() == before,
            "main DB changed; pending WAL: {pending_wal}"
        );

        let db =
            Connection::open_with_flags(key.database(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            cortex_core::store::schema::validate_header(&db, false).unwrap(),
            11
        );
        assert_eq!(
            db.query_row("SELECT id FROM hook_events", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "retired"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM format_history", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
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
