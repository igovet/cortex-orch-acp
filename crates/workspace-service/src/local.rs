use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

pub use workspace_protocol::FRAME_BYTES;
pub fn failure(code: &'static str) -> io::Error {
    io::Error::other(code)
}
pub fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// Reject every symbolic-link component, lexical escape and nonabsolute path.
pub fn canonical_directory(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(failure("absolute_path_required"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        match part {
            Component::RootDir | Component::Normal(_) => current.push(part),
            _ => return Err(failure("invalid_path")),
        }
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(failure("symlink_refused"));
        }
    }
    if !current.is_dir() {
        return Err(failure("directory_required"));
    }
    Ok(current)
}
pub fn private_directory(path: &Path) -> io::Result<PathBuf> {
    if !path.exists() {
        canonical_directory(path.parent().ok_or_else(|| failure("missing_parent"))?)?;
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        match builder.mode(0o700).create(path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        // A concurrent native lane may have created it; validate its owner/mode and
        // every path component below, rather than treating EEXIST as trusted success.
    }
    let path = canonical_directory(path)?;
    // Inspect and repair the opened directory itself, never a path that could be swapped.
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let meta = directory.metadata()?;
    let unsafe_directory = |problem| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            UnsafePrivateDirectory {
                path: path.clone(),
                problem,
            },
        )
    };
    if meta.uid() != uid() {
        return Err(unsafe_directory("owned by another account"));
    }
    let mode = meta.permissions().mode();
    if mode & 0o077 != 0 {
        // A folder this account owns whose extra bits never let another account write
        // into it (for example 0775 under the account's own group from a 002 umask, or
        // 0755) is made private in place. World-writable or another group's write
        // access may already have let someone plant content there, so it stays refused.
        if mode & 0o002 != 0 {
            return Err(unsafe_directory(
                "writable by every account on this machine",
            ));
        }
        if mode & 0o020 != 0 && meta.gid() != unsafe { libc::getegid() } {
            return Err(unsafe_directory("writable by another group"));
        }
        directory.set_permissions(fs::Permissions::from_mode(0o700))?;
        if directory.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_directory("could not be made private"));
        }
    }
    Ok(path)
}
/// One of this app's own private folders cannot be used safely. It displays as its
/// stable code; [`UnsafePrivateDirectory::owner_message`] names the folder and the
/// problem for the owner.
#[derive(Debug)]
pub struct UnsafePrivateDirectory {
    pub path: PathBuf,
    pub problem: &'static str,
}
impl std::fmt::Display for UnsafePrivateDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("private_directory_required")
    }
}
impl std::error::Error for UnsafePrivateDirectory {}
impl UnsafePrivateDirectory {
    /// The unsafe folder behind an I/O failure, if that is what it was.
    pub fn of(error: &io::Error) -> Option<&Self> {
        error.get_ref()?.downcast_ref::<Self>()
    }
    pub fn owner_message(&self) -> String {
        format!(
            "Cortex's private folder has unsafe permissions ({}): {}. Make it private to your account (for example chmod 700 on that folder), then try again.",
            self.problem,
            self.path.display()
        )
    }
}
pub fn private_tree(path: &Path) -> io::Result<PathBuf> {
    if !path.exists() {
        let parent = path.parent().ok_or_else(|| failure("missing_parent"))?;
        if !parent.exists() {
            private_tree(parent)?;
        }
    }
    private_directory(path)
}
fn owned_file(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.uid() != uid() || m.mode() & 0o077 != 0 || m.nlink() != 1 {
        return Err(failure("private_file_required"));
    }
    Ok(())
}
pub struct Endpoint {
    pub listener: UnixListener,
    pub path: PathBuf,
    _lock: File,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        // CLOEXEC does not close descriptors inherited by a concurrent fork until
        // exec. Release the shared flock when its owner ends, not that later close.
        unsafe { libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN) };
    }
}
impl Endpoint {
    pub fn bind(runtime: &Path) -> io::Result<Self> {
        let runtime = private_directory(runtime)?;
        let lock_path = runtime.join("service.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)?;
        owned_file(&lock_path)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(failure("service_already_running"));
        }
        let path = runtime.join("service.sock");
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_socket() && m.uid() == uid() => fs::remove_file(&path)?,
            Ok(_) => return Err(failure("unsafe_stale_socket")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        // Address the socket through a pinned directory descriptor: the actual
        // private socket remains at `path`, even when that path exceeds SUN_LEN.
        let dir = File::open(&runtime)?;
        let address = PathBuf::from(format!("/proc/self/fd/{}/service.sock", dir.as_raw_fd()));
        let listener = UnixListener::bind(&address)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
        })
    }
}
#[cfg(test)]
mod socket_tests {
    use super::*;

    #[tokio::test]
    async fn long_runtime_path_binds_and_connects() {
        let root = std::env::temp_dir().join(format!(
            "socket-{}-{}",
            std::process::id(),
            crate::identity().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = root.join("x".repeat(100));
        let endpoint = Endpoint::bind(&runtime).unwrap();
        let client = connect(&endpoint.path).await.unwrap();
        let (_peer, _) = endpoint.listener.accept().await.unwrap();
        drop(client);
        drop(endpoint);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod private_directory_tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "private-dir-{name}-{}-{}",
            std::process::id(),
            crate::identity().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        root
    }
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A settings folder created by `mkdir -p` under a 002 umask (0775, the account's own
    /// group) or a 022 umask (0755) is made private instead of failing every later read.
    #[test]
    fn own_merely_permissive_folders_are_made_private() {
        let root = base("repair");
        for permissive in [0o775, 0o755, 0o750, 0o705] {
            let folder = root.join(format!("config-{permissive:o}"));
            fs::create_dir(&folder).unwrap();
            fs::set_permissions(&folder, fs::Permissions::from_mode(permissive)).unwrap();
            assert_eq!(private_directory(&folder).unwrap(), folder);
            assert_eq!(mode(&folder), 0o700, "{permissive:o}");
            let nested = folder.join("a/b");
            assert_eq!(private_tree(&nested).unwrap(), nested);
            assert_eq!(mode(&nested), 0o700);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A folder anyone could have written into stays refused, and the refusal names it.
    #[test]
    fn world_writable_folders_stay_refused_with_a_named_reason() {
        let root = base("refuse");
        let folder = root.join("config");
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o777)).unwrap();
        let error = private_directory(&folder).unwrap_err();
        assert_eq!(error.to_string(), "private_directory_required");
        let unsafe_folder = UnsafePrivateDirectory::of(&error).expect("named folder");
        assert_eq!(unsafe_folder.path, folder);
        let message = unsafe_folder.owner_message();
        assert!(message.contains("writable by every account"), "{message}");
        assert!(message.contains(&folder.display().to_string()), "{message}");
        assert_eq!(mode(&folder), 0o777, "a refused folder is left as it was");
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

pub async fn connect(path: &Path) -> io::Result<UnixStream> {
    let parent = canonical_directory(path.parent().ok_or_else(|| failure("missing_parent"))?)?;
    let m = fs::metadata(&parent)?;
    let socket = fs::symlink_metadata(path)?;
    if m.uid() != uid()
        || m.mode() & 0o077 != 0
        || socket.uid() != uid()
        || socket.mode() & 0o077 != 0
        || !socket.file_type().is_socket()
    {
        return Err(failure("unsafe_socket"));
    }
    let dir = File::open(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| failure("missing_socket_name"))?;
    let address = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    let stream = UnixStream::connect(address).await?;
    authorize(&stream)?;
    Ok(stream)
}
pub fn authorize(stream: &UnixStream) -> io::Result<()> {
    if stream.peer_cred()?.uid() != uid() {
        return Err(failure("peer_uid_mismatch"));
    }
    Ok(())
}
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Value> {
    let n = reader.read_u32().await? as usize;
    if n == 0 || n > FRAME_BYTES {
        return Err(failure("frame_too_large"));
    }
    let mut bytes = vec![0; n];
    reader.read_exact(&mut bytes).await?;
    let mut frame = (n as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    workspace_protocol::decode_frame(&frame).map_err(|_| failure("invalid_json"))
}
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    let frame = workspace_protocol::encode_frame(value).map_err(|_| failure("invalid_frame"))?;
    writer.write_all(&frame).await
}
pub fn read_registry(path: &Path) -> io::Result<Vec<crate::Project>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
        Ok(_) => owned_file(path)?,
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take((FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > FRAME_BYTES {
        return Err(failure("registry_too_large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| failure("invalid_registry"))
}
pub fn save_registry(path: &Path, projects: &[crate::Project]) -> io::Result<()> {
    let bytes = serde_json::to_vec(projects).map_err(|_| failure("invalid_registry"))?;
    if bytes.len() > FRAME_BYTES {
        return Err(failure("registry_too_large"));
    }
    let temp = path.with_extension("new");
    // The lock winner may clean up only its own private regular crash residue.
    match fs::symlink_metadata(&temp) {
        Ok(_) => {
            owned_file(&temp)?;
            fs::remove_file(&temp)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(path.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
