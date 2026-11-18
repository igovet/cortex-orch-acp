//! Private state files: identity keys and JSON records.
//!
//! Every file this crate owns is a regular file, owned by the effective user, with no
//! group or other permission bits (0600). It is opened with `O_NOFOLLOW`, so a symlink
//! in its place is refused, and a file with any group/other bit is refused rather than
//! silently repaired. JSON records are replaced atomically: a fresh `create_new` temp
//! file in the same directory is written, fsynced and renamed over the record, then
//! the directory is fsynced.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use iroh::SecretKey;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::RemoteError;

/// Upper bound for any JSON state file this crate reads (1 MiB).
pub const STATE_FILE_BYTES: u64 = 1 << 20;

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn refused(path: &Path, problem: &str) -> RemoteError {
    RemoteError::InsecureState(format!("{}: {problem}", path.display()))
}

/// Ensures `dir` is a real directory owned by the effective user and not writable by
/// others, creating it (mode 0700) when absent. A symlink, a foreign owner or an
/// other-writable directory is refused. Group-writable is accepted because Android
/// app data directories are 0771 under the app's own group.
pub fn ensure_private_dir(dir: &Path) -> Result<(), RemoteError> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(refused(dir, "state directory is a symlink"));
    }
    if !meta.is_dir() {
        return Err(refused(dir, "state directory is not a directory"));
    }
    if meta.uid() != euid() {
        return Err(refused(dir, "state directory is owned by another user"));
    }
    if meta.mode() & 0o002 != 0 {
        return Err(refused(dir, "state directory is writable by other users"));
    }
    Ok(())
}

fn check_open_file(path: &Path, file: &File) -> Result<(), RemoteError> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(refused(path, "not a regular file"));
    }
    if meta.uid() != euid() {
        return Err(refused(path, "owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(refused(path, "group or other permission bits are set"));
    }
    Ok(())
}

/// Opens an existing private file for reading. `Ok(None)` when it does not exist.
fn open_private(path: &Path) -> Result<Option<File>, RemoteError> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => {
            check_open_file(path, &file)?;
            Ok(Some(file))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => Err(refused(path, "is a symlink")),
        Err(e) => Err(e.into()),
    }
}

fn create_private(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    // The umask can only remove bits; make the mode exact regardless.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Loads the identity key stored at `path`, generating and storing a new one on first
/// use. The file holds the 32 raw secret-key bytes. The key is never logged.
pub fn load_or_create_key(path: &Path) -> Result<SecretKey, RemoteError> {
    for _ in 0..2 {
        if let Some(mut file) = open_private(path)? {
            let mut bytes = Vec::with_capacity(33);
            (&mut file).take(33).read_to_end(&mut bytes)?;
            let bytes: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| refused(path, "identity key has the wrong length"))?;
            return Ok(SecretKey::from_bytes(&bytes));
        }
        let key = SecretKey::generate();
        match create_private(path) {
            Ok(mut file) => {
                file.write_all(&key.to_bytes())?;
                file.sync_all()?;
                if let Some(parent) = path.parent() {
                    sync_dir(parent)?;
                }
                return Ok(key);
            }
            // Another opener won the race; read what it wrote.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(refused(path, "is a symlink"))
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(refused(path, "identity key could not be created"))
}

/// Reads a private JSON record; `Ok(None)` when the file does not exist.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, RemoteError> {
    let Some(file) = open_private(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(STATE_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > STATE_FILE_BYTES {
        return Err(refused(path, "state file is too large"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| refused(path, "state file is not valid"))
}

fn temp_name(path: &Path) -> PathBuf {
    let mut random = [0u8; 8];
    crate::random_bytes(&mut random);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    path.with_file_name(format!(
        ".{name}.tmp-{}",
        data_encoding::HEXLOWER.encode(&random)
    ))
}

/// Atomically replaces the private JSON record at `path`.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), RemoteError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| RemoteError::InsecureState("state record could not be encoded".into()))?;
    if bytes.len() as u64 > STATE_FILE_BYTES {
        return Err(refused(path, "state record is too large"));
    }
    // Refuse to replace a symlink or a non-private file that someone put in place.
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(refused(path, "is a symlink"));
        }
        drop(open_private(path)?);
    }
    let temp = temp_name(path);
    let result = (|| -> io::Result<()> {
        let mut file = create_private(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}
