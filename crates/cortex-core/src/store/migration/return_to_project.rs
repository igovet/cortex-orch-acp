//! Explicit offline return of the latest verified home snapshot to its project.
use super::{
    checkpoint, create, digest,
    relocation::{
        INTENT as FORWARD_PENDING, check_space, children, copy_tree, entries, fence, fingerprint,
        present, rename_new, retained_identity, rewrite_paths, sql, top_service_file, verify_files,
    },
    verify,
};
use crate::{
    CoreError, CoreResult,
    error::refuse,
    ownership::{ProjectOwner, SchedulerOwner},
    store::{ProjectKey, location, locks, schema},
};
use rusqlite::backup::Backup;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const DATABASE: &str = "cortex.sqlite3";
const BACKUPS: &str = ".migration-backups";
pub(crate) const PENDING: &str = ".return-intent.json";
const RETURNED: &str = ".returned-store.json";
const BUILDING: &str = ".return-building.json";
const JOURNAL_LIMIT: u64 = 16 * 1024 * 1024;
type Identity = (u64, u64);

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Returned {
    binding: location::Binding,
    database: Identity,
    backup: Identity,
}

#[derive(Debug, Deserialize, Serialize)]
struct Move {
    path: PathBuf,
    identity: Identity,
}

#[derive(Debug, Deserialize, Serialize)]
struct Intent {
    returned: Returned,
    anchor: Identity,
    staging_id: String,
    database_digest: String,
    installed_digest: String,
    source_digest: String,
    moves: Vec<Move>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Building {
    binding: location::Binding,
    staging_id: String,
    identity: Identity,
}

fn identity(path: &Path) -> CoreResult<Identity> {
    let m = path.symlink_metadata()?;
    if !(m.is_file() || m.is_dir()) || m.uid() != unsafe { libc::getuid() } {
        return refuse("unsafe_storage");
    }
    Ok((m.dev(), m.ino()))
}

fn item_matches(path: &Path, expected: Identity) -> CoreResult<bool> {
    match path.symlink_metadata() {
        Ok(_) => Ok(identity(path)? == expected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> CoreResult<Option<T>> {
    if !present(path)? {
        return Ok(None);
    }
    let file = locks::open_private(path, false, true)?;
    if file.metadata()?.len() > JOURNAL_LIMIT {
        return refuse("unsafe_storage");
    }
    Ok(Some(serde_json::from_reader(file)?))
}

fn write_record(path: &Path, record: &impl Serialize) -> CoreResult<()> {
    let parent = path.parent().ok_or(CoreError::Refused("unsafe_storage"))?;
    let temporary = parent.join(format!(".return-record-{}.tmp", uuid::Uuid::new_v4()));
    let data = serde_json::to_vec(record)?;
    if data.len() as u64 > JOURNAL_LIMIT {
        return refuse("unsafe_storage");
    }
    let result = (|| {
        let mut file = create(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        rename_new(&temporary, path)?;
        locks::sync_directory(parent)
    })();
    if result.is_err() && present(&temporary)? {
        fs::remove_file(temporary)?;
    }
    result
}

fn archive_path(key: &ProjectKey, store_id: &str, create: bool) -> CoreResult<PathBuf> {
    let root = key
        .store_root()
        .ok_or(CoreError::Refused("store_root_mismatch"))?;
    locks::private_directory(root.path(), false)?;
    let backups = root.path().join(BACKUPS);
    if create || present(&backups)? {
        locks::private_directory(&backups, create)?;
    }
    if create {
        locks::sync_directory(root.path())?;
    }
    Ok(backups.join(format!("returned-{store_id}")))
}

pub(crate) fn returned_locally(key: &ProjectKey, home: &Path) -> CoreResult<bool> {
    let anchor = key.anchor_directory();
    let Some(record): Option<Returned> = read_record(&anchor.join(RETURNED))? else {
        return Ok(false);
    };
    record.binding.validate(key, home)?;
    uuid::Uuid::parse_str(&record.binding.store_id)
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    if present(home)? || !item_matches(&anchor.join(DATABASE), record.database)? {
        return refuse("store_return_conflict");
    }
    locks::regular(&anchor.join(DATABASE))?;
    Ok(true)
}

fn collect_moves(stage: &Path, anchor: &Path) -> CoreResult<Vec<Move>> {
    let mut moves = Vec::new();
    for item in children(stage)? {
        let name = item
            .file_name()
            .ok_or(CoreError::Refused("unsafe_storage"))?;
        if name == DATABASE {
            continue;
        }
        if name == BACKUPS {
            locks::private_directory(&item, false)?;
            locks::private_directory(&anchor.join(BACKUPS), true)?;
            for child in children(&item)? {
                let leaf = child
                    .file_name()
                    .ok_or(CoreError::Refused("unsafe_storage"))?;
                let path = Path::new(BACKUPS).join(leaf);
                let existing = anchor.join(&path);
                if present(&existing)? {
                    locks::regular(&existing)?;
                    locks::regular(&child)?;
                    if digest(&existing)? != digest(&child)? {
                        return refuse("store_return_conflict");
                    }
                    // Keep the identical historical project copy. Only the
                    // privately staged duplicate is removed.
                    fs::remove_file(child)?;
                    continue;
                }
                moves.push(Move {
                    path,
                    identity: identity(&child)?,
                });
            }
        } else {
            moves.push(Move {
                path: PathBuf::from(name),
                identity: identity(&item)?,
            });
        }
    }
    for item in &moves {
        if present(&anchor.join(&item.path))? {
            return refuse("store_return_conflict");
        }
    }
    Ok(moves)
}

fn validate_move(path: &Path) -> CoreResult<()> {
    let parts: Vec<_> = path.components().collect();
    match parts.as_slice() {
        [Component::Normal(name)]
            if name
                .to_str()
                .is_some_and(|s| !top_service_file(s) && !s.starts_with(".return")) =>
        {
            Ok(())
        }
        [Component::Normal(parent), Component::Normal(_)] if *parent == BACKUPS => Ok(()),
        _ => refuse("unsafe_storage"),
    }
}

fn staging_path(anchor: &Path, id: &str) -> CoreResult<PathBuf> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| CoreError::Refused("unsafe_storage"))?;
    Ok(anchor.join(format!(".return-stage-{id}")))
}

fn clear_incomplete_stage(anchor: &Path, binding: &location::Binding) -> CoreResult<()> {
    let record = anchor.join(BUILDING);
    let Some(build): Option<Building> = read_record(&record)? else {
        return Ok(());
    };
    if &build.binding != binding {
        return refuse("store_return_conflict");
    }
    let stage = staging_path(anchor, &build.staging_id)?;
    if present(&stage)? {
        locks::private_directory(&stage, false)?;
        if !item_matches(&stage, build.identity)? {
            return refuse("store_return_conflict");
        }
        // Staging can be interrupted after any copied file, but the live
        // source never points into this tree. Refuse to cross a mounted device.
        fn reject_mounted_children(path: &Path, device: u64) -> CoreResult<()> {
            for child in children(path)? {
                let metadata = child.symlink_metadata()?;
                if metadata.dev() != device {
                    return refuse("unsafe_storage");
                }
                if metadata.is_dir() {
                    reject_mounted_children(&child, device)?;
                }
            }
            Ok(())
        }
        reject_mounted_children(&stage, build.identity.0)?;
        fs::remove_dir_all(stage)?;
    }
    fs::remove_file(record)?;
    locks::sync_directory(anchor)
}

fn perform_moves(intent: &Intent, stage: &Path, anchor: &Path) -> CoreResult<()> {
    for item in &intent.moves {
        validate_move(&item.path)?;
        let source = stage.join(&item.path);
        let target = anchor.join(&item.path);
        if item.path.components().count() == 2 {
            locks::private_directory(source.parent().unwrap(), false)?;
            locks::private_directory(target.parent().unwrap(), false)?;
        }
        if item_matches(&source, item.identity)? && !present(&target)? {
            rename_new(&source, &target)?;
            locks::sync_directory(source.parent().unwrap())?;
            locks::sync_directory(target.parent().unwrap())?;
        } else if present(&source)? || !item_matches(&target, item.identity)? {
            return refuse("store_return_conflict");
        }
    }
    Ok(())
}

fn prepare(
    key: &ProjectKey,
    source: &Path,
    anchor: &Path,
    binding: location::Binding,
) -> CoreResult<Intent> {
    let db = sql(&source.join(DATABASE), true)?;
    verify(&db, schema::FORMAT)?;
    schema::validate_project(
        &db,
        key.root()
            .to_str()
            .ok_or(CoreError::Refused("invalid_project"))?,
    )?;
    let mut files = Vec::new();
    entries(source, Path::new(""), &mut files)?;
    check_space(source, anchor, &files)?;
    let staging_id = uuid::Uuid::new_v4().to_string();
    let stage = staging_path(anchor, &staging_id)?;
    locks::private_directory(&stage, true)?;
    write_record(
        &anchor.join(BUILDING),
        &Building {
            binding: binding.clone(),
            staging_id: staging_id.clone(),
            identity: identity(&stage)?,
        },
    )?;
    let stage_db = stage.join(DATABASE);
    locks::create_private_database(&stage_db)?;
    let mut copy = sql(&stage_db, false)?;
    Backup::new(&db, &mut copy)?.run_to_completion(1024, Duration::from_millis(5), None)?;
    copy_tree(source, &stage, &files)?;
    checkpoint("return_during_copy");
    let source_digest = fingerprint(&db, source, &files)?;
    if fingerprint(&copy, &stage, &files)? != source_digest {
        return refuse("migration_integrity_failed");
    }
    verify_files(&copy, source, &stage)?;
    let tx = copy.transaction()?;
    rewrite_paths(&tx, source, &stage, anchor)?;
    tx.execute("DROP TABLE store_location", [])?;
    verify(&tx, schema::FORMAT)?;
    tx.commit()?;
    let installed_digest = fingerprint(&copy, &stage, &files)?;
    copy.pragma_update(None, "journal_mode", "DELETE")?;
    copy.close().map_err(|(_, error)| error)?;
    locks::open_private(&stage_db, false, true)?.sync_all()?;
    locks::sync_directory(&stage)?;
    locks::sync_directory(anchor)?;
    let intent = Intent {
        returned: Returned {
            binding,
            database: identity(&stage_db)?,
            backup: identity(source)?,
        },
        anchor: identity(&anchor.join(DATABASE))?,
        staging_id,
        database_digest: digest(&stage_db)?,
        installed_digest,
        source_digest,
        moves: collect_moves(&stage, anchor)?,
    };
    write_record(&anchor.join(PENDING), &intent)?;
    Ok(intent)
}

/// A confirmed owner-local operation; never dispatched by ACP or MCP.
pub fn return_to_project(key: &ProjectKey, store_id: &str, confirmed: bool) -> CoreResult<()> {
    if !confirmed {
        return refuse("rollback_requires_confirmation");
    }
    if uuid::Uuid::parse_str(store_id).is_err() {
        return refuse("project_store_not_found");
    }
    let anchor = key.anchor_directory();
    let home = key
        .home_directory()
        .ok_or(CoreError::Refused("store_root_mismatch"))?;
    locks::private_directory(&anchor, false)?;
    let _scheduler = SchedulerOwner::acquire(key)?;
    let _owner = ProjectOwner::acquire(key)?;
    let _access = fence(&anchor, ".access.lock")?;
    let _connections = fence(&anchor, ".connections.lock")?;
    let pending = anchor.join(PENDING);
    let recovery: Option<Intent> = read_record(&pending)?;
    if recovery.is_none() && returned_locally(key, &home)? {
        let record: Returned = read_record(&anchor.join(RETURNED))?
            .ok_or(CoreError::Refused("store_return_conflict"))?;
        return if record.binding.store_id == store_id {
            Ok(())
        } else {
            refuse("project_store_not_found")
        };
    }
    let archived = archive_path(key, store_id, false)?;
    let source = match (present(&home)?, present(&archived)?) {
        (true, false) => &home,
        (false, true) if recovery.is_some() => &archived,
        _ => return refuse("store_return_conflict"),
    };
    locks::private_directory(source, false)?;
    let _source_access = fence(source, ".access.lock")?;
    let _source_connections = fence(source, ".connections.lock")?;
    key.clone()
        .at_directory(source.to_path_buf())
        .validate_sidecars()?;
    let binding = location::read_binding(&sql(&source.join(DATABASE), true)?)?;
    binding.validate(key, &home)?;
    if binding.store_id != store_id {
        return refuse("project_store_not_found");
    }
    if present(&source.join(FORWARD_PENDING))? {
        return refuse("store_relocation_conflict");
    }
    if binding.source_digest.is_some() {
        retained_identity(source, &binding)?;
    }
    archive_path(key, store_id, true)?;
    let live = anchor.join(DATABASE);
    let intent = match recovery {
        Some(intent)
            if intent.returned.binding == binding
                && item_matches(source, intent.returned.backup)? =>
        {
            intent
        }
        Some(_) => return refuse("store_return_conflict"),
        None if source == &home && location::read_anchor(&live)?.as_ref() == Some(&binding) => {
            clear_incomplete_stage(&anchor, &binding)?;
            prepare(key, source, &anchor, binding)?
        }
        None => return refuse("store_return_conflict"),
    };
    let stage = staging_path(&anchor, &intent.staging_id)?;
    if !item_matches(&live, intent.returned.database)? {
        locks::private_directory(&stage, false)?;
        let stage_db = stage.join(DATABASE);
        if source != &home
            || !item_matches(&live, intent.anchor)?
            || location::read_anchor(&live)?.as_ref() != Some(&intent.returned.binding)
            || !item_matches(&stage_db, intent.returned.database)?
            || digest(&stage_db)? != intent.database_digest
        {
            return refuse("store_return_conflict");
        }
        let original = sql(&source.join(DATABASE), true)?;
        let mut files = Vec::new();
        entries(source, Path::new(""), &mut files)?;
        if fingerprint(&original, source, &files)? != intent.source_digest {
            return refuse("store_return_conflict");
        }
        perform_moves(&intent, &stage, &anchor)?;
        if fingerprint(&original, &anchor, &files)? != intent.source_digest {
            return refuse("store_return_conflict");
        }
        verify_files(&original, source, &anchor)?;
        drop(original);
        checkpoint("return_before_database");
        fs::rename(&stage_db, &live)?;
        locks::sync_directory(&stage)?;
        locks::sync_directory(&anchor)?;
        checkpoint("return_after_database");
    }
    let published = sql(&live, true)?;
    verify(&published, schema::FORMAT)?;
    let original = sql(&source.join(DATABASE), true)?;
    let mut files = Vec::new();
    entries(source, Path::new(""), &mut files)?;
    if fingerprint(&original, source, &files)? != intent.source_digest
        || fingerprint(&published, &anchor, &files)? != intent.installed_digest
    {
        return refuse("store_return_conflict");
    }
    verify_files(&original, &home, &anchor)?;
    drop((published, original));
    if source == &home {
        rename_new(&home, &archived)?;
        locks::sync_directory(home.parent().unwrap())?;
        locks::sync_directory(archived.parent().unwrap())?;
        checkpoint("return_after_archive");
    }
    let returned = anchor.join(RETURNED);
    match read_record::<Returned>(&returned)? {
        Some(record) if record == intent.returned => {}
        Some(_) => return refuse("store_return_conflict"),
        None => write_record(&returned, &intent.returned)?,
    }
    if present(&stage)? {
        locks::private_directory(&stage, false)?;
        let empty_backups = stage.join(BACKUPS);
        if present(&empty_backups)? {
            fs::remove_dir(empty_backups)?;
        }
        fs::remove_dir(&stage)?;
    }
    let building = anchor.join(BUILDING);
    if let Some(record) = read_record::<Building>(&building)? {
        if record.binding != intent.returned.binding || record.staging_id != intent.staging_id {
            return refuse("store_return_conflict");
        }
        fs::remove_file(building)?;
    }
    fs::remove_file(&pending)?;
    locks::sync_directory(&anchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpenMode, Store, StoreChannel, store::StoreRoot};

    struct Fixture {
        base: PathBuf,
        key: ProjectKey,
        id: String,
    }

    impl Fixture {
        fn new() -> Self {
            Self::from_version(schema::FORMAT)
        }

        fn from_version(version: i64) -> Self {
            let base = std::env::temp_dir().join(format!("cortex-return-{}", uuid::Uuid::new_v4()));
            locks::private_directory(&base, true).unwrap();
            let project = base.join("project");
            locks::private_directory(&project, true).unwrap();
            let legacy = ProjectKey::new(&project, StoreChannel::Dev).unwrap();
            drop(Store::open(legacy.clone(), OpenMode::Initialize).unwrap());
            if version != schema::FORMAT {
                let db = sql(&legacy.database(), false).unwrap();
                db.pragma_update(None, "user_version", version).unwrap();
            }
            let key = StoreRoot::new(&base.join("state"), StoreChannel::Dev)
                .unwrap()
                .project_key(project)
                .unwrap();
            let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
            let dir = store.key().directory().join("draft-reports");
            locks::private_directory(&dir, true).unwrap();
            let draft = dir.join("r_draft.md");
            create(&draft)
                .unwrap()
                .write_all(b"Latest document\n")
                .unwrap();
            let metadata = draft.metadata().unwrap();
            store.write(|db| {
                db.execute("INSERT INTO tasks VALUES('t_latest',?1,?2)", (key.root().to_str(), crate::now()))?;
                db.execute(
                    "INSERT INTO drafts VALUES('d_latest','t_latest','thread','report','fixture',?1,?2,?3,?4,NULL,NULL)",
                    (draft.to_str(), crate::now(), metadata.dev() as i64, metadata.ino() as i64)
                )?;
                Ok(())
            }).unwrap();
            let id = store
                .read(|db| Ok(location::read_binding(db)?.store_id))
                .unwrap();
            Self { base, key, id }
        }

        fn crash(&self, point: &str) {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::migration::return_to_project::tests::return_child",
                ])
                .env("CORTEX_TEST_RETURN_BASE", &self.base)
                .env("CORTEX_TEST_RETURN_ID", &self.id)
                .env("CORTEX_CORE_MIGRATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(97));
        }

        fn assert_returned(&self) {
            let (anchor, home) = (
                self.key.anchor_directory(),
                self.key.home_directory().unwrap(),
            );
            let backup = archive_path(&self.key, &self.id, false).unwrap();
            assert!(backup.join(DATABASE).is_file());
            assert!(!home.exists());
            assert!(!anchor.join(PENDING).exists());
            let store = Store::open(self.key.clone(), OpenMode::Initialize).unwrap();
            assert_eq!(store.key().directory(), anchor);
            assert!(!home.exists());
            let draft = anchor.join("draft-reports/r_draft.md");
            let metadata = draft.metadata().unwrap();
            store
                .read(|db| {
                    verify(db, schema::FORMAT)?;
                    let row: (String, i64, i64) = db.query_row(
                        "SELECT path,device,inode FROM drafts WHERE id='d_latest'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                    assert_eq!(
                        row,
                        (
                            draft.to_str().unwrap().to_owned(),
                            metadata.dev() as i64,
                            metadata.ino() as i64
                        )
                    );
                    assert!(!schema::tables(db)?.contains("store_location"));
                    Ok(())
                })
                .unwrap();
            assert_eq!(fs::read(draft).unwrap(), b"Latest document\n");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    #[test]
    fn return_child() {
        let Some(base) = std::env::var_os("CORTEX_TEST_RETURN_BASE") else {
            return;
        };
        let base = PathBuf::from(base);
        let key = StoreRoot::new(&base.join("state"), StoreChannel::Dev)
            .unwrap()
            .project_key(base.join("project"))
            .unwrap();
        let id = std::env::var("CORTEX_TEST_RETURN_ID").unwrap();
        return_to_project(&key, &id, true).unwrap();
        panic!("return crash checkpoint did not run");
    }

    #[test]
    fn return_requires_confirmation_and_exact_binding() {
        let fixture = Fixture::new();
        assert_eq!(
            return_to_project(&fixture.key, &fixture.id, false)
                .unwrap_err()
                .code(),
            "rollback_requires_confirmation"
        );
        let wrong_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            return_to_project(&fixture.key, &wrong_id, true)
                .unwrap_err()
                .code(),
            "project_store_not_found"
        );
        assert!(!fixture.key.anchor_directory().join(PENDING).exists());
        let store = Store::open(fixture.key.clone(), OpenMode::RuntimeValidate).unwrap();
        assert_eq!(
            return_to_project(&fixture.key, &fixture.id, true)
                .unwrap_err()
                .code(),
            "busy"
        );
        drop(store);
        return_to_project(&fixture.key, &fixture.id, true).unwrap();
        return_to_project(&fixture.key, &fixture.id, true).unwrap();
        fixture.assert_returned();
    }

    #[test]
    fn return_resumes_crashes_and_refuses_a_replaced_staged_database() {
        let fixture = Fixture::new();
        fixture.crash("return_during_copy");
        let reader = Store::open(fixture.key.clone(), OpenMode::LiveReadOnly).unwrap();
        assert_eq!(Some(reader.key().directory()), fixture.key.home_directory());
        drop(reader);
        return_to_project(&fixture.key, &fixture.id, true).unwrap();
        fixture.assert_returned();
        for point in [
            "return_before_database",
            "return_after_database",
            "return_after_archive",
        ] {
            let fixture = Fixture::new();
            fixture.crash(point);
            assert_eq!(
                Store::open(fixture.key.clone(), OpenMode::LiveReadOnly)
                    .err()
                    .unwrap()
                    .code(),
                "store_return_pending"
            );
            return_to_project(&fixture.key, &fixture.id, true).unwrap();
            fixture.assert_returned();
        }
        let fixture = Fixture::new();
        fixture.crash("return_before_database");
        let anchor = fixture.key.anchor_directory();
        let intent: Intent = read_record(&anchor.join(PENDING)).unwrap().unwrap();
        let stage = anchor.join(format!(".return-stage-{}", intent.staging_id));
        let replaced = fixture.base.join("retained-stage.sqlite3");
        fs::rename(stage.join(DATABASE), &replaced).unwrap();
        fs::copy(&replaced, stage.join(DATABASE)).unwrap();
        assert_eq!(
            return_to_project(&fixture.key, &fixture.id, true)
                .unwrap_err()
                .code(),
            "store_return_conflict"
        );
        assert!(
            fixture
                .key
                .home_directory()
                .unwrap()
                .join(DATABASE)
                .is_file()
        );
        assert!(anchor.join(PENDING).is_file());
    }

    #[test]
    fn return_preserves_format11_backups_and_refuses_changed_published_copy() {
        let fixture = Fixture::from_version(11);
        return_to_project(&fixture.key, &fixture.id, true).unwrap();
        fixture.assert_returned();

        let fixture = Fixture::new();
        fixture.crash("return_after_database");
        let live = fixture.key.anchor_directory().join(DATABASE);
        let db = sql(&live, false).unwrap();
        db.execute(
            "UPDATE drafts SET template='changed' WHERE id='d_latest'",
            [],
        )
        .unwrap();
        drop(db);
        assert_eq!(
            return_to_project(&fixture.key, &fixture.id, true)
                .unwrap_err()
                .code(),
            "store_return_conflict"
        );
        assert!(
            fixture
                .key
                .home_directory()
                .unwrap()
                .join(DATABASE)
                .is_file()
        );
    }
}
