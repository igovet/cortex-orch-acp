//! Accidental cross-channel protection, not a boundary against same-user tampering.
use crate::local::{failure, uid};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const MARKER: &str = "owner.json";
const LEGACY: &str = ".cortex-dev-owner.json";
const STORE: &str = ".cortex-dev";
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Owner {
    channel: String,
    root: String,
    uid: u32,
    store: Option<(u64, u64)>,
}
fn refused() -> io::Error {
    failure("dev_project_refused: the dev store ownership changed; release data was not opened")
}
fn store(root: &Path) -> io::Result<Option<(u64, u64)>> {
    match fs::symlink_metadata(root.join(STORE)) {
        Ok(m) if m.is_dir() && m.uid() == uid() && m.mode() & 0o077 == 0 =>
            Ok(Some((m.dev(), m.ino()))),
        Ok(_) => Err(refused()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn read_owner(root: &Path, path: &Path) -> io::Result<Option<Owner>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && m.uid() == uid() && m.mode() & 0o077 == 0 && m.nlink() == 1 => (),
        Ok(_) => return Err(refused()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut bytes = Vec::new();
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?
        .take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(refused());
    }
    let value: Owner = serde_json::from_slice(&bytes).map_err(|_| refused())?;
    if value.channel != "dev" || value.root != root.to_string_lossy() || value.uid != uid() {
        return Err(refused());
    }
    Ok(Some(value))
}
fn marker(root: &Path) -> PathBuf {
    root.join(STORE).join(MARKER)
}
fn owner(root: &Path) -> io::Result<Option<Owner>> {
    // Validate the directory before traversing it, including read-only checks.
    store(root)?;
    let current = read_owner(root, &marker(root))?;
    let legacy = read_owner(root, &root.join(LEGACY))?;
    match (current, legacy) {
        (Some(a), Some(b)) if a != b && b.store.is_some() => Err(refused()),
        (Some(a), _) | (_, Some(a)) => Ok(Some(a)),
        _ => Ok(None),
    }
}
pub fn check(root: &Path) -> io::Result<()> {
    if !workspace_protocol::channel::current().is_dev() {
        return Ok(());
    }
    match (owner(root)?, store(root)?) {
        (None, None) => Ok(()),
        (Some(o), actual) if o.store == actual => Ok(()),
        _ => Err(refused()),
    }
}
fn write_new(path: &Path, value: &Owner) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW).open(path)?;
    file.write_all(&serde_json::to_vec(value).map_err(|_| refused())?)?;
    file.sync_all()
}
fn migrate_workspace_marker(root: &Path) -> io::Result<()> {
    let source = root.join(".cortex-workspace-dev.json");
    // Unknown, public, linked or foreign files are not ours to remove. No old database is opened.
    if let Ok(Some(value)) = read_owner(root, &source) {
        let target = root.join(STORE).join("legacy-workspace.json");
        if !target.exists() {
            write_new(&target, &value)?;
        } else if read_owner(root, &target)?.as_ref() != Some(&value) {
            return Err(refused());
        }
        // Recheck identity immediately before removal; preserve any conflicting file.
        if read_owner(root, &source)?.as_ref() != Some(&value) {
            return Err(refused());
        }
        fs::remove_file(source)?;
    }
    Ok(())
}
pub fn reserve(root: &Path) -> io::Result<()> {
    check(root)?;
    if !workspace_protocol::channel::current().is_dev() {
        return Ok(());
    }
    // The reservation itself now lives inside the private channel directory.
    // Creating it here binds the directory immediately, before any database is opened.
    if store(root)?.is_none() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(root.join(STORE))?;
    }
    let value = Owner {
        channel: "dev".into(), root: root.to_string_lossy().into_owned(),
        uid: uid(), store: store(root)?,
    };
    if read_owner(root, &marker(root))?.is_none() {
        write_new(&marker(root), &value)?;
    }
    // A legacy unbound reservation may have had store=None. Its identity was checked
    // before creation above; a conflicting existing store was already refused.
    if let Some(old) = read_owner(root, &root.join(LEGACY))? {
        if old.store.is_some() && old.store != value.store {
            return Err(refused());
        }
        fs::remove_file(root.join(LEGACY))?;
    }
    migrate_workspace_marker(root)
}
pub fn activate(root: &Path) -> io::Result<()> {
    reserve(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("workspace-markers-{}-{}",
                std::process::id(), std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }
    #[test]
    fn unbound_legacy_reservation_and_interrupted_migration_are_retryable() {
        let f = Fixture::new();
        let old = Owner { channel:"dev".into(), root:f.0.to_string_lossy().into_owned(),
                          uid:uid(), store:None };
        write_new(&f.0.join(LEGACY), &old).unwrap();
        reserve(&f.0).unwrap();
        assert!(marker(&f.0).is_file() && !f.0.join(LEGACY).exists());
        // A crash after the new owner was committed but before removing the old reservation.
        write_new(&f.0.join(LEGACY), &old).unwrap();
        reserve(&f.0).unwrap();
        assert!(!f.0.join(LEGACY).exists());
        check(&f.0).unwrap();
    }
    #[test]
    fn foreign_and_linked_workspace_markers_are_not_removed() {
        let f = Fixture::new();
        reserve(&f.0).unwrap();
        let path = f.0.join(".cortex-workspace-dev.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        reserve(&f.0).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        fs::remove_file(&path).unwrap();
        let target = f.0.join("foreign");
        fs::write(&target, "keep").unwrap();
        symlink(&target, &path).unwrap();
        reserve(&f.0).unwrap();
        assert!(path.is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert!(!f.0.join(STORE).join("legacy-workspace.json").exists());
    }
}

