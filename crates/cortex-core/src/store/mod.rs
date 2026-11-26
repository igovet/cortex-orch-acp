//! Reusable blocking connections. Never opens a home/global store or another channel.
mod diagnostic;
pub use diagnostic::StorageDiagnostic;
pub mod documents;
pub mod executor;
pub mod locks;
pub mod migration;
mod compaction;
mod outbox;
pub mod owner_file_leases;
pub mod publication;
mod retention;
pub use compaction::{CompactionBudget, CompactionReport};
pub use retention::expired_closed_archive;
pub(crate) use retention::quiet as require_quiet_workspace_task;
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
/// The longest a waiting read ([`Store::open_waiting`]) queues behind an exclusive access lock.
pub const READ_ACCESS_WAIT: Duration = Duration::from_secs(600);
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
    access_wait: Duration,
    /// True while the connection is at `synchronous=NORMAL` (see [`Store::write_relaxed`]).
    /// Changed only while holding `connection`.
    relaxed: std::sync::atomic::AtomicBool,
    /// A reader-pool connection: every write is refused (`storage_read_lane`).
    read_lane: bool,
}
impl Store {
    pub fn open(key: ProjectKey, mode: OpenMode) -> CoreResult<Self> {
        Self::open_with_wait(key, mode, Duration::from_secs(2), Duration::from_secs(15))
    }

    /// Open for a read the owner is waiting on. When the exclusive access lock is held
    /// (this service migrating or initializing the store, or another bounded exclusive
    /// operation), the open and every later `read` wait for it with a growing back-off
    /// instead of refusing `busy` after two seconds. Only a holder that outlasts
    /// [`READ_ACCESS_WAIT`] still yields `busy`, which clients show as a waiting state.
    pub fn open_waiting(key: ProjectKey, mode: OpenMode) -> CoreResult<Self> {
        Self::open_with_wait(key, mode, READ_ACCESS_WAIT, Duration::from_secs(15))
    }

    /// A scheduled deletion must not queue behind a busy store.
    pub fn open_for_workspace_retention(key: ProjectKey) -> CoreResult<Self> {
        Self::open_with_wait(
            key,
            OpenMode::RuntimeValidate,
            Duration::ZERO,
            Duration::ZERO,
        )
    }

    /// Enumerate reference evidence without contending with active writers.
    pub fn read_for_workspace_retention(key: ProjectKey) -> CoreResult<Self> {
        Self::open_with_wait(key, OpenMode::LiveReadOnly, Duration::ZERO, Duration::ZERO)
    }

    fn open_with_wait(
        key: ProjectKey,
        mode: OpenMode,
        access_wait: Duration,
        sqlite_wait: Duration,
    ) -> CoreResult<Self> {
        Self::open_checked(key, mode, access_wait, sqlite_wait, None)
            .map(|store| store.expect("ordinary store open does not probe ACP sessions"))
    }

    /// Display-only ACP history open. Returns no handle only if a format-11
    /// store contains the exact Rust session, so the caller may enter normal
    /// fenced initialization. Legacy and unknown sessions cannot opt in.
    pub fn open_acp_history(key: ProjectKey, session: &str) -> CoreResult<Option<Self>> {
        Self::open_checked(
            key,
            OpenMode::LiveReadOnly,
            Duration::from_secs(2),
            Duration::from_secs(15),
            Some(session),
        )
    }

    fn open_checked(
        key: ProjectKey,
        mode: OpenMode,
        access_wait: Duration,
        sqlite_wait: Duration,
        acp_session: Option<&str>,
    ) -> CoreResult<Option<Self>> {
        let initialize = mode == OpenMode::Initialize;
        let directory = key.directory();
        let release = key.channel() == StoreChannel::Release;
        if release {
            // Report a legacy-only project before creating any canonical files.
            // Stat the pathname; opening a plain descriptor could drop SQLite's locks.
            let unstarted = match locks::private_directory(&directory, false) {
                Ok(()) => match key.database().symlink_metadata() {
                    Ok(m) if m.is_file() && m.len() == 0 => {
                        key.validate_sidecars()?;
                        true
                    }
                    Ok(_) => false,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                    Err(e) => return Err(e.into()),
                },
                Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => return Err(e),
            };
            if unstarted && key.has_legacy_storage()? {
                return refuse("legacy_storage_root_detected");
            }
        }
        locks::private_directory(&directory, initialize)?;
        let inspection = mode == OpenMode::InspectImmutable;
        let read_only = inspection || mode == OpenMode::LiveReadOnly;
        let lock = AccessLock::acquire(
            &key.directory().join(".access.lock"),
            initialize,
            initialize || inspection,
            if initialize {
                Duration::ZERO
            } else {
                access_wait
            },
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
        db.busy_timeout(sqlite_wait)?;
        // Recognize a schema-free SQLite container for the release diagnostic
        // even on a read-only open; that mode still cannot initialize it.
        let version = schema::validate_header(&db, initialize || release)?;
        if version == 0 {
            if release && key.has_legacy_storage()? {
                return refuse("legacy_storage_root_detected");
            }
            if !initialize {
                return refuse("unsupported_storage");
            }
        }
        let has_tasks = schema::validate_project_has_tasks(
            &db,
            key.root()
                .to_str()
                .ok_or(CoreError::Refused("invalid_project"))?,
        )?;
        if version == schema::FORMAT && release && !has_tasks {
            // A previously opened but still empty store must not hide the old
            // project. A populated canonical store takes precedence over the
            // old root, even if that root is unreadable.
            let legacy = key.has_legacy_storage();
            if !matches!(legacy, Ok(false)) {
                let snapshot = db.unchecked_transaction()?;
                let empty = schema::is_empty_store(&snapshot)?;
                snapshot.commit()?;
                if empty && legacy? {
                    return refuse("legacy_storage_root_detected");
                }
            }
        }
        if version == 11 && !initialize {
            if let Some(session) = acp_session {
                let has_sessions: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='acp_host_sessions')",
                    [],
                    |r| r.get(0),
                )?;
                if has_sessions && db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM acp_host_sessions WHERE session_id=? AND project_root=?
                     AND json_extract(client_info,'$.name')='cortex-rust-acp')",
                    (session, key.root().to_str()),
                    |r| r.get::<_, bool>(0),
                )? {
                    return Ok(None);
                }
            }
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
            // A format-11 upgrade already installed and validated the schema.
            if version != 11 {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                schema::install(&tx)?;
                tx.commit()?;
            }
            // Planner statistics for the read indexes: SQLite's own open-time `optimize`, which analyzes
            // a table only if it was never analyzed or has grown many times over since, each pass
            // sampling a bounded number of rows (`analysis_limit`), so its cost does not grow with the store.
            // It runs under this open's exclusive `.access.lock`, in SQLite's own transaction.
            db.execute_batch("PRAGMA analysis_limit=1000; PRAGMA optimize=0x10002;")?;
        }
        schema::validate_groups(&db)?;
        if read_only {
            db.pragma_update(None, "query_only", "ON")?;
        } else {
            outbox::install(&mut db)?;
            if initialize {
                locks::open_private(&key.directory().join(".publication.lock"), true, false)?;
                ignore_in_git(&key.directory())?;
            }
        }
        use std::os::unix::fs::MetadataExt;
        let metadata = key.database().symlink_metadata()?;
        Ok(Some(Self {
            identity: (metadata.dev(), metadata.ino()),
            access_wait,
            key,
            mode,
            connection: Mutex::new(db),
            inspection: if inspection { Some(lock) } else { None },
            _connection_guard: connection_guard,
            relaxed: std::sync::atomic::AtomicBool::new(false),
            read_lane: false,
        }))
    }
    /// Make this a reader-pool store: reads only; any write refuses `storage_read_lane`.
    pub(crate) fn read_lane(mut self) -> Self {
        self.read_lane = true;
        self
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
                self.access_wait,
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
    /// Never wait for a process, engine or user inside body. A bounded filesystem
    /// publish that participates in a final write fence may run here; rollback
    /// cannot undo it. The body is never replayed here.
    /// Every such commit uses `synchronous=FULL`: it returns only once it is durable.
    pub fn write<T>(&self, body: impl FnOnce(&Transaction<'_>) -> CoreResult<T>) -> CoreResult<T> {
        self.write_with(true, body)
    }
    /// [`Self::write`] committed with `synchronous=NORMAL`: in WAL mode the commit does
    /// not wait for an fsync of the log. A power failure (never a process crash) may lose
    /// the newest such commits; the database stays consistent, and the next FULL commit
    /// makes every earlier one durable with its own. Only for display/progress rows that
    /// no authority, lifecycle, result, receipt or owner action depends on (see
    /// `docs/project/storage.md#commit-durability`).
    pub fn write_relaxed<T>(
        &self,
        body: impl FnOnce(&Transaction<'_>) -> CoreResult<T>,
    ) -> CoreResult<T> {
        self.write_with(false, body)
    }
    fn write_with<T>(
        &self,
        durable: bool,
        body: impl FnOnce(&Transaction<'_>) -> CoreResult<T>,
    ) -> CoreResult<T> {
        use std::sync::atomic::Ordering;
        if matches!(
            self.mode,
            OpenMode::InspectImmutable | OpenMode::LiveReadOnly
        ) {
            return refuse("inspection_read_only");
        }
        if self.read_lane {
            return refuse("storage_read_lane");
        }
        let _lock = self.access()?;
        let mut db = self
            .connection
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        // The level is a connection setting, changed outside any transaction. A FULL
        // write never starts while the connection is still relaxed.
        if self.relaxed.load(Ordering::SeqCst) == durable {
            db.pragma_update(None, "synchronous", if durable { "FULL" } else { "NORMAL" })?;
            self.relaxed.store(!durable, Ordering::SeqCst);
        }
        let result = (|| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = body(&tx)?;
            tx.commit()?;
            Ok(result)
        })();
        if !durable && db.pragma_update(None, "synchronous", "FULL").is_ok() {
            self.relaxed.store(false, Ordering::SeqCst);
        }
        result
    }
}
