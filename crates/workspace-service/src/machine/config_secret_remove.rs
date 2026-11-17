//! Remove only the id-derived single-link owner file. Credential bytes are never opened.
use super::*;
use std::{
    ffi::CString,
    fs,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
};

pub(super) fn unlink(folder: &Path, record: &Value) -> Result<bool> {
    if record["credential_ref"]["kind"] != "secret_file" {
        return Ok(false);
    }
    let id = record["id"].as_str().unwrap();
    let parent = folder.join("secrets");
    if record["credential_ref"]["path"].as_str() != parent.join(id).to_str() {
        return Ok(false);
    }
    let directory = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&parent)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            return Err(refuse(
                "unsafe_storage",
                "Secret directory is not a safe owner directory.",
            ))
        }
    };
    let meta = directory.metadata()?;
    if meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        return Err(refuse(
            "unsafe_storage",
            "Secret directory is not a safe owner directory.",
        ));
    }
    let name = CString::new(id).unwrap();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(e.into())
        };
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_uid != unsafe { libc::getuid() }
        || stat.st_nlink != 1
    {
        return Ok(false);
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.sync_all().map_err(|_|refuse("machine_outcome_unknown",
        "Secret removal completed but durability is unknown; inspect connection settings before retrying."))?;
    Ok(true)
}
