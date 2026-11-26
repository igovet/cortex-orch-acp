mod support;

use cortex_core::{store::locks::AccessLock, CoreError, OpenMode, ProjectKey, Store, StoreChannel};
use rusqlite::Connection;
use std::{
    fs::{self, File},
    os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
    path::Path,
    time::Duration,
};
use support::Project;

const MODES: [OpenMode; 4] = [
    OpenMode::Initialize,
    OpenMode::RuntimeValidate,
    OpenMode::InspectImmutable,
    OpenMode::LiveReadOnly,
];

fn diagnostic(err: CoreError, reason: &str, recovery: &str, retryable: bool) {
    let d = err.storage_diagnostic().expect("storage diagnostic");
    assert_eq!(err.code(), reason);
    assert_eq!(d.reason, reason);
    assert!(d.message.contains(recovery), "{}", d.message);
    assert_eq!(d.retryable, retryable);
}

fn release(p: &Project) -> ProjectKey {
    ProjectKey::new(&p.0, StoreChannel::Release).unwrap()
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

#[test]
fn format11_refusal_explains_how_to_request_the_verified_upgrade() {
    let p = Project::new();
    drop(p.open());
    let key = p.key();
    let db = Connection::open(key.database()).unwrap();
    db.pragma_update(None, "user_version", 11).unwrap();
    drop(db);
    let before = fs::read(key.database()).unwrap();
    for mode in [OpenMode::InspectImmutable, OpenMode::LiveReadOnly] {
        let err = Store::open(key.clone(), mode).err().unwrap();
        let d = err.storage_diagnostic().unwrap();
        assert!(d
            .message
            .contains("writable mode with a current Cortex release"));
        diagnostic(
            err,
            "legacy_storage_migration_required",
            ".migration-backups/",
            false,
        );
    }
    assert_eq!(fs::read(key.database()).unwrap(), before);
    assert!(!key.directory().join(".migration-backups").exists());
}

#[test]
fn partial_and_incomplete_schemas_and_moved_projects_have_distinct_diagnostics() {
    for (sql, reason, recovery) in [
        (
            "DROP TABLE workspace_task_options",
            "partial_storage_schema",
            "part of a group of required tables",
        ),
        (
            "ALTER TABLE tasks RENAME COLUMN created_at TO other_created_at",
            "incomplete_storage_schema",
            "missing required columns",
        ),
        (
            "INSERT INTO tasks VALUES ('task','/original-path','now')",
            "project_storage_mismatch",
            "original path",
        ),
    ] {
        let p = Project::new();
        drop(p.open());
        let key = p.key();
        let db = Connection::open(key.database()).unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        let before = fs::read(key.database()).unwrap();
        diagnostic(
            Store::open(key.clone(), OpenMode::InspectImmutable)
                .err()
                .unwrap(),
            reason,
            recovery,
            false,
        );
        assert_eq!(fs::read(key.database()).unwrap(), before);
    }
}

#[test]
fn busy_is_retryable_but_missing_or_unsafe_store_needs_a_different_action() {
    let p = Project::new();
    let key = release(&p);
    diagnostic(
        Store::open(key.clone(), OpenMode::LiveReadOnly)
            .err()
            .unwrap(),
        "storage_unavailable",
        "files and disk are available",
        false,
    );
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
    let lock = AccessLock::acquire(
        &key.directory().join(".access.lock"),
        false,
        true,
        Duration::ZERO,
    )
    .unwrap();
    diagnostic(
        Store::open(key.clone(), OpenMode::Initialize)
            .err()
            .unwrap(),
        "busy",
        "then try again",
        true,
    );
    drop(lock);
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
    fs::set_permissions(key.database(), fs::Permissions::from_mode(0o644)).unwrap();
    diagnostic(
        Store::open(key, OpenMode::InspectImmutable).err().unwrap(),
        "unsafe_storage",
        "ownership or permissions",
        false,
    );
    assert!(CoreError::Refused("invalid_cursor")
        .storage_diagnostic()
        .is_none());
}

#[test]
fn legacy_database_is_reported_in_all_release_modes_without_creating_a_store() {
    let p = Project::new();
    let key = release(&p);
    let legacy = p.0.join(".codex/cortex");
    fs::create_dir_all(&legacy).unwrap();
    let old_db = legacy.join("cortex.sqlite3");
    // A marker only: the detector must not open or validate the old DB itself.
    fs::write(&old_db, b"legacy canary").unwrap();
    for mode in MODES {
        diagnostic(
            Store::open(key.clone(), mode).err().unwrap(),
            "legacy_storage_root_detected",
            "Nothing was moved",
            false,
        );
        assert!(!key.directory().exists());
    }
    drop(p.open());
    assert_eq!(fs::read(&old_db).unwrap(), b"legacy canary");
    assert_eq!(fs::read_dir(&legacy).unwrap().count(), 1);
    assert!(!key.directory().exists());
}

#[test]
fn legacy_task_tree_is_reported_with_empty_release_store() {
    for state in ["zero byte", "schema-free SQLite", "initialized"] {
        let p = Project::new();
        let key = release(&p);
        let live = if state == "initialized" {
            drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
            // Start with any ordinary LiveReadOnly WAL/SHM housekeeping already
            // done, so the inventory comparison isolates the diagnostic itself.
            Some(Store::open(key.clone(), OpenMode::LiveReadOnly).unwrap())
        } else {
            private_dir(&key.directory());
            File::create(key.database()).unwrap();
            fs::set_permissions(key.database(), fs::Permissions::from_mode(0o600)).unwrap();
            if state == "schema-free SQLite" {
                let db = Connection::open(key.database()).unwrap();
                db.execute_batch("VACUUM").unwrap();
                assert!(fs::metadata(key.database()).unwrap().len() > 0);
                let lock = key.directory().join(".access.lock");
                File::create(&lock).unwrap();
                fs::set_permissions(lock, fs::Permissions::from_mode(0o600)).unwrap();
            }
            None
        };
        let tree = p.0.join(".codex/cortex/t_0123456789ab");
        fs::create_dir_all(&tree).unwrap();
        let report = tree.join("pipeline.md");
        fs::write(&report, b"# Legacy pipeline\n").unwrap();
        let before = fs::read(key.database()).unwrap();
        let entries = fs::read_dir(key.directory()).unwrap().count();
        for mode in MODES {
            diagnostic(
                Store::open(key.clone(), mode).err().unwrap(),
                "legacy_storage_root_detected",
                "historical Cortex release on a copy",
                false,
            );
            assert_eq!(fs::read(key.database()).unwrap(), before);
            assert_eq!(fs::read(&report).unwrap(), b"# Legacy pipeline\n");
            assert_eq!(fs::read_dir(key.directory()).unwrap().count(), entries);
        }
        drop(live);
    }
}

#[test]
fn canonical_project_defaults_even_without_tasks_take_priority_over_legacy_markers() {
    let p = Project::new();
    let key = release(&p);
    let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
    store
        .write(|db| {
            db.execute(
                "INSERT INTO project_configuration_defaults VALUES (?, '{}', '{}', 'now')",
                [key.root().to_str().unwrap()],
            )?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let legacy = p.0.join(".codex/cortex");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("cortex.sqlite3"), b"old DB").unwrap();
    for mode in MODES {
        let store = Store::open(key.clone(), mode).unwrap();
        assert_eq!(support::count(&store, "project_configuration_defaults"), 1);
        assert_eq!(support::count(&store, "tasks"), 0);
    }
}

#[test]
fn unrelated_entries_and_symlinked_old_roots_are_not_legacy_storage() {
    for linked in [false, true] {
        let p = Project::new();
        let key = release(&p);
        let old = if linked {
            fs::create_dir(p.0.join(".codex")).unwrap();
            let target = Project::new();
            fs::write(target.0.join("cortex.sqlite3"), b"outside project").unwrap();
            symlink(&target.0, p.0.join(".codex/cortex")).unwrap();
            Some(target)
        } else {
            let legacy = p.0.join(".codex/cortex");
            fs::create_dir_all(legacy.join("t_unrelated")).unwrap();
            fs::write(legacy.join("t_000000000000"), b"not a directory").unwrap();
            fs::write(legacy.join("routing.json"), b"{}").unwrap();
            None
        };
        drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
        assert!(key.database().is_file());
        if let Some(target) = old {
            assert_eq!(
                fs::read(target.0.join("cortex.sqlite3")).unwrap(),
                b"outside project"
            );
        }
    }
}

/// A read the owner waits for queues behind an exclusive access lock (a migration or an
/// initialization by this service) and completes once it is released, past the two
/// seconds after which an ordinary open reports `busy`.
#[test]
fn a_waiting_read_outlasts_an_exclusive_access_lock_and_then_completes() {
    use std::time::Instant;
    let p = Project::new();
    let key = release(&p);
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
    let reader = Store::open_waiting(key.clone(), OpenMode::LiveReadOnly).unwrap();
    let hold = |millis| {
        let path = key.directory().join(".access.lock");
        let held = AccessLock::acquire(&path, false, true, Duration::ZERO).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(millis));
            drop(held);
        })
    };
    // Opening while the lock is held waits, and finishes after the release.
    let holder = hold(2600);
    let started = Instant::now();
    let opened = Store::open_waiting(key.clone(), OpenMode::LiveReadOnly);
    assert!(opened.is_ok(), "{:?}", opened.err());
    assert!(started.elapsed() >= Duration::from_millis(2500));
    holder.join().unwrap();
    // A reader opened before the lock was taken waits on each read too.
    let holder = hold(2600);
    let started = Instant::now();
    assert_eq!(reader.read(|db| Ok(db.query_row("SELECT 7", [], |r| r.get::<_, i64>(0))?)).unwrap(), 7);
    assert!(started.elapsed() >= Duration::from_millis(2500));
    holder.join().unwrap();
    // The ordinary open still reports busy for the same holder.
    let holder = hold(2600);
    diagnostic(
        Store::open(key, OpenMode::LiveReadOnly).err().unwrap(),
        "busy",
        "then try again",
        true,
    );
    holder.join().unwrap();
}
