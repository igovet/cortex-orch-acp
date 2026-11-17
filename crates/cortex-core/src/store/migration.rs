//! Verified format-11 -> 12 migration and offline restore.
//! Mirrors migrate.py:141-275 and rollback:278-377. Maintenance holds the
//! exclusive access fence; restore also excludes live reusable Rust connections.
use super::{
    locks::{self, AccessLock},
    schema, ProjectKey,
};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{backup::Backup, Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct RestoreReceipt {
    pub restored_version: i64,
    pub backup_digest: String,
}
// Crash injection exists only in the unit-test executable, never in the library.
fn checkpoint(_point: &str) {
    #[cfg(test)]
    if std::env::var("CORTEX_CORE_MIGRATION_CRASH_POINT").as_deref() == Ok(_point) {
        std::process::exit(97);
    }
}
fn create(path: &Path) -> CoreResult<File> {
    Ok(fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}
fn digest(path: &Path) -> CoreResult<String> {
    let mut f = locks::open_private(path, false, true)?;
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify(db: &Connection, version: i64) -> CoreResult<()> {
    if schema::validate_header(db, false)? != version
        || db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))? != "ok"
        || db.prepare("PRAGMA foreign_key_check")?.exists([])?
    {
        return refuse("migration_integrity_failed");
    }
    Ok(())
}
/// Internal: Store::open holds exclusive access and connection fences.
pub(super) fn upgrade11(db: &mut Connection, directory: &Path) -> CoreResult<()> {
    if schema::validate_header(db, false)? != 11 {
        return refuse("unsupported_storage");
    }
    let backups = directory.join(".migration-backups");
    locks::private_directory(&backups, true)?;
    checkpoint("before_backup");
    let path = backups.join(format!(
        "cortex.sqlite3.format11-{}.bak",
        uuid::Uuid::new_v4().simple()
    ));
    drop(create(&path)?);
    let mut backup = Connection::open(&path)?;
    Backup::new(db, &mut backup)?.run_to_completion(128, Duration::from_millis(5), None)?;
    verify(&backup, 11)?;
    File::open(&path)?.sync_all()?;
    locks::sync_directory(&backups)?;
    let backup_digest = digest(&path)?;
    checkpoint("after_backup");
    // Every error before commit drops/rolls back the whole schema transaction.
    let result = (|| {
        let tx = db.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        schema::install(&tx)?;
        for table in [
            "hook_pending_sources",
            "hook_events",
            "hook_hints",
            "hook_agent_bindings",
            "hook_command_sessions",
        ] {
            tx.execute_batch(&format!("DROP TABLE IF EXISTS {table};"))?;
        }
        verify(&tx, 12)?;
        tx.execute(
            "INSERT INTO format_history VALUES (NULL,'migrate',11,12,?,?,?)",
            (
                path.to_str().ok_or(CoreError::Refused("unsafe_storage"))?,
                &backup_digest,
                crate::now(),
            ),
        )?;
        checkpoint("before_commit");
        tx.commit()?;
        checkpoint("after_commit");
        Ok(())
    })();
    if let Err(error) = result {
        // Restore the verified logical snapshot, including committed WAL rows,
        // even if schema/commit failed. Never silently continue a partial upgrade.
        if digest(&path)? != backup_digest {
            return refuse("migration_backup_invalid");
        }
        Backup::new(&backup, db)?.run_to_completion(128, Duration::from_millis(5), None)?;
        verify(db, 11)?;
        return Err(error);
    }
    Ok(())
}
/// Explicit offline restore. No Markdown, drafts, accounts or engine state are
/// traversed. The exact verified backup bytes replace only the database.
/// A failed pre-replace validation leaves the target untouched. On interruption
/// retrying the same backup is safe; stale private staging files are disposable.
pub fn restore(
    key: &ProjectKey,
    backup: &Path,
    expected_digest: &str,
    access_stopped: bool,
    confirm: bool,
) -> CoreResult<RestoreReceipt> {
    if !access_stopped {
        return refuse("migration_requires_stopped_access");
    }
    if !confirm {
        return refuse("rollback_requires_confirmation");
    }
    let dir = key.directory();
    locks::private_directory(&dir, false)?;
    let parent = backup
        .parent()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    locks::private_directory(parent, false)?;
    locks::regular(backup)?;
    if backup == key.database() {
        return refuse("rollback_backup_incompatible");
    }
    let _access = AccessLock::acquire(&dir.join(".access.lock"), false, true, Duration::ZERO)?;
    let _connections =
        AccessLock::acquire(&dir.join(".connections.lock"), true, true, Duration::ZERO)?;
    key.validate_sidecars()?;
    let log = dir.join("rollback.log");
    if log.symlink_metadata().is_ok() {
        locks::regular(&log)?;
    }
    for item in fs::read_dir(&dir)? {
        let path = item?.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".rollback-") && n.ends_with(".tmp"))
        {
            locks::regular(&path)?;
            fs::remove_file(path)?;
        }
    }
    let temp = dir.join(format!(".rollback-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut source = locks::open_private(backup, false, true)?;
        let mut target = create(&temp)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()?;
        drop(target);
        checkpoint("restore_after_temp");
        let actual = digest(&temp)?;
        if actual != expected_digest {
            return refuse("migration_backup_invalid");
        }
        let check = Connection::open_with_flags(&temp, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version = schema::validate_header(&check, false)
            .map_err(|_| CoreError::Refused("rollback_backup_incompatible"))?;
        if ![11, 12].contains(&version) {
            return refuse("rollback_backup_incompatible");
        }
        verify(&check, version)?;
        schema::validate_project(
            &check,
            key.root()
                .to_str()
                .ok_or(CoreError::Refused("invalid_project"))?,
        )?;
        drop(check);
        // Flush current WAL before removing sidecars. A crash before rename then
        // still leaves the complete old store. No live Core connection is allowed.
        if key.database().exists() {
            locks::regular(&key.database())?;
            let db =
                Connection::open_with_flags(key.database(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            schema::validate_header(&db, false)?;
            let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
            if busy != 0 {
                return refuse("busy");
            }
            drop(db);
        }
        key.validate_sidecars()?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let path = dir.join(format!("cortex.sqlite3{suffix}"));
            if path.symlink_metadata().is_ok() {
                locks::regular(&path)?;
                fs::remove_file(path)?;
            }
        }
        checkpoint("restore_before_replace");
        fs::rename(&temp, key.database())?;
        locks::sync_directory(&dir)?;
        checkpoint("restore_after_replace");
        let receipt = RestoreReceipt {
            restored_version: version,
            backup_digest: actual,
        };
        let log = dir.join("rollback.log");
        if log.symlink_metadata().is_ok() {
            locks::regular(&log)?;
        }
        let mut f = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&log)?;
        f.write_all(serde_json::to_string(&receipt)?.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        locks::sync_directory(&dir)?;
        Ok(receipt)
    })();
    if temp.exists() {
        fs::remove_file(temp)?;
        locks::sync_directory(&dir)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{OpenMode, Store, StoreChannel};
    use std::{os::unix::fs::DirBuilderExt, process::Command};
    fn key(root: &Path) -> ProjectKey {
        ProjectKey::new(root, StoreChannel::Dev).unwrap()
    }
    fn make(root: &Path) {
        drop(Store::open(key(root), OpenMode::Initialize).unwrap());
        let db = Connection::open(key(root).database()).unwrap();
        db.execute_batch("PRAGMA user_version=11;CREATE TABLE hook_events(id TEXT);INSERT INTO hook_events VALUES ('retired');").unwrap();
    }
    #[test]
    fn crash_child() {
        let Ok(root) = std::env::var("CORTEX_CORE_MIGRATION_CRASH_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        let k = key(root);
        if std::env::var("CORTEX_CORE_MIGRATION_CRASH_POINT")
            .unwrap()
            .starts_with("restore_")
        {
            let backup = k.directory().join(".migration-backups");
            let path = fs::read_dir(backup)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            restore(&k, &path, &digest(&path).unwrap(), true, true).unwrap();
        } else {
            drop(Store::open(k, OpenMode::Initialize).unwrap());
        }
        panic!("checkpoint did not run");
    }
    // Python test_migrate_crash_at_each_checkpoint_leaves_a_reconcilable_v10_store:
    // same crash/reopen invariant, restricted intentionally to supported format 11.
    #[test]
    fn migrate_crash_at_each_checkpoint_leaves_a_reconcilable_format11_store() {
        for point in [
            "before_backup",
            "after_backup",
            "before_commit",
            "after_commit",
        ] {
            let root =
                std::env::temp_dir().join(format!("cortex-migrate-crash-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            make(&root);
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::migration::tests::crash_child",
                    "--nocapture",
                ])
                .env("CORTEX_CORE_MIGRATION_CRASH_ROOT", &root)
                .env("CORTEX_CORE_MIGRATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(97));
            let s = Store::open(key(&root), OpenMode::Initialize).unwrap();
            s.read(|db| {
                assert_eq!(schema::validate_header(db, false)?, 12);
                verify(db, 12)
            })
            .unwrap();
            drop(s);
            fs::remove_dir_all(root).unwrap();
        }
    }
    // Python test_rollback_crash_at_each_checkpoint_leaves_a_reconcilable_store.
    #[test]
    fn rollback_crash_at_each_checkpoint_leaves_a_reconcilable_store() {
        for point in [
            "restore_after_temp",
            "restore_before_replace",
            "restore_after_replace",
        ] {
            let root =
                std::env::temp_dir().join(format!("cortex-restore-crash-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            make(&root);
            drop(Store::open(key(&root), OpenMode::Initialize).unwrap());
            let backup = fs::read_dir(key(&root).directory().join(".migration-backups"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let hash = digest(&backup).unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::migration::tests::crash_child",
                    "--nocapture",
                ])
                .env("CORTEX_CORE_MIGRATION_CRASH_ROOT", &root)
                .env("CORTEX_CORE_MIGRATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(97));
            restore(&key(&root), &backup, &hash, true, true).unwrap();
            assert_eq!(
                fs::read(key(&root).database()).unwrap(),
                fs::read(backup).unwrap()
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
}
