//! Fenced, restartable relocation of a project-local store to per-user storage.
use super::{checkpoint, create, digest, upgrade11, verify};
use crate::{
    error::refuse,
    store::{location, locks, schema, ProjectKey},
    CoreError, CoreResult,
};
use rusqlite::{backup::Backup, types::ValueRef, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    time::Duration,
};

const DATABASE: &str = "cortex.sqlite3";
const TEMP_ANCHOR: &str = ".relocation-anchor.tmp";
pub(crate) const INTENT: &str = ".relocation-intent.json";
pub(crate) const COMPLETE: &str = ".relocation-complete";
const BACKUPS: &str = ".migration-backups";
const SNAPSHOT: &str = ".relocation-snapshot";

#[derive(Deserialize, Serialize)]
struct Intent {
    binding: location::Binding,
    source_dev: u64,
    source_ino: u64,
    backup: String,
}

#[derive(Deserialize, Serialize)]
struct Completed {
    intent: Intent,
    backup_identity: (u64, u64),
}

pub(super) fn present(path: &Path) -> CoreResult<bool> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn sql(path: &Path, readonly: bool) -> CoreResult<Connection> {
    locks::regular(path)?;
    if path.file_name().is_some_and(|name| name == DATABASE) {
        validate_sidecars(path.parent().ok_or(CoreError::Refused("unsafe_storage"))?)?;
    }
    let flags = if readonly {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let db = Connection::open_with_flags(path, flags)?;
    db.busy_timeout(Duration::ZERO)?;
    Ok(db)
}

fn validate_sidecars(directory: &Path) -> CoreResult<()> {
    for name in [
        "cortex.sqlite3-wal",
        "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
    ] {
        let path = directory.join(name);
        if present(&path)? {
            locks::regular(&path)?;
        }
    }
    Ok(())
}

pub(super) fn fence(dir: &Path, name: &str) -> CoreResult<locks::AccessLock> {
    locks::AccessLock::acquire(&dir.join(name), true, true, Duration::ZERO)
}

pub(super) fn rename_new(source: &Path, target: &Path) -> CoreResult<()> {
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn top_service_file(name: &str) -> bool {
    matches!(
        name,
        DATABASE
            | "cortex.sqlite3-wal"
            | "cortex.sqlite3-shm"
            | "cortex.sqlite3-journal"
            | ".access.lock"
            | ".connections.lock"
            | ".publication.lock"
            | ".scheduler-owner.lock"
            | ".core-owner.lock"
            | ".project-mutation.lock"
            | ".gitignore"
            | "owner.json"
            | "legacy-workspace.json"
            | ".last-initialized"
            | TEMP_ANCHOR
            | SNAPSHOT
            | INTENT
            | COMPLETE
    )
}

pub(super) fn children(dir: &Path) -> CoreResult<Vec<PathBuf>> {
    let mut paths = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort_unstable();
    Ok(paths)
}

fn owned_file(path: &Path) -> CoreResult<File> {
    let meta = path.symlink_metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::getuid() } || meta.nlink() != 1 {
        return refuse("unsafe_storage");
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
        return refuse("unsafe_storage");
    }
    Ok(file)
}

fn bounded_text(path: &Path, limit: u64) -> CoreResult<String> {
    let file = owned_file(path)?;
    if file.metadata()?.len() > limit {
        return refuse("store_relocation_linked_workspace");
    }
    let mut value = String::new();
    file.take(limit + 1).read_to_string(&mut value)?;
    if value.len() as u64 > limit {
        return refuse("store_relocation_linked_workspace");
    }
    Ok(value)
}

fn check_self_contained_git(marker: &Path, metadata: &fs::Metadata) -> CoreResult<()> {
    let workspace = marker
        .parent()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    let admin = if metadata.is_dir() {
        marker.to_path_buf()
    } else {
        let marker_text = bounded_text(marker, 4096)?;
        let Some(relative) = marker_text.trim_end().strip_prefix("gitdir: ") else {
            return refuse("store_relocation_linked_workspace");
        };
        let relative = Path::new(relative);
        let mut admin = workspace.to_path_buf();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return refuse("store_relocation_linked_workspace");
            };
            admin.push(part);
            if !admin.symlink_metadata()?.is_dir() {
                return refuse("store_relocation_linked_workspace");
            }
        }
        if admin == workspace {
            return refuse("store_relocation_linked_workspace");
        }
        admin
    };
    if !admin.symlink_metadata()?.is_dir()
        || present(&admin.join("commondir"))?
        || present(&admin.join("gitdir"))?
    {
        return refuse("store_relocation_linked_workspace");
    }
    let registrations = admin.join("worktrees");
    if present(&registrations)?
        && (!registrations.symlink_metadata()?.is_dir() || !children(&registrations)?.is_empty())
    {
        return refuse("store_relocation_linked_workspace");
    }
    let objects = admin.join("objects");
    if present(&objects)? {
        if !objects.symlink_metadata()?.is_dir() {
            return refuse("store_relocation_linked_workspace");
        }
        let info = objects.join("info");
        if present(&info)?
            && (!info.symlink_metadata()?.is_dir()
                || present(&info.join("alternates"))?
                || present(&info.join("http-alternates"))?)
        {
            return refuse("store_relocation_linked_workspace");
        }
    }
    for config in [admin.join("config"), admin.join("config.worktree")] {
        if !present(&config)? {
            continue;
        }
        let contents = bounded_text(&config, 1024 * 1024)?;
        if contents.lines().any(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.starts_with("[include") || line.split([' ', '\t', '=']).next() == Some("worktree")
        }) {
            return refuse("store_relocation_linked_workspace");
        }
    }
    Ok(())
}

pub(super) fn entries(
    root: &Path,
    relative: &Path,
    output: &mut Vec<(PathBuf, bool, u64)>,
) -> CoreResult<()> {
    for path in children(&root.join(relative))? {
        let name = path
            .file_name()
            .ok_or(CoreError::Refused("unsafe_storage"))?;
        let name_text = name.to_str().unwrap_or("");
        if relative.as_os_str().is_empty() && top_service_file(name_text) {
            continue;
        }
        // The retained originals are deliberately outside subsequent snapshots.
        if relative == Path::new(BACKUPS) && name_text.starts_with("relocated-") {
            continue;
        }
        if name_text.starts_with(".publication_") && name_text.ends_with(".json") {
            return refuse("store_relocation_pending_publication");
        }
        let next = relative.join(name);
        let meta = path.symlink_metadata()?;
        if name_text == ".git" && !relative.as_os_str().is_empty() {
            check_self_contained_git(&path, &meta)?;
        }
        if meta.is_dir() {
            if meta.uid() != unsafe { libc::getuid() } {
                return refuse("unsafe_storage");
            }
            output.push((next.clone(), true, 0));
            entries(root, &next, output)?;
        } else {
            drop(owned_file(&path)?);
            output.push((next, false, meta.len()));
        }
    }
    Ok(())
}

fn sync_tree(root: &Path, entries: &[(PathBuf, bool, u64)]) -> CoreResult<()> {
    for (path, _, _) in entries.iter().rev().filter(|(_, is_dir, _)| *is_dir) {
        locks::sync_directory(&root.join(path))?;
    }
    locks::sync_directory(root)
}

pub(super) fn copy_tree(
    source: &Path,
    stage: &Path,
    entries: &[(PathBuf, bool, u64)],
) -> CoreResult<()> {
    for (path, directory, _) in entries {
        let target = stage.join(path);
        if *directory {
            locks::private_directory(&target, true)?;
        } else {
            let mut input = owned_file(&source.join(path))?;
            let mut output = create(&target)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        }
    }
    sync_tree(stage, entries)
}

pub(super) fn check_space(
    source: &Path,
    target: &Path,
    entries: &[(PathBuf, bool, u64)],
) -> CoreResult<()> {
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(target.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    let mut required: u128 = entries.iter().map(|(_, _, size)| u128::from(*size)).sum();
    for name in [DATABASE, "cortex.sqlite3-wal"] {
        let path = source.join(name);
        if present(&path)? {
            locks::regular(&path)?;
            required += u128::from(path.symlink_metadata()?.len());
        }
    }
    if u128::from(stat.f_bavail) * u128::from(stat.f_frsize) * 10 < required * 11 {
        return refuse("store_relocation_no_space");
    }
    Ok(())
}

fn frame(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn catalog_shape(db: &Connection) -> CoreResult<Vec<(String, i64, Option<i64>)>> {
    let mut tables = db.prepare(
        "SELECT name,sql FROM sqlite_schema
         WHERE type='table' AND name NOT IN ('store_location','format_history','sqlite_sequence') ORDER BY name",
    )?;
    let mut tables = tables.query([])?;
    let mut shape = Vec::new();
    while let Some(row) = tables.next()? {
        let table: String = row.get(0)?;
        let declaration: String = row.get(1)?;
        let maximum = if declaration.to_ascii_uppercase().contains("WITHOUT ROWID") {
            "NULL"
        } else {
            "max(rowid)"
        };
        let (count, highest) = db.query_row(
            &format!("SELECT count(*),{maximum} FROM {}", quote(&table)),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        shape.push((table, count, highest));
    }
    Ok(shape)
}

/// All persisted table values, with rowid ordering where present, and every copied
/// file (including unknown legacy trees). An update of an existing row changes this
/// digest even when row counts, maximum IDs and the database file size stay fixed.
pub(super) fn fingerprint(
    db: &Connection,
    root: &Path,
    entries: &[(PathBuf, bool, u64)],
) -> CoreResult<String> {
    let mut hash = Sha256::new();
    let mut tables = db.prepare(
        "SELECT name,sql FROM sqlite_schema WHERE type='table' AND name!='store_location' ORDER BY name",
    )?;
    let mut tables = tables.query([])?;
    while let Some(table) = tables.next()? {
        let name: String = table.get(0)?;
        let declaration: String = table.get(1)?;
        frame(&mut hash, name.as_bytes());
        frame(&mut hash, declaration.as_bytes());
        let without_rowid = declaration.to_ascii_uppercase().contains("WITHOUT ROWID");
        let select = if without_rowid {
            let sql = format!("PRAGMA table_info({})", quote(&name));
            let columns = db
                .prepare(&sql)?
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut primary: Vec<_> = columns.into_iter().filter(|(key, _)| *key != 0).collect();
            primary.sort_unstable();
            let order = primary
                .into_iter()
                .map(|(_, name)| quote(&name))
                .collect::<Vec<_>>();
            format!(
                "SELECT * FROM {} ORDER BY {}",
                quote(&name),
                order.join(",")
            )
        } else {
            format!("SELECT rowid,* FROM {} ORDER BY rowid", quote(&name))
        };
        let mut statement = db.prepare(&select)?;
        let column_count = statement.column_count();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hash.update([1]);
            for column in 0..column_count {
                match row.get_ref(column)? {
                    ValueRef::Null => hash.update([0]),
                    ValueRef::Integer(value) => {
                        hash.update([1]);
                        hash.update(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        hash.update([2]);
                        hash.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        hash.update([3]);
                        frame(&mut hash, value);
                    }
                    ValueRef::Blob(value) => {
                        hash.update([4]);
                        frame(&mut hash, value);
                    }
                }
            }
        }
        hash.update([0]);
    }
    for (path, is_dir, _) in entries {
        hash.update([u8::from(*is_dir)]);
        frame(&mut hash, path.as_os_str().as_bytes());
        if !is_dir {
            let mut file = owned_file(&root.join(path))?;
            let mut contents = Sha256::new();
            std::io::copy(&mut file, &mut contents)?;
            frame(&mut hash, &contents.finalize());
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn string(path: &Path) -> CoreResult<&str> {
    path.to_str().ok_or(CoreError::Refused("unsafe_storage"))
}

fn relative<'a>(path: &'a str, root: &Path) -> CoreResult<&'a Path> {
    let path = Path::new(path)
        .strip_prefix(root)
        .map_err(|_| CoreError::Refused("unsafe_storage"))?;
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return refuse("unsafe_storage");
    }
    Ok(path)
}

fn leaf(value: &str) -> CoreResult<&Path> {
    let path = Path::new(value);
    if !matches!(path.components().next(), Some(Component::Normal(_)))
        || path.components().count() != 1
    {
        return refuse("unsafe_storage");
    }
    Ok(path)
}

pub(super) fn verify_files(db: &Connection, source: &Path, stage: &Path) -> CoreResult<()> {
    let mut reports = db.prepare(
        "SELECT r.task_id,r.filename,e.digest FROM reports r JOIN editions e ON e.report_id=r.id
         WHERE e.sequence=(SELECT max(sequence) FROM editions WHERE report_id=r.id)",
    )?;
    let mut reports = reports.query([])?;
    while let Some(row) = reports.next()? {
        let task: String = row.get(0)?;
        let report: String = row.get(1)?;
        let expected: String = row.get(2)?;
        if digest(&stage.join(leaf(&task)?).join(leaf(&report)?))? != expected {
            return refuse("migration_integrity_failed");
        }
    }

    let mut drafts = db.prepare("SELECT path, published_digest FROM drafts")?;
    let mut drafts = drafts.query([])?;
    while let Some(row) = drafts.next()? {
        let path: String = row.get(0)?;
        let path = stage.join(relative(&path, source)?);
        let expected: Option<String> = row.get(1)?;
        if let Some(expected) = expected {
            if present(&path)? && digest(&path)? != expected {
                return refuse("migration_integrity_failed");
            }
        } else if !present(&path)? {
            return refuse("migration_integrity_failed");
        }
    }
    let mut pending = db.prepare("SELECT path,digest FROM pending_source_deletions")?;
    let mut pending = pending.query([])?;
    while let Some(row) = pending.next()? {
        let path: String = row.get(0)?;
        let expected: String = row.get(1)?;
        let path = stage.join(relative(&path, source)?);
        if present(&path)? && digest(&path)? != expected {
            return refuse("migration_integrity_failed");
        }
    }
    Ok(())
}

pub(super) fn rewrite_paths(
    db: &Connection,
    source: &Path,
    stage: &Path,
    destination: &Path,
) -> CoreResult<()> {
    let tables = schema::tables(db)?;
    for (table, column) in [
        ("drafts", "path"),
        ("pending_source_deletions", "path"),
        ("format_history", "backup_path"),
        ("workspace_snapshots", "root_path"),
        ("workspace_snapshots", "base_path"),
        ("candidate_staging", "stage_path"),
        ("task_workspaces", "workspace_root"),
        ("acp_session_workspaces", "workspace_root"),
    ] {
        if !tables.contains(table) {
            continue;
        }
        let mut statement = db.prepare(&format!(
            "SELECT rowid,{column} FROM {table} WHERE {column} IS NOT NULL"
        ))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (rowid, old) in rows {
            if !Path::new(&old).starts_with(source)
                && !matches!(table, "drafts" | "pending_source_deletions")
            {
                continue;
            }
            let tail = relative(&old, source)?;
            let new = destination.join(tail);
            if table == "drafts" && present(&stage.join(tail))? {
                let meta = stage.join(tail).symlink_metadata()?;
                db.execute(
                    "UPDATE drafts SET path=?,device=?,inode=? WHERE rowid=?",
                    (string(&new)?, meta.dev() as i64, meta.ino() as i64, rowid),
                )?;
            } else {
                db.execute(
                    &format!("UPDATE {table} SET {column}=? WHERE rowid=?"),
                    (string(&new)?, rowid),
                )?;
            }
        }
    }
    // This receipt is replayed as a live writable path by Publisher::create.
    // Other captured paths in transcripts and evidence remain historical.
    let mut receipts = db.prepare(
        "SELECT rowid,json_extract(response,'$.draft_path') FROM deliveries WHERE operation='create_draft'",
    )?;
    let rows = receipts
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (rowid, old) in rows {
        let new = destination.join(relative(&old, source)?);
        db.execute(
            "UPDATE deliveries SET response=json_set(response,'$.draft_path',?) WHERE rowid=?",
            (string(&new)?, rowid),
        )?;
    }
    Ok(())
}

fn stage_copy(
    key: &ProjectKey,
    source: &Path,
    db: &Connection,
    destination: &Path,
    parent: &Path,
    files: &[(PathBuf, bool, u64)],
) -> CoreResult<locks::AccessLock> {
    let slot = destination
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    let staging_root = parent.join(format!(".staging-{slot}-{}", key.channel().directory()));
    if present(&staging_root)? {
        locks::private_directory(&staging_root, false)?;
        fs::remove_dir_all(&staging_root)?;
        locks::sync_directory(parent)?;
    }
    check_space(source, parent, files)?;
    locks::private_directory(&staging_root, true)?;
    let stage = staging_root.join(key.channel().directory());
    let result = (|| {
        locks::private_directory(&stage, true)?;
        let lock = fence(&stage, ".access.lock")?;
        let path = stage.join(DATABASE);
        drop(create(&path)?);
        let mut copy = sql(&path, false)?;
        Backup::new(db, &mut copy)?.run_to_completion(1024, Duration::from_millis(5), None)?;
        copy_tree(source, &stage, files)?;
        verify_files(&copy, source, &stage)?;
        let mut binding = location::Binding::new(key, destination)?;
        binding.source_digest = Some(fingerprint(&copy, &stage, files)?);
        let shape = catalog_shape(&copy)?;
        let tx = copy.transaction()?;
        rewrite_paths(&tx, source, &stage, destination)?;
        location::write_binding(&tx, &binding)?;
        tx.execute(
            "INSERT INTO format_history(action,from_version,to_version,backup_path,backup_digest,at)
             VALUES ('relocate',12,12,?,?,?)",
            (string(&source.join(BACKUPS).join(format!("relocated-{}", binding.store_id)).join(DATABASE))?,
             binding.source_digest.as_deref(), crate::now()),
        )?;
        if catalog_shape(&tx)? != shape {
            return refuse("migration_integrity_failed");
        }
        schema::validate_project(&tx, string(key.root())?)?;
        verify(&tx, 12)?;
        let mut witness = create(&stage.join(SNAPSHOT))?;
        witness.write_all(fingerprint(&tx, &stage, files)?.as_bytes())?;
        witness.sync_all()?;
        tx.commit()?;
        copy.pragma_update(None, "journal_mode", "DELETE")?;
        drop(copy);
        File::open(&path)?.sync_all()?;
        locks::sync_directory(&stage)?;
        locks::sync_directory(&staging_root)?;
        checkpoint("relocate_before_install");
        rename_new(&stage, destination)?;
        locks::sync_directory(
            destination
                .parent()
                .ok_or(CoreError::Refused("unsafe_storage"))?,
        )?;
        locks::sync_directory(parent)?;
        checkpoint("relocate_after_install");
        Ok(lock)
    })();
    if present(&staging_root)? {
        fs::remove_dir_all(&staging_root)?;
        locks::sync_directory(parent)?;
    }
    result
}

fn read_intent(path: &Path) -> CoreResult<Option<Intent>> {
    if !present(path)? {
        return Ok(None);
    }
    let file = locks::open_private(path, false, true)?;
    if file.metadata()?.len() > 16384 {
        return refuse("unsafe_storage");
    }
    Ok(Some(serde_json::from_reader(file)?))
}

fn record_intent(path: &Path, intent: &Intent) -> CoreResult<()> {
    let directory = path.parent().ok_or(CoreError::Refused("unsafe_storage"))?;
    let temp = directory.join(format!(".intent-{}", uuid::Uuid::new_v4().simple()));
    let mut file = create(&temp)?;
    serde_json::to_writer(&mut file, intent)?;
    file.sync_all()?;
    drop(file);
    rename_new(&temp, path)?;
    locks::sync_directory(directory)
}

pub(super) fn remove_sidecars(source: &Path) -> CoreResult<()> {
    for name in [
        "cortex.sqlite3-wal",
        "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
    ] {
        let path = source.join(name);
        if present(&path)? {
            locks::regular(&path)?;
            fs::remove_file(path)?;
        }
    }
    locks::sync_directory(source)
}

fn archive_originals(source: &Path, backup: &Path) -> CoreResult<()> {
    for item in children(source)? {
        let name = item
            .file_name()
            .ok_or(CoreError::Refused("unsafe_storage"))?;
        let text = name.to_str().unwrap_or("");
        if text != BACKUPS && !top_service_file(text) {
            rename_new(&item, &backup.join(name))?;
        }
    }
    locks::sync_directory(backup)?;
    locks::sync_directory(source)
}

fn matches_inode(path: &Path, intent: &Intent, links: u64) -> CoreResult<bool> {
    let meta = path.symlink_metadata()?;
    Ok(meta.is_file()
        && meta.uid() == unsafe { libc::getuid() }
        && meta.mode() & 0o077 == 0
        && meta.nlink() == links
        && (meta.dev(), meta.ino()) == (intent.source_dev, intent.source_ino))
}

fn quarantine(destination: &Path) -> CoreResult<()> {
    let project = destination
        .parent()
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    let backups = project.join(BACKUPS);
    locks::private_directory(&backups, true)?;
    rename_new(
        destination,
        &backups.join(format!("stale-staging-{}", uuid::Uuid::new_v4().simple())),
    )?;
    locks::sync_directory(&backups)?;
    locks::sync_directory(project)
}

fn verify_installed_stage(key: &ProjectKey, destination: &Path) -> CoreResult<()> {
    let db = sql(&destination.join(DATABASE), true)?;
    schema::validate_project(&db, string(key.root())?)?;
    verify(&db, 12)?;
    let witness = locks::open_private(&destination.join(SNAPSHOT), false, true)?;
    if witness.metadata()?.len() != 64 {
        return refuse("migration_integrity_failed");
    }
    let mut expected = String::new();
    witness.take(64).read_to_string(&mut expected)?;
    let mut files = Vec::new();
    entries(destination, Path::new(""), &mut files)?;
    if fingerprint(&db, destination, &files)? != expected {
        return refuse("migration_integrity_failed");
    }
    Ok(())
}

fn complete(
    key: &ProjectKey,
    source: &Path,
    destination: &Path,
    db: Connection,
    binding: location::Binding,
    intent: Option<Intent>,
) -> CoreResult<()> {
    let original = source.join(DATABASE);
    let intent_path = destination.join(INTENT);
    let backups = source.join(BACKUPS);
    let backup_name = format!("relocated-{}", binding.store_id);
    let backup = backups.join(&backup_name);
    let backup_db = backup.join(DATABASE);
    let temp_anchor = source.join(TEMP_ANCHOR);

    let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        drop(db);
        quarantine(destination)?;
        return refuse("busy");
    }
    let mut files = Vec::new();
    entries(source, Path::new(""), &mut files)?;
    let actual = fingerprint(&db, source, &files)?;
    drop(db);
    if binding.source_digest.as_deref() != Some(&actual)
        || verify_installed_stage(key, destination).is_err()
    {
        if present(&temp_anchor)? {
            locks::regular(&temp_anchor)?;
            fs::remove_file(&temp_anchor)?;
        }
        if intent.is_some() && present(&backup)? && children(&backup)?.is_empty() {
            fs::remove_dir(&backup)?;
            locks::sync_directory(&backups)?;
        }
        quarantine(destination)?;
        return refuse("store_relocation_conflict");
    }
    remove_sidecars(source)?;
    let meta = original.symlink_metadata()?;
    let intent = match intent {
        Some(intent) => intent,
        None => {
            let intent = Intent {
                binding: binding.clone(),
                source_dev: meta.dev(),
                source_ino: meta.ino(),
                backup: backup_name,
            };
            record_intent(&intent_path, &intent)?;
            intent
        }
    };
    if !matches_inode(&original, &intent, 1)? {
        return refuse("unsafe_storage");
    }
    locks::private_directory(&backups, false)?;
    locks::private_directory(&backup, true)?;
    locks::sync_directory(&backups)?;
    checkpoint("relocate_before_link");
    fs::hard_link(&original, &backup_db)?;
    locks::sync_directory(&backup)?;
    checkpoint("relocate_after_link");
    if present(&temp_anchor)? {
        locks::regular(&temp_anchor)?;
        fs::remove_file(&temp_anchor)?;
    }
    location::prepare_anchor(&temp_anchor, &binding)?;
    checkpoint("relocate_before_anchor_rename");
    fs::rename(&temp_anchor, &original)?;
    locks::sync_directory(source)?;
    checkpoint("relocate_after_anchor_rename");
    finish(key, source, destination, &backup, &intent)
}

fn finish(
    key: &ProjectKey,
    source: &Path,
    destination: &Path,
    backup: &Path,
    intent: &Intent,
) -> CoreResult<()> {
    let retained = backup.join(DATABASE);
    locks::private_directory(backup, false)?;
    locks::regular(&retained)?;
    intent.binding.validate(key, destination)?;
    if !matches_inode(&retained, intent, 1)? {
        return refuse("unsafe_storage");
    }
    archive_originals(source, backup)?;
    let marker = destination.join(COMPLETE);
    if present(&marker)? {
        verify_retained_original(destination, backup, &intent.binding)?;
    } else {
        let directory = backup.symlink_metadata()?;
        let completed = Completed {
            intent: read_intent(&destination.join(INTENT))?
                .ok_or(CoreError::Refused("unsafe_storage"))?,
            backup_identity: (directory.dev(), directory.ino()),
        };
        let temp = destination.join(format!(".completion-{}", uuid::Uuid::new_v4().simple()));
        let mut file = create(&temp)?;
        serde_json::to_writer(&mut file, &completed)?;
        file.sync_all()?;
        drop(file);
        rename_new(&temp, &marker)?;
        locks::sync_directory(destination)?;
    }
    fs::remove_file(destination.join(INTENT))?;
    locks::sync_directory(destination)
}

pub(crate) fn verify_retained_original(
    destination: &Path,
    backup: &Path,
    binding: &location::Binding,
) -> CoreResult<(u64, u64)> {
    let completed = read_completed(destination, binding)?;
    locks::private_directory(backup, false)?;
    let metadata = backup.symlink_metadata()?;
    if completed.backup_identity != (metadata.dev(), metadata.ino())
        || !matches_inode(&backup.join(DATABASE), &completed.intent, 1)?
    {
        return refuse("unsafe_storage");
    }
    Ok(completed.backup_identity)
}

fn read_completed(destination: &Path, binding: &location::Binding) -> CoreResult<Completed> {
    let marker = locks::open_private(&destination.join(COMPLETE), false, true)?;
    if marker.metadata()?.len() > 16384 {
        return refuse("unsafe_storage");
    }
    let completed: Completed = serde_json::from_reader(marker)?;
    if &completed.intent.binding != binding {
        return refuse("unsafe_storage");
    }
    Ok(completed)
}

pub(crate) fn retained_identity(
    destination: &Path,
    binding: &location::Binding,
) -> CoreResult<(u64, u64)> {
    Ok(read_completed(destination, binding)?.backup_identity)
}

pub(crate) fn relocate12(key: &ProjectKey, destination: &Path) -> CoreResult<()> {
    let source = key.anchor_directory();
    locks::private_directory(&source, false)?;
    let _access = fence(&source, ".access.lock")?;
    let _connections = fence(&source, ".connections.lock")?;
    let original = source.join(DATABASE);
    let projects = destination
        .parent()
        .and_then(Path::parent)
        .ok_or(CoreError::Refused("unsafe_storage"))?;
    locks::private_directory(projects, false)?;
    let installed = present(destination)?;
    let _home_access = if installed {
        locks::private_directory(destination, false)?;
        Some(fence(destination, ".access.lock")?)
    } else {
        None
    };
    let intent = read_intent(&destination.join(INTENT))?;
    let binding = if installed {
        let home = sql(&destination.join(DATABASE), true)?;
        let binding = location::read_binding(&home)?;
        binding.validate(key, destination)?;
        if binding.source_digest.is_none() {
            return refuse("store_root_mismatch");
        }
        Some(binding)
    } else {
        None
    };
    validate_sidecars(&source)?;
    if let Some(intent) = &intent {
        if binding.as_ref() != Some(&intent.binding)
            || intent.backup != format!("relocated-{}", intent.binding.store_id)
        {
            return refuse("unsafe_storage");
        }
        let backup = source.join(BACKUPS).join(leaf(&intent.backup)?);
        let retained = backup.join(DATABASE);
        let metadata = original.symlink_metadata()?;
        if metadata.nlink() > 1 {
            // Undo only our exactly identified second link before SQLite opens.
            // A fresh, fenced fingerprint check decides whether to switch again.
            locks::private_directory(&backup, false)?;
            if !matches_inode(&original, intent, 2)? || !matches_inode(&retained, intent, 2)? {
                return refuse("unsafe_storage");
            }
            fs::remove_file(&retained)?;
            locks::sync_directory(&backup)?;
        } else if location::read_anchor(&original)?.as_ref() == Some(&intent.binding) {
            let _moving = key.relocation_started();
            return finish(key, &source, destination, &backup, intent);
        } else if !matches_inode(&original, intent, 1)? || present(&retained)? {
            return refuse("unsafe_storage");
        }
    } else if let Some(anchor) = location::read_anchor(&original)? {
        if binding.as_ref() == Some(&anchor) {
            return Ok(());
        }
        return refuse("store_root_mismatch");
    }

    let mut db = sql(&original, false)?;
    let version = schema::validate_header(&db, false)?;
    schema::validate_project(&db, string(key.root())?)?;
    let _moving = key.relocation_started();
    if version == 11 {
        upgrade11(&mut db, &source)?;
    }
    // Include even an empty backups directory in both snapshots. It will hold
    // the switch hard link, but the retained originals are outside the digest.
    locks::private_directory(&source.join(BACKUPS), true)?;
    let _new_home_access;
    let binding = if let Some(binding) = binding {
        binding
    } else {
        let mut files = Vec::new();
        entries(&source, Path::new(""), &mut files)?;
        _new_home_access = stage_copy(key, &source, &db, destination, projects, &files)?;
        location::read_binding(&sql(&destination.join(DATABASE), true)?)?
    };
    complete(key, &source, destination, db, binding, intent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        administration::{self, Removal, RootStatus},
        OpenMode, Store, StoreChannel, StoreRoot,
    };
    use std::process::Command;

    struct Fixture {
        base: PathBuf,
        project: PathBuf,
        stores: StoreRoot,
    }

    impl Fixture {
        fn new(project_is_home: bool) -> Self {
            let base = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("cortex-relocation-{}", uuid::Uuid::new_v4()));
            locks::private_directory(&base, true).unwrap();
            let project = if project_is_home {
                base.clone()
            } else {
                base.join("project")
            };
            locks::private_directory(&project, true).unwrap();
            let state = base.join(".local/state");
            let stores = StoreRoot::new(&state, StoreChannel::Dev).unwrap();
            let key = ProjectKey::new(&project, StoreChannel::Dev).unwrap();
            let legacy = Store::open(key.clone(), OpenMode::Initialize).unwrap();
            legacy.write(|tx| {
                tx.execute("INSERT INTO tasks VALUES ('t_moved',?, 'test')", [string(&project)?])?;
                tx.execute_batch("CREATE TABLE test_witness (value TEXT); INSERT INTO test_witness VALUES ('alpha');")?;
                Ok(())
            }).unwrap();
            let directory = key.directory().join(".source_blocks_staging");
            locks::private_directory(&directory, true).unwrap();
            create(&directory.join("uncatalogued"))
                .unwrap()
                .write_all(b"body")
                .unwrap();
            Self {
                base,
                project,
                stores,
            }
        }

        fn key(&self) -> ProjectKey {
            self.stores.project_key(&self.project).unwrap()
        }

        fn open(&self) -> CoreResult<Store> {
            Store::open(self.key(), OpenMode::Initialize)
        }

        fn crash_at(&self, point: &str) {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::migration::relocation::tests::relocation_child",
                ])
                .env("CORTEX_RELOCATION_FIXTURE", &self.base)
                .env("CORTEX_CORE_MIGRATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(97), "{point}");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    #[test]
    fn relocation_child() {
        let Some(base) = std::env::var_os("CORTEX_RELOCATION_FIXTURE").map(PathBuf::from) else {
            return;
        };
        let stores = StoreRoot::new(&base.join(".local/state"), StoreChannel::Dev).unwrap();
        let key = stores.project_key(base.join("project")).unwrap();
        drop(Store::open(key, OpenMode::Initialize).unwrap());
        panic!("relocation checkpoint did not run");
    }

    #[test]
    fn relocation_resumes_before_and_after_install_link_and_anchor_rename() {
        for point in [
            "relocate_before_install",
            "relocate_after_install",
            "relocate_before_link",
            "relocate_after_link",
            "relocate_before_anchor_rename",
            "relocate_after_anchor_rename",
        ] {
            let fixture = Fixture::new(false);
            fixture.crash_at(point);
            let store = fixture
                .open()
                .unwrap_or_else(|error| panic!("{point}: {error:?}"));
            assert_eq!(
                store.key().directory(),
                fixture.key().home_directory().unwrap(),
                "{point}"
            );
            let binding = location::read_anchor(&fixture.key().anchor_directory().join(DATABASE))
                .unwrap()
                .unwrap();
            let original = fixture
                .key()
                .anchor_directory()
                .join(BACKUPS)
                .join(format!("relocated-{}", binding.store_id));
            for root in [original, store.key().directory()] {
                assert_eq!(
                    fs::read(root.join(".source_blocks_staging/uncatalogued")).unwrap(),
                    b"body",
                    "{point}"
                );
                verify(&sql(&root.join(DATABASE), true).unwrap(), 12).unwrap();
            }
            assert!(!store.key().directory().join(INTENT).exists(), "{point}");
        }
    }

    #[test]
    fn relocation_rejects_old_wal_row_changes_and_corrupt_staging() {
        for corrupt_copy in [false, true] {
            let fixture = Fixture::new(false);
            fixture.crash_at("relocate_after_install");
            let home = fixture.key().home_directory().unwrap();
            let old = fixture.key().anchor_directory();
            let authoritative = Store::open(fixture.key(), OpenMode::LiveReadOnly).unwrap();
            assert_eq!(authoritative.key().directory(), old);
            drop(authoritative);
            // Keep the old connection alive, so its committed in-place change is
            // still in WAL during the attempt and cannot masquerade as the snapshot.
            let writer = sql(&old.join(DATABASE), false).unwrap();
            if corrupt_copy {
                fs::write(home.join(".source_blocks_staging/uncatalogued"), b"fake").unwrap();
            } else {
                writer
                    .execute("UPDATE test_witness SET value='omega'", [])
                    .unwrap();
                assert!(old.join("cortex.sqlite3-wal").metadata().unwrap().len() > 0);
            }
            assert_eq!(
                fixture.open().err().unwrap().code(),
                "store_relocation_conflict"
            );
            assert!(location::read_anchor(&old.join(DATABASE))
                .unwrap()
                .is_none());
            assert!(!home.exists());
            let quarantined = home.parent().unwrap().join(BACKUPS);
            assert_eq!(fs::read_dir(&quarantined).unwrap().count(), 1);
            drop(writer);
            drop(fixture.open().unwrap());
        }
    }

    #[test]
    fn relocation_refuses_an_unwitnessed_hardlink() {
        let fixture = Fixture::new(false);
        let anchor = fixture.key().anchor_directory().join(DATABASE);
        let stray = fixture.base.join("stray-link");
        fs::hard_link(&anchor, &stray).unwrap();
        for mode in [OpenMode::Initialize, OpenMode::LiveReadOnly] {
            assert_eq!(
                Store::open(fixture.key(), mode).err().unwrap().code(),
                "unsafe_storage"
            );
        }
        assert!(!fixture.key().home_directory().unwrap().exists());
        fs::remove_file(stray).unwrap();
        drop(fixture.open().unwrap());
    }

    #[test]
    fn relocation_quarantines_copy_when_switch_checkpoint_is_busy() {
        let fixture = Fixture::new(false);
        fixture.crash_at("relocate_after_install");
        let old = fixture.key().anchor_directory();
        let home = fixture.key().home_directory().unwrap();
        let mut db = sql(&old.join(DATABASE), false).unwrap();
        db.execute("UPDATE test_witness SET value='omega'", [])
            .unwrap();
        let observer = db.transaction().unwrap();
        assert_eq!(
            observer
                .query_row("SELECT value FROM test_witness", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "omega"
        );
        assert_eq!(fixture.open().err().unwrap().code(), "busy");
        assert!(!home.exists());
        assert_eq!(
            fs::read_dir(home.parent().unwrap().join(BACKUPS))
                .unwrap()
                .count(),
            1
        );
        drop(observer);
        drop(db);
        drop(fixture.open().unwrap());
    }

    #[test]
    fn relocation_copies_self_contained_workspaces_and_refuses_external_git_admin() {
        let fixture = Fixture::new(false);
        let old = fixture.key().anchor_directory();
        let checkout = old.join("workspaces/snapshot");
        let base = old.join("workspaces/base");
        let candidate = old.join("candidates/staged/worker");
        fs::create_dir_all(&checkout).unwrap();
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        let external = fixture.base.join("external-git-admin");
        fs::create_dir(&external).unwrap();
        let git = checkout.join(".git");
        fs::write(&git, format!("gitdir: {}\n", external.display())).unwrap();
        assert_eq!(
            fixture.open().err().unwrap().code(),
            "store_relocation_linked_workspace"
        );
        assert!(location::read_anchor(&old.join(DATABASE))
            .unwrap()
            .is_none());
        let admin = checkout.join(".git-meta");
        fs::create_dir(&admin).unwrap();
        fs::create_dir(admin.join("objects")).unwrap();
        fs::create_dir(admin.join("refs")).unwrap();
        fs::write(&git, "gitdir: .git-meta\n").unwrap();
        fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            admin.join("config"),
            "[core]\nrepositoryformatversion = 0\n",
        )
        .unwrap();
        fs::write(candidate.join("draft.txt"), "staged candidate").unwrap();
        let git_root = |path: &Path| {
            let output = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["rev-parse", "--show-toplevel"])
                .env("HOME", &fixture.base)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(output.status.success());
            PathBuf::from(String::from_utf8(output.stdout).unwrap().trim_end())
        };
        assert_eq!(git_root(&checkout), checkout);
        let legacy = sql(&old.join(DATABASE), false).unwrap();
        let draft_folder = old.join("draft-reports");
        locks::private_directory(&draft_folder, true).unwrap();
        let draft = draft_folder.join("d_pending.md");
        create(&draft)
            .unwrap()
            .write_all(b"pending source")
            .unwrap();
        let meta = draft.symlink_metadata().unwrap();
        let hash = digest(&draft).unwrap();
        legacy.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_digest)
             VALUES('d_pending','t_moved','owner','report','general',?,'test',?,?,?)",
            (string(&draft).unwrap(), meta.dev() as i64, meta.ino() as i64, &hash),
        ).unwrap();
        legacy
            .execute(
                "INSERT INTO pending_source_deletions VALUES('t_moved','d_pending',?,?)",
                (string(&draft).unwrap(), &hash),
            )
            .unwrap();
        legacy.execute_batch(
            "CREATE TABLE candidate_staging (task_id TEXT NOT NULL REFERENCES tasks(id), stage_path TEXT NOT NULL);"
        ).unwrap();
        legacy
            .execute(
                "INSERT INTO candidate_staging VALUES ('t_moved',?)",
                [string(&candidate).unwrap()],
            )
            .unwrap();
        legacy.execute(
            "INSERT INTO workspace_snapshots(
             workspace_binding_id,task_id,attempt_id,project_root,root_path,base_path,mode,manifest_json,base_kind,created_at)
             VALUES('legacy-snapshot','t_moved','worker',?,?,?,'test','{}','test','test')",
            (string(&fixture.project).unwrap(), string(&checkout).unwrap(), string(&base).unwrap()),
        ).unwrap();
        let census = |db: &Connection| -> CoreResult<Vec<(i64, i64)>> {
            [
                "tasks",
                "workspace_snapshots",
                "candidate_staging",
                "drafts",
                "pending_source_deletions",
            ]
            .iter()
            .map(|table| {
                Ok(db.query_row(
                    &format!("SELECT count(*),max(rowid) FROM {table}"),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .collect()
        };
        let original = census(&legacy).unwrap();
        drop(legacy);
        let store = fixture.open().unwrap();
        let home = store.key().directory();
        store.read(|db| {
            assert_eq!(census(db)?, original);
            let paths = db.query_row(
                "SELECT root_path,base_path,stage_path FROM workspace_snapshots,candidate_staging", [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            )?;
            assert_eq!(paths, (
                string(&home.join("workspaces/snapshot"))?.into(),
                string(&home.join("workspaces/base"))?.into(),
                string(&home.join("candidates/staged/worker"))?.into(),
            ));
            let (draft_path, deletion_path): (String, String) = db.query_row(
                "SELECT drafts.path,pending_source_deletions.path
                 FROM drafts JOIN pending_source_deletions ON drafts.id=pending_source_deletions.draft_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(draft_path, deletion_path);
            assert_eq!(draft_path, string(&home.join("draft-reports/d_pending.md"))?);
            verify(db, 12)
        }).unwrap();
        assert_eq!(
            fs::read(home.join("workspaces/snapshot/.git")).unwrap(),
            b"gitdir: .git-meta\n"
        );
        assert_eq!(
            git_root(&home.join("workspaces/snapshot")),
            home.join("workspaces/snapshot")
        );
        assert_eq!(
            fs::read(home.join("candidates/staged/worker/draft.txt")).unwrap(),
            b"staged candidate"
        );
    }

    #[test]
    fn relocation_respects_writer_fence_but_replaces_idle_reader() {
        let fixture = Fixture::new(false);
        let key = fixture.key();
        let old = key.anchor_directory();
        let reader = Store::open(key.clone(), OpenMode::LiveReadOnly).unwrap();
        let writer = locks::AccessLock::acquire(
            &old.join(".connections.lock"),
            false,
            false,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(fixture.open().err().unwrap().code(), "busy");
        assert!(!key.home_directory().unwrap().exists());
        drop(writer);
        drop(fixture.open().unwrap());
        assert_eq!(
            reader.read(|_| Ok(())).unwrap_err().code(),
            "storage_replaced"
        );
    }

    #[test]
    fn relocation_lock_child() {
        let Some(project) = std::env::var_os("CORTEX_RELOCATION_LOCK_PROJECT").map(PathBuf::from)
        else {
            return;
        };
        let writer = std::env::var_os("CORTEX_RELOCATION_LOCK_WRITER").unwrap() == "1";
        let mode = if writer {
            OpenMode::RuntimeValidate
        } else {
            OpenMode::LiveReadOnly
        };
        let key = ProjectKey::new(&project, StoreChannel::Dev).unwrap();
        let handle = Store::open(key, mode).unwrap();
        fs::write(project.join("ready"), b"ready").unwrap();
        let start = std::time::Instant::now();
        while !project.join("release").exists() {
            assert!(start.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(20));
        }
        if !writer {
            assert_eq!(
                handle.read(|_| Ok(())).unwrap_err().code(),
                "storage_replaced"
            );
        }
    }

    #[test]
    fn relocation_fences_writer_and_replaces_reader_in_other_process() {
        for writer in [true, false] {
            let fixture = Fixture::new(false);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::migration::relocation::tests::relocation_lock_child",
                ])
                .env("CORTEX_RELOCATION_LOCK_PROJECT", &fixture.project)
                .env(
                    "CORTEX_RELOCATION_LOCK_WRITER",
                    if writer { "1" } else { "0" },
                )
                .spawn()
                .unwrap();
            let start = std::time::Instant::now();
            while !fixture.project.join("ready").exists() {
                assert!(child.try_wait().unwrap().is_none());
                assert!(start.elapsed() < Duration::from_secs(30));
                std::thread::sleep(Duration::from_millis(20));
            }
            if writer {
                assert_eq!(fixture.open().err().unwrap().code(), "busy");
                assert!(!fixture.key().home_directory().unwrap().exists());
            } else {
                drop(fixture.open().unwrap());
            }
            fs::write(fixture.project.join("release"), b"release").unwrap();
            assert!(child.wait().unwrap().success());
        }
    }

    #[test]
    fn relocation_refreshes_draft_inode_and_replay_but_preserves_project_markers() {
        use crate::store::documents::Publisher;

        let fixture = Fixture::new(false);
        let legacy = ProjectKey::new(&fixture.project, StoreChannel::Dev).unwrap();
        let source = legacy.directory();
        create(&source.join("owner.json")).unwrap();
        create(&source.join("legacy-workspace.json")).unwrap();
        let old = Store::open(legacy, OpenMode::Initialize).unwrap();
        let draft = Publisher::new(&old)
            .create_draft("t_moved", "owner", "general", "draft-request")
            .unwrap();
        drop(old);
        let store = fixture.open().unwrap();
        let replay = Publisher::new(&store)
            .create_draft("t_moved", "owner", "general", "draft-request")
            .unwrap();
        assert_eq!(replay.draft_id, draft.draft_id);
        assert_eq!(replay.markdown, draft.markdown);
        assert!(!draft.draft_path.exists());
        assert!(replay.draft_path.starts_with(store.key().directory()));
        let metadata = replay.draft_path.symlink_metadata().unwrap();
        store
            .read(|db| {
                let identity = db.query_row(
                    "SELECT device,inode FROM drafts WHERE id=?",
                    [&replay.draft_id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )?;
                assert_eq!(identity, (metadata.dev() as i64, metadata.ino() as i64));
                Ok(())
            })
            .unwrap();
        assert!(source.join("owner.json").is_file());
        assert!(source.join("legacy-workspace.json").is_file());
        assert!(!store.key().directory().join("owner.json").exists());
    }

    #[test]
    fn relocation_upgrades_format11_in_project_home_and_retains_both_backups() {
        let fixture = Fixture::new(true);
        let old = fixture.key().anchor_directory();
        let db = sql(&old.join(DATABASE), false).unwrap();
        db.pragma_update(None, "user_version", 11).unwrap();
        drop(db);
        let store = fixture.open().unwrap();
        store
            .read(|db| {
                verify(db, 12)?;
                assert_eq!(
                    db.query_row("SELECT count(*) FROM format_history", [], |r| r
                        .get::<_, i64>(0))?,
                    2
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(fs::read_dir(old.join(BACKUPS)).unwrap().count(), 2);
    }

    #[test]
    fn relocation_admin_confirms_fences_and_recovers_offline_anchor() {
        let fixture = Fixture::new(false);
        drop(fixture.open().unwrap());
        let list = administration::list(&fixture.stores).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].root_status, RootStatus::Present);
        assert!(list[0].originals_retained);
        let id = &list[0].store_id;
        let remove = |action, confirm| administration::remove(&fixture.stores, id, action, confirm);
        assert_eq!(
            remove(Removal::Originals, false).unwrap_err().code(),
            "project_store_removal_confirmation_required"
        );
        let store = fixture.open().unwrap();
        assert_eq!(remove(Removal::Database, true).unwrap_err().code(), "busy");
        drop(store);
        remove(Removal::Originals, true).unwrap();
        assert!(!administration::list(&fixture.stores).unwrap()[0].originals_retained);
        let offline = fixture.base.join("disconnected");
        fs::rename(&fixture.project, &offline).unwrap();
        assert_eq!(
            administration::list(&fixture.stores).unwrap()[0].root_status,
            RootStatus::Missing
        );
        locks::private_directory(&fixture.project, true).unwrap();
        assert_eq!(
            administration::list(&fixture.stores).unwrap()[0].root_status,
            RootStatus::Unavailable
        );
        fs::remove_dir(&fixture.project).unwrap();
        remove(Removal::Database, true).unwrap();
        assert!(administration::list(&fixture.stores).unwrap().is_empty());
        fs::rename(offline, &fixture.project).unwrap();
        assert_eq!(fixture.open().err().unwrap().code(), "store_root_mismatch");
        remove(Removal::Database, true).unwrap();
        drop(fixture.open().unwrap());
        assert_ne!(
            administration::list(&fixture.stores).unwrap()[0].store_id,
            *id
        );
    }

    #[test]
    fn relocation_admin_retry_never_removes_a_recreated_store() {
        let fixture = Fixture::new(false);
        drop(fixture.open().unwrap());
        let old = administration::list(&fixture.stores)
            .unwrap()
            .remove(0)
            .store_id;
        let offline = fixture.base.join("disconnected");
        fs::rename(&fixture.project, &offline).unwrap();
        administration::remove(&fixture.stores, &old, Removal::Database, true).unwrap();
        fs::rename(offline, &fixture.project).unwrap();
        // Simulate the owner clearing the old anchor and opening a new store
        // before retrying the outstanding operation.
        fs::remove_file(fixture.key().anchor_directory().join(DATABASE)).unwrap();
        drop(fixture.open().unwrap());
        let new = administration::list(&fixture.stores)
            .unwrap()
            .remove(0)
            .store_id;
        assert_ne!(old, new);
        administration::remove(&fixture.stores, &old, Removal::Database, true).unwrap();
        drop(fixture.open().unwrap());
        assert_eq!(
            administration::list(&fixture.stores).unwrap()[0].store_id,
            new
        );
    }
}
