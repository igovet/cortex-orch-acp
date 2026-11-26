//! Cooperative access fences, not a hostile-code boundary.
use crate::{error::refuse, CoreResult};
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    time::{Duration, Instant},
};
/// A private folder this account owns, created when `create` and missing. Several
/// threads or processes may create the same shared folder at once: whoever loses
/// that race gets the folder the winner made, after the same checks as any folder
/// that already existed (never an `AlreadyExists` failure, never an unchecked one).
pub fn private_directory(path: &Path, create: bool) -> CoreResult<()> {
    if create {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let m = path.symlink_metadata()?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::getuid() }
        || m.permissions().mode() & 0o077 != 0
        || path.canonicalize()? != path
    {
        return refuse("unsafe_storage");
    }
    Ok(())
}
pub fn regular(path: &Path) -> CoreResult<()> {
    let m = path.symlink_metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.nlink() != 1
        || m.permissions().mode() & 0o077 != 0
    {
        return refuse("unsafe_storage");
    }
    Ok(())
}
/// Create a missing private SQLite database file without opening an existing one.
/// Closing any descriptor of a file drops every POSIX lock this process holds on it,
/// including those of live SQLite connections; another process closing its own
/// connection would then delete the WAL and SHM those connections still use.
pub fn create_private_database(path: &Path) -> CoreResult<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        // A file that did not exist has no connections whose locks this close could drop.
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    regular(path)
}
pub fn open_private(path: &Path, create: bool, read_only: bool) -> CoreResult<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(!read_only)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    regular(path)?;
    let m = f.metadata()?;
    let current = path.symlink_metadata()?;
    if m.dev() != current.dev() || m.ino() != current.ino() {
        return refuse("unsafe_storage");
    }
    Ok(f)
}
pub struct AccessLock(File);
impl AccessLock {
    pub fn downgrade(&self) -> CoreResult<()> {
        if unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
            return refuse("busy");
        }
        Ok(())
    }
    pub fn acquire(path: &Path, create: bool, exclusive: bool, wait: Duration) -> CoreResult<Self> {
        let f = open_private(path, create, !create)?;
        let started = Instant::now();
        let mut pause = Duration::from_millis(10);
        loop {
            // SAFETY: f owns a valid FD; flock does not retain Rust references.
            if unsafe {
                libc::flock(
                    f.as_raw_fd(),
                    (if exclusive {
                        libc::LOCK_EX
                    } else {
                        libc::LOCK_SH
                    }) | libc::LOCK_NB,
                )
            } == 0
            {
                return Ok(Self(f));
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) || started.elapsed() >= wait {
                return refuse("busy");
            }
            // Short waits stay responsive; a long migration is polled gently.
            pause = (pause * 2).min(Duration::from_millis(250));
            std::thread::sleep(pause);
        }
    }
}
impl Drop for AccessLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub fn sync_directory(path: &Path) -> CoreResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::private_directory;
    use std::{
        os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
        path::PathBuf,
        sync::{Arc, Barrier},
    };
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cc-locks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        root.canonicalize().unwrap()
    }
    /// Callers that create the same shared folder at once (one catalogue refresh thread per
    /// connection) must all get it: losing the race to create it is not a failure.
    #[test]
    fn concurrent_creators_of_one_private_folder_all_succeed() {
        let root = scratch("race");
        for round in 0..200 {
            let path = root.join(format!("shared-{round}"));
            let gate = Arc::new(Barrier::new(8));
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let (path, gate) = (path.clone(), gate.clone());
                    std::thread::spawn(move || {
                        gate.wait();
                        private_directory(&path, true).map_err(|e| e.to_string())
                    })
                })
                .collect();
            for t in threads {
                assert_eq!(t.join().unwrap(), Ok(()), "round {round}");
            }
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
    /// Accepting a folder someone else created never accepts an unsafe one.
    #[test]
    fn an_existing_unsafe_path_is_still_refused_when_creating() {
        let root = scratch("unsafe");
        let open = root.join("open");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&open)
            .unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(&open, true).is_err());
        let link = root.join("link");
        symlink(&root, &link).unwrap();
        assert!(private_directory(&link, true).is_err());
        let dangling = root.join("dangling");
        symlink(root.join("missing"), &dangling).unwrap();
        assert!(private_directory(&dangling, true).is_err());
        assert!(!root.join("missing").exists());
        let file = root.join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(private_directory(&file, true).is_err());
        assert!(private_directory(&root.join("absent"), false).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
