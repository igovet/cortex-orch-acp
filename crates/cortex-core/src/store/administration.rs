//! Owner-local inspection and confirmed removal of one home store.
use super::{location, locks, migration, schema, StoreRoot};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const DATABASE: &str = "cortex.sqlite3";
const LAST_OPEN: &str = ".last-initialized";

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RootStatus {
    Present,
    Missing,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ProjectDatabase {
    pub store_id: String,
    pub project_root: PathBuf,
    pub root_status: RootStatus,
    pub bytes: u64,
    pub last_opened_for_write: String,
    pub originals_retained: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Removal {
    Database,
    Originals,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RemovalIntent {
    store_id: String,
    action: Removal,
    binding: Option<location::Binding>,
    detached: bool,
    target_identity: Option<(u64, u64)>,
}

fn create(path: &Path) -> CoreResult<fs::File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

fn write_atomic(path: &Path, contents: &[u8]) -> CoreResult<()> {
    let parent = path.parent().ok_or(CoreError::Refused("unsafe_storage"))?;
    let tmp = parent.join(format!(".admin-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut output = create(&tmp)?;
        output.write_all(contents)?;
        output.sync_all()?;
        drop(output);
        fs::rename(&tmp, path)?;
        locks::sync_directory(parent)
    })();
    if location::exists(&tmp)? {
        fs::remove_file(tmp)?;
    }
    result
}

pub(crate) fn record_open(directory: &Path) -> CoreResult<()> {
    write_atomic(&directory.join(LAST_OPEN), crate::now().as_bytes())
}

fn root_status(binding: &location::Binding) -> RootStatus {
    match binding.project_root.symlink_metadata() {
        Ok(metadata)
            if metadata.is_dir() && (metadata.dev(), metadata.ino()) == binding.root_identity =>
        {
            RootStatus::Present
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // A missing child on its original filesystem, versus a path whose
            // nearest visible ancestor is on a different device.
            for parent in binding.project_root.ancestors().skip(1) {
                match parent.metadata() {
                    Ok(metadata) if metadata.dev() == binding.root_identity.0 => {
                        return RootStatus::Missing;
                    }
                    Ok(_) => return RootStatus::Unavailable,
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(_) => return RootStatus::Unavailable,
                }
            }
            RootStatus::Unavailable
        }
        _ => RootStatus::Unavailable,
    }
}

fn size(dir: &Path) -> CoreResult<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.path().symlink_metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn db_binding(dir: &Path) -> CoreResult<location::Binding> {
    let database = dir.join(DATABASE);
    locks::regular(&database)?;
    for name in [
        "cortex.sqlite3-wal",
        "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
    ] {
        let sidecar = dir.join(name);
        if location::exists(&sidecar)? {
            locks::regular(&sidecar)?;
        }
    }
    let db = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if schema::validate_header(&db, false)? != schema::FORMAT {
        return refuse("unsupported_storage");
    }
    let binding = location::read_binding(&db)?;
    if binding.directory != dir
        || Some(binding.channel.as_str()) != dir.file_name().and_then(|name| name.to_str())
        || !binding.project_root.is_absolute()
        || uuid::Uuid::parse_str(&binding.store_id).is_err()
    {
        return refuse("store_root_mismatch");
    }
    Ok(binding)
}

fn originals(binding: &location::Binding) -> PathBuf {
    binding
        .project_root
        .join(&binding.channel)
        .join(".migration-backups")
        .join(format!("relocated-{}", binding.store_id))
}

fn catalogue(root: &StoreRoot) -> CoreResult<Vec<PathBuf>> {
    if !location::exists(root.path())? {
        return Ok(Vec::new());
    }
    locks::private_directory(root.path(), false)?;
    let mut parents = Vec::new();
    for item in fs::read_dir(root.path())? {
        let item = item?;
        if item.file_name().as_encoded_bytes().first() != Some(&b'.') && item.file_type()?.is_dir()
        {
            locks::private_directory(&item.path(), false)?;
            parents.push(item.path());
        }
    }
    parents.sort_unstable();
    Ok(parents)
}

fn access(dir: &Path, exclusive: bool) -> CoreResult<locks::AccessLock> {
    locks::AccessLock::acquire(&dir.join(".access.lock"), false, exclusive, Duration::ZERO)
}

fn connections(dir: &Path) -> CoreResult<locks::AccessLock> {
    locks::AccessLock::acquire(&dir.join(".connections.lock"), true, true, Duration::ZERO)
}

fn is_authoritative(dir: &Path, binding: &location::Binding) -> CoreResult<bool> {
    if location::exists(&dir.join(migration::INTENT))? {
        return Ok(false);
    }
    if binding.source_digest.is_some() {
        return location::exists(&dir.join(migration::COMPLETE));
    }
    Ok(true)
}

pub fn list(root: &StoreRoot) -> CoreResult<Vec<ProjectDatabase>> {
    let mut stores = Vec::new();
    for parent in catalogue(root)? {
        let dir = parent.join(root.channel().directory());
        if !location::exists(&dir)? {
            continue;
        }
        locks::private_directory(&dir, false)?;
        let _access = access(&dir, false)?;
        let binding = db_binding(&dir)?;
        // A staged dual-location store is not the authoritative history yet.
        if !is_authoritative(&dir, &binding)? {
            continue;
        }
        let status = root_status(&binding);
        let original = originals(&binding);
        let last = dir.join(LAST_OPEN);
        let last_opened_for_write = if location::exists(&last)? {
            let file = locks::open_private(&last, false, true)?;
            if file.metadata()?.len() > 64 {
                return refuse("unsafe_storage");
            }
            use std::io::Read;
            let mut value = String::new();
            file.take(64).read_to_string(&mut value)?;
            value
        } else {
            binding.created_at
        };
        stores.push(ProjectDatabase {
            store_id: binding.store_id,
            project_root: binding.project_root,
            root_status: status,
            bytes: size(&dir)?,
            last_opened_for_write,
            originals_retained: original.join(DATABASE).is_file(),
        });
    }
    Ok(stores)
}

fn erase(journal: &Path, target: &Path, tomb: &Path, intent: &RemovalIntent) -> CoreResult<()> {
    let parent = target
        .parent()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    if !location::exists(journal)? {
        if location::exists(tomb)? || !location::exists(target)? {
            return refuse("project_store_not_found");
        }
        write_atomic(journal, &serde_json::to_vec(intent)?)?;
    } else {
        let file = locks::open_private(journal, false, true)?;
        if file.metadata()?.len() > 256
            || serde_json::from_reader::<_, RemovalIntent>(file)? != *intent
        {
            return refuse("unsafe_storage");
        }
    }
    if location::exists(target)? {
        locks::private_directory(target, false)?;
        if location::exists(tomb)? || intent.target_identity != Some(identity(target)?) {
            return refuse("unsafe_storage");
        }
        fs::rename(target, tomb)?;
        locks::sync_directory(parent)?;
    }
    if location::exists(tomb)? {
        erase_tree(tomb, intent.target_identity)?;
        locks::sync_directory(parent)?;
    }
    fs::remove_file(journal)?;
    locks::sync_directory(
        journal
            .parent()
            .ok_or(CoreError::Refused("unsafe_storage"))?,
    )
}

fn identity(path: &Path) -> CoreResult<(u64, u64)> {
    let metadata = path.symlink_metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn erase_tree(path: &Path, expected: Option<(u64, u64)>) -> CoreResult<()> {
    fn on_same_device(path: &Path, device: u64) -> CoreResult<()> {
        for item in fs::read_dir(path)? {
            let item = item?;
            let child = item.path();
            let metadata = child.symlink_metadata()?;
            if metadata.dev() != device {
                return refuse("unsafe_storage");
            }
            if metadata.is_dir() {
                on_same_device(&child, device)?;
            }
        }
        Ok(())
    }
    locks::private_directory(path, false)?;
    let actual = identity(path)?;
    if expected != Some(actual) {
        return refuse("unsafe_storage");
    }
    on_same_device(path, actual.0)?;
    fs::remove_dir_all(path)?;
    Ok(())
}

fn read_delete_intent(journal: &Path, dir: &Path, id: &str) -> CoreResult<RemovalIntent> {
    let file = locks::open_private(journal, false, true)?;
    if file.metadata()?.len() > 16384 {
        return refuse("unsafe_storage");
    }
    let intent: RemovalIntent = serde_json::from_reader(file)?;
    if intent.action != Removal::Database
        || intent.store_id != id
        || !intent.binding.as_ref().is_some_and(|binding| {
            binding.store_id == id
                && binding.directory == dir
                && binding.project_root.is_absolute()
                && Some(binding.channel.as_str()) == dir.file_name().and_then(|name| name.to_str())
        })
    {
        return refuse("unsafe_storage");
    }
    Ok(intent)
}

fn delete_database(
    dir: &Path,
    journal: &Path,
    tomb: &Path,
    mut intent: RemovalIntent,
) -> CoreResult<()> {
    let binding = intent
        .binding
        .clone()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    let anchor = binding.project_root.join(&binding.channel);
    let root_present = root_status(&binding) == RootStatus::Present;
    let anchor_present = root_present && location::exists(&anchor)?;
    let _anchor_fences = if anchor_present {
        locks::private_directory(&anchor, false)?;
        Some((access(&anchor, true)?, connections(&anchor)?))
    } else {
        None
    };
    if anchor_present && location::exists(&anchor.join(migration::RETURN_PENDING))? {
        return refuse("store_return_pending");
    }
    if !intent.detached && !location::exists(tomb)? {
        locks::private_directory(dir, false)?;
        let _access = access(dir, true)?;
        let _connections = connections(dir)?;
        if db_binding(dir)? != binding
            || !is_authoritative(dir, &binding)?
            || intent.target_identity != Some(identity(dir)?)
        {
            return refuse("store_relocation_conflict");
        }
        let db =
            Connection::open_with_flags(dir.join(DATABASE), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.busy_timeout(Duration::ZERO)?;
        let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            return refuse("busy");
        }
        drop(db);
        write_atomic(journal, &serde_json::to_vec(&intent)?)?;
        fs::rename(dir, tomb)?;
        locks::sync_directory(dir.parent().ok_or(CoreError::Refused("unsafe_storage"))?)?;
    }
    if !intent.detached {
        // Persist detach before removing any tomb contents. A retry can then
        // finish removal even if a newly initialized store has reused the name.
        intent.detached = true;
        write_atomic(journal, &serde_json::to_vec(&intent)?)?;
    }
    if location::exists(tomb)? {
        erase_tree(tomb, intent.target_identity)?;
        locks::sync_directory(tomb.parent().ok_or(CoreError::Refused("unsafe_storage"))?)?;
    }
    if !root_present {
        // The exact intent remains usable when this project is next mounted.
        return Ok(());
    }
    let marker = anchor.join(DATABASE);
    if anchor_present
        && location::exists(&marker)?
        && location::read_anchor(&marker)?.as_ref() == Some(&binding)
    {
        fs::remove_file(&marker)?;
        locks::sync_directory(&anchor)?;
    }
    fs::remove_file(journal)?;
    locks::sync_directory(
        journal
            .parent()
            .ok_or(CoreError::Refused("unsafe_storage"))?,
    )
}

pub fn remove(root: &StoreRoot, id: &str, action: Removal, confirm: bool) -> CoreResult<()> {
    if !confirm {
        return refuse("project_store_removal_confirmation_required");
    }
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| CoreError::Refused("project_store_not_found"))?
        .to_string();
    let intent = RemovalIntent {
        store_id: id.clone(),
        action,
        binding: None,
        detached: false,
        target_identity: None,
    };
    for parent in catalogue(root)? {
        let dir = parent.join(root.channel().directory());
        let journal = parent.join(format!(".delete-{id}.json"));
        let tomb = parent.join(format!(".deleted-{id}"));
        if action == Removal::Database && location::exists(&journal)? {
            let _operation = operation(&parent, &id)?;
            let pending = read_delete_intent(&journal, &dir, &id)?;
            return delete_database(&dir, &journal, &tomb, pending);
        }
        if !location::exists(&dir)? {
            continue;
        }
        locks::private_directory(&dir, false)?;
        // Match the requested ID without taking exclusive access on unrelated projects.
        let binding = {
            let _access = access(&dir, false)?;
            db_binding(&dir)?
        };
        if binding.store_id != id {
            continue;
        }
        let _operation = operation(&parent, &id)?;
        if action == Removal::Originals {
            return remove_originals(&dir, &binding, &intent);
        }
        if location::exists(&tomb)? || location::exists(&journal)? {
            return refuse("store_relocation_conflict");
        }
        return delete_database(
            &dir,
            &journal,
            &tomb,
            RemovalIntent {
                binding: Some(binding),
                target_identity: Some(identity(&dir)?),
                ..intent
            },
        );
    }
    refuse("project_store_not_found")
}

fn operation(parent: &Path, id: &str) -> CoreResult<locks::AccessLock> {
    locks::AccessLock::acquire(
        &parent.join(format!(".administration-{id}.lock")),
        true,
        true,
        Duration::ZERO,
    )
}

fn remove_originals(
    dir: &Path,
    binding: &location::Binding,
    intent: &RemovalIntent,
) -> CoreResult<()> {
    if binding.source_digest.is_none() || root_status(binding) != RootStatus::Present {
        return refuse("relocated_originals_unavailable");
    }
    let anchor = binding.project_root.join(&binding.channel);
    locks::private_directory(&anchor, false)?;
    let _anchor_access = access(&anchor, true)?;
    let _anchor_connections = connections(&anchor)?;
    if location::exists(&anchor.join(migration::RETURN_PENDING))? {
        return refuse("store_return_pending");
    }
    let _access = access(dir, true)?;
    let _connections = connections(dir)?;
    if db_binding(dir)? != *binding
        || location::read_anchor(&anchor.join(DATABASE))?.as_ref() != Some(binding)
        || !is_authoritative(dir, binding)?
    {
        return refuse("store_relocation_conflict");
    }
    let target = originals(binding);
    let backups = target
        .parent()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    locks::private_directory(backups, false)?;
    let journal = backups.join(format!(".cleanup-{}.json", binding.store_id));
    let tomb = backups.join(format!(".cleaned-{}", binding.store_id));
    let identity = if location::exists(&target)? {
        migration::verify_retained_original(dir, &target, binding)?
    } else {
        if !location::exists(&journal)? {
            return refuse("relocated_originals_unavailable");
        }
        migration::retained_identity(dir, binding)?
    };
    if !location::exists(&journal)? {
        if !location::exists(&target)? {
            return refuse("relocated_originals_unavailable");
        }
        locks::private_directory(&target, false)?;
        locks::regular(&target.join(DATABASE))?;
    }
    erase(
        &journal,
        &target,
        &tomb,
        &RemovalIntent {
            store_id: intent.store_id.clone(),
            action: intent.action,
            binding: None,
            detached: false,
            target_identity: Some(identity),
        },
    )
}
