//! Reusable blocking connections. Never opens a home/global store or another channel.
pub mod documents;
pub mod executor;
pub mod locks;
pub mod migration;
mod outbox;
pub mod publication;
mod retention;
pub mod schema;
pub use outbox::append_tx as append_public_tx;
mod project;
use crate::{error::refuse, CoreError, CoreResult};
use locks::AccessLock;
pub use project::{ProjectKey, StoreChannel};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::{sync::Mutex, time::Duration};

/// The store directory keeps itself out of the project's Git status with its own
/// `.gitignore` of `*`, which also ignores itself, in either channel: the project's
/// tracked `.gitignore` and `.git/info/exclude` are never changed. Written only when
/// absent and never through a link; a file already there is left exactly as it is.
fn ignore_in_git(directory: &std::path::Path) -> CoreResult<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let path = directory.join(".gitignore");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(b"*\n")?;
            file.sync_all()?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    Initialize,
    RuntimeValidate,
    InspectImmutable,
    /// Live WAL reader. Never initializes, migrates, installs hooks or writes domain data.
    LiveReadOnly,
}
pub struct Store {
    key: ProjectKey,
    mode: OpenMode,
    connection: Mutex<Connection>,
    inspection: Option<AccessLock>,
    // Fences offline replacement while a reusable connection is alive.
    _connection_guard: Option<AccessLock>,
    identity: (u64, u64),
}
impl Store {
    pub fn open(key: ProjectKey, mode: OpenMode) -> CoreResult<Self> {
        let initialize = mode == OpenMode::Initialize;
        locks::private_directory(&key.directory(), initialize)?;
        let inspection = mode == OpenMode::InspectImmutable;
        let read_only = inspection || mode == OpenMode::LiveReadOnly;
        let lock = AccessLock::acquire(
            &key.directory().join(".access.lock"),
            initialize,
            initialize || inspection,
            Duration::from_secs(if initialize { 0 } else { 2 }),
        )?;
        if initialize {
            locks::create_private_database(&key.database())?;
        }
        key.validate_sidecars()?;
        if inspection {
            let wal = key.directory().join("cortex.sqlite3-wal");
            if wal.try_exists()? && wal.metadata()?.len() > 0 {
                return refuse("storage_inspection_requires_checkpoint");
            }
        }
        let mut db = if inspection {
            // URI escaping prevents project names containing ?/#/% from changing flags.
            let bytes = key.database().as_os_str().as_encoded_bytes().to_vec();
            let uri = format!(
                "file:{}?mode=ro&immutable=1",
                bytes
                    .iter()
                    .map(|b| format!("%{b:02X}"))
                    .collect::<String>()
            );
            Connection::open_with_flags(
                uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?
        } else if read_only {
            Connection::open_with_flags(
                key.database(),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?
        } else {
            Connection::open_with_flags(
                key.database(),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?
        };
        db.busy_timeout(Duration::from_secs(15))?;
        let version = schema::validate_header(&db, initialize)?;
        schema::validate_project(
            &db,
            key.root()
                .to_str()
                .ok_or(CoreError::Refused("invalid_project"))?,
        )?;
        if version == 11 && !initialize {
            return refuse("legacy_storage_migration_required");
        }
        let connection_guard = if read_only {
            None
        } else {
            Some(AccessLock::acquire(
                &key.directory().join(".connections.lock"),
                true,
                version == 11,
                Duration::ZERO,
            )?)
        };
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        if version == 11 {
            migration::upgrade11(&mut db, &key.directory())?;
            connection_guard.as_ref().unwrap().downgrade()?;
        }
        if initialize {
            db.pragma_update(None, "journal_mode", "WAL")?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            schema::install(&tx)?;
            tx.commit()?;
        }
        schema::validate_groups(&db)?;
        if read_only {
            db.pragma_update(None, "query_only", "ON")?;
        } else {
            outbox::install(&db)?;
            if initialize {
                locks::open_private(&key.directory().join(".publication.lock"), true, false)?;
                ignore_in_git(&key.directory())?;
            }
        }
        use std::os::unix::fs::MetadataExt;
        let metadata = key.database().symlink_metadata()?;
        Ok(Self {
            identity: (metadata.dev(), metadata.ino()),
            key,
            mode,
            connection: Mutex::new(db),
            inspection: if inspection { Some(lock) } else { None },
            _connection_guard: connection_guard,
        })
    }
    pub fn key(&self) -> &ProjectKey {
        &self.key
    }
    fn access(&self) -> CoreResult<Option<AccessLock>> {
        let lock = if self.inspection.is_some() {
            None
        } else {
            Some(AccessLock::acquire(
                &self.key.directory().join(".access.lock"),
                false,
                false,
                Duration::from_secs(2),
            )?)
        };
        self.key.validate_sidecars()?;
        use std::os::unix::fs::MetadataExt;
        let metadata = self.key.database().symlink_metadata()?;
        if (metadata.dev(), metadata.ino()) != self.identity {
            return refuse("storage_replaced");
        }
        Ok(lock)
    }
    /// Query-only snapshot on the process-lifetime connection.
    pub fn read<T>(&self, body: impl FnOnce(&Connection) -> CoreResult<T>) -> CoreResult<T> {
        let _lock = self.access()?;
        let db = self
            .connection
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        db.pragma_update(None, "query_only", "ON")?;
        let result = (|| {
            let tx = db.unchecked_transaction()?;
            let value = body(&tx)?;
            tx.commit()?;
            Ok(value)
        })();
        // Restore the reusable connection even if BEGIN/body/COMMIT failed.
        let restore = if !matches!(
            self.mode,
            OpenMode::InspectImmutable | OpenMode::LiveReadOnly
        ) {
            db.pragma_update(None, "query_only", "OFF")
        } else {
            Ok(())
        };
        match result {
            Err(error) => Err(error),
            Ok(value) => {
                restore?;
                Ok(value)
            }
        }
    }
    /// FIRST table-touching statement is BEGIN IMMEDIATE. Drop rolls back on any error/panic.
    /// No external effects or waits belong inside body; it is never replayed here.
    pub fn write<T>(&self, body: impl FnOnce(&Transaction<'_>) -> CoreResult<T>) -> CoreResult<T> {
        if matches!(
            self.mode,
            OpenMode::InspectImmutable | OpenMode::LiveReadOnly
        ) {
            return refuse("inspection_read_only");
        }
        let _lock = self.access()?;
        let mut db = self
            .connection
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = body(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}
