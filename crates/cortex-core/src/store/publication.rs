//! Private fsync/intent/atomic-replace foundation. SQLite and the filesystem are
//! NOT one transaction. Keep the intent until DB publication is confirmed; recovery
//! checks editions before retaining a replacement or restoring its verified backup.
use super::locks;
use crate::{error::refuse, CoreResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{Metadata, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};

/// Separator between a newer pipeline edition and the file it is prepended to.
const SEPARATOR: &[u8] = b"\n\n---\n\n";
/// A digest verified by a reader is trusted only when the file last changed at least
/// this long before the check (a write inside one timestamp tick could otherwise go unseen).
const RACY_WINDOW: Duration = Duration::from_millis(20);

pub(super) fn checkpoint(_point: &str) {
    #[cfg(test)]
    if std::env::var("CORTEX_CORE_PUBLICATION_CRASH_POINT").as_deref() == Ok(_point) {
        std::process::exit(98);
    }
}

pub(super) fn scratch_name(name: &str) -> bool {
    [
        (".pending_", ".tmp"),
        (".publication_staging_", ".tmp"),
        (".backup_pipeline_", ".md"),
    ]
    .into_iter()
    .any(|(prefix, suffix)| {
        name.strip_prefix(prefix)
            .and_then(|tail| tail.strip_suffix(suffix))
            .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    })
}

/// What identifies one exact state of a regular file without reading it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Identity {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Identity {
    pub(crate) fn of(m: &Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
    fn changed_before(&self, moment: SystemTime) -> bool {
        let ctime = SystemTime::UNIX_EPOCH
            + Duration::new(
                self.ctime.0.max(0) as u64,
                self.ctime.1.clamp(0, 999_999_999) as u32,
            );
        ctime + RACY_WINDOW <= moment
    }
}
/// A file digest remembered by this process. A pipeline edition is hashed when it is
/// written (or once when first read); later reads and publications of the same unchanged
/// file (same inode, size, mtime and ctime) reuse it instead of re-reading the whole
/// file. Any change of the file changes its identity and forces a real hash.
struct Verified {
    identity: Identity,
    digest: String,
    characters: Option<usize>,
    /// Hashed from the bytes this process wrote, rather than verified by reading.
    written: bool,
    at: SystemTime,
}
static VERIFIED: Mutex<Option<HashMap<PathBuf, Verified>>> = Mutex::new(None);
/// The remembered digest (and Unicode character count, when known) of `path` in exactly
/// the state `identity`, if it was verified.
pub(crate) fn verified(path: &Path, identity: &Identity) -> Option<(String, Option<usize>)> {
    let guard = VERIFIED.lock().ok()?;
    let entry = guard.as_ref()?.get(path)?;
    (entry.identity == *identity && (entry.written || identity.changed_before(entry.at)))
        .then(|| (entry.digest.clone(), entry.characters))
}
pub(crate) fn remember(
    path: &Path,
    identity: Identity,
    digest: String,
    characters: Option<usize>,
    written: bool,
) {
    if let Ok(mut guard) = VERIFIED.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= 64 {
            map.clear();
        }
        map.insert(
            path.to_path_buf(),
            Verified {
                identity,
                digest,
                characters,
                written,
                at: SystemTime::now(),
            },
        );
    }
}
/// sha256 of the file at `path`; an unchanged, already verified file is not read again.
pub(crate) fn file_digest(path: &Path) -> CoreResult<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = locks::open_private(path, false, true)?;
    let before = Identity::of(&f.metadata()?);
    if let Some((digest, _)) = verified(path, &before) {
        if Identity::of(&path.symlink_metadata()?) == before {
            return Ok(digest);
        }
    }
    let mut hash = Sha256::new();
    let mut chunk = vec![0u8; 1 << 18];
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    let digest = format!("{:x}", hash.finalize());
    if Identity::of(&f.metadata()?) == before && Identity::of(&path.symlink_metadata()?) == before {
        remember(path, before, digest.clone(), None, false);
    }
    Ok(digest)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    report_id: String,
    target: String,
    staged: String,
    backup: Option<String>,
    old_digest: Option<String>,
    new_digest: String,
    /// In-process hints from `prepare` (never journaled): the exact states the staged file
    /// and the replaced file were verified in, so `install` need not read them again.
    #[serde(skip)]
    staged_identity: Option<Identity>,
    #[serde(skip)]
    old_identity: Option<Identity>,
    #[serde(skip)]
    characters: Option<usize>,
}
fn name(s: &str) -> CoreResult<()> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return refuse("unsafe_publication_path");
    }
    Ok(())
}
/// Create `path` exclusively with the concatenated `parts`; returns the synced file's identity.
fn write_new(path: &Path, parts: &[&[u8]]) -> CoreResult<Identity> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    for part in parts {
        f.write_all(part)?;
    }
    f.sync_all()?;
    Ok(Identity::of(&f.metadata()?))
}
fn read(path: &Path) -> CoreResult<Vec<u8>> {
    use std::io::Read;
    let mut f = locks::open_private(path, false, true)?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}
impl PublicationIntent {
    /// New immutable reports require old_digest=None; only pipeline.md can replace.
    pub fn prepare(
        directory: &Path,
        report_id: &str,
        target: &str,
        newest: &str,
        old_digest: Option<&str>,
    ) -> CoreResult<(Self, PathBuf)> {
        use sha2::{Digest, Sha256};
        locks::private_directory(directory, false)?;
        name(report_id)?;
        name(target)?;
        if old_digest.is_some() && target != "pipeline.md" {
            return refuse("immutable_report");
        }
        let target_path = directory.join(target);
        let mut old_identity = None;
        let mut old_characters = None;
        let old = if let Some(digest) = old_digest {
            use std::io::Read;
            let mut f = locks::open_private(&target_path, false, true)?;
            let before = Identity::of(&f.metadata()?);
            let mut bytes = Vec::with_capacity(before.len as usize);
            f.read_to_end(&mut bytes)?;
            // A file already verified in this exact state is not hashed again.
            match verified(&target_path, &before) {
                Some((d, characters)) if d == digest => old_characters = characters,
                _ => {
                    if crate::digest(&bytes) != digest {
                        return refuse("file_conflict");
                    }
                }
            }
            if Identity::of(&f.metadata()?) != before
                || Identity::of(&target_path.symlink_metadata()?) != before
            {
                return refuse("file_conflict");
            }
            old_identity = Some(before);
            Some(bytes)
        } else {
            if target_path.symlink_metadata().is_ok() {
                return refuse("immutable_report");
            }
            None
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut parts: Vec<&[u8]> = vec![newest.as_bytes()];
        if let Some(ref old) = old {
            parts.push(SEPARATOR);
            parts.push(old);
        }
        let mut hash = Sha256::new();
        for part in &parts {
            hash.update(part);
        }
        let new_digest = format!("{:x}", hash.finalize());
        let characters = match (old.is_some(), old_characters) {
            (false, _) => Some(newest.chars().count()),
            (true, Some(c)) => Some(newest.chars().count() + SEPARATOR.len() + c),
            (true, None) => None,
        };
        let staged = format!(".pending_{id}.tmp");
        // The previous file is kept by a hard link made at install time (no second copy).
        let backup = old.as_ref().map(|_| format!(".backup_pipeline_{id}.md"));
        checkpoint("before_staging");
        let staged_identity = write_new(&directory.join(&staged), &parts)?;
        checkpoint("after_staging");
        let intent = Self {
            report_id: report_id.into(),
            target: target.into(),
            staged,
            backup,
            old_digest: old_digest.map(str::to_owned),
            new_digest,
            staged_identity: Some(staged_identity),
            old_identity,
            characters,
        };
        let journal = directory.join(format!(".publication_{id}.json"));
        let temporary_journal = directory.join(format!(".publication_staging_{id}.tmp"));
        write_new(&temporary_journal, &[&serde_json::to_vec(&intent)?])?;
        checkpoint("after_intent_write");
        std::fs::rename(&temporary_journal, &journal)?;
        checkpoint("after_intent_rename");
        locks::sync_directory(directory)?;
        Ok((intent, journal))
    }
    pub fn digest(&self) -> &str {
        &self.new_digest
    }
    pub fn install(&self, directory: &Path) -> CoreResult<()> {
        let staged = directory.join(&self.staged);
        let target = directory.join(&self.target);
        let staged_as_prepared = match &self.staged_identity {
            Some(identity) => Identity::of(&staged.symlink_metadata()?) == *identity,
            None => false,
        };
        if !staged_as_prepared && crate::digest(&read(&staged)?) != self.new_digest {
            return refuse("publication_staging_conflict");
        }
        if let Some(ref old) = self.old_digest {
            let target_as_prepared = match &self.old_identity {
                Some(identity) => Identity::of(&target.symlink_metadata()?) == *identity,
                None => false,
            };
            if !target_as_prepared && file_digest(&target)? != *old {
                return refuse("file_conflict");
            }
        } else if target.symlink_metadata().is_ok() {
            return refuse("immutable_report");
        }
        if let Some(ref backup) = self.backup {
            let backup = directory.join(backup);
            if std::fs::hard_link(&target, &backup).is_err() {
                // No hard links on this filesystem: keep a real copy.
                write_new(&backup, &[&read(&target)?])?;
            }
        }
        checkpoint("after_backup");
        std::fs::rename(&staged, &target)?;
        checkpoint("after_replace");
        locks::sync_directory(directory)?;
        if let Ok(meta) = target.symlink_metadata() {
            remember(
                &target,
                Identity::of(&meta),
                self.new_digest.clone(),
                self.characters,
                true,
            );
        }
        Ok(())
    }
    /// Existing journal only. No "probably committed" heuristic and no automatic republish.
    pub fn reconcile(
        directory: &Path,
        journal: &Path,
        db: &rusqlite::Connection,
    ) -> CoreResult<()> {
        Self::reconcile_with_cleanup(directory, journal, db, true)
    }

    /// A durable installation and its committed row require no additional fsync
    /// for removing private scratch files. If cleanup survives only in part,
    /// the retained intent or an orphan backup is cleaned up by the next read.
    pub(super) fn reconcile_completed(
        directory: &Path,
        journal: &Path,
        db: &rusqlite::Connection,
    ) -> CoreResult<()> {
        Self::reconcile_with_cleanup(directory, journal, db, false)
    }

    fn reconcile_with_cleanup(
        directory: &Path,
        journal: &Path,
        db: &rusqlite::Connection,
        sync_cleanup: bool,
    ) -> CoreResult<()> {
        if journal.parent() != Some(directory) {
            return refuse("unsafe_publication_path");
        }
        let intent: Self = serde_json::from_slice(&read(journal)?)?;
        for s in [&intent.report_id, &intent.target, &intent.staged] {
            name(s)?;
        }
        if let Some(ref s) = intent.backup {
            name(s)?;
        }
        let committed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM editions e JOIN reports r ON r.id=e.report_id WHERE e.report_id=? AND e.digest=? AND r.filename=?)",
            (&intent.report_id, &intent.new_digest, &intent.target),
            |r| r.get(0),
        )?;
        let target = directory.join(&intent.target);
        // An install interrupted after its backup hard link but before the replacing
        // rename leaves the previous file under both names: drop the extra link.
        if let Some(ref backup) = intent.backup {
            let backup = directory.join(backup);
            if let (Ok(b), Ok(t)) = (backup.symlink_metadata(), target.symlink_metadata()) {
                if b.dev() == t.dev() && b.ino() == t.ino() {
                    std::fs::remove_file(&backup)?;
                }
            }
        }
        let current = if target.try_exists()? {
            Some(file_digest(&target)?)
        } else {
            None
        };
        if committed {
            if current.as_deref() != Some(&intent.new_digest) {
                return refuse("publication_integrity_failed");
            }
        } else if current.as_deref() == Some(&intent.new_digest) {
            if let (Some(backup), Some(old)) = (&intent.backup, &intent.old_digest) {
                if crate::digest(&read(&directory.join(backup))?) != *old {
                    return refuse("publication_backup_conflict");
                }
                std::fs::rename(directory.join(backup), &target)?;
            } else {
                std::fs::remove_file(&target)?;
            }
        } else if current != intent.old_digest {
            return refuse("publication_conflict");
        }
        for file in intent.backup.iter().chain(std::iter::once(&intent.staged)) {
            let path = directory.join(file);
            if path.try_exists()? {
                locks::regular(&path)?;
                std::fs::remove_file(path)?;
            }
        }
        checkpoint("during_cleanup");
        std::fs::remove_file(journal)?;
        if sync_cleanup || !committed {
            locks::sync_directory(directory)?;
        }
        Ok(())
    }
}
