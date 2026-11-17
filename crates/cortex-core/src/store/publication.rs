//! Private fsync/intent/atomic-replace foundation. SQLite and the filesystem are
//! NOT one transaction. Keep the intent until DB publication is confirmed; recovery
//! checks editions before retaining a replacement or restoring its verified backup.
use super::locks;
use crate::{error::refuse, CoreResult};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    report_id: String,
    target: String,
    staged: String,
    backup: Option<String>,
    old_digest: Option<String>,
    new_digest: String,
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
fn write_new(path: &Path, data: &[u8]) -> CoreResult<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
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
        locks::private_directory(directory, false)?;
        name(report_id)?;
        name(target)?;
        if old_digest.is_some() && target != "pipeline.md" {
            return refuse("immutable_report");
        }
        let old = if let Some(digest) = old_digest {
            let bytes = read(&directory.join(target))?;
            if crate::digest(&bytes) != digest {
                return refuse("file_conflict");
            }
            Some(bytes)
        } else {
            if directory.join(target).symlink_metadata().is_ok() {
                return refuse("immutable_report");
            }
            None
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut bytes = newest.as_bytes().to_vec();
        if let Some(ref old) = old {
            bytes.extend_from_slice(b"\n\n---\n\n");
            bytes.extend_from_slice(old);
        }
        let staged = format!(".pending_{id}.tmp");
        let backup = old.as_ref().map(|_| format!(".backup_pipeline_{id}.md"));
        write_new(&directory.join(&staged), &bytes)?;
        if let (Some(name), Some(old)) = (&backup, &old) {
            write_new(&directory.join(name), old)?;
        }
        let intent = Self {
            report_id: report_id.into(),
            target: target.into(),
            staged,
            backup,
            old_digest: old_digest.map(str::to_owned),
            new_digest: crate::digest(&bytes),
        };
        let journal = directory.join(format!(".publication_{id}.json"));
        write_new(&journal, &serde_json::to_vec(&intent)?)?;
        locks::sync_directory(directory)?;
        Ok((intent, journal))
    }
    pub fn digest(&self) -> &str {
        &self.new_digest
    }
    pub fn install(&self, directory: &Path) -> CoreResult<()> {
        if crate::digest(&read(&directory.join(&self.staged))?) != self.new_digest {
            return refuse("publication_staging_conflict");
        }
        if let Some(ref old) = self.old_digest {
            if crate::digest(&read(&directory.join(&self.target))?) != *old {
                return refuse("file_conflict");
            }
        } else if directory.join(&self.target).symlink_metadata().is_ok() {
            return refuse("immutable_report");
        }
        std::fs::rename(directory.join(&self.staged), directory.join(&self.target))?;
        locks::sync_directory(directory)
    }
    /// Existing journal only. No "probably committed" heuristic and no automatic republish.
    pub fn reconcile(
        directory: &Path,
        journal: &Path,
        db: &rusqlite::Connection,
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
        let current = if target.try_exists()? {
            Some(crate::digest(&read(&target)?))
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
        std::fs::remove_file(journal)?;
        locks::sync_directory(directory)
    }
}
