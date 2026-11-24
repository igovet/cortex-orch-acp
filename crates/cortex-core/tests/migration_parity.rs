mod support;
use cortex_core::{
    lifecycle::TaskMachine,
    store::{
        documents::{Publication, Publisher},
        locks::AccessLock,
        migration::restore,
    },
    OpenMode, Store,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
use support::*;
fn digest(path: impl AsRef<std::path::Path>) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}
fn legacy(p: &Project) {
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    drop(s);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("PRAGMA user_version=11; CREATE TABLE hook_events(id TEXT); INSERT INTO hook_events VALUES ('retired');
        CREATE TABLE hook_pending_sources(id TEXT); CREATE TABLE hook_hints(id TEXT); CREATE TABLE hook_agent_bindings(id TEXT); CREATE TABLE hook_command_sessions(id TEXT);").unwrap();
}
fn backup(p: &Project) -> (PathBuf, String) {
    let s = p.open();
    s.read(|db| {
        Ok(db.query_row(
            "SELECT backup_path,backup_digest FROM format_history ORDER BY sequence DESC LIMIT 1",
            [],
            |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?)),
        )?)
    })
    .unwrap()
}
fn version(p: &Project) -> i64 {
    Connection::open_with_flags(p.key().database(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
#[test] // Python tests/test_format_migration.py::test_store_open_automatically_migrates_format11_and_keeps_verified_backup
fn store_open_automatically_migrates_format11_and_keeps_verified_backup() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    assert_eq!(digest(&path), sha);
    assert_eq!(version(&p), 12);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    drop(p.open());
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}
#[test] // Python test_migrate_format11_removes_legacy_hook_tables_preserves_acp_data
fn migrate_format11_removes_legacy_hook_tables_preserves_acp_data() {
    let p = Project::new();
    legacy(&p);
    let s = p.open();
    assert_eq!(count(&s, "tasks"), 1);
    s.read(|db| {
        let names = cortex_core::store::schema::tables(db)?;
        for table in [
            "hook_events",
            "hook_hints",
            "hook_pending_sources",
            "hook_agent_bindings",
            "hook_command_sessions",
        ] {
            assert!(!names.contains(table));
        }
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?,
            "ok"
        );
        Ok(())
    })
    .unwrap();
}
#[test] // Python test_non_initializing_open_refuses_format11_without_mutating
fn non_initializing_open_refuses_format11_without_mutating() {
    let p = Project::new();
    legacy(&p);
    let before = digest(p.key().database());
    let names: Vec<_> = std::fs::read_dir(p.key().directory())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    for mode in [OpenMode::InspectImmutable, OpenMode::RuntimeValidate] {
        assert_eq!(
            Store::open(p.key(), mode).err().unwrap().code(),
            "legacy_storage_migration_required"
        );
    }
    assert_eq!(before, digest(p.key().database()));
    assert!(!p.key().directory().join(".migration-backups").exists());
    let after: Vec<_> = std::fs::read_dir(p.key().directory())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, after);
}
#[test] // Python test_migration_backup_includes_committed_wal_rows
fn migration_backup_includes_committed_wal_rows() {
    let p = Project::new();
    legacy(&p);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    db.execute(
        "INSERT INTO tasks VALUES ('wal-task',?, 'now')",
        [p.key().root().to_str().unwrap()],
    )
    .unwrap();
    // The raw SQLite handle keeps committed data in WAL, without a cooperating Core lock.
    let (path, _) = backup(&p);
    let b = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        b.query_row("SELECT count(*) FROM tasks WHERE id='wal-task'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert_eq!(version(&p), 12);
    drop(db);
}
#[test] // Python test_automatic_migration_failure_leaves_format11_for_retry
fn automatic_migration_failure_leaves_format11_for_retry() {
    let p = Project::new();
    legacy(&p);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_migration BEFORE INSERT ON format_history BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    drop(db);
    assert!(Store::open(p.key(), OpenMode::Initialize).is_err());
    assert_eq!(version(&p), 11);
    assert_eq!(
        std::fs::read_dir(p.key().directory().join(".migration-backups"))
            .unwrap()
            .count(),
        1
    );
    let db = Connection::open(p.key().database()).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM hook_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    db.execute_batch("DROP TRIGGER fail_migration;").unwrap();
    drop(db);
    drop(p.open());
    assert_eq!(version(&p), 12);
}
#[test] // Python test_rollback_restores_verified_backup_byte_for_byte
fn rollback_restores_verified_backup_byte_for_byte() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    let r = restore(&p.key(), &path, &sha, true, true).unwrap();
    assert_eq!(r.restored_version, 11);
    assert_eq!(r.backup_digest, sha);
    assert_eq!(
        std::fs::read(p.key().database()).unwrap(),
        std::fs::read(path).unwrap()
    );
    assert_eq!(version(&p), 11);
}
#[test] // Python test_rollback_requires_access_stopped_and_explicit_confirm
fn rollback_requires_access_stopped_and_explicit_confirm() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    let old = digest(p.key().database());
    assert_eq!(
        restore(&p.key(), &path, &sha, false, true)
            .unwrap_err()
            .code(),
        "migration_requires_stopped_access"
    );
    assert_eq!(
        restore(&p.key(), &path, &sha, true, false)
            .unwrap_err()
            .code(),
        "rollback_requires_confirmation"
    );
    assert_eq!(old, digest(p.key().database()));
}
#[test] // Python test_rollback_refused_while_a_store_connection_is_open
fn rollback_refused_while_a_store_connection_is_open() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    let s = p.open();
    assert_eq!(
        restore(&p.key(), &path, &sha, true, true)
            .unwrap_err()
            .code(),
        "busy"
    );
    drop(s);
    restore(&p.key(), &path, &sha, true, true).unwrap();
}
#[test] // Python test_second_mutating_migration_process_is_refused_while_first_holds_the_lock
fn second_mutating_migration_is_refused_while_first_holds_lock() {
    let p = Project::new();
    legacy(&p);
    let lock = AccessLock::acquire(
        &p.key().directory().join(".access.lock"),
        false,
        true,
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(
        Store::open(p.key(), OpenMode::Initialize)
            .err()
            .unwrap()
            .code(),
        "busy"
    );
    drop(lock);
    drop(p.open());
    assert_eq!(version(&p), 12);
}
#[test] // Python test_rollback_refuses_corrupt_backup + test_rollback_refuses_backup_from_a_foreign_application + test_rollback_refuses_an_unknown_newer_format
fn rollback_refuses_corrupt_foreign_and_unknown_backup() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    let old = digest(p.key().database());
    assert_eq!(
        restore(&p.key(), &path, "wrong", true, true)
            .unwrap_err()
            .code(),
        "migration_backup_invalid"
    );
    let bytes = std::fs::read(&path).unwrap();
    for sql in [
        "PRAGMA application_id=1",
        "PRAGMA user_version=999",
        "PRAGMA user_version=10",
    ] {
        std::fs::write(&path, &bytes).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        assert_eq!(
            restore(&p.key(), &path, &digest(&path), true, true)
                .unwrap_err()
                .code(),
            "rollback_backup_incompatible"
        );
        assert_eq!(old, digest(p.key().database()));
    }
    std::fs::write(&path, b"invalid database").unwrap();
    assert!(restore(&p.key(), &path, &digest(&path), true, true).is_err());
    assert_eq!(old, digest(p.key().database()));
    assert_ne!(sha, digest(&path));
}
#[test] // Python test_rollback_requires_private_backup_parent
fn rollback_requires_private_backup_parent_and_rejects_links() {
    let p = Project::new();
    legacy(&p);
    let (path, sha) = backup(&p);
    let parent = path.parent().unwrap();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        restore(&p.key(), &path, &sha, true, true)
            .unwrap_err()
            .code(),
        "unsafe_storage"
    );
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = parent.join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert_eq!(
        restore(&p.key(), &link, &sha, true, true)
            .unwrap_err()
            .code(),
        "unsafe_storage"
    );
}
#[test] // Python test_migrate_preserves_reports_drafts_and_every_row_it_does_not_touch + test_rollback_never_touches_markdown_engine_state_or_credentials
fn migration_and_restore_preserve_markdown_drafts_and_unrelated_state() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\nreport", d.draft_id),
    )
    .unwrap();
    let r = api
        .publish(
            "t",
            "owner",
            &d.draft_id,
            "p",
            Publication {
                title: "title",
                summary: "summary",
                author: "worker",
                source_revision: 0,
                artifacts: serde_json::json!([]),
            },
        )
        .unwrap();
    let draft = api
        .create_draft("t", "owner", "general", "retained")
        .unwrap();
    let report = p
        .key()
        .directory()
        .join("t")
        .join(format!("{}.md", r.report_id));
    let before = std::fs::read(&report).unwrap();
    let draft_before = std::fs::read(&draft.draft_path).unwrap();
    // Ordinary unrelated private file, not an actual credential.
    let other = p.key().directory().join("unrelated");
    std::fs::write(&other, b"untouched").unwrap();
    drop(s);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("PRAGMA user_version=11").unwrap();
    drop(db);
    let (path, sha) = backup(&p);
    restore(&p.key(), &path, &sha, true, true).unwrap();
    assert_eq!(std::fs::read(report).unwrap(), before);
    assert_eq!(std::fs::read(draft.draft_path).unwrap(), draft_before);
    assert_eq!(std::fs::read(other).unwrap(), b"untouched");
    let s = p.open();
    assert_eq!(count(&s, "reports"), 1);
    assert_eq!(count(&s, "deliveries"), 3);
    assert_eq!(count(&s, "drafts"), 2);
}
#[test] // Python format-10/unknown normal-open fail-closed contract
fn format10_and_unknown_fail_closed_without_backup() {
    for v in [10, 99] {
        let p = Project::new();
        legacy(&p);
        let db = Connection::open(p.key().database()).unwrap();
        db.pragma_update(None, "user_version", v).unwrap();
        drop(db);
        let before = digest(p.key().database());
        for mode in [
            OpenMode::Initialize,
            OpenMode::InspectImmutable,
            OpenMode::RuntimeValidate,
            OpenMode::LiveReadOnly,
        ] {
            let error = Store::open(p.key(), mode).err().unwrap();
            let diagnostic = error.storage_diagnostic().unwrap();
            assert_eq!(error.code(), "unsupported_storage");
            assert_eq!(error.to_string(), "unsupported_storage");
            assert_eq!(diagnostic.reason, "unsupported_storage");
            assert!(!diagnostic.retryable);
            if v == 10 {
                assert!(diagnostic
                    .message
                    .contains("format 10 from the retired Python Cortex"));
                assert!(diagnostic
                    .message
                    .contains("historical Cortex archive on a copy"));
            } else {
                assert!(!diagnostic.message.contains("format 10"));
                assert!(diagnostic
                    .message
                    .contains("restore a verified compatible backup"));
            }
            assert_eq!(before, digest(p.key().database()));
            assert!(!p.key().directory().join(".migration-backups").exists());
        }
    }
}
