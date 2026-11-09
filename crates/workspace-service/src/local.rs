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

pub const FRAME_BYTES: usize = 1024 * 1024;
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
        builder.mode(0o700).create(path)?;
    }
    let path = canonical_directory(path)?;
    let meta = fs::metadata(&path)?;
    if meta.uid() != uid() || meta.permissions().mode() & 0o077 != 0 {
        return Err(failure("private_directory_required"));
    }
    Ok(path)
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
        // The parent was private before bind; no access window for another user.
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
        })
    }
}
pub async fn connect(path: &Path) -> io::Result<UnixStream> {
    let parent = canonical_directory(path.parent().ok_or_else(|| failure("missing_parent"))?)?;
    let m = fs::metadata(parent)?;
    let socket = fs::symlink_metadata(path)?;
    if m.uid() != uid()
        || m.mode() & 0o077 != 0
        || socket.uid() != uid()
        || socket.mode() & 0o077 != 0
        || !socket.file_type().is_socket()
    {
        return Err(failure("unsafe_socket"));
    }
    let stream = UnixStream::connect(path).await?;
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
