//! Per-channel store roots injected by the host, and project-side relocation anchors.
use super::{OpenMode, ProjectKey, StoreChannel, ignore_in_git, locks, schema};
use crate::{CoreError, CoreResult, error::refuse};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const ANCHOR_VERSION: i64 = 12001;
const DATABASE: &str = "cortex.sqlite3";
type Fingerprint = (u64, u64, u64, i64, i64, i64, i64);
type AnchorCache = std::collections::BTreeMap<PathBuf, (Fingerprint, Option<Binding>)>;
static ANCHORS: Mutex<AnchorCache> = Mutex::new(AnchorCache::new());

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct StoreRoot {
    path: PathBuf,
    state: PathBuf,
    channel: StoreChannel,
}

impl StoreRoot {
    pub fn new(state: &Path, channel: StoreChannel) -> CoreResult<Self> {
        if !state.is_absolute() || state.components().any(|part| part == Component::ParentDir) {
            return refuse("relative_storage_root");
        }
        let state = canonical_base(state)?;
        Ok(Self {
            path: state.join("cortex/projects"),
            state,
            channel,
        })
    }

    pub(super) fn channel(&self) -> StoreChannel {
        self.channel
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn project_key(&self, project: impl AsRef<Path>) -> CoreResult<ProjectKey> {
        if !project.as_ref().is_absolute() {
            return refuse("invalid_project");
        }
        Ok(
            ProjectKey::new(project.as_ref().canonicalize()?, self.channel)?
                .with_store_root(self.clone()),
        )
    }

    pub(crate) fn directory(&self, key: &ProjectKey) -> PathBuf {
        let mut name = String::new();
        if let Some(basename) = key.root().file_name().and_then(|name| name.to_str()) {
            name.extend(basename.chars().take(40).map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            }));
        }
        if name.is_empty() {
            name.push_str("project");
        }
        self.path
            .join(format!("{name}-{}", key.identity()))
            .join(self.channel.directory())
    }

    fn create(&self) -> CoreResult<()> {
        let mut ancestor = self.state.as_path();
        while !exists(ancestor)? {
            ancestor = ancestor
                .parent()
                .ok_or(CoreError::Refused("store_root_unsuitable"))?;
        }
        require_local(ancestor)?;
        let mut directory = ancestor.to_path_buf();
        for part in self
            .path
            .strip_prefix(ancestor)
            .map_err(|_| CoreError::Refused("unsafe_storage"))?
        {
            directory.push(part);
            locks::private_directory(&directory, true)?;
            require_local(&directory)?;
            locks::sync_directory(directory.parent().unwrap())?;
        }
        Ok(())
    }

    fn validate(&self, parent: &Path) -> CoreResult<()> {
        for directory in [self.path.parent().unwrap(), self.path.as_path(), parent] {
            locks::private_directory(directory, false)?;
            require_local(directory)?;
        }
        Ok(())
    }
}

fn canonical_base(path: &Path) -> CoreResult<PathBuf> {
    match path.canonicalize() {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == ErrorKind::NotFound && !exists(path)? => {
            let parent = path
                .parent()
                .ok_or(CoreError::Refused("store_root_unsuitable"))?;
            Ok(canonical_base(parent)?.join(
                path.file_name()
                    .ok_or(CoreError::Refused("store_root_unsuitable"))?,
            ))
        }
        Err(error) => Err(error.into()),
    }
}

fn require_local(path: &Path) -> CoreResult<()> {
    use std::{ffi::CString, mem::MaybeUninit};
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| CoreError::Refused("store_root_unsuitable"))?;
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let filesystem = unsafe { stat.assume_init() }.f_type as u64 & 0xffff_ffff;
    // NFS, CIFS, SMB2, SMB, FUSE, TMPFS and RAMFS.
    if matches!(
        filesystem,
        0x6969 | 0xff534d42 | 0xfe534d42 | 0x517b | 0x65735546 | 0x01021994 | 0x858458f6
    ) {
        return refuse("store_root_unsuitable");
    }
    Ok(())
}

pub(crate) fn exists(path: &Path) -> CoreResult<bool> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Binding {
    pub(crate) store_id: String,
    pub(crate) channel: String,
    pub(crate) directory: PathBuf,
    pub(crate) project_root: PathBuf,
    pub(crate) root_identity: (u64, u64),
    pub(crate) git_identity: Option<(u64, u64)>,
    pub(crate) created_at: String,
    pub(crate) source_digest: Option<String>,
}

fn identity(path: &Path) -> CoreResult<(u64, u64)> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() {
        return refuse("project_storage_mismatch");
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn git_identity(root: &Path) -> CoreResult<Option<(u64, u64)>> {
    let path = root.join(".git");
    if exists(&path)? {
        identity(&path).map(Some)
    } else {
        Ok(None)
    }
}

impl Binding {
    pub(crate) fn new(key: &ProjectKey, directory: &Path) -> CoreResult<Self> {
        Ok(Self {
            store_id: uuid::Uuid::new_v4().to_string(),
            channel: key.channel().directory().into(),
            directory: directory.to_path_buf(),
            project_root: key.root().to_path_buf(),
            root_identity: identity(key.root())?,
            git_identity: git_identity(key.root())?,
            created_at: crate::now(),
            source_digest: None,
        })
    }

    pub(crate) fn validate(&self, key: &ProjectKey, home: &Path) -> CoreResult<()> {
        self.validate_address(key, home)?;
        if self.root_identity != identity(key.root())?
            || self.git_identity.is_some() && self.git_identity != git_identity(key.root())?
        {
            return refuse("project_storage_mismatch");
        }
        Ok(())
    }

    fn validate_address(&self, key: &ProjectKey, home: &Path) -> CoreResult<()> {
        if self.project_root != key.root() || self.channel != key.channel().directory() {
            return refuse("project_storage_mismatch");
        }
        if self.directory != home {
            return refuse("store_root_mismatch");
        }
        Ok(())
    }

    fn can_reattach(&self) -> CoreResult<bool> {
        Ok(self.root_identity == identity(&self.project_root)?
            && self.git_identity == git_identity(&self.project_root)?)
    }
}

pub(crate) fn write_binding(db: &Connection, binding: &Binding) -> CoreResult<()> {
    db.execute_batch(
        "CREATE TABLE store_location (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            binding_json TEXT NOT NULL CHECK(json_valid(binding_json))
        );",
    )?;
    db.execute(
        "INSERT INTO store_location VALUES(1,?1)",
        [serde_json::to_string(binding)?],
    )?;
    Ok(())
}

pub(crate) fn read_binding(db: &Connection) -> CoreResult<Binding> {
    let json: String = db
        .query_row(
            "SELECT binding_json FROM store_location WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| CoreError::Refused("project_storage_mismatch"))?;
    serde_json::from_str(&json).map_err(|_| CoreError::Refused("project_storage_mismatch"))
}

fn readonly(path: &Path) -> CoreResult<Connection> {
    locks::regular(path)?;
    let encoded: String = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    Ok(Connection::open_with_flags(
        format!("file:{encoded}?mode=ro&immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?)
}

pub(crate) fn read_anchor(path: &Path) -> CoreResult<Option<Binding>> {
    locks::regular(path)?;
    let metadata = path.symlink_metadata()?;
    let identity = (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    );
    let cached = ANCHORS
        .lock()
        .ok()
        .and_then(|cache| cache.get(path).cloned())
        .filter(|(known, _)| identity == *known)
        .map(|(_, binding)| binding);
    let binding = match cached {
        Some(binding) => binding,
        None => {
            let binding = read_anchor_uncached(path)?;
            if let Ok(mut cache) = ANCHORS.lock() {
                if cache.len() >= 256 {
                    cache.clear();
                }
                cache.insert(path.to_path_buf(), (identity, binding.clone()));
            }
            binding
        }
    };
    if binding.is_some() {
        for suffix in ["-wal", "-shm", "-journal"] {
            if exists(&path.with_file_name(format!("{DATABASE}{suffix}")))? {
                return refuse("unsafe_storage");
            }
        }
    }
    Ok(binding)
}

fn read_anchor_uncached(path: &Path) -> CoreResult<Option<Binding>> {
    let db = readonly(path)?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != ANCHOR_VERSION {
        return Ok(None);
    }
    let app: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if app != schema::APPLICATION_ID {
        return refuse("unsafe_storage");
    }
    Ok(Some(read_binding(&db)?))
}

pub(crate) fn prepare_anchor(path: &Path, binding: &Binding) -> CoreResult<()> {
    if exists(path)? {
        return refuse("unsafe_storage");
    }
    locks::create_private_database(path)?;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
    let tx = db.unchecked_transaction()?;
    tx.pragma_update(None, "application_id", schema::APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", ANCHOR_VERSION)?;
    write_binding(&tx, binding)?;
    tx.commit()?;
    db.close().map_err(|(_, error)| error)?;
    Ok(())
}

fn install_anchor(directory: &Path, binding: &Binding, replace_empty: bool) -> CoreResult<()> {
    let temporary = directory.join(format!(".anchor-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        prepare_anchor(&temporary, binding)?;
        let destination = directory.join(DATABASE);
        if replace_empty {
            locks::regular(&destination)?;
            if destination.symlink_metadata()?.len() != 0 {
                return refuse("store_relocation_conflict");
            }
            fs::rename(&temporary, destination)?;
        } else {
            rename_new(&temporary, &destination)?;
        }
        locks::sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_pre_database_routing(source: &Path) -> CoreResult<Option<Vec<u8>>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    const LIMIT: u64 = (workspace_protocol::FRAME_BYTES - 65_536) as u64;
    if !exists(source)? {
        return Ok(None);
    }
    locks::regular(source)?;
    let input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(source)?;
    let identity = |metadata: fs::Metadata| {
        (metadata.dev(), metadata.ino(), metadata.len(), metadata.ctime(), metadata.ctime_nsec())
    };
    let before = identity(input.metadata()?);
    let mut bytes = Vec::new();
    (&input).take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return refuse("machine_reply_too_large");
    }
    locks::regular(source)?;
    if before != identity(input.metadata()?) || before != identity(source.symlink_metadata()?) {
        return refuse("unsafe_storage");
    }
    Ok(Some(bytes))
}

fn copy_pre_database_routing(anchor: &Path, staging: &Path) -> CoreResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let Some(bytes) = read_pre_database_routing(&anchor.join("routing.json"))? else {
        return Ok(());
    };
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(staging.join("routing.json"))?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    Ok(())
}

fn archive_pre_database_routing(anchor: &Path, home: &Path) -> CoreResult<()> {
    let source = anchor.join("routing.json");
    let Some(before) = read_pre_database_routing(&source)? else {
        return Ok(());
    };
    if read_pre_database_routing(&home.join("routing.json"))?.as_ref() != Some(&before) {
        return refuse("store_relocation_conflict");
    }
    let backups = anchor.join(".migration-backups");
    locks::private_directory(&backups, true)?;
    let retained = backups.join("relocated-pre-database-routing");
    locks::private_directory(&retained, true)?;
    rename_new(&source, &retained.join("routing.json"))?;
    locks::sync_directory(&retained)?;
    locks::sync_directory(&backups)?;
    locks::sync_directory(anchor)
}

fn create_home(home: &Path, anchor: &Path, binding: &Binding) -> CoreResult<()> {
    let directory = home.parent().unwrap();
    let staging = directory.join(format!(".create-{}", uuid::Uuid::new_v4()));
    locks::private_directory(&staging, true)?;
    let result = (|| {
        copy_pre_database_routing(anchor, &staging)?;
        let path = staging.join(DATABASE);
        locks::create_private_database(&path)?;
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let tx = db.unchecked_transaction()?;
        schema::install(&tx)?;
        write_binding(&tx, binding)?;
        tx.commit()?;
        db.close().map_err(|(_, error)| error)?;
        locks::AccessLock::acquire(&staging.join(".access.lock"), true, true, Duration::ZERO)?;
        locks::sync_directory(&staging)?;
        rename_new(&staging, home)?;
        locks::sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn rename_new(source: &Path, destination: &Path) -> CoreResult<()> {
    use std::ffi::CString;
    let c_path = |path: &Path| {
        CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| CoreError::Refused("unsafe_storage"))
    };
    let source = c_path(source)?;
    let destination = c_path(destination)?;
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn resolve(key: ProjectKey, mode: OpenMode, wait: Duration) -> CoreResult<ProjectKey> {
    let Resolution {
        key,
        _anchor,
        binding,
    } = resolve_open(key, mode, wait)?;
    if let Some(binding) = binding {
        let _home =
            locks::AccessLock::acquire(&key.directory().join(".access.lock"), false, false, wait)?;
        check_binding(&live_binding_connection(&key)?, &binding)?;
    }
    Ok(key)
}

pub(super) struct Resolution {
    pub(super) key: ProjectKey,
    pub(super) _anchor: Option<locks::AccessLock>,
    pub(super) binding: Option<Binding>,
}

impl Resolution {
    fn local(key: ProjectKey, anchor: Option<locks::AccessLock>) -> Self {
        Self {
            key,
            _anchor: anchor,
            binding: None,
        }
    }
    fn home(key: ProjectKey, home: PathBuf, binding: Binding) -> Self {
        Self {
            key: key.at_directory(home),
            _anchor: None,
            binding: Some(binding),
        }
    }
}

pub(super) fn check_binding(db: &Connection, expected: &Binding) -> CoreResult<()> {
    if &read_binding(db)? != expected {
        return refuse("project_storage_mismatch");
    }
    Ok(())
}

fn live_binding_connection(key: &ProjectKey) -> CoreResult<Connection> {
    key.validate_sidecars()?;
    Ok(Connection::open_with_flags(
        key.database(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

pub(super) fn resolve_open(
    key: ProjectKey,
    mode: OpenMode,
    wait: Duration,
) -> CoreResult<Resolution> {
    let (Some(root), Some(home)) = (key.store_root(), key.home_directory()) else {
        return Ok(Resolution::local(key, None));
    };
    if home.starts_with(key.anchor_directory())
        || (home.starts_with(key.root()) && exists(&key.root().join(".git"))?)
    {
        return refuse("store_root_unsuitable");
    }
    let initialize = mode == OpenMode::Initialize;
    let anchor = key.anchor_directory();
    let anchor_db = anchor.join(DATABASE);
    let home_db = home.join(DATABASE);

    if initialize && !exists(&anchor_db)? && !exists(&home_db)? && key.has_legacy_storage()? {
        return refuse("legacy_storage_root_detected");
    }

    if !exists(&anchor)? && !initialize {
        return if exists(&home_db)? {
            refuse("project_store_anchor_missing")
        } else {
            Ok(Resolution::local(key, None))
        };
    }
    locks::private_directory(&anchor, initialize)?;
    let guard = locks::AccessLock::acquire(
        &anchor.join(".access.lock"),
        initialize,
        initialize || mode == OpenMode::InspectImmutable,
        if initialize { Duration::ZERO } else { wait },
    )?;
    if exists(&anchor.join(super::migration::RETURN_PENDING))? {
        return refuse("store_return_pending");
    }
    if super::migration::returned_locally(&key, &home)? {
        return Ok(Resolution::local(key.at_directory(anchor), Some(guard)));
    }
    let anchor_exists = exists(&anchor_db)?;
    let home_exists = exists(&home_db)?;
    let empty_placeholder = initialize && anchor_exists && anchor_db.symlink_metadata()?.len() == 0;
    if empty_placeholder {
        locks::regular(&anchor_db)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            if exists(&anchor.join(format!("{DATABASE}{suffix}")))? {
                return refuse("unsafe_storage");
            }
        }
    }
    if initialize && exists(&home.join(super::migration::INTENT))? {
        root.validate(home.parent().unwrap())?;
        require_local(&home)?;
        drop(guard);
        super::migration::relocate12(&key, &home)?;
        return resolve_open(key, mode, wait);
    }
    if anchor_exists && !empty_placeholder {
        if let Some(binding) = read_anchor(&anchor_db)? {
            binding.validate(&key, &home)?;
            if !home_exists {
                return refuse("store_root_mismatch");
            }
            root.validate(home.parent().unwrap())?;
            locks::private_directory(&home, false)?;
            require_local(&home)?;
            return Ok(Resolution::home(key, home, binding));
        }
        if initialize {
            root.create()?;
            locks::private_directory(home.parent().unwrap(), true)?;
            root.validate(home.parent().unwrap())?;
            if exists(&home)? {
                require_local(&home)?;
            }
            drop(guard);
            super::migration::relocate12(&key, &home)?;
            return resolve_open(key, mode, wait);
        }
        return Ok(Resolution::local(key.at_directory(anchor), Some(guard)));
    }
    if home_exists {
        if !initialize {
            return refuse("project_store_anchor_missing");
        }
        root.validate(home.parent().unwrap())?;
        locks::private_directory(&home, false)?;
        require_local(&home)?;
        let _home =
            locks::AccessLock::acquire(&home.join(".access.lock"), false, true, Duration::ZERO)?;
        let home_key = key.clone().at_directory(home.clone());
        let binding = read_binding(&live_binding_connection(&home_key)?)?;
        binding.validate_address(&key, &home)?;
        if !binding.can_reattach()? {
            return refuse("project_store_anchor_missing");
        }
        if binding.source_digest.is_none() {
            archive_pre_database_routing(&anchor, &home)?;
        }
        ignore_in_git(&anchor)?;
        install_anchor(&anchor, &binding, empty_placeholder)?;
        return Ok(Resolution::home(key, home, binding));
    }
    if !initialize {
        return Ok(Resolution::local(key.at_directory(anchor), Some(guard)));
    }
    if key.has_legacy_storage()? {
        return refuse("legacy_storage_root_detected");
    }
    root.create()?;
    locks::private_directory(home.parent().unwrap(), true)?;
    root.validate(home.parent().unwrap())?;
    let binding = Binding::new(&key, &home)?;
    create_home(&home, &anchor, &binding)?;
    archive_pre_database_routing(&anchor, &home)?;
    ignore_in_git(&anchor)?;
    install_anchor(&anchor, &binding, empty_placeholder)?;
    drop(guard);
    Ok(Resolution::home(key, home, binding))
}
